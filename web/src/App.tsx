import { type CSSProperties, useEffect, useState } from "react";
import { createSession, deleteSession, errorText } from "./api/client";
import { Chat } from "./components/Chat";
import { DisplayControls } from "./components/DisplayControls";
import { Lobby } from "./components/Lobby";
import { useGateway } from "./hooks/useGateway";
import { profileTone, useProfile } from "./hooks/useProfile";
import { useDisplay } from "./hooks/useDisplay";
import { fontReady } from "./pictochat/glyphs";

interface Session {
  id: number;
  room: number;
}

export function App() {
  const { settings, display, change } = useDisplay();
  const gateway = useGateway();
  const [profile, setProfile] = useProfile();
  const [session, setSession] = useState<Session | null>(null);
  const [joining, setJoining] = useState(false);
  const [problem, setProblem] = useState<string>();
  const [fontLoaded, setFontLoaded] = useState(false);
  const chatting = session !== null && fontLoaded;

  useEffect(() => {
    void fontReady().then(() => setFontLoaded(true));
  }, []);

  // Closing the tab should free the room slot (and, with a radio, the radio).
  const sessionId = session?.id;
  useEffect(() => {
    if (sessionId === undefined) return;
    const release = () => {
      void fetch(`/api/v1/sessions/${sessionId}`, { method: "DELETE", keepalive: true });
    };
    window.addEventListener("pagehide", release);
    return () => window.removeEventListener("pagehide", release);
  }, [sessionId]);

  const join = async (room: number) => {
    setJoining(true);
    const { data, error } = await createSession({
      body: {
        name: profile.name.trim(),
        room,
        profile_message: profile.message,
        profile_color: profile.color,
      },
    });
    setJoining(false);
    if (data) setSession({ id: data.session_id, room: data.room });
    else setProblem(errorText(error));
  };

  const leave = () => {
    if (session) void deleteSession({ path: { session_id: session.id } });
    setSession(null);
  };

  return (
    <>
      <DisplayControls settings={settings} display={display} onChange={change} />
      <main className="console" style={{ "--tone": profileTone(profile) } as CSSProperties}>
        {chatting ? (
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
