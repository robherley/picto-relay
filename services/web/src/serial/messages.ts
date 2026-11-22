import type { SessionEvent } from "../api/client";
import type { Profile } from "../hooks/useProfile";

// What the page and the worker that drives the ESP32 say to each other.

/** Where things stand with the ESP32. */
export type DeviceState =
  /** No port is open. `problem` says why, when there was one to open. */
  | { phase: "disconnected"; problem?: string }
  | { phase: "connecting" }
  /** The firmware answered. `build` is its revision. */
  | { phase: "connected"; build: string }
  /** A port is open but nothing on it speaks our protocol: no firmware, or another device. */
  | { phase: "silent" }
  /** The firmware speaks another version of the protocol. */
  | { phase: "outdated"; detail: string };

export type ToWorker =
  /** Open a port the user has allowed: the one described, or any. */
  | { type: "open"; port?: SerialPortInfo }
  /** Let go of the port, for something else to use it. */
  | { type: "close" }
  | { type: "join"; room: number; profile: Profile }
  | { type: "leave" }
  | { type: "send"; id: number; bitmap: Uint8Array };

export type FromWorker =
  | { type: "state"; state: DeviceState }
  /** The port has been let go of, in answer to `close`. */
  | { type: "closed" }
  | { type: "rooms"; counts: (number | null)[] }
  | { type: "event"; event: SessionEvent };
