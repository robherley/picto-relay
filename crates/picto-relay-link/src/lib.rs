//! Allocation-free USB framing between the host server and the ESP32 firmware.
//! The firmware implements the same protocol in C (`firmware/esp32/main/link.c`);
//! both sides are checked against `firmware/esp32/test/fixtures/link.bin`.
#![no_std]
/// Framing version. Firmware and server are always built from the same tree.
pub const VERSION: u8 = 2;
/// Runtime UART speed of the firmware and host.
pub const BAUD: u32 = 921_600;
/// Drawing bytes carried by one radio fragment.
pub const FRAGMENT_BYTES: usize = 120;
/// Largest application body: one fragment plus its 16-byte envelope.
pub const MAX_BODY: usize = FRAGMENT_BYTES + 16;
/// Smallest application body the DS protocol uses.
pub const MIN_BODY: usize = 16;
/// Largest packet payload.
pub const MAX_PAYLOAD: usize = 256;

// ---- Packet layout, before escaping ----------------------------------------

/// Offset of the framing version.
const VERSION_OFFSET: usize = 0;
/// Offset of the packet kind.
const KIND_OFFSET: usize = 1;
/// Offset of the little-endian request ID.
const ID_OFFSET: usize = 2;
/// Offset of the little-endian payload length.
const LENGTH_OFFSET: usize = ID_OFFSET + size_of::<u32>();
/// Offset of the payload; also the header length.
const PAYLOAD_OFFSET: usize = LENGTH_OFFSET + size_of::<u16>();
/// Length of the CRC32 that follows the payload.
const CRC_LEN: usize = size_of::<u32>();
/// Header plus CRC: the size of a packet with an empty payload.
const OVERHEAD: usize = PAYLOAD_OFFSET + CRC_LEN;
/// Largest unescaped packet.
const MAX_RAW: usize = MAX_PAYLOAD + OVERHEAD;

// ---- Byte stuffing ----------------------------------------------------------

/// Marks the start and end of every packet.
const DELIMITER: u8 = 0x7e;
/// Precedes an escaped byte.
const ESCAPE: u8 = 0x7d;
/// XORed with a byte to escape or unescape it.
const ESCAPE_XOR: u8 = 0x20;
/// Largest encoded packet: every byte escaped, plus both delimiters.
pub const MAX_ENCODED: usize = MAX_RAW * 2 + 2;

/// Reflected polynomial of the IEEE 802.3 CRC32.
const CRC32_POLYNOMIAL: u32 = 0xedb8_8320;

// ---- Packed body records ----------------------------------------------------

/// Tag bit marking a zero-run-compressed record.
const RECORD_COMPRESSED: u8 = 0x80;
/// Tag bits holding the encoded length of a record.
const RECORD_LENGTH_MASK: u8 = 0x7f;
/// Longest record data a one-byte tag can describe.
const RECORD_MAX_SHORT: usize = RECORD_LENGTH_MASK as usize;
/// Tag introducing an extended raw record: tag, length, body.
const RECORD_EXTENDED: u8 = 0;
/// Octets before the body in an extended record.
const RECORD_EXTENDED_HEADER: usize = 2;
/// Octets before the data in an ordinary record.
const RECORD_HEADER: usize = 1;
/// In compressed data, this byte is followed by a run length of zeros.
const ZERO_RUN: u8 = 0;
/// Largest packed record: an extended raw record of a full body.
pub const MAX_RECORD: usize = MAX_BODY + RECORD_EXTENDED_HEADER;
/// Octets in a MAC address.
pub const MAC_LEN: usize = 6;

// ---- Responses --------------------------------------------------------------

/// First byte of a RESPONSE payload when the command succeeded.
pub const RESPONSE_OK: u8 = 0;
/// First byte of a RESPONSE payload when the command was invalid or the device busy.
pub const RESPONSE_REJECTED: u8 = 1;

// ---- JOIN payload -----------------------------------------------------------

/// `PictoChat` rooms A through D.
pub const ROOM_COUNT: usize = 4;
/// Nickname field: ten UTF-16LE code units, zero-padded.
pub const NICKNAME_LEN: usize = 20;
/// Personal message field: 26 UTF-16LE code units, zero-padded.
pub const PROFILE_MESSAGE_LEN: usize = 52;
/// Highest DS profile color index.
pub const MAX_PROFILE_COLOR: u8 = 15;
/// Offset of the room number.
pub const JOIN_ROOM_OFFSET: usize = 0;
/// Offset of the Wi-Fi channel.
pub const JOIN_CHANNEL_OFFSET: usize = 1;
/// Offset of the nickname.
pub const JOIN_NICKNAME_OFFSET: usize = 2;
/// Offset of the personal message.
pub const JOIN_MESSAGE_OFFSET: usize = JOIN_NICKNAME_OFFSET + NICKNAME_LEN;
/// Offset of the profile color.
pub const JOIN_COLOR_OFFSET: usize = JOIN_MESSAGE_OFFSET + PROFILE_MESSAGE_LEN;
/// JOIN payload length.
pub const JOIN_LEN: usize = JOIN_COLOR_OFFSET + 1;

