//! Pdfium-backed rendering into canonical artwork.
//!
//! This adapter deliberately keeps dynamic-library discovery explicit. It also
//! normalizes Pdfium's bitmap output to straight-alpha RGBA before handing it
//! to the renderer-independent document core.

use crate::{
    CanonicalArtwork, Dimension, DocumentError, ForegroundSelection,
    OutputSizeConstraintsMicrometers, PhysicalSizeMicrometers, RasterizationPlan,
    SamplingPitchMicrometers, rgba_over_white_to_grayscale,
};
use pdfium_render::prelude::{
    PdfBitmapFormat, PdfColor, PdfPageIndex, PdfPoints, PdfRenderConfig, Pdfium, PdfiumError,
    Pixels,
};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::{error::Error, fmt};

const PDF_POINTS_PER_INCH: f64 = 72.0;
const MICROMETERS_PER_INCH: f64 = 25_400.0;
const REQUIRED_PAGE_COUNT: PdfPageIndex = 1;

/// Renderer settings that are independent of a particular Pdfium library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PdfRenderOptions {
    output_size_constraints: OutputSizeConstraintsMicrometers,
    sampling_pitch: SamplingPitchMicrometers,
    foreground_selection: ForegroundSelection,
}

impl PdfRenderOptions {
    /// Creates explicit PDF sizing, sampling, and foreground settings.
    #[must_use]
    pub const fn new(
        output_size_constraints: OutputSizeConstraintsMicrometers,
        sampling_pitch: SamplingPitchMicrometers,
        foreground_selection: ForegroundSelection,
    ) -> Self {
        Self {
            output_size_constraints,
            sampling_pitch,
            foreground_selection,
        }
    }

    /// Returns the maximum output extents applied while preserving aspect ratio.
    #[must_use]
    pub const fn output_size_constraints(self) -> OutputSizeConstraintsMicrometers {
        self.output_size_constraints
    }

    /// Returns the requested physical sampling pitch.
    #[must_use]
    pub const fn sampling_pitch(self) -> SamplingPitchMicrometers {
        self.sampling_pitch
    }

    /// Returns the grayscale-to-foreground classification rule.
    #[must_use]
    pub const fn foreground_selection(self) -> ForegroundSelection {
        self.foreground_selection
    }
}

/// A renderer backed by an explicitly initialized Pdfium instance.
///
/// Pdfium permits one initialization per process. Create the renderer during
/// serialized startup and share it; attempting to initialize another
/// [`Pdfium`] instance will panic inside `pdfium-render`.
#[derive(Debug)]
pub struct PdfiumRenderer {
    pdfium: Pdfium,
}

impl PdfiumRenderer {
    /// Loads the platform Pdfium dynamic library from `directory`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn bind_to_directory(directory: impl AsRef<Path>) -> Result<Self, PdfRenderError> {
        let library_path = Pdfium::pdfium_platform_library_name_at_path(directory.as_ref());
        let bindings = Pdfium::bind_to_library(library_path).map_err(PdfRenderError::Binding)?;

