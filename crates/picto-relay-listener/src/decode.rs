use std::collections::{BTreeMap, BTreeSet};

use picto_relay_codec::{
    BROADCAST, FcsMode, MESSAGE_FRAGMENT_PAYLOAD_BYTES, MacAddress, MacFrame, ManagementBody,
    MessageReassembler, MultiplayerHeader, NintendoBeacon, NintendoFlow, PictoChatEnvelope,
    PictoChatFragment, PictoChatMessage, ReassemblyKey, decode_nintendo_data, parse_management,
};
use serde::Serialize;

use crate::output::PrettyPrinter;

#[derive(Debug, Default, Serialize)]
pub(crate) struct DecodeSummary {
    pub(crate) ieee80211_frames: u64,
    pub(crate) malformed_radiotap: u64,
    pub(crate) malformed_ieee80211: u64,
    pub(crate) nintendo_frames: u64,
    pub(crate) room_changes: u64,
    pub(crate) associations: u64,
    pub(crate) leaves: u64,
    pub(crate) broadcast_deauthentications: u64,
    pub(crate) message_fragments: u64,
    pub(crate) completed_messages: u64,
    pub(crate) failed_reply_rounds: u64,
    pub(crate) packet_families: BTreeMap<String, u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RoomObservation {
    opaque_state: u32,
    room: u8,
    user_count: u8,
}

pub(crate) struct Decoder {
    printer: PrettyPrinter,
    verbose: bool,
    rooms: BTreeMap<MacAddress, RoomObservation>,
    seen_families: BTreeSet<(u8, u16)>,
    seen_failure_masks: BTreeSet<u16>,
    departed: BTreeSet<MacAddress>,
    reassembler: MessageReassembler,
    summary: DecodeSummary,
}

impl Decoder {
    pub(crate) fn new(printer: PrettyPrinter, verbose: bool) -> Self {
        Self {
            printer,
            verbose,
            rooms: BTreeMap::new(),
            seen_families: BTreeSet::new(),
            seen_failure_masks: BTreeSet::new(),
            departed: BTreeSet::new(),
            reassembler: MessageReassembler::new(MESSAGE_FRAGMENT_PAYLOAD_BYTES),
            summary: DecodeSummary::default(),
        }
    }

    pub(crate) fn malformed_radiotap(&mut self) {
        self.summary.malformed_radiotap += 1;
    }

    pub(crate) fn inspect(&mut self, elapsed: f64, bytes: &[u8], fcs_mode: FcsMode) {
        let Ok(frame) = MacFrame::parse(bytes, fcs_mode) else {
            self.summary.malformed_ieee80211 += 1;
            return;
        };
        self.summary.ieee80211_frames += 1;
        match frame.header.frame_control.frame_type() {
            picto_relay_codec::FrameType::Management => self.inspect_management(elapsed, &frame),
            picto_relay_codec::FrameType::Data => self.inspect_data(elapsed, &frame),
            picto_relay_codec::FrameType::Control | picto_relay_codec::FrameType::Reserved => {}
        }
    }

    pub(crate) fn finish(self) -> DecodeSummary {
        self.summary
    }

    fn inspect_management(&mut self, elapsed: f64, frame: &MacFrame) {
        if frame.header.frame_control.subtype() == 12 {
            self.inspect_deauthentication(elapsed, frame);
            return;
        }
        let Ok(body) = parse_management(frame) else {
            return;
        };
        match body {
            ManagementBody::Beacon(beacon) => {
                let Ok(nintendo) = NintendoBeacon::from_beacon(&beacon) else {
                    return;
                };
                let Ok(Some(room)) = nintendo.pictochat() else {
                    return;
                };
                let Some(host) = frame.header.address_2 else {
                    return;
                };
                self.observe_room(
                    elapsed,
                    host,
                    RoomObservation {
                        opaque_state: room.opaque_state,
                        room: room.room,
                        user_count: room.user_count,
                    },
                );
            }
            ManagementBody::Authentication(auth) => {
                let direction = endpoints(frame);
                self.printer.event(
                    elapsed,
                    "AUTH",
                    &format!(
                        "{direction} · algorithm {} · sequence {} · status {}",
                        auth.algorithm, auth.sequence, auth.status
                    ),
                );
            }
            ManagementBody::AssociationRequest(request) => {
                if let Some(station) = frame.header.address_2 {
                    self.departed.remove(&station);
                }
                self.printer.event(
                    elapsed,
                    "JOIN",
                    &format!(
                        "{} · association request · listen interval {}",
                        endpoints(frame),
                        request.listen_interval
                    ),
                );
            }
            ManagementBody::AssociationResponse(response) => {
                if response.status == 0 {
                    self.departed.remove(&frame.header.address_1);
                }
                self.summary.associations += 1;
                self.printer.event(
                    elapsed,
                    "JOIN",
                    &format!(
                        "{} · status {} · AID 0x{:04x} · slot {}",
                        endpoints(frame),
                        response.status,
                        response.association_id,
                        response.participant_slot()
                    ),
                );
            }
            ManagementBody::Unknown { .. } => {}
        }
    }

