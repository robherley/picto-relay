import { type Dispatch, useEffect, useReducer } from "react";
import type { Participant, SessionEvent } from "../api/client";
import { type Session, transport } from "../api/transport";
import { ROOM_LETTERS } from "../pictochat/constants";
import { fromBase64, type Pixels, unpack } from "../pictochat/drawing";

/** One line of the message log. */
export type LogEntry =
  | {
      id: number;
      kind: "message";
      stationId: number;
      name: string;
      pixels: Pixels;
      /** We sent it. `sending` until the room confirms it, `failed` if it never does. */
      own?: "sending" | "delivered" | "failed";
    }
  | { id: number; kind: "notice"; tone: "enter" | "leave" | "error"; text: string; name?: string };

/** What a station announced about itself. */
export interface StationProfile {
  name: string;
  /** The personal message from its DS settings. */
  message: string;
  /** DS profile color index. */
  color: number;
}

export interface SessionView {
  /** `joining` until the room accepts us; `lost` once the radio gives up. */
  phase: "joining" | "ready" | "lost";
  /** What the backend is doing right now, in its own words. */
  detail: string;
  /** Our own identity in rosters and messages, once the room accepts us. */
  stationId?: number;
  members: Participant[];
  /** The profile of each station that has announced one. */
  profiles: ReadonlyMap<number, StationProfile>;
  log: LogEntry[];
}

/** Log lines kept; older ones are dropped. */
const LOG_LIMIT = 200;

const initial: SessionView = {
  phase: "joining", detail: "", members: [], profiles: new Map(), log: [],
};

type Action =
  | { type: "event"; room: number; event: SessionEvent }
  | { type: "connection"; connected: boolean }
  /** We sent a drawing: show it now, as a DS does, rather than on delivery. */
  | { type: "sent"; name: string; pixels: Pixels }
  /** The server refused the drawing we just sent. */
  | { type: "rejected" }
  | { type: "reset" };

let nextEntryId = 0;
type NewEntry = LogEntry extends infer E ? (E extends LogEntry ? Omit<E, "id"> : never) : never;
function append(view: SessionView, entry: NewEntry): SessionView {
  const log = [...view.log, { ...entry, id: nextEntryId++ } as LogEntry];
  return { ...view, log: log.slice(-LOG_LIMIT) };
}

/** Settles the oldest of our messages still on its way to the room. */
function settle(view: SessionView, outcome: "delivered" | "failed"): SessionView | undefined {
  const index = view.log.findIndex((entry) => entry.kind === "message" && entry.own === "sending");
  if (index < 0) return undefined;
  const log = view.log.map((entry, i) => (i === index ? { ...entry, own: outcome } : entry));
  return { ...view, log };
}

function reduce(view: SessionView, action: Action): SessionView {
  if (action.type === "reset") return initial;
  if (action.type === "sent") {
    return append(view, {
      kind: "message", stationId: view.stationId ?? -1, name: action.name, pixels: action.pixels, own: "sending",
    });
  }
  if (action.type === "rejected") return settle(view, "failed") ?? view;
  if (action.type === "connection") {
    return action.connected ? view : { ...view, detail: "Reconnecting..." };
  }
  const { event, room } = action;
  const letter = ROOM_LETTERS[room];
  switch (event.type) {
    case "backend_state":
      switch (event.state) {
        case "ready":
          return { ...view, phase: "ready", detail: "" };
        case "connecting":
        case "reconnecting":
          return { ...view, phase: "joining", detail: event.detail };
        case "left":
          return { ...view, phase: "lost", detail: event.detail };
        default:
          // Upload and radio progress: the message is already in the log.
          return view;
      }
    case "session_ready":
      return { ...view, phase: "ready", detail: "", stationId: event.client_id };
    case "participants":
      return event.room === room ? { ...view, members: event.participants } : view;
    case "participant_joined":
      if (event.room !== room) return view;
      return append(view, {
        kind: "notice", tone: "enter", text: `Now entering [${letter}]: `, name: event.participant.name,
      });
    case "participant_left":
      if (event.room !== room) return view;
      return append(view, {
        kind: "notice", tone: "leave", text: `Now leaving [${letter}]: `, name: event.participant.name,
      });
    case "profile":
      return {
        ...view,
        profiles: new Map(view.profiles).set(event.station_id, {
          name: event.name, message: event.profile_message, color: event.profile_color,
        }),
      };
    case "message":
      // Our own drawing coming back confirms the copy already in the log.
      if (event.station_id === view.stationId) {
        const settled = settle(view, "delivered");
        if (settled) return settled;
      }
      try {
        const pixels = unpack(fromBase64(event.palette_bitmap));
        return append(view, { kind: "message", stationId: event.station_id, name: event.name, pixels });
      } catch {
        return append(view, { kind: "notice", tone: "error", text: "Received a drawing that could not be read." });
      }
    case "delivered":
      return view;
    case "delivery_timed_out":
      return append(settle(view, "failed") ?? view, {
        kind: "notice", tone: "error", text: "Your message was not delivered.",
      });
    case "error":
      return append(view, { kind: "notice", tone: "error", text: event.message });
  }
}

/** Follows a session's event stream and folds it into what the chat shows. */
export function useSession(session: Session | null): [SessionView, Dispatch<Action>] {
  const [view, dispatch] = useReducer(reduce, initial);
  const id = session?.id;
  const room = session?.room;

  useEffect(() => {
    if (id === undefined || room === undefined) return;
    dispatch({ type: "reset" });
    return transport.watchSession({ id, room }, {
      onMessage: (event) => dispatch({ type: "event", room, event }),
      onConnection: (connected) => dispatch({ type: "connection", connected }),
    });
  }, [id, room]);

  return [view, dispatch];
}
