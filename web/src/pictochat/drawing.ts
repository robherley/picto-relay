import {
  BAND_HEIGHT, DRAW_LEFT, DRAW_RIGHT, HEIGHT, NATIVE_HEIGHT, PAPER, RAINBOW_INKS,
  RAINBOW_STEP, TAG_HEIGHT, TAG_RIGHT, WIDTH,
} from "./constants";

/** One palette index per pixel of the `WIDTH` x `HEIGHT` surface, row-major. */
export type Pixels = Uint8Array;

/** A fixed palette index, or the pen that cycles through the rainbow shades. */
export type Ink = number | "rainbow";

export interface Point {
  x: number;
  y: number;
}

/** Bits per pixel of the packed palette encoding the API uses. */
const PACKED_BITS = 4;
const PACKED_MASK = (1 << PACKED_BITS) - 1;
const PACKED_BYTES = (WIDTH * HEIGHT) / 2;

export function blank(): Pixels {
  return new Uint8Array(WIDTH * HEIGHT);
}

/** Whether a DS shows this pixel: inside the box and clear of the name tag. */
export function isDrawable(x: number, y: number): boolean {
  if (y < 0 || y >= NATIVE_HEIGHT || x >= DRAW_RIGHT) return false;
  return x >= (y < TAG_HEIGHT ? TAG_RIGHT : DRAW_LEFT);
}

/** The rainbow shade for the `step`th pixel of travel. */
export function rainbowShade(step: number): number {
  return RAINBOW_INKS[Math.floor(step / RAINBOW_STEP) % RAINBOW_INKS.length]!;
}

/** Running state of the rainbow pen, so its shade carries across strokes. */
export interface RainbowState {
  step: number;
}

/**
 * Draws a line of square dabs `size` pixels wide from `from` to `to`.
 * Pixels outside the drawable area are left alone.
 */
export function drawLine(
  pixels: Pixels, from: Point, to: Point, size: number, ink: Ink, rainbow: RainbowState,
): void {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const steps = Math.max(1, Math.ceil(Math.hypot(dx, dy)));
  const offset = Math.floor((size - 1) / 2);
  for (let step = 0; step <= steps; step += 1) {
    const cx = Math.round(from.x + (dx * step) / steps) - offset;
    const cy = Math.round(from.y + (dy * step) / steps) - offset;
    const index = ink === "rainbow" ? rainbowShade(rainbow.step++) : ink;
    fillRect(pixels, cx, cy, size, size, index);
  }
}

/** Sets a rectangle to one palette index, clipped to the drawable area. */
export function fillRect(
  pixels: Pixels, left: number, top: number, width: number, height: number, index: number,
): void {
  for (let y = top; y < top + height; y += 1) {
    for (let x = left; x < left + width; x += 1) {
      if (isDrawable(x, y)) pixels[y * WIDTH + x] = index;
    }
  }
}

export function isBlank(pixels: Pixels): boolean {
  return pixels.every((index) => index === PAPER);
}

/** Rows a message occupies on a DS: whole bands down to its lowest ink. */
export function usedHeight(pixels: Pixels): number {
  let last = pixels.length - 1;
  while (last >= 0 && pixels[last] === PAPER) last -= 1;
  const lowestRow = Math.max(0, Math.floor(last / WIDTH));
  const bands = Math.floor(lowestRow / BAND_HEIGHT) + 1;
  return Math.min(bands * BAND_HEIGHT, NATIVE_HEIGHT);
}

/** Packs pixels four bits each, the left pixel in the low nibble. */
export function pack(pixels: Pixels): Uint8Array {
  const bytes = new Uint8Array(PACKED_BYTES);
  for (let i = 0; i < pixels.length; i += 1) {
    bytes[i >> 1]! |= (pixels[i]! & PACKED_MASK) << ((i & 1) * PACKED_BITS);
  }
  return bytes;
}

/** Unpacks a palette bitmap as events carry it: the inverse of `pack`. */
export function unpack(bytes: Uint8Array): Pixels {
  if (bytes.length !== PACKED_BYTES) throw new Error(`Unexpected bitmap length ${bytes.length}`);
  const pixels = blank();
  for (let i = 0; i < pixels.length; i += 1) {
    pixels[i] = (bytes[i >> 1]! >> ((i & 1) * PACKED_BITS)) & PACKED_MASK;
  }
  return pixels;
}

export function toBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

export function fromBase64(text: string): Uint8Array {
  return Uint8Array.from(atob(text), (character) => character.charCodeAt(0));
}
