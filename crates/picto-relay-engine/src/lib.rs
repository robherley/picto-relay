//! Deterministic virtual-client and Nintendo chat protocol state machine.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::{Duration, Instant},
};

use picto_relay_codec::{
    DecodeError, FcsMode, FrameType, MacAddress, MacFrame, ManagementBody, MessageReassembler,
    MultiplayerHeader, NintendoBeacon, NintendoFlow, PictoChatEnvelope, PictoChatFragment,
    ReassemblyKey, SCRIPTED_FRAGMENT_PAYLOAD, SequenceControl, build_fragment_frame,
    decode_nintendo_data, fragment_payload, parse_management,
};
use picto_relay_radio::{ReceivedFrame, TransmitFrame};
use thiserror::Error;

const SCRIPTED_BSSID: MacAddress = MacAddress([0x00, 0x09, 0xbf, 0x52, 0x00, 0x00]);
const DUPLICATE_WINDOW: usize = 256;

/// Stable identity of a connected relay client.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ClientId(pub u32);

/// A command issued by a relay client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientCommand {
    /// Create a virtual DS session.
    Connect { client_id: ClientId, name: String },
    /// Send one complete opaque message payload.
    SendMessage {
        client_id: ClientId,
        payload: Vec<u8>,
    },
    /// Remove a virtual DS session.
    Disconnect { client_id: ClientId },
}

/// An input applied to the engine at a monotonic time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineInput {
    pub at: Instant,
    pub event: EngineEvent,
}

/// Events accepted by the engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineEvent {
    Client(ClientCommand),
    Radio(ReceivedFrame),
    Tick,
}

/// A client-facing event emitted by the engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientEvent {
    Connected,
    MessageReceived {
        station_id: u32,
        message_id: u32,
        payload: Vec<u8>,
    },
    Delivered {
        message_id: u32,
    },
    DeliveryTimedOut {
        message_id: u32,
    },
    Disconnected,
}

/// An effect the application runner must execute.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineEffect {
    Transmit(TransmitFrame),
    Notify {
        client_id: ClientId,
        event: ClientEvent,
    },
    ScheduleWakeup(Instant),
}

/// Protocol engine errors.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum EngineError {
    #[error("client {0:?} is already connected")]
    DuplicateClient(ClientId),
    #[error("client {0:?} is not connected")]
    UnknownClient(ClientId),
    #[error(transparent)]
    Codec(#[from] DecodeError),
}

/// Observed progression of a physical room exchange.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RoomLifecycle {
    #[default]
    Scanning,
    Discovered,
    Authenticated,
    Associated,
}

/// Evidence-backed room fields learned from management traffic.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RoomState {
    pub lifecycle: RoomLifecycle,
    pub host: Option<MacAddress>,
    pub room: Option<u8>,
    pub stream_code: Option<u16>,
    pub advertised_users: Option<u8>,
    pub command_data_size: Option<u16>,
    pub reply_data_size: Option<u16>,
    pub participants: BTreeMap<u8, MacAddress>,
}

#[derive(Clone, Debug)]
struct Session {
    _name: String,
    mac: MacAddress,
    next_message_id: u16,
    next_sequence: u16,
}

#[derive(Clone, Debug)]
struct PendingMessage {
    frames: Vec<TransmitFrame>,
    final_sequence: u16,
    attempts: u8,
    deadline: Instant,
}

/// Configuration for retry behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineConfig {
    pub retry_interval: Duration,
    pub max_attempts: u8,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            retry_interval: Duration::from_millis(250),
            max_attempts: 3,
        }
    }
}

/// Stateful deterministic protocol engine.
#[derive(Debug)]
pub struct Engine {
    config: EngineConfig,
    sessions: BTreeMap<ClientId, Session>,
    pending: BTreeMap<(ClientId, u32), PendingMessage>,
    room: RoomState,
    reassembler: MessageReassembler,
    observed_sequences: BTreeSet<(MacAddress, u16)>,
    sequence_order: VecDeque<(MacAddress, u16)>,
}

impl Engine {
    #[must_use]
    pub fn new(config: EngineConfig) -> Self {
        Self {
            config,
            sessions: BTreeMap::new(),
            pending: BTreeMap::new(),
            room: RoomState::default(),
            reassembler: MessageReassembler::new(SCRIPTED_FRAGMENT_PAYLOAD),
            observed_sequences: BTreeSet::new(),
            sequence_order: VecDeque::new(),
        }
    }