// ---- DISCOVER response ------------------------------------------------------

/// Member count reported for a room whose beacon was not heard recently.
pub const NO_BEACON: u8 = u8::MAX;

// ---- STATUS payload ---------------------------------------------------------

/// Offset of the client state.
pub const STATUS_STATE_OFFSET: usize = 0;
/// Offset of the profile-exchange phase.
pub const STATUS_PHASE_OFFSET: usize = 1;
/// Offset of the number of bodies sent and acknowledged.
pub const STATUS_SENT_OFFSET: usize = 2;
/// Offset of the body count of the current upload.
pub const STATUS_COUNT_OFFSET: usize = 3;
/// Offset of the participant slot.
pub const STATUS_SLOT_OFFSET: usize = 4;
/// Offset of the room number.
pub const STATUS_ROOM_OFFSET: usize = 5;
/// Offset of the delivered flag.
pub const STATUS_DELIVERED_OFFSET: usize = 6;
/// Offset of the station MAC address.
pub const STATUS_STATION_OFFSET: usize = 8;
/// Offset of the host MAC address.
pub const STATUS_HOST_OFFSET: usize = STATUS_STATION_OFFSET + MAC_LEN;
/// Offset of the first little-endian u32 counter.
pub const STATUS_COUNTERS_OFFSET: usize = STATUS_HOST_OFFSET + MAC_LEN;
/// Counters: polls, replies, positive ACKs, negative ACKs, disconnect reason,
/// echoed fragments.
pub const STATUS_COUNTERS: usize = 6;
/// Index of the echoed-fragments counter.
pub const STATUS_ECHOED_COUNTER: usize = 5;
/// STATUS payload length.
pub const STATUS_LEN: usize = STATUS_COUNTERS_OFFSET + STATUS_COUNTERS * size_of::<u32>();

/// State value: no session.
pub const STATE_OFF: u8 = 0;
/// State value: associated with the DS host.
pub const STATE_CONNECTED: u8 = 4;
/// Phase value: profile exchange complete, drawings may be sent.
pub const PHASE_READY: u8 = 12;
/// Bodies in every upload that are not fragments: announcement and completion.
pub const CONTROL_BODIES: u8 = 2;

