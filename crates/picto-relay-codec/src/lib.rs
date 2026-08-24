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
    ManagementBody, NintendoBeacon, PictoChatBeacon, parse_management,
};
pub use frame::{
    BROADCAST, CLIENT_ACK, CLIENT_REPLY, FcsMode, FrameControl, FrameType, HOST_TO_CLIENT,
    MacAddress, MacFrame, MacHeader, SequenceControl, build_association_request,
    build_authentication_request, build_client_reply_frame, build_data_frame,
    build_management_frame,
};
pub use message::{
    DRAWING_BAND_BYTES, DRAWING_HEIGHT, DRAWING_METADATA_BYTES, DRAWING_WIDTH, PictoChatMessage,
    PixelBounds, encode_message,
};
pub use transport::{
    MESSAGE_FRAGMENT_PAYLOAD_BYTES, MessageReassembler, MultiplayerHeader, NintendoData,
    NintendoFlow, PictoChatEnvelope, PictoChatFragment, PictoChatProfile, PictoChatRoster,
    ReassemblyKey, build_ack_frame, build_fragment_frame, decode_nintendo_data,
    encode_fragment_envelope, fragment_payload, fragment_payload_starting_at,
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

pub(crate) fn read_u16(
    data: &[u8],
    offset: usize,
    context: &'static str,
) -> Result<u16, DecodeError> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or_else(|| too_short(context, offset + 2, data.len()))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

pub(crate) fn read_u32(
    data: &[u8],
    offset: usize,
    context: &'static str,
) -> Result<u32, DecodeError> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| too_short(context, offset + 4, data.len()))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}
