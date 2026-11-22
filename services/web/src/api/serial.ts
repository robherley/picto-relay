import { pack } from "../pictochat/drawing";
import type { DeviceState, FromWorker, ToWorker } from "../serial/messages";
import type { RoomsResponse, SessionEvent, StreamHandlers } from "./client";
import type { Device, Session, Transport } from "./transport";

// Rooms reached with no server at all: the page drives the ESP32 itself, over
// Web Serial. A worker owns the port (../serial/worker.ts) and does there what
// the relay server's worker does; this is the page's end of it.

export type { DeviceState };

/** Being in a room, and what happened there before anyone was watching. */
interface Stay {
  session: Session;
  watcher?: StreamHandlers<SessionEvent>;
  backlog: SessionEvent[];
  /** Who the room was last said to hold. */
  members: { client_id: number; name: string }[];
}

export function serial(): Transport {
  const supported = "serial" in navigator;
  let state: DeviceState = { phase: "disconnected" };
  let counts: (number | null)[] = [null, null, null, null];
  let stay: Stay | undefined;
  let nextSession = 1;
  let nextDrawing = 1;
  const roomWatchers = new Set<StreamHandlers<RoomsResponse>>();
  const stateWatchers = new Set<(state: DeviceState) => void>();
  /** Things waiting for the worker to let go of the port. */
  const closing: (() => void)[] = [];

  const worker = supported
    ? new Worker(new URL("../serial/worker.ts", import.meta.url), { type: "module" })
    : undefined;
  const tell = (message: ToWorker) => worker?.postMessage(message);

  /** The room list: what the last scan saw, and in the room we are in, who is there. */
  const rooms = (): RoomsResponse => ({
    rooms: counts.map((count, room) => {
      const here = stay?.session.room === room ? stay.members : undefined;
      return {
        room,
        observed: here ? true : count !== null,
        participant_count: here ? here.length : (count ?? 0),
        participants: here ?? [],
      };
    }),
  });
  const publishRooms = () => {
    for (const watcher of roomWatchers) watcher.onMessage(rooms());
  };

  const emit = (event: SessionEvent) => {
    if (!stay) return;
    if (event.type === "participants") {
      stay.members = event.participants;
      publishRooms();
    }
    if (stay.watcher) stay.watcher.onMessage(event);
    else stay.backlog.push(event);
  };

  if (worker) {
    worker.onmessage = ({ data }: MessageEvent<FromWorker>) => {
      switch (data.type) {
        case "state": {
          const was = state.phase === "connected";
          state = data.state;
          const now = state.phase === "connected";
          for (const watcher of stateWatchers) watcher(state);
          if (was === now) break;
          for (const watcher of roomWatchers) watcher.onConnection?.(now);
          if (now) break;
          // Without the ESP32 there are no rooms, and nobody is in one.
          counts = counts.map(() => null);
          publishRooms();
          emit({ type: "backend_state", state: "left", detail: "The ESP32 is not connected." });
          break;
        }
        case "closed":
          for (const done of closing.splice(0)) done();
          break;
        case "rooms":
          counts = data.counts;
          publishRooms();
          break;
        case "event":
          emit(data.event);
          break;
      }
    };
    // A port the user allowed on an earlier visit needs no asking again.
    tell({ type: "open" });
    navigator.serial?.addEventListener("connect", () => {
      if (state.phase === "disconnected") tell({ type: "open" });
    });
  }

  const device: Device = {
    supported,
    watch(listener) {
      stateWatchers.add(listener);
      listener(state);
      return () => stateWatchers.delete(listener);
    },
    async connect() {
      const port = await navigator.serial!.requestPort();
      tell({ type: "open", port: port.getInfo() });
    },
    release() {
      return new Promise((resolve) => {
        closing.push(resolve);
        tell({ type: "close" });
      });
    },
    reopen(port) {
      tell({ type: "open", port: port.getInfo() });
    },
  };

  return {
    device,
    backend: async () => "usb",

    watchRooms(handlers) {
      roomWatchers.add(handlers);
      handlers.onConnection?.(state.phase === "connected");
      handlers.onMessage(rooms());
      return () => roomWatchers.delete(handlers);
    },

    async join(profile, room) {
      if (state.phase !== "connected") return "Connect the ESP32 first.";
      stay = { session: { id: nextSession++, room }, backlog: [], members: [] };
      tell({ type: "join", room, profile });
      return stay.session;
    },

    watchSession(session, handlers) {
      const current = stay;
      if (current?.session.id !== session.id) return () => {};
      // Deferred so a mount that is immediately undone (React StrictMode) does
      // not take the backlog with it.
      const timer = window.setTimeout(() => {
        current.watcher = handlers;
        for (const event of current.backlog.splice(0)) handlers.onMessage(event);
      }, 0);
      return () => {
        window.clearTimeout(timer);
        if (current.watcher === handlers) current.watcher = undefined;
      };
    },

    async send(session, pixels) {
      if (stay?.session.id !== session.id || state.phase !== "connected") {
        return "The ESP32 is not connected.";
      }
      // Delivery, or the lack of it, is reported in the session's events.
      tell({ type: "send", id: nextDrawing++, bitmap: pack(pixels) });
      return undefined;
    },

    leave(session) {
      if (stay?.session.id !== session.id) return;
      stay = undefined;
      tell({ type: "leave" });
      publishRooms();
    },

    // The worker goes with the page; the firmware leaves the room by itself
    // when it stops hearing from a host.
    abandon(session) {
      this.leave(session);
    },
  };
}
