//! End-to-end Plaque3MF pipeline orchestration.

use plaque3mf_document::{
    DocumentError, ForegroundSelection, OutputSizeConstraintsMicrometers, SamplingPitchMicrometers,
};
use std::{error::Error, fmt};

pub use plaque3mf_job_spec::{
    GeometryOptions, JobSpec, NormalizedGeometryOptions, NormalizedJobSpec,
    NormalizedRasterOptions, NormalizedSizeConstraints, RasterOptions, SizeConstraints,
    ValidationErrors, ValidationIssue,
};

/// Validated document-rendering policy derived from a job specification.
///
/// This request is renderer-independent and remains available when the
/// feature-gated Pdfium adapter is disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentRenderRequest {
    output_size_constraints: OutputSizeConstraintsMicrometers,
    sampling_pitch: SamplingPitchMicrometers,
    foreground_selection: ForegroundSelection,
}

impl DocumentRenderRequest {
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

    fn from_normalized(spec: &NormalizedJobSpec) -> Result<Self, DocumentError> {
        let size = spec.size();
        let raster = spec.raster();
        let output_size_constraints = OutputSizeConstraintsMicrometers::new(
            size.max_width_micrometers(),
            size.max_height_micrometers(),
        )?;
        let sampling_pitch = SamplingPitchMicrometers::new(raster.sampling_pitch_micrometers())?;
        let foreground_selection =
            ForegroundSelection::new(raster.threshold(), raster.invert_foreground());

        Ok(Self {
            output_size_constraints,
            sampling_pitch,
            foreground_selection,
        })
    }
}

impl TryFrom<&NormalizedJobSpec> for DocumentRenderRequest {
    type Error = EngineError;

    fn try_from(spec: &NormalizedJobSpec) -> Result<Self, Self::Error> {
        Self::from_normalized(spec).map_err(EngineError::InvalidDocumentOptions)
    }
}

impl TryFrom<&JobSpec> for DocumentRenderRequest {
    type Error = EngineError;

    fn try_from(spec: &JobSpec) -> Result<Self, Self::Error> {
        let normalized = spec.normalize().map_err(EngineError::InvalidJobSpec)?;
        Self::try_from(&normalized)
    }
}

#[cfg(feature = "pdfium")]
impl From<DocumentRenderRequest> for plaque3mf_document::pdfium::PdfRenderOptions {
    fn from(request: DocumentRenderRequest) -> Self {
        Self::new(
            request.output_size_constraints,
            request.sampling_pitch,
            request.foreground_selection,
        )
    }
}

/// A failure while validating or preparing an end-to-end conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EngineError {
    /// The supplied job specification failed validation.
    InvalidJobSpec(ValidationErrors),
    /// Validated settings could not be represented by the document layer.
    InvalidDocumentOptions(DocumentError),
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJobSpec(error) => write!(formatter, "invalid job specification: {error}"),
            Self::InvalidDocumentOptions(error) => {
                write!(formatter, "invalid document rendering options: {error}")
            }
        }
    }
}

impl Error for EngineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidJobSpec(error) => Some(error),
            Self::InvalidDocumentOptions(error) => Some(error),
        }
    }
}

impl From<ValidationErrors> for EngineError {
    fn from(error: ValidationErrors) -> Self {
        Self::InvalidJobSpec(error)
    }
}

impl From<DocumentError> for EngineError {
    fn from(error: DocumentError) -> Self {
        Self::InvalidDocumentOptions(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_job_maps_to_document_request() {
        let request =
            DocumentRenderRequest::try_from(&JobSpec::default()).expect("default job is valid");

        assert_eq!(request.output_size_constraints().max_width(), Some(100_000));
        assert_eq!(request.output_size_constraints().max_height(), None);
        assert_eq!(request.sampling_pitch().micrometers(), 40);
        assert_eq!(request.foreground_selection().threshold(), 255);
        assert!(!request.foreground_selection().invert());
    }

    #[test]
    fn all_document_settings_map_exactly() {
        let mut spec = JobSpec::default();
        spec.size.max_width_mm = Some(80.0);
        spec.size.max_height_mm = Some(60.0);
        spec.raster.mm_per_pixel = 0.025;
        spec.raster.threshold = 127;
        spec.raster.invert_foreground = true;

        let request = DocumentRenderRequest::try_from(&spec).expect("custom job is valid");

        assert_eq!(request.output_size_constraints().max_width(), Some(80_000));
        assert_eq!(request.output_size_constraints().max_height(), Some(60_000));
        assert_eq!(request.sampling_pitch().micrometers(), 25);
        assert_eq!(request.foreground_selection().threshold(), 127);
        assert!(request.foreground_selection().invert());
    }

    #[test]
    fn normalized_snapshot_can_be_reused_without_rereading_the_source() {
        let mut spec = JobSpec::default();
        let normalized = spec.normalize().expect("default job normalizes");

        spec.size.max_width_mm = Some(25.0);
        spec.raster.mm_per_pixel = 0.020;
        spec.raster.threshold = 10;

        assert_eq!(spec.size.max_width_mm, Some(25.0));
        assert_eq!(spec.raster.mm_per_pixel, 0.020);
        assert_eq!(spec.raster.threshold, 10);

        let request =
            DocumentRenderRequest::try_from(&normalized).expect("normalized job remains valid");

        assert_eq!(request.output_size_constraints().max_width(), Some(100_000));
        assert_eq!(request.sampling_pitch().micrometers(), 40);
        assert_eq!(request.foreground_selection().threshold(), 255);
    }

    #[test]
    fn invalid_job_preserves_the_complete_issue_list() {
        let mut spec = JobSpec::default();
        spec.size.max_width_mm = None;
        spec.size.max_height_mm = None;
        spec.raster.mm_per_pixel = 0.0;

        let error =
            DocumentRenderRequest::try_from(&spec).expect_err("invalid job must be rejected");

        assert!(matches!(
            error,
            EngineError::InvalidJobSpec(ref errors)
                if errors.issues()
                    == [
                        ValidationIssue::MissingSizeConstraint,
                        ValidationIssue::MustBePositive {
                            field: "raster.mm_per_pixel"
                        }
                    ]
        ));
    }

    #[test]
    fn post_normalization_invalid_job_is_rejected() {
        let mut spec = JobSpec::default();
        spec.geometry.minimum_feature_width_mm = 1.001_2;
        spec.raster.mm_per_pixel = 0.500_6;

        let error = DocumentRenderRequest::try_from(&spec)
            .expect_err("fixed-point resolution constraint must be enforced");

        assert!(matches!(
            error,
            EngineError::InvalidJobSpec(ref errors)
                if errors.issues() == [ValidationIssue::RasterResolutionTooCoarse]
        ));
    }

    #[test]
    fn typed_constructor_errors_map_to_engine_errors() {
        let source = DocumentError::MissingOutputSizeConstraint;

        assert_eq!(
            EngineError::from(source.clone()),
            EngineError::InvalidDocumentOptions(source)
        );
    }

    #[cfg(feature = "pdfium")]
    #[test]
    fn document_request_converts_to_pdf_render_options() {
        use plaque3mf_document::pdfium::PdfRenderOptions;

        let request =
            DocumentRenderRequest::try_from(&JobSpec::default()).expect("default job is valid");
        let options = PdfRenderOptions::from(request);

        assert_eq!(
            options.output_size_constraints(),
            request.output_size_constraints()
        );
        assert_eq!(options.sampling_pitch(), request.sampling_pitch());
        assert_eq!(
            options.foreground_selection(),
            request.foreground_selection()
        );
    }
}
