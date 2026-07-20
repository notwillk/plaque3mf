//! Document normalization into canonical two-dimensional artwork.
//!
//! Renderer adapters supply an already-composited grayscale raster. The core
//! types here remain independent of PDF renderers and image libraries.

#[cfg(feature = "pdfium")]
pub mod pdfium;
mod rgba;
mod sizing;

pub use rgba::rgba_over_white_to_grayscale;
pub use sizing::{OutputSizeConstraintsMicrometers, RasterizationPlan, SamplingPitchMicrometers};

use std::{error::Error, fmt};

/// Maximum number of pixels accepted by one canonical raster.
///
/// A fixed limit makes resource validation deterministic across platforms and
/// prevents a threshold operation from unexpectedly allocating an enormous
/// second image.
pub const MAX_RASTER_PIXELS: u64 = 268_435_456;

/// Validated dimensions for a non-empty pixel grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelDimensions {
    width: u32,
    height: u32,
    pixel_count: usize,
}

impl PixelDimensions {
    /// Creates pixel dimensions within [`MAX_RASTER_PIXELS`].
    pub fn new(width: u32, height: u32) -> Result<Self, DocumentError> {
        if width == 0 {
            return Err(DocumentError::ZeroPixelDimension {
                dimension: Dimension::Width,
            });
        }
        if height == 0 {
            return Err(DocumentError::ZeroPixelDimension {
                dimension: Dimension::Height,
            });
        }

        let pixel_count = u64::from(width) * u64::from(height);
        if pixel_count > MAX_RASTER_PIXELS {
            return Err(DocumentError::RasterTooLarge {
                width,
                height,
                max_pixels: MAX_RASTER_PIXELS,
            });
        }

        Ok(Self {
            width,
            height,
            pixel_count: usize::try_from(pixel_count)
                .expect("MAX_RASTER_PIXELS fits into usize on supported targets"),
        })
    }

    /// Returns the number of columns.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Returns the number of rows.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }

    /// Returns `width * height`.
    #[must_use]
    pub const fn pixel_count(self) -> usize {
        self.pixel_count
    }

    fn index(self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(y as usize * self.width as usize + x as usize)
    }
}

/// Positive physical extents in integer micrometres.
///
/// The extents describe the full raster. They are stored separately from pixel
/// dimensions because rounding can give the X and Y axes slightly different
/// effective sampling pitches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalSizeMicrometers {
    width: i64,
    height: i64,
}

impl PhysicalSizeMicrometers {
    /// Creates positive physical extents.
    pub fn new(width: i64, height: i64) -> Result<Self, DocumentError> {
        if width <= 0 {
            return Err(DocumentError::NonPositivePhysicalDimension {
                dimension: Dimension::Width,
                micrometers: width,
            });
        }
        if height <= 0 {
            return Err(DocumentError::NonPositivePhysicalDimension {
                dimension: Dimension::Height,
                micrometers: height,
            });
        }
        Ok(Self { width, height })
    }

    /// Returns the width in micrometres.
    #[must_use]
    pub const fn width(self) -> i64 {
        self.width
    }

    /// Returns the height in micrometres.
    #[must_use]
    pub const fn height(self) -> i64 {
        self.height
    }
}

/// A row-major grayscale raster with a top-left origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrayscaleRaster {
    dimensions: PixelDimensions,
    pixels: Vec<u8>,
}

impl GrayscaleRaster {
    /// Creates a raster and validates its shape and data length.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, DocumentError> {
        let dimensions = PixelDimensions::new(width, height)?;
        if pixels.len() != dimensions.pixel_count() {
            return Err(DocumentError::DataLengthMismatch {
                expected: dimensions.pixel_count(),
                actual: pixels.len(),
            });
        }
        Ok(Self { dimensions, pixels })
    }

    /// Returns the raster dimensions.
    #[must_use]
    pub const fn dimensions(&self) -> PixelDimensions {
        self.dimensions
    }

    /// Returns all row-major grayscale samples.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Returns one sample, or `None` for an out-of-bounds coordinate.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<u8> {
        self.dimensions.index(x, y).map(|index| self.pixels[index])
    }
}

/// Rules for classifying grayscale samples as foreground.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForegroundSelection {
    threshold: u8,
    invert: bool,
}

