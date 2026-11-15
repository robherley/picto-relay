use crate::{DecodeError, MacAddress, read_word_swapped_mac, word_swapped_mac};

// Drawing surface geometry.

/// Width of the native `PictoChat` drawing surface in pixels.
pub const DRAWING_WIDTH: usize = 256;
/// Maximum height of the native `PictoChat` drawing surface in pixels.
pub const DRAWING_HEIGHT: usize = 80;
/// Pixel rows in one drawing band; drawings grow in whole bands.
pub const DRAWING_BAND_HEIGHT: usize = 16;
/// Largest number of drawing bands in one message.
const MAX_BANDS: usize = DRAWING_HEIGHT / DRAWING_BAND_HEIGHT;
/// Pixels on the full, zero-padded drawing canvas.
const PIXEL_COUNT: usize = DRAWING_WIDTH * DRAWING_HEIGHT;

// 4bpp pixel packing.

/// Bits used by one palette index.
const BITS_PER_PIXEL: u32 = 4;
/// Mask selecting one palette index; also the largest valid palette index.
const PIXEL_MASK: u8 = (1 << BITS_PER_PIXEL) - 1;
/// Pixels packed into each drawing byte (low nibble left, high nibble right).
const PIXELS_PER_BYTE: usize = 2;
/// Palette index of a transparent/white pixel in observed drawings.
const TRANSPARENT_PALETTE_INDEX: u8 = 0;
/// Encoded bytes covering one full-width pixel row.
const ROW_BYTES: usize = DRAWING_WIDTH / PIXELS_PER_BYTE;
/// Bytes used for each encoded sixteen-row drawing band.
pub const DRAWING_BAND_BYTES: usize = DRAWING_BAND_HEIGHT * ROW_BYTES;

// Nintendo DS tile layout.

/// Width of one Nintendo DS tile in pixels.
const TILE_WIDTH: usize = 8;
/// Height of one Nintendo DS tile in pixels.
const TILE_HEIGHT: usize = 8;
/// Encoded bytes covering one pixel row of a tile.
const TILE_ROW_BYTES: usize = TILE_WIDTH / PIXELS_PER_BYTE;
/// Encoded bytes in one 4bpp tile.
const TILE_BYTES: usize = TILE_ROW_BYTES * TILE_HEIGHT;
/// Tiles across the drawing surface.
const TILES_PER_ROW: usize = DRAWING_WIDTH / TILE_WIDTH;

// Message metadata layout.

/// Two-byte metadata prefix captured from a real client; observed, meaning unknown.
const OPAQUE_METADATA_PREFIX: [u8; 2] = [0x03, 0x02];
/// Offset of the sender MAC, stored as three little-endian 16-bit words.
const METADATA_SENDER_OFFSET: usize = OPAQUE_METADATA_PREFIX.len();
/// End of the sender MAC and start of the remaining opaque metadata.
const METADATA_SENDER_END: usize = METADATA_SENDER_OFFSET + crate::MAC_ADDRESS_LEN;
/// Opaque metadata bytes following the sender MAC.
const OPAQUE_METADATA_TAIL_BYTES: usize = 28;
/// Bytes preceding the native tiled drawing payload.
pub const DRAWING_METADATA_BYTES: usize = METADATA_SENDER_END + OPAQUE_METADATA_TAIL_BYTES;
/// Leading nonzero part of the opaque metadata after the sender MAC, replayed verbatim
/// from a captured client drawing; observed, meaning unknown. The rest of the opaque
/// tail is zero.
const OPAQUE_METADATA_AFTER_SENDER: [u8; 12] = [
    0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x01, 0x04, 0x09, 0x0b, 0x10, 0x1b,
];
/// End of the replayed opaque metadata within the metadata block.
const OPAQUE_METADATA_AFTER_SENDER_END: usize =
    METADATA_SENDER_END + OPAQUE_METADATA_AFTER_SENDER.len();
