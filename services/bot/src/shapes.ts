import { BLACK, BOX_WIDTH, Canvas, DRAW_LEFT, INK_MAX, INK_NAMES, NATIVE_HEIGHT } from "./canvas.ts";
import type { Ink } from "./canvas.ts";
import { stamp } from "./text.ts";

// A picture described as a list of shapes, the way the model draws: lines,
// boxes, ellipses, polygons, SVG paths and text, each with an outline ink and
// an optional fill. Coordinates are in the message's drawable box, whose
// top-left corner is column `DRAW_LEFT` of the surface.

type Point = readonly [number, number];

/** A run of connected points. A closed one joins its last point to its first. */
export interface Subpath {
  points: Point[];
  closed: boolean;
}

/** Most shapes one drawing may list. */
export const MAX_SHAPES = 400;
/** Thickest outline, in pixels. */
export const MAX_THICKNESS = 4;

/** Straight pieces a curve is cut into. Plenty for a surface 228 pixels wide. */
const CURVE_STEPS = 16;

// --- Reading the model's input ---------------------------------------------------

/** Why a drawing could not be drawn, worded for the model that asked for it. */
export class ShapeError extends Error {}

type Fields = Record<string, unknown>;

function number(shape: Fields, key: string, where: string): number {
  const value = shape[key];
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new ShapeError(`${where}: "${key}" must be a number`);
  }
  return value;
}

function ink(value: unknown, where: string, key: string): Ink {
  if (value === "rainbow") return value;
  if (typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= INK_MAX) {
    return value;
  }
  const named = typeof value === "string" ? INK_NAMES.indexOf(value.toLowerCase() as never) : -1;
  if (named >= 0) return named;
  throw new ShapeError(`${where}: "${key}" must be an ink number 0–${INK_MAX} or "rainbow"`);
}

function points(value: unknown, where: string): Point[] {
  const valid =
    Array.isArray(value) &&
    value.length >= 2 &&
    value.every(
      (point) =>
        Array.isArray(point) &&
        point.length === 2 &&
        point.every((part) => typeof part === "number" && Number.isFinite(part)),
    );
  if (!valid) throw new ShapeError(`${where}: "points" must be a list of at least two [x, y] pairs`);
  return value as Point[];
}

// --- SVG paths -------------------------------------------------------------------