pub const HELLO: u8 = 1;
pub const JOIN: u8 = 2;
pub const LEAVE: u8 = 3;
pub const PING: u8 = 4;
pub const BEGIN: u8 = 5;
/// Batch of packed application bodies.
pub const BODIES: u8 = 6;
pub const COMMIT: u8 = 7;
/// Read recent room beacon counts while idle, on a selected channel.
pub const DISCOVER: u8 = 8;
pub const RESPONSE: u8 = 128;
pub const STATUS: u8 = 129;
pub const RX: u8 = 130;
pub const GAP: u8 = 131;
/// Radio timing counters, emitted only by `PICTO_DIAGNOSTICS` firmware builds.
pub const DIAG: u8 = 132;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Packet {
    pub kind: u8,
    pub id: u32,
    pub len: usize,
    pub payload: [u8; MAX_PAYLOAD],
}
impl Packet {
    pub fn new(kind: u8, id: u32, data: &[u8]) -> Option<Self> {
        if data.len() > MAX_PAYLOAD {
            return None;
        }
        let mut p = Self {
            kind,
            id,
            len: data.len(),
            payload: [0; MAX_PAYLOAD],
        };
        p.payload[..data.len()].copy_from_slice(data);
        Some(p)
    }
    pub fn data(&self) -> &[u8] {
        &self.payload[..self.len]
    }
}
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
pub fn encode(packet: &Packet, output: &mut [u8; MAX_ENCODED]) -> usize {
    let payload_end = PAYLOAD_OFFSET + packet.len;
    let raw_len = payload_end + CRC_LEN;
    let mut raw = [0; MAX_RAW];
    raw[VERSION_OFFSET] = VERSION;
    raw[KIND_OFFSET] = packet.kind;
    raw[ID_OFFSET..LENGTH_OFFSET].copy_from_slice(&packet.id.to_le_bytes());
    raw[LENGTH_OFFSET..PAYLOAD_OFFSET].copy_from_slice(&(packet.len as u16).to_le_bytes());
    raw[PAYLOAD_OFFSET..payload_end].copy_from_slice(packet.data());
    let sum = crc32(&raw[..payload_end]);
    raw[payload_end..raw_len].copy_from_slice(&sum.to_le_bytes());

    output[0] = DELIMITER;
    let mut n = 1;
    for &b in &raw[..raw_len] {
        if matches!(b, DELIMITER | ESCAPE) {
            output[n] = ESCAPE;
            n += 1;
            output[n] = b ^ ESCAPE_XOR;
            n += 1;
        } else {
            output[n] = b;
            n += 1;
        }
    }
    output[n] = DELIMITER;
    n + 1
}
pub struct Decoder {
    raw: [u8; MAX_RAW],
    len: usize,
    escape: bool,
    active: bool,
}
impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}
impl Decoder {
    pub const fn new() -> Self {
        Self {
            raw: [0; MAX_RAW],
            len: 0,
            escape: false,
            active: false,
        }
    }
    pub fn push(&mut self, byte: u8) -> Option<Packet> {
        if byte == DELIMITER {
            let result = if self.active && !self.escape {
                self.finish()
            } else {
                None
            };
            self.len = 0;
            self.escape = false;
            self.active = true;
            return result;
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
        if self.len == self.raw.len() {
            self.active = false;
            return None;
        }
        self.raw[self.len] = byte;
        self.len += 1;
        None
    }
    fn finish(&self) -> Option<Packet> {
        if self.len < OVERHEAD || self.raw[VERSION_OFFSET] != VERSION {
            return None;
        }
        let size = usize::from(u16::from_le_bytes(
            self.raw[LENGTH_OFFSET..PAYLOAD_OFFSET].try_into().ok()?,
        ));
        if size > MAX_PAYLOAD || self.len != size + OVERHEAD {
            return None;
        }
        let payload_end = PAYLOAD_OFFSET + size;
        let crc = u32::from_le_bytes(self.raw[payload_end..self.len].try_into().ok()?);
        if crc32(&self.raw[..payload_end]) != crc {
            return None;
        }
        Packet::new(
            self.raw[KIND_OFFSET],
            u32::from_le_bytes(self.raw[ID_OFFSET..LENGTH_OFFSET].try_into().ok()?),
            &self.raw[PAYLOAD_OFFSET..payload_end],
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_recovers_after_noise_corruption_and_truncation() {
        let data = core::array::from_fn::<_, 256, _>(|i| i as u8);
        let packet = Packet::new(BODIES, 0x7e7d1234, &data).unwrap();
        let mut wire = [0; MAX_ENCODED];
        let n = encode(&packet, &mut wire);
        let mut decoder = Decoder::new();
        for b in b"boot log\r\n" {
            assert!(decoder.push(*b).is_none());
        }
        for b in &wire[..n / 2] {
            assert!(decoder.push(*b).is_none());
        }
        let mut got = None;
        for &b in &wire[..n] {
            got = decoder.push(b).or(got);
        }
        assert_eq!(got, Some(packet));
        wire[10] ^= 1;
        for &b in &wire[..n] {
            assert!(decoder.push(b).is_none());
        }
    }
    #[test]
    fn crc_matches_standard_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }
}

/// Encodes one body as a length-tagged record, using zero runs when smaller.
///
/// - Tag 1..=127: that many raw bytes follow.
/// - Tag with [`RECORD_COMPRESSED`] set: the low bits give the encoded length;
///   nonzero bytes are literal and `0, N` expands to N zeros.
/// - Tag 0: extended raw record `0, length, body` for bodies too long for a tag.
pub fn pack_body(body: &[u8], output: &mut [u8; MAX_RECORD]) -> Option<usize> {
    if !(MIN_BODY..=MAX_BODY).contains(&body.len()) {
        return None;
    }
    // Worst case: every byte is an isolated zero, encoded as two bytes.
    let mut packed = [0; MAX_BODY * 2];
    let (mut i, mut n) = (0, 0);
    while i < body.len() {
        if body[i] == 0 {
            let start = i;
            while i < body.len() && body[i] == 0 {
                i += 1;
            }
            packed[n] = ZERO_RUN;
            n += 1;
            packed[n] = (i - start) as u8;
            n += 1;
        } else {
            packed[n] = body[i];
            n += 1;
            i += 1;
        }
    }
    if n < body.len() && n <= RECORD_MAX_SHORT {
        output[0] = RECORD_COMPRESSED | n as u8;
        output[RECORD_HEADER..RECORD_HEADER + n].copy_from_slice(&packed[..n]);
        Some(RECORD_HEADER + n)
    } else if body.len() > RECORD_MAX_SHORT {
        output[0] = RECORD_EXTENDED;
        output[1] = body.len() as u8;
        output[RECORD_EXTENDED_HEADER..RECORD_EXTENDED_HEADER + body.len()].copy_from_slice(body);
        Some(RECORD_EXTENDED_HEADER + body.len())
    } else {
        output[0] = body.len() as u8;
        output[RECORD_HEADER..RECORD_HEADER + body.len()].copy_from_slice(body);
        Some(RECORD_HEADER + body.len())
    }
}

/// Returns the first complete record's wire length, including its header.
pub fn packed_record_len(records: &[u8]) -> Option<usize> {
    let tag = *records.first()?;
    let n = if tag == RECORD_EXTENDED {
        let len = usize::from(*records.get(1)?);
        if !(RECORD_MAX_SHORT + 1..=MAX_BODY).contains(&len) {
            return None;
        }
        RECORD_EXTENDED_HEADER + len
    } else {
        RECORD_HEADER + usize::from(tag & RECORD_LENGTH_MASK)
    };
    (records.len() >= n).then_some(n)
}

/// Decodes one complete length-tagged record into a bounded application body.
pub fn unpack_body(record: &[u8], output: &mut [u8; MAX_BODY]) -> Option<usize> {
    if packed_record_len(record)? != record.len() {
        return None;
    }
    let tag = *record.first()?;
    let bytes = if tag == RECORD_EXTENDED {
        &record[RECORD_EXTENDED_HEADER..]
    } else {
        &record[RECORD_HEADER..]
    };
    if tag & RECORD_COMPRESSED == 0 {
        if !(MIN_BODY..=MAX_BODY).contains(&bytes.len()) {
            return None;
        }
        output[..bytes.len()].copy_from_slice(bytes);
        return Some(bytes.len());
    }
    let (mut i, mut n) = (0, 0);
    while i < bytes.len() {
        let value = bytes[i];
        i += 1;
        let count = if value == ZERO_RUN {
            let count = usize::from(*bytes.get(i)?);
            i += 1;
            count
        } else {
            1
        };
        if count == 0 || n + count > MAX_BODY {
            return None;
        }
        output[n..n + count].fill(value);
        n += count;
    }
    (n >= MIN_BODY).then_some(n)
}

#[cfg(test)]
mod packed_tests {
    use super::*;
    #[test]
    fn packed_bodies_roundtrip_for_every_supported_length() {
        for len in 16..=MAX_BODY {
            for salt in 0..4 {
                let mut body = [0; MAX_BODY];
                for (i, b) in body[..len].iter_mut().enumerate() {
                    *b = match salt {
                        0 => 0,
                        1 => 0x7e,
                        2 => (i % 2) as u8,
                        _ => i as u8,
                    };
                }
                let mut record = [0; MAX_RECORD];
                let n = pack_body(&body[..len], &mut record).unwrap();
                assert!(n <= len + 2);
                let mut decoded = [0; MAX_BODY];
                assert_eq!(unpack_body(&record[..n], &mut decoded), Some(len));
                assert_eq!(&decoded[..len], &body[..len]);
            }
        }
    }
    #[test]
    fn malformed_or_oversized_runs_are_rejected() {
        let mut out = [0; MAX_BODY];
        for bytes in [
            &[0x81, 0][..],
            &[0x82, 0, 0],
            &[0x82, 0, MAX_BODY as u8 + 1],
            &[0x82, 0, 15],
            &[0x82, 0],
            &[0x90, 1],
        ] {
            assert_eq!(unpack_body(bytes, &mut out), None);
        }
    }

    #[test]
    fn extended_records_require_a_complete_bounded_body() {
        let mut out = [0; MAX_BODY];
        for bytes in [&[0][..], &[0, 127], &[0, MAX_BODY as u8 + 1], &[0, 128, 1]] {
            assert_eq!(packed_record_len(bytes), None);
            assert_eq!(unpack_body(bytes, &mut out), None);
        }
        let mut record = [0x55; MAX_BODY + 3];
        record[0] = 0;
        record[1] = MAX_BODY as u8;
        assert_eq!(packed_record_len(&record), Some(MAX_BODY + 2));
        // A stream may contain another record, but the single-record decoder
        // must not accept trailing bytes as part of this body.
        assert_eq!(unpack_body(&record, &mut out), None);
        assert_eq!(
            unpack_body(&record[..MAX_BODY + 2], &mut out),
            Some(MAX_BODY)
        );
    }
}

const _: () = {
    // Must match LINK_JOIN_LEN and LINK_STATUS_LEN in firmware/esp32/main/link.h.
    assert!(JOIN_LEN == 75);
    assert!(STATUS_LEN == 44);
};
