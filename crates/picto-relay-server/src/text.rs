//! Text for REST clients, drawn the way the web client's keyboard types it:
//! in the DS font, one line of text per ruled line, starting beside the name
//! tag.
//!
//! The glyph bitmaps come from `font.json`, which the web client generates
//! from the DS font (`npm run glyphs` in `services/web/`) and draws with too, so text
//! sent through the API and text typed in the browser are the same pixels.

use std::{collections::HashMap, sync::LazyLock};

use serde::{Deserialize, Serialize};

use crate::protocol::{
    BLACK_INK, CANVAS_WIDTH, DRAWABLE_LEFT, DRAWABLE_NAMETAG_HEIGHT, DRAWABLE_NAMETAG_LEFT,
    DRAWABLE_RIGHT, INK_INDICES, NATIVE_DRAWING_HEIGHT, PALETTE_BITMAP_BYTES,
    PALETTE_BITS_PER_PIXEL, PALETTE_INDEX_MASK, PALETTE_PIXELS_PER_BYTE, RAINBOW_INKS,
    TEXT_MAX_UNITS,
};

// --- Ink -------------------------------------------------------------------------

/// The only accepted non-numeric ink: cycles through the rainbow shades.
const RAINBOW_INK_MODE: &str = "rainbow";

/// Width in pixels of each color band of rainbow text.
const RAINBOW_BAND_WIDTH: usize = 6;

// --- Layout (pixels) -------------------------------------------------------------

/// Height of one line of text: the spacing of the ruled lines.
const LINE_HEIGHT: usize = 16;

/// Lines of text a drawing holds.
const LINES: usize = NATIVE_DRAWING_HEIGHT / LINE_HEIGHT;

/// Row of the text baseline within its line.
const BASELINE: usize = 13;

/// Gap kept between text and the edges of the box and name tag.
const TEXT_INSET: usize = 4;

/// First column text may not reach.
const TEXT_RIGHT: usize = DRAWABLE_RIGHT - TEXT_INSET;

/// Error for text that needs more lines than a drawing holds.
const TOO_LONG: &str = "Text is too long for the PictoChat drawing area";

// --- Font ------------------------------------------------------------------------

/// Columns a glyph row can describe: the bits of a row mask.
const GLYPH_COLUMNS: usize = u16::BITS as usize;

/// One character of the DS font.
struct Glyph {
    /// Columns the pen moves right after this character.
    advance: usize,
    /// Column of the leftmost inked pixel, relative to the pen.
    x: usize,
    /// Row of the topmost inked pixel, relative to the baseline; negative is above it.
    y: isize,
    /// One mask per inked row, top down. Bit `k` is the pixel `k` columns right of `x`.
    rows: Vec<u16>,
}

impl Glyph {
    /// Reads a table entry: `[advance]` for a blank glyph, else
    /// `[advance, x, y, ...rows]`.
    fn from_table(values: &[i32]) -> Option<Self> {
        let (advance, shape) = values.split_first()?;
        let (x, y, rows) = match shape {
            [] => (0, 0, &[][..]),
            [x, y, rows @ ..] => (*x, *y, rows),
            [_] => return None,
        };
        Some(Self {
            advance: usize::try_from(*advance).ok()?,
            x: usize::try_from(x).ok()?,
            y: isize::try_from(y).ok()?,
            rows: rows
                .iter()
                .map(|row| u16::try_from(*row).ok())
                .collect::<Option<_>>()?,
        })
    }
}

/// The DS font as bitmaps.
struct Font {
    /// Drawn for a character the font lacks.
    placeholder: Glyph,
    glyphs: HashMap<char, Glyph>,
}

impl Font {
    fn glyph(&self, character: char) -> &Glyph {
        self.glyphs.get(&character).unwrap_or(&self.placeholder)
    }
}

/// The layout of `font.json`.
#[derive(Deserialize)]
struct FontTable {
    placeholder: Vec<i32>,
    glyphs: HashMap<String, Vec<i32>>,
}

/// The bundled font, or `None` if `font.json` is malformed.
static FONT: LazyLock<Option<Font>> = LazyLock::new(|| {
    let table: FontTable = serde_json::from_str(include_str!("font.json")).ok()?;
    let glyphs = table
        .glyphs
        .iter()
        .map(|(key, values)| {
            let mut characters = key.chars();
            let character = characters.next().filter(|_| characters.next().is_none())?;
            Some((character, Glyph::from_table(values)?))
        })
        .collect::<Option<_>>()?;
    Some(Font {
        placeholder: Glyph::from_table(&table.placeholder)?,
        glyphs,
    })
});

/// A fixed pen color, or the string `rainbow`.
///
/// Fixed colors are DS palette indices. As a DS shows them: 1 black, 2 gray,
/// 3 magenta, 4 pink, 5 red, 6 red orange, 7 orange, 8 yellow, 9 lime,
/// 10 green, 11 teal, 12 cyan, 13 light blue, 14 blue, 15 dark blue. `rainbow`
/// cycles through 3 to 15 in that order.
#[derive(Clone, Debug, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(untagged)]
pub enum Ink {
    /// One of the indices in `ink_indices` from `GET /status`.
    Color(u8),
    /// Only `rainbow` is accepted.
    Mode(String),
}

