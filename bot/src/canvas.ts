// The PictoChat drawing surface, in DS pixels. These numbers mirror the
// server's `protocol.rs`; `GET /api/v1/status` reports the same ones.

/** Width of the drawing surface the API exchanges. */
export const WIDTH = 256;
/** Height of the API surface. Rows below `NATIVE_HEIGHT` are padding. */
export const HEIGHT = 96;
/** Height of the largest drawing a DS can show. */
export const NATIVE_HEIGHT = 80;
/** Leftmost drawable column below the name tag. */
export const DRAW_LEFT = 24;
/** First column right of the drawable area. */
export const DRAW_RIGHT = 252;
/** Leftmost drawable column beside the name tag. */
export const TAG_RIGHT = 80;
/** Rows the name tag covers at the top-left of a message. */
export const TAG_HEIGHT = 16;
/** Width of a message's drawable box. */
export const BOX_WIDTH = DRAW_RIGHT - DRAW_LEFT;

/** Palette index of blank paper. */
export const PAPER = 0;
/** Palette index of the black pen. */
export const BLACK = 1;
/** Highest palette index. */
export const INK_MAX = 15;
/** The rainbow pen's shades, in order. */
export const RAINBOW_INKS = [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
/** Columns a rainbow stroke travels before changing shade. */
export const RAINBOW_STEP = 6;

/** The pen colors by palette index, named as a DS shows them. */
export const INK_NAMES = [
  "paper",
  "black",
  "gray",
  "magenta",
  "pink",
  "red",
  "red orange",
  "orange",
  "yellow",
  "lime",
  "green",
  "teal",
  "cyan",
  "light blue",
  "blue",
  "dark blue",
] as const;

/** Screen colors of the sixteen palette indices, as the web client shows them. */
export const INK_RGB: readonly (readonly [number, number, number])[] = [
  [255, 255, 255],
  [0, 0, 0],
  [216, 224, 248],
  [232, 80, 248],
  [248, 48, 176],
  [248, 24, 0],
  [248, 72, 0],
  [248, 152, 0],
  [232, 216, 0],
  [160, 232, 0],
  [104, 224, 0],
  [72, 240, 232],
  [0, 208, 248],
  [0, 120, 248],
  [0, 64, 248],
  [0, 48, 224],
];

/** A palette index, or the pen that changes shade as it moves right. */
export type Ink = number | "rainbow";

/** The palette index an ink leaves in `column`. */
export function inkAt(ink: Ink, column: number): number {
  if (ink !== "rainbow") return ink;
  return RAINBOW_INKS[Math.floor(column / RAINBOW_STEP) % RAINBOW_INKS.length]!;
}

/** Whether a DS shows this pixel: inside the box and clear of the name tag. */
export function drawable(x: number, y: number): boolean {
  const left = y < TAG_HEIGHT ? TAG_RIGHT : DRAW_LEFT;
  return y >= 0 && y < NATIVE_HEIGHT && x >= left && x < DRAW_RIGHT;
}

/** One drawing: a palette index for every pixel of the API surface. */
export class Canvas {
  readonly pixels = new Uint8Array(WIDTH * HEIGHT);

  /** Reads a `palette_bitmap`: two pixels a byte, the left one in the low nibble. */
  static unpack(bitmap: Uint8Array): Canvas {
    if (bitmap.length !== (WIDTH * HEIGHT) / 2) {
      throw new Error(`A palette bitmap is ${(WIDTH * HEIGHT) / 2} bytes, not ${bitmap.length}`);
    }
    const canvas = new Canvas();
    bitmap.forEach((byte, index) => {
      canvas.pixels[index * 2] = byte & 0x0f;
      canvas.pixels[index * 2 + 1] = byte >> 4;
    });
    return canvas;
  }

  /** The drawing as a `palette_bitmap`. */
  pack(): Uint8Array {
    const bitmap = new Uint8Array(this.pixels.length / 2);
    for (let index = 0; index < bitmap.length; index += 1) {
      bitmap[index] = this.pixels[index * 2]! | (this.pixels[index * 2 + 1]! << 4);
    }
    return bitmap;
  }

  get(x: number, y: number): number {
    return this.pixels[y * WIDTH + x] ?? PAPER;
  }

  /** Inks one pixel. Pixels a DS would not show are left alone. */
  set(x: number, y: number, ink: Ink): void {
    if (drawable(x, y)) this.pixels[y * WIDTH + x] = inkAt(ink, x);
  }

  /** Whether nothing is drawn. A DS cannot receive a blank drawing. */
  get blank(): boolean {
    return this.pixels.every((index) => index === PAPER);
  }
}
