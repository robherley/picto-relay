import type { Canvas } from "./canvas.ts";

// The relay server's JSON API, as far as the bot uses it: join a room, send
// drawings, listen for what happens there. `docs/json-api.md` describes it.

/** Letters of the four PictoChat rooms, by room number. */
export const ROOM_LETTERS = ["A", "B", "C", "D"] as const;

/** Longest nickname a DS profile holds, in UTF-16 units. */
export const NAME_MAX_UNITS = 10;
/** Longest personal message a DS profile holds, in UTF-16 units. */
export const PROFILE_MESSAGE_MAX_UNITS = 26;

export interface Participant {
  client_id: number;
  name: string;
}

/** Something that happened in a session, selected by its `type`. */
export type SessionEvent =
  | { type: "session_ready"; client_id: number }
  | { type: "backend_state"; state: string; detail: string }
  | { type: "participants"; room: number; participants: Participant[] }
  | { type: "participant_joined"; room: number; participant: Participant }
  | { type: "participant_left"; room: number; participant: Participant }
  | {
      type: "profile";
      station_id: number;
      name: string;
      profile_message: string;
      profile_color: number;
    }
  | {
      type: "message";
      station_id: number;
      name: string;
      message_id: number;
      palette_bitmap: string;
    }
  | { type: "delivered"; message_id: number }
  | { type: "delivery_timed_out"; message_id: number }
  | { type: "error"; code: string; message: string };

export interface JoinRequest {
  name: string;
  room: number;
  profile_message: string;
}

/** The server said no. `code` is its stable error code, when it sent one. */
export class RelayError extends Error {
  readonly code: string | undefined;

  constructor(message: string, code?: string, options?: ErrorOptions) {
    super(message, options);
    this.code = code;
  }
}

export class Relay {
  private readonly api: URL;

  /** `server` is the relay's address, such as `http://127.0.0.1:3001`. */
  constructor(server: string) {
    this.api = new URL("/api/v1/", server);
  }

  private async request(method: string, path: string, body?: unknown): Promise<Response> {
    let response: Response;
    try {
      response = await fetch(new URL(path, this.api), {
        method,
        headers: body === undefined ? undefined : { "content-type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch (cause) {
      throw new RelayError(`Cannot reach the relay server at ${this.api.origin}`, undefined, {
        cause,
      });
    }
    if (response.ok) return response;
    const text = await response.text();
    let error: { code?: string; message?: string } = {};
    try {
      error = JSON.parse(text) as typeof error;
    } catch {
      // Axum rejects malformed requests in plain text.
    }
    throw new RelayError(error.message ?? (text || response.statusText), error.code);
  }

  /** Joins a room. Resolves with the session's id once the server has allocated it. */
  async join(request: JoinRequest): Promise<number> {
    const response = await this.request("POST", "sessions", request);
    return ((await response.json()) as { session_id: number }).session_id;
  }

  /** Queues a drawing. The session's `delivered` event confirms it arrived. */
  async send(session: number, drawing: Canvas): Promise<void> {
    await this.request("POST", `sessions/${session}/messages`, {
      palette_bitmap: Buffer.from(drawing.pack()).toString("base64"),
    });
  }

  async leave(session: number): Promise<void> {
    await this.request("DELETE", `sessions/${session}`);
  }

  /** The session's events as they happen, until the server closes the stream. */
  async *events(session: number): AsyncGenerator<SessionEvent> {
    const url = new URL(`sessions/${session}/events/ws`, this.api);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(url);

    const queue: SessionEvent[] = [];
    let closed = false;
    let wake = () => {};
    socket.onmessage = (message) => {
      queue.push(JSON.parse(String(message.data)) as SessionEvent);
      wake();
    };
    socket.onclose = () => {
      closed = true;
      wake();
    };
    try {
      for (;;) {
        while (queue.length > 0) yield queue.shift()!;
        if (closed) return;
        await new Promise<void>((resolve) => {
          wake = resolve;
        });
      }
    } finally {
      socket.close();
    }
  }
}