        Ok(Self::from_pdfium(Pdfium::new(bindings)))
    }

    /// Loads Pdfium through the platform's system library search mechanism.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn bind_to_system_library() -> Result<Self, PdfRenderError> {
        let bindings = Pdfium::bind_to_system_library().map_err(PdfRenderError::Binding)?;

        Ok(Self::from_pdfium(Pdfium::new(bindings)))
    }

    /// Uses an already initialized Pdfium instance.
    #[must_use]
    pub const fn from_pdfium(pdfium: Pdfium) -> Self {
        Self { pdfium }
    }

    /// Returns the underlying Pdfium instance.
    #[must_use]
    pub const fn pdfium(&self) -> &Pdfium {
        &self.pdfium
    }

    /// Renders an exactly-one-page PDF byte slice into canonical artwork.
    pub fn render_pdf_bytes(
        &self,
        bytes: &[u8],
        options: PdfRenderOptions,
    ) -> Result<CanonicalArtwork, PdfRenderError> {
        let document = self
            .pdfium
            .load_pdf_from_byte_slice(bytes, None)
            .map_err(PdfRenderError::Loading)?;
        let pages = document.pages();
        let page_count = pages.len();

        if page_count != REQUIRED_PAGE_COUNT {
            return Err(PdfRenderError::UnexpectedPageCount {
                expected: REQUIRED_PAGE_COUNT,
                actual: page_count,
            });
        }

        let page = pages.get(0).map_err(PdfRenderError::PageAccess)?;
        let source_size = physical_size_from_page_points(page.width(), page.height())?;
        let plan = RasterizationPlan::fit(
            source_size,
            options.output_size_constraints,
            options.sampling_pitch,
        )
        .map_err(PdfRenderError::Document)?;
        let planned = plan.pixel_dimensions();
        let render_width = pixel_dimension_for_pdfium(planned.width(), Dimension::Width)?;
        let render_height = pixel_dimension_for_pdfium(planned.height(), Dimension::Height)?;
        let config = deterministic_render_config(render_width, render_height);
        let bitmap = page
            .render_with_config(&config)
            .map_err(PdfRenderError::Rendering)?;

        validate_rendered_dimensions(&plan, bitmap.width(), bitmap.height())?;

        canonical_artwork_from_rgba(
            &plan,
            options.foreground_selection,
            bitmap.as_rgba_bytes().as_slice(),
        )
    }
}

impl From<Pdfium> for PdfiumRenderer {
    fn from(pdfium: Pdfium) -> Self {
        Self::from_pdfium(pdfium)
    }
}

/// A failure while binding Pdfium or rendering a PDF into canonical artwork.
#[derive(Debug)]
#[non_exhaustive]
pub enum PdfRenderError {
    /// Pdfium's dynamic library could not be loaded.
    Binding(PdfiumError),
    /// Pdfium could not parse or open the supplied PDF bytes.
    Loading(PdfiumError),
    /// The PDF must contain exactly one page.
    UnexpectedPageCount {
        /// Required number of pages.
        expected: PdfPageIndex,
        /// Number of pages in the supplied document.
        actual: PdfPageIndex,
    },
    /// Pdfium could not open the sole page after counting it.
    PageAccess(PdfiumError),
    /// Pdfium reported a non-positive, non-finite, or unrepresentable page extent.
    InvalidPageDimension {
        /// Invalid axis.
        dimension: Dimension,
        /// Extent reported by Pdfium, in PDF points.
        points: f32,
    },
    /// A planned pixel extent cannot be passed to Pdfium's signed pixel API.
    PixelDimensionOutOfRange {
        /// Invalid axis.
        dimension: Dimension,
        /// Planned extent.
        pixels: u32,
    },
    /// Pdfium failed while rendering the page.
    Rendering(PdfiumError),
    /// Pdfium returned a bitmap whose extents differ from the fixed render plan.
    RenderedDimensionsMismatch {
        /// Planned width.
        expected_width: u32,
        /// Planned height.
        expected_height: u32,
        /// Actual Pdfium bitmap width.
        actual_width: Pixels,
        /// Actual Pdfium bitmap height.
        actual_height: Pixels,
    },
    /// Renderer-independent document validation failed.
    Document(DocumentError),
}

impl fmt::Display for PdfRenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Binding(error) => write!(formatter, "could not bind to Pdfium: {error}"),
            Self::Loading(error) => write!(formatter, "could not load PDF bytes: {error}"),
            Self::UnexpectedPageCount { expected, actual } => write!(
                formatter,
                "PDF contains {actual} pages; expected exactly {expected}"
            ),
            Self::PageAccess(error) => write!(formatter, "could not open PDF page: {error}"),
            Self::InvalidPageDimension { dimension, points } => write!(
                formatter,
                "PDF page {dimension} is invalid or unrepresentable: {points} points"
            ),
            Self::PixelDimensionOutOfRange { dimension, pixels } => write!(
                formatter,
                "planned pixel {dimension} {pixels} exceeds Pdfium's signed pixel range"
            ),
            Self::Rendering(error) => write!(formatter, "could not render PDF page: {error}"),
            Self::RenderedDimensionsMismatch {
                expected_width,
                expected_height,
                actual_width,
                actual_height,
            } => write!(
                formatter,
                "Pdfium returned {actual_width}x{actual_height} pixels; expected {expected_width}x{expected_height}"
            ),
            Self::Document(error) => write!(formatter, "invalid canonical document: {error}"),
        }
    }
}

