# picto-relay-listener

`picto-relay-listener` is the tagged capture and inspection tool used while reverse
engineering PictoChat. The installed binary is named `picto-relay-listen`.

It can prepare a Linux monitor interface, record classic radiotap PCAP files, write a
JSON scenario manifest, and print decoded room, join, packet-family, fragment, message,
and leave events as they occur. It can also replay an existing capture offline.

## Capture a scenario

```sh
sudo cargo run -p picto-relay-listener -- \
  --prepare \
  --interface wlan1 \
  --channel 1 \
  --scenario client-one-dot \
  --tag devices=2 \
  --notes "client joined, sent one dot, then left"
```

Capture continues until Ctrl-C unless `--duration SECONDS` is provided.

## Replay a capture

```sh
cargo run -p picto-relay-listener -- --read path/to/capture.pcap
```

## Verify

```sh
cargo test -p picto-relay-listener --locked
```

See [listener.md](../../docs/listener.md) for the guided capture workflow and
[protocol-evidence.md](../../docs/protocol-evidence.md) for capture provenance.