const _: () = assert!(OPAQUE_METADATA_AFTER_SENDER.len() <= OPAQUE_METADATA_TAIL_BYTES);

/// Encodes a 16- through 80-row, 4bpp native `PictoChat` drawing.
///
/// `pixels` contains row-major palette indices for a 256-pixel-wide surface whose
/// height is a multiple of sixteen. Palette index zero is transparent/white in
/// observed drawings; indices 1 through 15 are ink values.
///
/// # Errors
///
/// Returns [`DecodeError::InvalidMessage`] unless the input contains one through
/// five complete drawing bands and every palette index fits in four bits.
pub fn encode_message(sender: MacAddress, pixels: &[u8]) -> Result<Vec<u8>, DecodeError> {
    if pixels.is_empty() || !pixels.len().is_multiple_of(DRAWING_WIDTH) {
        return Err(DecodeError::InvalidMessage(
            "drawing must contain complete 256-pixel rows",
        ));
    }
    let height = pixels.len() / DRAWING_WIDTH;
    if height > DRAWING_HEIGHT || !height.is_multiple_of(DRAWING_BAND_HEIGHT) {
        return Err(DecodeError::InvalidMessage(
            "drawing height must be 16 through 80 rows in sixteen-row bands",
        ));
    }
    if pixels.iter().any(|&pixel| pixel > PIXEL_MASK) {
        return Err(DecodeError::InvalidMessage(
            "drawing palette indices must fit in four bits",
        ));
    }

    let mut message = vec![0; DRAWING_METADATA_BYTES + height * ROW_BYTES];
    message[..METADATA_SENDER_OFFSET].copy_from_slice(&OPAQUE_METADATA_PREFIX);
    message[METADATA_SENDER_OFFSET..METADATA_SENDER_END].copy_from_slice(&word_swapped_mac(sender));
    message[METADATA_SENDER_END..OPAQUE_METADATA_AFTER_SENDER_END]
        .copy_from_slice(&OPAQUE_METADATA_AFTER_SENDER);

    for tile_y in 0..height / TILE_HEIGHT {
        for tile_x in 0..TILES_PER_ROW {
            let tile_offset =
                DRAWING_METADATA_BYTES + (tile_y * TILES_PER_ROW + tile_x) * TILE_BYTES;
            for row_in_tile in 0..TILE_HEIGHT {
                let y = tile_y * TILE_HEIGHT + row_in_tile;
                for pair_in_row in 0..TILE_ROW_BYTES {
                    let x = tile_x * TILE_WIDTH + pair_in_row * PIXELS_PER_BYTE;
                    let left = pixels[y * DRAWING_WIDTH + x];
                    let right = pixels[y * DRAWING_WIDTH + x + 1];
                    message[tile_offset + row_in_tile * TILE_ROW_BYTES + pair_in_row] =
                        left | (right << BITS_PER_PIXEL);
                }
            }
        }
    }
    Ok(message)
}

/// Inclusive bounds of nonzero pixels in a decoded drawing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PixelBounds {
    /// Leftmost nonzero pixel.
    pub min_x: usize,
    /// Topmost nonzero pixel.
    pub min_y: usize,
    /// Rightmost nonzero pixel.
    pub max_x: usize,
    /// Bottommost nonzero pixel.
    pub max_y: usize,
}

/// Reassembled native `PictoChat` metadata and normalized 4bpp drawing pixels.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PictoChatMessage {
    metadata: [u8; DRAWING_METADATA_BYTES],
    encoded_height: usize,
    pixels: Vec<u8>,
}

