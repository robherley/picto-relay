//! Versioned request, response, and event shapes for the JSON API.

use serde::{Deserialize, Serialize};

/// Current JSON API protocol version.
pub const PROTOCOL_VERSION: u8 = 1;

// --- Rooms -----------------------------------------------------------------

/// Number of `PictoChat` rooms, A through D.
pub const ROOM_COUNT: u8 = 4;

/// Display letters of the `PictoChat` rooms, indexed by room number.
pub const ROOM_NAMES: [&str; ROOM_COUNT as usize] = ["A", "B", "C", "D"];

// --- Drawing surface ---------------------------------------------------------

/// Logical width of a `PictoChat` drawing in pixels.
pub const CANVAS_WIDTH: usize = 256;

/// Logical height of a `PictoChat` drawing in pixels.
pub const CANVAS_HEIGHT: usize = 96;

/// Pixels stored in each byte of a packed one-bit drawing.
pub const MONO_PIXELS_PER_BYTE: usize = u8::BITS as usize;

/// Bits used by each pixel of a packed palette drawing.
pub const PALETTE_BITS_PER_PIXEL: usize = 4;

/// Pixels stored in each byte of a packed palette drawing.
pub const PALETTE_PIXELS_PER_BYTE: usize = u8::BITS as usize / PALETTE_BITS_PER_PIXEL;

/// Mask selecting one palette index (one nibble) from a packed palette byte.
pub const PALETTE_INDEX_MASK: u8 = (1 << PALETTE_BITS_PER_PIXEL) - 1;

/// Bytes in a packed one-bit drawing.
pub const BITMAP_BYTES: usize = CANVAS_WIDTH * CANVAS_HEIGHT / MONO_PIXELS_PER_BYTE;
/// Packed 4bpp palette bitmap size in bytes (low nibble is the left pixel).
pub const PALETTE_BITMAP_BYTES: usize = CANVAS_WIDTH * CANVAS_HEIGHT / PALETTE_PIXELS_PER_BYTE;

// --- Drawable area (pixels), measured conservatively from a DS sample --------

/// Height in pixels of the native DS drawing; rows below it are API padding.
pub const NATIVE_DRAWING_HEIGHT: usize = picto_relay_codec::DRAWING_HEIGHT;

/// Leftmost drawable column below the name tag.
pub const DRAWABLE_LEFT: usize = 24;

/// First column right of the drawable area (exclusive right edge).
pub const DRAWABLE_RIGHT: usize = 252;

/// Leftmost drawable column beside the name tag.
pub const DRAWABLE_NAMETAG_LEFT: usize = 80;

/// Height in pixels of the name-tag notch at the top-left of a drawing.
pub const DRAWABLE_NAMETAG_HEIGHT: usize = 16;

/// Safe drawing rectangles as `[x, y, width, height]` in pixels: the strip
/// right of the name tag, then the full-width area below it.
pub const DRAWABLE_REGIONS: [[usize; 4]; 2] = [
    [
        DRAWABLE_NAMETAG_LEFT,
        0,
        DRAWABLE_RIGHT - DRAWABLE_NAMETAG_LEFT,
        DRAWABLE_NAMETAG_HEIGHT,
    ],
    [
        DRAWABLE_LEFT,
        DRAWABLE_NAMETAG_HEIGHT,
        DRAWABLE_RIGHT - DRAWABLE_LEFT,
        NATIVE_DRAWING_HEIGHT - DRAWABLE_NAMETAG_HEIGHT,
    ],
];

// --- Drawing ink palette indices ---------------------------------------------

/// Palette index of blank paper.
pub const PAPER_INK: u8 = 0;

/// Palette index of black ink, the only ink of a one-bit drawing.
pub const BLACK_INK: u8 = 1;

/// First palette index of the rainbow shades.
pub const FIRST_RAINBOW_INK: u8 = 3;

