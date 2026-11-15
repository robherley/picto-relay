//! Bounded async broker for API-owned virtual stations.

use std::{collections::BTreeMap, sync::mpsc as std_mpsc, thread, time::Duration};

use crate::worker::{Esp32Config, LiveCommand, LiveEvent};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, warn};

use crate::protocol::{
    BITMAP_BYTES, PALETTE_BITMAP_BYTES, Participant, ROOM_COUNT, RoomSummary, SessionEvent,
    SessionUser,
};

pub use crate::worker::DEFAULT_SERIAL_PORT;

// --- Queue depths --------------------------------------------------------------

/// Commands the broker actor queues from HTTP tasks before reporting busy.
const COMMAND_CAPACITY: usize = 64;

/// Events queued per API session before a slow session is dropped.
const CLIENT_EVENT_CAPACITY: usize = 32;

/// Commands queued for the physical radio worker thread before reporting busy.
const HARDWARE_COMMAND_CAPACITY: usize = 8;

// --- Identifiers ---------------------------------------------------------------

/// First session identifier handed out by a broker; zero is never assigned.
const FIRST_CLIENT_ID: u32 = 1;

/// First message identifier assigned within a session.
const FIRST_MESSAGE_ID: u32 = 0;

// --- Rooms and passive discovery -----------------------------------------------

/// Number of `PictoChat` rooms, as an array length.
const ROOM_SLOTS: usize = ROOM_COUNT as usize;

/// ASCII letter of room 0; later rooms follow alphabetically.
const FIRST_ROOM_LETTER: u8 = b'A';

/// Member count reported by the radio for a room whose beacon was not heard.
const ROOM_UNOBSERVED: u8 = picto_relay_link::NO_BEACON;

/// Discovery result when no room has been observed.
const NO_ROOMS_OBSERVED: [u8; ROOM_SLOTS] = [ROOM_UNOBSERVED; ROOM_SLOTS];

/// Interval between passive room scans while no session owns the radio.
const DISCOVERY_INTERVAL: Duration = Duration::from_secs(5);

/// Name of the thread that drives the ESP32 serial link.
const HARDWARE_THREAD_NAME: &str = "picto-relay-esp32";

/// Stable identity assigned to one API session.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ClientId(pub u32);

/// Handle used by HTTP tasks to communicate with the broker actor.
#[derive(Clone, Debug)]
pub struct BrokerHandle {
    commands: mpsc::Sender<BrokerCommand>,
    rooms: watch::Receiver<Vec<RoomSummary>>,
}

/// An API session registered with the broker.
#[derive(Debug)]
pub struct RelaySession {
    /// Assigned virtual station identity.
    pub client_id: ClientId,
    /// Bounded stream of events for this session.
    pub events: mpsc::Receiver<SessionEvent>,
}

/// What the broker knows about one session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionInfo {
    /// Room the session is in.
    pub room: u8,
    /// Everyone seen in the room since the session began, in the order they
    /// were first seen.
    pub users: Vec<SessionUser>,
}

/// Failures returned to an API task.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum BrokerError {
    /// The single broker actor has stopped.
    #[error("broker is unavailable")]
    Unavailable,
    /// The client's command queue is full.
    #[error("broker command queue is full")]
    Busy,
    /// A bitmap does not have the required packed size.
    #[error(
        "bitmap must contain {BITMAP_BYTES} monochrome or {PALETTE_BITMAP_BYTES} palette bytes, received {0}"
    )]
    InvalidBitmapSize(usize),
    /// A room outside A through D was requested.
    #[error("room must be between 0 (A) and 3 (D), received {0}")]
    InvalidRoom(u8),
}

impl BrokerHandle {
    /// Starts the in-memory development broker.
    ///
    /// This backend exercises the JSON session contract only. It does not
    /// simulate Nintendo wireless delivery or claim protocol compatibility.
    #[must_use]
    pub fn spawn_memory() -> Self {
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (room_updates, rooms) = watch::channel(empty_room_summaries());
        tokio::spawn(Broker::new(receiver, room_updates).run());
        Self { commands, rooms }
    }

    /// Starts a single-station ESP32 USB backend. With `capture`, every radio
    /// frame the ESP32 hears or sends during a session is appended to that
    /// pcap file.
    #[must_use]
    pub fn spawn_esp32(port: String, capture: Option<std::path::PathBuf>) -> Self {
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (room_updates, rooms) = watch::channel(empty_room_summaries());
        let broker = Esp32Broker::new(
            Esp32Config {
                port,
                capture,
                ..Esp32Config::default()
            },
            receiver,
            room_updates,
        );
        tokio::spawn(broker.run());
        Self { commands, rooms }
    }

    /// Registers a caller and allocates its virtual station.
    ///
    /// # Errors
    ///
    /// Returns [`BrokerError`] if the actor is busy or unavailable.
    pub async fn register(&self, name: String, room: u8) -> Result<RelaySession, BrokerError> {
        self.register_with_profile(name, room, String::new(), 0)
            .await
    }

