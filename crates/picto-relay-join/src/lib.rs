//! Reusable physical Nintendo DS session worker.
//!
//! The active-join binary and the server share the capture-backed association,
//! profile, firmware-template, and drawing transport implementation.

#[expect(
    dead_code,
    reason = "the shared implementation also contains the standalone probe entry point"
)]
#[path = "main.rs"]
mod implementation;

pub use implementation::{LiveCommand, LiveConfig, LiveEvent, run_live_backend};
