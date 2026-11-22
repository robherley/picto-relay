import { useEffect, useState } from "react";
import type { RoomSummary } from "../api/client";
import { transport } from "../api/transport";

export interface Gateway {
  /** What is at the far end, once known: `esp32`, `memory` or `usb`. */
  backend?: string;
  /** Whether the live room stream is connected. */
  online: boolean;
  rooms: RoomSummary[];
}

/** What is at the far end, and a live view of rooms A through D. */
export function useGateway(): Gateway {
  const [backend, setBackend] = useState<string>();
  const [online, setOnline] = useState(false);
  const [rooms, setRooms] = useState<RoomSummary[]>([]);

  useEffect(() => {
    if (!online) return;
    let current = true;
    void transport.backend().then((name) => {
      if (current && name) setBackend(name);
    });
    return () => {
      current = false;
    };
  }, [online]);

  useEffect(
    () =>
      transport.watchRooms({
        onMessage: (response) => setRooms(response.rooms),
        onConnection: setOnline,
      }),
    [],
  );

  return { backend, online, rooms };
}
