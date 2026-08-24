# Picto Relay

Picto Relay is a Rust service exposing PictoChat send/receive operations through a
small browser interface and versioned JSON API. A deliberately protocol-free in-memory
backend supports local development; only the AR9271 backend claims Nintendo DS
interoperability.
Physical AR9271 testing has verified discovery, authentication, association, SIFS-timed
membership, both profile passes, and a visibly rendered `PictoRelay` nickname against
a real DS host. It has also verified outbound drawing delivery: a generated dot rendered
on the DS and the host's complete fourteen-fragment rebroadcast was decoded through the
same JSON backend. The server exposes this transport through the same JSON API.

See [docs/plan.md](docs/plan.md) for architecture, assumptions, and phases.
See [docs/pictochat-protocol.md](docs/pictochat-protocol.md) for the consolidated
protocol and compatibility specification, and
[docs/protocol-evidence.md](docs/protocol-evidence.md) for capture provenance and
remaining unknowns. See [docs/packet-shapes.md](docs/packet-shapes.md) for the
byte-level reference covering every currently observed packet family. See
[docs/listener.md](docs/listener.md) for tagged live capture on the Raspberry Pi and
[docs/active-join.md](docs/active-join.md) for the first physical association probe.
See [docs/json-api.md](docs/json-api.md) for the room and send/receive interface.
See [docs/progress-notes.md](docs/progress-notes.md) for the chronological engineering
log, retained evidence, current runtime state, and next work.

Each workspace crate also documents its own boundary and usage:

- [`picto-relay-codec`](crates/picto-relay-codec/README.md)
- [`picto-relay-radio`](crates/picto-relay-radio/README.md)
- [`picto-relay-join`](crates/picto-relay-join/README.md)
- [`picto-relay-listener`](crates/picto-relay-listener/README.md)
- [`picto-relay-server`](crates/picto-relay-server/README.md)

## Run locally

Start the Rust service:

```sh
cargo run -p picto-relay-server
```

On the Raspberry Pi, use the physical backend with the matching custom firmware:

```sh
sudo /home/rob/.local/bin/picto-relay-server \
  --backend ar9271 \
  --prepare \
  --interface wlan1 \
  --channel 1 \
  --room A
```

Open `http://localhost:3001/`, or `http://blueberry.lab.reb.gg:3001/` when the server
runs on the Pi. The page lists relay clients in rooms A–D, joins a selected room, and
sends or receives typed and hand-drawn messages. The JSON API remains available under
`/api/v1`.

## Verify

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```
