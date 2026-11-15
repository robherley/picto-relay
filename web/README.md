# Picto Relay web client

The browser UI for [picto-relay-server](../crates/picto-relay-server/README.md),
made to look and work like PictoChat on a Nintendo DS: a room list, a message
log on the top screen, and a drawing area with the DS keyboard on the bottom.

It is a React app built with Vite. The DS look comes from
[ds.css](https://github.com/spiritov/ds.css), and so does the font: `npm run
glyphs` turns it into the table of glyph bitmaps
([`font.json`](../crates/picto-relay-server/src/font.json)) that both this
client and the server draw text with. The REST client is generated from
[`docs/openapi.json`](../docs/openapi.json) with
[@hey-api/openapi-ts](https://heyapi.dev) on every build, so it cannot drift
from the server; it is not checked in.

## Build

```sh
just web-build      # writes web/dist
```

The Rust server embeds `web/dist` when it compiles, so `just server` and
`just server-memory` run this first. Nothing else connects the two builds.

## Develop

```sh
just server-memory  # terminal 1: the API, without hardware
just web-dev        # terminal 2: http://localhost:5173 with hot reload
```

The dev server proxies `/api` (including WebSockets) to `127.0.0.1:3001`; set
`PICTO_RELAY_URL` to point it at a server somewhere else.

```sh
npm --prefix web test   # generate the client, type-check, run unit tests
```

After changing an API type in the server, run `just openapi` to rewrite
`docs/openapi.json`; the next build picks it up.

## Layout

| Path | What it holds |
| --- | --- |
| `src/api/` | `client.ts` re-exports the generated client and opens the two WebSocket streams |
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
  them. The size slider stops at what the window has room for, and at its top
  position the console keeps filling the window as the window changes. The
  choices are remembered.
