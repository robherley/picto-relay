// Borders with the DS's stair-stepped corners, drawn as pixel art and used as
// CSS border images. ds.css ships the same shapes as fixed gray PNGs; building
// them here lets each one take any color.

/** A box's corner, two rings: `o` outer, `i` inner. Each side is 4 pixels. */
const BOX_ART = [
  "....oo....",
  "...oiio...",
  "..oi..io..",
  ".oi....io.",
  "oi......io",
  "oi......io",
  ".oi....io.",
  "..oi..io..",
  "...oiio...",
  "....oo....",
];

/** A slimmer box corner: one ring, 2 pixels a side, joined by a single step. */
const SLIM_ART = [
  "..oo..",
  ".o..o.",
  "o....o",
  "o....o",
  ".o..o.",
  "..oo..",
];

/**
 * A name tag's right and bottom edges, which meet in a step at the corner.
 * Each side is 2 pixels; only the right and bottom ones are used.
 */
const TAG_ART = [
  "..oooo",
  ".o...o",
  "o....o",
  "o....o",
  "o....o",
  "o....o",
  "o...o.",
  "oooo..",
];

/** CSS pixels per pixel of art. */
const PIXEL = 2;

const images = new Map<string, string>();

/** A CSS `url()` of pixel art, one character per pixel, colored by `colors`. */
function pixelImage(art: readonly string[], colors: Record<string, string>): string {
  const key = art.join() + JSON.stringify(colors);
  let url = images.get(key);
  if (!url) {
    const width = art[0]!.length;
    const cells = art
      .flatMap((row, y) =>
        Array.from(row, (cell, x) =>
          colors[cell] ? `<rect x="${x}" y="${y}" width="1" height="1" fill="${colors[cell]}"/>` : "",
        ),
      )
      .join("");
    const svg =
      `<svg xmlns="http://www.w3.org/2000/svg" width="${width * PIXEL}" height="${art.length * PIXEL}" ` +
      `viewBox="0 0 ${width} ${art.length}" shape-rendering="crispEdges">${cells}</svg>`;
    url = `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
    images.set(key, url);
  }
  return url;
}

/** Border image for a box: an outer ring and an inner one. */
export const boxBorder = (outer: string, inner: string) => pixelImage(BOX_ART, { o: outer, i: inner });

/** Border image for a box with a single slim ring. */
export const slimBorder = (color: string) => pixelImage(SLIM_ART, { o: color });

/** Border image for the right and bottom edges of a name tag. */
export const tagBorder = (color: string) => pixelImage(TAG_ART, { o: color });

/** Blends a `#rrggbb` color toward white by `amount` (0 to 1). */
export function lighten(hex: string, amount: number): string {
  const channels = [1, 3, 5].map((start) => parseInt(hex.slice(start, start + 2), 16));
  return `#${channels
    .map((channel) => Math.round(channel + (255 - channel) * amount).toString(16).padStart(2, "0"))
    .join("")}`;
}
