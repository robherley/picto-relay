import { pack, toBase64 } from "../pictochat/drawing";
import {
  createSession, deleteSession, errorText, getStatus, openStream, ROOMS_STREAM, sendMessage, sessionStream,
} from "./client";
import type { Transport } from "./transport";

/** Rooms reached through the relay server. */
export const relay: Transport = {
  async backend() {
    return (await getStatus()).data?.backend;
  },

  watchRooms: (handlers) => openStream(ROOMS_STREAM, handlers),

  async join(profile, room) {
    const { data, error } = await createSession({
      body: {
        name: profile.name.trim(),
        room,
        profile_message: profile.message,
        profile_color: profile.color,
      },
    });
    return data ? { id: data.session_id, room: data.room } : errorText(error);
  },

  watchSession: (session, handlers) => openStream(sessionStream(session.id), handlers),

  async send(session, pixels) {
    const { error } = await sendMessage({
      path: { session_id: session.id },
      body: { palette_bitmap: toBase64(pack(pixels)) },
    });
    return error ? errorText(error) : undefined;
  },

  leave(session) {
    void deleteSession({ path: { session_id: session.id } });
  },

  // Closing the tab should free the room slot (and, with a radio, the radio).
  abandon(session) {
    void fetch(`/api/v1/sessions/${session.id}`, { method: "DELETE", keepalive: true });
  },
};
