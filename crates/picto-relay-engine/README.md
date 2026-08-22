# picto-relay-engine

`picto-relay-engine` is Picto Relay's deterministic protocol state machine. It accepts
timestamped client, radio, and timer events and returns explicit effects for the
application runner to execute.

The engine has no Tokio dependency and never reads the wall clock itself. Tests can
therefore drive association state, retries, acknowledgements, timeouts, and message
reassembly without sleeping or owning a real radio.

## Boundary

Inputs enter through `Engine::handle` as an `EngineInput` containing one of:

- `ClientCommand` for connect, send, and disconnect operations;
- `ReceivedFrame` from a radio backend;
- `Tick` for scheduled retry and timeout work.

The result is a list of `EngineEffect` values:

- transmit a raw frame;
- notify a specific client;
- schedule a future wakeup.

Room discovery and participant-slot evidence are exposed through `RoomState`. Packet
encoding and decoding remain in `picto-relay-codec`; actual I/O remains in
`picto-relay-radio`.

## Verify

```sh
cargo test -p picto-relay-engine --locked
```

See [plan.md](../../docs/plan.md) for the reducer-style architecture and phase history.
