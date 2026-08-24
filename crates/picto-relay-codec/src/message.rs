use crate::{DecodeError, MacAddress};

/// Width of the native `PictoChat` drawing surface in pixels.
pub const DRAWING_WIDTH: usize = 256;
/// Maximum height of the native `PictoChat` drawing surface in pixels.
pub const DRAWING_HEIGHT: usize = 80;
/// Bytes preceding the native tiled drawing payload.
pub const DRAWING_METADATA_BYTES: usize = 36;
/// Bytes used for each encoded sixteen-row drawing band.
pub const DRAWING_BAND_BYTES: usize = 2_048;

const TILE_WIDTH: usize = 8;
const TILE_HEIGHT: usize = 8;
const TILE_BYTES: usize = 32;
const TILES_PER_ROW: usize = DRAWING_WIDTH / TILE_WIDTH;
const PIXEL_COUNT: usize = DRAWING_WIDTH * DRAWING_HEIGHT;

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
    if pixels.is_empty() || pixels.len() % DRAWING_WIDTH != 0 {
        return Err(DecodeError::InvalidMessage(
            "drawing must contain complete 256-pixel rows",
        ));
    }
    let height = pixels.len() / DRAWING_WIDTH;
    if height > DRAWING_HEIGHT || height % 16 != 0 {
        return Err(DecodeError::InvalidMessage(
            "drawing height must be 16 through 80 rows in sixteen-row bands",
        ));
    }
    if pixels.iter().any(|&pixel| pixel > 0x0f) {
        return Err(DecodeError::InvalidMessage(
            "drawing palette indices must fit in four bits",
        ));
    }

    let mac = sender.octets();
    let mut message = vec![0; DRAWING_METADATA_BYTES + height * DRAWING_WIDTH / 2];
    message[..20].copy_from_slice(&[
        0x03, 0x02, mac[1], mac[0], mac[3], mac[2], mac[5], mac[4], 0x00, 0x04, 0x00, 0x00, 0x00,
        0x00, 0x01, 0x04, 0x09, 0x0b, 0x10, 0x1b,
    ]);

    for tile_y in 0..height / TILE_HEIGHT {
        for tile_x in 0..TILES_PER_ROW {
            let tile_offset =
                DRAWING_METADATA_BYTES + (tile_y * TILES_PER_ROW + tile_x) * TILE_BYTES;
            for row_in_tile in 0..TILE_HEIGHT {
                let y = tile_y * TILE_HEIGHT + row_in_tile;
                for pair_in_row in 0..(TILE_WIDTH / 2) {
                    let x = tile_x * TILE_WIDTH + pair_in_row * 2;
                    let left = pixels[y * DRAWING_WIDTH + x];
                    let right = pixels[y * DRAWING_WIDTH + x + 1];
                    message[tile_offset + row_in_tile * (TILE_WIDTH / 2) + pair_in_row] =
                        left | (right << 4);
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
            || drawing.len() > DRAWING_BAND_BYTES * 5
            || drawing.len() % DRAWING_BAND_BYTES != 0
        {
            return Err(DecodeError::InvalidMessage(
                "drawing must contain one through five 2048-byte bands",
            ));
        }
        let metadata = <[u8; DRAWING_METADATA_BYTES]>::try_from(metadata)
            .map_err(|_| DecodeError::InvalidMessage("metadata is truncated"))?;
        let encoded_height = drawing.len() / (DRAWING_WIDTH / 2);
        let mut pixels = vec![0; PIXEL_COUNT];
        for (byte_offset, &packed) in drawing.iter().enumerate() {
            let tile_index = byte_offset / TILE_BYTES;
            let byte_in_tile = byte_offset % TILE_BYTES;
            let tile_y = tile_index / TILES_PER_ROW;
            let tile_x = tile_index % TILES_PER_ROW;
            let row_in_tile = byte_in_tile / (TILE_WIDTH / 2);
            let pair_in_row = byte_in_tile % (TILE_WIDTH / 2);
            let y = tile_y * TILE_HEIGHT + row_in_tile;
            let x = tile_x * TILE_WIDTH + pair_in_row * 2;
            pixels[y * DRAWING_WIDTH + x] = packed & 0x0f;
            pixels[y * DRAWING_WIDTH + x + 1] = packed >> 4;
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
        MacAddress([
            self.metadata[3],
            self.metadata[2],
            self.metadata[5],
            self.metadata[4],
            self.metadata[7],
            self.metadata[6],
        ])
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
            if palette_index == 0 {
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
