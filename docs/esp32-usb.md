# ESP32 USB bridge

The computer runs the HTTP/WebSocket server and the drawing codec. The
ESP-WROOM-32 runs the radio side in C: channel discovery, authentication,
association, the profile exchange, poll replies and delivery tracking. It joins a
DS-hosted room; it cannot host one.

There is one configuration and no negotiation. Firmware and server are built from
the same tree; a mismatch fails at the first command because the framing version
differs.

| Setting | Value | Defined in |
| --- | --- | --- |
| UART | 921600 baud, 8N1, UART0 | `LINK_BAUD` / `link::BAUD` |
| Drawing fragment | 120 bytes | `LINK_FRAGMENT_BYTES` / `link::FRAGMENT_BYTES` |
| Framing version | 2 | `LINK_VERSION` / `link::VERSION` |

The firmware side is `firmware/esp32/main/link.{h,c}`; the host side is
`crates/picto-relay-link`. Both are tested against the same fixture,
`firmware/esp32/test/fixtures/link.bin`.

Only one process may open the port. On macOS the server holds the device open
while running `stty`, because closing the final descriptor can restore the
terminal settings.

## Framing

A packet is: version u8, kind u8, request ID u32 LE, payload length u16 LE,
payload (0–256 bytes), then IEEE CRC32 LE over header and payload. Packets are
delimited by `0x7e`; `0x7e` and `0x7d` inside a packet are escaped as `0x7d`
followed by the byte XOR `0x20`. Maximum encoded size is 538 bytes. Noise,
corrupt CRCs and incomplete frames are discarded; the next delimiter restores
framing.

## Commands (host → ESP32)

| Kind | Command | Payload |
| --- | --- | --- |
| 1 | HELLO | Empty. Response: station MAC (6 bytes) |
| 2 | JOIN | Room 0–3, channel 1–14, nickname (20 bytes UTF-16LE), personal message (52 bytes UTF-16LE), color 0–15. Always 75 bytes, zero-padded |
| 3 | LEAVE | Empty. Stop replying and send a deauthentication |
| 4 | PING | Empty. Keeps the USB owner alive |
| 5 | BEGIN | Transfer ID u32 LE, body count u8 (3–132) |
| 6 | BODIES | Transfer ID u32 LE, first index u8, packed body records |
| 7 | COMMIT | Transfer ID u32 LE |
| 8 | DISCOVER | Channel 1–14, only while idle. Response: member counts for rooms A–D, `255` when no beacon was heard in the last three seconds |

Every command gets a response, kind 128, with the same request ID and a status
byte (0 success, 1 invalid or busy) followed by any response data. The host
retries the same request every 150 ms for up to three seconds. The firmware
remembers its latest successful command by ID and payload checksum and replays
the response instead of running it twice. A connection begins with HELLO and
LEAVE.

### Uploading a drawing

The host encodes the whole drawing into application bodies: an announcement,
the fragments, and a completion control. It sends BEGIN, the bodies in order,
then COMMIT, which enables transmission. The firmware sends one body per DS
poll. The next upload must wait for delivery of the previous one.

BODIES carries consecutive records up to the payload limit. Each record is a tag
byte followed by data:

- Tag 1–127: that many raw body bytes.
- Tag with the high bit set: the low seven bits give the encoded length.
  Nonzero bytes are literal; `0, N` expands to N zeros (N nonzero).
- Tag 0: extended raw record `0, length, body` for bodies of 128–136 bytes.

A decoded body is 16–136 bytes. The firmware validates the entire batch before
storing any of it, so a retried or malformed batch cannot be partially consumed.

Delivery means the DS echoed the announcement and every fragment exactly, and
acknowledged the completion. Acceptance by the radio driver alone does not count.

## Events (ESP32 → host)

- **129 STATUS**, every 250 ms while a host is attached. Packet ID is the current
  transfer ID. Payload is 44 bytes:

  | Offset | Field |
  | --- | --- |
  | 0 | State: 0 off, 1 discovering, 2 authenticating, 3 associating, 4 connected |
  | 1 | Profile phase; 12 means ready |
  | 2 | Bodies sent and acknowledged |
  | 3 | Body count of the current upload |
  | 4 | Slot (association ID) |
  | 5 | Room |
  | 6 | Delivered flag |
  | 7 | Reserved |
  | 8 | Station MAC (6 bytes) |
  | 14 | Host MAC (6 bytes) |
  | 20 | Six u32 LE counters: polls, replies, positive ACKs, negative ACKs, disconnect reason, echoed fragments |

- **130 RX**: a DS application envelope, without the 802.11 and Nintendo poll
  headers. Rosters, controls and fragments only; each distinct payload once.
- **131 GAP**: number of packets lost to the bounded USB output queue, u32 LE.
  The host discards partial reassemblies and reports an error.
- **132 DIAG**: radio timing counters, only from firmware built with
  `-DPICTO_DIAGNOSTICS=ON`. A list of u32 LE values in the order of the enum in
  `firmware/esp32/main/diag.h`. The server logs them at debug level.

## Timeouts and limits

- The host sends PING every second. After five seconds without any command the
  firmware leaves the room.
- A session ends after five seconds without a frame from its DS host.
- The serial worker reconnects on I/O errors, a missing status stream or a
  delivery timeout. Interrupted drawings fail; they are not resent into a new
  session.
- Queues are bounded: firmware RX and UART output hold 64 packets each, the host
  holds 256 serial events and four pending drawings. Overflow is reported, never
  silently dropped.
- One physical station, one web session. The server scans channels 1–13 while
  idle (250 ms per channel, about every five seconds) and before joining;
  `--channel` is scanned first.

## History

Timing measurements and the experiments that led to this configuration are in
[history/esp32-latency.md](history/esp32-latency.md). Raw evidence is in the
[capture archive](../captures/captures.tar.gz).