impl PictoChatMessage {
    /// Decodes a reassembled message containing one to five sixteen-row drawing bands.
    ///
    /// Drawing bytes use Nintendo's 8x8 4bpp tile order. Within each byte, the low nibble
    /// is the left pixel and the high nibble is the right pixel.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::InvalidMessage`] when the metadata is truncated or the drawing
    /// length is not a complete 16-row band between 16 and 80 rows.
    pub fn parse(message: &[u8]) -> Result<Self, DecodeError> {
        let Some((metadata, drawing)) = message.split_at_checked(DRAWING_METADATA_BYTES) else {
            return Err(DecodeError::InvalidMessage("metadata is truncated"));
        };
        if drawing.is_empty()
            || drawing.len() > DRAWING_BAND_BYTES * MAX_BANDS
            || drawing.len() % DRAWING_BAND_BYTES != 0
        {
            return Err(DecodeError::InvalidMessage(
                "drawing must contain one through five 2048-byte bands",
            ));
        }
        let metadata = <[u8; DRAWING_METADATA_BYTES]>::try_from(metadata)
            .map_err(|_| DecodeError::InvalidMessage("metadata is truncated"))?;
        let encoded_height = drawing.len() / ROW_BYTES;
        let mut pixels = vec![TRANSPARENT_PALETTE_INDEX; PIXEL_COUNT];
        for (byte_offset, &packed) in drawing.iter().enumerate() {
            let tile_index = byte_offset / TILE_BYTES;
            let byte_in_tile = byte_offset % TILE_BYTES;
            let tile_y = tile_index / TILES_PER_ROW;
            let tile_x = tile_index % TILES_PER_ROW;
            let row_in_tile = byte_in_tile / TILE_ROW_BYTES;
            let pair_in_row = byte_in_tile % TILE_ROW_BYTES;
            let y = tile_y * TILE_HEIGHT + row_in_tile;
            let x = tile_x * TILE_WIDTH + pair_in_row * PIXELS_PER_BYTE;
            pixels[y * DRAWING_WIDTH + x] = packed & PIXEL_MASK;
            pixels[y * DRAWING_WIDTH + x + 1] = packed >> BITS_PER_PIXEL;
        }
        Ok(Self {
            metadata,
            encoded_height,
            pixels,
        })
    }

    /// Returns the 36-byte metadata block retained from the wire.
    #[must_use]
    pub const fn metadata(&self) -> &[u8; DRAWING_METADATA_BYTES] {
        &self.metadata
    }

    /// Returns the sender MAC encoded as three little-endian words in captured metadata.
    #[must_use]
    pub const fn sender(&self) -> MacAddress {
        read_word_swapped_mac(&self.metadata, METADATA_SENDER_OFFSET)
    }

    /// Returns the number of encoded drawing rows before zero-padding to 80 rows.
    #[must_use]
    pub const fn encoded_height(&self) -> usize {
        self.encoded_height
    }

    /// Returns all normalized row-major palette indices for the 256x80 canvas.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Returns one 4-bit palette index, or `None` for a coordinate outside the canvas.
    #[must_use]
    pub fn pixel(&self, x: usize, y: usize) -> Option<u8> {
        if x >= DRAWING_WIDTH || y >= DRAWING_HEIGHT {
            return None;
        }
        Some(self.pixels[y * DRAWING_WIDTH + x])
    }

