//! The drawing surface the JSON API and the web client exchange: 256 by 96
//! pixels, packed one bit or four bits to a pixel.

/// Logical width of a `PictoChat` drawing in pixels.
pub const CANVAS_WIDTH: usize = 256;

/// Logical height of a `PictoChat` drawing in pixels.
pub const CANVAS_HEIGHT: usize = 96;

/// Pixels stored in each byte of a packed one-bit drawing.
pub const MONO_PIXELS_PER_BYTE: usize = u8::BITS as usize;

/// Bits used by each pixel of a packed palette drawing.
pub const PALETTE_BITS_PER_PIXEL: usize = 4;

/// Pixels stored in each byte of a packed palette drawing.
pub const PALETTE_PIXELS_PER_BYTE: usize = u8::BITS as usize / PALETTE_BITS_PER_PIXEL;

/// Mask selecting one palette index (one nibble) from a packed palette byte.
pub const PALETTE_INDEX_MASK: u8 = (1 << PALETTE_BITS_PER_PIXEL) - 1;

/// Bytes in a packed one-bit drawing.
pub const BITMAP_BYTES: usize = CANVAS_WIDTH * CANVAS_HEIGHT / MONO_PIXELS_PER_BYTE;
/// Packed 4bpp palette bitmap size in bytes (low nibble is the left pixel).
pub const PALETTE_BITMAP_BYTES: usize = CANVAS_WIDTH * CANVAS_HEIGHT / PALETTE_PIXELS_PER_BYTE;

/// Palette index of blank paper.
pub const PAPER_INK: u8 = 0;

/// Palette index of black ink, the only ink of a one-bit drawing.
pub const BLACK_INK: u8 = 1;
