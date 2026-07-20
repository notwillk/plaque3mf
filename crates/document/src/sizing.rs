use crate::{Dimension, DocumentError, PhysicalSizeMicrometers, PixelDimensions};

/// Optional maximum output dimensions expressed in integer micrometres.
///
/// At least one maximum must be present. When both are present, the source is
/// fitted inside their bounding box without changing its aspect ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputSizeConstraintsMicrometers {
    max_width: Option<i64>,
    max_height: Option<i64>,
}

impl OutputSizeConstraintsMicrometers {
    /// Creates validated output-size constraints.
    pub fn new(max_width: Option<i64>, max_height: Option<i64>) -> Result<Self, DocumentError> {
        if max_width.is_none() && max_height.is_none() {
            return Err(DocumentError::MissingOutputSizeConstraint);
        }
        if let Some(micrometers) = max_width {
            if micrometers <= 0 {
                return Err(DocumentError::NonPositivePhysicalDimension {
                    dimension: Dimension::Width,
                    micrometers,
                });
            }
        }
        if let Some(micrometers) = max_height {
            if micrometers <= 0 {
                return Err(DocumentError::NonPositivePhysicalDimension {
                    dimension: Dimension::Height,
                    micrometers,
                });
            }
        }
        Ok(Self {
            max_width,
            max_height,
        })
    }

    /// Returns the maximum width, if one was supplied.
    #[must_use]
    pub const fn max_width(self) -> Option<i64> {
        self.max_width
    }

    /// Returns the maximum height, if one was supplied.
    #[must_use]
    pub const fn max_height(self) -> Option<i64> {
        self.max_height
    }
}

/// A positive maximum physical sampling pitch in integer micrometres per pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamplingPitchMicrometers(i64);

impl SamplingPitchMicrometers {
    /// Creates a positive sampling pitch.
    pub fn new(micrometers: i64) -> Result<Self, DocumentError> {
        if micrometers <= 0 {
            return Err(DocumentError::NonPositiveSamplingPitch { micrometers });
        }
        Ok(Self(micrometers))
    }

    /// Returns the maximum number of micrometres represented by one pixel.
    #[must_use]
    pub const fn micrometers(self) -> i64 {
        self.0
    }
}

/// Deterministic physical and pixel dimensions for one rasterization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RasterizationPlan {
    physical_size: PhysicalSizeMicrometers,
    pixel_dimensions: PixelDimensions,
}

impl RasterizationPlan {
    /// Fits a source page into the output constraints and selects a pixel grid.
    ///
    /// The limiting physical dimension is used exactly. The other dimension is
    /// rounded to the nearest micrometre, with positive half ties rounded up.
    /// Each pixel dimension is then rounded up independently so its effective
    /// sampling pitch is never coarser than `sampling_pitch`.
    pub fn fit(
        source_size: PhysicalSizeMicrometers,
        constraints: OutputSizeConstraintsMicrometers,
        sampling_pitch: SamplingPitchMicrometers,
    ) -> Result<Self, DocumentError> {
        let physical_size = fit_physical_size(source_size, constraints)?;
        let pitch = positive_i64_as_u128(sampling_pitch.micrometers());
        let width_pixels = divide_ceil(positive_i64_as_u128(physical_size.width()), pitch);
        let height_pixels = divide_ceil(positive_i64_as_u128(physical_size.height()), pitch);

        let width =
            u32::try_from(width_pixels).map_err(|_| DocumentError::PixelDimensionTooLarge {
                dimension: Dimension::Width,
                pixels: width_pixels,
            })?;
        let height =
            u32::try_from(height_pixels).map_err(|_| DocumentError::PixelDimensionTooLarge {
                dimension: Dimension::Height,
                pixels: height_pixels,
            })?;
        let pixel_dimensions = PixelDimensions::new(width, height)?;

        Ok(Self {
            physical_size,
            pixel_dimensions,
        })
    }