    /// Registers a station with a personal profile message.
    /// # Errors
    /// Returns an error when the broker is busy or unavailable.
    pub async fn register_with_profile(
        &self,
        name: String,
        room: u8,
        profile_message: String,
        profile_color: u8,
    ) -> Result<RelaySession, BrokerError> {
        if room >= ROOM_COUNT {
            return Err(BrokerError::InvalidRoom(room));
        }
        let (reply, response) = oneshot::channel();
        self.commands
            .try_send(BrokerCommand::Register {
                name,
                room,
                profile_message,
                profile_color,
                reply,
            })
            .map_err(|error| map_send_error(&error))?;
        response.await.map_err(|_| BrokerError::Unavailable)?
    }

    /// Returns the participant rosters known to the selected backend.
    ///
    /// # Errors
    ///
    /// Returns [`BrokerError`] if the actor is busy or unavailable.
    pub async fn rooms(&self) -> Result<Vec<RoomSummary>, BrokerError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .try_send(BrokerCommand::Rooms { reply })
            .map_err(|error| map_send_error(&error))?;
        response.await.map_err(|_| BrokerError::Unavailable)
    }

    /// Describes a session, or `None` if the broker has no such session.
    ///
    /// # Errors
    ///
    /// Returns [`BrokerError`] if the actor is busy or unavailable.
    pub async fn inspect(&self, client_id: ClientId) -> Result<Option<SessionInfo>, BrokerError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .try_send(BrokerCommand::Inspect { client_id, reply })
            .map_err(|error| map_send_error(&error))?;
        response.await.map_err(|_| BrokerError::Unavailable)
    }

    /// Subscribes to complete room snapshots, including the current value.
    #[must_use]
    pub fn subscribe_rooms(&self) -> watch::Receiver<Vec<RoomSummary>> {
        self.rooms.clone()
    }

    /// Submits one packed monochrome message.
    ///
    /// # Errors
    ///
    /// Returns [`BrokerError`] for an invalid bitmap or unavailable actor.
    pub fn send_message(&self, client_id: ClientId, bitmap: Vec<u8>) -> Result<(), BrokerError> {
        if bitmap.len() != BITMAP_BYTES && bitmap.len() != PALETTE_BITMAP_BYTES {
            return Err(BrokerError::InvalidBitmapSize(bitmap.len()));
        }
        self.commands
            .try_send(BrokerCommand::SendMessage { client_id, bitmap })
            .map_err(|error| map_send_error(&error))
    }

    /// Requests removal of a relay session.
    pub fn unregister(&self, client_id: ClientId) {
        if let Err(error) = self
            .commands
            .try_send(BrokerCommand::Unregister { client_id })
        {
            debug!(?error, ?client_id, "could not enqueue client disconnect");
        }
    }
}

fn map_send_error<T>(error: &mpsc::error::TrySendError<T>) -> BrokerError {
    match error {
        mpsc::error::TrySendError::Full(_) => BrokerError::Busy,
        mpsc::error::TrySendError::Closed(_) => BrokerError::Unavailable,
    }
}

#[derive(Debug)]
enum BrokerCommand {
    Register {
        name: String,
        profile_message: String,
        profile_color: u8,
        room: u8,
        reply: oneshot::Sender<Result<RelaySession, BrokerError>>,
    },
    Rooms {
        reply: oneshot::Sender<Vec<RoomSummary>>,
    },
    Inspect {
        client_id: ClientId,
        reply: oneshot::Sender<Option<SessionInfo>>,
    },
    SendMessage {
        client_id: ClientId,
        bitmap: Vec<u8>,
    },
    Unregister {
        client_id: ClientId,
    },
}

#[derive(Debug)]
struct ConnectedClient {
    name: String,
    room: u8,
    events: mpsc::Sender<SessionEvent>,
    next_message_id: u32,
    /// Everyone this session has seen in its room, present or not.
    seen: Vec<SessionUser>,
}

/// Brings a session's record of its room up to date with who is there now:
/// newcomers are added, and anyone no longer present is marked inactive.
fn record_presence(seen: &mut Vec<SessionUser>, present: &[Participant]) {
    for user in seen.iter_mut() {
        user.active = present
            .iter()
            .any(|participant| participant.client_id == user.client_id);
    }
    for participant in present {
        match seen
            .iter_mut()
            .find(|user| user.client_id == participant.client_id)
        {
            Some(user) => user.name.clone_from(&participant.name),
            None => seen.push(SessionUser {
                client_id: participant.client_id,
                name: participant.name.clone(),
                active: true,
            }),
        }
    }
}

struct Broker {
    commands: mpsc::Receiver<BrokerCommand>,
    room_updates: watch::Sender<Vec<RoomSummary>>,
    clients: BTreeMap<ClientId, ConnectedClient>,
    next_client_id: u32,
}

