# Picto Relay protocol evidence

This document separates fields verified in a two-DS capture from structures that
remain opaque. Physical testing has verified discovery, authentication, association,
slot admission, hardware-timed poll replies, sustained transport membership, visible
profile publication, outbound drawing delivery, and host-rebroadcast reception.

## Capture provenance

`captures/blueberry/01-two-ds-baseline.pcap` was recorded on Raspberry Pi `blueberry`
with an AR9271 (`ath9k_htc`, firmware 1.3). It contains one DS-hosted Room A session:

| Item | Observed value |
|---|---|
| Capture frames | 42,556 over 88.68 seconds |
| Active two-DS exchange | 22.870803 seconds |
| Host | `00:24:f3:67:41:87` |
| Client | `00:16:56:4d:27:ca` |
| Association | status 0, AID `0xc001`, participant slot 1 |
| Traffic | 4,611 host commands, 2,517 client replies, 4,648 host acknowledgements |
| Leave | client deauthentication, reason 3 |

The checked-in fixtures exclude radiotap and FCS. Their originating radiotap header
advertised 2 Mbps, short preamble, and an included FCS.

The canonical outbound capture is
`captures/blueberry/1787275056402-virtual-client-outbound-dot-padded-final.pcap`.
It contains a complete join by virtual station `00:09:bf:52:00:01`, profile
initialization, a generated 2,084-byte dot drawing, and the real host's complete
fourteen-fragment rebroadcast. Its JSON sidecar records counts and SHA-256.

## Verified wire layers

### Multiplayer transport

The three Nintendo multicast destinations remain:

| Flow | Address | 802.11 shape | Prefix |
|---|---|---|---|
| Host command | `03:09:bf:00:00:00` | CF-Poll data | `client_time: u16`, `client_mask: u16` |
| Client reply | logical DA `03:09:bf:00:00:10` | To-DS data; receiver is host BSSID | no prefix |
| Host acknowledgement | `03:09:bf:00:00:03` | data | `command_count: u16`, `failed_client_mask: u16` |

The dominant host header was `client_time=998`, with client mask `0x0002` while the
client was polled and `0x0000` otherwise. Acknowledgements used a four-byte body;
4,576 carried failure mask zero and 72 carried mask `0x0002`.

There is no `AA AA 03` LLC header in this capture. The previous LLC interpretation
was removed.

### Application envelope

Host commands and client replies carry:

```text
packet_id: u16 | packet_sequence: u16 | declared_length: u16 | payload
```

For host commands, `declared_length == payload.len()`. Every captured client reply
used `declared_length == payload.len() + 2`; the additional two bytes are an observed
length convention, not bytes missing from the PCAP.

The outbound capture adds a host-only exception: the short final rebroadcast uses
packet ID `0x9e56` and a 172-byte physical payload but retains logical declared length
16. The packet ID describes physical slot size for this frame. The client final is
unmodified and unpadded (`0x8e08`, 14 physical bytes, declared length 16).

For ordinary packets, the packet ID low byte is the declared payload size in 16-bit
words: `0x56` means 172 bytes, `0x48` means 144, `0x38` means 112, and `0x08` means
16. For the padded host final it instead describes the 172-byte physical payload. The
low nibble of the high byte identifies the application family (`0x0e` for transfers);
the remaining high bits vary with direction/state and remain opaque. Other families
are retained as envelopes but are not decoded as message fragments.

### Message fragments

Message-fragment payloads use:

```text
opaque_header[2]
data_length: u8
final_fragment: u8       # 0 or 1
data_offset: u16
opaque_id[2]
data[data_length]
frame_number: u16
host_trailer: u16        # host commands only; observed value 2
```

Full fragments carry 160 message bytes. Completion is explicit; it is not inferred
from a short fragment. One client-originated transfer contained thirteen 160-byte
fragments plus a four-byte final fragment at offset `0x0820`, producing exactly 2,084
bytes. Its first 36 bytes are metadata and the following 2,048 bytes are drawing data.
The complete transfer is checked in and reassembled by a golden test.

The real host rebroadcasts a four-byte final fragment inside a fixed 160-byte data
slot: four logical data bytes, 156 padding bytes, then the host frame number and trailer.
This is why a decoder must read host trailers from the physical payload's end rather
than immediately after logical data.

The metadata stores the sender MAC as three little-endian 16-bit words. Drawing data
is the standard Nintendo DS 8x8 tiled 4bpp layout: tiles proceed left-to-right and
top-to-bottom, and each byte stores the left pixel in its low nibble and the right
pixel in its high nibble. Our captures verify one- and three-band transfers (256x16
and 256x48). The decoder accepts the native five-band 256x80 surface documented by
independent compatible implementations; that 80-row limit is external evidence, not
yet verified in our captures. Each band is 2,048 bytes. Palette index 1 is black in
the controlled capture; the remaining palette meanings are not yet established.

### Beacon membership

The eight-byte custom PictoChat beacon payload contains an opaque four-byte beacon
value, room, user count, and suffix `0x0004`. The first four bytes are not a membership identifier: new captures show
them rotating while room and user count remain unchanged.

### Identity/profile initialization

The baseline join establishes a repeatable application sequence: `c34` session
exchange, `d0a` 84-byte transfer announcement, host `e30` identity transfer, then a
client `e30` profile transfer. The client profile is an 84-byte record containing a
word-swapped MAC, fixed-width UTF-16LE nickname and personal message, colour, and two
birthday bytes. The host rebroadcasts the received profile.

