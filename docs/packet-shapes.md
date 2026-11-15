# PictoChat packet shapes

This is a byte-layout reference for packet families relevant to Picto Relay. Unless
stated otherwise, multibyte integers are little-endian and offsets begin at the start
of the containing structure.

The layouts marked **verified** were observed in the real-DS baseline and the
canonical virtual-client outbound capture listed in
[`captures/pi/README.md`](../captures/captures.tar.gz). **Standard**
layouts come from IEEE 802.11. **Opaque** fields are preserved but do not yet have a
reliable meaning.

## Encapsulation overview

```text
Capture only
┌──────────────┐
│ Radiotap     │ variable length
├──────────────┤
│ 802.11 MAC   │ normally 24 bytes for frames described here
├──────────────┤
│ Flow prefix  │ 0 or 4 bytes, depending on Nintendo flow
├──────────────┤
│ App envelope │ 0 or 6 bytes
├──────────────┤
│ Packet body  │ packet-family-specific
├──────────────┤
│ FCS          │ 4 bytes when radiotap says it is present
└──────────────┘
```

Golden fixture files omit radiotap and FCS. Offsets in the sections below therefore
restart at each layer rather than using absolute capture offsets.

## Capture wrapper

### Radiotap — observed

Radiotap is supplied by the Linux capture interface, not transmitted as part of the
802.11 frame.

| Field | Size | Captured value or treatment |
|---|---:|---|
| Version | 1 | `0` |
| Padding | 1 | `0` |
| Header length | 2 | Variable; start of 802.11 frame |
| Present words and fields | Variable | Parse according to present bitmap |
| Rate | Field-dependent | 2 Mbps in the baseline capture |
| Flags | Field-dependent | `0x12`: short preamble and FCS present |

The FCS is the final four bytes of the captured 802.11 frame when the radiotap FCS
flag is set.

## IEEE 802.11 MAC headers

### Three-address management/data header — standard

| Offset | Size | Field |
|---:|---:|---|
| `0x00` | 2 | Frame control |
| `0x02` | 2 | Duration/ID |
| `0x04` | 6 | Address 1 |
| `0x0a` | 6 | Address 2 |
| `0x10` | 6 | Address 3 |
| `0x16` | 2 | Sequence control: 12-bit sequence, 4-bit fragment |
| `0x18` | Variable | Frame body |

Relevant frame-control bits:

| Bits | Meaning |
|---|---|
| `2..3` | Type: management `0`, data `2` |
| `4..7` | Subtype |
| `8` | To DS |
| `9` | From DS |
| `10` | More fragments |
| `11` | Retry |

### Nintendo address placement — verified

| Flow | To DS | Address 1 | Address 2 | Address 3 |
|---|---:|---|---|---|
| Host command | 0 | `03:09:bf:00:00:00` | Host | Host/BSSID |
| Client reply | 1 | Host/BSSID | Client | `03:09:bf:00:00:10` |
| Host acknowledgement | 0 | `03:09:bf:00:00:03` | Host | Host/BSSID |

The client-reply multicast value is the logical destination in address 3. It is not
the radio receiver in address 1.

## Management packet bodies

All of these follow the 24-byte management MAC header.

### Beacon — standard outer shape

Management subtype: `8`.

| Offset | Size | Field |
|---:|---:|---|
| `0x00` | 8 | Timestamp |
| `0x08` | 2 | Beacon interval |
| `0x0a` | 2 | Capability flags |
| `0x0c` | Variable | Tagged information elements |

Each information element is:

```text
element_id: u8 | length: u8 | data[length]
```

Relevant observed elements include supported rates (`id=0x01`), channel
(`id=0x03`), TIM (`id=0x05`), and Nintendo vendor data (`id=0xdd`).

### Nintendo beacon vendor element — verified

The table starts at the first byte after the information element's `id` and `length`.

| Offset | Size | Field | Confidence |
|---:|---:|---|---|
| `0x00` | 4 | Nintendo OUI/type `00 09 bf 00` | Verified |
| `0x04` | 2 | Stepping offset | Documented |
| `0x06` | 2 | Video synchronization | Documented |
| `0x08` | 4 | Fixed/title-specific identifier | Opaque |
| `0x0c` | 4 | Game/application identifier | Opaque for PictoChat |
| `0x10` | 2 | Session stream code | Documented |
| `0x12` | 1 | Custom payload length | Verified |
| `0x13` | 1 | Beacon type; `1` for PictoChat/multicart | Verified |
| `0x14` | 2 | Host command data capacity | Verified |
| `0x16` | 2 | Client reply data capacity | Verified |
| `0x18` | Variable | Custom payload | Type-specific |

