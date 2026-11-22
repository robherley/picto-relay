import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { type Device, transport } from "../api/transport";
import { useDevice } from "../hooks/useDevice";
import type { Gateway } from "../hooks/useGateway";
import { type Profile, profileTone } from "../hooks/useProfile";
import {
  NAME_MAX_UNITS, PROFILE_COLORS, PROFILE_MESSAGE_MAX_UNITS, ROOM_CAPACITY, ROOM_LETTERS,
} from "../pictochat/constants";
import { type Firmware, firmwareOnOffer, flash } from "../serial/flash";
import type { DeviceState } from "../serial/messages";
import { Loading } from "./Loading";
import { ICONS, PixelIcon, ROOM_ICONS } from "./PixelIcon";

interface Props {
  profile: Profile;
  onProfile: (change: Partial<Profile>) => void;
  gateway: Gateway;
  joining: boolean;
  onJoin: (room: number) => void;
}

function hint(gateway: Gateway, named: boolean, joining: boolean, device?: DeviceState): string {
  if (transport.device && !transport.device.supported) return "Needs Chrome or Edge on a computer.";
  switch (device?.phase) {
    case "disconnected":
      return "Plug in the ESP32.";
    case "connecting":
      return "Connecting to the ESP32...";
    case "silent":
      return "The ESP32 did not answer.";
    case "outdated":
      return "The firmware is out of date.";
  }
  if (!gateway.online) return "Connecting to the relay...";
  if (joining) return "Joining...";
  if (!named) return "Enter a nickname first.";
  if (gateway.backend === "esp32" || gateway.backend === "usb") return "Rooms are hosted by a DS.";
  if (gateway.backend === "memory") return "Test rooms: no radio attached.";
  return "";
}

/** The browser's own word that the user closed its list of ports without picking one. */
const cancelled = (error: unknown) => error instanceof DOMException && error.name === "NotFoundError";
const reason = (error: unknown) => (error instanceof Error ? error.message : String(error));

/**
 * Connecting and flashing the ESP32, where the page is what drives it: the
 * buttons for the bottom bar, and the dialogs they lead to, which cover `screen`.
 */
function DeviceControls({ device, state, screen }: { device: Device; state: DeviceState; screen: HTMLElement }) {
  const [firmware, setFirmware] = useState<Firmware>();
  /** Flashing: not asked for, waiting for a yes, or how far along it is. */
  const [flashing, setFlashing] = useState<"confirm" | number>();
  const [problem, setProblem] = useState<string>();

  useEffect(() => {
    void firmwareOnOffer().then(setFirmware);
  }, []);
  // A port that would not open is worth a sentence; a bar has room for a few words.
  const trouble = state.phase === "disconnected" ? state.problem : undefined;
  useEffect(() => setProblem(trouble), [trouble]);

  const connect = async () => {
    try {
      await device.connect();
    } catch (error) {
      if (!cancelled(error)) setProblem(reason(error));
    }
  };
  const write = async (image: Firmware) => {
    setFlashing(0);
    try {
      await flash(device, image, setFlashing);
    } catch (error) {
      if (!cancelled(error)) setProblem(`The firmware was not flashed: ${reason(error)}`);
    }
    setFlashing(undefined);
  };

  const idle = state.phase === "disconnected" || state.phase === "silent";
  const dialog = problem ? (
    <div className="alert dialog">
      <span>{problem}</span>
      <button type="button" onClick={() => setProblem(undefined)}>
        OK
      </button>
    </div>
  ) : flashing === "confirm" && firmware ? (
    <div className="alert dialog">
      <span>
        Flash Picto Relay onto the ESP32?
        <small>Build {firmware.build}. It replaces what is on the board.</small>
      </span>
      <span className="device-buttons">
        <button type="button" onClick={() => void write(firmware)}>
          Flash
        </button>
        <button type="button" onClick={() => setFlashing(undefined)}>
          Cancel
        </button>
      </span>
    </div>
  ) : typeof flashing === "number" ? (
    <div className="alert">
      <Loading />
      <span>
        Flashing the ESP32... {Math.round(flashing * 100)}%<small>Keep it plugged in.</small>
      </span>
      <Loading />
    </div>
  ) : null;
  return (
    <>
      <span className="device-buttons">
        {firmware && (idle || state.phase === "outdated") && (
          <button type="button" onClick={() => setFlashing("confirm")}>
            Flash
          </button>
        )}
        {idle && (
          <button type="button" onClick={() => void connect()}>
            Connect
          </button>
        )}
      </span>
      {dialog && createPortal(<div className="overlay">{dialog}</div>, screen)}
    </>
  );
}

/** Top screen: who you are. Bottom screen: the DS room list. */
export function Lobby({ profile, onProfile, gateway, joining, onJoin }: Props) {
  // The profile color themes the bars, as the favorite color does on a DS.
  const bar = { backgroundColor: profileTone(profile) };
  const named = profile.name.trim() !== "";
  const canJoin = gateway.online && !joining && named;
  const device = useDevice();
  /** The bottom screen, which the device's dialogs cover. */
  const [roomsScreen, setRoomsScreen] = useState<HTMLElement | null>(null);

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

      <section className="screen rooms" aria-label="Chat rooms" ref={setRoomsScreen}>
        <div className="bar title-bar centered" style={bar}>
          Choose a Chat Room to join.
        </div>
        <div className="room-list">
          {ROOM_LETTERS.map((letter, room) => {
            const summary = gateway.rooms.find((candidate) => candidate.room === room);
            const observed = summary?.observed ?? false;
            // Someone is in it: on a DS, the rooms worth joining are lit.
            const live = observed && (summary?.participant_count ?? 0) > 0;
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
                <span className="room-icon">
                  <PixelIcon art={ROOM_ICONS[room]!} />
                </span>
                <span className="room-name">Chat Room {letter}</span>
                <span className="room-count">
                  <PixelIcon art={ICONS.person} />
                  {observed ? summary?.participant_count : "--"}/{ROOM_CAPACITY}
                </span>
                <span className={live ? "room-signal live" : "room-signal"}>
                  <PixelIcon
                    art={observed ? ICONS.signal : ICONS.noSignal}
                    label={live ? "In use" : observed ? "In range" : "Not found"}
                  />
                </span>
              </button>
            );
          })}
        </div>
        <div className="bar-lg-reverse action-bar" style={bar}>
          <span>{hint(gateway, named, joining, device)}</span>
          {transport.device?.supported && device && roomsScreen && (
            <DeviceControls device={transport.device} state={device} screen={roomsScreen} />
          )}
        </div>
      </section>
    </>
  );
}
