//! Keeps this crate and the C firmware on the same protocol.
//!
//! - `link_protocol.h` must equal the header generated from this crate's constants.
//! - `link.bin` is produced by the Rust encoder; `firmware/esp32/test/test.c` checks
//!   that the C decoder reads it and the C encoder reproduces it.
//!
//! After an intentional protocol change run `just protocol-header`, which sets
//! `PICTO_WRITE_FIXTURES=1` to rewrite both files.
use picto_relay_link::{C_HEADER_NAME, DRAW, HELLO, PROTOCOL_VERSION, RESPONSE, c_header, encode};
use std::path::PathBuf;

fn firmware(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../firmware/esp32")
        .join(path)
}

fn check(path: PathBuf, expected: &[u8]) {
    if std::env::var_os("PICTO_WRITE_FIXTURES").is_some() {
        std::fs::write(&path, expected).unwrap();
    }
    assert!(
        std::fs::read(&path).unwrap() == expected,
        "{} is out of date; run `just protocol-header`",
        path.display()
    );
}

/// The C test rebuilds this payload: every byte value, so both escape bytes occur.
fn every_byte() -> Vec<u8> {
    (0..=u8::MAX).collect()
}

#[test]
fn c_header_is_generated_from_this_crate() {
    check(firmware("main").join(C_HEADER_NAME), c_header().as_bytes());
}

#[test]
fn rust_encoder_matches_the_fixture_shared_with_the_c_firmware() {
    let mut wire = Vec::new();
    wire.extend(encode(HELLO, 1, &[PROTOCOL_VERSION]).unwrap());
    wire.extend(encode(DRAW, 0x7e7d_1234, &every_byte()).unwrap());
    wire.extend(encode(RESPONSE, 2, &[]).unwrap());
    check(firmware("test/fixtures/link.bin"), &wire);
}