The PictoChat capture advertises command and reply capacities of `0x00c0`.

### PictoChat beacon custom payload — verified

Exactly eight bytes when `beacon_type=1` and `custom_length=8`.

| Offset | Size | Field | Captured examples |
|---:|---:|---|---|
| `0x00` | 4 | Opaque rotating beacon value | `0x473e2348`, `0x2fe6bd8a` |
| `0x04` | 1 | Room: `0=A`, `1=B`, `2=C`, `3=D` | `0` |
| `0x05` | 1 | User count including host | `1`, then `2` |
| `0x06` | 2 | Fixed suffix | `0x0004` |

The first four bytes rotate while room and user count remain unchanged. They are
not a stable membership or session identifier.

### Authentication — standard

Management subtype: `11`.

| Offset | Size | Field |
|---:|---:|---|
| `0x00` | 2 | Authentication algorithm |
| `0x02` | 2 | Exchange sequence |
| `0x04` | 2 | Status code |
| `0x06` | Variable | Optional information elements |

The captured host response used open-system algorithm `0`, sequence `2`, status `0`.
The request was not present in the baseline capture.

### Association request — standard outer shape

Management subtype: `0`.

| Offset | Size | Field |
|---:|---:|---|
| `0x00` | 2 | Capability flags |
| `0x02` | 2 | Listen interval |
| `0x04` | Variable | Information elements, including Nintendo's binary SSID |

### Association response — standard outer shape

Management subtype: `1`.

| Offset | Size | Field |
|---:|---:|---|
| `0x00` | 2 | Capability flags |
| `0x02` | 2 | Status code; zero is success |
| `0x04` | 2 | Association ID |
| `0x06` | Variable | Information elements |

The capture used association ID `0xc001`. Nintendo uses its low nibble as the
participant slot, so this client occupied slot `1`.

### Deauthentication — standard

Management subtype: `12`.

| Offset | Size | Field |
|---:|---:|---|
| `0x00` | 2 | Reason code |

The captured client leave used reason `3`.

## Nintendo multiplayer transport

### Host command — verified

Data subtype: `2` (CF-Poll). Destination: `03:09:bf:00:00:00`.

```text
client_time: u16
client_mask: u16
command_payload[...]       # envelope or short idle payload
```

| Offset | Size | Field | Common values |
|---:|---:|---|---|
| `0x00` | 2 | Client reply window | `998`; occasionally `214` |
| `0x02` | 2 | Invited-client bit mask | `0x0002` or `0x0000` |
| `0x04` | Variable | Command payload | See below |

The capture includes 2,451 commands with `(998, 0x0002)`, 2,144 with
`(998, 0x0000)`, and 16 with `(214, 0x0002)`.

Idle host commands were six bytes total and therefore contained only an opaque
trailing word after the four-byte multiplayer prefix. They do not contain a complete
six-byte application envelope.

### Client reply — verified

Data subtype: `1`, To-DS set. There is no multiplayer prefix:

```text
reply_payload[...]         # envelope, two-byte status, or empty
```

Observed non-envelope replies were empty, `00 00`, or `00 80`. Their bit-level
meaning is unknown.

### Host acknowledgement — verified

Data subtype: `1`. Destination: `03:09:bf:00:00:03`.

| Offset | Size | Field |
|---:|---:|---|
| `0x00` | 2 | Command count |
| `0x02` | 2 | Failed-client mask |

The body is exactly four bytes. Of 4,648 captured acknowledgements, 4,576 used
failure mask `0`, while 72 used mask `0x0002`.

## Application envelope

Application-bearing host commands and client replies use a six-byte envelope:

| Offset | Size | Field |
|---:|---:|---|
| `0x00` | 2 | Packet ID |
| `0x02` | 2 | Packet sequence |
| `0x04` | 2 | Declared length |
| `0x06` | Variable | Packet payload |