impl Default for Ink {
    fn default() -> Self {
        Self::Color(BLACK_INK)
    }
}

impl Ink {
    /// The palette index for a pixel in `column`. Rainbow text changes shade
    /// every few columns, so a line runs through the colors.
    fn at(&self, column: usize) -> u8 {
        match self {
            Self::Color(index) => *index,
            Self::Mode(_) => RAINBOW_INKS[(column / RAINBOW_BAND_WIDTH) % RAINBOW_INKS.len()],
        }
    }
}

/// Renders text as a packed palette bitmap.
///
/// Words wrap whole where they can; a word longer than a line is split.
/// Characters the font lacks are drawn as a placeholder box.
///
/// # Errors
///
/// Rejects invalid ink, empty or oversized text, and text that needs more
/// lines than a drawing holds.
pub fn render(text: &str, ink: &Ink) -> Result<Vec<u8>, &'static str> {
    match ink {
        Ink::Color(index) if INK_INDICES.contains(index) => {}
        Ink::Mode(mode) if mode == RAINBOW_INK_MODE => {}
        _ => return Err("Ink must be 1–15 or rainbow"),
    }
    let text = text.trim();
    if text.is_empty() || text.encode_utf16().count() > TEXT_MAX_UNITS || text.contains('\0') {
        return Err("Text must contain 1–220 UTF-16 units and no NUL characters");
    }
    let font = FONT.as_ref().ok_or("Invalid bundled font")?;
    let characters: Vec<char> = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .collect();

    let mut page = Page::new();
    let mut gap = 0;
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if character == '\n' {
            page.break_line()?;
            gap = 0;
            index += 1;
            continue;
        }
        if character.is_whitespace() {
            gap += font.glyph(' ').advance;
            index += 1;
            continue;
        }
        let end = characters[index..]
            .iter()
            .position(|next| next.is_whitespace())
            .map_or(characters.len(), |length| index + length);
        let word = &characters[index..end];
        let width: usize = word.iter().map(|c| font.glyph(*c).advance).sum();
        // Keep the word whole on the next line if it does not fit on this one.
        // The space before it goes with the line break.
        if !page.at_line_start() && page.x + gap + width > TEXT_RIGHT {
            page.break_line()?;
        } else {
            page.x += gap;
        }
        gap = 0;
        for character in word {
            let glyph = font.glyph(*character);
            // A word longer than a whole line is split where it runs out of room.
            if page.x + glyph.advance > TEXT_RIGHT {
                page.break_line()?;
            }
            page.stamp(glyph, ink);
        }
        index = end;
    }
    Ok(page.bitmap)
}

/// A drawing being typed onto: the bitmap and the pen.
struct Page {
    bitmap: Vec<u8>,
    /// Column of the pen.
    x: usize,
    /// Line the pen is on, from zero.
    line: usize,
}

impl Page {
    fn new() -> Self {
        Self {
            bitmap: vec![0; PALETTE_BITMAP_BYTES],
            x: line_start(0),
            line: 0,
        }
    }

    fn at_line_start(&self) -> bool {
        self.x == line_start(self.line)
    }

    fn break_line(&mut self) -> Result<(), &'static str> {
        if self.line + 1 >= LINES {
            return Err(TOO_LONG);
        }
        self.line += 1;
        self.x = line_start(self.line);
        Ok(())
    }

    /// Draws a glyph at the pen and advances the pen past it.
    fn stamp(&mut self, glyph: &Glyph, ink: &Ink) {
        let baseline = self.line * LINE_HEIGHT + BASELINE;
        for (row, mask) in glyph.rows.iter().enumerate() {
            let Some(y) = baseline
                .checked_add(row)
                .and_then(|y| y.checked_add_signed(glyph.y))
            else {
                continue;
            };
            for column in (0..GLYPH_COLUMNS).filter(|column| mask & (1 << column) != 0) {
                let x = self.x + glyph.x + column;
                if drawable(x, y) {
                    self.set(x, y, ink.at(x));
                }
            }
        }
        self.x += glyph.advance;
    }

    fn set(&mut self, x: usize, y: usize, index: u8) {
        let pixel = y * CANVAS_WIDTH + x;
        let shift = (pixel % PALETTE_PIXELS_PER_BYTE) * PALETTE_BITS_PER_PIXEL;
        let byte = &mut self.bitmap[pixel / PALETTE_PIXELS_PER_BYTE];
        *byte = (*byte & !(PALETTE_INDEX_MASK << shift)) | (index << shift);
    }
}

/// Leftmost pen position on a line; the first line starts beside the name tag.
const fn line_start(line: usize) -> usize {
    let left = if line * LINE_HEIGHT < DRAWABLE_NAMETAG_HEIGHT {
        DRAWABLE_NAMETAG_LEFT
    } else {
        DRAWABLE_LEFT
    };
    left + TEXT_INSET
}