Picto Relay preloads nine reply templates in AR9271 firmware before association:
the opening session response, two profile announcements, reusable empty reply, two
generated profiles, two completion records, and steady idle reply. This preserves the
observed SIFS response window while allowing the profile MAC, participant slot, and
nickname to be generated in Rust for the active association.

`session-state-fix-2.pcap` records the complete physical virtual-client milestone.
The host admits slot 1, advances its beacon from one to two users, accepts both
generated `PictoRelay` profile passes, sustains 1,404 clean firmware transmissions,
and releases the client after firmware disarm and deauthentication. Direct observation
also confirmed that `PictoRelay` appeared in the physical DS participant list. The
consolidated state machine is documented in
[pictochat-protocol.md](pictochat-protocol.md).

`1787275056402-virtual-client-outbound-dot-padded-final.pcap` extends that milestone
through drawing delivery. The DS visibly rendered the generated dot and rebroadcast
all fourteen drawing fragments. The earlier API timeout was a decoder error caused by
the padded host final, not a radio-delivery failure.

`20260820-two-dots-after-clean-rejoin-success.pcap` is the canonical repeated-send
milestone. After a clean leave and rejoin, the virtual station sent two drawings in
one association without reloading the AR9271 driver or firmware. The physical DS
visibly rendered both, the host echoed both `d0a` announcements, and the host
rebroadcast both complete fourteen-fragment transfers.

The controlled failures immediately before that success established four pieces of
per-session/per-transfer state:

- the two-byte fragment transfer tag remained `0x0000` for both messages;
- the fragment frame counter continued across messages (`3..16`, then `17..30`);
- each drawing needed a fresh `d0a` announcement, including advanced command and
  transfer counters plus a changed opaque nonce;
- the completion transfer counter was the announcement transfer counter plus one.

The successful virtual sequence used announcement command counters `12, 15`,
announcement transfer counters `5, 7`, and completion counters `6, 8`. These values
match the progression seen across controlled real-DS sends. The exact nonce-generation
algorithm is not known; Picto Relay generates a distinct deterministic value for each
transfer and the DS accepted the tested sequence.

API delivery correlation now requires both a byte-for-byte matching host `d0a`
announcement echo and a complete host-rebroadcast drawing from the virtual station.
Merely reassembling the station's own transmitted fragments is not delivery evidence.

## Implemented evidence

- [packet-shapes.md](packet-shapes.md) is the byte-level reference for management,
  multiplayer, envelope, packet-family, fragment, and reassembled-message shapes.
- `captured_transport.rs` decodes exact client, host-rebroadcast, acknowledgement,
  and final-fragment frames from the two-DS capture.
- `captured-client-transfer.hex` contains the fourteen application fragments and
  verifies complete 2,084-byte reassembly and exact metadata.
- `host-padded-final-fragment.hex` preserves the canonical outbound host tail and
  verifies the 16-byte logical/172-byte physical exception.
- `golden.rs` verifies both a historical idle Room A beacon and the new two-user
  joined beacon.
- Property tests exercise arbitrary raw frame inputs and randomized out-of-order
  fragment reassembly.
- The engine and scripted broker use the capture-backed envelope, 160-byte fragment
  size, explicit completion marker, To-DS client address layout, and acknowledgement
  command counter.

## Remaining unknowns

- Message metadata beyond sender MAC, palette indices beyond observed black index 1,
  and possible checksums or keyed values.
- Meaning of the two opaque fragment-header bytes and the transfer tag, which remained
  zero throughout controlled repeated sends.
- Meaning of the host-only trailer word and packet-ID high-bit variants.
- Counter wraparound, failed-client retries, and delivery behavior under packet loss.
- Meaning and lifecycle of the variable `c34`/`d0a` session fields that the original
  DS regenerates.
- Multiple virtual source MAC addresses and multiple participant slots on one AR9271.

Unknown bytes remain opaque instead of receiving speculative names.

## Next controlled captures

1. Capture three or more drawings to confirm the observed counter strides beyond the
   second transfer and across 16-bit wraparound.
2. Capture a drawing sent from the real host while the virtual client is present.
3. Change only pen color to isolate remaining metadata and palette fields.
4. Exercise 16-, 32-, 48-, 64-, and 80-row messages in both directions.
5. Induce a failed fragment reply and observe announcement/completion retry behavior.

## Sources

- [GBATEK: Nintendo beacons](https://problemkaputt.de/gbatek-ds-wifi-nintendo-beacons.htm)
- [GBATEK: IEEE 802.11 frames](https://problemkaputt.de/gbatek-ds-wifi-ieee802-11-frames.htm)
- [melonDS Wi-Fi implementation](https://github.com/melonDS-emu/melonDS/blob/master/src/Wifi.cpp)
- [Nintendo DS multiplayer Wireshark dissector](https://gist.github.com/alvinhochun/b7fb10bdfc7647c22847)
- [PictoChat reverse-engineering notes](https://matthil.de/w/pictochat.html)
- [Working ESP32 PictoChat host implementation](https://github.com/mjwells2002/pictochat-rs)
- [PictoChat web bridge protocol notes](https://github.com/ksavill/pictochat-web-bridge/blob/main/docs/pictochat-protocol.md)
- [PictoChat Protocol Adapter / PictoSniff-derived decoder](https://github.com/Thesola10/PictoChat)
