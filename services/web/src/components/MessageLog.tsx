import { type ReactNode, type Ref, useEffect, useRef } from "react";
import type { LogEntry } from "../hooks/useSession";
import { BAND_HEIGHT, BOX_WIDTH, ROOM_LETTERS } from "../pictochat/constants";
import { type Pixels, usedHeight } from "../pictochat/drawing";
import { MessageBox } from "./MessageBox";
import { paintInk } from "./paint";
import { ICONS, PixelIcon } from "./PixelIcon";

interface Props {
  room: number;
  log: LogEntry[];
  /** The color to show a message in, as `#rrggbb`. */
  toneOf: (entry: LogEntry & { kind: "message" }) => string;
  connected: boolean;
  scroller: Ref<HTMLDivElement>;
  /** Shown over the log, for example while joining. */
  children?: ReactNode;
}

/** Log entries the side strip marks, newest at the bottom. */
const TICKS = 40;
const NOTICE_TONE = "#494949";

function Drawing({ pixels, height }: { pixels: Pixels; height: number }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    if (canvas.current) paintInk(canvas.current, pixels, height);
  }, [pixels, height]);
  return <canvas ref={canvas} className="ink" width={BOX_WIDTH} height={height} />;
}

/** The top screen: everything said in the room, newest at the bottom. */
export function MessageLog({ room, log, toneOf, connected, scroller, children }: Props) {
  const bottom = useRef<HTMLDivElement>(null);
  useEffect(() => {
    bottom.current?.scrollIntoView({ block: "end" });
  }, [log.length]);

  const message = (entry: LogEntry & { kind: "message" }) => {
    const height = usedHeight(entry.pixels);
    // A one-line message is exactly as tall as its name tag.
    const shape = height === BAND_HEIGHT ? "single" : "";
    const state = entry.own === "sending" || entry.own === "failed" ? entry.own : "";
    return (
      <MessageBox key={entry.id} name={entry.name} tone={toneOf(entry)} className={`${shape} ${state}`}>
        <Drawing pixels={entry.pixels} height={height} />
      </MessageBox>
    );
  };

  return (
    <section className="screen log" aria-label={`Chat Room ${ROOM_LETTERS[room]}`}>
      <div className="log-strip">
        <PixelIcon
          art={connected ? ICONS.signal : ICONS.noSignal}
          label={connected ? "Connected" : "Not connected"}
        />
        <div className="ticks" aria-hidden>
          {log.slice(-TICKS).map((entry) => (
            <i key={entry.id} style={{ background: entry.kind === "message" ? toneOf(entry) : NOTICE_TONE }} />
          ))}
        </div>
        <span className="room-letter">{ROOM_LETTERS[room]}</span>
      </div>
      <div className="log-scroll" ref={scroller} role="log" aria-live="polite">
        {log.map((entry) =>
          entry.kind === "message" ? (
            message(entry)
          ) : (
            <div key={entry.id} className={`pictochat-status notice ${entry.tone}`}>
              {entry.text}
              {entry.name && <b>{entry.name}</b>}
            </div>
          ),
        )}
        <div ref={bottom} />
      </div>
      {children}
    </section>
  );
}
