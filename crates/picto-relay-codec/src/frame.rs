use std::fmt;

use crate::{DecodeError, read_u16, too_short};

/// Nintendo host-to-client multiplayer destination.
pub const HOST_TO_CLIENT: MacAddress = MacAddress([0x03, 0x09, 0xbf, 0x00, 0x00, 0x00]);
/// Nintendo host acknowledgement destination.
pub const CLIENT_ACK: MacAddress = MacAddress([0x03, 0x09, 0xbf, 0x00, 0x00, 0x03]);
/// Nintendo client-to-host multiplayer destination.
pub const CLIENT_REPLY: MacAddress = MacAddress([0x03, 0x09, 0xbf, 0x00, 0x00, 0x10]);
/// IEEE 802.11 broadcast address.
pub const BROADCAST: MacAddress = MacAddress([u8::MAX; 6]);

/// A 48-bit IEEE 802 MAC address.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacAddress(pub [u8; 6]);

impl MacAddress {
    /// Returns the raw six address octets.
    #[must_use]
    pub const fn octets(self) -> [u8; 6] {
        self.0
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

/// IEEE 802.11 frame class from the frame-control field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameType {
    /// Management frame.
    Management,
    /// Control frame.
    Control,
    /// Data frame.
    Data,
    /// Reserved frame class.
    Reserved,
}

/// Decoded IEEE 802.11 frame-control field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameControl(pub u16);

impl FrameControl {
    /// Returns the frame class.
    #[must_use]
    pub const fn frame_type(self) -> FrameType {
        match (self.0 >> 2) & 0x3 {
            0 => FrameType::Management,
            1 => FrameType::Control,
            2 => FrameType::Data,
            _ => FrameType::Reserved,
        }
    }

    /// Returns the four-bit subtype.
    #[must_use]
    pub const fn subtype(self) -> u8 {
        ((self.0 >> 4) & 0x0f) as u8
    }

    /// Reports whether the frame is a retransmission.
    #[must_use]
    pub const fn retry(self) -> bool {
        self.0 & (1 << 11) != 0
    }

    /// Reports whether the frame has more IEEE 802.11 fragments.
    #[must_use]
    pub const fn more_fragments(self) -> bool {
        self.0 & (1 << 10) != 0
    }

    /// Reports whether the frame travels to the distribution system.
    #[must_use]
    pub const fn to_ds(self) -> bool {
        self.0 & (1 << 8) != 0
    }

    /// Reports whether the frame travels from the distribution system.
    #[must_use]
    pub const fn from_ds(self) -> bool {
        self.0 & (1 << 9) != 0
    }
}

/// Decoded IEEE 802.11 sequence-control field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SequenceControl(pub u16);

impl SequenceControl {
    /// Twelve-bit sequence number.
    #[must_use]
    pub const fn sequence_number(self) -> u16 {
        self.0 >> 4
    }

    /// Four-bit IEEE 802.11 fragment number.
    #[must_use]
    pub const fn fragment_number(self) -> u8 {
        (self.0 & 0x0f) as u8
    }
}

/// Parsed portions of an IEEE 802.11 MAC header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MacHeader {
    /// Frame-control flags and subtype.
    pub frame_control: FrameControl,
    /// Duration or association identifier field.
    pub duration: u16,
    /// Receiver/destination address when present.
    pub address_1: MacAddress,
    /// Transmitter/source address when present.
    pub address_2: Option<MacAddress>,
    /// BSSID or additional address when present.
    pub address_3: Option<MacAddress>,
    /// Sequence information when present.
    pub sequence_control: Option<SequenceControl>,
    /// Fourth address used by WDS/mesh data frames.
    pub address_4: Option<MacAddress>,
}

/// A lossless raw IEEE 802.11 frame with a decoded MAC header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MacFrame {
    raw: Vec<u8>,
    header_len: usize,
    body_end: usize,
    has_fcs: bool,
    /// Decoded header fields.
    pub header: MacHeader,
}

/// Whether an input frame includes its trailing four-byte FCS.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FcsMode {
    /// Input ends at the frame body.
    Absent,
    /// Input includes a four-byte FCS.
    Present,
}