    /// Returns the fitted physical output dimensions.
    #[must_use]
    pub const fn physical_size(self) -> PhysicalSizeMicrometers {
        self.physical_size
    }

    /// Returns the raster dimensions required for the configured sampling pitch.
    #[must_use]
    pub const fn pixel_dimensions(self) -> PixelDimensions {
        self.pixel_dimensions
    }
}

fn fit_physical_size(
    source_size: PhysicalSizeMicrometers,
    constraints: OutputSizeConstraintsMicrometers,
) -> Result<PhysicalSizeMicrometers, DocumentError> {
    let source_width = positive_i64_as_u128(source_size.width());
    let source_height = positive_i64_as_u128(source_size.height());
    let max_width = constraints.max_width().map(positive_i64_as_u128);
    let max_height = constraints.max_height().map(positive_i64_as_u128);

    let (width, height) = match (max_width, max_height) {
        (Some(width), None) => (
            width,
            divide_round_half_up(source_height * width, source_width),
        ),
        (None, Some(height)) => (
            divide_round_half_up(source_width * height, source_height),
            height,
        ),
        (Some(width), Some(height)) if width * source_height <= height * source_width => (
            width,
            divide_round_half_up(source_height * width, source_width),
        ),
        (Some(_), Some(height)) => (
            divide_round_half_up(source_width * height, source_height),
            height,
        ),
        (None, None) => return Err(DocumentError::MissingOutputSizeConstraint),
    };

    let width = output_dimension(width, Dimension::Width)?;
    let height = output_dimension(height, Dimension::Height)?;
    PhysicalSizeMicrometers::new(width, height)
}

fn output_dimension(value: u128, dimension: Dimension) -> Result<i64, DocumentError> {
    if value == 0 {
        return Err(DocumentError::OutputDimensionRoundsToZero { dimension });
    }
    i64::try_from(value).map_err(|_| DocumentError::OutputDimensionTooLarge { dimension })
}

fn positive_i64_as_u128(value: i64) -> u128 {
    u128::try_from(value).expect("validated physical values are positive")
}

fn divide_round_half_up(numerator: u128, denominator: u128) -> u128 {
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    let half_rounded_up = denominator / 2 + denominator % 2;
    quotient + u128::from(remainder >= half_rounded_up)
}

