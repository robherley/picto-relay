//! USB protocol between the host server and the ESP32 firmware.
//!
//! This crate is the single definition of the protocol. The firmware's C header
//! (`firmware/esp32/main/link_protocol.h`) is generated from the constants below
//! by [`c_header`]; a test fails when the checked-in header is out of date. The
//! framing itself is implemented twice (here and in `firmware/esp32/main/link.c`)
//! and both are checked against `firmware/esp32/test/fixtures/link.bin`.
//!
//! See `docs/esp32-usb.md` for the prose description.

/// Declares the protocol constants once, as Rust `pub const`s and as a table
/// from which the firmware's C header is generated.
macro_rules! protocol {
    ($( $(#[doc = $doc:literal])+ $name:ident: $ty:ty = $value:expr; )+) => {
        $( $(#[doc = $doc])+ pub const $name: $ty = $value; )+

        /// Name, value and documentation of every protocol constant.
        const CONSTANTS: &[(&str, u64, &[&str])] = &[
            $( (stringify!($name), $name as u64, &[$($doc),+]), )+
        ];
    };
}

protocol! {
    // ---- Link ---------------------------------------------------------------

    /// Protocol version. HELLO fails with a clear error when the two sides differ.
    PROTOCOL_VERSION: u8 = 3;
    /// UART speed, 8N1.
    BAUD: u32 = 921_600;
    /// Largest payload of a command (host to ESP32).
    MAX_COMMAND_PAYLOAD: usize = 1024;
    /// Largest payload of a response or event (ESP32 to host).
    MAX_EVENT_PAYLOAD: usize = 256;
    /// The firmware leaves the room after this long without any command, in milliseconds.
    HOST_TIMEOUT_MS: u32 = 5000;
    /// Interval between STATUS events while idle, in milliseconds.
    STATUS_INTERVAL_MS: u32 = 1000;
    /// Interval between STATUS events while a drawing is being sent, in milliseconds.
    STATUS_SENDING_INTERVAL_MS: u32 = 250;

    // ---- Framing ------------------------------------------------------------

    /// Marks the start and end of every packet.
    DELIMITER: u8 = 0x7e;
    /// Precedes an escaped byte.
    ESCAPE: u8 = 0x7d;
    /// XORed with a byte to escape or unescape it.
    ESCAPE_XOR: u8 = 0x20;
    /// Packet header: offset of the packet kind.
    KIND_OFFSET: usize = 0;
    /// Packet header: offset of the little-endian u32 request or transfer ID.
    ID_OFFSET: usize = 1;
    /// Packet header: offset of the little-endian u16 payload length.
    LENGTH_OFFSET: usize = 5;
    /// Packet header length; the payload starts here.
    PAYLOAD_OFFSET: usize = 7;
    /// Length of the little-endian IEEE CRC32 that follows the payload.
    CRC_LEN: usize = 4;

    // ---- Commands (host to ESP32) -------------------------------------------

    /// Attach to the firmware. Payload: the host's protocol version.
    HELLO: u8 = 1;
    /// Find and join a room. Payload: see the `JOIN_*` layout.
    JOIN: u8 = 2;
    /// Leave the room, telling the DS host.
    LEAVE: u8 = 3;
    /// Keep the session alive; any command does, this one has no other effect.
    PING: u8 = 4;
    /// While idle, sweep all channels once and report a ROOMS event.
    SCAN: u8 = 5;
    /// Upload part of a drawing. Payload: see the `DRAW_*` layout.
    DRAW: u8 = 6;
    /// Turn frame capture on (payload 1) or off (payload 0). While on, every
    /// `PictoChat` radio frame heard or sent is reported as a FRAME event.
    TRACE: u8 = 7;

    // ---- Responses ----------------------------------------------------------

    /// Answer to a command, with the same ID. Payload: status byte, then data.
    RESPONSE: u8 = 128;
    /// Status: the command succeeded.
    STATUS_OK: u8 = 0;
    /// Status: try again shortly (the device or its drawing buffer is occupied).
    STATUS_BUSY: u8 = 1;
    /// Status: malformed command; retrying cannot help.
    STATUS_INVALID: u8 = 2;
    /// Status: the command does not apply in the current session state.
    STATUS_WRONG_STATE: u8 = 3;
    /// Status: HELLO from a host with a different protocol version.
    STATUS_VERSION_MISMATCH: u8 = 4;
    /// Status: a command arrived before HELLO.
    STATUS_NOT_ATTACHED: u8 = 5;

    /// HELLO response data: offset of the firmware's protocol version.
    HELLO_VERSION_OFFSET: usize = 0;
    /// HELLO response data: offset of the station MAC address.
    HELLO_MAC_OFFSET: usize = 1;
    /// HELLO response data: offset of the firmware build label (ASCII, to the end).
    HELLO_BUILD_OFFSET: usize = 7;
    /// Longest firmware build label.
    MAX_BUILD_LEN: usize = 32;

    // ---- Shared sizes -------------------------------------------------------

    /// Octets in a MAC address.
    MAC_LEN: usize = 6;
    /// `PictoChat` rooms A through D.
    ROOM_COUNT: usize = 4;
    /// Nickname field: ten UTF-16LE code units, zero-padded.
    NICKNAME_LEN: usize = 20;
    /// Personal message field: 26 UTF-16LE code units, zero-padded.
    PROFILE_MESSAGE_LEN: usize = 52;
    /// Highest DS profile color index.
    MAX_PROFILE_COLOR: u8 = 15;
    /// Largest native drawing: 36 bytes of metadata and five 2048-byte bands.
    MAX_DRAWING_BYTES: usize = 10276;
    /// Smallest native drawing: metadata and one band.
    MIN_DRAWING_BYTES: usize = 2084;

    // ---- JOIN payload -------------------------------------------------------

    /// JOIN: offset of the room number, 0 to 3.
    JOIN_ROOM_OFFSET: usize = 0;
    /// JOIN: offset of the nickname.
    JOIN_NICKNAME_OFFSET: usize = 1;
    /// JOIN: offset of the personal message.
    JOIN_MESSAGE_OFFSET: usize = JOIN_NICKNAME_OFFSET + NICKNAME_LEN;
    /// JOIN: offset of the profile color.
    JOIN_COLOR_OFFSET: usize = JOIN_MESSAGE_OFFSET + PROFILE_MESSAGE_LEN;
    /// JOIN: payload length.
    JOIN_LEN: usize = JOIN_COLOR_OFFSET + 1;

    // ---- DRAW payload -------------------------------------------------------

    /// DRAW: offset of the little-endian u32 transfer ID chosen by the host.
    DRAW_ID_OFFSET: usize = 0;
    /// DRAW: offset of the little-endian u16 position of this chunk in the drawing.
    DRAW_POSITION_OFFSET: usize = 4;
    /// DRAW: offset of the little-endian u16 total drawing length.
    DRAW_TOTAL_OFFSET: usize = 6;
    /// DRAW: offset of the chunk's drawing bytes, which run to the end.
    DRAW_DATA_OFFSET: usize = 8;
    /// DRAW: most drawing bytes in one command.
    MAX_DRAW_CHUNK: usize = MAX_COMMAND_PAYLOAD - DRAW_DATA_OFFSET;

    // ---- Events (ESP32 to host) ---------------------------------------------

    /// Periodic session report; the packet ID is the drawing being sent, if any.
    STATUS: u8 = 129;
    /// A DS application packet from the room host, forwarded once.
    RX: u8 = 130;
    /// Events were dropped because the output queue was full. Payload: u32 count.
    GAP: u8 = 131;
    /// Radio timing counters, only from firmware built with `PICTO_DIAGNOSTICS`.
    DIAG: u8 = 132;
    /// The profile exchange finished; drawings may be sent.
    READY: u8 = 133;
    /// The DS confirmed the drawing whose transfer ID is the packet ID.
    DELIVERED: u8 = 134;
    /// The session ended. Payload: see the `LEFT_*` layout.
    LEFT: u8 = 135;
    /// Result of a SCAN: member counts of rooms A to D.
    ROOMS: u8 = 136;

    /// STATUS: offset of the session state.
    STATUS_STATE_OFFSET: usize = 0;
    /// STATUS: offset of the profile-exchange phase.
    STATUS_PHASE_OFFSET: usize = 1;
    /// STATUS: offset of the participant slot.
    STATUS_SLOT_OFFSET: usize = 2;
    /// STATUS: offset of the room number.
    STATUS_ROOM_OFFSET: usize = 3;
    /// STATUS: offset of the Wi-Fi channel.
    STATUS_CHANNEL_OFFSET: usize = 4;
    /// STATUS: offset of the number of fragments the DS has acknowledged.
    STATUS_SENT_OFFSET: usize = 5;
    /// STATUS: offset of the number of fragments in the drawing being sent.
    STATUS_TOTAL_OFFSET: usize = 6;
    /// STATUS: offset of the number of fragments the DS has echoed back.
    STATUS_ECHOED_OFFSET: usize = 7;
    /// STATUS: offset of the little-endian u32 counters: polls, replies,
    /// positive acknowledgements, negative acknowledgements.
    STATUS_COUNTERS_OFFSET: usize = 8;
    /// STATUS: number of counters.
    STATUS_COUNTERS: usize = 4;
    /// STATUS: payload length.
    STATUS_LEN: usize = STATUS_COUNTERS_OFFSET + STATUS_COUNTERS * 4;

    /// Session state: idle.
    STATE_OFF: u8 = 0;
    /// Session state: searching channels for the room.
    STATE_DISCOVER: u8 = 1;
    /// Session state: authenticating with the DS host.
    STATE_AUTHENTICATING: u8 = 2;
    /// Session state: associating with the DS host.
    STATE_ASSOCIATING: u8 = 3;
    /// Session state: in the room.
    STATE_CONNECTED: u8 = 4;
    /// Profile phase at which the exchange is complete.
    PHASE_READY: u8 = 12;

    /// READY: offset of the DS host's MAC address.
    READY_HOST_OFFSET: usize = 0;
    /// READY: offset of the participant slot.
    READY_SLOT_OFFSET: usize = READY_HOST_OFFSET + MAC_LEN;
    /// READY: offset of the room number.
    READY_ROOM_OFFSET: usize = READY_SLOT_OFFSET + 1;
    /// READY: offset of the Wi-Fi channel.
    READY_CHANNEL_OFFSET: usize = READY_ROOM_OFFSET + 1;
    /// READY: payload length.
    READY_LEN: usize = READY_CHANNEL_OFFSET + 1;

    /// LEFT: offset of the reason.
    LEFT_REASON_OFFSET: usize = 0;
    /// LEFT: offset of the little-endian u16 IEEE status or reason code, if any.
    LEFT_CODE_OFFSET: usize = 1;
    /// LEFT: payload length.
    LEFT_LEN: usize = 3;
    /// Left because the host asked, or stopped sending commands.
    LEFT_REQUESTED: u8 = 0;
    /// No DS is hosting the room on any channel.
    LEFT_NO_HOST: u8 = 1;
    /// The DS host did not answer the join.
    LEFT_JOIN_TIMEOUT: u8 = 2;
    /// The DS host refused the join; the code is its IEEE status.
    LEFT_REJECTED: u8 = 3;
    /// The DS host removed us; the code is its IEEE reason.
    LEFT_KICKED: u8 = 4;
    /// Nothing was heard from the DS host for the session timeout.
    LEFT_HOST_LOST: u8 = 5;
    /// The radio driver never reported a transmission as finished.
    LEFT_RADIO_STALLED: u8 = 6;

    /// ROOMS: member count for a room whose beacon was not heard.
    NO_BEACON: u8 = 255;

    /// One captured 802.11 frame, without its FCS; sent only while TRACE is on.
    FRAME: u8 = 137;
    /// FRAME: offset of the little-endian u32 radio clock in microseconds.
    FRAME_TIME_OFFSET: usize = 0;
    /// FRAME: offset of the flags.
    FRAME_FLAGS_OFFSET: usize = 4;
    /// FRAME: offset of the little-endian u16 length of the frame on the air.
    FRAME_LENGTH_OFFSET: usize = 5;
    /// FRAME: offset of the frame bytes, which may be cut short.
    FRAME_DATA_OFFSET: usize = 7;
    /// FRAME: most frame bytes carried; longer frames are truncated.
    FRAME_MAX_BYTES: usize = MAX_EVENT_PAYLOAD - FRAME_DATA_OFFSET;
    /// FRAME flag: the relay transmitted this frame (otherwise it received it).
    FRAME_FLAG_SENT: u8 = 1;
}

/// Header plus CRC: the size of a packet with an empty payload.
const OVERHEAD: usize = PAYLOAD_OFFSET + CRC_LEN;
/// Reflected polynomial of the IEEE 802.3 CRC32.
const CRC32_POLYNOMIAL: u32 = 0xedb8_8320;
/// File name of the generated C header.
pub const C_HEADER_NAME: &str = "link_protocol.h";

const _: () = {
    assert!(STATUS_LEN <= MAX_EVENT_PAYLOAD);
    assert!(HELLO_BUILD_OFFSET + MAX_BUILD_LEN < MAX_EVENT_PAYLOAD);
    assert!(MAX_DRAWING_BYTES <= u16::MAX as usize);
};

/// One framed packet in either direction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Packet {
    pub kind: u8,
    pub id: u32,
    pub payload: Vec<u8>,
}

/// IEEE 802.3 CRC32.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..u8::BITS {
            let low_bit_mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (CRC32_POLYNOMIAL & low_bit_mask);
        }
    }
    !crc
}