impl Broker {
    fn new(
        commands: mpsc::Receiver<BrokerCommand>,
        room_updates: watch::Sender<Vec<RoomSummary>>,
    ) -> Self {
        Self {
            commands,
            room_updates,
            clients: BTreeMap::new(),
            next_client_id: FIRST_CLIENT_ID,
        }
    }

    async fn run(mut self) {
        while let Some(command) = self.commands.recv().await {
            self.handle(command);
        }
    }

    fn handle(&mut self, command: BrokerCommand) {
        match command {
            BrokerCommand::Register {
                name,
                room,
                profile_message: _,
                profile_color: _,
                reply,
            } => self.register(name, room, reply),
            BrokerCommand::Rooms { reply } => {
                let _ = reply.send(room_summaries(self.clients.iter()));
            }
            BrokerCommand::Inspect { client_id, reply } => {
                let _ = reply.send(self.clients.get(&client_id).map(|client| SessionInfo {
                    room: client.room,
                    users: client.seen.clone(),
                }));
            }
            BrokerCommand::SendMessage { client_id, bitmap } => {
                self.send_message(client_id, &bitmap);
            }
            BrokerCommand::Unregister { client_id } => self.unregister(client_id),
        }
    }

    fn register(
        &mut self,
        name: String,
        room: u8,
        reply: oneshot::Sender<Result<RelaySession, BrokerError>>,
    ) {
        let client_id = ClientId(self.next_client_id);
        self.next_client_id = self.next_client_id.wrapping_add(1).max(FIRST_CLIENT_ID);
        let (events, receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let joined = Participant {
            client_id: client_id.0,
            name: name.clone(),
        };
        self.clients.insert(
            client_id,
            ConnectedClient {
                name,
                room,
                events: events.clone(),
                next_message_id: FIRST_MESSAGE_ID,
                seen: Vec::new(),
            },
        );
        self.send_to(
            client_id,
            SessionEvent::SessionReady {
                client_id: client_id.0,
            },
        );
        self.broadcast_to_room(
            room,
            &SessionEvent::ParticipantJoined {
                room,
                participant: joined,
            },
        );
        self.broadcast_participants(room);

        if reply
            .send(Ok(RelaySession {
                client_id,
                events: receiver,
            }))
            .is_err()
        {
            self.unregister(client_id);
        }
    }

    fn send_message(&mut self, client_id: ClientId, bitmap: &[u8]) {
        let Some(sender) = self.clients.get_mut(&client_id) else {
            return;
        };
        let message_id = sender.next_message_id;
        sender.next_message_id = sender.next_message_id.wrapping_add(1);
        let sender_name = sender.name.clone();
        let room = sender.room;
        let message = SessionEvent::Message {
            station_id: client_id.0,
            name: sender_name,
            message_id,
            palette_bitmap: STANDARD.encode(crate::bitmap::palette_bitmap(bitmap)),
        };
        let previous_clients = self.clients.len();
        self.clients.retain(|_, client| {
            client.room != room || client.events.try_send(message.clone()).is_ok()
        });
        if self.clients.len() != previous_clients {
            self.publish_rooms();
        }
        self.send_to(client_id, SessionEvent::Delivered { message_id });
    }

    fn unregister(&mut self, client_id: ClientId) {
        let Some(client) = self.clients.remove(&client_id) else {
            return;
        };
        self.broadcast_to_room(
            client.room,
            &SessionEvent::ParticipantLeft {
                room: client.room,
                participant: Participant {
                    client_id: client_id.0,
                    name: client.name,
                },
            },
        );
        self.broadcast_participants(client.room);
    }

    fn broadcast_to_room(&mut self, room: u8, event: &SessionEvent) {
        self.clients.retain(|_, client| {
            client.room != room || client.events.try_send(event.clone()).is_ok()
        });
    }

    fn broadcast_participants(&mut self, room: u8) {
        let participants = self
            .clients
            .iter()
            .filter(|(_, client)| client.room == room)
            .map(participant)
            .collect::<Vec<_>>();
        for client in self
            .clients
            .values_mut()
            .filter(|client| client.room == room)
        {
            record_presence(&mut client.seen, &participants);
        }
        let event = SessionEvent::Participants { room, participants };
        self.clients.retain(|_, client| {
            client.room != room || client.events.try_send(event.clone()).is_ok()
        });
        self.publish_rooms();
    }

    fn send_to(&mut self, client_id: ClientId, event: SessionEvent) {
        let should_remove = self
            .clients
            .get(&client_id)
            .is_some_and(|client| client.events.try_send(event).is_err());
        if should_remove {
            self.clients.remove(&client_id);
            self.publish_rooms();
        }
    }

    fn publish_rooms(&self) {
        publish_rooms_if_changed(&self.room_updates, room_summaries(self.clients.iter()));
    }
}

fn participant((&client_id, client): (&ClientId, &ConnectedClient)) -> Participant {
    Participant {
        client_id: client_id.0,
        name: client.name.clone(),
    }
}

fn room_summaries<'a>(
    clients: impl Iterator<Item = (&'a ClientId, &'a ConnectedClient)>,
) -> Vec<RoomSummary> {
    let mut rooms = empty_room_summaries();
    for (client_id, client) in clients {
        let room = &mut rooms[usize::from(client.room)];
        room.participants.push(participant((client_id, client)));
        room.participant_count = room.participants.len();
    }
    rooms
}

