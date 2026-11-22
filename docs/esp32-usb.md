# ESP32 USB protocol

The computer runs the HTTP/WebSocket server and encodes drawings into the DS's
native format. The ESP-WROOM-32 does everything on the radio, in C: finding the
room, joining, the profile exchange, splitting a drawing into fragments,
answering polls and tracking delivery. It joins a DS-hosted room; it cannot
host one.

The host is usually the relay server. The web client's demo page is one too:
it speaks this protocol from the browser, over Web Serial, with the same Rust
code compiled to WebAssembly (see the [web client](../services/web/README.md#the-demo-page)).

The protocol is defined once, in `crates/picto-relay-link/src/lib.rs`. The
firmware's `main/link_protocol.h` is generated from it (`just protocol-header`)
and a test fails if the checked-in header is stale. Names below are the Rust
constants; the C header prefixes them with `LINK_`.

| Setting | Value |
| --- | --- |
| UART | 921600 baud, 8N1, UART0 |
| Protocol version | 3 |
| Largest command payload | 1024 bytes |
| Largest response or event payload | 256 bytes |

Only one process may open the port. On macOS the server holds the device open
while running `stty`, because closing the final descriptor can restore the
terminal settings.

## Framing

A packet is: kind u8, ID u32 LE, payload length u16 LE, payload, then IEEE CRC32
LE over header and payload. Packets are delimited by `0x7e`; `0x7e` and `0x7d`
inside a packet are escaped as `0x7d` followed by the byte XOR `0x20`. Noise,
corrupt CRCs and incomplete packets are discarded; the next delimiter restores
framing.

## Commands (host → ESP32)

| Kind | Command | Payload |
| --- | --- | --- |
| 1 | HELLO | The host's protocol version (1 byte) |
| 2 | JOIN | Room 0–3, nickname (20 bytes UTF-16LE), personal message (52 bytes UTF-16LE), color 0–15. Always 74 bytes, zero-padded |
| 3 | LEAVE | Empty. Leave the room, telling the DS |
| 4 | PING | Empty. Keeps the session alive |
| 5 | SCAN | Empty. While idle, sweep channels 1–13 once and send a ROOMS event |
| 6 | DRAW | Transfer ID u32 LE, position u16 LE, total length u16 LE, then drawing bytes |
| 7 | TRACE | 1 to start capturing radio frames, 0 to stop |

Every command gets a RESPONSE (kind 128) with the same ID: a status byte, then
any response data.

| Status | Meaning | Host reaction |
| --- | --- | --- |
| 0 OK | Done | |
| 1 BUSY | Try again shortly | Retried every 150 ms for up to 3 s |
| 2 INVALID | Malformed command | Fails immediately |
| 3 WRONG_STATE | Not possible in the current session state | Fails immediately |
| 4 VERSION_MISMATCH | HELLO from a different protocol version | Tells the user to reflash |
| 5 NOT_ATTACHED | A command arrived before HELLO | Fails immediately |

A command with no response is also resent every 150 ms. The firmware remembers
its latest successful command by ID and payload checksum and replays the
response instead of running it twice.

### HELLO

HELLO is always answered, even when the versions differ, with: the firmware's
protocol version, its station MAC address, and a build label (`git describe` of
the tree the firmware was built from). On a mismatch the server reports both
versions and the build instead of timing out. Nothing else is accepted, and no
events are sent, until a HELLO succeeds.

### JOIN and channel search

JOIN carries no channel. The firmware tunes through channels 1–13, 250 ms each,
until it hears a beacon for the requested room that also advertises the tuned
channel (beacons leak in from neighboring channels). It starts with the channel
that room was last heard on, then 1, 7 and 13, which DS hosts favor, then the
rest. If nothing is found within seven seconds the session ends with
`LEFT_NO_HOST`.

### DRAW: sending a drawing

The host uploads the native drawing (2,084–10,276 bytes) in chunks of up to
1,016 bytes. A chunk at position 0 starts a new upload, replacing any
unfinished one; later chunks must follow on exactly. The chunk that completes
the total length queues the drawing.

The firmware builds the announcement, the 120-byte fragments and the completion
itself, including the per-drawing counters and frame numbers the DS expects to
advance through a session, and sends one per poll.

It holds two drawings: the one being sent and one waiting. A third upload is
answered BUSY until the first is delivered, so the host may upload the next
drawing while the current one is still on the air.

Delivery means the DS echoed the announcement and every fragment exactly and
acknowledged the completion. Acceptance by the radio driver alone does not count.

### Rooms with several DSes

When more than one client is in the room, the DS host polls them together: its
poll carries a mask of slots and a per-client reply window (998 µs observed).
The clients answer one after another in slot order, each in its own window, and
the host sends one acknowledgement afterwards with a bit per missed slot.

The firmware answers in the relay's own window whatever its slot. It counts the
polled slots below its own and waits that many windows (window plus a 10 µs gap)
from the moment the poll arrived before transmitting, so its reply sits at the
same offset into its window as an immediate reply does when it is polled alone.
It then accepts the host's verdict until after the last polled client's window.

The slot order and the 10 µs gap come from how DS emulators implement this
protocol. They have since held up on hardware: the relay joined as slot 2
behind a real DS and delivered drawings.

How members learn about each other, and what the relay sends when joining, is
described in [the protocol reference](pictochat-protocol.md#rooms-with-several-clients).

## Events (ESP32 → host)

Events are sent when they happen, not on the status timer.

| Kind | Event | Payload |
| --- | --- | --- |
| 129 | STATUS | See below. Packet ID is the drawing being sent, or 0 |
| 130 | RX | A DS application packet from the room host |
| 131 | GAP | u32 LE count of events dropped because the output queue was full |
| 132 | DIAG | Radio timing counters; only from `PICTO_DIAGNOSTICS` builds |
| 133 | READY | Host MAC (6), slot, room, channel. The profile exchange finished |
| 134 | DELIVERED | Empty. Packet ID is the delivered drawing's transfer ID |
| 135 | LEFT | Reason u8, IEEE code u16 LE. The session ended |
| 136 | ROOMS | Member counts for rooms A–D after a SCAN; 255 for a room not heard |
| 137 | FRAME | A captured radio frame, while TRACE is on: radio clock u32 LE (µs), flags u8 (bit 0: sent by the relay), on-air length u16 LE, then up to 249 frame bytes |

**STATUS** is a heartbeat: every second, or every 250 ms while a drawing is being
sent. 24 bytes:

| Offset | Field |
| --- | --- |
| 0 | State: 0 off, 1 searching, 2 authenticating, 3 associating, 4 connected |
| 1 | Profile phase; 12 means ready |
| 2 | Slot (association ID) |
| 3 | Room |
| 4 | Wi-Fi channel |
| 5 | Fragments acknowledged by the DS |
| 6 | Fragments in the drawing being sent |
| 7 | Fragments echoed back by the DS |
| 8 | Four u32 LE counters: polls, replies, positive ACKs, negative ACKs |

**LEFT** reasons:

| Reason | Meaning |
| --- | --- |
| 0 REQUESTED | The host sent LEAVE, or stopped sending commands |
| 1 NO_HOST | No DS is hosting the room on any channel |
| 2 JOIN_TIMEOUT | The DS did not answer authentication or association |
| 3 REJECTED | The DS refused the join; the code is its IEEE status |
| 4 KICKED | The DS removed us; the code is its IEEE reason |
| 5 HOST_LOST | Nothing heard from the DS for five seconds |
| 6 RADIO_STALLED | The radio driver never reported a transmission as finished |

**RX** carries rosters, transfer controls and fragments, without the 802.11 and
Nintendo poll headers. A DS host repeats each payload many times; the firmware
forwards a packet only when it differs from the last one of its kind, ignoring
the flag bit and roster counters that change on every repeat. The server
reassembles drawings and tolerates duplicates.

**DIAG** is a list of u32 LE values in the order of the enum in
`firmware/esp32/main/diag.h`. The server logs them at debug level.

## Capturing radio traffic

The ESP32 hears every frame in the room, including other DSes' replies, so it
doubles as a capture device. Start the server with `--capture FILE` (or
`just server-capture`) and it turns TRACE on for each session and appends every
PictoChat frame heard or sent to a pcap file that Wireshark opens directly.

Captured are local-multiplayer data frames (polls, replies, acknowledgements)
and join/leave management frames; beacons are not. Frames longer than 249 bytes
are cut short. Frames appear in the order the firmware handled them, which is
reliable. The timestamps are not yet: in the first capture, runs of received
frames carried identical times, and sent frames are stamped from a different
clock reading than received ones. Use frame order, not timestamps, until that
is sorted out.

## Timeouts and limits

- The host sends PING every second. After five seconds without any command the
  firmware leaves the room and stops sending events.
- A session ends after five seconds without a frame from its DS host.
- The server reconnects on I/O errors, a LEFT event, a missing status stream or
  a delivery timeout. Interrupted drawings fail; they are not resent into a new
  session.
- Queues are bounded: firmware RX and UART output hold 64 packets each, the
  server holds 256 events and four drawings beyond the two in the firmware.
  Overflow is reported, never silent.
- One physical station, one web session. While no session is open the server
  asks for a SCAN about every five seconds to show room occupancy.

## Changing the protocol

Edit the constants in `crates/picto-relay-link/src/lib.rs`, run
`just protocol-header`, and update both implementations. `just test` checks
that the generated header is current, that the Rust and C framing agree on a
shared fixture, and that the C firmware produces byte-for-byte the drawing
bodies the Rust codec defines.

## History

Timing measurements and the experiments that led to this design are in
[history/esp32-latency.md](history/esp32-latency.md). Raw evidence is in the
[capture archive](../captures/captures.tar.gz). Those documents describe an
earlier protocol in which the host built every radio body and uploaded them
with BEGIN/BODIES/COMMIT.