/** Reads SVG path data into runs of points, curves cut into straight pieces. */
export function parsePath(data: string): Subpath[] {
  const subpaths: Subpath[] = [];
  let at = 0;

  const skip = () => {
    while (at < data.length && /[\s,]/.test(data[at]!)) at += 1;
  };
  const value = (): number => {
    skip();
    const match = /^[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?/.exec(data.slice(at));
    if (!match) throw new ShapeError(`path: expected a number at "${data.slice(at, at + 12)}"`);
    at += match[0].length;
    return Number(match[0]);
  };
  // Arc flags are single digits that may run into the number after them.
  const flag = (): boolean => {
    skip();
    const digit = data[at];
    if (digit !== "0" && digit !== "1") throw new ShapeError("path: an arc flag must be 0 or 1");
    at += 1;
    return digit === "1";
  };

  let current: Subpath | undefined;
  let x = 0;
  let y = 0;
  let startX = 0;
  let startY = 0;
  // The last control point, which a smooth curve reflects.
  let control: Point | undefined;
  let command = "";

  const lineTo = (toX: number, toY: number) => {
    if (!current) {
      current = { points: [[x, y]], closed: false };
      subpaths.push(current);
    }
    current.points.push([toX, toY]);
    x = toX;
    y = toY;
  };
  const curveTo = (along: (t: number) => Point, steps = CURVE_STEPS) => {
    for (let step = 1; step <= steps; step += 1) lineTo(...along(step / steps));
  };

  for (;;) {
    skip();
    if (at >= data.length) break;
    if (/[a-zA-Z]/.test(data[at]!)) {
      command = data[at]!;
      at += 1;
    } else if (command === "") {
      throw new ShapeError("path: must start with a command such as M");
    } else if (command === "M") {
      command = "L"; // Further pairs after a move are lines.
    } else if (command === "m") {
      command = "l";
    }
    const relative = command === command.toLowerCase();
    const dx = relative ? x : 0;
    const dy = relative ? y : 0;
    let nextControl: Point | undefined;

    switch (command.toUpperCase()) {
      case "M": {
        x = value() + dx;
        y = value() + dy;
        startX = x;
        startY = y;
        current = undefined;
        break;
      }
      case "L":
        lineTo(value() + dx, value() + dy);
        break;
      case "H":
        lineTo(value() + dx, y);
        break;
      case "V":
        lineTo(x, value() + dy);
        break;
      case "C":
      case "S": {
        const [fromX, fromY] = [x, y];
        const smooth = command.toUpperCase() === "S";
        const [ax, ay]: Point = smooth
          ? control
            ? [2 * x - control[0], 2 * y - control[1]]
            : [x, y]
          : [value() + dx, value() + dy];
        const [bx, by] = [value() + dx, value() + dy];
        const [toX, toY] = [value() + dx, value() + dy];
        curveTo((t) => {
          const u = 1 - t;
          return [
            u * u * u * fromX + 3 * u * u * t * ax + 3 * u * t * t * bx + t * t * t * toX,
            u * u * u * fromY + 3 * u * u * t * ay + 3 * u * t * t * by + t * t * t * toY,
          ];
        });
        nextControl = [bx, by];
        break;
      }
      case "Q":
      case "T": {
        const [fromX, fromY] = [x, y];
        const smooth = command.toUpperCase() === "T";
        const [ax, ay]: Point = smooth
          ? control
            ? [2 * x - control[0], 2 * y - control[1]]
            : [x, y]
          : [value() + dx, value() + dy];
        const [toX, toY] = [value() + dx, value() + dy];
        curveTo((t) => {
          const u = 1 - t;
          return [
            u * u * fromX + 2 * u * t * ax + t * t * toX,
            u * u * fromY + 2 * u * t * ay + t * t * toY,
          ];
        });
        nextControl = [ax, ay];
        break;
      }
      case "A": {
        const [rx, ry, turn] = [Math.abs(value()), Math.abs(value()), (value() * Math.PI) / 180];
        const [large, sweep] = [flag(), flag()];
        const [toX, toY] = [value() + dx, value() + dy];
        const arc = arcFrom(x, y, toX, toY, rx, ry, turn, large, sweep);
        if (arc) curveTo(arc.at, arc.steps);
        else lineTo(toX, toY);
        break;
      }
      case "Z": {
        if (current) current.closed = true;
        x = startX;
        y = startY;
        current = undefined;
        break;
      }
      default:
        throw new ShapeError(`path: unknown command "${command}"`);
    }
    control = nextControl;
  }
  return subpaths.filter((subpath) => subpath.points.length >= 2);
}

/**
 * An SVG arc between two points as a function of 0..1, or nothing for an arc
 * with no size, which SVG draws as a straight line.
 */
function arcFrom(
  x1: number,
  y1: number,
  x2: number,
  y2: number,
  rx: number,
  ry: number,
  turn: number,
  large: boolean,
  sweep: boolean,
): { at: (t: number) => Point; steps: number } | undefined {
  if (rx === 0 || ry === 0 || (x1 === x2 && y1 === y2)) return undefined;
  const cos = Math.cos(turn);
  const sin = Math.sin(turn);
  // The SVG specification's conversion from endpoints to a center and angles.
  const px = (cos * (x1 - x2)) / 2 + (sin * (y1 - y2)) / 2;
  const py = (-sin * (x1 - x2)) / 2 + (cos * (y1 - y2)) / 2;
  const stretch = Math.sqrt((px * px) / (rx * rx) + (py * py) / (ry * ry));
  if (stretch > 1) {
    rx *= stretch;
    ry *= stretch;
  }
  const above = rx * rx * ry * ry - rx * rx * py * py - ry * ry * px * px;
  const below = rx * rx * py * py + ry * ry * px * px;
  const scale = (large === sweep ? -1 : 1) * Math.sqrt(Math.max(0, above / below));
  const centerX = (scale * rx * py) / ry;
  const centerY = (-scale * ry * px) / rx;
  const cx = cos * centerX - sin * centerY + (x1 + x2) / 2;
  const cy = sin * centerX + cos * centerY + (y1 + y2) / 2;
  const start = Math.atan2((py - centerY) / ry, (px - centerX) / rx);
  let span = Math.atan2((-py - centerY) / ry, (-px - centerX) / rx) - start;
  if (sweep && span < 0) span += 2 * Math.PI;
  if (!sweep && span > 0) span -= 2 * Math.PI;
  return {
    steps: Math.max(2, Math.ceil((Math.abs(span) / (2 * Math.PI)) * CURVE_STEPS * 3)),
    at: (t) => {
      const angle = start + span * t;
      const ex = rx * Math.cos(angle);
      const ey = ry * Math.sin(angle);
      return [cx + cos * ex - sin * ey, cy + sin * ex + cos * ey];
    },
  };
}

// --- Putting ink on the surface --------------------------------------------------

/** The pixels a pen of this thickness leaves around its position. */
function brush(thickness: number): Point[] {
  const low = -Math.floor((thickness - 1) / 2);
  const high = Math.ceil((thickness - 1) / 2);
  const center = (low + high) / 2;
  const reach = (thickness / 2) ** 2 - 0.5; // Rounds the corners off pens of 3 and up.
  const dots: Point[] = [];
  for (let dy = low; dy <= high; dy += 1) {
    for (let dx = low; dx <= high; dx += 1) {
      if (thickness < 3 || (dx - center) ** 2 + (dy - center) ** 2 <= reach) dots.push([dx, dy]);
    }
  }
  return dots;
}

/** Draws along runs of points with a pen. */
export function stroke(canvas: Canvas, subpaths: Subpath[], ink: Ink, thickness = 1): void {
  const dots = brush(thickness);
  const dab = (x: number, y: number) => {
    for (const [dx, dy] of dots) canvas.set(DRAW_LEFT + x + dx, y + dy, ink);
  };
  for (const { points, closed } of subpaths) {
    const path = closed ? [...points, points[0]!] : points;
    for (let index = 1; index < path.length; index += 1) {
      // Bresenham's line between the two pixels.
      let [x, y] = path[index - 1]!.map(Math.round) as [number, number];
      const [toX, toY] = path[index]!.map(Math.round) as [number, number];
      const dx = Math.abs(toX - x);
      const dy = -Math.abs(toY - y);
      const stepX = x < toX ? 1 : -1;
      const stepY = y < toY ? 1 : -1;
      let error = dx + dy;
      // Far outside the surface there is nothing to draw; do not walk there.
      for (let guard = 0; guard < 4096; guard += 1) {
        dab(x, y);
        if (x === toX && y === toY) break;
        const twice = 2 * error;
        if (twice >= dy) {
          error += dy;
          x += stepX;
        }
        if (twice <= dx) {
          error += dx;
          y += stepY;
        }
      }
    }
  }
}

/**
 * Colors the inside of the area the runs of points enclose, each run closed,
 * by SVG's default rule: a point is inside if the outlines wind around it.
 */
export function fill(canvas: Canvas, subpaths: Subpath[], ink: Ink): void {
  for (let y = 0; y < NATIVE_HEIGHT; y += 1) {
    const crossings: { x: number; winding: number }[] = [];
    for (const { points } of subpaths) {
      for (let index = 0; index < points.length; index += 1) {
        const [x1, y1] = points[index]!;
        const [x2, y2] = points[(index + 1) % points.length]!;
        if (y1 === y2 || y < Math.min(y1, y2) || y >= Math.max(y1, y2)) continue;
        crossings.push({ x: x1 + ((y - y1) * (x2 - x1)) / (y2 - y1), winding: y2 > y1 ? 1 : -1 });
      }
    }
    crossings.sort((a, b) => a.x - b.x);
    let winding = 0;
    for (let index = 0; index + 1 < crossings.length; index += 1) {
      winding += crossings[index]!.winding;
      if (winding === 0) continue;
      const from = Math.max(0, Math.ceil(crossings[index]!.x));
      const to = Math.min(BOX_WIDTH - 1, Math.floor(crossings[index + 1]!.x));
      for (let x = from; x <= to; x += 1) canvas.set(DRAW_LEFT + x, y, ink);
    }
  }
  // The scanlines stop short of the outline's own pixels; ink those too.
  stroke(
    canvas,
    subpaths.map((subpath) => ({ ...subpath, closed: true })),
    ink,
  );
}

/** The outline of one shape, as runs of points. */
function outline(shape: Fields, where: string): Subpath[] {
  switch (shape.type) {
    case "line":
      return [
        {
          points: [
            [number(shape, "x1", where), number(shape, "y1", where)],
            [number(shape, "x2", where), number(shape, "y2", where)],
          ],
          closed: false,
        },
      ];
    case "rect": {
      const [x, y] = [number(shape, "x", where), number(shape, "y", where)];
      const right = x + Math.max(1, number(shape, "width", where)) - 1;
      const bottom = y + Math.max(1, number(shape, "height", where)) - 1;
      return [
        {
          points: [
            [x, y],
            [right, y],
            [right, bottom],
            [x, bottom],
          ],
          closed: true,
        },
      ];
    }
    case "ellipse": {
      const [cx, cy] = [number(shape, "cx", where), number(shape, "cy", where)];
      const [rx, ry] = [number(shape, "rx", where), number(shape, "ry", where)];
      const steps = Math.min(180, Math.max(12, Math.ceil(Math.max(rx, ry) * 3)));
      const around: Point[] = [];
      for (let step = 0; step < steps; step += 1) {
        const angle = (step / steps) * 2 * Math.PI;
        around.push([cx + rx * Math.cos(angle), cy + ry * Math.sin(angle)]);
      }
      return [{ points: around, closed: true }];
    }
    case "polygon":
      return [{ points: points(shape.points, where), closed: true }];
    case "polyline":
      return [{ points: points(shape.points, where), closed: false }];
    case "path":
      if (typeof shape.d !== "string") throw new ShapeError(`${where}: "d" must be SVG path data`);
      return parsePath(shape.d);
    default:
      throw new ShapeError(
        `${where}: "type" must be line, rect, ellipse, polygon, polyline, path or text`,
      );
  }
}

/**
 * Draws a list of shapes onto a new drawing, in order, later ones on top.
 * Throws a `ShapeError` the model can act on if the list is not valid.
 */
export function draw(shapes: unknown): Canvas {
  if (!Array.isArray(shapes) || shapes.length === 0) {
    throw new ShapeError('"shapes" must be a list of at least one shape');
  }
  if (shapes.length > MAX_SHAPES) {
    throw new ShapeError(`A drawing holds at most ${MAX_SHAPES} shapes`);
  }
  const canvas = new Canvas();
  shapes.forEach((entry: unknown, index) => {
    const where = `shapes[${index}]`;
    if (typeof entry !== "object" || entry === null) throw new ShapeError(`${where}: not a shape`);
    const shape = entry as Fields;
    const pen = shape.ink === undefined ? BLACK : ink(shape.ink, where, "ink");

    if (shape.type === "text") {
      if (typeof shape.text !== "string") throw new ShapeError(`${where}: "text" must be a string`);
      const [x, y] = [number(shape, "x", where), number(shape, "y", where)];
      stamp(canvas, shape.text, DRAW_LEFT + Math.round(x), Math.round(y), pen);
      return;
    }

    const subpaths = outline(shape, where);
    if (shape.fill !== undefined && shape.fill !== null) {
      fill(canvas, subpaths, ink(shape.fill, where, "fill"));
    }
    const thickness = shape.thickness === undefined ? 1 : number(shape, "thickness", where);
    if (!Number.isInteger(thickness) || thickness < 1 || thickness > MAX_THICKNESS) {
      throw new ShapeError(`${where}: "thickness" must be a whole number 1–${MAX_THICKNESS}`);
    }
    stroke(canvas, subpaths, pen, thickness);
  });
  return canvas;
}
