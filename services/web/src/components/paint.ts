import { BOX_WIDTH, DRAW_LEFT, INK_RGB, PAPER, WIDTH } from "../pictochat/constants";
import type { Pixels } from "../pictochat/drawing";

/**
 * Paints the top `height` rows of a drawing's box interior onto a canvas of
 * `BOX_WIDTH` by `height`. Paper is left transparent.
 */
export function paintInk(canvas: HTMLCanvasElement, pixels: Pixels, height: number): void {
  const context = canvas.getContext("2d");
  if (!context) return;
  const image = context.createImageData(BOX_WIDTH, height);
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < BOX_WIDTH; x += 1) {
      const index = pixels[y * WIDTH + DRAW_LEFT + x]!;
      if (index === PAPER) continue;
      const offset = (y * BOX_WIDTH + x) * 4;
      image.data.set(INK_RGB[index] ?? INK_RGB[1]!, offset);
      image.data[offset + 3] = 255;
    }
  }
  context.putImageData(image, 0, 0);
}
