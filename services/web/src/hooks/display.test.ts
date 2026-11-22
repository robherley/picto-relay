import { describe, expect, it } from "vitest";
import { DEFAULT_DISPLAY, resolveDisplay } from "./display";

const laptop = { width: 1440, height: 760 };
const tall = { width: 900, height: 1300 };
const phone = { width: 375, height: 770 };

describe("resolveDisplay", () => {
  it("stacks the screens when there is room and goes wide when there is not", () => {
    expect(resolveDisplay(DEFAULT_DISPLAY, tall)).toMatchObject({ layout: "stacked", zoom: 1.5 });
    expect(resolveDisplay(DEFAULT_DISPLAY, laptop)).toMatchObject({ layout: "wide", zoom: 1 });
  });

  it("honors a forced arrangement, shrinking to fit if it must", () => {
    const stacked = resolveDisplay({ ...DEFAULT_DISPLAY, layout: "stacked" }, laptop);
    expect(stacked.layout).toBe("stacked");
    expect(stacked.zoom).toBeLessThan(1);
    expect(resolveDisplay({ ...DEFAULT_DISPLAY, layout: "wide" }, tall).layout).toBe("wide");
  });

  it("uses the chosen scale while the window has room for it", () => {
    expect(resolveDisplay({ ...DEFAULT_DISPLAY, scale: 1 }, tall).zoom).toBe(0.5);
    expect(resolveDisplay({ ...DEFAULT_DISPLAY, scale: 2 }, tall).zoom).toBe(1);
  });

  it("never lets the console outgrow the window", () => {
    const display = resolveDisplay({ layout: "stacked", scale: 6, gap: 12 }, phone);
    expect(display.zoom * 520).toBeLessThanOrEqual(phone.width);
    expect(display.largestScale).toBe(1);
  });

  it("counts the gap between the screens", () => {
    const settings = { ...DEFAULT_DISPLAY, layout: "stacked" as const };
    const snug = resolveDisplay({ ...settings, gap: 0 }, { width: 600, height: 800 });
    const apart = resolveDisplay({ ...settings, gap: 160 }, { width: 600, height: 800 });
    expect(snug.zoom).toBe(1);
    expect(apart.zoom).toBeLessThan(1);
  });
});
