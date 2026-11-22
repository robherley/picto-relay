//! Conversion between API bitmaps and native DS drawings.
use crate::surface::{
    BITMAP_BYTES as API_BITMAP_BYTES, BLACK_INK, CANVAS_HEIGHT as API_BITMAP_HEIGHT,
    CANVAS_WIDTH as API_BITMAP_WIDTH, MONO_PIXELS_PER_BYTE, PALETTE_BITMAP_BYTES,
    PALETTE_BITS_PER_PIXEL, PALETTE_INDEX_MASK, PALETTE_PIXELS_PER_BYTE, PAPER_INK,
};
use anyhow::{Result, bail};
use picto_relay_codec::{DRAWING_BAND_HEIGHT, DRAWING_HEIGHT, DRAWING_WIDTH, PictoChatMessage};

/// Tallest drawing the DS accepts, in pixel rows; taller API ink is scaled down.
const MAX_NATIVE_DRAWING_HEIGHT: usize = DRAWING_HEIGHT;

/// Height in pixel rows of one native drawing band; a native drawing is always
/// a whole number of bands tall.
const NATIVE_BAND_HEIGHT: usize = DRAWING_BAND_HEIGHT;

/// Bytes in one row of a packed one-bit API bitmap.
const MONO_ROW_BYTES: usize = API_BITMAP_WIDTH / MONO_PIXELS_PER_BYTE;

/// Bytes in one row of a packed palette API bitmap.
const PALETTE_ROW_BYTES: usize = API_BITMAP_WIDTH / PALETTE_PIXELS_PER_BYTE;

/// Index of the byte holding pixel (`x`, `y`) in a packed one-bit bitmap.
const fn mono_byte_index(x: usize, y: usize) -> usize {
    y * MONO_ROW_BYTES + x / MONO_PIXELS_PER_BYTE
}

/// Bit mask of column `x` within its one-bit bitmap byte; the leftmost pixel
/// is the most significant bit.
const fn mono_bit_mask(x: usize) -> u8 {
    1 << (MONO_PIXELS_PER_BYTE - 1 - x % MONO_PIXELS_PER_BYTE)
}

/// Bit offset of the `index`th pixel within its packed palette byte; the left
/// pixel occupies the low nibble.
const fn palette_shift(index: usize) -> usize {
    (index % PALETTE_PIXELS_PER_BYTE) * PALETTE_BITS_PER_PIXEL
}

/// Converts an API bitmap to the DS native drawing surface.
/// # Errors
/// Rejects invalid size and completely blank drawings.
pub fn api_bitmap_to_native(bitmap: &[u8]) -> Result<Vec<u8>> {
    if bitmap.len() != API_BITMAP_BYTES && bitmap.len() != PALETTE_BITMAP_BYTES {
        bail!(
            "bitmap must contain {API_BITMAP_BYTES} monochrome or {PALETTE_BITMAP_BYTES} palette bytes, received {}",
            bitmap.len()
        );
    }
    let mut bounds: Option<(usize, usize, usize, usize)> = None;
    for y in 0..API_BITMAP_HEIGHT {
        for x in 0..API_BITMAP_WIDTH {
            if api_pixel(bitmap, x, y) == PAPER_INK {
                continue;
            }
            bounds = Some(match bounds {
                Some((min_x, min_y, max_x, max_y)) => {
                    (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y))
                }
                None => (x, y, x, y),
            });
        }
    }
    let Some((min_x, min_y, max_x, max_y)) = bounds else {
        bail!("PictoChat does not support a completely blank drawing");
    };
    if max_y < MAX_NATIVE_DRAWING_HEIGHT {
        let encoded_height = (max_y + NATIVE_BAND_HEIGHT) / NATIVE_BAND_HEIGHT * NATIVE_BAND_HEIGHT;
        let mut pixels = vec![PAPER_INK; DRAWING_WIDTH * encoded_height];
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                if api_pixel(bitmap, x, y) != PAPER_INK {
                    pixels[y * DRAWING_WIDTH + x] = api_pixel(bitmap, x, y);
                }
            }
        }
        return Ok(pixels);
    }

    let source_width = max_x - min_x + 1;
    let source_height = max_y - min_y + 1;
    let target_width =
        (source_width * MAX_NATIVE_DRAWING_HEIGHT / source_height).clamp(1, DRAWING_WIDTH);
    let offset_x = (DRAWING_WIDTH - target_width) / 2;
    let mut pixels = vec![PAPER_INK; DRAWING_WIDTH * MAX_NATIVE_DRAWING_HEIGHT];
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            if api_pixel(bitmap, x, y) != PAPER_INK {
                let target_x =
                    offset_x + ((x - min_x) * target_width / source_width).min(target_width - 1);
                let target_y = ((y - min_y) * MAX_NATIVE_DRAWING_HEIGHT / source_height)
                    .min(MAX_NATIVE_DRAWING_HEIGHT - 1);
                pixels[target_y * DRAWING_WIDTH + target_x] = api_pixel(bitmap, x, y);
            }
        }
    }
    Ok(pixels)
}

