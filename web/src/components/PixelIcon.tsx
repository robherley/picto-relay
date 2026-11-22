// Icons are drawn as pixel art: one character per DS pixel.

const PIXEL = 2;

/** Surrounds the white (`o`) pixels of some art with a dark outline. */
function outlined(art: readonly string[]): string[] {
  const width = art[0]!.length + 2;
  const padded = [".".repeat(width), ...art.map((row) => `.${row}.`), ".".repeat(width)];
  const inked = (x: number, y: number) => padded[y]?.[x] === "o";
  return padded.map((row, y) =>
    Array.from(row, (cell, x) => {
      if (cell === "o") return "o";
      const near = [-1, 0, 1].some((dy) => [-1, 0, 1].some((dx) => inked(x + dx, y + dy)));
      return near ? "#" : ".";
    }).join(""),
  );
}
const INKS: Record<string, string> = {
  "#": "currentColor",
  o: "#ffffff",
  "+": "#8a8a8a",
  // A line that should sit back behind the main ink.
  "-": "#74747c",
};

export function PixelIcon({ art, label }: { art: readonly string[]; label?: string }) {
  const width = Math.max(...art.map((row) => row.length));
  return (
    <svg
      className="pixel-icon"
      width={width * PIXEL}
      height={art.length * PIXEL}
      viewBox={`0 0 ${width} ${art.length}`}
      shapeRendering="crispEdges"
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      {art.flatMap((row, y) =>
        Array.from(row, (character, x) => {
          const fill = INKS[character];
          return fill ? <rect key={`${x},${y}`} x={x} y={y} width={1} height={1} fill={fill} /> : null;
        }),
      )}
    </svg>
  );
}

/** The letters of rooms A to D, drawn bold: white, to be outlined. */
const ROOM_LETTERS_ART = [
  [
    "....oooo....",
    "...oooooo...",
    "..oooooooo..",
    "..ooo..ooo..",
    ".ooo....ooo.",
    ".ooo....ooo.",
    "ooo......ooo",
    "ooo......ooo",
    "oooooooooooo",
    "oooooooooooo",
    "oooooooooooo",
    "ooo......ooo",
    "ooo......ooo",
    "ooo......ooo",
    "ooo......ooo",
  ],
  [
    "oooooooooo..",
    "ooooooooooo.",
    "oooooooooooo",
    "ooo......ooo",
    "ooo......ooo",
    "ooo.....oooo",
    "ooooooooooo.",
    "oooooooooo..",
    "ooooooooooo.",
    "ooo.....oooo",
    "ooo......ooo",
    "ooo......ooo",
    "oooooooooooo",
    "ooooooooooo.",
    "oooooooooo..",
  ],
  [
    "..oooooooo..",
    ".oooooooooo.",
    "oooooooooooo",
    "oooo....oooo",
    "ooo......ooo",
    "ooo.........",
    "ooo.........",
    "ooo.........",
    "ooo.........",
    "ooo.........",
    "ooo......ooo",
    "oooo....oooo",
    "oooooooooooo",
    ".oooooooooo.",
    "..oooooooo..",
  ],
  [
    "ooooooooo...",
    "oooooooooo..",
    "ooooooooooo.",
    "ooo....ooooo",
    "ooo......ooo",
    "ooo......ooo",
    "ooo......ooo",
    "ooo......ooo",
    "ooo......ooo",
    "ooo......ooo",
    "ooo......ooo",
    "ooo....ooooo",
    "ooooooooooo.",
    "oooooooooo..",
    "ooooooooo...",
  ],
];

/** The badge of each room, by room number: its letter, outlined. */
export const ROOM_ICONS = ROOM_LETTERS_ART.map(outlined);

export const ICONS = {
  up: [
    "....#....",
    "...###...",
    "..#####..",
    ".#######.",
    "#########",
  ],
  down: [
    "#########",
    ".#######.",
    "..#####..",
    "...###...",
    "....#....",
  ],
  pen: [
    ".......##.",
    "......####",
    ".....####.",
    "....####..",
    "...####...",
    "..####....",
    ".####.....",
    ".###......",
    "##........",
    "#.........",
  ],
  eraser: [
    "....#######",
    "...#ooooo##",
    "..#ooooo#o#",
    ".#######oo#",
    ".#ooooo#o#.",
    ".#ooooo##..",
    ".#######...",
  ],
  thick: [
    "####",
    "####",
    "####",
    "####",
  ],
  thin: [
    "##",
    "##",
  ],
  close: [
    "##...##",
    ".##.##.",
    "..###..",
    "..###..",
    ".##.##.",
    "##...##",
  ],
  signal: [
    ".........##",
    ".........##",
    "......##.##",
    "......##.##",
    "...##.##.##",
    "...##.##.##",
    "##.##.##.##",
    "##.##.##.##",
  ],
  noSignal: [
    ".........++",
    ".........++",
    "......++.++",
    "......++.++",
    "...++.++.++",
    "...++.++.++",
    "++.++.++.++",
    "++.++.++.++",
  ],
  person: [
    "..###..",
    "..###..",
    "..###..",
    ".#####.",
    "#######",
    "#######",
    "#######",
  ],
  // A message leaving its tray, and one arriving in it.
  send: [
    ".........##.........",
    "........####........",
    ".......######.......",
    "......########......",
    ".....##########.....",
    "........####........",
    "........####........",
    "#######.oooo.#######",
    "#ooooo#.####.#ooooo#",
    "#oooooooooooooooooo#",
    "#oooooooooooooooooo#",
    "####################",
  ],
  copy: [
    "........####........",
    "........oooo........",
    "........####........",
    "........oooo........",
    ".....##########.....",
    "#####.########.#####",
    "#oooo#.######.#oooo#",
    "#ooooo#.####.#ooooo#",
    "#oooooo#.##.#oooooo#",
    "#oooooooooooooooooo#",
    "####################",
  ],
  // The tray again, under a tight ring of marks with a white glow.
  clear: [
    "........o##o........",
    "....ooooo##ooooo....",
    "....o##oo##oo##o....",
    "....o##oooooo##o....",
    "----oooooooooooo----",
    "-oooooooooooooooooo-",
    "-oo###oooooooo###oo-",
    "-oo###oooooooo###oo-",
    "-oooooooooooooooooo-",
    "----oooooooooooo----",
    "....o##oooooo##o....",
    "....o##oo##oo##o....",
    "....ooooo##ooooo....",
    "........o##o........",
  ],
  // "SEND", white letters to be outlined.
  sendLabel: outlined([
    "oooo.oooo.o..o.ooo.",
    "o....o....oo.o.o..o",
    "oooo.ooo..o.oo.o..o",
    "...o.o....o..o.o..o",
    "oooo.oooo.o..o.ooo.",
  ]),
} as const;
