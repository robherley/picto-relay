import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { BLACK, Canvas, DRAW_LEFT, TAG_RIGHT, WIDTH } from "../src/canvas.ts";
import type { Ink } from "../src/canvas.ts";
import { BASELINE, FIRST_LINE_LETTERS, LINE_HEIGHT, LINE_LETTERS, MESSAGE_LETTERS, type } from "../src/text.ts";

/** The server's golden samples: a text, its ink, and the hash of its bitmap. */
const FIXTURES = JSON.parse(
  readFileSync(new URL("../../../crates/picto-relay-server/tests/text-fixtures.json", import.meta.url), "utf8"),
) as { text: string; ink: Ink; hash: number }[];

/** FNV-1a, as the server's test hashes a bitmap. */
function hash(bitmap: Uint8Array): number {
  return bitmap.reduce((hash, byte) => Math.imul(hash ^ byte, 16_777_619) >>> 0, 2_166_136_261);
}

function firstInk(canvas: Canvas, y: number): number {
  for (let x = 0; x < WIDTH; x += 1) if (canvas.get(x, y) !== 0) return x;
  return -1;
}

test("text is the same pixels the server draws", () => {
  assert.ok(FIXTURES.length > 0);
  for (const { text, ink, hash: expected } of FIXTURES) {
    const pages = type(text, ink);
    assert.equal(pages.length, 1, text);
    assert.equal(hash(pages[0]!.pack()), expected, text);
  }
});

test("text the server would refuse as too long carries on into another message", () => {
  const pages = type("HEAD ".repeat(44), BLACK);
  assert.equal(pages.length, 2);
  // Each message starts beside the name tag, then at the left edge.
  for (const page of pages) {
    assert.equal(firstInk(page, BASELINE), TAG_RIGHT + 4);
  }
  assert.equal(firstInk(pages[0]!, LINE_HEIGHT + BASELINE), DRAW_LEFT + 4);
});

test("a message holds as many letters as it says", () => {
  assert.equal(MESSAGE_LETTERS, FIRST_LINE_LETTERS + 4 * LINE_LETTERS);
  assert.equal(type("n".repeat(MESSAGE_LETTERS), BLACK).length, 1);
  assert.equal(type("n".repeat(MESSAGE_LETTERS + 1), BLACK).length, 2);
});

test("blank text and blank pages are not messages", () => {
  assert.deepEqual(type("  \n ", BLACK), []);
  assert.equal(type(`a${"\n".repeat(12)}b`, BLACK).length, 2);
});

test("a drawing survives packing", () => {
  const [page] = type("Hello", "rainbow");
  assert.deepEqual(Canvas.unpack(page!.pack()).pixels, page!.pixels);
});
