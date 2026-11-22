import { describe, expect, it } from "vitest";
import tableText from "../../../../crates/picto-relay-server/src/font.json?raw";
// The server's golden samples of rendered text.
import fixtures from "../../../../crates/picto-relay-server/tests/text-fixtures.json";
import { buildFontTable } from "../../scripts/font-table.mjs";
import { blank, type Ink, pack } from "./drawing";
import { glyphOf } from "./glyphs";
import { Typewriter } from "./typing";

interface Fixture {
  text: string;
  ink: number | string;
  /** Whether typing the text character by character gives the same drawing. */
  typed: boolean;
  hash: number;
}

/** FNV-1a, as the server's test computes it. */
const hash = (bytes: Uint8Array) =>
  bytes.reduce((value, byte) => Math.imul(value ^ byte, 16777619) >>> 0, 2166136261);

describe("the glyph table", () => {
  it("is what the generator produces from the font", () => {
    // If this fails, run `npm run glyphs`.
    expect(tableText).toBe(buildFontTable());
  });

  it("holds the DS font's characters", () => {
    expect(glyphOf("A").advance).toBe(6);
    expect(glyphOf(" ")).toEqual({ advance: 5, dots: [] });
    expect(glyphOf("あ").advance).toBe(10);
    expect(glyphOf("g").dots.some((dot) => dot.y > 0)).toBe(true);
  });

  it("gives a placeholder for anything else", () => {
    expect(glyphOf("\u{1F600}")).toBe(glyphOf("\u{1F601}"));
    expect(glyphOf("\u{1F600}").dots.length).toBeGreaterThan(0);
    expect(glyphOf("constructor").dots).toEqual(glyphOf("\u{1F600}").dots);
  });
});

describe("typed text", () => {
  it("matches the server's rendering of the same text", () => {
    const typed = (fixtures as Fixture[]).filter((fixture) => fixture.typed);
    expect(typed.length).toBeGreaterThan(0);
    for (const fixture of typed) {
      const pixels = blank();
      const typewriter = new Typewriter(glyphOf);
      for (const character of fixture.text) {
        if (character === "\n") typewriter.newline();
        else typewriter.type(pixels, character, fixture.ink as Ink);
      }
      expect(hash(pack(pixels)), fixture.text).toBe(fixture.hash);
    }
  });
});
