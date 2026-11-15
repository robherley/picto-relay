import type { Gateway } from "../hooks/useGateway";
import { type Profile, profileTone } from "../hooks/useProfile";
import {
  NAME_MAX_UNITS, PROFILE_COLORS, PROFILE_MESSAGE_MAX_UNITS, ROOM_CAPACITY, ROOM_LETTERS,
} from "../pictochat/constants";
import { ICONS, PixelIcon } from "./PixelIcon";

interface Props {
  profile: Profile;
  onProfile: (change: Partial<Profile>) => void;
  gateway: Gateway;
  joining: boolean;
  onJoin: (room: number) => void;
}

function hint(gateway: Gateway, named: boolean, joining: boolean): string {
  if (!gateway.online) return "Connecting to the relay...";
  if (joining) return "Joining...";
  if (!named) return "Enter a nickname first.";
  if (gateway.backend === "esp32") return "Rooms are hosted by a DS.";
  if (gateway.backend === "memory") return "Test rooms: no radio attached.";
  return "";
}

/** Top screen: who you are. Bottom screen: the DS room list. */
export function Lobby({ profile, onProfile, gateway, joining, onJoin }: Props) {
  // The profile color themes the bars, as the favorite color does on a DS.
  const bar = { backgroundColor: profileTone(profile) };
  const named = profile.name.trim() !== "";
  const canJoin = gateway.online && !joining && named;

  return (
    <>
      <section className="screen ds-grid" aria-label="Your profile">
        <div className="bar title-bar" style={bar}>
          <h1>Picto Relay</h1>
          <span>{gateway.online ? (gateway.backend ?? "") : "offline"}</span>
        </div>
        <form className="profile" onSubmit={(event) => event.preventDefault()}>
          <label>
            <span>Nickname</span>
            <div className="input-wrapper">
              <input
                type="text"
                value={profile.name}
                maxLength={NAME_MAX_UNITS}
                autoComplete="nickname"
                spellCheck={false}
                onChange={(event) => onProfile({ name: event.target.value })}
              />
            </div>
          </label>
          <label>
            <span>Profile message</span>
            <div className="input-wrapper">
              <input
                type="text"
                className="input-lg"
                value={profile.message}
                maxLength={PROFILE_MESSAGE_MAX_UNITS}
                autoComplete="off"
                spellCheck={false}
                onChange={(event) => onProfile({ message: event.target.value })}
              />
            </div>
          </label>
          <fieldset className="swatches">
            <legend>Color</legend>
            <div>
              {PROFILE_COLORS.map((color, index) => (
                <button
                  key={color.name}
                  type="button"
                  className="plain swatch"
                  style={{ background: color.hex }}
                  aria-label={color.label}
                  aria-pressed={index === profile.color}
                  title={color.label}
                  onClick={() => onProfile({ color: index })}
                />
              ))}
            </div>
          </fieldset>
        </form>
      </section>

      <section className="screen rooms" aria-label="Chat rooms">
        <div className="bar title-bar centered" style={bar}>
          Choose a Chat Room to join.
        </div>
        <div className="room-list">
          {ROOM_LETTERS.map((letter, room) => {
            const summary = gateway.rooms.find((candidate) => candidate.room === room);
            const observed = summary?.observed ?? false;
            const names = summary?.participants.map((participant) => participant.name).join(", ");
            return (
              <button
                key={letter}
                type="button"
                className="plain room"
                disabled={!canJoin}
                title={names || undefined}
                onClick={() => onJoin(room)}
              >
                <span className="room-icon">{letter}</span>
                <span className="room-name">Chat Room {letter}</span>
                <span className="room-count">
                  <PixelIcon art={ICONS.person} />
                  {observed ? summary?.participant_count : "--"}/{ROOM_CAPACITY}
                </span>
                <PixelIcon
                  art={observed ? ICONS.signal : ICONS.noSignal}
                  label={observed ? "In range" : "Not found"}
                />
              </button>
            );
          })}
        </div>
        <div className="bar-lg-reverse action-bar" style={bar}>
          <span>{hint(gateway, named, joining)}</span>
        </div>
      </section>
    </>
  );
}
