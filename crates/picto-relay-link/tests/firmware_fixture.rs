//! Keeps this crate and the firmware's C implementation (`firmware/esp32/main/link.c`)
//! on the same wire format. Both sides are tested against one fixture file: this
//! test proves the Rust encoder produces it; `firmware/esp32/test/test.c` proves the
//! C decoder, unpacker and encoder agree with it.
//!
//! After an intentional protocol change, regenerate with
//! `PICTO_WRITE_FIXTURES=1 cargo test -p picto-relay-link --test firmware_fixture`.
use picto_relay_link::{
    BODIES, HELLO, MAX_BODY, MAX_ENCODED, MAX_RECORD, Packet, RESPONSE, encode, pack_body,
};

const PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../firmware/esp32/test/fixtures/link.bin"
);

/// The same three bodies are rebuilt by the C test: all zeros (compresses to one
/// run), a counting pattern (incompressible, needs the extended raw record) and a
/// short body containing both framing escape bytes.
fn bodies() -> [Vec<u8>; 3] {
    [
        vec![0; MAX_BODY],
        (0..MAX_BODY).map(|i| i as u8 + 1).collect(),
        (0..24)
            .map(|i| if i % 2 == 0 { 0x7e } else { 0x7d })
            .collect(),
    ]
}

fn fixture() -> Vec<u8> {
    let mut payload = 7u32.to_le_bytes().to_vec();
    payload.push(0);
    for body in bodies() {
        let mut record = [0; MAX_RECORD];
        let n = pack_body(&body, &mut record).unwrap();
        payload.extend_from_slice(&record[..n]);
    }
    let mut bytes = Vec::new();
    for packet in [
        Packet::new(HELLO, 1, &[]).unwrap(),
        Packet::new(BODIES, 0x7e7d_1234, &payload).unwrap(),
        Packet::new(RESPONSE, 2, &[0, 1, 2, 3, 4, 5, 6]).unwrap(),
    ] {
        let mut wire = [0; MAX_ENCODED];
        let n = encode(&packet, &mut wire);
        bytes.extend_from_slice(&wire[..n]);
    }
    bytes
}

#[test]
fn rust_encoder_matches_the_fixture_shared_with_the_c_firmware() {
    if std::env::var_os("PICTO_WRITE_FIXTURES").is_some() {
        std::fs::write(PATH, fixture()).unwrap();
    }
    assert_eq!(std::fs::read(PATH).unwrap(), fixture());
}