    fn observe_room(&mut self, elapsed: f64, host: MacAddress, observed: RoomObservation) {
        let previous = self.rooms.insert(host, observed);
        let semantic_change = previous.is_none_or(|previous| {
            previous.room != observed.room || previous.user_count != observed.user_count
        });
        if semantic_change {
            self.summary.room_changes += 1;
            self.printer.event(
                elapsed,
                "ROOM",
                &format!(
                    "host {host} · room {} · {} user(s)",
                    room_name(observed.room),
                    observed.user_count
                ),
            );
        } else if self.verbose
            && previous.is_some_and(|previous| previous.opaque_state != observed.opaque_state)
        {
            self.printer.event(
                elapsed,
                "BEACON",
                &format!(
                    "host {host} · opaque state 0x{:08x} → 0x{:08x}",
                    previous.map_or(0, |previous| previous.opaque_state),
                    observed.opaque_state
                ),
            );
        }
    }

    fn inspect_deauthentication(&mut self, elapsed: f64, frame: &MacFrame) {
        let Some(reason) = read_u16(frame.body(), 0) else {
            return;
        };
        let Some(source) = frame.header.address_2 else {
            return;
        };
        let destination = frame.header.address_1;
        let source_is_host = self.rooms.contains_key(&source);
        if destination == BROADCAST {
            if source_is_host {
                self.summary.broadcast_deauthentications += 1;
                if self.verbose {
                    self.printer.event(
                        elapsed,
                        "CONTROL",
                        &format!("host {source} · broadcast deauthentication · reason {reason}"),
                    );
                }
            }
            return;
        }
        let departed = if source_is_host {
            destination
        } else if self.rooms.contains_key(&destination) {
            source
        } else {
            return;
        };
        if !self.departed.insert(departed) {
            if self.verbose {
                self.printer.event(
                    elapsed,
                    "RETRY",
                    &format!("station {departed} · repeated deauthentication reason {reason}"),
                );
            }
            return;
        }
        self.summary.leaves += 1;
        self.printer.event(
            elapsed,
            "LEAVE",
            &format!("station {departed} · deauthentication reason {reason}"),
        );
    }

    fn inspect_data(&mut self, elapsed: f64, frame: &MacFrame) {
        let Ok(data) = decode_nintendo_data(frame) else {
            return;
        };
        self.summary.nintendo_frames += 1;
        if let MultiplayerHeader::HostAcknowledgement {
            command_count,
            failed_client_mask,
        } = data.multiplayer
        {
            if failed_client_mask != 0 {
                self.summary.failed_reply_rounds += 1;
                if self.verbose || self.seen_failure_masks.insert(failed_client_mask) {
                    self.printer.event(
                        elapsed,
                        "FAIL",
                        &format!(
                            "host acknowledgement · command {command_count} · failed mask 0x{failed_client_mask:04x}{}",
                            if self.verbose {
                                ""
                            } else {
                                " · first seen; repeats summarized"
                            }
                        ),
                    );
                }
            } else if self.verbose {
                self.printer.event(
                    elapsed,
                    "ACK",
                    &format!("command {command_count} · failed mask 0"),
                );
            }
            return;
        }

        let Ok(envelope) = PictoChatEnvelope::parse(data.flow, &data.payload) else {
            return;
        };
        self.report_envelope(elapsed, data.flow, &envelope);
        if !envelope.is_message_fragment() {
            return;
        }
        let Ok(fragment) = PictoChatFragment::parse(&envelope.payload, data.flow) else {
            return;
        };
        self.summary.message_fragments += 1;
        self.report_fragment(elapsed, data.flow, &envelope, &fragment);
        let key = ReassemblyKey {
            source: data.source,
            opaque_id: fragment.opaque_id,
        };
        match self.reassembler.push(key, &fragment) {
            Ok(Some(message)) => self.report_message(
                elapsed,
                data.flow,
                data.source,
                fragment.opaque_id,
                &message,
            ),
            Ok(None) => {}
            Err(error) => self
                .printer
                .warning(elapsed, &format!("fragment reassembly: {error}")),
        }
    }

