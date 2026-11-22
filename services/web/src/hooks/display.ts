// How the two screens are arranged and sized in the window. The console is
// laid out at two CSS pixels per DS pixel and then zoomed as one piece, so a
// "scale" of 4 (four screen pixels per DS pixel) is a zoom of 2.

export type Arrangement = "stacked" | "wide";

export interface DisplaySettings {
  /** Screens one above the other, side by side, or whichever fits best. */
  layout: Arrangement | "auto";
  /** Screen pixels per DS pixel, or `fit` for the largest sharp size that fits. */
  scale: number | "fit";
  /** Space between the two screens, in CSS pixels before zooming. */
  gap: number;
}

export interface Display {
  layout: Arrangement;
  zoom: number;
  /** The largest whole scale the window has room for. */
  largestScale: number;
}

export const DEFAULT_DISPLAY: DisplaySettings = { layout: "auto", scale: "fit", gap: 12 };

/** The largest scale offered, in screen pixels per DS pixel. */
export const LARGEST_SCALE = 6;
export const GAP_LIMIT = 160;
export const GAP_STEP = 4;

/** CSS pixels per DS pixel in the unzoomed layout. */
const LAYOUT_SCALE = 2;
/** One screen, unzoomed. */
const SCREEN = { width: 512, height: 384 };
/** Room kept above and below the console, and for the screens' outlines. */
const VERTICAL_MARGIN = 24;
const OUTLINE = 8;
/** Zooms `fit` may choose: each keeps a DS pixel a whole number of half pixels. */
const SHARP_ZOOMS = [3, 2.5, 2, 1.5, 1];

/** The unzoomed size of the console in an arrangement. */
export function consoleSize(layout: Arrangement, gap: number) {
  return layout === "stacked"
    ? { width: SCREEN.width + OUTLINE, height: SCREEN.height * 2 + gap + VERTICAL_MARGIN }
    : { width: SCREEN.width * 2 + gap + OUTLINE, height: SCREEN.height + VERTICAL_MARGIN };
}

/**
 * Settles the arrangement and zoom for a window. The window is the limit: a
 * scale too large for it is reduced until the console fits.
 */
export function resolveDisplay(
  settings: DisplaySettings, viewport: { width: number; height: number },
): Display {
  const room = (layout: Arrangement) => {
    const size = consoleSize(layout, settings.gap);
    return Math.min(viewport.width / size.width, viewport.height / size.height);
  };
  const zoomFor = (layout: Arrangement) =>
    settings.scale === "fit"
      ? (SHARP_ZOOMS.find((zoom) => zoom <= room(layout)) ?? room(layout))
      : Math.min(settings.scale / LAYOUT_SCALE, room(layout));

  const layouts: Arrangement[] = settings.layout === "auto" ? ["stacked", "wide"] : [settings.layout];
  // Stacked, like an open DS, unless side by side can be shown larger.
  const layout = layouts.reduce((best, next) => (zoomFor(next) > zoomFor(best) ? next : best));
  const mostRoom = Math.max(...layouts.map(room));
  return {
    layout,
    zoom: zoomFor(layout),
    largestScale: Math.min(Math.floor(mostRoom * LAYOUT_SCALE), LARGEST_SCALE),
  };
}