impl Error for PdfRenderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Binding(error)
            | Self::Loading(error)
            | Self::PageAccess(error)
            | Self::Rendering(error) => Some(error),
            Self::Document(error) => Some(error),
            Self::UnexpectedPageCount { .. }
            | Self::InvalidPageDimension { .. }
            | Self::PixelDimensionOutOfRange { .. }
            | Self::RenderedDimensionsMismatch { .. } => None,
        }
    }
}

impl From<DocumentError> for PdfRenderError {
    fn from(error: DocumentError) -> Self {
        Self::Document(error)
    }
}

fn physical_size_from_page_points(
    width: PdfPoints,
    height: PdfPoints,
) -> Result<PhysicalSizeMicrometers, PdfRenderError> {
    PhysicalSizeMicrometers::new(
        point_dimension_to_micrometers(width, Dimension::Width)?,
        point_dimension_to_micrometers(height, Dimension::Height)?,
    )
    .map_err(PdfRenderError::Document)
}

fn point_dimension_to_micrometers(
    points: PdfPoints,
    dimension: Dimension,
) -> Result<i64, PdfRenderError> {
    let micrometers = f64::from(points.value) * MICROMETERS_PER_INCH / PDF_POINTS_PER_INCH;
    let rounded = micrometers.round();

    if !rounded.is_finite() || rounded < 1.0 || rounded >= i64::MAX as f64 {
        return Err(PdfRenderError::InvalidPageDimension {
            dimension,
            points: points.value,
        });
    }

    Ok(rounded as i64)
}

fn pixel_dimension_for_pdfium(pixels: u32, dimension: Dimension) -> Result<Pixels, PdfRenderError> {
    Pixels::try_from(pixels)
        .map_err(|_| PdfRenderError::PixelDimensionOutOfRange { dimension, pixels })
}

fn deterministic_render_config(width: Pixels, height: Pixels) -> PdfRenderConfig {
    PdfRenderConfig::new()
        .set_fixed_size(width, height)
        .set_format(PdfBitmapFormat::BGRA)
        .clear_before_rendering(true)
        .set_clear_color(PdfColor::new(255, 255, 255, 0))
        .render_form_data(true)
        .render_annotations(true)
        .use_lcd_text_rendering(false)
        .disable_native_text_rendering(true)
        .use_grayscale_rendering(false)
        .limit_render_image_cache_size(true)
        .force_half_tone(false)
        .use_print_quality(true)
        .set_text_smoothing(true)
        .set_image_smoothing(true)
        .set_path_smoothing(true)
        .set_reverse_byte_order(true)
        .render_fills_as_strokes(false)
}

fn validate_rendered_dimensions(
    plan: &RasterizationPlan,
    actual_width: Pixels,
    actual_height: Pixels,
) -> Result<(), PdfRenderError> {
    let expected = plan.pixel_dimensions();
    let expected_width = pixel_dimension_for_pdfium(expected.width(), Dimension::Width)?;
    let expected_height = pixel_dimension_for_pdfium(expected.height(), Dimension::Height)?;

    if actual_width != expected_width || actual_height != expected_height {
        return Err(PdfRenderError::RenderedDimensionsMismatch {
            expected_width: expected.width(),
            expected_height: expected.height(),
            actual_width,
            actual_height,
        });
    }

    Ok(())
}