/// Frames a packet for the wire. Returns `None` when the payload is larger
/// than any packet may carry.
pub fn encode(kind: u8, id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    if payload.len() > MAX_COMMAND_PAYLOAD {
        return None;
    }
    let length = u16::try_from(payload.len()).ok()?;
    let mut raw = Vec::with_capacity(OVERHEAD + payload.len());
    raw.push(kind);
    raw.extend_from_slice(&id.to_le_bytes());
    raw.extend_from_slice(&length.to_le_bytes());
    raw.extend_from_slice(payload);
    raw.extend_from_slice(&crc32(&raw).to_le_bytes());

    let mut wire = vec![DELIMITER];
    for byte in raw {
        if matches!(byte, DELIMITER | ESCAPE) {
            wire.push(ESCAPE);
            wire.push(byte ^ ESCAPE_XOR);
        } else {
            wire.push(byte);
        }
    }
    wire.push(DELIMITER);
    Some(wire)
}

/// Incremental decoder. Noise, corrupt packets and truncated packets are
/// dropped; the next delimiter restores framing.
#[derive(Default)]
pub struct Decoder {
    raw: Vec<u8>,
    escape: bool,
    active: bool,
}

impl Decoder {
    pub const fn new() -> Self {
        Self {
            raw: Vec::new(),
            escape: false,
            active: false,
        }
    }

