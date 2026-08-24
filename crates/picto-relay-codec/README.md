# picto-relay-codec

`picto-relay-codec` contains the byte-level codecs used by Picto Relay. It parses,
builds, fragments, and reassembles the IEEE 802.11, Nintendo local-wireless, and
PictoChat structures observed in captures from real Nintendo DS hardware.

The crate is deliberately synchronous and independent of the network runtime. Known
fields are represented by typed Rust values; bytes whose meaning has not been proven
remain opaque so decoded packets can round-trip without losing evidence.

## Responsibilities

- Parse and serialize 802.11 management and data frames.
- Decode Nintendo beacon, authentication, association, and multiplayer structures.
- Decode and construct PictoChat envelopes and message fragments.
- Decode `c34` participant-slot rosters and `e30` UTF-16LE identity profiles.
- Reassemble captured client and host message streams.
- Encode one to five drawing bands into the native PictoChat message layout.
- Preserve unknown fields and optional frame check sequences losslessly.

This crate does not open wireless interfaces, schedule transmissions, or manage
participants. Those responsibilities belong to `picto-relay-radio` and the physical
session worker in `picto-relay-join`.

## Verify

```sh
cargo test -p picto-relay-codec --locked
```

Golden capture tests live under `tests/`. See
[packet-shapes.md](../../docs/packet-shapes.md) for the byte-level catalog and
[pictochat-protocol.md](../../docs/pictochat-protocol.md) for the consolidated protocol
description.
