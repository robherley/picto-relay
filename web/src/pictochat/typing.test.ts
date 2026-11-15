import { describe, expect, it } from "vitest";
import { BLACK, RAINBOW_INKS, WIDTH } from "./constants";
import { blank, isBlank } from "./drawing";
import { type Glyph, stampGlyph, Typewriter } from "./typing";

/** A font of solid 5x7 blocks, six columns apart. */
const block: Glyph = {
  advance: 6,
  dots: Array.from({ length: 35 }, (_, i) => ({ x: i % 5, y: -Math.floor(i / 5) })),
};
const typewriter = () => new Typewriter(() => block);

/** The leftmost inked column of a row. */
function firstInk(pixels: Uint8Array, y: number): number {
  return pixels.subarray(y * WIDTH, (y + 1) * WIDTH).findIndex((index) => index !== 0);
}

describe("stampGlyph", () => {
  it("draws a character anywhere, clipped to the drawable area", () => {
    const pixels = blank();
    stampGlyph(pixels, block, 100, 40, BLACK);
    expect(firstInk(pixels, 40)).toBe(100);
    expect(firstInk(pixels, 34)).toBe(100);
    expect(firstInk(pixels, 33)).toBe(-1);

    // Half under the name tag: only the part outside it is drawn.
    const clipped = blank();
    stampGlyph(clipped, block, 78, 10, BLACK);
    expect(firstInk(clipped, 10)).toBe(80);
  });
});

describe("Typewriter", () => {
  it("starts beside the name tag and at the left edge below it", () => {
    const pixels = blank();
    const text = typewriter();
    text.type(pixels, "a", BLACK);
    text.newline();
    text.type(pixels, "b", BLACK);
    expect(firstInk(pixels, 13)).toBe(84);
    expect(firstInk(pixels, 29)).toBe(28);
  });

  it("wraps at the right edge and stops when the message is full", () => {
    const pixels = blank();
    const text = typewriter();
    let typed = 0;
    while (text.type(pixels, "x", BLACK)) typed += 1;
    // 27 fit beside the name tag, 36 on each of the four lines below.
    expect(typed).toBe(27 + 4 * 36);
    expect(text.newline()).toBe(false);
  });

  it("erases everything it typed, across wraps and line breaks", () => {
    const pixels = blank();
    const text = typewriter();
    for (let i = 0; i < 40; i += 1) text.type(pixels, "x", BLACK);
    text.newline();
    text.type(pixels, "y", BLACK);
    while (text.backspace(pixels));
    expect(isBlank(pixels)).toBe(true);
    text.type(pixels, "z", BLACK);
    expect(firstInk(pixels, 13)).toBe(84);
  });

  it("colors rainbow text by column", () => {
    const pixels = blank();
    const text = typewriter();
    for (let i = 0; i < 20; i += 1) text.type(pixels, "x", "rainbow");
    const shades = new Set(pixels);
    shades.delete(0);
    expect(shades.size).toBeGreaterThan(5);
    expect([...shades].every((shade) => RAINBOW_INKS.includes(shade))).toBe(true);
  });

  it("reports the last character until the pen moves", () => {
    const pixels = blank();
    const text = typewriter();
    expect(text.lastCharacter()).toBeUndefined();
    text.type(pixels, "か", BLACK);
    expect(text.lastCharacter()).toBe("か");
    text.newline();
    expect(text.lastCharacter()).toBeUndefined();
  });
});