    /// # Errors
    /// Returns an error for an invalid command or malformed recognized frame.
    pub fn handle(&mut self, input: EngineInput) -> Result<Vec<EngineEffect>, EngineError> {
        match input.event {
            EngineEvent::Client(command) => self.handle_client(input.at, command),
            EngineEvent::Radio(frame) => self.handle_radio(&frame),
            EngineEvent::Tick => Ok(self.handle_tick(input.at)),
        }
    }

    #[must_use]
    pub fn contains_client(&self, client_id: ClientId) -> bool {
        self.sessions.contains_key(&client_id)
    }

    #[must_use]
    pub fn client_mac(&self, client_id: ClientId) -> Option<MacAddress> {
        self.sessions.get(&client_id).map(|session| session.mac)
    }

    #[must_use]
    pub const fn room(&self) -> &RoomState {
        &self.room
    }

    fn handle_client(
        &mut self,
        at: Instant,
        command: ClientCommand,
    ) -> Result<Vec<EngineEffect>, EngineError> {
        match command {
            ClientCommand::Connect { client_id, name } => {
                if self.sessions.contains_key(&client_id) {
                    return Err(EngineError::DuplicateClient(client_id));
                }
                self.sessions.insert(
                    client_id,
                    Session {
                        _name: name,
                        mac: virtual_mac(client_id),
                        next_message_id: 0,
                        next_sequence: 0,
                    },
                );
                Ok(vec![EngineEffect::Notify {
                    client_id,
                    event: ClientEvent::Connected,
                }])
            }
            ClientCommand::SendMessage { client_id, payload } => {
                self.send_message(at, client_id, &payload)
            }
            ClientCommand::Disconnect { client_id } => {
                if self.sessions.remove(&client_id).is_none() {
                    return Err(EngineError::UnknownClient(client_id));
                }
                self.pending
                    .retain(|(pending_client, _), _| *pending_client != client_id);
                Ok(vec![EngineEffect::Notify {
                    client_id,
                    event: ClientEvent::Disconnected,
                }])
            }
        }
    }

    fn send_message(
        &mut self,
        at: Instant,
        client_id: ClientId,
        payload: &[u8],
    ) -> Result<Vec<EngineEffect>, EngineError> {
        let session = self
            .sessions
            .get_mut(&client_id)
            .ok_or(EngineError::UnknownClient(client_id))?;
        let wire_id = session.next_message_id;
        session.next_message_id = session.next_message_id.wrapping_add(1);
        let fragments =
            fragment_payload(payload, wire_id.to_le_bytes(), SCRIPTED_FRAGMENT_PAYLOAD)?;
        let mut frames = Vec::with_capacity(fragments.len());
        let bssid = self.room.host.unwrap_or(SCRIPTED_BSSID);
        for fragment in fragments {
            let sequence = session.next_sequence & 0x0fff;
            session.next_sequence = session.next_sequence.wrapping_add(1) & 0x0fff;
            frames.push(TransmitFrame {
                bytes: build_fragment_frame(
                    NintendoFlow::ClientToHost,
                    session.mac,
                    bssid,
                    sequence,
                    false,
                    &fragment,
                )?,
            });
        }
        let final_sequence = frames
            .last()
            .and_then(|frame| MacFrame::parse(&frame.bytes, FcsMode::Absent).ok())
            .and_then(|frame| frame.header.sequence_control)
            .map_or(0, SequenceControl::sequence_number);
        let message_id = u32::from(wire_id);
        let deadline = at + self.config.retry_interval;
        self.pending.insert(
            (client_id, message_id),
            PendingMessage {
                frames: frames.clone(),
                final_sequence,
                attempts: 1,
                deadline,
            },
        );
        let mut effects = frames
            .into_iter()
            .map(EngineEffect::Transmit)
            .collect::<Vec<_>>();
        effects.push(EngineEffect::ScheduleWakeup(deadline));
        Ok(effects)
    }

    fn handle_radio(&mut self, received: &ReceivedFrame) -> Result<Vec<EngineEffect>, EngineError> {
        let Ok(frame) = MacFrame::parse(&received.bytes, FcsMode::Absent) else {
            return Ok(Vec::new());
        };
        match frame.header.frame_control.frame_type() {
            FrameType::Management => {
                self.observe_management(&frame)?;
                Ok(Vec::new())
            }
            FrameType::Data => self.observe_data(&frame),
            FrameType::Control | FrameType::Reserved => Ok(Vec::new()),
        }
    }

