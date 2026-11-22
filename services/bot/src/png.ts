import { crc32, deflateSync } from "node:zlib";

import { Canvas, INK_RGB, NATIVE_HEIGHT, PAPER, WIDTH, drawable } from "./canvas.ts";

// A model reads a drawing as a picture. This writes one as an indexed PNG:
// the inks in their screen colors on white paper, and the part of the surface
// a DS never shows (outside the box, under the name tag) in the frame color.

/** Palette entry for blank surface outside the drawable box. */
const FRAME = INK_RGB.length;
const FRAME_RGB = [200, 200, 200] as const;

/** Screen pixels per DS pixel. A drawing is small; models see it better enlarged. */
const SCALE = 4;

const SIGNATURE = Uint8Array.of(0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a);

function chunk(type: string, data: Uint8Array): Uint8Array {
  const body = Buffer.concat([Buffer.from(type, "latin1"), data]);
  const out = Buffer.alloc(body.length + 8);
  out.writeUInt32BE(data.length, 0);
  body.copy(out, 4);
  out.writeUInt32BE(crc32(body), body.length + 4);
  return out;
}

/** The drawing's native rows as a PNG, enlarged. */
export function png(canvas: Canvas): Uint8Array {
  const width = WIDTH * SCALE;
  const height = NATIVE_HEIGHT * SCALE;

  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8; // bits per palette index
  header[9] = 3; // indexed color

  const palette = Uint8Array.from([...INK_RGB, FRAME_RGB].flat());

  // Each row starts with its filter type; zero is "none".
  const rows = Buffer.alloc((width + 1) * height);
  for (let y = 0; y < NATIVE_HEIGHT; y += 1) {
    const row = Buffer.alloc(width + 1);
    for (let x = 0; x < WIDTH; x += 1) {
      const ink = canvas.get(x, y);
      row.fill(ink === PAPER && !drawable(x, y) ? FRAME : ink, 1 + x * SCALE, 1 + (x + 1) * SCALE);
    }
    for (let copy = 0; copy < SCALE; copy += 1) row.copy(rows, (y * SCALE + copy) * (width + 1));
  }

  return Buffer.concat([
    SIGNATURE,
    chunk("IHDR", header),
    chunk("PLTE", palette),
    chunk("IDAT", deflateSync(rows)),
    chunk("IEND", new Uint8Array()),
  ]);
}