Length conventions differ by flow:

```text
host:   declared_length == packet_payload.len()
client: declared_length == packet_payload.len() + 2
```

The client convention was consistent across every recognized envelope in the
baseline. It is modeled as a wire convention, not as two bytes lost from the PCAP.
Ordinarily the packet ID's low byte equals `declared_length / 2`; consequently
application envelopes declare an even logical length.

The canonical outbound capture proves one host-side exception. A rebroadcast short
final fragment uses packet ID `0x9e56` to describe its 172-byte **physical padded
payload**, while the envelope declares its 16-byte **logical payload**. The exception
is restricted to host transfer-family frames whose packet-ID length matches the
physical payload and whose declared length does not exceed it.

## Observed packet-ID families

The table lists lengths from the envelope. “Wire payload” is the number of bytes
actually present after the six-byte envelope.

| Family | Flow and IDs | Sequence | Declared / wire payload | Count | Current interpretation |
|---|---|---:|---:|---:|---|
| `c34` | Host `0x1c34`, `0x9c34` | usually 5 | `104 / 104` | 2,380 | Member roster; first instance requests client session state |
| `c34` | Client `0x8c34` | 6 | `104 / 102` | 1 | Initial client session response |
| `d0a` | Host `0x1d0a`, `0x9d0a` | 1 | `20 / 20` | 6 | Transfer announcement/control |
| `d0a` | Client `0x0d0a`, `0x8d0a` | 0 or 3 | `20 / 18` | 6 | Client transfer announcement/control reply |
| `e30` | Host `0x1e30`, `0x9e30` | 2 | `96 / 96` | 6 | 84-byte identity/profile fragment |
| `e30` | Client `0x0e30`, `0x8e30` | 2 | `96 / 94` | 2 | 84-byte client identity/profile fragment |
| `e56` | Host `0x1e56`, `0x9e56` | 2 | `172 / 172` | 56 | Full message fragment |
| `e56` padded final | Host `0x9e56` | 2 | `16 / 172` | Canonical outbound capture | Four-byte logical final in a full physical transfer slot |
| `e56` | Client `0x8e56` | 2 | `172 / 170` | 13 | Full message fragment |
| `e48` | Host `0x1e48`, `0x9e48` | 2 | `144 / 144` | Subsequent capture | Host final fragment with 132 data bytes |
| `e38` | Host `0x1e38` | 2 | `112 / 112` | 2 | Host-originated final fragment with 100 data bytes |
| `e08` | Host `0x9e08` | 2 | `16 / 16` | Earlier real-client capture | Unpadded four-byte host final shape; not used by the canonical virtual-client rebroadcast |
| `e08` | Client `0x8e08` | 2 | `16 / 14` | 1 | Four-byte final message fragment |

For ordinary packets, the packet ID low byte is the declared length in 16-bit words
(`0x56 * 2 = 172`, for example). For the padded host final it instead matches physical
payload length. The low nibble of the high byte identifies the family. The meaning of
the remaining high bits, including `0x8000`, is not established; IDs are retained
exactly rather than normalized.

### Drawing announcement/control (`d0a`)

A client drawing begins with an `0x8d0a` envelope at packet sequence `0`. Its
18-byte payload has this controlled-capture shape:

| Offset | Size | Field | Confidence |
|---:|---:|---|---|
| `0x00` | 1 | Participant slot | Verified |
| `0x01` | 1 | Zero | Observed constant |
| `0x02` | 2 | `0xffff` | Observed constant |
| `0x04` | 4 | Reassembled drawing length | Verified; `2084` for one band |
| `0x08` | 4 | Command counter | Progresses by 3 between captured sends |
| `0x0c` | 4 | Opaque per-transfer nonce | Must change; generation remains unknown |
| `0x10` | 2 | Transfer counter | Progresses by 2 between captured sends |

The host accepts the announcement by echoing the first 16 payload bytes in a
20-byte `0x1d0a`/`0x9d0a` payload. The two host-only tail bytes remain opaque. A
client completion uses `0x8d0a` packet sequence `3`; its transfer counter equals the
corresponding announcement counter plus one.

Two messages in one verified virtual session used fragment frame ranges `3..16` and
`17..30`, respectively, while both retained opaque transfer tag `0x0000`. Replaying
the first drawing's `d0a` control values caused the host to omit the second
announcement echo and the DS rendered only the first drawing.

