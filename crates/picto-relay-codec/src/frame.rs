use std::fmt;

use crate::{DecodeError, read_u16, too_short};

// ---- Field sizes -----------------------------------------------------------

/// Octets in an IEEE 802 MAC address.
pub const MAC_ADDRESS_LEN: usize = 6;
/// Octets in the frame check sequence that may trail a captured frame.
const FCS_LEN: usize = 4;
/// Octets in the `QoS` control field appended to `QoS` data headers.
const QOS_CONTROL_LEN: usize = 2;

// ---- MAC header layout -----------------------------------------------------

/// Offset of the frame-control field.
const FRAME_CONTROL_OFFSET: usize = 0;
/// Offset of the duration/ID field.
pub(crate) const DURATION_OFFSET: usize = 2;
/// Offset of the receiver address.
const ADDRESS_1_OFFSET: usize = 4;
/// Offset of the transmitter address.
const ADDRESS_2_OFFSET: usize = ADDRESS_1_OFFSET + MAC_ADDRESS_LEN;
/// Offset of the BSSID/third address.
const ADDRESS_3_OFFSET: usize = ADDRESS_2_OFFSET + MAC_ADDRESS_LEN;
/// Offset of the sequence-control field.
const SEQUENCE_CONTROL_OFFSET: usize = ADDRESS_3_OFFSET + MAC_ADDRESS_LEN;
/// Offset of the fourth address in WDS data frames.
const ADDRESS_4_OFFSET: usize = HEADER_LEN;

/// Shortest header: frame control, duration and one address (ACK, CTS).
const RECEIVER_ONLY_HEADER_LEN: usize = ADDRESS_2_OFFSET;
/// Control-frame header carrying receiver and transmitter addresses.
const TWO_ADDRESS_HEADER_LEN: usize = ADDRESS_3_OFFSET;
/// Header of management frames and three-address data frames.
pub(crate) const HEADER_LEN: usize = SEQUENCE_CONTROL_OFFSET + 2;

// ---- Frame-control field ---------------------------------------------------

/// Bit position of the two-bit frame type.
const TYPE_SHIFT: u32 = 2;
/// Mask of the frame type after shifting.
const TYPE_MASK: u16 = 0x3;
/// Frame type: management.
const TYPE_MANAGEMENT: u16 = 0;
/// Frame type: control.
const TYPE_CONTROL: u16 = 1;
/// Frame type: data.
const TYPE_DATA: u16 = 2;
/// Bit position of the four-bit subtype.
const SUBTYPE_SHIFT: u32 = 4;
/// Mask of the subtype after shifting.
const SUBTYPE_MASK: u16 = 0x0f;
/// Flag: frame travels to the distribution system.
const FLAG_TO_DS: u16 = 1 << 8;
/// Flag: frame travels from the distribution system.
const FLAG_FROM_DS: u16 = 1 << 9;
/// Flag: retransmission.
const FLAG_RETRY: u16 = 1 << 11;
/// Flag: power management. A DS station sets it on everything it sends.
const FLAG_POWER_MANAGEMENT: u16 = 1 << 12;

/// Management subtype: association request.
pub(crate) const SUBTYPE_ASSOCIATION_REQUEST: u8 = 0;
/// Management subtype: association response.
pub(crate) const SUBTYPE_ASSOCIATION_RESPONSE: u8 = 1;
/// Management subtype: beacon.
pub(crate) const SUBTYPE_BEACON: u8 = 8;
/// Management subtype: authentication.
pub(crate) const SUBTYPE_AUTHENTICATION: u8 = 11;
/// Control subtype: clear to send.
const SUBTYPE_CTS: u8 = 0x0c;
/// Control subtype: acknowledgement.
const SUBTYPE_ACK: u8 = 0x0d;
/// Data subtype: Data + CF-Ack, used by Nintendo client replies and host acknowledgements.
pub(crate) const SUBTYPE_DATA_CF_ACK: u8 = 1;
/// Data subtype: Data + CF-Poll, used by Nintendo host polls.
pub(crate) const SUBTYPE_DATA_CF_POLL: u8 = 2;
/// Data subtype bit marking a `QoS` data frame.
const SUBTYPE_QOS_BIT: u8 = 0x08;

// ---- Sequence-control field ------------------------------------------------

/// Bit position of the twelve-bit sequence number.
const SEQUENCE_NUMBER_SHIFT: u32 = 4;
/// Mask of the four-bit fragment number.
const FRAGMENT_NUMBER_MASK: u16 = 0x0f;
/// Mask of the sequence number within the field.
const SEQUENCE_NUMBER_MASK: u16 = !FRAGMENT_NUMBER_MASK;

