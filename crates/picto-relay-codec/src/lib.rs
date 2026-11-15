//! Lossless Nintendo DS local-wireless and `PictoChat` protocol structures.
//!
//! Verified IEEE 802.11 and Nintendo beacon fields are exposed as typed values.
//! Fields whose meaning is not established remain opaque and round-trip without
//! modification.

mod beacon;
mod frame;
mod message;
mod transport;

pub use beacon::{
    AssociationRequest, AssociationResponse, Authentication, Beacon, InformationElement,
    ManagementBody, NintendoBeacon, PictoChatBeacon, PictoChatBeaconConfig,
    PictoChatBeaconTemplate, build_pictochat_beacon, build_pictochat_beacon_template,
    parse_management,
};
pub use frame::{
    BROADCAST, CLIENT_ACK, CLIENT_REPLY, FcsMode, FrameControl, FrameType, HOST_TO_CLIENT,
    MAC_ADDRESS_LEN, MacAddress, MacFrame, MacHeader, SequenceControl, build_association_request,
    build_association_response, build_authentication_request, build_authentication_response,
    build_client_reply_frame, build_data_frame, build_management_frame,
};
pub use message::{
    DRAWING_BAND_BYTES, DRAWING_BAND_HEIGHT, DRAWING_HEIGHT, DRAWING_METADATA_BYTES, DRAWING_WIDTH,
    PictoChatMessage, PixelBounds, encode_message,
};
pub use transport::{
    MESSAGE_FRAGMENT_PAYLOAD_BYTES, MessageReassembler, MultiplayerHeader, NintendoData,
    NintendoFlow, PictoChatEnvelope, PictoChatFragment, PictoChatProfile, PictoChatRoster,
    ReassemblyKey, build_ack_frame, build_fragment_frame, build_host_command_frame,
    decode_nintendo_data, encode_fragment_envelope, fragment_payload, fragment_payload_starting_at,
};

use thiserror::Error;

/// Errors produced while parsing or constructing wireless frames.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum DecodeError {
    /// A structure is shorter than its fixed header.
    #[error("{context} is too short: expected at least {minimum} bytes, received {actual}")]
    TooShort {
        /// Structure being decoded.
        context: &'static str,
        /// Minimum required length.
        minimum: usize,
        /// Actual input length.
        actual: usize,
    },
    /// An information element extends beyond the containing management frame.
    #[error(
        "information element at offset {offset} declares {declared} bytes with only {remaining} remaining"
    )]
    TruncatedInformationElement {
        /// Offset of the element header.
        offset: usize,
        /// Declared element data length.
        declared: usize,
        /// Available bytes after the element header.
        remaining: usize,
    },
    /// A management decoder received a non-management frame.
    #[error("frame is not an IEEE 802.11 management frame")]
    NotManagementFrame,
    /// A Nintendo data decoder received a frame outside the known multiplayer flows.
    #[error("frame is not a recognized Nintendo local-multiplayer data flow")]
    NotNintendoDataFrame,
    /// No Nintendo vendor element was present in the beacon.
    #[error("beacon does not contain a Nintendo vendor information element")]
    MissingNintendoBeacon,
    /// A Nintendo vendor element has an unsupported or malformed layout.
    #[error("Nintendo beacon data is malformed: {0}")]
    InvalidNintendoBeacon(&'static str),
    /// The `PictoChat` room number is outside A through D.
    #[error("invalid PictoChat room number {0}")]
    InvalidRoom(u8),
    /// An application envelope carries inconsistent type or length fields.
    #[error("PictoChat envelope is malformed: {0}")]
    InvalidEnvelope(&'static str),
    /// A payload is not a supported message-fragment packet.
    #[error("PictoChat fragment is malformed: {0}")]
    InvalidFragment(&'static str),
    /// A reassembled message has an unsupported metadata or drawing layout.
    #[error("PictoChat message is malformed: {0}")]
    InvalidMessage(&'static str),
    /// A fragment cannot fit in the observed one-byte packet-length field.
    #[error("fragment contains too much data: {0} bytes")]
    FragmentTooLarge(usize),
    /// A payload offset cannot fit in the observed 16-bit field.
    #[error("fragment offset is too large: {0}")]
    OffsetTooLarge(usize),
    /// An 84-byte identity record has an invalid marker, subtype, or length.
    #[error("PictoChat profile is malformed: {0}")]
    InvalidProfile(&'static str),
    /// A host member/session record has an invalid length.
    #[error("PictoChat roster is malformed: {0}")]
    InvalidRoster(&'static str),
    /// Two fragments claim different data for the same byte range.
    #[error("fragment overlaps previously received data with different contents")]
    ConflictingFragment,
}

pub(crate) fn too_short(context: &'static str, minimum: usize, actual: usize) -> DecodeError {
    DecodeError::TooShort {
        context,
        minimum,
        actual,
    }
}

// Shared wire-format widths.

/// Bytes in one little-endian 16-bit wire word (a Nintendo "halfword").
pub(crate) const U16_BYTES: usize = size_of::<u16>();
/// Bytes in one little-endian 32-bit wire word.
pub(crate) const U32_BYTES: usize = size_of::<u32>();

pub(crate) fn read_u16(
    data: &[u8],
    offset: usize,
    context: &'static str,
) -> Result<u16, DecodeError> {
    let end = offset + U16_BYTES;
    data.get(offset..end)
        .and_then(|bytes| <[u8; U16_BYTES]>::try_from(bytes).ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| too_short(context, end, data.len()))
}

pub(crate) fn read_u32(
    data: &[u8],
    offset: usize,
    context: &'static str,
) -> Result<u32, DecodeError> {
    let end = offset + U32_BYTES;
    data.get(offset..end)
        .and_then(|bytes| <[u8; U32_BYTES]>::try_from(bytes).ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| too_short(context, end, data.len()))
}

/// Reads a MAC address stored as three little-endian 16-bit words, the encoding Nintendo
/// uses inside `PictoChat` records: each adjacent pair of octets is swapped on the wire.
///
/// # Panics
///
/// Panics when `data` holds fewer than [`MAC_ADDRESS_LEN`] bytes after `offset`.
pub(crate) const fn read_word_swapped_mac(data: &[u8], offset: usize) -> MacAddress {
    let mut octets = [0; MAC_ADDRESS_LEN];
    let mut index = 0;
    while index < MAC_ADDRESS_LEN {
        // Flipping the lowest index bit selects the other octet of the same 16-bit word.
        octets[index] = data[offset + (index ^ 1)];
        index += 1;
    }
    MacAddress(octets)
}

/// Encodes a MAC address as three little-endian 16-bit words. Swapping adjacent octets
/// is its own inverse, so this reuses [`read_word_swapped_mac`].
pub(crate) const fn word_swapped_mac(address: MacAddress) -> [u8; MAC_ADDRESS_LEN] {
    read_word_swapped_mac(&address.0, 0).0
}