fn api_pixel(bitmap: &[u8], x: usize, y: usize) -> u8 {
    if bitmap.len() == PALETTE_BITMAP_BYTES {
        let byte = y * PALETTE_ROW_BYTES + x / PALETTE_PIXELS_PER_BYTE;
        return (bitmap[byte] >> palette_shift(x)) & PALETTE_INDEX_MASK;
    }
    if bitmap[mono_byte_index(x, y)] & mono_bit_mask(x) == 0 {
        PAPER_INK
    } else {
        BLACK_INK
    }
}

/// Converts a decoded DS drawing to a padded API palette bitmap.
#[must_use]
pub fn native_message_to_api_bitmap(message: &PictoChatMessage) -> Vec<u8> {
    let mut bitmap = vec![0; PALETTE_BITMAP_BYTES];
    for (i, &pixel) in message.pixels().iter().enumerate() {
        bitmap[i / PALETTE_PIXELS_PER_BYTE] |= pixel << palette_shift(i);
    }
    bitmap
}

/// An API bitmap of either kind as a palette bitmap, the form events carry.
/// A one-bit bitmap becomes black ink on paper.
#[must_use]
pub fn palette_bitmap(bitmap: &[u8]) -> Vec<u8> {
    if bitmap.len() == PALETTE_BITMAP_BYTES {
        return bitmap.to_vec();
    }
    let mut palette = vec![0; PALETTE_BITMAP_BYTES];
    for y in 0..API_BITMAP_HEIGHT {
        for x in 0..API_BITMAP_WIDTH {
            let pixel = y * API_BITMAP_WIDTH + x;
            palette[pixel / PALETTE_PIXELS_PER_BYTE] |=
                api_pixel(bitmap, x, y) << palette_shift(pixel);
        }
    }
    palette
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn palette_round_trip_preserves_every_index() {
        let mut palette = vec![0; PALETTE_BITMAP_BYTES];
        for i in 0u8..16 {
            palette[usize::from(i) / 2] |= i << ((i % 2) * 4);
        }
        let native = api_bitmap_to_native(&palette).unwrap();
        assert_eq!(&native[..16], &(0u8..16).collect::<Vec<_>>());
        let encoded =
            picto_relay_codec::encode_message(picto_relay_codec::MacAddress([0; 6]), &native)
                .unwrap();
        let decoded = PictoChatMessage::parse(&encoded).unwrap();
        assert_eq!(native_message_to_api_bitmap(&decoded), palette);
        assert_eq!(palette_bitmap(&palette), palette);
        // A one-bit bitmap comes out as black ink, pixel for pixel.
        let mut mono = vec![0; API_BITMAP_BYTES];
        mono[0] = 0xa0;
        assert_eq!(&palette_bitmap(&mono)[..2], &[0x01, 0x01]);
    }

    #[test]
    fn scaling_tall_palette_drawings_keeps_ink_indices() {
        let mut palette = vec![0; PALETTE_BITMAP_BYTES];
        palette[95 * (API_BITMAP_WIDTH / 2) + 44] = 13;
        let native = api_bitmap_to_native(&palette).unwrap();
        assert_eq!(native.len(), DRAWING_WIDTH * 80);
        assert!(native.contains(&13));
        assert!(native.iter().all(|&v| v == 0 || v == 13));
    }
    #[test]
    fn live_bitmap_should_reject_a_blank_drawing() {
        let bitmap = vec![0; API_BITMAP_BYTES];

        assert!(api_bitmap_to_native(&bitmap).is_err());
    }

    #[test]
    fn live_bitmap_should_preserve_horizontal_position_for_native_height_ink() {
        let mut bitmap = vec![0; API_BITMAP_BYTES];
        let (x, y) = (88, 8);
        let byte = y * (API_BITMAP_WIDTH / 8) + x / 8;
        bitmap[byte] |= 1 << (7 - x % 8);

        let pixels = api_bitmap_to_native(&bitmap).expect("drawing should fit");

        assert_eq!(pixels[y * DRAWING_WIDTH + x], 1);
    }

    #[test]
    fn live_bitmap_should_fit_tall_ink_into_five_native_bands() {
        let mut bitmap = vec![0; API_BITMAP_BYTES];
        for y in 0..API_BITMAP_HEIGHT {
            let x = y * 2;
            let byte = y * (API_BITMAP_WIDTH / 8) + x / 8;
            bitmap[byte] |= 1 << (7 - x % 8);
        }

        let pixels = api_bitmap_to_native(&bitmap).expect("drawing should fit");

        assert_eq!(pixels.len(), DRAWING_WIDTH * MAX_NATIVE_DRAWING_HEIGHT);
        assert!(pixels.iter().any(|&pixel| pixel != 0));
        assert!(
            pixels[((MAX_NATIVE_DRAWING_HEIGHT - 1) * DRAWING_WIDTH)..]
                .iter()
                .any(|&pixel| pixel != 0)
        );
    }
}
