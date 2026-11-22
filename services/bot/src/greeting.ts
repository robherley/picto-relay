import { BLACK, BOX_WIDTH, Canvas, DRAW_LEFT } from "./canvas.ts";
import { fill, parsePath } from "./shapes.ts";
import { BASELINE, LINE_HEIGHT, LINES, measure, stamp } from "./text.ts";

// The bot's first message in a room: the fx logo, with the model's name and
// the version of fx hosting it beside it.

/** The fx mark, as fx.sh draws it in its header. */
const LOGO = {
  path: "M237.89 0C243.18 0 249.38 1.42 253.03 3.07L255.09 4.01L250.08 18.63L247.68 17.75C244.9 16.72 241.94 15.8 238.49 15.8C234.98 15.8 232.79 16.56 231.08 18.32C229.23 20.23 227.63 23.64 226.23 29.76L225.14 34.85H241.67L260.43 34.95L260.69 34.95L260.84 35.17L278.85 61.63L296.74 34.95H320.87L291.68 76.74L322.1 119.75H299.33L299.18 119.55L241.14 40.48L239.35 49.4H222.07L205.69 127.21C203.93 135.71 201.19 142.84 196.78 147.87C192.27 153.01 186.2 155.75 178.34 155.75C174.18 155.75 170.75 155.11 167.91 154.11L166.24 153.52V137.18L166.9 137.4L169.53 138.28C172.18 139.16 174.41 139.8 177.14 139.8C178.53 139.8 179.7 139.53 180.73 138.98C181.76 138.43 182.68 137.6 183.52 136.44C185.3 133.99 186.72 130.13 187.9 124.67L203.76 49.4H189.87L191.76 39.44L192.04 39.35L206.82 34.47L208.15 28.64C210.52 18.21 213.77 10.94 218.71 6.32C223.74 1.61 230.13 0 237.89 0ZM273.99 99.08L260.07 120.25H234.54L261 82.02L273.99 99.08Z",
  /** Top-left corner and size of the mark in the path's own units. */
  left: 166.24,
  top: 0,
  size: 156,
};

/** Where the logo sits in the box, clear of the name tag: its corner and height. */
const LOGO_X = 8;
const LOGO_Y = 20;
const LOGO_HEIGHT = 56;

/** Column of the box where the model's name starts. */
const NAME_X = LOGO_X + LOGO_HEIGHT + 12;
/** Gap kept between the name and the right edge of the box. */
const NAME_INSET = 4;

/**
 * Splits a model id into lines no wider than `width` pixels, breaking after
 * the slashes, hyphens and dots it already has where it can.
 */
export function wrapModel(model: string, width: number): string[] {
  const lines: string[] = [];
  let line = "";
  for (const piece of model.match(/[^/\-.:_ ]*[/\-.:_ ]?/g) ?? []) {
    if (line !== "" && measure(line + piece) > width) {
      lines.push(line);
      line = "";
    }
    // A piece too long for a line of its own is cut wherever it runs out.
    for (const character of piece) {
      if (line !== "" && measure(line + character) > width) {
        lines.push(line);
        line = "";
      }
      line += character;
    }
  }
  if (line !== "") lines.push(line);
  return lines;
}

/** The greeting drawing for a model hosted by this version of fx. */
export function greeting(model: string, fxVersion: string): Canvas {
  const canvas = new Canvas();

  const scale = LOGO_HEIGHT / LOGO.size;
  const mark = parsePath(LOGO.path).map(({ points, closed }) => ({
    closed,
    points: points.map(
      ([x, y]) => [LOGO_X + (x - LOGO.left) * scale, LOGO_Y + (y - LOGO.top) * scale] as const,
    ),
  }));
  fill(canvas, mark, BLACK);

  // The name goes on the ruled lines beside the logo, with the version on the
  // line below it, the block centered on the logo's height.
  const lines = wrapModel(model, BOX_WIDTH - NAME_X - NAME_INSET).slice(0, LINES - 2);
  lines.push(`fx ${fxVersion}`);
  const first = 1 + Math.floor((LINES - 1 - lines.length) / 2);
  lines.forEach((line, index) => {
    stamp(canvas, line, DRAW_LEFT + NAME_X, (first + index) * LINE_HEIGHT + BASELINE, BLACK);
  });
  return canvas;
}
