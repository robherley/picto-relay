//! Nintendo `PictoChat` application packets and drawings.
//!
//! Covers what the server sees over USB: application envelopes, fragments,
//! rosters, profiles and the native drawing format. IEEE 802.11 framing is the
//! firmware's job. Fields whose meaning is not established remain opaque.

mod mac;
mod message;
mod transport;

pub use mac::{MAC_ADDRESS_LEN, MacAddress};
pub use message::{
    DRAWING_BAND_HEIGHT, DRAWING_HEIGHT, DRAWING_WIDTH, PictoChatMessage, encode_message,
};
pub use transport::{
    MessageReassembler, NintendoFlow, PictoChatEnvelope, PictoChatFragment, PictoChatProfile,
    PictoChatRoster, ReassemblyKey, encode_fragment_envelope, fragment_payload_starting_at,
};

use thiserror::Error;

/// Errors produced while parsing or constructing `PictoChat` packets.
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
