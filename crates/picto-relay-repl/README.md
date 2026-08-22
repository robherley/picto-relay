# picto-relay-repl

`picto-relay-repl` is the reference interactive client for the Picto Relay JSON API.
It creates a virtual-station session, polls for events, renders typed text or simple
drawing commands into the 256×96 packed bitmap contract, and prints incoming drawings
as compact terminal previews.

## Run

Start `picto-relay-server`, then run:

```sh
cargo run -p picto-relay-repl -- --name Terminal
```

To use a gateway on the Raspberry Pi:

```sh
cargo run -p picto-relay-repl -- \
  --url http://blueberry.lab.reb.gg:3001/api/v1 \
  --name Terminal
```

Enter plain text to send it. The REPL also supports `/dot`, `/file`, `/poll`,
`/status`, `/help`, and `/quit`. `--poll-ms` controls the receive interval. The
capture-calibrated first rows start at x=88 to avoid the DS nametag; rows below the
nametag start at x=28. Text up to 85 characters uses a large five-line pixel font.
Longer text falls back to as many as ten compact lines (up to 340 characters) across
the full native 256x80 drawing. Larger input is rejected instead of truncated.

## Verify

```sh
cargo test -p picto-relay-repl --locked
```

See [json-api.md](../../docs/json-api.md) for the API contract used by this client.
