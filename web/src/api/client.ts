// The REST client is generated from docs/openapi.json (`npm run generate`).
// OpenAPI cannot describe WebSocket frames, so the two streams are opened
// here and typed with the generated schemas.

import type { ErrorResponse } from "./generated";

export * from "./generated";

export const ROOMS_STREAM = "/api/v1/rooms/ws";
export const sessionStream = (sessionId: number) => `/api/v1/sessions/${sessionId}/events/ws`;

/** Delay before reopening a stream that closed. */
const RECONNECT_MS = 1000;

interface StreamHandlers<T> {
  onMessage: (value: T) => void;
  /** Called with `false` when the stream drops and `true` when it is back. */
  onConnection?: (connected: boolean) => void;
}

/**
 * Opens a WebSocket of JSON text frames and keeps it open, reconnecting after
 * a drop. Returns a function that closes it for good.
 */
export function openStream<T>(path: string, handlers: StreamHandlers<T>): () => void {
  let socket: WebSocket | undefined;
  let timer: number | undefined;
  let closed = false;

  const connect = () => {
    const url = new URL(path, window.location.href);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    socket = new WebSocket(url);
    socket.onopen = () => handlers.onConnection?.(true);
    socket.onmessage = (event) => handlers.onMessage(JSON.parse(event.data as string) as T);
    socket.onclose = () => {
      if (closed) return;
      handlers.onConnection?.(false);
      timer = window.setTimeout(connect, RECONNECT_MS);
    };
  };
  // Deferred so a mount that is immediately undone (React StrictMode) never
  // opens a socket: a session's queued events go to whichever socket attaches
  // first and would be lost with it.
  timer = window.setTimeout(connect, 0);

  return () => {
    closed = true;
    window.clearTimeout(timer);
    socket?.close(1000);
  };
}

/** The server's explanation of a failed request, or a generic one. */
export function errorText(error: unknown): string {
  const message = (error as Partial<ErrorResponse> | undefined)?.message;
  return typeof message === "string" ? message : "The relay server did not respond.";
}
