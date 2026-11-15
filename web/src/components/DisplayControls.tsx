import { useEffect, useRef, useState } from "react";
import { type Display, type DisplaySettings, GAP_LIMIT, GAP_STEP } from "../hooks/display";
import { PixelIcon } from "./PixelIcon";

interface Props {
  settings: DisplaySettings;
  display: Display;
  onChange: (update: Partial<DisplaySettings>) => void;
}

const STACKED_ICON = [
  "#######",
  "#.....#",
  "#######",
  ".......",
  "#######",
  "#.....#",
  "#######",
];
const WIDE_ICON = [
  "#####.#####",
  "#...#.#...#",
  "#...#.#...#",
  "#####.#####",
];

const LAYOUTS = [
  { value: "auto", label: "Automatic arrangement", content: "Auto" },
  { value: "stacked", label: "One above the other", content: <PixelIcon art={STACKED_ICON} /> },
  { value: "wide", label: "Side by side", content: <PixelIcon art={WIDE_ICON} /> },
] as const;

const MENU_ICON = [
  "..#.#..",
  ".#####.",
  "##...##",
  ".#...#.",
  "##...##",
  ".#####.",
  "..#.#..",
];

/**
 * Arrangement, size and spacing of the two screens: a small button in the
 * corner of the page, outside the console, that opens a menu over it.
 */
export function DisplayControls({ settings, display, onChange }: Props) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  // The size slider runs from one screen pixel per DS pixel up to the most the
  // window has room for. Its top position means "fit": the console then keeps
  // filling the window as the window changes.
  const largest = Math.max(display.largestScale, 1);

  // Clicking anywhere else, or Escape, closes the menu.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  return (
    <div className="display-controls" ref={root}>
      <button
        type="button"
        className="plain choice"
        aria-label="Display settings"
        title="Display settings"
        aria-expanded={open}
        aria-pressed={open}
        onClick={() => setOpen(!open)}
      >
        <PixelIcon art={MENU_ICON} />
      </button>

      {open && (
        <div className="display-menu" role="group" aria-label="Display settings">
          <div role="group" aria-label="Arrangement">
            {LAYOUTS.map(({ value, label, content }) => (
              <button
                key={value}
                type="button"
                className="plain choice"
                aria-label={label}
                title={label}
                aria-pressed={settings.layout === value}
                onClick={() => onChange({ layout: value })}
              >
                {content}
              </button>
            ))}
          </div>

          <div>
            <label htmlFor="display-scale">Size</label>
            <input
              id="display-scale"
              type="range"
              min={1}
              max={largest}
              step={1}
              value={settings.scale === "fit" ? largest : Math.min(settings.scale, largest)}
              disabled={largest === 1}
              onChange={(event) => {
                const scale = Number(event.target.value);
                onChange({ scale: scale === largest ? "fit" : scale });
              }}
            />
          </div>

          <div>
            <label htmlFor="display-gap">Gap</label>
            <input
              id="display-gap"
              type="range"
              min={0}
              max={GAP_LIMIT}
              step={GAP_STEP}
              value={settings.gap}
              onChange={(event) => onChange({ gap: Number(event.target.value) })}
            />
          </div>
        </div>
      )}
    </div>
  );
}