    /// Feeds one received byte; returns a packet when one completes.
    pub fn push(&mut self, byte: u8) -> Option<Packet> {
        if byte == DELIMITER {
            let packet = if self.active && !self.escape {
                self.finish()
            } else {
                None
            };
            self.raw.clear();
            self.escape = false;
            self.active = true;
            return packet;
        }
        if !self.active {
            return None;
        }
        let byte = if self.escape {
            self.escape = false;
            byte ^ ESCAPE_XOR
        } else if byte == ESCAPE {
            self.escape = true;
            return None;
        } else {
            byte
        };
        if self.raw.len() == MAX_COMMAND_PAYLOAD + OVERHEAD {
            // Oversized: wait for the next delimiter.
            self.active = false;
            return None;
        }
        self.raw.push(byte);
        None
    }

    fn finish(&self) -> Option<Packet> {
        let raw = &self.raw;
        let length = raw.get(LENGTH_OFFSET..PAYLOAD_OFFSET)?;
        let length = usize::from(u16::from_le_bytes(length.try_into().ok()?));
        let payload_end = PAYLOAD_OFFSET + length;
        if raw.len() != payload_end + CRC_LEN {
            return None;
        }
        let crc = u32::from_le_bytes(raw[payload_end..].try_into().ok()?);
        if crc32(&raw[..payload_end]) != crc {
            return None;
        }
        Some(Packet {
            kind: raw[KIND_OFFSET],
            id: u32::from_le_bytes(raw[ID_OFFSET..LENGTH_OFFSET].try_into().ok()?),
            payload: raw[PAYLOAD_OFFSET..payload_end].to_vec(),
        })
    }
}