## Join identity exchange — verified

The first 120 milliseconds after the baseline association show this application
sequence. Intervening polls receive empty or `00 80` replies.

```text
host c34 member roster     → client c34 session response
host d0a announces 84 B   → client d0a announces its 84 B profile
host e30 host profile      → client zero-payload reply
host c34 member roster     → client e30 virtual profile
host e30 rebroadcast       → client idle reply
```

The firmware-assisted client keys state changes to those normalized host IDs rather
than poll count, so duplicate host packets do not skip a reply stage. The complete
opening-session and two-pass identity selector is documented in
[pictochat-protocol.md](pictochat-protocol.md).

### Identity/profile record

The `e30` fragment data is exactly 84 bytes:

| Offset | Size | Field | Confidence |
|---:|---:|---|---|
| `0x00` | 1 | Record magic `3` | Verified |
| `0x01` | 1 | Identity subtype `0` or `1` | Verified |
| `0x02` | 6 | Subject MAC, each adjacent byte pair swapped | Verified |
| `0x08` | 20 | Nickname, fixed UTF-16LE, up to 10 code units | Verified |
| `0x1c` | 52 | Personal message, fixed UTF-16LE | Verified layout |
| `0x50` | 2 | Profile colour, little-endian | Capture-derived |
| `0x52` | 1 | Birth month/day field | Capture-derived |
| `0x53` | 1 | Birth day/month field | Capture-derived |

The last two fields' order remains unresolved; Picto Relay preserves configurable
safe values. In the baseline client record, MAC `00:16:56:4d:27:ca` appears as
`16 00 4d 56 ca 27`.

## Message-fragment packet body

The packet body inside `e56`, `e48`, `e38`, and `e08` envelopes has this shape:

| Offset | Size | Field | Confidence |
|---:|---:|---|---|
| `0x00` | 2 | Opaque fragment header | Opaque; examples begin `01 6c`, `01 9b`, `01 04` |
| `0x02` | 1 | Fragment data length | Verified |
| `0x03` | 1 | Final marker: `0` or `1` | Verified |
| `0x04` | 2 | Reassembled-message byte offset | Verified |
| `0x06` | 2 | Opaque identifier | Opaque; zero in captured message transfer |
| `0x08` | `N` | Message data | `N = data_length` |
| After data/padding | 2 | Frame number | Client: immediately after data. Host: final four physical bytes begin here. |
| Final 2 physical bytes | 2 | Host-only trailer | Host commands only; observed value `2` |

### Full client fragment

```text
fragment header  8 bytes
message data    160 bytes
frame number      2 bytes
-------------------------
wire payload     170 bytes
declared length  172
```

### Full host fragment

```text
fragment header  8 bytes
message data    160 bytes
frame number      2 bytes
host trailer      2 bytes
-------------------------
wire payload     172 bytes
declared length  172
```

### Captured client final fragment

```text
opaque header    01 04
data length      04
final marker     01
data offset      20 08       # 0x0820 = 2080
opaque id        00 00
message data     00 00 00 00
frame number     10 00       # 16
```

The packet body is 14 bytes and its client envelope declares 16.

### Canonical padded host final fragment

The physical DS host rebroadcasts that logical tail using a full transfer slot:

```text
packet ID          0x9e56      # physical payload is 0x56 * 2 = 172
packet sequence    2
declared length    16          # logical payload length
opaque header      01 04
data length        04
final marker       01
data offset        20 08       # 0x0820 = 2080
opaque id          00 00
message data       00 00 00 00
zero padding       156 bytes
frame number       2b 00       # 43 in this host transfer
host trailer       02 00
```

The host parser must take message data from the start according to `data_length`, then
take frame number and trailer from the end of the physical payload. Treating the frame
number as immediately following four logical data bytes reads padding and prevents
message completion.

Completion is determined by `final_marker=1`; a short-fragment heuristic is incorrect.

## Reassembled PictoChat message

### Client-originated message — verified complete

```text
message metadata   36 bytes
drawing payload  2048 bytes
--------------------------
total            2084 bytes
```

Metadata captured from the client:

