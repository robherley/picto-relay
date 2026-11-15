import { type CSSProperties, useEffect, useReducer, useRef, useState } from "react";
import type { Participant } from "../api/client";
import {
  BLACK, BOX_WIDTH, DRAW_LEFT, inkColor, INKS, NATIVE_HEIGHT, PAPER,
} from "../pictochat/constants";
import { blank, drawLine, type Ink, isBlank, type Pixels, type Point } from "../pictochat/drawing";
import { glyphOf } from "../pictochat/glyphs";
import { type Key, type KeyboardState, markKana, type Page } from "../pictochat/keyboard";
import { GLYPH_HALF_HEIGHT, stampGlyph, Typewriter } from "../pictochat/typing";
import { boxBorder } from "./borders";
import { Keyboard } from "./Keyboard";
import { MessageBox } from "./MessageBox";
import { paintInk } from "./paint";
import { ICONS, PixelIcon } from "./PixelIcon";

interface Props {
  name: string;
  /** Our own color, as `#rrggbb`. */
  tone: string;
  members: Participant[];
  /** A member's color, as `#rrggbb`. */
  toneOf: (stationId: number) => string;
  /** Whether the room is ready to take a message. */
  ready: boolean;
  /** The newest message in the log, for the copy button. */
  latest: Pixels | undefined;
  onSend: (pixels: Pixels) => void;
  onLeave: () => void;
  /** Scrolls the message log up (-1) or down (1). */
  onScroll: (direction: -1 | 1) => void;
}

type Tool = "pen" | "eraser";

/** Width in pixels of each tool's thick and thin tips. */
const TIP = { pen: { thick: 2, thin: 1 }, eraser: { thick: 8, thin: 4 } };

const PAGES: { page: Page; label: string; name: string }[] = [
  { page: "latin", label: "A", name: "Letters and numbers" },
  { page: "accents", label: "é", name: "Accented letters" },
  { page: "kana", label: "あ", name: "Kana" },
  { page: "symbols", label: "@", name: "Symbols" },
  { page: "pictograms", label: "\uE026", name: "Pictograms" },
];

/** Outlines of the gray panel behind the composer and of the button group on it. */
const BACKDROP_BORDER = { borderImageSource: boxBorder("#494949", "#c8c8c8") };
const ACTION_BORDER = { borderImageSource: boxBorder("#5a5a62", "#f8f8f8") };

const keepFocus = (event: React.MouseEvent) => event.preventDefault();

const RAINBOW = "var(--rainbow)";
const inkStyle = (ink: Ink) =>
  ({ "--ink": ink === "rainbow" ? RAINBOW : inkColor(ink) }) as CSSProperties;

/** The tool-strip button showing the pen color, and the palette it opens. */
function InkPicker({ ink, onPick }: { ink: Ink; onPick: (ink: Ink) => void }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);

  // Clicking anywhere else, or Escape, puts the palette away.
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

  const swatch = (value: Ink, label: string) => (
    <button
      key={value}
      type="button"
      className="plain ink-swatch"
      style={inkStyle(value)}
      aria-label={label}
      title={label}
      aria-pressed={value === ink}
      onMouseDown={keepFocus}
      onClick={() => {
        onPick(value);
        setOpen(false);
      }}
    />
  );

  return (
    <div className="ink-tool" ref={root}>
      <button
        type="button"
        className="plain tool"
        aria-label="Pen color"
        title="Pen color"
        aria-expanded={open}
        onMouseDown={keepFocus}
        onClick={() => setOpen(!open)}
      >
        <span className="ink-chip" style={inkStyle(ink)} />
      </button>
      {open && (
        <div className="ink-palette" role="group" aria-label="Pen color">
          {INKS.map(({ index, label }) => swatch(index, label))}
          {swatch("rainbow", "Rainbow")}
        </div>
      )}
    </div>
  );
}