impl ForegroundSelection {
    /// Creates classification rules.
    ///
    /// A sample is foreground exactly when it is strictly less than
    /// `threshold`; `invert` negates that boolean result.
    #[must_use]
    pub const fn new(threshold: u8, invert: bool) -> Self {
        Self { threshold, invert }
    }

    /// Returns the exclusive threshold.
    #[must_use]
    pub const fn threshold(self) -> u8 {
        self.threshold
    }

    /// Returns whether classification is inverted.
    #[must_use]
    pub const fn invert(self) -> bool {
        self.invert
    }

    /// Classifies one sample.
    #[must_use]
    pub const fn is_foreground(self, grayscale: u8) -> bool {
        (grayscale < self.threshold) != self.invert
    }
}

/// A row-major binary mask with a top-left origin.
///
/// Bytes returned by [`Self::as_bytes`] are canonical: zero is background and
/// one is foreground.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryMask {
    dimensions: PixelDimensions,
    pixels: Vec<u8>,
}

impl BinaryMask {
    /// Creates a mask from canonical zero/one bytes.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, DocumentError> {
        let dimensions = PixelDimensions::new(width, height)?;
        if pixels.len() != dimensions.pixel_count() {
            return Err(DocumentError::DataLengthMismatch {
                expected: dimensions.pixel_count(),
                actual: pixels.len(),
            });
        }
        if let Some((index, value)) = pixels
            .iter()
            .copied()
            .enumerate()
            .find(|(_, value)| *value > 1)
        {
            return Err(DocumentError::InvalidBinaryValue { index, value });
        }
        Ok(Self { dimensions, pixels })
    }

    /// Thresholds an already-composited grayscale raster.
    #[must_use]
    pub fn from_grayscale(raster: &GrayscaleRaster, selection: ForegroundSelection) -> Self {
        let pixels = raster
            .pixels()
            .iter()
            .map(|sample| u8::from(selection.is_foreground(*sample)))
            .collect();
        Self {
            dimensions: raster.dimensions(),
            pixels,
        }
    }

    /// Returns the mask dimensions.
    #[must_use]
    pub const fn dimensions(&self) -> PixelDimensions {
        self.dimensions
    }

    /// Returns the canonical zero/one bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.pixels
    }

    /// Returns one classification, or `None` if the coordinate is out of bounds.
    #[must_use]
    pub fn is_foreground(&self, x: u32, y: u32) -> Option<bool> {
        self.dimensions
            .index(x, y)
            .map(|index| self.pixels[index] == 1)
    }

    /// Counts foreground pixels.
    #[must_use]
    pub fn foreground_pixel_count(&self) -> usize {
        self.pixels.iter().map(|value| usize::from(*value)).sum()
    }
}

/// Renderer-independent artwork consumed by planar geometry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalArtwork {
    physical_size: PhysicalSizeMicrometers,
    foreground: BinaryMask,
}

impl CanonicalArtwork {
    /// Combines physical extents with a binary foreground mask.
    #[must_use]
    pub const fn new(physical_size: PhysicalSizeMicrometers, foreground: BinaryMask) -> Self {
        Self {
            physical_size,
            foreground,
        }
    }

    /// Thresholds a grayscale raster into canonical artwork.
    #[must_use]
    pub fn from_grayscale(
        physical_size: PhysicalSizeMicrometers,
        raster: &GrayscaleRaster,
        selection: ForegroundSelection,
    ) -> Self {
        Self::new(physical_size, BinaryMask::from_grayscale(raster, selection))
    }

    /// Returns the physical extents.
    #[must_use]
    pub const fn physical_size(&self) -> PhysicalSizeMicrometers {
        self.physical_size
    }

    /// Returns the binary foreground mask.
    #[must_use]
    pub const fn foreground(&self) -> &BinaryMask {
        &self.foreground
    }
}

/// Width or height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    /// Horizontal extent.
    Width,
    /// Vertical extent.
    Height,
}

impl fmt::Display for Dimension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Width => formatter.write_str("width"),
            Self::Height => formatter.write_str("height"),
        }
    }
}

