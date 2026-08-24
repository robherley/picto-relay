# Picto Relay JSON API

The versioned HTTP API is the programmatic interface to Picto Relay. The browser UI
served at `/` uses the same endpoints.

The server defaults to `memory` for hardware-free API development. It loops submitted
drawings back as API events but does not simulate Nintendo packets or radio delivery.
On Linux, select `ar9271` to attach the JSON API to the custom-firmware radio worker.
That mode supports one API session because the current firmware represents one physical
virtual station. The session may select any room A through D.

Start the server:

```sh
cargo run -p picto-relay-server
```

Physical Raspberry Pi launch:

```sh
sudo /home/rob/.local/bin/picto-relay-server \
  --backend ar9271 \
  --prepare \
  --interface wlan1 \
  --channel 1 \
  --room A
```

Creating the physical session starts scanning and association in a dedicated radio
thread. Stream its events for `backend_state` progress. `session_ready` is emitted
only after the DS host completes both profile passes and the nickname is live.

The default base URL is `http://localhost:3001/api/v1`.

## Use the browser UI

Open `http://HOST:3001/`. The page receives room rosters and session events over
WebSockets, lets the user join rooms A–D, and packs typed or hand-drawn canvas content
into the bitmap format below. No frontend build step or external assets are required.

The physical outbound encoder preserves one to five native 256x16 PictoChat drawing
bands. A sixteen-slot circular firmware window streams larger messages and retries a
reply when the host acknowledgement marks the virtual slot as failed. Incoming and
outgoing physical messages may use the full native 256x80 surface and are padded to
the API's 256x96 contract.

## Inspect the gateway

```sh
curl http://localhost:3001/api/v1/status
```

Example response:

```json
{
  "version": 1,
  "backend": "memory",
  "bitmap_width": 256,
  "bitmap_height": 96,
  "bitmap_bytes": 3072,
  "bitmap_encoding": "base64_row_major_mono_1bpp_msb_first"
}
```

## List rooms

```sh
curl http://localhost:3001/api/v1/rooms
```

The response always contains rooms `0` through `3`, corresponding to A through D.
`participant_count` is available before joining: the physical backend passively reads
Nintendo room beacons and expires a room after two seconds without an advertisement.
The `participants` roster contains identities currently known to the relay. In memory
mode these are API clients. With the physical backend, the active room contains the host
`c34` slot roster joined to nicknames from host-rebroadcast `e30` profiles. A station
whose profile has not yet been observed uses a stable `DS <mac>` label.

```json
{
  "version": 1,
  "rooms": [
    {"room":0,"participant_count":2,"participants":[{"client_id":4083630471,"name":"Rob"},{"client_id":3209822209,"name":"Relay"}]},
    {"room":1,"participant_count":0,"participants":[]},
    {"room":2,"participant_count":0,"participants":[]},
    {"room":3,"participant_count":0,"participants":[]}
  ]
}
```

## Create a session

```sh
curl -i \
  -H 'content-type: application/json' \
  -d '{"name":"API Relay","room":0}' \
  http://localhost:3001/api/v1/sessions
```

The `201 Created` response contains the `session_id` used by the other endpoints:

```json
{"version":1,"session_id":1,"room":0}
```

`room` is optional for compatibility and uses the server's `--room` value when
omitted. Explicit values must be `0` through `3`.

## Send a drawing

`bitmap` is exactly 3,072 decoded bytes: a 256x96 row-major monochrome bitmap,
eight pixels per byte, most-significant bit first.

```sh
curl -i \
  -H 'content-type: application/json' \
  -d '{"bitmap":"BASE64_BITMAP"}' \
  http://localhost:3001/api/v1/sessions/1/messages
```

The `202 Accepted` response confirms that the message entered the bounded broker
queue, not that a physical DS received it:

```json
{"version":1,"status":"queued"}
```

Delivery is reported later as an event.

## Receive events

The preferred event transport is a WebSocket connection:

```text
ws://localhost:3001/api/v1/sessions/1/events/ws
```

Each server text frame contains one `ServerEnvelope` object. There is no wrapper or
client acknowledgement message:

```json
{"version":1,"type":"backend_state","state":"scanning","detail":"scanning for Room A"}
```

Only one event consumer may own a session queue at a time. While its WebSocket is
attached, the polling endpoint below returns `409 session_stream_attached`. Closing
the socket returns the queue to the session so a reconnect or polling client can
resume without creating a new physical station.

The non-blocking REST endpoint remains available for simple scripts and existing API
clients:

```sh
curl http://localhost:3001/api/v1/sessions/1/events
```

This non-blocking endpoint drains up to 32 queued events. Poll it again when `events`
is empty. Events are returned as versioned JSON envelopes:

```json
{
  "version": 1,
  "events": [
    {"version":1,"type":"session_ready","client_id":1},
    {"version":1,"type":"participant_joined","room":0,"participant":{"client_id":1,"name":"API Relay"}},
    {"version":1,"type":"participants","room":0,"participants":[{"client_id":1,"name":"API Relay"}]},
    {"version":1,"type":"message","station_id":1,"name":"API Relay","message_id":0,"bitmap":"..."},
    {"version":1,"type":"delivered","message_id":0}
  ]
}
```

Possible event types are `session_ready`, `participants`, `participant_joined`,
`participant_left`, `message`, `delivered`, `delivery_timed_out`, `backend_state`,
and `error`. Lifecycle events carry the affected `room` and complete `participant`
identity. Roster snapshots remain authoritative; lifecycle events describe the delta
that led to a new snapshot. Learning a better nickname for an existing station does
not emit a false leave and rejoin because membership is compared by `client_id`.

With the physical backend,
`delivered` means the DS host first echoed the submitted drawing's matching `d0a`
announcement and then rebroadcast a complete drawing from the virtual station. This
two-part correlation prevents a locally observed transmission from being mistaken
for DS acceptance. If that proof is not observed before the deadline, the broker emits
`delivery_timed_out`.

## Stream room rosters

Connect to:

```text
ws://localhost:3001/api/v1/rooms/ws
```

The server immediately sends the same complete `RoomsResponse` returned by
`GET /api/v1/rooms`, then sends a replacement snapshot whenever any room roster
changes. This stream is independent of session ownership, so any number of browser or
API observers may subscribe. The REST room endpoint remains available for one-shot
inspection.

## Delete a session

```sh
curl -i -X DELETE http://localhost:3001/api/v1/sessions/1
```

The server returns `204 No Content` and disconnects that virtual station.

## Errors

API-generated errors have a stable JSON shape:

```json
{
  "version": 1,
  "code": "unknown_session",
  "message": "session 42 does not exist"
}
```

Current codes include `invalid_name`, `invalid_bitmap`, `invalid_bitmap_size`,
`invalid_room`, `unknown_session`, `session_stream_attached`, `broker_busy`, and
`broker_unavailable`.