/** The bottom screen: tools, the message being composed, and the keyboard. */
export function Composer({
  name, tone, members, toneOf, ready, latest, onSend, onLeave, onScroll,
}: Props) {
  // The drawing changes on every pointer move, so it lives outside React
  // state; `repaint` asks for the canvas to be redrawn.
  const pixels = useRef(blank()).current;
  const typewriter = useRef(new Typewriter(glyphOf)).current;
  const rainbow = useRef({ step: 0 }).current;
  const [revision, repaint] = useReducer((count: number) => count + 1, 0);
  const canvas = useRef<HTMLCanvasElement>(null);
  const lastPoint = useRef<Point | null>(null);

  const [tool, setTool] = useState<Tool>("pen");
  // The pen is one fixed color, or the rainbow; clicking the selected pen
  // switches between the two, as on a DSi.
  const [fixedInk, setFixedInk] = useState<number>(BLACK);
  const [rainbowPen, setRainbowPen] = useState(false);
  const [thick, setThick] = useState(true);
  const [keyboard, setKeyboard] = useState<KeyboardState>({
    page: "latin", caps: false, shift: false, katakana: false,
  });

  const penInk: Ink = rainbowPen ? "rainbow" : fixedInk;

  const pickInk = (ink: Ink) => {
    setRainbowPen(ink === "rainbow");
    if (ink !== "rainbow") setFixedInk(ink);
    setTool("pen");
  };

  useEffect(() => {
    if (canvas.current) paintInk(canvas.current, pixels, NATIVE_HEIGHT);
  }, [pixels, revision]);

  const clear = () => {
    pixels.fill(PAPER);
    typewriter.reset();
    rainbow.step = 0;
    repaint();
  };

  const send = () => {
    if (!ready || isBlank(pixels)) return;
    onSend(pixels.slice());
    clear();
  };

  const copyLatest = () => {
    if (!latest) return;
    pixels.set(latest);
    typewriter.reset();
    repaint();
  };

  // --- Drawing ---------------------------------------------------------------

  /** The drawing pixel under a point of the window. */
  const pixelAt = (clientX: number, clientY: number, box: DOMRect): Point => ({
    x: DRAW_LEFT + Math.floor(((clientX - box.left) * BOX_WIDTH) / box.width),
    y: Math.floor(((clientY - box.top) * NATIVE_HEIGHT) / box.height),
  });

  const pointAt = (event: React.PointerEvent<HTMLCanvasElement>): Point =>
    pixelAt(event.clientX, event.clientY, event.currentTarget.getBoundingClientRect());

  const strokeTo = (point: Point) => {
    const size = TIP[tool][thick ? "thick" : "thin"];
    const ink = tool === "eraser" ? PAPER : penInk;
    drawLine(pixels, lastPoint.current ?? point, point, size, ink, rainbow);
    lastPoint.current = point;
    repaint();
  };

  // --- Typing ----------------------------------------------------------------

  const type = (character: string) => {
    if (typewriter.type(pixels, character, penInk)) repaint();
  };

  // A character dragged off the keyboard lands wherever it is dropped, its
  // middle under the pointer. It is ink like any stroke: Backspace leaves it.
  const dropCharacter = (character: string, point: { x: number; y: number }) => {
    const box = canvas.current?.getBoundingClientRect();
    if (!box) return;
    const inside =
      point.x >= box.left && point.x < box.right && point.y >= box.top && point.y < box.bottom;
    if (!inside) return;
    const glyph = glyphOf(character);
    const { x, y } = pixelAt(point.x, point.y, box);
    stampGlyph(pixels, glyph, x - Math.floor(glyph.advance / 2), y + GLYPH_HALF_HEIGHT, penInk);
    if (keyboard.shift) setKeyboard({ ...keyboard, shift: false });
    repaint();
  };

  const erase = () => {
    if (typewriter.backspace(pixels)) repaint();
  };

  const newline = () => {
    typewriter.newline();
  };

  const pressKey = (key: Key) => {
    if (key.character !== undefined) {
      type(key.character);
      if (keyboard.shift) setKeyboard({ ...keyboard, shift: false });
      return;
    }
    switch (key.action) {
      case "backspace": return erase();
      case "enter": return newline();
      case "space": return type(" ");
      case "caps": return setKeyboard({ ...keyboard, caps: !keyboard.caps });
      case "shift": return setKeyboard({ ...keyboard, shift: !keyboard.shift });
      case "katakana": return setKeyboard({ ...keyboard, katakana: !keyboard.katakana });
      case "dakuten":
      case "handakuten":
      case "small": {
        const last = typewriter.lastCharacter();
        const marked = last && markKana(last, key.action);
        if (!marked) return;
        typewriter.backspace(pixels);
        return type(marked);
      }
    }
  };

  // The physical keyboard types too. Enter starts a new line, as on the DS;
  // Ctrl or Command with Enter sends.
  const handlers = useRef({ type, erase, newline, send });
  handlers.current = { type, erase, newline, send };
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const focused = document.activeElement;
      if (focused instanceof HTMLInputElement || focused instanceof HTMLButtonElement) return;
      const { type, erase, newline, send } = handlers.current;
      if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) send();
      else if (event.ctrlKey || event.metaKey || event.altKey) return;
      else if (event.key === "Enter") newline();
      else if (event.key === "Backspace") erase();
      else if (Array.from(event.key).length === 1) type(event.key);
      else return;
      event.preventDefault();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  // --- View ------------------------------------------------------------------

  const toolButton = (
    label: string, art: readonly string[], pressed: boolean, onClick: () => void, extra = "",
  ) => (
    <button
      type="button"
      className={`plain tool ${extra}`}
      aria-label={label}
      title={label}
      aria-pressed={pressed}
      onMouseDown={keepFocus}
      onClick={onClick}
    >
      <PixelIcon art={art} />
    </button>
  );

  return (
    <section className="screen composer" aria-label="Write a message">
      <div className="tool-strip">
        <button type="button" className="plain tool scroll" aria-label="Scroll up" onMouseDown={keepFocus} onClick={() => onScroll(-1)}>
          <PixelIcon art={ICONS.up} />
        </button>
        <button type="button" className="plain tool scroll" aria-label="Scroll down" onMouseDown={keepFocus} onClick={() => onScroll(1)}>
          <PixelIcon art={ICONS.down} />
        </button>
        <hr />
        {toolButton(
          rainbowPen ? "Rainbow pen (click for a plain color)" : "Pen (click again for rainbow)",
          ICONS.pen,
          tool === "pen",
          () => {
            if (tool === "pen") setRainbowPen(!rainbowPen);
            setTool("pen");
          },
          rainbowPen ? "rainbow" : "",
        )}
        {toolButton("Eraser", ICONS.eraser, tool === "eraser", () => setTool("eraser"))}
        <InkPicker ink={penInk} onPick={pickInk} />
        <hr />
        {toolButton("Thick", ICONS.thick, thick, () => setThick(true))}
        {toolButton("Thin", ICONS.thin, !thick, () => setThick(false))}
        <hr />
        {PAGES.map(({ page, label, name }) => (
          <button
            key={page}
            type="button"
            className="plain tool"
            aria-label={name}
            title={name}
            aria-pressed={keyboard.page === page}
            onMouseDown={keepFocus}
            onClick={() => setKeyboard({ ...keyboard, page, shift: false })}
          >
            {label}
          </button>
        ))}
      </div>

      <div className="backdrop ring pictochat-board" style={BACKDROP_BORDER} />

      <div className="room-status">
        <ul className="members" aria-label="In this room">
          {members.map((member) => (
            <li key={member.client_id} style={{ "--tone": toneOf(member.client_id) } as CSSProperties}>
              {member.name}
            </li>
          ))}
        </ul>
        <button type="button" className="plain close" aria-label="Leave the room" title="Leave the room" onClick={onLeave}>
          <PixelIcon art={ICONS.close} />
        </button>
      </div>

      <MessageBox name={name} tone={tone} className="compose">
        <canvas
          ref={canvas}
          className="ink ruled"
          width={BOX_WIDTH}
          height={NATIVE_HEIGHT}
          aria-label="Drawing area"
          onPointerDown={(event) => {
            event.currentTarget.setPointerCapture(event.pointerId);
            lastPoint.current = null;
            strokeTo(pointAt(event));
          }}
          onPointerMove={(event) => {
            if (lastPoint.current) strokeTo(pointAt(event));
          }}
          onPointerUp={() => (lastPoint.current = null)}
          onPointerCancel={() => (lastPoint.current = null)}
        />
      </MessageBox>

      <Keyboard state={keyboard} onKey={pressKey} onDrop={dropCharacter} inkStyle={inkStyle(penInk)} />

      <div className="actions" style={ACTION_BORDER}>
        <button type="button" className="plain action send" disabled={!ready} aria-label="Send" title="Send (Ctrl+Enter)" onMouseDown={keepFocus} onClick={send}>
          <PixelIcon art={ICONS.send} />
          <PixelIcon art={ICONS.sendLabel} />
        </button>
        <button type="button" className="plain action" disabled={!latest} aria-label="Copy the newest message" title="Copy the newest message" onMouseDown={keepFocus} onClick={copyLatest}>
          <PixelIcon art={ICONS.copy} />
        </button>
        <button type="button" className="plain action" aria-label="Clear" title="Clear" onMouseDown={keepFocus} onClick={clear}>
          <PixelIcon art={ICONS.clear} />
        </button>
      </div>
    </section>
  );
}
