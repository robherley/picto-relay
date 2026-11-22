import assert from "node:assert/strict";
import { test } from "node:test";
import { inflateSync } from "node:zlib";

import { Canvas, INK_RGB, NATIVE_HEIGHT, WIDTH } from "../src/canvas.ts";
import { greeting } from "../src/greeting.ts";
import { png } from "../src/png.ts";

/** Reads back an indexed PNG's size, palette and rows. */
function decode(bytes: Uint8Array) {
  const data = Buffer.from(bytes);
  assert.equal(data.subarray(1, 4).toString("latin1"), "PNG");
  const chunks = new Map<string, Buffer>();
  for (let offset = 8; offset < data.length; ) {
    const length = data.readUInt32BE(offset);
    chunks.set(data.subarray(offset + 4, offset + 8).toString("latin1"), data.subarray(offset + 8, offset + 8 + length));
    offset += length + 12;
  }
  const header = chunks.get("IHDR")!;
  const [width, height] = [header.readUInt32BE(0), header.readUInt32BE(4)];
  const rows = inflateSync(chunks.get("IDAT")!);
  const rgb = (x: number, y: number) => {
    const index = rows[y * (width + 1) + 1 + x]!;
    return [...chunks.get("PLTE")!.subarray(index * 3, index * 3 + 3)];
  };
  return { width, height, rgb };
}

test("a drawing becomes an enlarged picture in screen colors", () => {
  const canvas = new Canvas();
  canvas.set(100, 40, 5);
  const { width, height, rgb } = decode(png(canvas));
  assert.equal(width / WIDTH, height / NATIVE_HEIGHT);
  const scale = width / WIDTH;
  assert.deepEqual(rgb(100 * scale, 40 * scale), [...INK_RGB[5]!]);
  assert.deepEqual(rgb(101 * scale - 1, 41 * scale - 1), [...INK_RGB[5]!]);
  assert.deepEqual(rgb(101 * scale, 40 * scale), [255, 255, 255]);
  // The frame around the box is neither paper nor an ink.
  assert.deepEqual(rgb(0, 0), [200, 200, 200]);
});

test("the greeting shows the logo, the whole model name and the fx version", () => {
  const short = greeting("anthropic/claude-haiku-5.5", "0.0.17");
  const long = greeting("some-provider/an-unusually-long-model-name-2026-11-15-preview", "0.0.17");
  // The version is part of the picture: another version is another drawing.
  assert.notDeepEqual(short.pixels, greeting("anthropic/claude-haiku-5.5", "0.0.18").pixels);
  assert.ok(!short.blank);
  // A longer name takes more lines beside the same logo.
  const inked = (canvas: Canvas) => canvas.pixels.filter((ink) => ink !== 0).length;
  assert.ok(inked(long) > inked(short));
  // Nothing spills out of the box: the last drawable column stays clear.
  for (let y = 0; y < NATIVE_HEIGHT; y += 1) assert.equal(long.get(251, y), 0);
});
