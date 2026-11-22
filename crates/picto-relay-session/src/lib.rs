//! The host's side of a Picto Relay session, with no I/O.
//!
//! The ESP32 does everything on the radio. Whatever drives it over USB, the
//! relay server or a web page, has the same things to work out: what to put in
//! the commands it sends, and what the packets coming back mean for the room.
//! That is this crate. Reading and writing the serial port, and all timing,
//! are left to the caller.

pub mod bitmap;
pub mod surface;

mod host;
mod room;

pub use host::{draw_commands, join_payload, left_text, native_drawing, station_id, status_text};
pub use room::{Participant, Room, RoomEvent};
