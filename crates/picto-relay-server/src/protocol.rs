//! Versioned request, response, and event shapes for the JSON API.

use serde::{Deserialize, Serialize};

/// Current JSON API protocol version.
pub const PROTOCOL_VERSION: u8 = 1;

/// Number of `PictoChat` rooms, A through D.
pub const ROOM_COUNT: u8 = 4;

/// Logical width of a `PictoChat` drawing in pixels.
pub const CANVAS_WIDTH: usize = 256;

/// Logical height of a `PictoChat` drawing in pixels.
pub const CANVAS_HEIGHT: usize = 96;

/// Bytes in a packed one-bit drawing.
pub const BITMAP_BYTES: usize = CANVAS_WIDTH * CANVAS_HEIGHT / 8;

/// Request body for creating one virtual `PictoChat` session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct CreateSessionRequest {
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

/// Request body for submitting one packed monochrome drawing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct SendMessageRequest {
    /// Base64-encoded, row-major, one-bit 256x96 bitmap.
    pub bitmap: String,
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
