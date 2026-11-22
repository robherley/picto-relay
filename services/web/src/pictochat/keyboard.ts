// The on-screen keyboard's pages. Keys sit on a grid of half-key columns.

export type KeyAction =
  | "backspace" | "enter" | "space" | "caps" | "shift"
  | "katakana" | "dakuten" | "handakuten" | "small";

/** A key, or with neither `character` nor `action`, an empty space. */
export interface Key {
  /** The character this key types. */
  character?: string;
  /** What the key does instead of typing a character. */
  action?: KeyAction;
  /** Text on the keycap, when it is not the character itself. */
  label?: string;
  /** Width in half-key columns. */
  span: number;
  /** Height in rows, when more than one. */
  rows?: number;
}

export interface KeyRow {
  /** Half-key columns left empty before the first key. */
  indent: number;
  keys: Key[];
}

export type Page = "latin" | "accents" | "kana" | "symbols" | "pictograms";

export interface KeyboardState {
  page: Page;
  /** Caps is held on. */
  caps: boolean;
  /** Shift applies to the next character only. */
  shift: boolean;
  /** The kana page shows katakana. */
  katakana: boolean;
}

/** Half-key columns in a row. */
export const KEYBOARD_COLUMNS = 24;
const KEY_SPAN = 2;

const characters = (text: string): Key[] =>
  Array.from(text, (character) => ({ character, span: KEY_SPAN }));
const action = (name: KeyAction, label: string, span: number): Key => ({
  action: name, label, span,
});

const BACKSPACE = "←";
const SHIFTED: Record<string, string> = {
  "1": "!", "2": "@", "3": "#", "4": "$", "5": "%", "6": "^", "7": "&", "8": "*",
  "9": "(", "0": ")", "-": "_", "=": "+", ",": "<", ".": ">", "/": "?",
  ";": ":", "'": "\"", "[": "{", "]": "}",
};

function latin(state: KeyboardState): KeyRow[] {
  const upper = state.caps !== state.shift;
  const keys = (text: string) =>
    characters(text).map((key) => {
      const character = key.character!;
      const shifted = state.shift ? SHIFTED[character] : undefined;
      return { ...key, character: shifted ?? (upper ? character.toUpperCase() : character) };
    });
  return [
    { indent: 0, keys: keys("1234567890-=") },
    { indent: 1, keys: [...keys("qwertyuiop"), action("backspace", BACKSPACE, 3)] },
    { indent: 0, keys: [action("caps", "CAPS", 3), ...keys("asdfghjkl"), action("enter", "ENTER", 3)] },
    { indent: 0, keys: [action("shift", "SHIFT", 4), ...keys("zxcvbnm,./")] },
    { indent: 4, keys: [...keys(";'"), action("space", "SPACE", 10), ...keys("[]")] },
  ];
}

/** Character keys across every page but the Latin one. */
const GRID_COLUMNS = 11;
const gap = (span: number): Key => ({ span });

/**
 * The editing keys down the right edge of a grid page, by row: nothing,
 * Backspace, Enter (two rows tall), nothing, Space.
 */
const EDITING: Key[][] = [
  [],
  [action("backspace", BACKSPACE, KEY_SPAN)],
  [{ ...action("enter", "ENTER", KEY_SPAN), rows: 2 }],
  [],
  [action("space", "SPACE", KEY_SPAN)],
];

/** Five rows of up to eleven keys, with the editing keys on the right. */
function grid(rows: Key[][]): KeyRow[] {
  return rows.map((keys, index) => {
    const used = keys.reduce((sum, key) => sum + key.span, 0);
    const padding = GRID_COLUMNS * KEY_SPAN - used;
    return { indent: 0, keys: [...keys, ...(padding > 0 ? [gap(padding)] : []), ...EDITING[index]!] };
  });
}