fn divide_ceil(numerator: u128, denominator: u128) -> u128 {
    let quotient = numerator / denominator;
    quotient + u128::from(numerator % denominator != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_RASTER_PIXELS;

    fn physical_size(width: i64, height: i64) -> PhysicalSizeMicrometers {
        PhysicalSizeMicrometers::new(width, height).expect("test size is positive")
    }

    fn constraints(
        max_width: Option<i64>,
        max_height: Option<i64>,
    ) -> OutputSizeConstraintsMicrometers {
        OutputSizeConstraintsMicrometers::new(max_width, max_height)
            .expect("test constraints are valid")
    }

    fn pitch(micrometers: i64) -> SamplingPitchMicrometers {
        SamplingPitchMicrometers::new(micrometers).expect("test pitch is positive")
    }

    fn plan_for_same_size(
        width: i64,
        height: i64,
        pitch_micrometers: i64,
    ) -> Result<RasterizationPlan, DocumentError> {
        RasterizationPlan::fit(
            physical_size(width, height),
            constraints(Some(width), Some(height)),
            pitch(pitch_micrometers),
        )
    }

    #[test]
    fn output_constraints_require_one_positive_dimension() {
        assert_eq!(
            OutputSizeConstraintsMicrometers::new(None, None),
            Err(DocumentError::MissingOutputSizeConstraint)
        );
        assert_eq!(
            OutputSizeConstraintsMicrometers::new(Some(0), None),
            Err(DocumentError::NonPositivePhysicalDimension {
                dimension: Dimension::Width,
                micrometers: 0,
            })
        );
        assert_eq!(
            OutputSizeConstraintsMicrometers::new(Some(1), Some(-1)),
            Err(DocumentError::NonPositivePhysicalDimension {
                dimension: Dimension::Height,
                micrometers: -1,
            })
        );

        let both = constraints(Some(10), Some(20));
        assert_eq!(both.max_width(), Some(10));
        assert_eq!(both.max_height(), Some(20));
    }

    #[test]
    fn sampling_pitch_must_be_positive() {
        assert_eq!(
            SamplingPitchMicrometers::new(0),
            Err(DocumentError::NonPositiveSamplingPitch { micrometers: 0 })
        );
        assert_eq!(
            SamplingPitchMicrometers::new(-1),
            Err(DocumentError::NonPositiveSamplingPitch { micrometers: -1 })
        );
        assert_eq!(pitch(40).micrometers(), 40);
    }

    #[test]
    fn one_constraint_sets_that_dimension_exactly() {
        let width_limited = RasterizationPlan::fit(
            physical_size(200_000, 100_000),
            constraints(Some(100_000), None),
            pitch(40),
        )
        .expect("width-limited plan fits");
        assert_eq!(
            width_limited.physical_size(),
            physical_size(100_000, 50_000)
        );

        let height_limited = RasterizationPlan::fit(
            physical_size(200_000, 100_000),
            constraints(None, Some(75_000)),
            pitch(40),
        )
        .expect("height-limited plan fits");
        assert_eq!(
            height_limited.physical_size(),
            physical_size(150_000, 75_000)
        );
    }

    #[test]
    fn two_constraints_select_the_tighter_scale() {
        let width_limited = RasterizationPlan::fit(
            physical_size(4_000, 3_000),
            constraints(Some(100_000), Some(100_000)),
            pitch(40),
        )
        .expect("width is limiting");
        assert_eq!(
            width_limited.physical_size(),
            physical_size(100_000, 75_000)
        );

        let height_limited = RasterizationPlan::fit(
            physical_size(4_000, 3_000),
            constraints(Some(100_000), Some(50_000)),
            pitch(40),
        )
        .expect("height is limiting");
        assert_eq!(
            height_limited.physical_size(),
            physical_size(66_667, 50_000)
        );

        let exact_tie = RasterizationPlan::fit(
            physical_size(4_000, 2_000),
            constraints(Some(100_000), Some(50_000)),
            pitch(40),
        )
        .expect("matching aspect ratio fits exactly");
        assert_eq!(exact_tie.physical_size(), physical_size(100_000, 50_000));
    }

    #[test]
    fn derived_physical_dimension_uses_half_up_rounding() {
        let plan =
            RasterizationPlan::fit(physical_size(2, 1), constraints(Some(1), None), pitch(1))
                .expect("half a micrometre rounds up");
        assert_eq!(plan.physical_size(), physical_size(1, 1));
    }

    #[test]
    fn unrepresentably_thin_output_is_rejected_instead_of_clamped() {
        assert_eq!(
            RasterizationPlan::fit(physical_size(3, 1), constraints(Some(1), None), pitch(1),),
            Err(DocumentError::OutputDimensionRoundsToZero {
                dimension: Dimension::Height,
            })
        );
        assert_eq!(
            RasterizationPlan::fit(physical_size(1, 3), constraints(None, Some(1)), pitch(1),),
            Err(DocumentError::OutputDimensionRoundsToZero {
                dimension: Dimension::Width,
            })
        );
    }

    #[test]
    fn derived_physical_dimension_overflow_is_reported() {
        assert_eq!(
            RasterizationPlan::fit(
                physical_size(1, i64::MAX),
                constraints(Some(i64::MAX), None),
                pitch(i64::MAX),
            ),
            Err(DocumentError::OutputDimensionTooLarge {
                dimension: Dimension::Height,
            })
        );
        assert_eq!(
            RasterizationPlan::fit(
                physical_size(i64::MAX, 1),
                constraints(None, Some(i64::MAX)),
                pitch(i64::MAX),
            ),
            Err(DocumentError::OutputDimensionTooLarge {
                dimension: Dimension::Width,
            })
        );
    }

    #[test]
    fn pixel_dimensions_use_ceiling_division() {
        let plan = plan_for_same_size(100_001, 50_000, 40).expect("raster is within limits");
        assert_eq!(plan.pixel_dimensions().width(), 2_501);
        assert_eq!(plan.pixel_dimensions().height(), 1_250);

        let subpixel_extent = plan_for_same_size(1, 1, 40).expect("one pixel is sufficient");
        assert_eq!(subpixel_extent.pixel_dimensions().width(), 1);
        assert_eq!(subpixel_extent.pixel_dimensions().height(), 1);
    }

    #[test]
    fn familiar_page_size_has_stable_output() {
        let plan = RasterizationPlan::fit(
            physical_size(210_000, 297_000),
            constraints(Some(100_000), None),
            pitch(40),
        )
        .expect("default-scale A4 page is practical");

        assert_eq!(plan.physical_size(), physical_size(100_000, 141_429));
        assert_eq!(plan.pixel_dimensions().width(), 2_500);
        assert_eq!(plan.pixel_dimensions().height(), 3_536);
    }

    #[test]
    fn pixel_axis_overflow_is_reported_before_narrowing() {
        let too_wide = i64::from(u32::MAX) + 1;
        assert_eq!(
            plan_for_same_size(too_wide, 1, 1),
            Err(DocumentError::PixelDimensionTooLarge {
                dimension: Dimension::Width,
                pixels: u128::from(u32::MAX) + 1,
            })
        );

        let too_tall = i64::from(u32::MAX) + 1;
        assert_eq!(
            plan_for_same_size(1, too_tall, 1),
            Err(DocumentError::PixelDimensionTooLarge {
                dimension: Dimension::Height,
                pixels: u128::from(u32::MAX) + 1,
            })
        );
    }

    #[test]
    fn raster_resource_limit_is_enforced_at_its_exact_boundary() {
        let accepted =
            plan_for_same_size(16_384, 16_384, 1).expect("the exact pixel-count limit is accepted");
        assert_eq!(
            accepted.pixel_dimensions().pixel_count(),
            MAX_RASTER_PIXELS as usize
        );

        assert_eq!(
            plan_for_same_size(16_385, 16_384, 1),
            Err(DocumentError::RasterTooLarge {
                width: 16_385,
                height: 16_384,
                max_pixels: MAX_RASTER_PIXELS,
            })
        );
    }

    #[test]
    fn aspect_fit_invariants_hold_for_small_integer_inputs() {
        for source_width in 1..=12 {
            for source_height in 1..=12 {
                for max_width in 1..=12 {
                    for max_height in 1..=12 {
                        let result = RasterizationPlan::fit(
                            physical_size(source_width, source_height),
                            constraints(Some(max_width), Some(max_height)),
                            pitch(1),
                        );
                        match result {
                            Ok(plan) => {
                                let output = plan.physical_size();
                                assert!(output.width() <= max_width);
                                assert!(output.height() <= max_height);
                                assert!(
                                    output.width() == max_width || output.height() == max_height
                                );
                            }
                            Err(DocumentError::OutputDimensionRoundsToZero { .. }) => {}
                            Err(error) => panic!("unexpected sizing failure: {error}"),
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn integer_helpers_cover_exact_fractional_and_extreme_values() {
        assert_eq!(divide_round_half_up(10, 4), 3);
        assert_eq!(divide_round_half_up(9, 4), 2);
        assert_eq!(divide_round_half_up(u128::MAX - 1, u128::MAX), 1);
        assert_eq!(divide_ceil(8, 4), 2);
        assert_eq!(divide_ceil(9, 4), 3);
        assert_eq!(divide_ceil(u128::MAX, u128::MAX), 1);
    }
}
