import {
  BAND_HEIGHT, DRAW_LEFT, DRAW_RIGHT, NATIVE_HEIGHT, PAPER, TAG_HEIGHT, TAG_RIGHT, WIDTH,
} from "./constants";
import { fillRect, type Ink, isDrawable, type Pixels, type Point, rainbowShade } from "./drawing";

/** A character of the DS font as pixels relative to the pen and the baseline. */
export interface Glyph {
  /** Columns the pen moves right after this character. */
  advance: number;
  /** Inked pixels; `y` is negative above the baseline. */
  dots: readonly Point[];
}

export type GlyphSource = (character: string) => Glyph;

/**
 * Draws one character with its pen position at `x` and its baseline on row
 * `baseline`. Pixels outside the drawable area are left alone. Rainbow ink
 * changes shade by column, so a line of text runs through the colors.
 */
export function stampGlyph(
  pixels: Pixels, glyph: Glyph, x: number, baseline: number, ink: Ink,
): void {
  for (const dot of glyph.dots) {
    const column = x + dot.x;
    const row = baseline + dot.y;
    if (!isDrawable(column, row)) continue;
    pixels[row * WIDTH + column] = ink === "rainbow" ? rainbowShade(column) : ink;
  }
}

/** Rows from the middle of a capital letter down to its baseline. */
export const GLYPH_HALF_HEIGHT = 4;

/** Typed text sits on the ruled lines, one line per band. */
const LINE_HEIGHT = BAND_HEIGHT;
const LINES = NATIVE_HEIGHT / LINE_HEIGHT;
/** Row of the text baseline within its line. */
const BASELINE = 13;
/** Gap kept between text and the edges of the box and name tag. */
const TEXT_INSET = 4;
const TEXT_RIGHT = DRAW_RIGHT - TEXT_INSET;

/** Leftmost pen position on a line; the first line starts beside the name tag. */
function lineStart(line: number): number {
  return (line * LINE_HEIGHT < TAG_HEIGHT ? TAG_RIGHT : DRAW_LEFT) + TEXT_INSET;
}

type Entry =
  | { kind: "character"; character: string; x: number; line: number; advance: number }
  /** A line break; `wrapped` when the text ran out of room rather than Enter. */
  | { kind: "break"; x: number; line: number; wrapped: boolean };

/**
 * Types characters onto a drawing the way the DS keyboard does: each lands at
 * a pen position that advances along ruled lines, and Backspace removes the
 * most recent one, erasing its cell.
 */
export class Typewriter {
  private entries: Entry[] = [];
  private x = lineStart(0);
  private line = 0;

  constructor(private readonly glyphOf: GlyphSource) {}

  /** Stamps a character at the pen. False when the message is full. */
  type(pixels: Pixels, character: string, ink: Ink): boolean {
    const glyph = this.glyphOf(character);
    if (this.x + glyph.advance > TEXT_RIGHT && !this.breakLine(true)) return false;
    stampGlyph(pixels, glyph, this.x, this.line * LINE_HEIGHT + BASELINE, ink);
    this.entries.push({
      kind: "character", character, x: this.x, line: this.line, advance: glyph.advance,
    });
    this.x += glyph.advance;
    return true;
  }

  /** Moves the pen to the next line. False on the last line. */
  newline(): boolean {
    return this.breakLine(false);
  }

  /** Removes the most recent character or line break. False when none is left. */
  backspace(pixels: Pixels): boolean {
    const entry = this.entries.pop();
    if (!entry) return false;
    this.x = entry.x;
    this.line = entry.line;
    if (entry.kind === "character") {
      fillRect(pixels, entry.x, entry.line * LINE_HEIGHT, entry.advance, LINE_HEIGHT, PAPER);
      // A character that wrapped takes its automatic line break with it.
      const previous = this.entries.at(-1);
      if (previous?.kind === "break" && previous.wrapped) this.backspace(pixels);
    }
    return true;
  }

  /** The most recently typed character, if the pen has not moved since. */
  lastCharacter(): string | undefined {
    const entry = this.entries.at(-1);
    return entry?.kind === "character" ? entry.character : undefined;
  }

  /** Forgets what was typed and returns the pen to the start. */
  reset(): void {
    this.entries = [];
    this.x = lineStart(0);
    this.line = 0;
  }

  private breakLine(wrapped: boolean): boolean {
    if (this.line + 1 >= LINES) return false;
    this.entries.push({ kind: "break", x: this.x, line: this.line, wrapped });
    this.line += 1;
    this.x = lineStart(this.line);
    return true;
  }
}
