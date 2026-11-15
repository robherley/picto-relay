// Builds the table of glyph bitmaps that both the server and this client draw
// text with, from the DS font that ds.css ships. Run `npm run glyphs` after
// updating ds.css; a test fails while the checked-in table is stale.
//
// The font's outlines are polygons on a ten-unit grid, so asking whether the
// middle of each grid square is inside the outline gives the exact bitmap.

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import opentype from "opentype.js";

const FONT = new URL(
  "../node_modules/@spiritov/ds.css/dist/assets/fonts/nitrods-font.ttf",
  import.meta.url,
);
/** Where the table lives: with the server, which must build without this client. */
const TABLE = fileURLToPath(
  new URL("../../crates/picto-relay-server/src/font.json", import.meta.url),
);

/** Font size at which one grid square of the font is one pixel. */
const EM = 10;
/** The pixels examined around the pen: rows above and below the baseline, and columns. */
const ROWS_ABOVE = 12;
const ROWS_BELOW = 4;
const COLUMNS = 16;

/**
 * Drawn for a character the font lacks: a hollow box the size of a capital
 * letter. (The font's own "not defined" glyph is a solid ten-pixel block.)
 */
const PLACEHOLDER = { advance: 6, x: 0, y: -8, rows: [31, 17, 17, 17, 17, 17, 17, 17, 31] };

/** A glyph's outline as closed polygons, in pixels, y growing downward. */
function contours(glyph) {
  const polygons = [];
  for (const command of glyph.getPath(0, 0, EM).commands) {
    if (command.type === "M") polygons.push([]);
    if (command.type === "M" || command.type === "L") polygons.at(-1).push([command.x, command.y]);
    else if (command.type !== "Z") throw new Error(`Unexpected curve in ${glyph.name}`);
  }
  return polygons;
}

/** Whether a point is inside the outline, by the nonzero winding rule. */
function inside(polygons, px, py) {
  let winding = 0;
  for (const polygon of polygons) {
    for (let i = 0; i < polygon.length; i += 1) {
      const [x1, y1] = polygon[i];
      const [x2, y2] = polygon[(i + 1) % polygon.length];
      const side = (x2 - x1) * (py - y1) - (px - x1) * (y2 - y1);
      if (y1 <= py && y2 > py && side > 0) winding += 1;
      else if (y1 > py && y2 <= py && side < 0) winding -= 1;
    }
  }
  return winding !== 0;
}

/** One glyph as its advance and the bitmap of its inked rows. */
function rasterize(glyph, unitsPerPixel) {
  const polygons = contours(glyph);
  const pixels = [];
  for (let y = -ROWS_ABOVE; y < ROWS_BELOW; y += 1) {
    for (let x = 0; x < COLUMNS; x += 1) {
      if (inside(polygons, x + 0.5, y + 0.5)) pixels.push([x, y]);
    }
  }
  const advance = Math.round(glyph.advanceWidth / unitsPerPixel);
  if (pixels.length === 0) return { advance, x: 0, y: 0, rows: [] };
  const left = Math.min(...pixels.map(([x]) => x));
  const top = Math.min(...pixels.map(([, y]) => y));
  const bottom = Math.max(...pixels.map(([, y]) => y));
  const rows = new Array(bottom - top + 1).fill(0);
  // Bit k of a row is the pixel k columns right of `x`.
  for (const [x, y] of pixels) rows[y - top] |= 1 << (x - left);
  return { advance, x: left, y: top, rows };
}

/** `[advance]` for a blank glyph, else `[advance, x, y, ...rows]`. */
const pack = ({ advance, x, y, rows }) => (rows.length === 0 ? [advance] : [advance, x, y, ...rows]);

/** The table as the exact text of font.json. */
export function buildFontTable() {
  const file = readFileSync(FONT);
  const font = opentype.parse(file.buffer.slice(file.byteOffset, file.byteOffset + file.byteLength));
  const unitsPerPixel = font.unitsPerEm / EM;

  const glyphs = new Map();
  for (let index = 0; index < font.glyphs.length; index += 1) {
    const glyph = font.glyphs.get(index);
    for (const codePoint of glyph.unicodes ?? []) {
      // Control characters have no place in a drawing.
      if (codePoint < 0x20) continue;
      glyphs.set(codePoint, pack(rasterize(glyph, unitsPerPixel)));
    }
  }

  const lines = [...glyphs.entries()]
    .sort(([a], [b]) => a - b)
    .map(([codePoint, glyph]) => `    ${JSON.stringify(String.fromCodePoint(codePoint))}: ${JSON.stringify(glyph)}`);
  return [
    "{",
    `  "placeholder": ${JSON.stringify(pack(PLACEHOLDER))},`,
    '  "glyphs": {',
    lines.join(",\n"),
    "  }",
    "}",
    "",
  ].join("\n");
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  writeFileSync(TABLE, buildFontTable());
  console.log(`wrote ${TABLE}`);
}
