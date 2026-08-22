# Picto Relay JSON API

The versioned HTTP API is the primary headless interface to Picto Relay. The bundled
terminal REPL uses this API and serves as the reference interactive client.

The server defaults to `scripted` for hardware-free development. On Linux, select
`ar9271` to attach the JSON API to the custom-firmware radio worker. That mode supports
one API session because the current firmware represents one physical virtual station.

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
thread. Poll its events for `backend_state` progress. `session_ready` is emitted only
after the DS host completes both profile passes and the nickname is live.

The default base URL is `http://localhost:3001/api/v1`.

## Use the terminal REPL

With the server running, start the bundled terminal client:

```sh
cargo run -p picto-relay-repl -- --name Terminal
```

Plain text is rendered into the physical backend's native 256x80 drawing and submitted
through this API. A real-DS calibration reserves x=0..87 in the first rows for the
sender nametag; lower rows use x=28..243. Text up to 85 characters uses a large
five-line pixel font, while longer text uses as many as ten compact lines (up to 340
characters), without vertical resampling. Incoming drawings are polled automatically
and displayed as ASCII.
The command set is:

```text
TEXT             render and send typed text
/dot [x y]       send a 3x3 dot (default: center)
/file PATH       send a raw 3072-byte packed bitmap
/poll            receive queued events now
/status          show gateway/backend status
/help            show help
/quit            delete the session and exit
```

Use `--url http://HOST:3001/api/v1` to connect to a relay on another machine and
`--poll-ms N` to change the receive interval (minimum 100 ms).

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
  "backend": "scripted",
  "bitmap_width": 256,
  "bitmap_height": 96,
  "bitmap_bytes": 3072,
  "bitmap_encoding": "base64_row_major_mono_1bpp_msb_first"
}
```

## Create a session

```sh
curl -i \
  -H 'content-type: application/json' \
  -d '{"name":"API Relay"}' \
  http://localhost:3001/api/v1/sessions
```

The `201 Created` response contains the `session_id` used by the other endpoints:

```json
{"version":1,"session_id":1}
```

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
    {"version":1,"type":"participants","participants":[{"client_id":1,"name":"API Relay"}]},
    {"version":1,"type":"message","station_id":1,"name":"API Relay","message_id":0,"bitmap":"..."},
    {"version":1,"type":"delivered","message_id":0}
  ]
}
```

Possible event types are `session_ready`, `participants`, `message`, `delivered`,
`delivery_timed_out`, `backend_state`, and `error`. With the physical backend,
`delivered` means the DS host first echoed the submitted drawing's matching `d0a`
announcement and then rebroadcast a complete drawing from the virtual station. This
two-part correlation prevents a locally observed transmission from being mistaken
for DS acceptance. If that proof is not observed before the deadline, the broker emits
`delivery_timed_out`.

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
`unknown_session`, `broker_busy`, and `broker_unavailable`.
