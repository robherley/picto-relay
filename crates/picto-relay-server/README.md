# picto-relay-server

`picto-relay-server` owns the bounded async room/session broker, connects the API to
either an in-memory or physical backend, exposes the versioned JSON HTTP API, and
serves the [web client](../../web/README.md) embedded in its binary. Room snapshots and
session events are pushed over WebSockets; REST polling remains available for API
compatibility. Session streams include explicit participant join/leave deltas alongside
authoritative roster snapshots.

## Backends

- `memory` is the default and exercises session, roster, message, and delivery event
  shapes without simulating Nintendo packets.
- `esp32` uses an ESP-WROOM-32 running the USB bridge firmware. One web
  session owns the physical station. The ESP32 handles radio timing; this server
  owns HTTP, WebSockets and drawing encoding/decoding. It joins a DS-hosted room.
  While no session is open, room member counts come from periodic channel scans.

## Run with the ESP32

Flash [the firmware](../../firmware/esp32/README.md) from the same checkout, then:

```sh
PICTO_RELAY_BIND=127.0.0.1:3001 cargo run -p picto-relay-server -- \
  --backend esp32 --serial /dev/cu.usbserial-0001
```

On Linux use the appropriate port, for example `/dev/ttyUSB0`, with serial-device
permissions. The host requires `stty`. Open the UI, choose a nickname and join a
room; the firmware finds its Wi-Fi channel. The serial worker reconnects automatically;
messages interrupted by a disconnect report a delivery timeout and are not replayed.
The UI reports upload and radio progress plus queue depth. A full-height drawing
is 86 radio fragments, so bursts queue behind earlier messages. See [the transport contract](../../docs/esp32-usb.md).

## Run locally

```sh
just web-build
cargo run -p picto-relay-server
```

The web client is built separately into `web/dist` and embedded when the server
compiles. Without it the server still builds and serves the API, and answers
`/` with a note saying how to build the client.

The default bind address is `0.0.0.0:3001`; override it with `PICTO_RELAY_BIND`.
Open `http://localhost:3001/` for the browser UI. The API is rooted at
`http://localhost:3001/api/v1`. WebSocket paths and JSON frame shapes are documented
in [json-api.md](../../docs/json-api.md).

## Verify

```sh
cargo test -p picto-relay-server --locked
```

See [json-api.md](../../docs/json-api.md) for endpoints and event shapes. The
OpenAPI schema is served at `/api/v1/openapi.json` and checked in as
[`docs/openapi.json`](../../docs/openapi.json); regenerate it with `just openapi`
after changing a handler or an API type.