/// Number of rainbow shades, palette indices 3 through 15.
pub const RAINBOW_INK_COUNT: u8 = PALETTE_INDEX_MASK - FIRST_RAINBOW_INK + 1;

/// Fixed pen palette indices accepted for text: black and the rainbow shades.
/// Index 2 is excluded because its meaning is unknown.
pub const INK_INDICES: &[u8] = &[1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

// --- Request limits ------------------------------------------------------------

/// Maximum display name length in UTF-16 code units.
pub const NAME_MAX_UNITS: usize = picto_relay_link::NICKNAME_LEN / size_of::<u16>();

/// Maximum text message length in UTF-16 code units.
pub const TEXT_MAX_UNITS: usize = 220;

/// Maximum personal profile message length in UTF-16 code units.
pub const PROFILE_MESSAGE_MAX_UNITS: usize =
    picto_relay_link::PROFILE_MESSAGE_LEN / size_of::<u16>();

/// Inclusive maximum DS profile color index.
pub const PROFILE_COLOR_MAX: u8 = picto_relay_link::MAX_PROFILE_COLOR;

/// Request body for creating one virtual `PictoChat` session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct CreateSessionRequest {
    /// DS profile color index (0–15); omitted requests retain the original color.
    #[serde(default)]
    pub profile_color: u8,
    /// Personal message advertised during profile exchange (up to 26 UTF-16 units).
    #[serde(default)]
    pub profile_message: String,
    /// Display name assigned to the virtual station.
    pub name: String,
    /// Room number from `0` (A) through `3` (D). Omitted requests use the server default.
    #[serde(default)]
    pub room: Option<u8>,
}

/// Response returned after creating a virtual `PictoChat` session.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct CreateSessionResponse {
    /// JSON API version.
    pub version: u8,
    /// Identifier used by subsequent message and event requests.
    pub session_id: u32,
    /// Room number assigned to the session.
    pub room: u8,
}

/// Snapshot of the participants known to the selected backend in one room.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RoomSummary {
    /// Whether a current roster or recent beacon has been observed.
    pub observed: bool,
    /// Room number from `0` (A) through `3` (D).
    pub room: u8,
    /// Number of clients advertised by the room, including clients whose profile is not known yet.
    pub participant_count: usize,
    /// API clients in memory mode, or physical roster members in the active live room.
    pub participants: Vec<Participant>,
}

/// Response returned by the room roster endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RoomsResponse {
    /// JSON API version.
    pub version: u8,
    /// Room snapshots in A-through-D order.
    pub rooms: Vec<RoomSummary>,
}

/// Request body for submitting one packed drawing.
#[derive(Clone, Debug, Deserialize)]
pub struct SendMessageRequest {
    /// Text to render; mutually exclusive with the bitmap fields.
    pub text: Option<String>,
    /// Ink for text rendering; defaults to black.
    pub ink: Option<crate::text::Ink>,
    /// Base64-encoded, row-major, one-bit 256x96 bitmap.
    #[serde(default)]
    pub bitmap: String,
    /// Optional base64 row-major 4bpp bitmap; mutually exclusive with bitmap.
    pub palette_bitmap: Option<String>,
}

/// Response returned when a drawing has entered the broker queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct AcceptedResponse {
    /// JSON API version.
    pub version: u8,
    /// Stable submission state; currently always `queued`.
    pub status: &'static str,
}

/// Queued events returned by the polling endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EventsResponse {
    /// JSON API version.
    pub version: u8,
    /// Events accumulated since this session's previous poll.
    pub events: Vec<ServerEnvelope>,
}

