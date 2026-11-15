import table from "../../../crates/picto-relay-server/src/font.json";
import type { Point } from "./drawing";
import type { Glyph } from "./typing";

// Typed text is drawn from a table of the DS font's bitmaps, the same table
// the server renders API text with, so both produce the same pixels. The
// table is generated from the font ds.css ships: see scripts/font-table.mjs.

/** `[advance]` for a blank glyph, else `[advance, x, y, ...rows]`. */
type Packed = number[];

const TABLE = table as { placeholder: Packed; glyphs: Record<string, Packed> };

/** Bit `k` of a row is the pixel `k` columns right of the glyph's `x`. */
function unpack([advance = 0, left = 0, top = 0, ...rows]: Packed): Glyph {
  const dots: Point[] = [];
  rows.forEach((row, index) => {
    for (let column = 0; row >> column !== 0; column += 1) {
      if ((row >> column) & 1) dots.push({ x: left + column, y: top + index });
    }
  });
  return { advance, dots };
}

/** Drawn for a character the font lacks. */
const PLACEHOLDER = unpack(TABLE.placeholder);
const cache = new Map<string, Glyph>();

/** The bitmap of one character, or the placeholder if the font lacks it. */
export function glyphOf(character: string): Glyph {
  let glyph = cache.get(character);
  if (!glyph) {
    const packed = Object.hasOwn(TABLE.glyphs, character) ? TABLE.glyphs[character] : undefined;
    glyph = packed ? unpack(packed) : PLACEHOLDER;
    cache.set(character, glyph);
  }
  return glyph;
}