    fn report_envelope(&mut self, elapsed: f64, flow: NintendoFlow, envelope: &PictoChatEnvelope) {
        let family = format!("{}:0x{:04x}", flow_name(flow), envelope.packet_id);
        *self.summary.packet_families.entry(family).or_default() += 1;
        let first_family = self
            .seen_families
            .insert((flow_code(flow), envelope.packet_id));
        if self.verbose || first_family {
            let payload = if self.verbose {
                format!(" · payload {}", hex_bytes(&envelope.payload))
            } else {
                String::new()
            };
            self.printer.event(
                elapsed,
                "PACKET",
                &format!(
                    "{} · id 0x{:04x} · seq {} · declared {} · payload {} B{}{payload}",
                    flow_label(flow),
                    envelope.packet_id,
                    envelope.packet_sequence,
                    envelope.declared_length,
                    envelope.payload.len(),
                    if first_family { " · first seen" } else { "" }
                ),
            );
        }
    }

    fn report_fragment(
        &self,
        elapsed: f64,
        flow: NintendoFlow,
        envelope: &PictoChatEnvelope,
        fragment: &PictoChatFragment,
    ) {
        self.printer.event(
            elapsed,
            "FRAGMENT",
            &format!(
                "{} · id 0x{:04x} · tag 0x{:04x} · offset 0x{:04x} · {} B · frame {}{}",
                flow_label(flow),
                envelope.packet_id,
                transfer_tag(fragment.opaque_id),
                fragment.data_offset,
                fragment.data.len(),
                fragment.frame_number,
                if fragment.final_fragment {
                    " · FINAL"
                } else {
                    ""
                }
            ),
        );
    }

    fn report_message(
        &mut self,
        elapsed: f64,
        flow: NintendoFlow,
        source: MacAddress,
        opaque_id: [u8; 2],
        message: &[u8],
    ) {
        match PictoChatMessage::parse(message) {
            Ok(decoded) => {
                self.summary.completed_messages += 1;
                let bounds = decoded.ink_bounds().map_or_else(
                    || "no ink".to_owned(),
                    |bounds| {
                        format!(
                            "ink x={}..{}, y={}..{}",
                            bounds.min_x, bounds.max_x, bounds.min_y, bounds.max_y
                        )
                    },
                );
                let mut used_palette = [false; 16];
                for &pixel in decoded.pixels() {
                    used_palette[usize::from(pixel)] = true;
                }
                let palette = used_palette
                    .iter()
                    .enumerate()
                    .filter_map(|(index, &used)| used.then_some(index.to_string()))
                    .collect::<Vec<_>>()
                    .join(",");
                let metadata = if self.verbose {
                    format!(" · metadata {}", hex_bytes(decoded.metadata()))
                } else {
                    String::new()
                };
                self.printer.event(
                    elapsed,
                    "MESSAGE",
                    &format!(
                        "{} · sender {} · tag 0x{:04x} · 256x{} 4bpp · {bounds} · palette [{palette}] · {} B{metadata}",
                        flow_label(flow),
                        decoded.sender(),
                        transfer_tag(opaque_id),
                        decoded.encoded_height(),
                        message.len()
                    ),
                );
            }
            Err(error) => self.printer.event(
                elapsed,
                "TRANSFER",
                &format!(
                    "{} · source {source} · tag 0x{:04x} · {} B · not a drawing ({error})",
                    flow_label(flow),
                    transfer_tag(opaque_id),
                    message.len()
                ),
            ),
        }
    }
}

const fn transfer_tag(opaque_id: [u8; 2]) -> u16 {
    u16::from_le_bytes(opaque_id)
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
    ];
    bytes
        .iter()
        .flat_map(|byte| [HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0x0f)]])
        .collect()
}

const fn room_name(room: u8) -> char {
    match room {
        0 => 'A',
        1 => 'B',
        2 => 'C',
        3 => 'D',
        _ => '?',
    }
}

fn endpoints(frame: &MacFrame) -> String {
    let source = frame
        .header
        .address_2
        .map_or_else(|| "?".into(), |mac| mac.to_string());
    format!("{source} → {}", frame.header.address_1)
}

const fn flow_code(flow: NintendoFlow) -> u8 {
    match flow {
        NintendoFlow::HostToClient => 0,
        NintendoFlow::ClientToHost => 1,
        NintendoFlow::HostAcknowledgement => 2,
    }
}