/// Description of the active gateway and drawing contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct StatusResponse {
    /// Accepted fixed pen indices; rainbow is also available for text.
    pub ink_indices: &'static [u8],
    /// Whether text accepts rainbow ink.
    pub rainbow_text: bool,
    /// Maximum text length in UTF-16 units.
    pub text_max_units: usize,
    /// Maximum personal message length in UTF-16 units.
    pub profile_message_max_units: usize,
    /// Inclusive maximum profile color index.
    pub profile_color_max: u8,
    /// Safe drawing rectangles as [x, y, width, height].
    pub drawable_regions: [[usize; 4]; 2],
    /// JSON API version.
    pub version: u8,
    /// Radio implementation currently attached to the broker.
    pub backend: &'static str,
    /// Required logical bitmap width.
    pub bitmap_width: usize,
    /// Required logical bitmap height.
    pub bitmap_height: usize,
    /// Required decoded bitmap byte length.
    pub bitmap_bytes: usize,
    /// Encoding expected in JSON request bodies.
    pub bitmap_encoding: &'static str,
    /// Decoded byte length for indexed color drawings.
    pub palette_bitmap_bytes: usize,
    /// Encoding for the optional indexed color field.
    pub palette_bitmap_encoding: &'static str,
}

/// Stable JSON error response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ErrorResponse {
    /// JSON API version.
    pub version: u8,
    /// Stable machine-readable error code.
    pub code: &'static str,
    /// Human-readable explanation.
    pub message: String,
}

/// A versioned event returned to an API client.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ServerEnvelope {
    /// Protocol version used to encode this event.
    pub version: u8,
    /// Event payload.
    #[serde(flatten)]
    pub event: ServerWireEvent,
}

impl ServerEnvelope {
    /// Wraps an event in the current protocol version.
    #[must_use]
    pub const fn new(event: ServerWireEvent) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            event,
        }
    }
}

/// Server-originated session events.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerWireEvent {
    /// Personal profile observed in a physical room.
    Profile {
        /// MAC-derived participant identifier.
        station_id: u32,
        /// Profile nickname.
        name: String,
        /// Personal message.
        profile_message: String,
        /// Raw DS profile color.
        profile_color: u16,
    },
    /// Reports physical radio association and profile progress.
    BackendState {
        /// Stable state name suitable for interactive clients.
        state: &'static str,
        /// Human-readable state detail.
        detail: String,
    },
    /// Confirms that an API session owns a virtual station.
    SessionReady {
        /// Assigned virtual station identity.
        client_id: u32,
    },
    /// Replaces the client's participant list.
    Participants {
        /// Room whose roster is being replaced.
        room: u8,
        /// Currently known virtual or physical participants.
        participants: Vec<Participant>,
    },
    /// Reports that one participant appeared in a room roster.
    ParticipantJoined {
        /// Room the participant joined.
        room: u8,
        /// Participant added to the roster.
        participant: Participant,
    },
    /// Reports that one participant disappeared from a room roster.
    ParticipantLeft {
        /// Room the participant left.
        room: u8,
        /// Last known identity of the removed participant.
        participant: Participant,
    },
    /// Delivers a complete drawing.
    Message {
        /// Provisional sending station identity.
        station_id: u32,
        /// Best available display name.
        name: String,
        /// Sender transfer tag promoted to the API message identifier.
        message_id: u32,
        /// Base64-encoded packed monochrome bitmap.
        bitmap: String,
        /// Optional lossless 4bpp palette bitmap; bitmap remains a monochrome fallback.
        #[serde(skip_serializing_if = "Option::is_none")]
        palette_bitmap: Option<String>,
    },
    /// Reports acknowledgement of a submitted message.
    Delivered {
        /// Acknowledged transfer identifier.
        message_id: u32,
    },
    /// Reports that a submitted message exhausted its retries.
    DeliveryTimedOut {
        /// Timed-out transfer identifier.
        message_id: u32,
    },
    /// Reports a recoverable command or connection problem.
    Error {
        /// Stable machine-readable error code.
        code: &'static str,
        /// Human-readable explanation.
        message: String,
    },
}

/// One participant in a room roster.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Participant {
    /// Provisional station identity; live backends derive it from the wireless MAC.
    pub client_id: u32,
    /// API-provided name or nickname parsed from a physical DS profile.
    pub name: String,
}
