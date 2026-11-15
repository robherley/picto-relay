import { useLayoutEffect, useState } from "react";
import { DEFAULT_DISPLAY, type Display, type DisplaySettings, resolveDisplay } from "./display";

const STORAGE_KEY = "picto-relay.display";

function load(): DisplaySettings {
  try {
    const stored = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null") as Partial<DisplaySettings> | null;
    return { ...DEFAULT_DISPLAY, ...stored };
  } catch {
    return DEFAULT_DISPLAY;
  }
}

function measure(settings: DisplaySettings): Display {
  // Not `window.innerWidth`: on a phone that grows to fit the console itself,
  // which is wider than the screen until it is zoomed.
  const { clientWidth, clientHeight } = document.documentElement;
  return resolveDisplay(settings, { width: clientWidth, height: clientHeight });
}

/**
 * The display settings, remembered between visits, and the arrangement and
 * zoom they come to in this window. Keeps the page's layout in step.
 */
export function useDisplay() {
  const [settings, setSettings] = useState(load);
  const [display, setDisplay] = useState(() => measure(settings));

  useLayoutEffect(() => {
    const apply = () => {
      const next = measure(settings);
      const root = document.documentElement;
      root.dataset.layout = next.layout;
      root.style.setProperty("--zoom", String(next.zoom));
      root.style.setProperty("--gap", `${settings.gap}px`);
      setDisplay(next);
    };
    apply();
    window.addEventListener("resize", apply);
    return () => window.removeEventListener("resize", apply);
  }, [settings]);

  const change = (update: Partial<DisplaySettings>) => {
    const next = { ...settings, ...update };
    setSettings(next);
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
    } catch {
      // Private browsing: the settings simply last for this visit.
    }
  };

  return { settings, display, change };
}