impl MacFrame {
    /// Parses a raw IEEE 802.11 frame without discarding unknown bytes.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the frame is shorter than the header implied
    /// by its type and flags.
    pub fn parse(raw: &[u8], fcs_mode: FcsMode) -> Result<Self, DecodeError> {
        if raw.len() < 10 {
            return Err(too_short("IEEE 802.11 frame", 10, raw.len()));
        }
        let frame_control = FrameControl(read_u16(raw, 0, "IEEE 802.11 frame")?);
        let duration = read_u16(raw, 2, "IEEE 802.11 frame")?;
        let address_1 = read_mac(raw, 4, "IEEE 802.11 receiver address")?;
        let mut address_2 = None;
        let mut address_3 = None;
        let mut sequence_control = None;
        let mut address_4 = None;

        let header_len = match frame_control.frame_type() {
            FrameType::Management => 24,
            FrameType::Control => control_header_len(frame_control.subtype()),
            FrameType::Data => {
                let mut length = 24;
                if frame_control.to_ds() && frame_control.from_ds() {
                    length += 6;
                }
                if frame_control.subtype() & 0x08 != 0 {
                    length += 2;
                }
                length
            }
            FrameType::Reserved => 10,
        };
        if raw.len() < header_len {
            return Err(too_short("IEEE 802.11 MAC header", header_len, raw.len()));
        }
        if header_len >= 16 {
            address_2 = Some(read_mac(raw, 10, "IEEE 802.11 transmitter address")?);
        }
        if matches!(
            frame_control.frame_type(),
            FrameType::Management | FrameType::Data
        ) {
            address_3 = Some(read_mac(raw, 16, "IEEE 802.11 third address")?);
            sequence_control = Some(SequenceControl(read_u16(
                raw,
                22,
                "IEEE 802.11 sequence control",
            )?));
        }
        if frame_control.frame_type() == FrameType::Data
            && frame_control.to_ds()
            && frame_control.from_ds()
        {
            address_4 = Some(read_mac(raw, 24, "IEEE 802.11 fourth address")?);
        }

        let has_fcs = fcs_mode == FcsMode::Present;
        let body_end = raw
            .len()
            .checked_sub(usize::from(has_fcs) * 4)
            .ok_or_else(|| too_short("IEEE 802.11 frame with FCS", header_len + 4, raw.len()))?;
        if body_end < header_len {
            return Err(too_short(
                "IEEE 802.11 frame body",
                header_len + usize::from(has_fcs) * 4,
                raw.len(),
            ));
        }

        Ok(Self {
            raw: raw.to_vec(),
            header_len,
            body_end,
            has_fcs,
            header: MacHeader {
                frame_control,
                duration,
                address_1,
                address_2,
                address_3,
                sequence_control,
                address_4,
            },
        })
    }

    /// Returns the original frame bytes exactly as supplied.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.raw
    }

    /// Returns the frame body, excluding the MAC header and optional FCS.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        &self.raw[self.header_len..self.body_end]
    }

    /// Returns the optional captured FCS bytes.
    #[must_use]
    pub fn fcs(&self) -> Option<[u8; 4]> {
        self.has_fcs.then(|| {
            let bytes = &self.raw[self.body_end..];
            [bytes[0], bytes[1], bytes[2], bytes[3]]
        })
    }
}

/// Builds a 24-byte-header IEEE 802.11 data frame.
#[must_use]
pub fn build_data_frame(
    subtype: u8,
    destination: MacAddress,
    source: MacAddress,
    bssid: MacAddress,
    sequence_number: u16,
    retry: bool,
    body: &[u8],
) -> Vec<u8> {
    let mut control = (2_u16 << 2) | (u16::from(subtype & 0x0f) << 4);
    if retry {
        control |= 1 << 11;
    }
    build_24_byte_frame(
        control,
        0,
        destination,
        source,
        bssid,
        sequence_number,
        body,
    )
}

/// Builds a To-DS data frame with receiver, transmitter, and logical destination ordering.
#[must_use]
pub fn build_data_frame_to_ds(
    subtype: u8,
    receiver: MacAddress,
    source: MacAddress,
    destination: MacAddress,
    sequence_number: u16,
    retry: bool,
    body: &[u8],
) -> Vec<u8> {
    let mut control = (2_u16 << 2) | (u16::from(subtype & 0x0f) << 4) | (1 << 8);
    if retry {
        control |= 1 << 11;
    }
    build_24_byte_frame(
        control,
        0,
        receiver,
        source,
        destination,
        sequence_number,
        body,
    )
}

