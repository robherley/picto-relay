import { useRef, useState } from "react";
import { errorText, sendMessage } from "../api/client";
import { type Profile, profileTone } from "../hooks/useProfile";
import { useSession } from "../hooks/useSession";
import { profileColor, ROOM_LETTERS } from "../pictochat/constants";
import { pack, type Pixels, toBase64 } from "../pictochat/drawing";
import { Composer } from "./Composer";
import { Loading } from "./Loading";
import { MessageLog } from "./MessageLog";

interface Props {
  session: { id: number; room: number };
  profile: Profile;
  onLeave: () => void;
}

/** Pixels one press of a scroll button moves the log: about one message. */
const SCROLL_STEP = 176;

export function Chat({ session, profile, onLeave }: Props) {
  const [view, dispatch] = useSession(session);
  const scroller = useRef<HTMLDivElement>(null);
  const [problem, setProblem] = useState<string>();

  // A DS announces its color in its profile; our own is known from the start.
  const ownTone = profileTone(profile);
  const toneOf = (stationId: number) =>
    stationId === view.stationId ? ownTone : profileColor(view.colors.get(stationId)).hex;

  const send = async (pixels: Pixels) => {
    dispatch({ type: "sent", name: profile.name, pixels });
    const { error } = await sendMessage({
      path: { session_id: session.id },
      body: { palette_bitmap: toBase64(pack(pixels)) },
    });
    if (error) {
      dispatch({ type: "rejected" });
      setProblem(errorText(error));
    }
  };

  const newest = view.log.findLast((entry) => entry.kind === "message");
  const letter = ROOM_LETTERS[session.room];

  return (
    <>
      <MessageLog
        room={session.room}
        log={view.log}
        toneOf={(entry) => (entry.own ? ownTone : toneOf(entry.stationId))}
        connected={view.phase === "ready"}
        scroller={scroller}
      >
        {problem ? (
          <div className="overlay">
            <div className="alert dialog">
              <span>{problem}</span>
              <button type="button" onClick={() => setProblem(undefined)}>
                OK
              </button>
            </div>
          </div>
        ) : view.phase === "joining" ? (
          <div className="overlay">
            <div className="alert">
              <Loading />
              <span>
                Joining Chat Room {letter}...
                <small>{view.detail}</small>
              </span>
              <Loading />
            </div>
          </div>
        ) : view.phase === "lost" ? (
          <div className="overlay">
            <div className="alert dialog">
              <span>
                Left Chat Room {letter}.<small>{view.detail}</small>
              </span>
              <button type="button" onClick={onLeave}>
                OK
              </button>
            </div>
          </div>
        ) : null}
      </MessageLog>
      <Composer
        name={profile.name}
        tone={ownTone}
        members={view.members}
        toneOf={toneOf}
        ready={view.phase === "ready"}
        latest={newest?.kind === "message" ? newest.pixels : undefined}
        onSend={(pixels) => void send(pixels)}
        onLeave={onLeave}
        onScroll={(direction) => scroller.current?.scrollBy({ top: direction * SCROLL_STEP })}
      />
    </>
  );
}
