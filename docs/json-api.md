# Picto Relay REST API v1

Base URL: `http://localhost:3001/api/v1`. The browser uses this API; WebSockets
are optional. REST polling supports the complete session lifecycle. The version
is in the path; bodies carry no version field.

| Operation | Endpoint |
|---|---|
| Formats, limits, and drawing bounds | `GET /status` |
| Current room counts and roster | `GET /rooms` |
| Join with nickname, personal message, and profile color | `POST /sessions` |
| The session's room and everyone seen in it | `GET /sessions/{id}` |
| Send text or a drawing | `POST /sessions/{id}/messages` |
| Drain queued events | `GET /sessions/{id}/events` |
| Leave | `DELETE /sessions/{id}` |
| Optional live room snapshots | WebSocket `/rooms/ws` |
| Optional session event stream | WebSocket `/sessions/{id}/events/ws` |
| Machine-readable description of all of the above | `GET /openapi.json` |

## OpenAPI schema

The API is described by an OpenAPI 3.1 document, generated from the server's
handlers and types so it cannot drift from the implementation:

- [`docs/openapi.json`](openapi.json) is checked in, for generating clients
  without running anything.
- A running server serves the same document at `/api/v1/openapi.json`.

Point any OpenAPI generator at either, for example:

```sh
npx openapi-typescript docs/openapi.json -o picto-relay.d.ts
openapi-generator-cli generate -i docs/openapi.json -g python -o client/
```

Notes for generated clients:

- A session event is a `SessionEvent`, selected by its `type` field
  (`session_ready`, `message`, `delivered`, ...). The same shape arrives from
  polling and from the WebSocket.
- OpenAPI cannot describe WebSocket messages. The two `/ws` endpoints are
  listed with a `101` response; their text frames are `RoomsResponse` and
  `SessionEvent` JSON, both defined in the schema.
- Identifiers are unsigned 32-bit values and are declared as 64-bit integers so
  typed clients do not overflow.

After changing a handler or a type in `protocol.rs`, run `just openapi` to
rewrite the checked-in document; `cargo test` fails while it is stale.

## Run

The default `memory` backend loops drawings back to clients in the same room. It
does not emulate physical profile exchange or prove radio delivery.

```sh
cargo run -p picto-relay-server
```

For the connected ESP32:

```sh
cargo run -p picto-relay-server -- \
  --backend esp32 --serial /dev/cu.usbserial-0001
```

The physical backend supports one API session at a time, joining a room hosted
by a DS. See [firmware and USB protocol](esp32-usb.md).

## Formats and limits

`GET /status` reports:

- `backend`
- `bitmap_width: 256`, `bitmap_height: 96`
- `bitmap_bytes: 3072`, `palette_bitmap_bytes: 12288`
- `ink_indices: [1,2,3,4,5,6,7,8,9,10,11,12,13,14,15]`
- `text_max_units: 220`, `profile_message_max_units: 26`, `profile_color_max: 15`
- `drawable_regions: [[80,0,172,16],[24,16,228,64]]`

Regions are `[x,y,width,height]`, measured conservatively from the user's DS
sample. The first region avoids the name tag; the second is the lower drawing
area. The native image is at most 256×80; the API retains its padded 256×96
surface. Bitmap callers should keep ink inside these regions. Legacy taller
bitmaps are fitted to the native height by the encoder. Incoming pixels are
preserved, including ink outside the suggested composer bounds.

## Discover rooms

```sh
curl http://localhost:3001/api/v1/rooms
```

The response contains four `rooms` (0=A, 1=B, 2=C, 3=D):

```json
{"observed":true,"room":0,"participant_count":2,"participants":[{"client_id":4083630471,"name":"Rob"},{"client_id":3209822209,"name":"Relay"}]}
```

While idle, the server asks the firmware to scan Wi-Fi channels 1–13 about every
five seconds, listening for 250 ms on each channel. Names require joining;
a positive count with an empty `participants` array is valid. `observed: false`
means unknown, not an empty room. Passive discovery pauses during a session,
whose roster then supplies its count and names. Joining scans for the selected
room and tunes to its observed channel before authenticating. A DS must already
be hosting that room.

## Join with profile settings

```sh
curl -i -H 'content-type: application/json' \
  -d '{"name":"API Relay","room":0,"profile_message":"Hello from REST","profile_color":2}' \
  http://localhost:3001/api/v1/sessions
```

Response: HTTP 201 with `{"session_id":1,"room":0}`.

- `name`: required; whitespace is trimmed and the name normalized by the server.
- `room`: optional, defaults to the server's `--room`; range 0–3.
- `profile_message`: optional, defaults to empty; at most 26 UTF-16 code units,
  with no embedded NUL. Supplementary Unicode characters use two units.
- `profile_color`: optional, defaults to the original wire value 0; range 0–15.

| Index | Profile color | Index | Profile color |
|---:|---|---:|---|
| 0 | Gray/default | 8 | Dark green |
| 1 | Brown | 9 | Teal |
| 2 | Red | 10 | Light blue |
| 3 | Pink | 11 | Blue |
| 4 | Orange | 12 | Dark blue |
| 5 | Yellow | 13 | Dark purple |
| 6 | Lime | 14 | Purple |
| 7 | Green | 15 | Magenta |