```text
03 02 16 00 4d 56 ca 27 00 04 00 00 00 00 01 04
09 0b 10 1b 00 00 00 00 00 00 00 00 00 00 00 00
00 00 00 00
```

Known subshape:

| Offset | Size | Field |
|---:|---:|---|
| `0x00` | 2 | Opaque metadata prefix |
| `0x02` | 6 | Sender MAC encoded as three little-endian `u16` words |
| `0x08` | 28 | Opaque metadata |
| `0x24` | 2,048 | Drawing payload |

For sender `00:16:56:4d:27:ca`, bytes `16 00 4d 56 ca 27` decode wordwise to that MAC.
Drawing data uses the standard Nintendo DS tiled 4bpp format. Each 8x8 tile occupies
32 bytes; tiles run left-to-right across 32 columns and then top-to-bottom. Within a
byte, the low nibble is the left pixel and the high nibble is the right pixel. One
2,048-byte band therefore represents 256x16 pixels. Palette index 1 was black in the
controlled capture.

### Host-originated message — verified size, incomplete capture

The host transfer's final offset and length establish a total of 6,180 bytes, encoding
three 16-row bands of a 256x48 drawing. This is the largest shape verified in our
captures, not the native surface limit:

```text
message metadata   36 bytes
drawing payload  6144 bytes
--------------------------
total            6180 bytes
```

Four host fragments were not captured, so this message cannot yet be reconstructed
byte-for-byte. Its metadata begins:

```text
03 02 24 00 67 f3 87 41 00 05 00 00 00 00 03 06 ...
```

Bytes `24 00 67 f3 87 41` encode host MAC `00:24:f3:67:41:87` using the same wordwise
ordering.

## Shape recognition order

A decoder should classify in this order:

1. Strip radiotap according to its declared length and note whether FCS is present.
2. Parse frame control and the 802.11 address layout.
3. Remove FCS only when capture metadata says it exists.
4. Dispatch management subtypes to beacon/authentication/association/deauthentication.
5. For data, infer Nintendo flow from the multicast address placement.
6. Parse the flow-specific multiplayer prefix.
7. Treat payloads shorter than six bytes as idle/status traffic, not envelopes.
8. Parse and validate the direction-specific application-envelope length, including
   the host transfer-family padded-final exception.
9. Dispatch by the exact packet ID.
10. Parse transfer-family (`0x0e`) fragments after validating logical and physical
    lengths; for host frames, locate the four-byte trailer at the physical end.
11. Reassemble by source, opaque ID, byte offsets, and correlated frame numbers, using
    the explicit final marker.
12. Decode completed drawing transfers as 36 metadata bytes followed by one to five
    2,048-byte Nintendo DS 4bpp tile bands. Captures currently verify one and three.

## Known gaps

- Remaining random/session bytes inside `c34` and `d0a`; the working subtype-1
  exchange is captured, but several field meanings remain unknown.
- Packet-ID high-bit semantics.
- Opaque fragment header, zero-valued transfer tag semantics, frame-counter wraparound,
  and host trailer.
- The 36-byte message metadata beyond sender MAC.
- Palette/color meanings beyond observed black index 1.
- Exact retry behavior when acknowledgement failure bits are set.
- Multi-client scheduling and buffer lifecycle across many firmware rearm cycles. A
  single-client disarm, deauthentication, and clean host slot release are verified.

No opaque field should receive a semantic name until controlled captures vary it in
isolation.

## Executable fixtures

| Fixture | Shape demonstrated |
|---|---|
| `pictochat-room-a-beacon.hex` | Historical one-user Room A beacon |
| `joined-room-a-beacon.hex` | Captured two-user Room A beacon |
| `client-first-fragment.hex` | To-DS client message fragment |
| `host-first-fragment.hex` | Host rebroadcast with multiplayer prefix and trailer |
| `host-ack.hex` | Four-byte host acknowledgement |
| `client-final-fragment.hex` | Explicit final marker and four-byte tail |
| `host-padded-final-fragment.hex` | Real host `0x9e56` final with 16 logical and 172 physical bytes |
| `captured-client-transfer.hex` | All 14 fragments of the complete 2,084-byte message |

These live in `crates/picto-relay-codec/tests/fixtures/` and are exercised by
`tests/captured_transport.rs` and `tests/golden.rs`.