/// Builds a Nintendo multiplayer client reply with the captured power-management and
/// duration fields. At 2 Mbps, each payload byte consumes four microseconds from the
/// observed 1030-microsecond reply reservation.
#[must_use]
pub fn build_client_reply_frame(
    station: MacAddress,
    host: MacAddress,
    sequence_number: u16,
    body: &[u8],
) -> Vec<u8> {
    let payload_airtime = u16::try_from(body.len())
        .unwrap_or(u16::MAX)
        .saturating_mul(4);
    let duration = 1030_u16.saturating_sub(payload_airtime);
    let control = (2_u16 << 2) | (1 << 4) | (1 << 8) | (1 << 12);
    build_24_byte_frame(
        control,
        duration,
        host,
        station,
        CLIENT_REPLY,
        sequence_number,
        body,
    )
}

/// Builds a 24-byte-header IEEE 802.11 management frame.
#[must_use]
pub fn build_management_frame(
    subtype: u8,
    destination: MacAddress,
    source: MacAddress,
    bssid: MacAddress,
    sequence_number: u16,
    body: &[u8],
) -> Vec<u8> {
    let control = u16::from(subtype & 0x0f) << 4;
    build_24_byte_frame(
        control,
        0,
        destination,
        source,
        bssid,
        sequence_number,
        body,
    )
}

/// Builds an open-system authentication request shaped like a Nintendo DS station.
#[must_use]
pub fn build_authentication_request(
    station: MacAddress,
    host: MacAddress,
    sequence_number: u16,
    retry: bool,
) -> Vec<u8> {
    let mut control = (11_u16 << 4) | (1 << 12);
    if retry {
        control |= 1 << 11;
    }
    build_24_byte_frame(
        control,
        0x00a2,
        host,
        station,
        host,
        sequence_number,
        &[0, 0, 1, 0, 0, 0],
    )
}

/// Builds a Nintendo local-multiplayer association request from beacon session fields.
#[must_use]
pub fn build_association_request(
    station: MacAddress,
    host: MacAddress,
    game_id: u32,
    stream_code: u16,
    sequence_number: u16,
    retry: bool,
) -> Vec<u8> {
    let mut binary_ssid = [0; 32];
    binary_ssid[..4].copy_from_slice(&game_id.to_le_bytes());
    binary_ssid[4..6].copy_from_slice(&stream_code.to_le_bytes());
    let mut body = Vec::with_capacity(42);
    body.extend_from_slice(&0x0021_u16.to_le_bytes());
    body.extend_from_slice(&1_u16.to_le_bytes());
    body.extend_from_slice(&[0, 32]);
    body.extend_from_slice(&binary_ssid);
    body.extend_from_slice(&[1, 2, 0x82, 0x84]);
    let mut control = 1 << 12;
    if retry {
        control |= 1 << 11;
    }
    build_24_byte_frame(control, 0x00a2, host, station, host, sequence_number, &body)
}

fn build_24_byte_frame(
    control: u16,
    duration: u16,
    destination: MacAddress,
    source: MacAddress,
    bssid: MacAddress,
    sequence_number: u16,
    body: &[u8],
) -> Vec<u8> {
    let mut frame = Vec::with_capacity(24 + body.len());
    frame.extend_from_slice(&control.to_le_bytes());
    frame.extend_from_slice(&duration.to_le_bytes());
    frame.extend_from_slice(&destination.0);
    frame.extend_from_slice(&source.0);
    frame.extend_from_slice(&bssid.0);
    frame.extend_from_slice(&(sequence_number.wrapping_shl(4) & 0xfff0).to_le_bytes());
    frame.extend_from_slice(body);
    frame
}

fn control_header_len(subtype: u8) -> usize {
    match subtype {
        0x0c | 0x0d => 10,
        _ => 16,
    }
}

fn read_mac(data: &[u8], offset: usize, context: &'static str) -> Result<MacAddress, DecodeError> {
    let bytes = data
        .get(offset..offset + 6)
        .ok_or_else(|| too_short(context, offset + 6, data.len()))?;
    Ok(MacAddress([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5],
    ]))
}
