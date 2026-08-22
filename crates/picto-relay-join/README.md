# picto-relay-join

`picto-relay-join` implements the physical Nintendo DS client session. It discovers a
DS-hosted PictoChat room, authenticates, associates, completes the two profile passes,
maintains slot-poll replies through the custom AR9271 firmware, and exchanges drawings.

The package provides two entry points:

- the `picto-relay-join` diagnostic CLI for one active join attempt;
- `run_live_backend`, the reusable blocking worker used by `picto-relay-server`.

## Run the diagnostic probe

On the Raspberry Pi with the dedicated adapter attached and matching firmware loaded:

```sh
sudo cargo run -p picto-relay-join -- \
  --prepare \
  --interface wlan1 \
  --channel 1 \
  --room A \
  --firmware-assist \
  --hold-seconds 20
```

Add `--capture PATH` to record the exchange. `--send-dot` submits the built-in drawing
probe after initialization.

This crate is tightly coupled to the evidence-backed AR9271 firmware protocol. It is
not the general packet codec or HTTP API.

## Verify

```sh
cargo test -p picto-relay-join --locked
```

See [active-join.md](../../docs/active-join.md) for setup and
[pictochat-protocol.md](../../docs/pictochat-protocol.md) for the exchange sequence.