/// The firmware's `link_protocol.h`: every protocol constant as a `LINK_` macro.
pub fn c_header() -> String {
    let mut header = String::from(
        "/* Generated from crates/picto-relay-link/src/lib.rs. Do not edit.\n \
         * Regenerate with: just protocol-header */\n#pragma once\n",
    );
    for (name, value, docs) in CONSTANTS {
        header.push('\n');
        for doc in *docs {
            header.push_str(&format!("/*{} */\n", doc.replace('`', "")));
        }
        header.push_str(&format!("#define LINK_{name} {value}\n"));
    }
    header
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_recovers_after_noise_corruption_and_truncation() {
        let data: Vec<u8> = (0..MAX_COMMAND_PAYLOAD).map(|i| i as u8).collect();
        let mut wire = encode(DRAW, 0x7e7d_1234, &data).unwrap();
        let mut decoder = Decoder::new();
        for b in b"boot log\r\n" {
            assert!(decoder.push(*b).is_none());
        }
        for b in &wire[..wire.len() / 2] {
            assert!(decoder.push(*b).is_none());
        }
        let mut got = None;
        for &b in &wire {
            got = decoder.push(b).or(got);
        }
        assert_eq!(
            got,
            Some(Packet {
                kind: DRAW,
                id: 0x7e7d_1234,
                payload: data.clone()
            })
        );
        wire[10] ^= 1;
        for &b in &wire {
            assert!(decoder.push(b).is_none());
        }
        assert!(encode(DRAW, 1, &[0; MAX_COMMAND_PAYLOAD + 1]).is_none());
    }

    #[test]
    fn empty_and_runt_packets_are_handled() {
        let mut decoder = Decoder::new();
        let wire = encode(PING, 9, &[]).unwrap();
        let packets: Vec<_> = wire.iter().filter_map(|&b| decoder.push(b)).collect();
        assert_eq!(packets.len(), 1);
        assert!(packets[0].payload.is_empty());
        for b in [DELIMITER, 1, 2, 3, DELIMITER, DELIMITER] {
            assert!(decoder.push(b).is_none());
        }
    }

    #[test]
    fn crc_matches_standard_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn layouts_are_consistent() {
        assert_eq!(JOIN_LEN, 74);
        assert_eq!(STATUS_LEN, 24);
        assert_eq!(READY_LEN, 9);
        assert_eq!(HELLO_BUILD_OFFSET, HELLO_MAC_OFFSET + MAC_LEN);
    }
}