fn canonical_artwork_from_rgba(
    plan: &RasterizationPlan,
    selection: ForegroundSelection,
    rgba: &[u8],
) -> Result<CanonicalArtwork, PdfRenderError> {
    let dimensions = plan.pixel_dimensions();
    let grayscale = rgba_over_white_to_grayscale(dimensions.width(), dimensions.height(), rgba)
        .map_err(PdfRenderError::Document)?;

    Ok(CanonicalArtwork::from_grayscale(
        plan.physical_size(),
        &grayscale,
        selection,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_plan() -> RasterizationPlan {
        RasterizationPlan::fit(
            PhysicalSizeMicrometers::new(200, 100).expect("valid source size"),
            OutputSizeConstraintsMicrometers::new(Some(200), Some(100)).expect("valid constraints"),
            SamplingPitchMicrometers::new(100).expect("valid sampling pitch"),
        )
        .expect("valid rasterization plan")
    }

    #[test]
    fn options_preserve_all_explicit_policy() {
        let constraints = OutputSizeConstraintsMicrometers::new(Some(100_000), Some(50_000))
            .expect("valid constraints");
        let pitch = SamplingPitchMicrometers::new(200).expect("valid pitch");
        let foreground = ForegroundSelection::new(200, true);
        let options = PdfRenderOptions::new(constraints, pitch, foreground);

        assert_eq!(options.output_size_constraints(), constraints);
        assert_eq!(options.sampling_pitch(), pitch);
        assert_eq!(options.foreground_selection(), foreground);
    }

    #[test]
    fn pdf_points_convert_to_integer_micrometers() {
        assert_eq!(
            point_dimension_to_micrometers(PdfPoints::new(72.0), Dimension::Width)
                .expect("one inch is representable"),
            25_400
        );
        assert_eq!(
            point_dimension_to_micrometers(PdfPoints::new(612.0), Dimension::Height)
                .expect("eight and a half inches is representable"),
            215_900
        );
    }

    #[test]
    fn invalid_pdf_point_dimensions_are_typed() {
        for points in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(matches!(
                point_dimension_to_micrometers(PdfPoints::new(points), Dimension::Width),
                Err(PdfRenderError::InvalidPageDimension {
                    dimension: Dimension::Width,
                    ..
                })
            ));
        }
    }

    #[test]
    fn pdfium_pixel_conversion_checks_signed_range() {
        assert_eq!(
            pixel_dimension_for_pdfium(2_147_483_647, Dimension::Width).expect("i32::MAX is valid"),
            i32::MAX
        );
        assert!(matches!(
            pixel_dimension_for_pdfium(2_147_483_648, Dimension::Height),
            Err(PdfRenderError::PixelDimensionOutOfRange {
                dimension: Dimension::Height,
                pixels: 2_147_483_648
            })
        ));
    }

    #[test]
    fn rendered_dimensions_must_equal_the_fixed_plan() {
        let plan = sample_plan();

        validate_rendered_dimensions(&plan, 2, 1).expect("planned dimensions match");
        assert!(matches!(
            validate_rendered_dimensions(&plan, 1, 2),
            Err(PdfRenderError::RenderedDimensionsMismatch {
                expected_width: 2,
                expected_height: 1,
                actual_width: 1,
                actual_height: 2
            })
        ));
    }

    #[test]
    fn normalized_rgba_is_composited_and_thresholded_without_pdfium() {
        let plan = sample_plan();
        let artwork = canonical_artwork_from_rgba(
            &plan,
            ForegroundSelection::new(255, false),
            &[
                0, 0, 0, 255, // opaque black
                0, 0, 0, 0, // transparent hidden black becomes white
            ],
        )
        .expect("valid normalized RGBA");

        assert_eq!(artwork.physical_size(), plan.physical_size());
        assert_eq!(artwork.foreground().as_bytes(), &[1, 0]);
    }

    #[test]
    fn normalized_rgba_length_errors_remain_typed_document_errors() {
        let error = canonical_artwork_from_rgba(
            &sample_plan(),
            ForegroundSelection::new(255, false),
            &[0; 7],
        )
        .expect_err("seven bytes cannot describe two RGBA pixels");

        assert!(matches!(
            error,
            PdfRenderError::Document(DocumentError::RgbaDataLengthMismatch {
                expected: 8,
                actual: 7
            })
        ));
    }
}