Labels follow the [libnds theme table](https://github.com/devkitPro/libnds/blob/master/include/nds/system.h).
Red and the full 26-letter personal message were verified on a real DS.
Profile settings are advertised in both identity passes, separately from drawings.
Changing them requires leaving and creating a new session. HTTP 201 allocates the
session; the physical `session_ready` event confirms the completed profile exchange.

## Inspect a session

```sh
curl http://localhost:3001/api/v1/sessions/1
```

```json
{"room":0,"room_letter":"A","users":[
  {"client_id":3209822209,"name":"API Relay","active":true},
  {"client_id":4083630471,"name":"Rob","active":true},
  {"client_id":2694480327,"name":"Bob","active":false}]}
```

`users` is everyone the session has seen in its room since it began, the
session's own user included, in the order they were first seen. Someone who has
left stays in the list with `active: false`, and becomes `active` again if they
return. `client_id` is the same identity rosters use, and the `station_id` of
that person's messages and profile.

## Send text

```sh
curl -i -H 'content-type: application/json' \
  -d '{"text":"Hello from the REST API","ink":"rainbow"}' \
  http://localhost:3001/api/v1/sessions/1/messages
```

`ink` defaults to black (`1`). Supply a numeric fixed pen index from `/status`, or
`"rainbow"`. Profile colors and drawing ink are different palettes.

| Index | Ink | Index | Ink |
|---:|---|---:|---|
| 1 | Black | 9 | Lime |
| 2 | Gray | 10 | Green |
| 3 | Magenta | 11 | Teal |
| 4 | Pink | 12 | Cyan |
| 5 | Red | 13 | Light blue |
| 6 | Red orange | 14 | Blue |
| 7 | Orange | 15 | Dark blue |
| 8 | Yellow | | |

The names are how each index looks on a real DS, read from a photograph of one
showing all sixteen indices side by side. `"rainbow"` cycles through 3 to 15 in
that order.

Text is drawn exactly as the web client's keyboard types it: in the DS font,
one line of text per ruled line, the first line starting beside the name tag.
Both draw from the same table of glyphs, so the pixels are identical. The font
covers Latin letters, kana, common kanji, symbols and the DS pictograms; a
character it lacks is drawn as a hollow box.

Words wrap whole where they can, explicit newlines are kept, and a word longer
than a line is split. A drawing holds five lines. Text must contain 1–220
UTF-16 units and no NULs; text needing more than five lines is rejected with
`invalid_text`. Rainbow text changes color every six pixels across.

## Send a drawing

Supply exactly one format: `text`, `bitmap`, or `palette_bitmap`. `ink` applies
only to text; a bitmap already specifies every pixel's ink.

Monochrome: base64 encoding of exactly 3,072 bytes, row-major, eight pixels per
byte, most-significant bit first. Zero is paper, one is black.

```json
{"bitmap":"BASE64_MONOCHROME_BYTES"}
```

Color: base64 encoding of exactly 12,288 bytes, row-major, two 4-bit indices per
byte. The left pixel occupies the low nibble. Byte `0xf3` means indices 3 then 15.

```json
{"palette_bitmap":"BASE64_INDEXED_BYTES"}
```

Indices: 0 paper, and the inks listed under [Send text](#send-text). Every index
is carried as given. RGB previews are approximate;
wire indices are lossless. Color uses the same native radio payload size as mono.

Both formats use `POST /sessions/{id}/messages`. HTTP 202, with no body,
acknowledges queueing, not DS delivery.
Blank physical drawings are rejected by the native encoder through error events.
UI brush selection, erasing, and clearing are local editor actions; an API client
implements these by changing its next bitmap. There is no shared server-side canvas.

## Receive and track delivery using REST

```sh
curl http://localhost:3001/api/v1/sessions/1/events
```

Polling drains up to 32 queued events and returns `{"events":[...]}`.
Poll again when the list is empty. Examples of individual events:

```json
{"type":"session_ready","client_id":1}
{"type":"profile","station_id":4083630471,"name":"Rob","profile_message":"abcdefghijklmnopqrstuvwxyz","profile_color":11}
{"type":"message","station_id":4083630471,"name":"Rob","message_id":0,"palette_bitmap":"..."}
{"type":"delivered","message_id":0}
```

`profile` is emitted when a physical profile is first observed or its nickname,
message, or color changes. The duplicate identity pass is suppressed. Its
`station_id` matches the roster's `client_id` for that physical station. Profiles
are event updates, not a historical REST profile database.

Every drawing arrives as a `palette_bitmap`, in the format described under
[Send a drawing](#send-a-drawing), whatever format it was sent in: a drawing
sent as a one-bit `bitmap` comes back as black ink. Text submissions arrive as
drawings too.

Other events: `backend_state`, `participants`, `participant_joined`,
`participant_left`, `delivery_timed_out`, and `error`. Roster snapshots are
authoritative. Physical `delivered` requires the matching announcement and complete
host-rebroadcast drawing; timeouts are reported separately. Message IDs belong to
the sending station/session, not a globally unique namespace.

Each session has one event consumer. Attaching its WebSocket makes REST polling
return HTTP 409 `session_stream_attached`. Closing the socket restores polling.
Room WebSocket observers do not own the session queue and may coexist freely.
Queues are bounded: consume events continuously while connected.

## Leave

```sh
curl -i -X DELETE http://localhost:3001/api/v1/sessions/1
```

HTTP 204 disconnects the virtual station and releases the session. The ESP32
returns to passive discovery after the worker releases the radio.

## Errors

Application errors use `{"code":"...","message":"..."}`.
Common codes include `invalid_name`, `invalid_room`, `invalid_profile_message`,
`invalid_profile_color`, `invalid_text`, `invalid_message`, `invalid_bitmap`,
`invalid_bitmap_size`, `unknown_session`, `session_stream_attached`, `broker_busy`,
and `broker_unavailable`. Malformed JSON or wrong JSON types may be rejected by
Axum before the application handler. Physical connection/encoding failures can
also arrive as asynchronous `error` and `delivery_timed_out` events.
