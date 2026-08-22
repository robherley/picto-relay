# Picto Relay

Picto Relay is a headless Rust service exposing PictoChat send/receive operations
through a versioned JSON API. The bundled terminal REPL is its primary interactive
client. The complete hardware-free path uses Nintendo-shaped 802.11 frames through
the deterministic `ScriptedRadio`.
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
See [docs/json-api.md](docs/json-api.md) for the headless send/receive interface.
See [docs/progress-notes.md](docs/progress-notes.md) for the chronological engineering
log, retained evidence, current runtime state, and next work.

Each workspace crate also documents its own boundary and usage:

- [`picto-relay-codec`](crates/picto-relay-codec/README.md)
- [`picto-relay-engine`](crates/picto-relay-engine/README.md)
- [`picto-relay-radio`](crates/picto-relay-radio/README.md)
- [`picto-relay-join`](crates/picto-relay-join/README.md)
- [`picto-relay-listener`](crates/picto-relay-listener/README.md)
- [`picto-relay-server`](crates/picto-relay-server/README.md)
- [`picto-relay-repl`](crates/picto-relay-repl/README.md)

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

The JSON API is available at `http://localhost:3001/api/v1`; start with
`GET /api/v1/status`.

In a second terminal, start the interactive sender/receiver:

```sh
cargo run -p picto-relay-repl -- --name Terminal
```

From another machine, point the REPL at the Pi:

```sh
cargo run -p picto-relay-repl -- \
  --url http://blueberry.lab.reb.gg:3001/api/v1 \
  --name Terminal
```

Type a line to render it into a PictoChat bitmap and send it. The REPL polls for
incoming messages automatically and prints a compact terminal preview. Enter `/help`
for drawing, file, status, and session commands.

## Verify

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```