    fn observe_management(&mut self, frame: &MacFrame) -> Result<(), EngineError> {
        match parse_management(frame)? {
            ManagementBody::Beacon(beacon) => {
                let nintendo = match NintendoBeacon::from_beacon(&beacon) {
                    Ok(nintendo) => nintendo,
                    Err(DecodeError::MissingNintendoBeacon) => return Ok(()),
                    Err(error) => return Err(error.into()),
                };
                let Some(pictochat) = nintendo.pictochat()? else {
                    return Ok(());
                };
                self.room.lifecycle = RoomLifecycle::Discovered;
                self.room.host = frame.header.address_2;
                self.room.room = Some(pictochat.room);
                self.room.stream_code = Some(nintendo.stream_code);
                self.room.advertised_users = Some(pictochat.user_count);
                self.room.command_data_size = Some(nintendo.command_data_size);
                self.room.reply_data_size = Some(nintendo.reply_data_size);
            }
            ManagementBody::Authentication(authentication) if authentication.status == 0 => {
                self.room.lifecycle = RoomLifecycle::Authenticated;
            }
            ManagementBody::AssociationResponse(response) if response.status == 0 => {
                self.room.lifecycle = RoomLifecycle::Associated;
                self.room
                    .participants
                    .insert(response.participant_slot(), frame.header.address_1);
            }
            ManagementBody::Authentication(_)
            | ManagementBody::AssociationRequest(_)
            | ManagementBody::AssociationResponse(_)
            | ManagementBody::Unknown { .. } => {}
        }
        Ok(())
    }

