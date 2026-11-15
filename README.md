# Picto Relay

Picto Relay connects a Nintendo DS running PictoChat to a browser and versioned
JSON API. An ESP-WROOM-32 runs the radio protocol in C; the computer runs the
Rust web server, bitmap codec, and USB serial worker.

The ESP32 joins a DS-hosted room and exchanges profiles and drawings. Hosting a
room on the relay is not supported. The former patched AR9271 radio, Linux monitor
backend, and Pi software-host implementation have been retired.

## Run

Install Rust and `just` (`brew install just` on macOS). Building firmware also
needs ESP-IDF v5.5.2, which `just espressif` downloads into the project; see
the [firmware instructions](firmware/esp32/README.md).

```sh
just                    # List commands
just server             # Run with the connected ESP32
just server-memory      # Run without hardware
```

Stop the server before uploading firmware so the serial port is free:

```sh
just espressif          # Once: download ESP-IDF into .esp/
just firmware-build     # Build only
just firmware-upload    # Build, then flash the ESP32
```

Recipes default to `/dev/cu.usbserial-0001` and `127.0.0.1:3001`. Override them
through environment variables:

```sh
PICTO_SERIAL_PORT=/dev/ttyUSB0 just firmware-upload
PICTO_SERIAL_PORT=/dev/ttyUSB0 PICTO_RELAY_BIND=127.0.0.1:3002 just server
```

`PICTO_FLASH_BAUD` sets the flashing speed (default `460800`). The runtime link has
no settings: the firmware finds the room's Wi-Fi channel itself, and firmware and
server share one [USB protocol](docs/esp32-usb.md). Flash the firmware from the same
checkout as the server you run; a mismatch is reported when the server connects.

Open http://localhost:3001/, enter a nickname, and join the room open on the DS.
Only one API session can own the ESP32. For hardware-free UI/API development,
`just server-memory` uses the in-memory backend.

## Components

- [Server](crates/picto-relay-server/README.md): browser UI, JSON API, sessions, and USB worker.
- [Codec](crates/picto-relay-codec/README.md): captured Nintendo packet and drawing formats.
- [USB link](crates/picto-relay-link/README.md): the USB protocol definition and host-side framing.
- [Firmware](firmware/esp32/README.md): C protocol state machine and ESP-IDF radio hooks.

See the [JSON API](docs/json-api.md) and its [OpenAPI schema](docs/openapi.json), [protocol reference](docs/pictochat-protocol.md),
[packet shapes](docs/packet-shapes.md), and [capture evidence](docs/protocol-evidence.md).
Captures and hardware evidence are bundled in one [archive](captures/README.md).
Earlier investigations are retained under [docs/history](docs/history/README.md);
their commands and runtime descriptions are historical, not supported workflows.

## Verify

```sh
just test               # Rust, browser-tool and firmware protocol tests
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

None of these need hardware or ESP-IDF; the firmware protocol code is compiled
and tested on the host. Only `just firmware-build` needs ESP-IDF.

## Hardware drawing benchmark

Stop the server and leave a DS in the selected room, then run:

```sh
just benchmark
PICTO_BENCH_ROOM=2 PICTO_BENCH_TRIALS=5 PICTO_BENCH_PATTERN=dense just benchmark
```

This ignored Rust integration test owns the ESP32 session and sends visible
full-height drawings. Rooms are 0–3 (A–D); defaults are Room A, three trials, and
`cross` (border with diagonals). `dense` sends a checkerboard. The serial port
setting is the same as `just server`.

Each trial reports submission-to-DS-delivery time; upload timing is logged with
`RUST_LOG=picto_relay_server=debug`. The test fails on
reconnection, delivery failure, or timeout and releases its session afterward.
Normal `cargo test` runs skip this hardware test.
