# picto-relay-codec

`picto-relay-codec` contains the byte-level codecs the server needs for what the
ESP32 forwards over USB: PictoChat application packets and drawings, as observed
in captures from real Nintendo DS hardware. IEEE 802.11 framing, beacons and the
join handshake are the firmware's job and live in `firmware/esp32/main/client.c`.

The crate is synchronous and independent of the network runtime. Bytes whose
meaning has not been proven remain opaque.

## Responsibilities

- Decode PictoChat envelopes and message fragments.
- Decode `c34` participant-slot rosters and `e30` UTF-16LE identity profiles.
- Reassemble fragmented messages rebroadcast by a DS host.
- Encode and decode one to five drawing bands in the native PictoChat layout.
- Build fragment envelopes, as the reference the C firmware's output is tested
  against.

## Verify

```sh
cargo test -p picto-relay-codec --locked
```

Capture tests live under `tests/`. See
[packet-shapes.md](../../docs/packet-shapes.md) for the byte-level catalog and
[pictochat-protocol.md](../../docs/pictochat-protocol.md) for the consolidated protocol
description.
