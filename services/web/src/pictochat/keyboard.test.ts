import { describe, expect, it } from "vitest";
import { KEYBOARD_COLUMNS, keyboardRows, markKana, type Page, toKatakana } from "./keyboard";

const state = { page: "latin" as Page, caps: false, shift: false, katakana: false };
const typed = (page: Page, overrides = {}) =>
  keyboardRows({ ...state, page, ...overrides })
    .flatMap((row) => row.keys.map((key) => key.character ?? ""))
    .join("");

describe("keyboard pages", () => {
  it("fit the grid", () => {
    for (const page of ["latin", "accents", "kana", "symbols", "pictograms"] as const) {
      const rows = keyboardRows({ ...state, page });
      expect(rows).toHaveLength(5);
      for (const row of rows) {
        const width = row.indent + row.keys.reduce((sum, key) => sum + key.span, 0);
        expect(width).toBeLessThanOrEqual(KEYBOARD_COLUMNS);
      }
    }
  });

  it("put the editing keys in the last column of the grid pages", () => {
    for (const page of ["accents", "kana", "symbols", "pictograms"] as const) {
      const rows = keyboardRows({ ...state, page });
      const last = rows.map((row) => row.keys.at(-1));
      expect(last[1]?.action).toBe("backspace");
      expect(last[2]).toMatchObject({ action: "enter", rows: 2 });
      expect(last[4]?.action).toBe("space");
      // Backspace, Enter and Space each end flush with the right edge.
      for (const index of [1, 2, 4]) {
        const row = rows[index]!;
        expect(row.keys.reduce((sum, key) => sum + key.span, 0)).toBe(KEYBOARD_COLUMNS);
      }
    }
  });

  it("types each character from one key only", () => {
    for (const page of ["accents", "kana", "symbols", "pictograms"] as const) {
      const characters = Array.from(typed(page));
      expect(new Set(characters).size).toBe(characters.length);
    }
  });

  it("shift and caps change the Latin keys", () => {
    expect(typed("latin")).toContain("qwertyuiop");
    expect(typed("latin", { caps: true })).toContain("QWERTYUIOP");
    expect(typed("latin", { caps: true })).toContain("1234567890-=");
    expect(typed("latin", { shift: true })).toContain("!@#$%^&*()_+");
    expect(typed("latin", { caps: true, shift: true })).toContain("qwertyuiop");
  });

  it("switches the kana page to katakana", () => {
    expect(typed("kana")).toContain("あかさたなはまやらわ");
    expect(typed("kana", { katakana: true })).toContain("アカサタナハマヤラワ");
    expect(toKatakana("ひらがな、ok")).toBe("ヒラガナ、ok");
  });
});

describe("kana marks", () => {
  it("adds, swaps and removes marks", () => {
    expect(markKana("か", "dakuten")).toBe("が");
    expect(markKana("が", "dakuten")).toBe("か");
    expect(markKana("は", "handakuten")).toBe("ぱ");
    expect(markKana("ぱ", "dakuten")).toBe("ば");
    expect(markKana("つ", "small")).toBe("っ");
    expect(markKana("っ", "small")).toBe("つ");
  });

  it("works on katakana", () => {
    expect(markKana("カ", "dakuten")).toBe("ガ");
    expect(markKana("ポ", "handakuten")).toBe("ホ");
    expect(markKana("ヤ", "small")).toBe("ャ");
  });

  it("ignores characters a mark does not apply to", () => {
    expect(markKana("あ", "dakuten")).toBeUndefined();
    expect(markKana("か", "handakuten")).toBeUndefined();
    expect(markKana("a", "small")).toBeUndefined();
  });
});