const ACCENTS = ["àáâäèéêëìíî", "ïòóôöœùúûüç", "ñßÀÁÂÄÈÉÊËÌ", "ÍÎÏÒÓÔÖŒÙÚÛ", "ÜÇÑ¡¿€"];
const SYMBOLS = ["!?&\"'～:;@~_", "+-*/×÷=→←↑↓", "「」“”()<>{}・", "%※〒#♭♪±$¢£\\", "^°|／＼∞∴…™©®"];
/**
 * The DS's pictograms. The faces are private-use characters of the DS font;
 * the rest sit at their Unicode code points.
 */
const PICTOGRAMS = [
  "1234567890=",
  "\uE026\uE027\uE028\uE029☀☁☂☃✉📱⏰",
  "ⒶⒷⓍⓎⓁⓇ✚♠♦♥♣",
  "❕❓+-☆○◇□△▽◎",
  "➡⬅⬆⬇★●◆■▲▼×",
];
/** Hiragana by vowel row; the last two rows end in the mark and mode keys. */
const HIRAGANA = ["あかさたなはまやらわー", "いきしちにひみ「りを、", "うくすつぬふむゆるん。", "えけせてねへめ」れ", "おこそとのほもよろ"];

/** Offset from a hiragana code point to the matching katakana. */
const KATAKANA_OFFSET = 0x60;
const isHiragana = (character: string) => character >= "ぁ" && character <= "ゖ";
const isKatakana = (character: string) => character >= "ァ" && character <= "ヶ";
const shift = (character: string, by: number) =>
  String.fromCodePoint(character.codePointAt(0)! + by);

export function toKatakana(text: string): string {
  return Array.from(text, (c) => (isHiragana(c) ? shift(c, KATAKANA_OFFSET) : c)).join("");
}

/** Each plain kana followed by its marked or small form. */
const DAKUTEN = "かがきぎくぐけげこごさざしじすずせぜそぞただちぢつづてでとどはばひびふぶへべほぼ";
const HANDAKUTEN = "はぱひぴふぷへぺほぽ";
const SMALL = "あぁいぃうぅえぇおぉつっやゃゆゅよょわゎ";

function pairs(text: string): Map<string, string> {
  const list = Array.from(text);
  return new Map(list.flatMap((c, i) => (i % 2 === 0 ? [[c, list[i + 1]!] as const] : [])));
}
const MARKS = { dakuten: pairs(DAKUTEN), handakuten: pairs(HANDAKUTEN), small: pairs(SMALL) };
/** Every marked or small kana back to its plain form. */
const PLAIN = new Map(
  Object.values(MARKS).flatMap((map) => [...map].map(([plain, marked]) => [marked, plain] as const)),
);

/**
 * What a kana becomes when a mark key is pressed after it: the mark is added,
 * swapped for the other mark, or removed if already present. Undefined when
 * the mark does not apply.
 */
export function markKana(
  character: string, mark: "dakuten" | "handakuten" | "small",
): string | undefined {
  const katakana = isKatakana(character);
  const hiragana = katakana ? shift(character, -KATAKANA_OFFSET) : character;
  const plain = PLAIN.get(hiragana) ?? hiragana;
  const marked = MARKS[mark].get(plain);
  if (!marked) return undefined;
  const result = marked === hiragana ? plain : marked;
  return katakana ? shift(result, KATAKANA_OFFSET) : result;
}

function kana(state: KeyboardState): KeyRow[] {
  const rows = HIRAGANA.map((row) => characters(state.katakana ? toKatakana(row) : row));
  rows[3]!.push(action("dakuten", "゛", KEY_SPAN), action("handakuten", "゜", KEY_SPAN));
  rows[4]!.push(action("small", "小", KEY_SPAN), action("katakana", state.katakana ? "あ" : "ア", KEY_SPAN));
  return grid(rows);
}

export function keyboardRows(state: KeyboardState): KeyRow[] {
  switch (state.page) {
    case "latin": return latin(state);
    case "accents": return grid(ACCENTS.map(characters));
    case "symbols": return grid(SYMBOLS.map(characters));
    case "pictograms": return grid(PICTOGRAMS.map(characters));
    case "kana": return kana(state);
  }
}