/// An invalid document-layer value.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocumentError {
    /// A pixel-grid dimension is zero.
    ZeroPixelDimension {
        /// The invalid dimension.
        dimension: Dimension,
    },
    /// The grid exceeds the deterministic resource limit.
    RasterTooLarge {
        /// Requested width.
        width: u32,
        /// Requested height.
        height: u32,
        /// Maximum accepted pixel count.
        max_pixels: u64,
    },
    /// Data length does not equal width times height.
    DataLengthMismatch {
        /// Required sample count.
        expected: usize,
        /// Supplied sample count.
        actual: usize,
    },
    /// A physical dimension is zero or negative.
    NonPositivePhysicalDimension {
        /// The invalid dimension.
        dimension: Dimension,
        /// Supplied micrometres.
        micrometers: i64,
    },
    /// Neither a maximum output width nor height was supplied.
    MissingOutputSizeConstraint,
    /// A sampling pitch is zero or negative.
    NonPositiveSamplingPitch {
        /// Supplied micrometres per pixel.
        micrometers: i64,
    },
    /// Aspect-ratio fitting rounded an output dimension to zero micrometres.
    OutputDimensionRoundsToZero {
        /// The dimension that became zero.
        dimension: Dimension,
    },
    /// An output dimension cannot be represented in integer micrometres.
    OutputDimensionTooLarge {
        /// The dimension that exceeded the supported range.
        dimension: Dimension,
    },
    /// A pixel dimension cannot be represented as a 32-bit unsigned integer.
    PixelDimensionTooLarge {
        /// The dimension that exceeded the supported range.
        dimension: Dimension,
        /// Requested pixel count along that dimension.
        pixels: u128,
    },
    /// RGBA byte length does not equal four times width times height.
    RgbaDataLengthMismatch {
        /// Required byte count.
        expected: usize,
        /// Supplied byte count.
        actual: usize,
    },
    /// A binary mask contains a byte other than zero or one.
    InvalidBinaryValue {
        /// Row-major position of the byte.
        index: usize,
        /// Invalid byte value.
        value: u8,
    },
}

impl fmt::Display for DocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroPixelDimension { dimension } => {
                write!(formatter, "pixel {dimension} must be greater than zero")
            }
            Self::RasterTooLarge {
                width,
                height,
                max_pixels,
            } => write!(
                formatter,
                "raster {width}x{height} exceeds the limit of {max_pixels} pixels"
            ),
            Self::DataLengthMismatch { expected, actual } => write!(
                formatter,
                "pixel data has length {actual}; expected {expected}"
            ),
            Self::NonPositivePhysicalDimension {
                dimension,
                micrometers,
            } => write!(
                formatter,
                "physical {dimension} must be positive; got {micrometers} micrometres"
            ),
            Self::MissingOutputSizeConstraint => {
                formatter.write_str("at least one maximum output dimension is required")
            }
            Self::NonPositiveSamplingPitch { micrometers } => write!(
                formatter,
                "sampling pitch must be positive; got {micrometers} micrometres per pixel"
            ),
            Self::OutputDimensionRoundsToZero { dimension } => write!(
                formatter,
                "fitted output {dimension} rounds to zero micrometres"
            ),
            Self::OutputDimensionTooLarge { dimension } => write!(
                formatter,
                "fitted output {dimension} exceeds the supported micrometre range"
            ),
            Self::PixelDimensionTooLarge { dimension, pixels } => write!(
                formatter,
                "pixel {dimension} of {pixels} exceeds the supported 32-bit range"
            ),
            Self::RgbaDataLengthMismatch { expected, actual } => write!(
                formatter,
                "RGBA data has length {actual}; expected {expected}"
            ),
            Self::InvalidBinaryValue { index, value } => write!(
                formatter,
                "binary mask byte at index {index} is {value}; expected 0 or 1"
            ),
        }
    }
}

