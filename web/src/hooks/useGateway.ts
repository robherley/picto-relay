import { useEffect, useState } from "react";
import { getStatus, openStream, ROOMS_STREAM, type RoomsResponse, type RoomSummary } from "../api/client";

export interface Gateway {
  /** The server's radio backend, once known. */
  backend?: string;
  /** Whether the live room stream is connected. */
  online: boolean;
  rooms: RoomSummary[];
}

/** The server's backend and a live view of rooms A through D. */
export function useGateway(): Gateway {
  const [backend, setBackend] = useState<string>();
  const [online, setOnline] = useState(false);
  const [rooms, setRooms] = useState<RoomSummary[]>([]);

  useEffect(() => {
    if (!online) return;
    let current = true;
    void getStatus().then(({ data }) => {
      if (current && data) setBackend(data.backend);
    });
    return () => {
      current = false;
    };
  }, [online]);

  useEffect(
    () =>
      openStream<RoomsResponse>(ROOMS_STREAM, {
        onMessage: (response) => setRooms(response.rooms),
        onConnection: setOnline,
      }),
    [],
  );

  return { backend, online, rooms };
}
