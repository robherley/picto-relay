import { describe, expect, it } from "vitest";
import { BLACK, RAINBOW_INKS, WIDTH } from "./constants";
import {
  blank, drawLine, fillRect, fromBase64, isBlank, isDrawable, pack, toBase64, unpack, usedHeight,
} from "./drawing";

const at = (x: number, y: number) => y * WIDTH + x;

describe("packing", () => {
  it("puts the left pixel in the low nibble", () => {
    const bytes = new Uint8Array(12288);
    bytes[0] = 0xf3;
    expect(Array.from(unpack(bytes).slice(0, 4))).toEqual([3, 15, 0, 0]);
  });

  it("rejects other lengths", () => {
    expect(() => unpack(new Uint8Array(3072))).toThrow(/length/);
  });

  it("round-trips through pack and base64", () => {
    const pixels = blank();
    drawLine(pixels, { x: 30, y: 30 }, { x: 230, y: 60 }, 2, "rainbow", { step: 0 });
    expect(unpack(fromBase64(toBase64(pack(pixels))))).toEqual(pixels);
  });
});

describe("drawing", () => {
  it("clips strokes to the box and the name-tag notch", () => {
    const pixels = blank();
    const rainbow = { step: 0 };
    drawLine(pixels, { x: 0, y: 8 }, { x: 255, y: 8 }, 1, BLACK, rainbow);
    drawLine(pixels, { x: 0, y: 30 }, { x: 255, y: 30 }, 1, BLACK, rainbow);
    drawLine(pixels, { x: 100, y: 0 }, { x: 100, y: 95 }, 1, BLACK, rainbow);
    expect(pixels[at(79, 8)]).toBe(0);
    expect(pixels[at(80, 8)]).toBe(1);
    expect(pixels[at(23, 30)]).toBe(0);
    expect(pixels[at(24, 30)]).toBe(1);
    expect(pixels[at(251, 30)]).toBe(1);
    expect(pixels[at(252, 30)]).toBe(0);
    expect(pixels[at(100, 79)]).toBe(1);
    expect(pixels[at(100, 80)]).toBe(0);
    pixels.forEach((index, i) => {
      if (index !== 0) expect(isDrawable(i % WIDTH, Math.floor(i / WIDTH))).toBe(true);
    });
  });

  it("cycles the rainbow pen through every shade and nothing else", () => {
    const pixels = blank();
    drawLine(pixels, { x: 30, y: 30 }, { x: 230, y: 30 }, 1, "rainbow", { step: 0 });
    expect(new Set(pixels)).toEqual(new Set([0, ...RAINBOW_INKS]));
  });

  it("erases with paper", () => {
    const pixels = blank();
    drawLine(pixels, { x: 100, y: 30 }, { x: 120, y: 30 }, 2, BLACK, { step: 0 });
    fillRect(pixels, 90, 20, 40, 20, 0);
    expect(isBlank(pixels)).toBe(true);
  });

  it("measures a message in whole bands", () => {
    const pixels = blank();
    expect(usedHeight(pixels)).toBe(16);
    pixels[at(100, 15)] = 1;
    expect(usedHeight(pixels)).toBe(16);
    pixels[at(100, 16)] = 1;
    expect(usedHeight(pixels)).toBe(32);
    pixels[at(100, 79)] = 1;
    expect(usedHeight(pixels)).toBe(80);
  });
});
