import { useEffect, useState } from "react";
import { profileColor } from "../pictochat/constants";

/** How this browser introduces itself to a room. */
export interface Profile {
  name: string;
  message: string;
  /** DS profile color index, 0 through 15. */
  color: number;
}

/** A profile's color as `#rrggbb`. */
export function profileTone(profile: Profile): string {
  return profileColor(profile.color).hex;
}

const STORAGE_KEY = "picto-relay.profile";

/**
 * Nicknames for someone who has not chosen one: characters from Nintendo's
 * games, each short enough for a DS profile.
 */
export const STARTER_NAMES = [
  "Mario", "Luigi", "Toad", "Peach", "Yoshi", "Bowser", "Daisy", "Wario", "Waluigi", "Rosalina",
  "Link", "Zelda", "Samus", "Kirby", "Pikachu", "Isabelle", "Tom Nook", "Olimar", "Ness", "Fox",
];

/** One of the starter names. `random` is a number from 0 up to, not including, 1. */
export function starterName(random = Math.random()): string {
  return STARTER_NAMES[Math.floor(random * STARTER_NAMES.length)] ?? STARTER_NAMES[0]!;
}

/** The profile of someone who has not been here before. */
function fresh(): Profile {
  return { name: starterName(), message: "", color: 11 };
}

function load(): Profile {
  const starting = fresh();
  try {
    const stored = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null") as Partial<Profile> | null;
    return {
      // A name left blank was never chosen either.
      name: stored?.name?.trim() ? stored.name : starting.name,
      message: stored?.message ?? starting.message,
      color: stored?.color ?? starting.color,
    };
  } catch {
    return starting;
  }
}

/** The profile, remembered between visits. */
export function useProfile(): [Profile, (change: Partial<Profile>) => void] {
  const [profile, setProfile] = useState(load);
  useEffect(() => {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(profile));
    } catch {
      // Private browsing: the profile simply lasts for this visit.
    }
  }, [profile]);
  return [profile, (change) => setProfile((current) => ({ ...current, ...change }))];
}
