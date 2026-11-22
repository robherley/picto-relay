import { describe, expect, it } from "vitest";
import { NAME_MAX_UNITS } from "../pictochat/constants";
import { STARTER_NAMES, starterName } from "./useProfile";

describe("starter names", () => {
  it("all fit a DS nickname, and none repeats", () => {
    for (const name of STARTER_NAMES) {
      expect(name.trim()).toBe(name);
      expect(name.length).toBeGreaterThan(0);
      expect(name.length).toBeLessThanOrEqual(NAME_MAX_UNITS);
    }
    expect(new Set(STARTER_NAMES).size).toBe(STARTER_NAMES.length);
  });

  it("are picked across the whole list", () => {
    expect(starterName(0)).toBe(STARTER_NAMES[0]);
    expect(starterName(0.999999)).toBe(STARTER_NAMES.at(-1));
    expect(STARTER_NAMES).toContain(starterName());
  });
});
