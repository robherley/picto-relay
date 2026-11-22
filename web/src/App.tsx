import { type CSSProperties, useEffect, useState } from "react";
import { type Session, transport } from "./api/transport";
import { Chat } from "./components/Chat";
import { DisplayControls } from "./components/DisplayControls";
import { Lobby } from "./components/Lobby";
import { useGateway } from "./hooks/useGateway";
import { profileTone, useProfile } from "./hooks/useProfile";
import { useDisplay } from "./hooks/useDisplay";

export function App() {
  const { settings, display, change } = useDisplay();
  const gateway = useGateway();
  const [profile, setProfile] = useProfile();
  const [session, setSession] = useState<Session | null>(null);
  const [joining, setJoining] = useState(false);
  const [problem, setProblem] = useState<string>();

  // Closing the tab should free the place in the room.
  useEffect(() => {
    if (!session) return;
    const release = () => transport.abandon(session);
    window.addEventListener("pagehide", release);
    return () => window.removeEventListener("pagehide", release);
  }, [session]);

  const join = async (room: number) => {
    setJoining(true);
    const joined = await transport.join(profile, room);
    setJoining(false);
    if (typeof joined === "string") setProblem(joined);
    else setSession(joined);
  };

  const leave = () => {
    if (session) transport.leave(session);
    setSession(null);
  };

  return (
    <>
      <DisplayControls settings={settings} display={display} onChange={change} />
      <main className="console" style={{ "--tone": profileTone(profile) } as CSSProperties}>
        {session ? (
          <Chat key={session.id} session={session} profile={profile} onLeave={leave} />
        ) : (
          <Lobby
            profile={profile}
            onProfile={setProfile}
            gateway={gateway}
            joining={joining}
            onJoin={(room) => void join(room)}
          />
        )}
        {problem && (
          <div className="overlay whole">
            <div className="alert dialog">
              <span>{problem}</span>
              <button type="button" onClick={() => setProblem(undefined)}>
                OK
              </button>
            </div>
          </div>
        )}
      </main>
    </>
  );
}
