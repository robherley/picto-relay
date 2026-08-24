# picto-relay-server

`picto-relay-server` owns the bounded async room/session broker, connects the API to
either an in-memory or physical backend, exposes the versioned JSON HTTP API, and
serves the framework-free browser client embedded in its binary. Room snapshots and
session events are pushed over WebSockets; REST polling remains available for API
compatibility. Session streams include explicit participant join/leave deltas alongside
authoritative roster snapshots.

## Backends

- `memory` is the default and exercises session, roster, message, and delivery event
  shapes without simulating Nintendo packets.
- `ar9271` starts the physical session worker from `picto-relay-join`. The current
  firmware represents one virtual station, so only one live API session is supported.
  Passive beacon observation supplies live A-through-D room counts before the relay
  joins. Its active room roster contains physical DS nicknames decoded from host
  profile rebroadcasts.

## Run locally

```sh
cargo run -p picto-relay-server
```

The default bind address is `0.0.0.0:3001`; override it with `PICTO_RELAY_BIND`.
Open `http://localhost:3001/` for the browser UI. The API is rooted at
`http://localhost:3001/api/v1`. WebSocket paths and JSON frame shapes are documented
in [json-api.md](../../docs/json-api.md).

## Run on the Raspberry Pi

```sh
sudo cargo run -p picto-relay-server -- \
  --backend ar9271 \
  --prepare \
  --interface wlan1 \
  --channel 1 \
  --room A
```

## Verify

```sh
cargo test -p picto-relay-server --locked
```

See [json-api.md](../../docs/json-api.md) for endpoints and event shapes.
