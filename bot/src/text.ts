import table from "../../crates/picto-relay-server/src/font.json" with { type: "json" };

import { Canvas, DRAW_LEFT, DRAW_RIGHT, NATIVE_HEIGHT, TAG_HEIGHT, TAG_RIGHT } from "./canvas.ts";
import type { Ink } from "./canvas.ts";

// Text is drawn from the table of DS font bitmaps the server and the web
// client share, laid out the way the server lays out API text (`text.rs`):
// one line of text per ruled line, the first starting beside the name tag,
// words wrapping whole. Where the server rejects text that needs a sixth
// line, this carries on into another drawing.

/** Height of one line of text: the spacing of the ruled lines. */
export const LINE_HEIGHT = 16;
/** Lines of text a drawing holds. */
export const LINES = NATIVE_HEIGHT / LINE_HEIGHT;
/** Row of the text baseline within its line. */
export const BASELINE = 13;
/** Gap kept between text and the edges of the box and name tag. */
const TEXT_INSET = 4;
/** First column text may not reach. */
const TEXT_RIGHT = DRAW_RIGHT - TEXT_INSET;

/** `[advance]` for a blank glyph, else `[advance, x, y, ...rows]`. */
type Packed = number[];

const TABLE = table as { placeholder: Packed; glyphs: Record<string, Packed> };

/** The glyph of one character, or the placeholder box if the font lacks it. */
function glyph(character: string): Packed {
  return Object.hasOwn(TABLE.glyphs, character) ? TABLE.glyphs[character]! : TABLE.placeholder;
}

/** Whether the DS font can draw this character. */
export function inFont(character: string): boolean {
  return Object.hasOwn(TABLE.glyphs, character);
}

/** Columns the pen moves across this text. */
export function measure(text: string): number {
  let width = 0;
  for (const character of text) width += glyph(character)[0]!;
  return width;
}

/**
 * Draws one line of text with its baseline on row `baseline`, the pen
 * starting at column `x`. Returns the column the pen ends on.
 */
export function stamp(canvas: Canvas, text: string, x: number, baseline: number, ink: Ink): number {
  for (const character of text) {
    // Bit `k` of a row is the pixel `k` columns right of the glyph's `left`.
    const [advance = 0, left = 0, top = 0, ...rows] = glyph(character);
    rows.forEach((row, index) => {
      for (let column = 0; row >> column !== 0; column += 1) {
        if ((row >> column) & 1) canvas.set(x + left + column, baseline + top + index, ink);
      }
    });
    x += advance;
  }
  return x;
}

/** Leftmost pen position on a line; the first line starts beside the name tag. */
function lineStart(line: number): number {
  return (line * LINE_HEIGHT < TAG_HEIGHT ? TAG_RIGHT : DRAW_LEFT) + TEXT_INSET;
}

/** Width of most Latin letters, digits and punctuation in the DS font. */
const LETTER_WIDTH = 6;
/** Letters that fit on the first line of a message, beside the name tag. */
export const FIRST_LINE_LETTERS = Math.floor((TEXT_RIGHT - lineStart(0)) / LETTER_WIDTH);
/** Letters that fit on each line after the first. */
export const LINE_LETTERS = Math.floor((TEXT_RIGHT - lineStart(1)) / LETTER_WIDTH);
/** Letters one message holds when every line is full. Wrapping at words wastes a few. */
export const MESSAGE_LETTERS = FIRST_LINE_LETTERS + (LINES - 1) * LINE_LETTERS;

/**
 * Types text as the keyboard would, onto as many drawings as it needs.
 * Returns none for text that is only whitespace.
 */
export function type(text: string, ink: Ink): Canvas[] {
  const characters = Array.from(text.trim().replace(/\r\n?/g, "\n"));
  if (characters.length === 0) return [];

  const pages = [new Canvas()];
  let line = 0;
  let x = lineStart(0);
  const breakLine = () => {
    line += 1;
    if (line === LINES) {
      pages.push(new Canvas());
      line = 0;
    }
    x = lineStart(line);
  };

  const space = glyph(" ")[0]!;
  let gap = 0;
  let index = 0;
  while (index < characters.length) {
    const character = characters[index]!;
    if (character === "\n") {
      breakLine();
      gap = 0;
      index += 1;
      continue;
    }
    if (/\s/u.test(character)) {
      gap += space;
      index += 1;
      continue;
    }
    let end = index;
    while (end < characters.length && !/\s/u.test(characters[end]!)) end += 1;
    const word = characters.slice(index, end);
    // Keep the word whole on the next line if it does not fit on this one.
    // The space before it goes with the line break.
    if (x !== lineStart(line) && x + gap + measure(word.join("")) > TEXT_RIGHT) breakLine();
    else x += gap;
    gap = 0;
    for (const letter of word) {
      // A word longer than a whole line is split where it runs out of room.
      if (x + glyph(letter)[0]! > TEXT_RIGHT) breakLine();
      x = stamp(pages.at(-1)!, letter, x, line * LINE_HEIGHT + BASELINE, ink);
    }
    index = end;
  }
  // Blank lines can fill a whole drawing, and a DS cannot receive a blank one.
  return pages.filter((page) => !page.blank);
}
