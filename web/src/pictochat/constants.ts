// The PictoChat drawing surface, in DS pixels. These mirror the server's
// `protocol.rs`; `GET /api/v1/status` reports the same numbers.

/** Width of the drawing surface the API exchanges. */
export const WIDTH = 256;
/** Height of the API surface. Rows below `NATIVE_HEIGHT` are padding. */
export const HEIGHT = 96;
/** Height of the largest drawing a DS can show. */
export const NATIVE_HEIGHT = 80;
/** A DS message is one to five bands of this many rows. */
export const BAND_HEIGHT = 16;

/** Leftmost drawable column below the name tag. */
export const DRAW_LEFT = 24;
/** First column right of the drawable area. */
export const DRAW_RIGHT = 252;
/** Leftmost drawable column beside the name tag. */
export const TAG_RIGHT = 80;
/** Rows the name tag covers at the top-left of a message. */
export const TAG_HEIGHT = 16;
/** Width of a message box's interior. */
export const BOX_WIDTH = DRAW_RIGHT - DRAW_LEFT;

/** Palette index of blank paper. */
export const PAPER = 0;
/** Palette index of the black pen. */
export const BLACK = 1;
/** The rainbow pen's shades, in order. Matches the server's `RAINBOW_INKS`. */
export const RAINBOW_INKS = [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
/** Pixels a rainbow stroke or line of text travels before changing shade. */
export const RAINBOW_STEP = 6;

/**
 * Screen colors of the sixteen palette indices, read from a photograph of a
 * DS showing a block of each index side by side (2026-11-15): each block was averaged,
 * scaled so the paper is white, and rounded to the DS's five bits a channel.
 * Close, but a photograph of a screen is not a measurement.
 */
export const INK_RGB: readonly (readonly [number, number, number])[] = [
  [255, 255, 255], // 0 paper
  [0, 0, 0], // 1 black
  [216, 224, 248], // 2 gray
  [232, 80, 248], // 3 magenta
  [248, 48, 176], // 4 pink
  [248, 24, 0], // 5 red
  [248, 72, 0], // 6 red orange
  [248, 152, 0], // 7 orange
  [232, 216, 0], // 8 yellow
  [160, 232, 0], // 9 lime
  [104, 224, 0], // 10 green
  [72, 240, 232], // 11 teal
  [0, 208, 248], // 12 cyan
  [0, 120, 248], // 13 light blue
  [0, 64, 248], // 14 blue
  [0, 48, 224], // 15 dark blue
];

/** The pen colors offered, by palette index, named as a DS shows them. */
export const INKS = [
  { index: 1, label: "Black" },
  { index: 2, label: "Gray" },
  { index: 3, label: "Magenta" },
  { index: 4, label: "Pink" },
  { index: 5, label: "Red" },
  { index: 6, label: "Red orange" },
  { index: 7, label: "Orange" },
  { index: 8, label: "Yellow" },
  { index: 9, label: "Lime" },
  { index: 10, label: "Green" },
  { index: 11, label: "Teal" },
  { index: 12, label: "Cyan" },
  { index: 13, label: "Light blue" },
  { index: 14, label: "Blue" },
  { index: 15, label: "Dark blue" },
] as const;

/** A palette index as a CSS color. */
export function inkColor(index: number): string {
  return `rgb(${(INK_RGB[index] ?? INK_RGB[BLACK]!).join(" ")})`;
}

/** The sixteen DS profile colors, by wire index, as ds.css names them. */
export const PROFILE_COLORS = [
  { name: "slate", label: "Gray", hex: "#61829a" },
  { name: "maroon", label: "Brown", hex: "#ba4900" },
  { name: "red", label: "Red", hex: "#fb0018" },
  { name: "pink", label: "Pink", hex: "#fb8afb" },
  { name: "orange", label: "Orange", hex: "#fb9200" },
  { name: "yellow", label: "Yellow", hex: "#f3e300" },
  { name: "neonyellow", label: "Lime", hex: "#aafb00" },
  { name: "lime", label: "Green", hex: "#00fb00" },
  { name: "green", label: "Dark green", hex: "#00a238" },
  { name: "teal", label: "Teal", hex: "#49db8a" },
  { name: "turquoise", label: "Light blue", hex: "#30baf3" },
  { name: "blue", label: "Blue", hex: "#0059f3" },
  { name: "navy", label: "Dark blue", hex: "#000092" },
  { name: "darkpurple", label: "Dark purple", hex: "#8a00d3" },
  { name: "magenta", label: "Purple", hex: "#d300eb" },
  { name: "fuschia", label: "Magenta", hex: "#fb0092" },
] as const;

/** The profile color for a wire index, falling back to the DS default. */
export function profileColor(index: number | undefined) {
  return PROFILE_COLORS[index ?? 0] ?? PROFILE_COLORS[0];
}

export const ROOM_LETTERS = ["A", "B", "C", "D"] as const;
/** Members a PictoChat room holds. */
export const ROOM_CAPACITY = 16;

/** Longest nickname a DS profile holds, in UTF-16 units. */
export const NAME_MAX_UNITS = 10;
/** Longest personal message a DS profile holds, in UTF-16 units. */
export const PROFILE_MESSAGE_MAX_UNITS = 26;
