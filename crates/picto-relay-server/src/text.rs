//! Text layout for REST clients, using the browser's shared bitmap font.
use crate::protocol::{
    BLACK_INK, CANVAS_WIDTH, DRAWABLE_NAMETAG_HEIGHT, FIRST_RAINBOW_INK, INK_INDICES,
    PALETTE_BITMAP_BYTES, PALETTE_BITS_PER_PIXEL, PALETTE_PIXELS_PER_BYTE, RAINBOW_INK_COUNT,
    TEXT_MAX_UNITS,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// --- Ink -------------------------------------------------------------------------

/// The only accepted non-numeric ink: cycles through the rainbow shades.
const RAINBOW_INK_MODE: &str = "rainbow";

/// Width in pixels of each horizontal color band of rainbow text.
const RAINBOW_BAND_WIDTH: usize = 6;

// --- Bitmap font -----------------------------------------------------------------

/// Pixel rows in one unscaled font glyph.
const GLYPH_ROWS: usize = 7;

/// Pixel columns in one unscaled font glyph.
const GLYPH_WIDTH: usize = 5;

/// Unscaled horizontal distance in pixels from one glyph to the next: the
/// glyph plus a one-pixel gap. A space advances by the same amount.
const GLYPH_ADVANCE: usize = GLYPH_WIDTH + 1;

/// One glyph: a bit mask per row, the leftmost pixel in bit `GLYPH_WIDTH - 1`.
type Glyph = [u8; GLYPH_ROWS];

/// Box drawn for characters missing from the font, matching the browser.
const FALLBACK_GLYPH: Glyph = [
    0b11111, 0b10001, 0b10101, 0b10101, 0b10101, 0b10001, 0b11111,
];

// --- Text area (pixels) ----------------------------------------------------------

/// Row of the top of the first text line.
const TEXT_ORIGIN_Y: usize = 1;

/// Leftmost text column on lines that start beside the name tag.
const NAMETAG_SAFE_MIN_X: usize = 88;

/// Leftmost text column on lines that start below the name tag.
const FULL_SAFE_MIN_X: usize = 28;

/// First column right of the visible text area (exclusive right edge).
const VISIBLE_TEXT_MAX_X: usize = 244;

// --- Text sizes ------------------------------------------------------------------

/// Size and line metrics of one text rendering attempt.
#[derive(Clone, Copy)]
struct TextLayout {
    /// Pixels drawn per font pixel in each direction.
    scale: usize,
    /// Vertical distance in pixels between the tops of consecutive lines.
    line_advance: usize,
    /// Maximum number of lines that fit the drawing area.
    max_lines: usize,
}

/// Preferred double-size text.
const LARGE_TEXT: TextLayout = TextLayout {
    scale: 2,
    line_advance: 16,
    max_lines: 5,
};

/// Single-size text, used when the large layout does not fit.
const COMPACT_TEXT: TextLayout = TextLayout {
    scale: 1,
    line_advance: 8,
    max_lines: 10,
};

/// Fixed DS drawing ink index or the string `rainbow`.
#[derive(Clone, Debug, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(untagged)]
pub enum Ink {
    /// One of the supported fixed pen colors.
    Color(u8),
    /// Only `rainbow` is accepted.
    Mode(String),
}
impl Default for Ink {
    fn default() -> Self {
        Self::Color(BLACK_INK)
    }
}

/// Renders wrapped text in the same native-safe area as the browser.
/// # Errors
/// Rejects invalid ink, empty or oversized text, or text that cannot fit.
pub fn render(text: &str, ink: &Ink) -> Result<Vec<u8>, &'static str> {
    match ink {
        Ink::Color(index) if INK_INDICES.contains(index) => {}
        Ink::Mode(mode) if mode == RAINBOW_INK_MODE => {}
        _ => return Err("Ink must be 1, 3–15, or rainbow"),
    }
    let text = text.trim();
    if text.is_empty() || text.encode_utf16().count() > TEXT_MAX_UNITS || text.contains('\0') {
        return Err("Text must contain 1–220 UTF-16 units and no NUL characters");
    }
    let font: BTreeMap<char, Glyph> = serde_json::from_str(include_str!("../assets/font.json"))
        .map_err(|_| "Invalid bundled font")?;
    layout(text, ink, &font, LARGE_TEXT)
        .or_else(|| layout(text, ink, &font, COMPACT_TEXT))
        .ok_or("Text is too long for the PictoChat drawing area")
}
/// Leftmost text column of `line`, which is indented while it starts beside
/// the name tag.
fn origin(line: usize, advance: usize) -> usize {
    if line * advance < DRAWABLE_NAMETAG_HEIGHT {
        NAMETAG_SAFE_MIN_X
    } else {
        FULL_SAFE_MIN_X
    }
}
fn layout(
    text: &str,
    ink: &Ink,
    font: &BTreeMap<char, Glyph>,
    text_layout: TextLayout,
) -> Option<Vec<u8>> {
    let TextLayout {
        scale,
        line_advance: advance,
        max_lines: lines,
    } = text_layout;
    let glyph_width = GLYPH_WIDTH * scale;
    let glyph_advance = GLYPH_ADVANCE * scale;
    let mut bitmap = vec![0; PALETTE_BITMAP_BYTES];
    let chars: Vec<char> = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .collect();
    let (mut line, mut x, mut spaces, mut i) = (0, origin(0, advance), 0, 0);
    while i < chars.len() {
        if chars[i] == '\n' {
            line += 1;
            if line >= lines {
                return None;
            }
            x = origin(line, advance);
            spaces = 0;
            i += 1;
            continue;
        }
        if chars[i].is_whitespace() {
            spaces += 1;
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        let width = (i - start - 1) * glyph_advance + glyph_width;
        let gap = if x > origin(line, advance) {
            spaces * glyph_advance
        } else {
            0
        };
        if x > origin(line, advance) && x + gap + width > VISIBLE_TEXT_MAX_X {
            line += 1;
            if line >= lines {
                return None;
            }
            x = origin(line, advance);
        } else {
            x += gap;
        }
        spaces = 0;
        for c in &chars[start..i] {
            if x + glyph_width > VISIBLE_TEXT_MAX_X {
                line += 1;
                if line >= lines {
                    return None;
                }
                x = origin(line, advance);
            }
            let glyph = font.get(&c.to_ascii_uppercase()).unwrap_or(&FALLBACK_GLYPH);
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..GLYPH_WIDTH {
                    if bits & (1 << (GLYPH_WIDTH - 1 - col)) == 0 {
                        continue;
                    }
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let px = x + col * scale + dx;
                            let py = TEXT_ORIGIN_Y + line * advance + row * scale + dy;
                            let color = match ink {
                                Ink::Color(c) => *c,
                                Ink::Mode(_) => {
                                    let band = px / RAINBOW_BAND_WIDTH;
                                    FIRST_RAINBOW_INK
                                        + u8::try_from(band % usize::from(RAINBOW_INK_COUNT))
                                            .ok()?
                                }
                            };
                            let index = py * CANVAS_WIDTH + px;
                            bitmap[index / PALETTE_PIXELS_PER_BYTE] |= color
                                << ((index % PALETTE_PIXELS_PER_BYTE) * PALETTE_BITS_PER_PIXEL);
                        }
                    }
                }
            }
            x += glyph_advance;
        }
    }
    Some(bitmap)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Deserialize)]
    struct Fixture {
        text: String,
        ink: Ink,
        hash: u32,
    }
    #[test]
    fn rest_renderer_matches_browser_golden_samples() {
        let fixtures: Vec<Fixture> =
            serde_json::from_str(include_str!("../tests/text-fixtures.json")).unwrap();
        for f in fixtures {
            let bytes = render(&f.text, &f.ink).unwrap();
            let hash = bytes.iter().fold(2_166_136_261u32, |h, b| {
                (h ^ u32::from(*b)).wrapping_mul(16_777_619)
            });
            assert_eq!(hash, f.hash, "{}", f.text);
        }
    }
    #[test]
    fn invalid_text_and_ink_are_rejected() {
        for text in [String::new(), "a".repeat(221), "a\0b".into()] {
            assert!(render(&text, &Ink::default()).is_err());
        }
        for ink in [
            Ink::Color(0),
            Ink::Color(2),
            Ink::Color(16),
            Ink::Mode("red".into()),
        ] {
            assert!(render("Hi", &ink).is_err());
        }
    }
}