    fn observe_data(&mut self, frame: &MacFrame) -> Result<Vec<EngineEffect>, EngineError> {
        let data = match decode_nintendo_data(frame) {
            Ok(data) => data,
            Err(DecodeError::NotNintendoDataFrame) => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        if let MultiplayerHeader::HostAcknowledgement {
            command_count,
            failed_client_mask: 0,
        } = data.multiplayer
        {
            return Ok(self.observe_ack(command_count));
        }
        if data.flow == NintendoFlow::HostAcknowledgement
            || !self.remember_sequence(data.source, data.sequence_number)
        {
            return Ok(Vec::new());
        }
        let envelope = PictoChatEnvelope::parse(data.flow, &data.payload)?;
        if !envelope.is_message_fragment() {
            return Ok(Vec::new());
        }
        let fragment = PictoChatFragment::parse(&envelope.payload, data.flow)?;
        let key = ReassemblyKey {
            source: data.source,
            opaque_id: fragment.opaque_id,
        };
        let Some(payload) = self.reassembler.push(key, &fragment)? else {
            return Ok(Vec::new());
        };
        let sender_id = station_id(data.source);
        let message_id = u32::from(u16::from_le_bytes(fragment.opaque_id));
        Ok(self
            .sessions
            .keys()
            .copied()
            .map(|client_id| EngineEffect::Notify {
                client_id,
                event: ClientEvent::MessageReceived {
                    station_id: sender_id,
                    message_id,
                    payload: payload.clone(),
                },
            })
            .collect())
    }

    fn observe_ack(&mut self, sequence: u16) -> Vec<EngineEffect> {
        let acknowledged = self
            .pending
            .iter()
            .find_map(|(&(client_id, message_id), pending)| {
                (pending.final_sequence == sequence).then_some((client_id, message_id))
            });
        let Some((client_id, message_id)) = acknowledged else {
            return Vec::new();
        };
        self.pending.remove(&(client_id, message_id));
        self.sessions
            .contains_key(&client_id)
            .then_some(EngineEffect::Notify {
                client_id,
                event: ClientEvent::Delivered { message_id },
            })
            .into_iter()
            .collect()
    }

    fn remember_sequence(&mut self, source: MacAddress, sequence: u16) -> bool {
        let key = (source, sequence);
        if !self.observed_sequences.insert(key) {
            return false;
        }
        self.sequence_order.push_back(key);
        if self.sequence_order.len() > DUPLICATE_WINDOW {
            if let Some(expired) = self.sequence_order.pop_front() {
                self.observed_sequences.remove(&expired);
            }
        }
        true
    }

    fn handle_tick(&mut self, at: Instant) -> Vec<EngineEffect> {
        let mut effects = Vec::new();
        self.pending.retain(|&(client_id, message_id), pending| {
            if pending.deadline > at {
                return true;
            }
            if pending.attempts < self.config.max_attempts {
                pending.attempts += 1;
                pending.deadline = at + self.config.retry_interval;
                effects.extend(pending.frames.iter().cloned().map(|mut frame| {
                    mark_retry(&mut frame.bytes);
                    EngineEffect::Transmit(frame)
                }));
                effects.push(EngineEffect::ScheduleWakeup(pending.deadline));
                true
            } else {
                effects.push(EngineEffect::Notify {
                    client_id,
                    event: ClientEvent::DeliveryTimedOut { message_id },
                });
                false
            }
        });
        effects
    }
}

/// Deterministic locally administered address retaining the client identifier.
#[must_use]
pub const fn virtual_mac(client_id: ClientId) -> MacAddress {
    let id = client_id.0.to_be_bytes();
    MacAddress([0x02, 0x09, id[0], id[1], id[2], id[3]])
}

/// Converts the final four MAC octets into a station identifier.
#[must_use]
pub const fn station_id(mac: MacAddress) -> u32 {
    u32::from_be_bytes([mac.0[2], mac.0[3], mac.0[4], mac.0[5]])
}

fn mark_retry(frame: &mut [u8]) {
    if let Some(flags) = frame.get_mut(1) {
        *flags |= 0x08;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use picto_relay_codec::build_management_frame;

    fn radio_input(now: Instant, bytes: Vec<u8>) -> EngineInput {
        EngineInput {
            at: now,
            event: EngineEvent::Radio(ReceivedFrame {
                received_at: now,
                tsft_us: None,
                bytes,
                rssi_dbm: Some(-40),
                antenna: None,
            }),
        }
    }

    fn decode_hex(text: &str) -> Vec<u8> {
        text.split_ascii_whitespace()
            .map(|byte| u8::from_str_radix(byte, 16).expect("fixture must contain valid hex"))
            .collect()
    }

    #[test]
    fn management_exchange_should_track_room_and_participant_slot() {
        let now = Instant::now();
        let beacon = decode_hex(include_str!(
            "../../picto-relay-codec/tests/fixtures/pictochat-room-a-beacon.hex"
        ));
        let mut engine = Engine::new(EngineConfig::default());
        engine
            .handle(radio_input(now, beacon))
            .expect("captured beacon should be accepted");
        assert_eq!(engine.room().lifecycle, RoomLifecycle::Discovered);
        assert_eq!(engine.room().room, Some(0));
        assert_eq!(engine.room().advertised_users, Some(1));

        let host = engine.room().host.expect("beacon should identify its host");
        let station = virtual_mac(ClientId(7));
        let authentication =
            build_management_frame(11, station, host, host, 1, &[0, 0, 2, 0, 0, 0]);
        engine
            .handle(radio_input(now, authentication))
            .expect("authentication response should be accepted");
        assert_eq!(engine.room().lifecycle, RoomLifecycle::Authenticated);

        let association =
            build_management_frame(1, station, host, host, 2, &[0x21, 0, 0, 0, 3, 0xc0]);
        engine
            .handle(radio_input(now, association))
            .expect("association response should be accepted");
        assert_eq!(engine.room().lifecycle, RoomLifecycle::Associated);
        assert_eq!(engine.room().participants.get(&3), Some(&station));
    }

    #[test]
    fn unacknowledged_message_should_retry_then_time_out() {
        let now = Instant::now();
        let retry_interval = Duration::from_millis(10);
        let alice = ClientId(1);
        let mut engine = Engine::new(EngineConfig {
            retry_interval,
            max_attempts: 2,
        });
        engine
            .handle(EngineInput {
                at: now,
                event: EngineEvent::Client(ClientCommand::Connect {
                    client_id: alice,
                    name: "Alice".into(),
                }),
            })
            .expect("client should connect");
        engine
            .handle(EngineInput {
                at: now,
                event: EngineEvent::Client(ClientCommand::SendMessage {
                    client_id: alice,
                    payload: b"ping".to_vec(),
                }),
            })
            .expect("message should transmit");

        let retry = engine
            .handle(EngineInput {
                at: now + retry_interval,
                event: EngineEvent::Tick,
            })
            .expect("first deadline should retry");
        let retry_frame = retry
            .iter()
            .find_map(|effect| match effect {
                EngineEffect::Transmit(frame) => Some(frame),
                EngineEffect::Notify { .. } | EngineEffect::ScheduleWakeup(_) => None,
            })
            .expect("retry should transmit");
        assert!(
            MacFrame::parse(&retry_frame.bytes, FcsMode::Absent)
                .expect("retry frame should parse")
                .header
                .frame_control
                .retry()
        );

        let timeout = engine
            .handle(EngineInput {
                at: now + retry_interval + retry_interval,
                event: EngineEvent::Tick,
            })
            .expect("second deadline should time out");
        assert_eq!(
            timeout,
            vec![EngineEffect::Notify {
                client_id: alice,
                event: ClientEvent::DeliveryTimedOut { message_id: 0 },
            }]
        );
    }
}