fn empty_room_summaries() -> Vec<RoomSummary> {
    (0..ROOM_COUNT)
        .map(|room| RoomSummary {
            observed: true,
            room,
            participant_count: 0,
            participants: Vec::new(),
        })
        .collect()
}

fn publish_rooms_if_changed(updates: &watch::Sender<Vec<RoomSummary>>, rooms: Vec<RoomSummary>) {
    let changed = *updates.borrow() != rooms;
    if changed {
        updates.send_replace(rooms);
    }
}

struct Esp32Broker {
    config: Esp32Config,
    commands: mpsc::Receiver<BrokerCommand>,
    room_updates: watch::Sender<Vec<RoomSummary>>,
    hardware_events: mpsc::UnboundedReceiver<LiveEvent>,
    hardware_event_sender: mpsc::UnboundedSender<LiveEvent>,
    hardware_commands: Option<std_mpsc::SyncSender<LiveCommand>>,
    hardware_running: bool,
    client: Option<(ClientId, ConnectedClient)>,
    participants: Vec<Participant>,
    roster_initialized: bool,
    discovered: [u8; ROOM_SLOTS],
    next_message_id: u32,
    next_client_id: Option<u32>,
}

impl Esp32Broker {
    fn new(
        config: Esp32Config,
        commands: mpsc::Receiver<BrokerCommand>,
        room_updates: watch::Sender<Vec<RoomSummary>>,
    ) -> Self {
        let (hardware_event_sender, hardware_events) = mpsc::unbounded_channel();
        Self {
            config,
            commands,
            room_updates,
            hardware_events,
            hardware_event_sender,
            hardware_commands: None,
            hardware_running: false,
            client: None,
            participants: Vec::new(),
            roster_initialized: false,
            discovered: NO_ROOMS_OBSERVED,
            next_message_id: FIRST_MESSAGE_ID,
            next_client_id: Some(FIRST_CLIENT_ID),
        }
    }

