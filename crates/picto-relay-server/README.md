# picto-relay-server

`picto-relay-server` is the headless Picto Relay gateway. It owns the bounded async
session broker, connects protocol execution to either a scripted or physical backend,
and exposes the versioned JSON HTTP API.

The server does not serve a frontend or expose a WebSocket endpoint.

## Backends

- `scripted` is the default and supports deterministic local development with no
  hardware.
- `ar9271` starts the physical session worker from `picto-relay-join`. The current
  firmware represents one virtual station, so only one live API session is supported.

## Run locally

```sh
cargo run -p picto-relay-server
```

The default bind address is `0.0.0.0:3001`; override it with `PICTO_RELAY_BIND`.
The API is rooted at `http://localhost:3001/api/v1`.

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