impl Error for DocumentError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_dimensions_reject_empty_and_excessive_rasters() {
        assert_eq!(
            PixelDimensions::new(0, 1),
            Err(DocumentError::ZeroPixelDimension {
                dimension: Dimension::Width
            })
        );
        assert_eq!(
            PixelDimensions::new(1, 0),
            Err(DocumentError::ZeroPixelDimension {
                dimension: Dimension::Height
            })
        );
        assert_eq!(
            PixelDimensions::new(16_385, 16_384),
            Err(DocumentError::RasterTooLarge {
                width: 16_385,
                height: 16_384,
                max_pixels: MAX_RASTER_PIXELS
            })
        );
        assert_eq!(
            PixelDimensions::new(16_384, 16_384)
                .expect("dimensions at the limit are valid")
                .pixel_count(),
            MAX_RASTER_PIXELS as usize
        );
    }

    #[test]
    fn physical_size_requires_positive_micrometers() {
        assert_eq!(
            PhysicalSizeMicrometers::new(0, 1),
            Err(DocumentError::NonPositivePhysicalDimension {
                dimension: Dimension::Width,
                micrometers: 0
            })
        );
        assert_eq!(
            PhysicalSizeMicrometers::new(1, -1),
            Err(DocumentError::NonPositivePhysicalDimension {
                dimension: Dimension::Height,
                micrometers: -1
            })
        );
    }

    #[test]
    fn grayscale_raster_validates_length_and_is_row_major() {
        assert_eq!(
            GrayscaleRaster::new(2, 2, vec![0, 1, 2]),
            Err(DocumentError::DataLengthMismatch {
                expected: 4,
                actual: 3
            })
        );
        let raster = GrayscaleRaster::new(2, 2, vec![10, 20, 30, 40]).expect("valid raster");
        assert_eq!(raster.pixel(0, 0), Some(10));
        assert_eq!(raster.pixel(1, 0), Some(20));
        assert_eq!(raster.pixel(0, 1), Some(30));
        assert_eq!(raster.pixel(1, 1), Some(40));
        assert_eq!(raster.pixel(2, 0), None);
    }

    #[test]
    fn binary_mask_rejects_noncanonical_bytes() {
        assert_eq!(
            BinaryMask::new(2, 2, vec![0, 1, 2, 0]),
            Err(DocumentError::InvalidBinaryValue { index: 2, value: 2 })
        );
    }

    #[test]
    fn threshold_is_strict_and_inversion_negates_it() {
        let raster = GrayscaleRaster::new(4, 1, vec![0, 127, 128, 255]).expect("valid raster");
        let normal = BinaryMask::from_grayscale(&raster, ForegroundSelection::new(128, false));
        let inverted = BinaryMask::from_grayscale(&raster, ForegroundSelection::new(128, true));
        assert_eq!(normal.as_bytes(), &[1, 1, 0, 0]);
        assert_eq!(inverted.as_bytes(), &[0, 0, 1, 1]);
    }

    #[test]
    fn threshold_extremes_keep_nonwhite_default_semantics() {
        let all_values = GrayscaleRaster::new(256, 1, (u8::MIN..=u8::MAX).collect())
            .expect("all samples form a raster");
        let zero = BinaryMask::from_grayscale(&all_values, ForegroundSelection::new(0, false));
        let inverted_zero =
            BinaryMask::from_grayscale(&all_values, ForegroundSelection::new(0, true));
        let default = BinaryMask::from_grayscale(&all_values, ForegroundSelection::new(255, false));
        assert_eq!(zero.foreground_pixel_count(), 0);
        assert_eq!(inverted_zero.foreground_pixel_count(), 256);
        assert_eq!(default.foreground_pixel_count(), 255);
        assert_eq!(default.is_foreground(254, 0), Some(true));
        assert_eq!(default.is_foreground(255, 0), Some(false));
    }

    #[test]
    fn every_threshold_and_sample_obeys_the_contract() {
        for threshold in u8::MIN..=u8::MAX {
            for sample in u8::MIN..=u8::MAX {
                assert_eq!(
                    ForegroundSelection::new(threshold, false).is_foreground(sample),
                    sample < threshold
                );
                assert_eq!(
                    ForegroundSelection::new(threshold, true).is_foreground(sample),
                    sample >= threshold
                );
            }
        }
    }

    #[test]
    fn canonical_artwork_retains_physical_and_pixel_extents() {
        let physical = PhysicalSizeMicrometers::new(100_000, 50_000).expect("positive extents");
        let raster = GrayscaleRaster::new(2, 1, vec![0, 255]).expect("valid raster");
        let artwork = CanonicalArtwork::from_grayscale(
            physical,
            &raster,
            ForegroundSelection::new(255, false),
        );
        assert_eq!(artwork.physical_size(), physical);
        assert_eq!(artwork.foreground().dimensions(), raster.dimensions());
        assert_eq!(artwork.foreground().as_bytes(), &[1, 0]);
        assert_eq!(artwork.foreground().is_foreground(0, 0), Some(true));
        assert_eq!(artwork.foreground().is_foreground(2, 0), None);
    }
}