    async fn run(mut self) {
        let mut discovery_tick = tokio::time::interval(DISCOVERY_INTERVAL);
        discovery_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = discovery_tick.tick(), if !self.hardware_running && self.client.is_none() => {
                    let config = self.config.clone();
                    // Await completion before accepting a join: serial ownership never overlaps.
                    let result = tokio::task::spawn_blocking(move || crate::esp32::discover(&config.port)).await;
                    self.discovered = match result {
                        Ok(Ok(counts)) => counts,
                        error => { debug!(?error, "passive discovery unavailable"); NO_ROOMS_OBSERVED }
                    };
                    self.publish_rooms();
                }

                command = self.commands.recv() => {
                    let Some(command) = command else {
                        self.shutdown_hardware();
                        break;
                    };
                    self.handle_command(command);
                }
                event = self.hardware_events.recv() => {
                    let Some(event) = event else {
                        break;
                    };
                    self.handle_hardware_event(event);
                }
            }
        }
    }

    fn handle_command(&mut self, command: BrokerCommand) {
        match command {
            BrokerCommand::Register {
                name,
                room,
                profile_message,
                profile_color,
                reply,
            } => self.register(&name, room, profile_message, profile_color, reply),
            BrokerCommand::Rooms { reply } => {
                let _ = reply.send(self.room_summaries());
            }
            BrokerCommand::Inspect { client_id, reply } => {
                let info =
                    self.client
                        .as_ref()
                        .filter(|(id, _)| *id == client_id)
                        .map(|(_, client)| SessionInfo {
                            room: client.room,
                            users: client.seen.clone(),
                        });
                let _ = reply.send(info);
            }
            BrokerCommand::SendMessage { client_id, bitmap } => {
                self.send_message(client_id, bitmap);
            }
            BrokerCommand::Unregister { client_id } => self.unregister(client_id),
        }
    }

    fn allocate_client_id(&mut self) -> Option<ClientId> {
        let id = self.next_client_id?;
        // Never reuse an ID while an HTTP request or WebSocket can still refer to it.
        self.next_client_id = id.checked_add(1);
        Some(ClientId(id))
    }

    fn register(
        &mut self,
        name: &str,
        room: u8,
        profile_message: String,
        profile_color: u8,
        reply: oneshot::Sender<Result<RelaySession, BrokerError>>,
    ) {
        if self.client.is_some() || self.hardware_running {
            let _ = reply.send(Err(BrokerError::Busy));
            return;
        }
        let Some(client_id) = self.allocate_client_id() else {
            let _ = reply.send(Err(BrokerError::Unavailable));
            return;
        };
        let (events, receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        self.client = Some((
            client_id,
            ConnectedClient {
                name: name.to_owned(),
                room,
                events,
                next_message_id: FIRST_MESSAGE_ID,
                seen: Vec::new(),
            },
        ));
        let (hardware_commands, hardware_receiver) =
            std_mpsc::sync_channel(HARDWARE_COMMAND_CAPACITY);
        self.hardware_commands = Some(hardware_commands);
        self.hardware_running = true;
        self.next_message_id = FIRST_MESSAGE_ID;
        self.roster_initialized = false;
        let joined = Participant {
            client_id: station_id(self.config.station),
            name: name.to_owned(),
        };
        self.participants = vec![joined.clone()];
        self.record_presence();
        let config = self.config.clone();
        let hardware_events = self.hardware_event_sender.clone();
        let hardware_name = name.to_owned();
        let spawn_result = thread::Builder::new()
            .name(HARDWARE_THREAD_NAME.into())
            .spawn(move || {
                let emit = |event| {
                    let _ = hardware_events.send(event);
                };
                let result = crate::esp32::run(
                    &config.port,
                    config.capture.as_deref(),
                    room,
                    &hardware_name,
                    &crate::worker::ProfileOptions {
                        message: profile_message,
                        color: profile_color,
                    },
                    &hardware_receiver,
                    &emit,
                );
                if let Err(error) = result {
                    emit(LiveEvent::Error {
                        code: "hardware_failed",
                        message: error.to_string(),
                    });
                    emit(LiveEvent::Left);
                }
            });
        if let Err(error) = spawn_result {
            self.hardware_running = false;
            self.hardware_commands = None;
            self.client = None;
            self.participants.clear();
            let _ = reply.send(Err(BrokerError::Unavailable));
            warn!(%error, "failed to start physical radio thread");
            return;
        }
        self.send_to(SessionEvent::Participants {
            room,
            participants: self.participants.clone(),
        });
        self.publish_rooms();
        let _ = reply.send(Ok(RelaySession {
            client_id,
            events: receiver,
        }));
    }

    fn send_message(&mut self, client_id: ClientId, bitmap: Vec<u8>) {
        if self.client.as_ref().map(|(id, _)| *id) != Some(client_id) {
            return;
        }
        let message_id = self.next_message_id;
        self.next_message_id = self.next_message_id.wrapping_add(1);
        let result = self
            .hardware_commands
            .as_ref()
            .ok_or("physical radio is not running")
            .and_then(|sender| {
                sender
                    .try_send(LiveCommand::SendMessage { message_id, bitmap })
                    .map_err(|error| match error {
                        std_mpsc::TrySendError::Full(_) => "physical radio command queue is full",
                        std_mpsc::TrySendError::Disconnected(_) => {
                            "physical radio command channel closed"
                        }
                    })
            });
        if let Err(message) = result {
            self.send_to(SessionEvent::Error {
                code: "hardware_unavailable",
                message: message.into(),
            });
            self.send_to(SessionEvent::DeliveryTimedOut { message_id });
        }
    }

    fn unregister(&mut self, client_id: ClientId) {
        if self.client.as_ref().map(|(id, _)| *id) != Some(client_id) {
            return;
        }
        self.client = None;
        self.participants.clear();
        self.roster_initialized = false;
        self.shutdown_hardware();
        self.publish_rooms();
    }

    fn shutdown_hardware(&mut self) {
        if let Some(sender) = self.hardware_commands.take() {
            let _ = sender.try_send(LiveCommand::Shutdown);
        }
    }

    fn update_station(&mut self, station: picto_relay_codec::MacAddress) {
        let previous = station_id(self.config.station);
        self.config.station = station;
        for p in &mut self.participants {
            if p.client_id == previous {
                p.client_id = station_id(station);
            }
        }
        if let Some((_, client)) = &mut self.client {
            for user in &mut client.seen {
                if user.client_id == previous {
                    user.client_id = station_id(station);
                }
            }
        }
        self.publish_rooms();
    }

    fn handle_hardware_event(&mut self, event: LiveEvent) {
        let wire_event = match event {
            LiveEvent::Profile(profile) => SessionEvent::Profile {
                station_id: station_id(profile.station),
                name: profile.nickname,
                profile_message: profile.message,
                profile_color: profile.color,
            },
            LiveEvent::Station { station } => {
                self.update_station(station);
                return;
            }
            LiveEvent::State { state, detail } => SessionEvent::BackendState { state, detail },
            LiveEvent::Ready { host, room, slot } => {
                let detail = format!(
                    "joined Room {} hosted by {host} in slot {slot}",
                    char::from(FIRST_ROOM_LETTER + room)
                );
                self.send_to(SessionEvent::BackendState {
                    state: "ready",
                    detail,
                });
                let joined = self
                    .participants
                    .iter()
                    .find(|participant| participant.client_id == station_id(self.config.station))
                    .cloned();
                if let Some(participant) = joined {
                    self.send_to(SessionEvent::ParticipantJoined { room, participant });
                }
                if self.client.is_none() {
                    return;
                }
                // The identity the session has in rosters and on its messages.
                SessionEvent::SessionReady {
                    client_id: station_id(self.config.station),
                }
            }
            LiveEvent::Participants { room, participants } => {
                let participants = participants
                    .into_iter()
                    .map(|participant| Participant {
                        client_id: station_id(participant.station),
                        name: participant.name,
                    })
                    .collect::<Vec<_>>();
                if self.roster_initialized {
                    self.send_participant_changes(room, &participants);
                } else {
                    self.roster_initialized = true;
                }
                self.participants = participants;
                self.record_presence();
                SessionEvent::Participants {
                    room,
                    participants: self.participants.clone(),
                }
            }
            LiveEvent::Message {
                station_id,
                name,
                message_id,
                bitmap,
            } => SessionEvent::Message {
                station_id,
                name,
                message_id,
                palette_bitmap: STANDARD.encode(crate::bitmap::palette_bitmap(&bitmap)),
            },
            LiveEvent::Delivered { message_id } => SessionEvent::Delivered { message_id },
            LiveEvent::DeliveryTimedOut { message_id } => {
                SessionEvent::DeliveryTimedOut { message_id }
            }
            LiveEvent::Error { code, message } => SessionEvent::Error { code, message },
            LiveEvent::Left => {
                self.hardware_running = false;
                self.hardware_commands = None;
                self.roster_initialized = false;
                let room = self.client.as_ref().map(|(_, client)| client.room);
                let participants = std::mem::take(&mut self.participants);
                self.record_presence();
                if let Some(room) = room {
                    for participant in participants {
                        self.send_to(SessionEvent::ParticipantLeft { room, participant });
                    }
                    self.send_to(SessionEvent::Participants {
                        room,
                        participants: Vec::new(),
                    });
                }
                SessionEvent::BackendState {
                    state: "left",
                    detail: "physical station left the PictoChat room".into(),
                }
            }
        };
        self.send_to(wire_event);
        self.publish_rooms();
    }

    /// Brings the session's record of its room up to date with the roster.
    fn record_presence(&mut self) {
        if let Some((_, client)) = &mut self.client {
            record_presence(&mut client.seen, &self.participants);
        }
    }

    fn send_participant_changes(&mut self, room: u8, participants: &[Participant]) {
        let left = self
            .participants
            .iter()
            .filter(|current| {
                !participants
                    .iter()
                    .any(|next| next.client_id == current.client_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        let joined = participants
            .iter()
            .filter(|next| {
                !self
                    .participants
                    .iter()
                    .any(|current| current.client_id == next.client_id)
            })
            .cloned()
            .collect::<Vec<_>>();

        for participant in left {
            self.send_to(SessionEvent::ParticipantLeft { room, participant });
        }
        for participant in joined {
            self.send_to(SessionEvent::ParticipantJoined { room, participant });
        }
    }

    fn send_to(&mut self, event: SessionEvent) {
        let should_remove = self
            .client
            .as_ref()
            .is_some_and(|(_, client)| client.events.try_send(event).is_err());
        if should_remove {
            self.client = None;
            self.shutdown_hardware();
        }
    }

    fn room_summaries(&self) -> Vec<RoomSummary> {
        let mut rooms = empty_room_summaries();
        for (room, count) in rooms.iter_mut().zip(self.discovered) {
            room.observed = count != ROOM_UNOBSERVED && !self.hardware_running;
            room.participant_count = if room.observed { usize::from(count) } else { 0 };
        }
        if let Some((_, client)) = &self.client {
            let room = &mut rooms[usize::from(client.room)];
            room.observed = self.roster_initialized;
            room.participants.clone_from(&self.participants);
            room.participant_count = room.participants.len();
        }
        rooms
    }

    fn publish_rooms(&self) {
        publish_rooms_if_changed(&self.room_updates, self.room_summaries());
    }
}

/// Derives a participant identifier from the low four octets of a MAC,
/// skipping the two leading vendor octets.
pub(crate) fn station_id(mac: picto_relay_codec::MacAddress) -> u32 {
    let [_, _, device_octets @ ..] = mac.octets();
    u32::from_be_bytes(device_octets)
}

#[cfg(test)]
mod tests {
    use crate::worker::{Esp32Config, LiveEvent, LiveParticipant};
    use picto_relay_codec::MacAddress;

    use super::{
        BITMAP_BYTES, BrokerHandle, CLIENT_EVENT_CAPACITY, ClientId, ConnectedClient, Esp32Broker,
    };
    use crate::protocol::SessionEvent;

    #[tokio::test]
    async fn broker_should_deliver_message_to_two_virtual_clients() {
        let broker = BrokerHandle::spawn_memory();
        let mut alice = broker
            .register("Alice".into(), 0)
            .await
            .expect("Alice should register");
        let mut bob = broker
            .register("Bob".into(), 0)
            .await
            .expect("Bob should register");
        drain(&mut alice.events);
        drain(&mut bob.events);

        broker
            .send_message(alice.client_id, vec![0x55; BITMAP_BYTES])
            .expect("bitmap should enqueue");

        let alice_message = receive_message(&mut alice.events).await;
        let bob_message = receive_message(&mut bob.events).await;
        assert!(matches!(
            alice_message,
            SessionEvent::Message { station_id: 1, .. }
        ));
        assert!(matches!(
            bob_message,
            SessionEvent::Message { station_id: 1, .. }
        ));
    }

    #[tokio::test]
    async fn rooms_should_group_clients_by_selected_room() {
        let broker = BrokerHandle::spawn_memory();
        let _alice = broker
            .register("Alice".into(), 0)
            .await
            .expect("Alice should register");
        let _bob = broker
            .register("Bob".into(), 2)
            .await
            .expect("Bob should register");

        let rooms = broker.rooms().await.expect("rooms should be available");

        assert_eq!(rooms[0].participants[0].name, "Alice");
        assert_eq!(rooms[2].participants[0].name, "Bob");
    }

    #[tokio::test]
    async fn rooms_should_count_memory_clients() {
        let broker = BrokerHandle::spawn_memory();
        let _alice = broker
            .register("Alice".into(), 0)
            .await
            .expect("Alice should register");

        let rooms = broker.rooms().await.expect("rooms should be available");

        assert_eq!(rooms[0].participant_count, 1);
    }

    #[tokio::test]
    async fn room_subscription_should_publish_complete_snapshots() {
        let broker = BrokerHandle::spawn_memory();
        let mut rooms = broker.subscribe_rooms();

        let _alice = broker
            .register("Alice".into(), 1)
            .await
            .expect("Alice should register");
        rooms
            .changed()
            .await
            .expect("broker should remain available");

        assert_eq!(rooms.borrow()[1].participants[0].name, "Alice");
    }

    #[tokio::test]
    async fn register_should_publish_join_event_to_room_members() {
        let broker = BrokerHandle::spawn_memory();
        let mut alice = broker
            .register("Alice".into(), 0)
            .await
            .expect("Alice should register");
        drain(&mut alice.events);

        let _bob = broker
            .register("Bob".into(), 0)
            .await
            .expect("Bob should register");
        let event = alice.events.recv().await.expect("join should be published");

        assert!(matches!(
            event,
            SessionEvent::ParticipantJoined { participant, .. }
                if participant.name == "Bob"
        ));
    }

    #[tokio::test]
    async fn unregister_should_publish_leave_event_to_room_members() {
        let broker = BrokerHandle::spawn_memory();
        let mut alice = broker
            .register("Alice".into(), 0)
            .await
            .expect("Alice should register");
        let bob = broker
            .register("Bob".into(), 0)
            .await
            .expect("Bob should register");
        drain(&mut alice.events);

        broker.unregister(bob.client_id);
        let event = alice
            .events
            .recv()
            .await
            .expect("leave should be published");

        assert!(matches!(
            event,
            SessionEvent::ParticipantLeft { participant, .. }
                if participant.name == "Bob"
        ));
    }

    #[tokio::test]
    async fn broker_should_not_deliver_messages_across_rooms() {
        let broker = BrokerHandle::spawn_memory();
        let mut alice = broker
            .register("Alice".into(), 0)
            .await
            .expect("Alice should register");
        let mut bob = broker
            .register("Bob".into(), 1)
            .await
            .expect("Bob should register");
        drain(&mut alice.events);
        drain(&mut bob.events);

        broker
            .send_message(alice.client_id, vec![0x55; BITMAP_BYTES])
            .expect("bitmap should enqueue");
        let _alice_message = receive_message(&mut alice.events).await;

        assert!(bob.events.try_recv().is_err());
    }

    #[test]
    fn stale_live_session_should_not_control_its_replacement() {
        let (_commands, receiver) = tokio::sync::mpsc::channel(1);
        let (room_updates, _) = tokio::sync::watch::channel(super::empty_room_summaries());
        let mut broker = Esp32Broker::new(Esp32Config::default(), receiver, room_updates);
        let first = broker.allocate_client_id().unwrap();
        let second = broker.allocate_client_id().unwrap();
        let (events, _receiver) = tokio::sync::mpsc::channel(CLIENT_EVENT_CAPACITY);
        broker.client = Some((
            second,
            ConnectedClient {
                name: "new session".into(),
                room: 0,
                events,
                next_message_id: 0,
                seen: Vec::new(),
            },
        ));
        let (commands, radio) = std::sync::mpsc::sync_channel(8);
        broker.hardware_commands = Some(commands);
        broker.unregister(first);
        broker.send_message(first, vec![0; BITMAP_BYTES]);
        assert_eq!(broker.client.as_ref().map(|(id, _)| *id), Some(second));
        assert!(matches!(
            radio.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        broker.send_message(second, vec![0; BITMAP_BYTES]);
        assert!(matches!(
            radio.try_recv(),
            Ok(crate::worker::LiveCommand::SendMessage { .. })
        ));
    }

    #[test]
    fn exhausted_live_session_ids_should_not_wrap() {
        let (_commands, receiver) = tokio::sync::mpsc::channel(1);
        let (room_updates, _) = tokio::sync::watch::channel(super::empty_room_summaries());
        let mut broker = Esp32Broker::new(Esp32Config::default(), receiver, room_updates);
        broker.next_client_id = Some(u32::MAX);
        assert_eq!(broker.allocate_client_id(), Some(ClientId(u32::MAX)));
        assert_eq!(broker.allocate_client_id(), None);
    }

    #[test]
    fn live_broker_should_publish_physical_join_names() {
        let (_commands, receiver) = tokio::sync::mpsc::channel(1);
        let (room_updates, _rooms) = tokio::sync::watch::channel(super::empty_room_summaries());
        let mut broker = Esp32Broker::new(Esp32Config::default(), receiver, room_updates);
        let (events, mut event_receiver) = tokio::sync::mpsc::channel(CLIENT_EVENT_CAPACITY);
        broker.client = Some((
            ClientId(1),
            ConnectedClient {
                name: "Relay".into(),
                room: 0,
                events,
                next_message_id: 0,
                seen: Vec::new(),
            },
        ));
        broker.participants = vec![crate::protocol::Participant {
            client_id: super::station_id(Esp32Config::default().station),
            name: "Relay".into(),
        }];
        broker.roster_initialized = true;

        broker.handle_hardware_event(LiveEvent::Participants {
            room: 0,
            participants: vec![
                LiveParticipant {
                    station: Esp32Config::default().station,
                    name: "Relay".into(),
                },
                LiveParticipant {
                    station: MacAddress([0, 0x25, 0xa0, 0xdc, 0x81, 0xdf]),
                    name: "Rob".into(),
                },
            ],
        });

        let event = event_receiver
            .try_recv()
            .expect("physical join event should be published");
        assert!(matches!(
            event,
            SessionEvent::ParticipantJoined { participant, .. }
                if participant.name == "Rob"
        ));
    }

    #[test]
    fn live_broker_should_publish_physical_leave_names() {
        let (_commands, receiver) = tokio::sync::mpsc::channel(1);
        let (room_updates, _rooms) = tokio::sync::watch::channel(super::empty_room_summaries());
        let mut broker = Esp32Broker::new(Esp32Config::default(), receiver, room_updates);
        let (events, mut event_receiver) = tokio::sync::mpsc::channel(CLIENT_EVENT_CAPACITY);
        broker.client = Some((
            ClientId(1),
            ConnectedClient {
                name: "Relay".into(),
                room: 0,
                events,
                next_message_id: 0,
                seen: Vec::new(),
            },
        ));
        broker.participants = vec![crate::protocol::Participant {
            client_id: 42,
            name: "Rob".into(),
        }];
        broker.roster_initialized = true;

        broker.handle_hardware_event(LiveEvent::Participants {
            room: 0,
            participants: Vec::new(),
        });

        let event = event_receiver
            .try_recv()
            .expect("physical leave event should be published");
        assert!(matches!(
            event,
            SessionEvent::ParticipantLeft { participant, .. }
                if participant.name == "Rob"
        ));
    }

    fn drain(events: &mut tokio::sync::mpsc::Receiver<SessionEvent>) {
        while events.try_recv().is_ok() {}
    }

    async fn receive_message(
        events: &mut tokio::sync::mpsc::Receiver<SessionEvent>,
    ) -> SessionEvent {
        loop {
            let envelope = events.recv().await.expect("broker should remain available");
            if matches!(envelope, SessionEvent::Message { .. }) {
                return envelope;
            }
        }
    }
    #[test]
    fn passive_counts_publish_without_a_session_and_expire_to_unknown() {
        let (_commands, receiver) = tokio::sync::mpsc::channel(1);
        let (updates, rooms) = tokio::sync::watch::channel(super::empty_room_summaries());
        let mut broker = Esp32Broker::new(Esp32Config::default(), receiver, updates);
        broker.discovered = [2, 255, 255, 255];
        broker.publish_rooms();
        assert!(rooms.has_changed().unwrap());
        assert_eq!(rooms.borrow()[0].participant_count, 2);
        assert!(rooms.borrow()[0].observed);
        assert!(rooms.borrow()[0].participants.is_empty());
        assert!(!rooms.borrow()[1].observed);
        broker.discovered = [255; 4];
        broker.publish_rooms();
        assert!(!rooms.borrow()[0].observed);
        assert_eq!(rooms.borrow()[0].participant_count, 0);
    }
}