/// Whether a DS shows this pixel: inside the box and clear of the name tag.
const fn drawable(x: usize, y: usize) -> bool {
    let left = if y < DRAWABLE_NAMETAG_HEIGHT {
        DRAWABLE_NAMETAG_LEFT
    } else {
        DRAWABLE_LEFT
    };
    y < NATIVE_DRAWING_HEIGHT && x >= left && x < DRAWABLE_RIGHT
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A text, the ink it is drawn in, and the FNV-1a hash of its bitmap.
    /// `typed` marks texts that never wrap at a word, which the web client's
    /// keyboard reproduces exactly; its tests check the same hashes.
    #[derive(Deserialize, Serialize)]
    struct Fixture {
        text: String,
        ink: Ink,
        typed: bool,
        hash: u32,
    }

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/text-fixtures.json");

    fn hash(bitmap: &[u8]) -> u32 {
        bitmap.iter().fold(2_166_136_261, |hash, byte| {
            (hash ^ u32::from(*byte)).wrapping_mul(16_777_619)
        })
    }

    /// The leftmost inked column of a row.
    fn first_ink(bitmap: &[u8], y: usize) -> Option<usize> {
        (0..CANVAS_WIDTH).find(|x| {
            let pixel = y * CANVAS_WIDTH + x;
            let shift = (pixel % PALETTE_PIXELS_PER_BYTE) * PALETTE_BITS_PER_PIXEL;
            (bitmap[pixel / PALETTE_PIXELS_PER_BYTE] >> shift) & PALETTE_INDEX_MASK != 0
        })
    }

    /// With `PICTO_WRITE_FIXTURES=1` this rewrites the hashes, for after a
    /// deliberate change to the font or the layout.
    #[test]
    fn renderer_matches_golden_samples() {
        let mut fixtures: Vec<Fixture> =
            serde_json::from_str(&std::fs::read_to_string(FIXTURES).unwrap()).unwrap();
        if std::env::var_os("PICTO_WRITE_FIXTURES").is_some() {
            for fixture in &mut fixtures {
                fixture.hash = hash(&render(&fixture.text, &fixture.ink).unwrap());
            }
            let json = serde_json::to_string_pretty(&fixtures).unwrap();
            std::fs::write(FIXTURES, json + "\n").unwrap();
            return;
        }
        for fixture in fixtures {
            let bitmap = render(&fixture.text, &fixture.ink).unwrap();
            assert_eq!(hash(&bitmap), fixture.hash, "{}", fixture.text);
            assert!(first_ink(&bitmap, BASELINE).is_some(), "{}", fixture.text);
        }
    }

    #[test]
    fn text_starts_beside_the_name_tag_and_then_at_the_left_edge() {
        let bitmap = render("H\nH", &Ink::default()).unwrap();
        assert_eq!(first_ink(&bitmap, BASELINE), Some(84));
        assert_eq!(first_ink(&bitmap, LINE_HEIGHT + BASELINE), Some(28));
    }

    #[test]
    fn words_wrap_whole_and_the_space_goes_with_the_break() {
        let font = FONT.as_ref().unwrap();
        let word = "word";
        let width: usize = word.chars().map(|c| font.glyph(c).advance).sum();
        let space = font.glyph(' ').advance;
        // One word more than the first line has room for.
        let fit = (TEXT_RIGHT - line_start(0) + space) / (width + space);
        let wrapped = render(&vec![word; fit + 1].join(" "), &Ink::default()).unwrap();
        let broken = format!("{}\n{word}", vec![word; fit].join(" "));
        assert_eq!(wrapped, render(&broken, &Ink::default()).unwrap());
    }

    #[test]
    fn characters_missing_from_the_font_become_a_placeholder() {
        let missing = render("\u{1F600}", &Ink::default()).unwrap();
        assert_eq!(missing, render("\u{1F601}", &Ink::default()).unwrap());
        assert_ne!(missing, render("O", &Ink::default()).unwrap());
        assert_eq!(first_ink(&missing, BASELINE), Some(84));
    }

    #[test]
    fn rainbow_text_uses_only_rainbow_inks() {
        let bitmap = render("rainbow rainbow rainbow", &Ink::Mode("rainbow".into())).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for byte in bitmap {
            seen.insert(byte & PALETTE_INDEX_MASK);
            seen.insert(byte >> PALETTE_BITS_PER_PIXEL);
        }
        seen.remove(&0);
        assert!(seen.len() > 5);
        assert!(seen.iter().all(|index| RAINBOW_INKS.contains(index)));
    }

    #[test]
    fn invalid_text_and_ink_are_rejected() {
        for text in [String::new(), "a".repeat(221), "a\0b".into()] {
            assert!(render(&text, &Ink::default()).is_err());
        }
        for ink in [Ink::Color(0), Ink::Color(16), Ink::Mode("red".into())] {
            assert!(render("Hi", &ink).is_err());
        }
        assert_eq!(render(&"word ".repeat(44), &Ink::default()), Err(TOO_LONG));
        assert_eq!(render("1\n2\n3\n4\n5\n6", &Ink::default()), Err(TOO_LONG));
    }
}
