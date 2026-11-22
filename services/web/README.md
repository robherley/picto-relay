# Picto Relay web client

The browser UI for [picto-relay-server](../../crates/picto-relay-server/README.md),
made to look and work like PictoChat on a Nintendo DS: a room list, a message
log on the top screen, and a drawing area with the DS keyboard on the bottom.

It is a React app built with Vite. The DS look comes from
[ds.css](https://github.com/spiritov/ds.css), and so does the font: `npm run
glyphs` turns it into the table of glyph bitmaps
([`font.json`](../../crates/picto-relay-server/src/font.json)) that both this
client and the server draw text with. The REST client is generated from
[`docs/openapi.json`](../../docs/openapi.json) with
[@hey-api/openapi-ts](https://heyapi.dev) on every build, so it cannot drift
from the server; it is not checked in.

## Build

```sh
just web-build      # writes services/web/dist
```

The Rust server embeds `services/web/dist` when it compiles, so `just server` and
`just server-memory` run this first. Nothing else connects the two builds.

## Develop

```sh
just server-memory  # terminal 1: the API, without hardware
just web-dev        # terminal 2: http://localhost:5173 with hot reload
```

The dev server proxies `/api` (including WebSockets) to `127.0.0.1:3001`; set
`PICTO_RELAY_URL` to point it at a server somewhere else.

```sh
npm --prefix services/web test   # generate the client, type-check, run unit tests
```

After changing an API type in the server, run `just openapi` to rewrite
`docs/openapi.json`; the next build picks it up.

## The demo page

Built with `--mode serial`, the same client needs no server. The page opens
the ESP32's serial port itself, with
[Web Serial](https://developer.mozilla.org/docs/Web/API/Web_Serial_API), and
does what the server's worker does: attaches to the firmware, scans for rooms,
joins, keeps the session alive and uploads drawings. Web Serial is in Chrome
and Edge on a computer; other browsers are told so.

```sh
just demo           # http://localhost:5173 with hot reload
just demo-build     # static files in services/web/dist-demo, to host anywhere over HTTPS
```

Only one program can use the serial port, so stop the server first.

- **The protocol is the server's.** What goes in each command and what each
  packet means comes from the Rust crates the server uses, compiled to
  WebAssembly ([picto-relay-wasm](../../crates/picto-relay-wasm/README.md)).
  [`src/serial/driver.ts`](src/serial/driver.ts) adds the timing, and is tested
  against a stand-in for the firmware that speaks the real framing.
- **A worker owns the port.** The firmware gives up on a host it has not heard
  from for five seconds, and browsers slow a hidden tab's timers.
- **It flashes too.** The Flash button writes the firmware with Espressif's
  [esptool-js](https://github.com/espressif/esptool-js). The images are copied
  from the last firmware build (`just firmware-build`) into `public/firmware/`
  when the page is built; without a build, the button is not offered.
- **It reconnects.** A port allowed once is opened again on the next visit,
  and when the ESP32 is plugged back in.

Which of the two a build is for is decided in
[`src/api/transport.ts`](src/api/transport.ts), and is the only difference.

## Layout

| Path | What it holds |
| --- | --- |
| `src/api/` | `transport.ts` is how the client reaches rooms: `relay.ts` through the relay server (`client.ts` re-exports its generated client and opens its two WebSocket streams), `serial.ts` through the ESP32 directly |
| `src/serial/` | The demo page's driver for the ESP32, the worker that runs it, the WebAssembly wrapper and the flasher |
| `src/pictochat/` | The drawing surface, typing, keyboard pages and glyphs: no React, unit tested |
| `scripts/` | `font-table.mjs`, which builds the glyph table from the DS font |
| `src/hooks/` | Rooms, session events, the saved profile, and the display settings |
| `src/components/` | The two screens of the lobby and of a chat |
| `src/styles.css` | Layout at twice DS size, on top of ds.css |

## How it behaves

- The drawing is 256 by 80 DS pixels with a notch for the name tag, exactly
  what a DS shows. Keys type onto the drawing in the DS font; Backspace removes
  the last character. A key can also be dragged off the keyboard and dropped
  anywhere on the drawing. Enter starts a new line and Ctrl+Enter sends.
- The color chip under the eraser opens the pen colors a DS can show: black,
  gray, thirteen shades, and the rainbow pen. It colors typed text as well as
  strokes. Clicking the pen while it is selected switches between the chosen
  color and rainbow.
- A message you send appears in the log at once, faded until the room confirms
  it, the way a DS shows yours without waiting.
- Your profile color themes the interface, as the favorite color does on a DS.
- Pressing your own name tag, or a name in the list beside the close button,
  shows that person's profile message in place of the drawing, with the rest
  of the screen grayed out until it is closed.
- The button in the top-right corner of the page opens the display settings,
  which set how the two screens are shown: one above the
  other, side by side, or whichever fits; how large they are; and the gap between
  them. The sizes offered stop at what the window has room for; "Fit" takes
  the largest sharp size that fits, and keeps doing so as the window changes.
  The choices are remembered.