// ---- Values a Nintendo DS puts in its frames -------------------------------

/// Duration field of DS management frames, in microseconds.
const MANAGEMENT_DURATION: u16 = 0x00a2;
/// Duration field of frames that reserve no airtime.
const NO_DURATION: u16 = 0;
/// Reply window a host poll reserves for one client, in microseconds.
const REPLY_WINDOW_MICROSECONDS: u16 = 1030;
/// Airtime of one payload byte at 2 Mbps, in microseconds.
const MICROSECONDS_PER_BYTE: u16 = 4;

/// Authentication algorithm: open system.
const AUTHENTICATION_OPEN_SYSTEM: u16 = 0;
/// Authentication transaction number of the station's request.
const AUTHENTICATION_REQUEST_TRANSACTION: u16 = 1;
/// Authentication transaction number of the host's response.
const AUTHENTICATION_RESPONSE_TRANSACTION: u16 = 2;
/// IEEE status code: success.
const STATUS_SUCCESS: u16 = 0;

/// Capability flags used by DS hosts and stations: ESS and short preamble.
pub(crate) const CAPABILITY_ESS_SHORT_PREAMBLE: u16 = 0x0021;
/// Listen interval a DS station requests, in beacon intervals.
const LISTEN_INTERVAL: u16 = 1;
/// Information element: SSID.
const ELEMENT_SSID: u8 = 0;
/// Information element: supported rates.
pub(crate) const ELEMENT_SUPPORTED_RATES: u8 = 1;
/// Length of the binary SSID a DS station sends when associating.
const BINARY_SSID_LEN: usize = 32;
/// Supported rates: 1 and 2 Mbps, both marked basic (high bit).
pub(crate) const BASIC_RATES_1_2_MBPS: [u8; 2] = [0x82, 0x84];
/// Flag bits a DS host sets above the slot in an association ID.
const ASSOCIATION_ID_FLAGS: u16 = 0xc000;
/// Mask of the participant slot within an association ID.
pub(crate) const ASSOCIATION_SLOT_MASK: u16 = 0x0f;

/// Nintendo host-to-client multiplayer destination.
pub const HOST_TO_CLIENT: MacAddress = MacAddress([0x03, 0x09, 0xbf, 0x00, 0x00, 0x00]);
/// Nintendo host acknowledgement destination.
pub const CLIENT_ACK: MacAddress = MacAddress([0x03, 0x09, 0xbf, 0x00, 0x00, 0x03]);
/// Nintendo client-to-host multiplayer destination.
pub const CLIENT_REPLY: MacAddress = MacAddress([0x03, 0x09, 0xbf, 0x00, 0x00, 0x10]);
/// IEEE 802.11 broadcast address.
pub const BROADCAST: MacAddress = MacAddress([u8::MAX; MAC_ADDRESS_LEN]);

/// A 48-bit IEEE 802 MAC address.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacAddress(pub [u8; MAC_ADDRESS_LEN]);

impl MacAddress {
    /// Returns the raw six address octets.
    #[must_use]
    pub const fn octets(self) -> [u8; MAC_ADDRESS_LEN] {
        self.0
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, octet) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str(":")?;
            }
            write!(formatter, "{octet:02x}")?;
        }
        Ok(())
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
        match (self.0 >> TYPE_SHIFT) & TYPE_MASK {
            TYPE_MANAGEMENT => FrameType::Management,
            TYPE_CONTROL => FrameType::Control,
            TYPE_DATA => FrameType::Data,
            _ => FrameType::Reserved,
        }
    }

    /// Returns the four-bit subtype.
    #[must_use]
    pub const fn subtype(self) -> u8 {
        ((self.0 >> SUBTYPE_SHIFT) & SUBTYPE_MASK) as u8
    }

    /// Reports whether the frame is a retransmission.
    #[must_use]
    pub const fn retry(self) -> bool {
        self.0 & FLAG_RETRY != 0
    }

    /// Reports whether the frame travels to the distribution system.
    #[must_use]
    pub const fn to_ds(self) -> bool {
        self.0 & FLAG_TO_DS != 0
    }

    /// Reports whether the frame travels from the distribution system.
    #[must_use]
    pub const fn from_ds(self) -> bool {
        self.0 & FLAG_FROM_DS != 0
    }
}

/// Decoded IEEE 802.11 sequence-control field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SequenceControl(pub u16);

impl SequenceControl {
    /// Twelve-bit sequence number.
    #[must_use]
    pub const fn sequence_number(self) -> u16 {
        self.0 >> SEQUENCE_NUMBER_SHIFT
    }