const fn flow_name(flow: NintendoFlow) -> &'static str {
    match flow {
        NintendoFlow::HostToClient => "host",
        NintendoFlow::ClientToHost => "client",
        NintendoFlow::HostAcknowledgement => "ack",
    }
}

const fn flow_label(flow: NintendoFlow) -> &'static str {
    match flow {
        NintendoFlow::HostToClient => "host → clients",
        NintendoFlow::ClientToHost => "client → host",
        NintendoFlow::HostAcknowledgement => "host acknowledgement",
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let bytes = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

#[cfg(test)]
#[path = "../../picto-relay-codec/tests/support/mod.rs"]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;

    use super::test_support::decode_hex;

    fn fixture(name: &str) -> Vec<u8> {
        let text = match name {
            "room-a" => include_str!("../../picto-relay-codec/tests/fixtures/room-refresh-a.hex"),
            "room-b" => include_str!("../../picto-relay-codec/tests/fixtures/room-refresh-b.hex"),
            "broadcast-deauth" => {
                include_str!("../../picto-relay-codec/tests/fixtures/host-broadcast-deauth.hex")
            }
            "client-leave" => {
                include_str!("../../picto-relay-codec/tests/fixtures/client-leave-deauth.hex")
            }
            _ => panic!("unknown fixture {name}"),
        };
        decode_hex(text)
    }

    #[test]
    fn opaque_beacon_rotation_should_not_count_as_room_change() {
        let mut decoder = Decoder::new(PrettyPrinter::new(true), false);
        decoder.inspect(0.0, &fixture("room-a"), FcsMode::Absent);
        decoder.inspect(1.0, &fixture("room-b"), FcsMode::Absent);
        assert_eq!(decoder.finish().room_changes, 1);
    }

    #[test]
    fn host_broadcast_deauthentication_should_not_count_as_leave() {
        let mut decoder = Decoder::new(PrettyPrinter::new(true), false);
        decoder.inspect(0.0, &fixture("room-a"), FcsMode::Absent);
        decoder.inspect(1.0, &fixture("broadcast-deauth"), FcsMode::Absent);
        let summary = decoder.finish();
        assert_eq!(
            (summary.leaves, summary.broadcast_deauthentications),
            (0, 1)
        );
    }

    #[test]
    fn user_count_change_should_count_as_second_room_change() {
        let mut decoder = Decoder::new(PrettyPrinter::new(true), false);
        decoder.inspect(0.0, &fixture("room-a"), FcsMode::Absent);
        let joined = decode_hex(include_str!(
            "../../picto-relay-codec/tests/fixtures/joined-room-a-beacon.hex"
        ));
        decoder.inspect(1.0, &joined, FcsMode::Absent);
        assert_eq!(decoder.finish().room_changes, 2);
    }

    #[test]
    fn client_unicast_deauthentication_should_count_as_leave() {
        let mut decoder = Decoder::new(PrettyPrinter::new(true), false);
        decoder.inspect(0.0, &fixture("room-a"), FcsMode::Absent);
        decoder.inspect(1.0, &fixture("client-leave"), FcsMode::Absent);
        assert_eq!(decoder.finish().leaves, 1);
    }

    #[test]
    fn repeated_client_deauthentication_should_count_as_one_leave() {
        let mut decoder = Decoder::new(PrettyPrinter::new(true), false);
        decoder.inspect(0.0, &fixture("room-a"), FcsMode::Absent);
        let leave = fixture("client-leave");
        decoder.inspect(1.0, &leave, FcsMode::Absent);
        decoder.inspect(1.1, &leave, FcsMode::Absent);
        assert_eq!(decoder.finish().leaves, 1);
    }

    #[test]
    fn captured_client_fragment_should_be_counted() {
        let compact =
            include_str!("../../picto-relay-codec/tests/fixtures/client-first-fragment.hex").trim();
        let bytes = compact
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(
                    std::str::from_utf8(pair).expect("fixture must be ASCII"),
                    16,
                )
                .expect("fixture must contain hex")
            })
            .collect::<Vec<_>>();
        let mut decoder = Decoder::new(PrettyPrinter::new(true), false);
        decoder.inspect(0.0, &bytes, FcsMode::Absent);
        assert_eq!(decoder.finish().message_fragments, 1);
    }

    #[test]
    fn transfer_tag_should_decode_opaque_identifier_as_little_endian() {
        assert_eq!(transfer_tag([0x34, 0x12]), 0x1234);
    }

    #[test]
    fn hex_bytes_should_encode_lowercase_without_separators() {
        assert_eq!(hex_bytes(&[0x00, 0xaf, 0x19]), "00af19");
    }
}
