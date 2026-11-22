import { type CSSProperties, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { type Key, type KeyboardState, keyboardRows } from "../pictochat/keyboard";
import { slimBorder } from "./borders";

interface Props {
  state: KeyboardState;
  onKey: (key: Key) => void;
  /** A character key was dragged off the keyboard and let go at this point of the window. */
  onDrop: (character: string, point: { x: number; y: number }) => void;
  /** Sets `--ink`, the color a dragged character is shown in. */
  inkStyle: CSSProperties;
}

/** Keys labelled with a word are set in the DS's small print. */
const wordy = (key: Key) => (key.label?.length ?? 0) > 1;

/** The keyboard panel's outline. */
const PANEL_BORDER = { borderImageSource: slimBorder("#6a6a72") };

/** Keys never take focus, so the physical keyboard keeps typing. */
const keepFocus = (event: React.MouseEvent) => event.preventDefault();

/** How far the pointer must travel before a press becomes a drag. */
const DRAG_THRESHOLD = 6;
/** The interface's type size before zooming. */
const FONT_SIZE = 20;

export function Keyboard({ state, onKey, onDrop, inkStyle }: Props) {
  // As on a DS, a character can be dragged off its key and dropped anywhere
  // on the drawing. A press that never moves is an ordinary key press.
  const drag = useRef<{ character: string; x: number; y: number; moved: boolean } | null>(null);
  const dragged = useRef(false);
  const [ghost, setGhost] = useState<{ character: string; x: number; y: number; size: number } | null>(null);

  const beginDrag = (character: string) => (event: React.PointerEvent<HTMLButtonElement>) => {
    event.currentTarget.setPointerCapture(event.pointerId);
    drag.current = { character, x: event.clientX, y: event.clientY, moved: false };
    dragged.current = false;
  };

  const moveDrag = (event: React.PointerEvent) => {
    const current = drag.current;
    if (!current) return;
    const travel = Math.hypot(event.clientX - current.x, event.clientY - current.y);
    if (!current.moved && travel < DRAG_THRESHOLD) return;
    current.moved = true;
    // The ghost is drawn outside the zoomed console, so it scales itself.
    const zoom = Number(getComputedStyle(document.documentElement).getPropertyValue("--zoom")) || 1;
    setGhost({ character: current.character, x: event.clientX, y: event.clientY, size: FONT_SIZE * zoom });
  };

  const endDrag = (event: React.PointerEvent, drop: boolean) => {
    const current = drag.current;
    drag.current = null;
    setGhost(null);
    if (!current?.moved) return;
    // The click that follows a drag must not also type the character.
    dragged.current = true;
    if (drop) onDrop(current.character, { x: event.clientX, y: event.clientY });
  };

  const press = (key: Key) => {
    if (dragged.current) {
      dragged.current = false;
      return;
    }
    onKey(key);
  };

  const lit = (key: Key) =>
    (key.action === "caps" && state.caps) || (key.action === "shift" && state.shift);

  // Each key is placed on a grid of half-key columns by walking its row.
  const keys = keyboardRows(state).flatMap((row, rowIndex) => {
    let column = row.indent + 1;
    return row.keys.flatMap((key) => {
      const place: CSSProperties = {
        gridColumn: `${column} / span ${key.span}`,
        gridRow: `${rowIndex + 1} / span ${key.rows ?? 1}`,
      };
      column += key.span;
      if (key.character === undefined && key.action === undefined) return [];
      const { character } = key;
      return [
        <button
          key={`${rowIndex}:${key.action ?? character}`}
          type="button"
          className={`plain key${key.action ? " function" : ""}${wordy(key) ? " word" : ""}`}
          style={place}
          aria-label={key.action}
          aria-pressed={key.action === "caps" || key.action === "shift" ? lit(key) : undefined}
          onMouseDown={keepFocus}
          onPointerDown={character === undefined ? undefined : beginDrag(character)}
          onPointerMove={moveDrag}
          onPointerUp={(event) => endDrag(event, true)}
          onPointerCancel={(event) => endDrag(event, false)}
          onClick={() => press(key)}
        >
          {key.label ?? character}
        </button>,
      ];
    });
  });

  return (
    <div className="keyboard slim-ring" style={PANEL_BORDER}>
      {keys}
      {ghost &&
        createPortal(
          <span
            className="key-ghost"
            style={{ ...inkStyle, left: ghost.x, top: ghost.y, fontSize: ghost.size }}
          >
            {ghost.character}
          </span>,
          document.body,
        )}
    </div>
  );
}