    /// Finds the inclusive rectangle containing every nonzero palette index.
    #[must_use]
    pub fn ink_bounds(&self) -> Option<PixelBounds> {
        let mut bounds = None;
        for (index, &palette_index) in self.pixels.iter().enumerate() {
            if palette_index == TRANSPARENT_PALETTE_INDEX {
                continue;
            }
            let x = index % DRAWING_WIDTH;
            let y = index / DRAWING_WIDTH;
            bounds = Some(match bounds {
                Some(PixelBounds {
                    min_x,
                    min_y,
                    max_x,
                    max_y,
                }) => PixelBounds {
                    min_x: min_x.min(x),
                    min_y: min_y.min(y),
                    max_x: max_x.max(x),
                    max_y: max_y.max(y),
                },
                None => PixelBounds {
                    min_x: x,
                    min_y: y,
                    max_x: x,
                    max_y: y,
                },
            });
        }
        bounds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_band_message() -> Vec<u8> {
        vec![0; DRAWING_METADATA_BYTES + DRAWING_BAND_BYTES]
    }

    #[test]
    fn parse_should_decode_low_and_high_nibbles_in_tiled_order() {
        let mut bytes = one_band_message();
        let tile_index = TILES_PER_ROW + 3;
        let byte_in_tile = 4 * (TILE_WIDTH / 2) + 2;
        bytes[DRAWING_METADATA_BYTES + tile_index * TILE_BYTES + byte_in_tile] = 0xa3;
        let message = PictoChatMessage::parse(&bytes).expect("message should decode");
        assert_eq!(
            (message.pixel(28, 12), message.pixel(29, 12)),
            (Some(3), Some(10))
        );
    }

    #[test]
    fn parse_should_reject_partial_drawing_band() {
        let bytes = vec![0; DRAWING_METADATA_BYTES + DRAWING_BAND_BYTES - 1];
        assert!(PictoChatMessage::parse(&bytes).is_err());
    }

    #[test]
    fn parse_should_decode_the_fifth_drawing_band() {
        let mut bytes = vec![0; DRAWING_METADATA_BYTES + DRAWING_BAND_BYTES * 5];
        let final_byte = bytes.last_mut().expect("drawing should have a final byte");
        *final_byte = 0xb4;
        let message = PictoChatMessage::parse(&bytes).expect("message should decode");
        assert_eq!(message.encoded_height(), 80);
        assert_eq!(
            (message.pixel(254, 79), message.pixel(255, 79)),
            (Some(4), Some(11))
        );
    }

    #[test]
    fn ink_bounds_should_enclose_nonzero_pixels() {
        let mut bytes = one_band_message();
        bytes[DRAWING_METADATA_BYTES] = 0x01;
        let message = PictoChatMessage::parse(&bytes).expect("message should decode");
        assert_eq!(
            message.ink_bounds(),
            Some(PixelBounds {
                min_x: 0,
                min_y: 0,
                max_x: 0,
                max_y: 0
            })
        );
    }

    #[test]
    fn encode_message_should_round_trip_one_band_sender_and_pixels() {
        let sender = MacAddress([0x00, 0x16, 0x56, 0x4d, 0x27, 0xca]);
        let mut pixels = vec![0; DRAWING_WIDTH * 16];
        pixels[12 * DRAWING_WIDTH + 28] = 3;
        pixels[12 * DRAWING_WIDTH + 29] = 10;

        let bytes = encode_message(sender, &pixels).expect("drawing should encode");
        let message = PictoChatMessage::parse(&bytes).expect("drawing should decode");

        assert_eq!(message.sender(), sender);
        assert_eq!(message.encoded_height(), 16);
        assert_eq!(message.pixel(28, 12), Some(3));
        assert_eq!(message.pixel(29, 12), Some(10));
        assert_eq!(
            &bytes[..DRAWING_METADATA_BYTES],
            &[
                0x03, 0x02, 0x16, 0x00, 0x4d, 0x56, 0xca, 0x27, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00,
                0x01, 0x04, 0x09, 0x0b, 0x10, 0x1b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ]
        );
    }

    #[test]
    fn encode_message_should_round_trip_five_drawing_bands() {
        let sender = MacAddress([0x00, 0x16, 0x56, 0x4d, 0x27, 0xca]);
        let mut pixels = vec![0; DRAWING_WIDTH * DRAWING_HEIGHT];
        pixels[79 * DRAWING_WIDTH + 255] = 3;

        let encoded = encode_message(sender, &pixels).expect("drawing should encode");
        let decoded = PictoChatMessage::parse(&encoded).expect("drawing should decode");

        assert_eq!(decoded.pixel(255, 79), Some(3));
    }

    #[test]
    fn encode_message_should_reject_invalid_input() {
        let sender = MacAddress([0; 6]);
        assert!(encode_message(sender, &[0; 1]).is_err());
        let mut pixels = vec![0; DRAWING_WIDTH * 16];
        pixels[0] = 16;
        assert!(encode_message(sender, &pixels).is_err());
    }
}
