//! Reusable physical Nintendo DS session worker.
//!
//! The active-join binary and the server share the capture-backed association,
//! profile, firmware-template, and drawing transport implementation.

mod implementation;

pub use implementation::{
    LiveCommand, LiveConfig, LiveEvent, LiveParticipant, prepare_live_config, run_live_backend,
    run_live_room_observer, run_probe,
};
