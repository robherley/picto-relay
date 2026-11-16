import assert from "node:assert/strict";
import { test } from "node:test";

import { DRAW_LEFT, RAINBOW_INKS } from "../src/canvas.ts";
import { ShapeError, draw, parsePath } from "../src/shapes.ts";

/** The ink at a point of the drawable box. */
const at = (canvas: ReturnType<typeof draw>, x: number, y: number) => canvas.get(DRAW_LEFT + x, y);

test("a filled rectangle covers exactly its pixels", () => {
  const canvas = draw([{ type: "rect", x: 100, y: 30, width: 10, height: 5, ink: 5, fill: 8 }]);
  assert.equal(at(canvas, 100, 30), 5);
  assert.equal(at(canvas, 109, 34), 5);
  assert.equal(at(canvas, 104, 32), 8);
  assert.equal(at(canvas, 99, 32), 0);
  assert.equal(at(canvas, 110, 32), 0);
  assert.equal(at(canvas, 104, 35), 0);
  assert.equal(canvas.pixels.filter((ink) => ink !== 0).length, 50);
});

test("lines, thickness and ellipses land where asked", () => {
  const canvas = draw([
    { type: "line", x1: 60, y1: 40, x2: 70, y2: 40 },
    { type: "line", x1: 60, y1: 50, x2: 70, y2: 50, thickness: 3, ink: 14 },
    { type: "ellipse", cx: 150, cy: 40, rx: 20, ry: 10, fill: 9 },
  ]);
  assert.equal(at(canvas, 65, 40), 1);
  assert.equal(at(canvas, 65, 41), 0);
  assert.deepEqual([at(canvas, 65, 49), at(canvas, 65, 50), at(canvas, 65, 51)], [14, 14, 14]);
  assert.equal(at(canvas, 150, 40), 9);
  assert.equal(at(canvas, 170, 40), 1);
  assert.equal(at(canvas, 172, 40), 0);
});

test("paths draw curves, arcs and holes", () => {
  // A ring: the inner circle winds the other way, so it stays paper.
  const ring = draw([
    {
      type: "path",
      d: "M100 40a20 20 0 1 0 40 0a20 20 0 1 0-40 0zM110 40a10 10 0 1 1 20 0a10 10 0 1 1-20 0z",
      fill: 3,
      ink: 3,
    },
  ]);
  assert.equal(at(ring, 105, 40), 3);
  assert.equal(at(ring, 120, 40), 0);

  const [curve] = parsePath("M0 0 Q10 20 20 0 T40 0");
  assert.deepEqual(curve!.points.at(-1), [40, 0]);
  assert.ok(Math.abs(curve!.points[8]![1] - 10) < 0.01);
  // Compact arc flags, as minified SVG writes them.
  assert.equal(parsePath("M0 0a5 5 0 015 5").length, 1);
});

test("nothing shows under the name tag or outside the box", () => {
  const canvas = draw([{ type: "rect", x: -50, y: -50, width: 400, height: 300, fill: 1 }]);
  assert.equal(at(canvas, 10, 5), 0);
  assert.equal(at(canvas, 56, 5), 1);
  assert.equal(at(canvas, 10, 16), 1);
  assert.equal(canvas.get(DRAW_LEFT - 1, 40), 0);
  assert.equal(canvas.get(DRAW_LEFT + 228, 40), 0);
  assert.equal(canvas.get(DRAW_LEFT + 100, 80), 0);
});

test("text and rainbow ink work in a drawing", () => {
  const canvas = draw([{ type: "text", x: 60, y: 40, text: "HI", ink: "rainbow" }]);
  const inks = new Set(canvas.pixels);
  inks.delete(0);
  assert.ok(inks.size > 0);
  assert.ok([...inks].every((ink) => RAINBOW_INKS.includes(ink)));
  assert.equal(at(canvas, 60, 40), RAINBOW_INKS[Math.floor((DRAW_LEFT + 60) / 6) % 13]);
});

test("a bad drawing says what is wrong with it", () => {
  for (const shapes of [
    [],
    "circle",
    [{ type: "circle" }],
    [{ type: "line", x1: 0, y1: 0, x2: "ten", y2: 0 }],
    [{ type: "rect", x: 0, y: 0, width: 5, height: 5, ink: 16 }],
    [{ type: "polygon", points: [[1, 2]] }],
    [{ type: "path", d: "10 10" }],
    [{ type: "line", x1: 0, y1: 0, x2: 9, y2: 9, thickness: 9 }],
  ]) {
    assert.throws(() => draw(shapes), ShapeError, JSON.stringify(shapes));
  }
  // Ink names are understood, since a model may well use them.
  assert.equal(at(draw([{ type: "line", x1: 60, y1: 40, x2: 70, y2: 40, ink: "Dark Blue" }]), 65, 40), 15);
});
