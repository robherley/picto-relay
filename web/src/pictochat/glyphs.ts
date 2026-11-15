import type { Point } from "./drawing";
import type { Glyph } from "./typing";

// Typed text uses the same pixel font as the interface (ds.css's recreation of
// the DS system font). Its outlines sit on a ten-unit grid, so drawing a
// character much larger than life and sampling the middle of each grid square
// recovers the exact bitmap, however the browser antialiases the edges.

const FONT_FAMILY = "ds-system";
/** Font size at which one grid square of the font is one pixel. */
const EM = 10;
/** Canvas pixels drawn per font pixel while sampling. */
const SAMPLE_SCALE = 8;
/** Font pixels sampled above and below the baseline, and across. */
const ASCENT = 12;
const DESCENT = 4;
const CELL_WIDTH = 24;
/** Room left of the pen for characters that overhang it. */
const LEFT_MARGIN = 2;
const OPAQUE = 128;

const cache = new Map<string, Glyph>();
let context: CanvasRenderingContext2D | null = null;

/** Resolves once the pixel font can be drawn to a canvas. */
export async function fontReady(): Promise<void> {
  await document.fonts.load(`${EM}px ${FONT_FAMILY}`);
}

/** The bitmap of one character. Call after `fontReady`. */
export function glyphOf(character: string): Glyph {
  let glyph = cache.get(character);
  if (!glyph) {
    glyph = rasterize(character);
    cache.set(character, glyph);
  }
  return glyph;
}

function rasterize(character: string): Glyph {
  const width = CELL_WIDTH * SAMPLE_SCALE;
  const height = (ASCENT + DESCENT) * SAMPLE_SCALE;
  if (!context) {
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) throw new Error("This browser cannot draw to a canvas");
    context.font = `${EM * SAMPLE_SCALE}px ${FONT_FAMILY}`;
    context.textBaseline = "alphabetic";
  }
  context.clearRect(0, 0, width, height);
  context.fillText(character, LEFT_MARGIN * SAMPLE_SCALE, ASCENT * SAMPLE_SCALE);
  const alpha = context.getImageData(0, 0, width, height).data;
  const middle = SAMPLE_SCALE / 2;
  const dots: Point[] = [];
  for (let y = 0; y < ASCENT + DESCENT; y += 1) {
    for (let x = 0; x < CELL_WIDTH; x += 1) {
      const sample = (y * SAMPLE_SCALE + middle) * width + x * SAMPLE_SCALE + middle;
      if (alpha[sample * 4 + 3]! >= OPAQUE) dots.push({ x: x - LEFT_MARGIN, y: y - ASCENT });
    }
  }
  const advance = Math.round(context.measureText(character).width / SAMPLE_SCALE);
  return { advance, dots };
}
