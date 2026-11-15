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
const DEFAULT_PROFILE: Profile = { name: "Relay", message: "", color: 11 };

function load(): Profile {
  try {
    const stored = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null") as Partial<Profile> | null;
    return {
      name: stored?.name ?? DEFAULT_PROFILE.name,
      message: stored?.message ?? DEFAULT_PROFILE.message,
      color: stored?.color ?? DEFAULT_PROFILE.color,
    };
  } catch {
    return DEFAULT_PROFILE;
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
