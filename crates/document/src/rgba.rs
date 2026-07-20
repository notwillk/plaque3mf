//! Deterministic conversion of straight-alpha RGBA pixels into grayscale.

use crate::{DocumentError, GrayscaleRaster, PixelDimensions};

const RGBA_CHANNEL_COUNT: usize = 4;

/// Composites row-major straight-alpha RGBA pixels over white and converts
/// them to grayscale.
///
/// Each color channel is composited in byte space using round-to-nearest
/// integer arithmetic. The resulting RGB channels are converted with fixed
/// integer weights (`77`, `150`, and `29`), so results do not depend on
/// floating-point behavior or an image-processing library.
pub fn rgba_over_white_to_grayscale(
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<GrayscaleRaster, DocumentError> {
    let dimensions = PixelDimensions::new(width, height)?;
    let expected = dimensions.pixel_count() * RGBA_CHANNEL_COUNT;

    if rgba.len() != expected {
        return Err(DocumentError::RgbaDataLengthMismatch {
            expected,
            actual: rgba.len(),
        });
    }

    let grayscale = rgba
        .chunks_exact(RGBA_CHANNEL_COUNT)
        .map(|pixel| {
            let alpha = u32::from(pixel[3]);
            let red = composite_over_white(pixel[0], alpha);
            let green = composite_over_white(pixel[1], alpha);
            let blue = composite_over_white(pixel[2], alpha);

            ((77 * red + 150 * green + 29 * blue + 128) >> 8) as u8
        })
        .collect();

    GrayscaleRaster::new(width, height, grayscale)
}

fn composite_over_white(channel: u8, alpha: u32) -> u32 {
    (u32::from(channel) * alpha + 255 * (255 - alpha) + 127) / 255
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fully_transparent_pixels_are_white_regardless_of_hidden_color() {
        let raster = rgba_over_white_to_grayscale(
            4,
            1,
            &[0, 0, 0, 0, 255, 0, 0, 0, 0, 255, 0, 0, 17, 83, 201, 0],
        )
        .expect("valid RGBA raster");

        assert_eq!(raster.pixels(), &[255, 255, 255, 255]);
    }

    #[test]
    fn fully_opaque_primary_and_extreme_colors_use_fixed_weights() {
        let raster = rgba_over_white_to_grayscale(
            5,
            1,
            &[
                255, 0, 0, 255, // red
                0, 255, 0, 255, // green
                0, 0, 255, 255, // blue
                0, 0, 0, 255, // black
                255, 255, 255, 255, // white
            ],
        )
        .expect("valid RGBA raster");

        assert_eq!(raster.pixels(), &[77, 149, 29, 0, 255]);
    }

    #[test]
    fn half_alpha_rounding_boundary_is_stable() {
        let raster = rgba_over_white_to_grayscale(
            4,
            1,
            &[
                0, 0, 0, 127, // just below half opacity
                0, 0, 0, 128, // exactly the upper integer half
                255, 255, 255, 127, 255, 255, 255, 128,
            ],
        )
        .expect("valid RGBA raster");

        assert_eq!(raster.pixels(), &[128, 127, 255, 255]);
    }

    #[test]
    fn conversion_preserves_top_left_row_major_order() {
        let raster = rgba_over_white_to_grayscale(
            2,
            2,
            &[
                0, 0, 0, 255, // (0, 0)
                255, 255, 255, 255, // (1, 0)
                255, 0, 0, 255, // (0, 1)
                0, 255, 0, 255, // (1, 1)
            ],
        )
        .expect("valid RGBA raster");

        assert_eq!(raster.pixels(), &[0, 255, 77, 149]);
        assert_eq!(raster.pixel(0, 0), Some(0));
        assert_eq!(raster.pixel(1, 0), Some(255));
        assert_eq!(raster.pixel(0, 1), Some(77));
        assert_eq!(raster.pixel(1, 1), Some(149));
    }

    #[test]
    fn invalid_rgba_length_reports_expected_and_actual_byte_counts() {
        assert_eq!(
            rgba_over_white_to_grayscale(2, 1, &[0; 7]),
            Err(DocumentError::RgbaDataLengthMismatch {
                expected: 8,
                actual: 7,
            })
        );
        assert_eq!(
            rgba_over_white_to_grayscale(1, 1, &[0; 5]),
            Err(DocumentError::RgbaDataLengthMismatch {
                expected: 4,
                actual: 5,
            })
        );
    }
}
