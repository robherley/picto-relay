//! Room-aware Picto Relay JSON protocol and session broker.

pub mod broker;
pub mod protocol;

mod esp32;

pub(crate) use picto_relay_session::bitmap;
mod capture;
mod worker;

pub mod text;