    /// Four-bit IEEE 802.11 fragment number.
    #[must_use]
    pub const fn fragment_number(self) -> u8 {
        (self.0 & FRAGMENT_NUMBER_MASK) as u8
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
        if raw.len() < RECEIVER_ONLY_HEADER_LEN {
            return Err(too_short(
                "IEEE 802.11 frame",
                RECEIVER_ONLY_HEADER_LEN,
                raw.len(),
            ));
        }
        let frame_control = FrameControl(read_u16(raw, FRAME_CONTROL_OFFSET, "IEEE 802.11 frame")?);
        let duration = read_u16(raw, DURATION_OFFSET, "IEEE 802.11 frame")?;
        let address_1 = read_mac(raw, ADDRESS_1_OFFSET, "IEEE 802.11 receiver address")?;
        let mut address_2 = None;
        let mut address_3 = None;
        let mut sequence_control = None;
        let mut address_4 = None;

        let header_len = match frame_control.frame_type() {
            FrameType::Management => HEADER_LEN,
            FrameType::Control => control_header_len(frame_control.subtype()),
            FrameType::Data => {
                let mut length = HEADER_LEN;
                if frame_control.to_ds() && frame_control.from_ds() {
                    length += MAC_ADDRESS_LEN;
                }
                if frame_control.subtype() & SUBTYPE_QOS_BIT != 0 {
                    length += QOS_CONTROL_LEN;
                }
                length
            }
            FrameType::Reserved => RECEIVER_ONLY_HEADER_LEN,
        };
        if raw.len() < header_len {
            return Err(too_short("IEEE 802.11 MAC header", header_len, raw.len()));
        }
        if header_len >= TWO_ADDRESS_HEADER_LEN {
            address_2 = Some(read_mac(
                raw,
                ADDRESS_2_OFFSET,
                "IEEE 802.11 transmitter address",
            )?);
        }
        if matches!(
            frame_control.frame_type(),
            FrameType::Management | FrameType::Data
        ) {
            address_3 = Some(read_mac(
                raw,
                ADDRESS_3_OFFSET,
                "IEEE 802.11 third address",
            )?);
            sequence_control = Some(SequenceControl(read_u16(
                raw,
                SEQUENCE_CONTROL_OFFSET,
                "IEEE 802.11 sequence control",
            )?));
        }
        if frame_control.frame_type() == FrameType::Data
            && frame_control.to_ds()
            && frame_control.from_ds()
        {
            address_4 = Some(read_mac(
                raw,
                ADDRESS_4_OFFSET,
                "IEEE 802.11 fourth address",
            )?);
        }

        let fcs_len = match fcs_mode {
            FcsMode::Present => FCS_LEN,
            FcsMode::Absent => 0,
        };
        let body_end = raw.len().checked_sub(fcs_len).ok_or_else(|| {
            too_short(
                "IEEE 802.11 frame with FCS",
                header_len + FCS_LEN,
                raw.len(),
            )
        })?;
        if body_end < header_len {
            return Err(too_short(
                "IEEE 802.11 frame body",
                header_len + fcs_len,
                raw.len(),
            ));
        }

        Ok(Self {
            raw: raw.to_vec(),
            header_len,
            body_end,
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
    let mut control = frame_control(TYPE_DATA, subtype);
    if retry {
        control |= FLAG_RETRY;
    }
    build_24_byte_frame(
        control,
        NO_DURATION,
        destination,
        source,
        bssid,
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
        .saturating_mul(MICROSECONDS_PER_BYTE);
    let duration = REPLY_WINDOW_MICROSECONDS.saturating_sub(payload_airtime);
    let control =
        frame_control(TYPE_DATA, SUBTYPE_DATA_CF_ACK) | FLAG_TO_DS | FLAG_POWER_MANAGEMENT;
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
    build_24_byte_frame(
        frame_control(TYPE_MANAGEMENT, subtype),
        NO_DURATION,
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
    let mut control =
        frame_control(TYPE_MANAGEMENT, SUBTYPE_AUTHENTICATION) | FLAG_POWER_MANAGEMENT;
    if retry {
        control |= FLAG_RETRY;
    }
    build_24_byte_frame(
        control,
        MANAGEMENT_DURATION,
        host,
        station,
        host,
        sequence_number,
        &authentication_body(AUTHENTICATION_REQUEST_TRANSACTION),
    )
}

/// Builds the successful open-system response emitted by a Nintendo room host.
#[must_use]
pub fn build_authentication_response(
    host: MacAddress,
    station: MacAddress,
    sequence_number: u16,
) -> Vec<u8> {
    build_24_byte_frame(
        frame_control(TYPE_MANAGEMENT, SUBTYPE_AUTHENTICATION),
        MANAGEMENT_DURATION,
        station,
        host,
        host,
        sequence_number,
        &authentication_body(AUTHENTICATION_RESPONSE_TRANSACTION),
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
    // The SSID is binary: the session's game ID then stream code, zero-padded.
    let game_id = game_id.to_le_bytes();
    let stream_code = stream_code.to_le_bytes();
    let mut binary_ssid = [0; BINARY_SSID_LEN];
    binary_ssid[..game_id.len()].copy_from_slice(&game_id);
    binary_ssid[game_id.len()..game_id.len() + stream_code.len()].copy_from_slice(&stream_code);

    let mut body = Vec::new();
    body.extend_from_slice(&CAPABILITY_ESS_SHORT_PREAMBLE.to_le_bytes());
    body.extend_from_slice(&LISTEN_INTERVAL.to_le_bytes());
    append_element(&mut body, ELEMENT_SSID, &binary_ssid);
    append_element(&mut body, ELEMENT_SUPPORTED_RATES, &BASIC_RATES_1_2_MBPS);
    let mut control =
        frame_control(TYPE_MANAGEMENT, SUBTYPE_ASSOCIATION_REQUEST) | FLAG_POWER_MANAGEMENT;
    if retry {
        control |= FLAG_RETRY;
    }
    build_24_byte_frame(
        control,
        MANAGEMENT_DURATION,
        host,
        station,
        host,
        sequence_number,
        &body,
    )
}

/// Builds a successful Nintendo local-multiplayer association response.
#[must_use]
pub fn build_association_response(
    host: MacAddress,
    station: MacAddress,
    slot: u8,
    sequence_number: u16,
) -> Vec<u8> {
    let association_id = ASSOCIATION_ID_FLAGS | (u16::from(slot) & ASSOCIATION_SLOT_MASK);
    let mut body = Vec::new();
    body.extend_from_slice(&CAPABILITY_ESS_SHORT_PREAMBLE.to_le_bytes());
    body.extend_from_slice(&STATUS_SUCCESS.to_le_bytes());
    body.extend_from_slice(&association_id.to_le_bytes());
    append_element(&mut body, ELEMENT_SUPPORTED_RATES, &BASIC_RATES_1_2_MBPS);
    build_24_byte_frame(
        frame_control(TYPE_MANAGEMENT, SUBTYPE_ASSOCIATION_RESPONSE),
        MANAGEMENT_DURATION,
        station,
        host,
        host,
        sequence_number,
        &body,
    )
}

/// Composes a frame-control value from a frame type and subtype.
fn frame_control(frame_type: u16, subtype: u8) -> u16 {
    (frame_type << TYPE_SHIFT) | ((u16::from(subtype) & SUBTYPE_MASK) << SUBTYPE_SHIFT)
}

/// Fixed fields of a successful open-system authentication frame.
fn authentication_body(transaction: u16) -> Vec<u8> {
    [AUTHENTICATION_OPEN_SYSTEM, transaction, STATUS_SUCCESS]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// Appends an information element. Callers pass fixed-size data; anything
/// longer than the one-octet length field can express is a programming error.
fn append_element(output: &mut Vec<u8>, id: u8, data: &[u8]) {
    let length = u8::try_from(data.len()).expect("information element fits its length octet");
    output.push(id);
    output.push(length);
    output.extend_from_slice(data);
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
    let mut frame = Vec::with_capacity(HEADER_LEN + body.len());
    frame.extend_from_slice(&control.to_le_bytes());
    frame.extend_from_slice(&duration.to_le_bytes());
    frame.extend_from_slice(&destination.0);
    frame.extend_from_slice(&source.0);
    frame.extend_from_slice(&bssid.0);
    frame.extend_from_slice(
        &(sequence_number.wrapping_shl(SEQUENCE_NUMBER_SHIFT) & SEQUENCE_NUMBER_MASK).to_le_bytes(),
    );
    frame.extend_from_slice(body);
    frame
}

fn control_header_len(subtype: u8) -> usize {
    match subtype {
        SUBTYPE_CTS | SUBTYPE_ACK => RECEIVER_ONLY_HEADER_LEN,
        _ => TWO_ADDRESS_HEADER_LEN,
    }
}

fn read_mac(data: &[u8], offset: usize, context: &'static str) -> Result<MacAddress, DecodeError> {
    let end = offset + MAC_ADDRESS_LEN;
    data.get(offset..end)
        .and_then(|bytes| bytes.try_into().ok())
        .map(MacAddress)
        .ok_or_else(|| too_short(context, end, data.len()))
}
