import type { Profile } from "../hooks/useProfile";
import type { Pixels } from "../pictochat/drawing";
import type { DeviceState } from "../serial/messages";
import type { RoomsResponse, SessionEvent, StreamHandlers } from "./client";
import { relay } from "./relay";

// The web client reaches rooms in one of two ways, chosen when it is built:
//
// - `relay`: the relay server's REST API and WebSockets (docs/json-api.md).
//   The server owns the ESP32, or stands in for it.
// - `serial` (`--mode serial`): no server. The page owns the ESP32 itself,
//   over Web Serial.
//
// Everything else in the client sees only this interface.

/** Being in a room. `id` tells one stay from the next. */
export interface Session {
  id: number;
  room: number;
}

/** The ESP32, where the page is what drives it. */
export interface Device {
  /** Whether this browser can reach a serial port at all. */
  supported: boolean;
  /** Follows where things stand with the ESP32. Returns a function that stops. */
  watch(listener: (state: DeviceState) => void): () => void;
  /** Asks the user which port the ESP32 is on, and opens it. Call from a click. */
  connect(): Promise<void>;
  /** Lets go of the port, for the flasher to use it. */
  release(): Promise<void>;
  /** Opens a port again, after the flasher is done with it. */
  reopen(port: SerialPort): void;
}

export interface Transport {
  /** The ESP32, if the page drives it. With the relay server there is nothing to connect. */
  device?: Device;
  /** What is at the far end, for the lobby to show: `esp32`, `memory` or `usb`. */
  backend(): Promise<string | undefined>;
  /** Follows the room list. Returns a function that stops. */
  watchRooms(handlers: StreamHandlers<RoomsResponse>): () => void;
  /** Joins a room. Resolves to the session, or to why there is none. */
  join(profile: Profile, room: number): Promise<Session | string>;
  /** Follows what happens in the session's room. Returns a function that stops. */
  watchSession(session: Session, handlers: StreamHandlers<SessionEvent>): () => void;
  /** Sends a drawing. Resolves to why it was not accepted, if it was not. */
  send(session: Session, pixels: Pixels): Promise<string | undefined>;
  leave(session: Session): void;
  /** Leaves as the page goes away, when nothing can be waited for. */
  abandon(session: Session): void;
}

export const transport: Transport =
  import.meta.env.VITE_TRANSPORT === "serial" ? (await import("./serial")).serial() : relay;
