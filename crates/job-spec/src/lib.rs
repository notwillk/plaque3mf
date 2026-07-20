//! Versioned job settings, defaults, and validation.

use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

/// The schema version implemented by this crate.
pub const JOB_SPEC_VERSION: u32 = 1;

/// Default maximum output width for version 1, in millimetres.
pub const DEFAULT_MAX_WIDTH_MM: f64 = 100.0;
/// Default backing thickness for version 1, in millimetres.
pub const DEFAULT_BACKING_THICKNESS_MM: f64 = 1.2;
/// Default total thickness for version 1, in millimetres.
pub const DEFAULT_TOTAL_THICKNESS_MM: f64 = 2.0;
/// Default border width for version 1, in millimetres.
pub const DEFAULT_BORDER_WIDTH_MM: f64 = 2.0;
/// Default minimum printable feature width for version 1, in millimetres.
pub const DEFAULT_MINIMUM_FEATURE_WIDTH_MM: f64 = 0.4;
/// Default contour simplification tolerance for version 1, in millimetres.
pub const DEFAULT_CONTOUR_TOLERANCE_MM: f64 = 0.05;
/// Default raster sampling pitch for version 1, in millimetres per pixel.
pub const DEFAULT_MM_PER_PIXEL: f64 = 0.04;
/// Default grayscale threshold for version 1.
pub const DEFAULT_RASTER_THRESHOLD: u8 = 255;

const MICROMETERS_PER_MILLIMETER: f64 = 1_000.0;
const I64_MICROMETER_UPPER_EXCLUSIVE: f64 = 9_223_372_036_854_775_808.0;

/// A complete, serializable set of settings for one Plaque3MF conversion.
///
/// Input and output paths are orchestration concerns and intentionally do not
/// belong to this data model. `schema_version` is required when deserializing;
/// omitted settings within that version receive the frozen version 1 defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobSpec {
    schema_version: u32,
    /// Maximum physical output dimensions.
    #[serde(default)]
    pub size: SizeConstraints,
    /// Printable geometry settings.
    #[serde(default)]
    pub geometry: GeometryOptions,
    /// Rasterization and foreground extraction settings.
    #[serde(default)]
    pub raster: RasterOptions,
}

impl JobSpec {
    /// Returns the schema version encoded in this specification.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Validates and converts this specification into integer micrometres.
    ///
    /// Every dimensional value is rounded to the nearest micrometre, with
    /// positive half ties rounded up. Cross-field constraints are evaluated on
    /// those normalized integers so downstream geometry observes exactly the
    /// values that were validated.
    pub fn normalize(&self) -> Result<NormalizedJobSpec, ValidationErrors> {
        let mut issues = Vec::new();

        if self.schema_version != JOB_SPEC_VERSION {
            issues.push(ValidationIssue::UnsupportedSchemaVersion {
                found: self.schema_version,
                supported: JOB_SPEC_VERSION,
            });
        }

        if self.size.max_width_mm.is_none() && self.size.max_height_mm.is_none() {
            issues.push(ValidationIssue::MissingSizeConstraint);
        }

        let max_width_micrometers = normalize_optional_positive_mm(
            &mut issues,
            "size.max_width_mm",
            self.size.max_width_mm,
        );
        let max_height_micrometers = normalize_optional_positive_mm(
            &mut issues,
            "size.max_height_mm",
            self.size.max_height_mm,
        );
        let backing_thickness_micrometers = normalize_positive_mm(
            &mut issues,
            "geometry.backing_thickness_mm",
            self.geometry.backing_thickness_mm,
        );
        let total_thickness_micrometers = normalize_positive_mm(
            &mut issues,
            "geometry.total_thickness_mm",
            self.geometry.total_thickness_mm,
        );
        let border_width_micrometers = normalize_nonnegative_mm(
            &mut issues,
            "geometry.border_width_mm",
            self.geometry.border_width_mm,
        );
        let minimum_feature_width_micrometers = normalize_positive_mm(
            &mut issues,
            "geometry.minimum_feature_width_mm",
            self.geometry.minimum_feature_width_mm,
        );
        let contour_tolerance_micrometers = normalize_positive_mm(
            &mut issues,
            "geometry.contour_tolerance_mm",
            self.geometry.contour_tolerance_mm,
        );
        let sampling_pitch_micrometers =
            normalize_positive_mm(&mut issues, "raster.mm_per_pixel", self.raster.mm_per_pixel);

        if matches!(
            (backing_thickness_micrometers, total_thickness_micrometers),
            (Some(backing), Some(total)) if total <= backing
        ) {
            issues.push(ValidationIssue::TotalThicknessNotGreaterThanBacking);
        }

        if matches!(
            (border_width_micrometers, minimum_feature_width_micrometers),
            (Some(border), Some(minimum)) if border > 0 && border < minimum
        ) {
            issues.push(ValidationIssue::BorderBelowMinimumFeatureWidth);
        }

        if matches!(
            (
                contour_tolerance_micrometers,
                minimum_feature_width_micrometers
            ),
            (Some(tolerance), Some(minimum)) if twice(tolerance) > i128::from(minimum)
        ) {
            issues.push(ValidationIssue::ContourToleranceTooLarge);
        }

        if matches!(
            (sampling_pitch_micrometers, minimum_feature_width_micrometers),
            (Some(pitch), Some(minimum)) if twice(pitch) > i128::from(minimum)
        ) {
            issues.push(ValidationIssue::RasterResolutionTooCoarse);
        }

        if let Some(border_width_micrometers) = border_width_micrometers {
            if border_does_not_fit(border_width_micrometers, max_width_micrometers) {
                issues.push(ValidationIssue::BorderDoesNotFit {
                    dimension: "size.max_width_mm",
                });
            }
            if border_does_not_fit(border_width_micrometers, max_height_micrometers) {
                issues.push(ValidationIssue::BorderDoesNotFit {
                    dimension: "size.max_height_mm",
                });
            }
        }

        if !issues.is_empty() {
            return Err(ValidationErrors { issues });
        }

        Ok(NormalizedJobSpec {
            schema_version: self.schema_version,
            size: NormalizedSizeConstraints {
                max_width_micrometers,
                max_height_micrometers,
            },
            geometry: NormalizedGeometryOptions {
                backing_thickness_micrometers: backing_thickness_micrometers
                    .expect("successful validation retained backing thickness"),
                total_thickness_micrometers: total_thickness_micrometers
                    .expect("successful validation retained total thickness"),
                border_width_micrometers: border_width_micrometers
                    .expect("successful validation retained border width"),
                minimum_feature_width_micrometers: minimum_feature_width_micrometers
                    .expect("successful validation retained minimum feature width"),
                contour_tolerance_micrometers: contour_tolerance_micrometers
                    .expect("successful validation retained contour tolerance"),
            },
            raster: NormalizedRasterOptions {
                sampling_pitch_micrometers: sampling_pitch_micrometers
                    .expect("successful validation retained sampling pitch"),
                threshold: self.raster.threshold,
                invert_foreground: self.raster.invert_foreground,
            },
        })
    }

    /// Validates this specification and reports every independent problem.
    ///
    /// Validation uses the same integer-micrometre snapshot returned by
    /// [`Self::normalize`].
    pub fn validate(&self) -> Result<(), ValidationErrors> {
        self.normalize().map(drop)
    }
}

impl Default for JobSpec {
    fn default() -> Self {
        Self {
            schema_version: JOB_SPEC_VERSION,
            size: SizeConstraints::default(),
            geometry: GeometryOptions::default(),
            raster: RasterOptions::default(),
        }
    }
}

/// Maximum physical dimensions while preserving the source aspect ratio.
///
/// At least one constraint must be present. When both are present, output is
/// scaled to fit within the bounding box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SizeConstraints {
    /// Maximum output width in millimetres.
    pub max_width_mm: Option<f64>,
    /// Maximum output height in millimetres.
    pub max_height_mm: Option<f64>,
}

impl Default for SizeConstraints {
    fn default() -> Self {
        Self {
            max_width_mm: Some(DEFAULT_MAX_WIDTH_MM),
            max_height_mm: None,
        }
    }
}

/// Settings that control printable geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeometryOptions {
    /// Thickness of the continuous backing in millimetres.
    pub backing_thickness_mm: f64,
    /// Total finished thickness in millimetres.
    pub total_thickness_mm: f64,
    /// Width of the generated border in millimetres; zero disables it.
    pub border_width_mm: f64,
    /// Smallest feature retained for printing, in millimetres.
    pub minimum_feature_width_mm: f64,
    /// Maximum contour simplification error in millimetres.
    pub contour_tolerance_mm: f64,
}

impl Default for GeometryOptions {
    fn default() -> Self {
        Self {
            backing_thickness_mm: DEFAULT_BACKING_THICKNESS_MM,
            total_thickness_mm: DEFAULT_TOTAL_THICKNESS_MM,
            border_width_mm: DEFAULT_BORDER_WIDTH_MM,
            minimum_feature_width_mm: DEFAULT_MINIMUM_FEATURE_WIDTH_MM,
            contour_tolerance_mm: DEFAULT_CONTOUR_TOLERANCE_MM,
        }
    }
}

/// Settings that control rasterization and foreground extraction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RasterOptions {
    /// Physical sampling pitch in millimetres per pixel.
    pub mm_per_pixel: f64,
    /// Grayscale values strictly below this value are foreground.
    pub threshold: u8,
    /// Whether to invert the binary foreground mask after thresholding.
    pub invert_foreground: bool,
}

impl Default for RasterOptions {
    fn default() -> Self {
        Self {
            mm_per_pixel: DEFAULT_MM_PER_PIXEL,
            threshold: DEFAULT_RASTER_THRESHOLD,
            invert_foreground: false,
        }
    }
}

/// A validated, deterministic snapshot of one job specification.
///
/// Dimensional settings are represented as integer micrometres and all
/// cross-field invariants have been checked against those exact values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizedJobSpec {
    schema_version: u32,
    size: NormalizedSizeConstraints,
    geometry: NormalizedGeometryOptions,
    raster: NormalizedRasterOptions,
}

impl NormalizedJobSpec {
    /// Returns the validated schema version.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Returns normalized output-size constraints.
    #[must_use]
    pub const fn size(&self) -> &NormalizedSizeConstraints {
        &self.size
    }

    /// Returns normalized printable-geometry settings.
    #[must_use]
    pub const fn geometry(&self) -> &NormalizedGeometryOptions {
        &self.geometry
    }

    /// Returns normalized rasterization settings.
    #[must_use]
    pub const fn raster(&self) -> &NormalizedRasterOptions {
        &self.raster
    }
}

/// Validated maximum output dimensions in integer micrometres.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizedSizeConstraints {
    max_width_micrometers: Option<i64>,
    max_height_micrometers: Option<i64>,
}

impl NormalizedSizeConstraints {
    /// Returns the maximum output width in micrometres, if supplied.
    #[must_use]
    pub const fn max_width_micrometers(&self) -> Option<i64> {
        self.max_width_micrometers
    }

    /// Returns the maximum output height in micrometres, if supplied.
    #[must_use]
    pub const fn max_height_micrometers(&self) -> Option<i64> {
        self.max_height_micrometers
    }
}

/// Validated printable-geometry settings in integer micrometres.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizedGeometryOptions {
    backing_thickness_micrometers: i64,
    total_thickness_micrometers: i64,
    border_width_micrometers: i64,
    minimum_feature_width_micrometers: i64,
    contour_tolerance_micrometers: i64,
}

impl NormalizedGeometryOptions {
    /// Returns the backing thickness in micrometres.
    #[must_use]
    pub const fn backing_thickness_micrometers(&self) -> i64 {
        self.backing_thickness_micrometers
    }

    /// Returns the total finished thickness in micrometres.
    #[must_use]
    pub const fn total_thickness_micrometers(&self) -> i64 {
        self.total_thickness_micrometers
    }

    /// Returns the generated border width in micrometres.
    #[must_use]
    pub const fn border_width_micrometers(&self) -> i64 {
        self.border_width_micrometers
    }

    /// Returns the minimum printable feature width in micrometres.
    #[must_use]
    pub const fn minimum_feature_width_micrometers(&self) -> i64 {
        self.minimum_feature_width_micrometers
    }

    /// Returns the contour simplification tolerance in micrometres.
    #[must_use]
    pub const fn contour_tolerance_micrometers(&self) -> i64 {
        self.contour_tolerance_micrometers
    }
}

/// Validated rasterization and foreground settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizedRasterOptions {
    sampling_pitch_micrometers: i64,
    threshold: u8,
    invert_foreground: bool,
}

impl NormalizedRasterOptions {
    /// Returns the physical sampling pitch in micrometres per pixel.
    #[must_use]
    pub const fn sampling_pitch_micrometers(&self) -> i64 {
        self.sampling_pitch_micrometers
    }

    /// Returns the exclusive grayscale foreground threshold.
    #[must_use]
    pub const fn threshold(&self) -> u8 {
        self.threshold
    }

    /// Returns whether foreground selection is inverted.
    #[must_use]
    pub const fn invert_foreground(&self) -> bool {
        self.invert_foreground
    }
}

/// One deterministic, field-addressed validation problem.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValidationIssue {
    /// The document uses a schema version this crate does not implement.
    UnsupportedSchemaVersion {
        /// Version found in the document.
        found: u32,
        /// Version implemented by this crate.
        supported: u32,
    },
    /// Neither maximum width nor maximum height was supplied.
    MissingSizeConstraint,
    /// A numeric field is NaN or infinite.
    NotFinite {
        /// JSON-style path to the field.
        field: &'static str,
    },
    /// A numeric field must be greater than zero.
    MustBePositive {
        /// JSON-style path to the field.
        field: &'static str,
    },
    /// A numeric field must be zero or greater.
    MustBeNonnegative {
        /// JSON-style path to the field.
        field: &'static str,
    },
    /// A millimetre value cannot be represented as an `i64` micrometre value.
    TooLargeForMicrometers {
        /// JSON-style path to the field.
        field: &'static str,
    },
    /// A positive millimetre value rounds to zero micrometres.
    RoundsToZeroMicrometers {
        /// JSON-style path to the field.
        field: &'static str,
    },
    /// Total thickness must be greater than backing thickness.
    TotalThicknessNotGreaterThanBacking,
    /// A nonzero border is smaller than the printable feature width.
    BorderBelowMinimumFeatureWidth,
    /// Contour tolerance exceeds half the minimum printable feature width.
    ContourToleranceTooLarge,
    /// The configured sampling pitch cannot resolve the minimum feature width.
    RasterResolutionTooCoarse,
    /// Twice the border width is not smaller than a target dimension.
    BorderDoesNotFit {
        /// JSON-style path to the target dimension.
        dimension: &'static str,
    },
}

impl fmt::Display for ValidationIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchemaVersion { found, supported } => write!(
                formatter,
                "schema_version {found} is unsupported; expected {supported}"
            ),
            Self::MissingSizeConstraint => formatter.write_str(
                "at least one of size.max_width_mm or size.max_height_mm is required",
            ),
            Self::NotFinite { field } => write!(formatter, "{field} must be finite"),
            Self::MustBePositive { field } => {
                write!(formatter, "{field} must be greater than zero")
            }
            Self::MustBeNonnegative { field } => {
                write!(formatter, "{field} must be zero or greater")
            }
            Self::TooLargeForMicrometers { field } => write!(
                formatter,
                "{field} is too large to represent as integer micrometres"
            ),
            Self::RoundsToZeroMicrometers { field } => write!(
                formatter,
                "{field} must round to at least one integer micrometre"
            ),
            Self::TotalThicknessNotGreaterThanBacking => formatter.write_str(
                "geometry.total_thickness_mm must be greater than geometry.backing_thickness_mm",
            ),
            Self::BorderBelowMinimumFeatureWidth => formatter.write_str(
                "a nonzero geometry.border_width_mm must be at least geometry.minimum_feature_width_mm",
            ),
            Self::ContourToleranceTooLarge => formatter.write_str(
                "geometry.contour_tolerance_mm must not exceed half of geometry.minimum_feature_width_mm",
            ),
            Self::RasterResolutionTooCoarse => formatter.write_str(
                "raster.mm_per_pixel must not exceed half of geometry.minimum_feature_width_mm",
            ),
            Self::BorderDoesNotFit { dimension } => write!(
                formatter,
                "twice geometry.border_width_mm must be smaller than {dimension}"
            ),
        }
    }
}

/// The complete set of problems found while validating a [`JobSpec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationErrors {
    issues: Vec<ValidationIssue>,
}

impl ValidationErrors {
    /// Returns the number of validation problems.
    #[must_use]
    pub fn len(&self) -> usize {
        self.issues.len()
    }

    /// Returns whether no validation problems are present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }

    /// Returns all validation problems in deterministic validation order.
    #[must_use]
    pub fn issues(&self) -> &[ValidationIssue] {
        &self.issues
    }

    /// Consumes the collection and returns its validation problems.
    #[must_use]
    pub fn into_issues(self) -> Vec<ValidationIssue> {
        self.issues
    }
}

impl fmt::Display for ValidationErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "job specification has {} validation error(s)",
            self.issues.len()
        )?;
        for issue in &self.issues {
            write!(formatter, "; {issue}")?;
        }
        Ok(())
    }
}

impl Error for ValidationErrors {}

fn border_does_not_fit(border_width_micrometers: i64, dimension_micrometers: Option<i64>) -> bool {
    dimension_micrometers
        .is_some_and(|dimension| twice(border_width_micrometers) >= i128::from(dimension))
}

fn twice(value: i64) -> i128 {
    i128::from(value) * 2
}

fn normalize_optional_positive_mm(
    issues: &mut Vec<ValidationIssue>,
    field: &'static str,
    value: Option<f64>,
) -> Option<i64> {
    value.and_then(|value| normalize_positive_mm(issues, field, value))
}

fn normalize_positive_mm(
    issues: &mut Vec<ValidationIssue>,
    field: &'static str,
    value: f64,
) -> Option<i64> {
    if !value.is_finite() {
        issues.push(ValidationIssue::NotFinite { field });
        return None;
    }
    if value <= 0.0 {
        issues.push(ValidationIssue::MustBePositive { field });
        return None;
    }
    normalize_micrometer_range(issues, field, value)
}

fn normalize_nonnegative_mm(
    issues: &mut Vec<ValidationIssue>,
    field: &'static str,
    value: f64,
) -> Option<i64> {
    if !value.is_finite() {
        issues.push(ValidationIssue::NotFinite { field });
        return None;
    }
    if value < 0.0 {
        issues.push(ValidationIssue::MustBeNonnegative { field });
        return None;
    }
    if value == 0.0 {
        return Some(0);
    }
    normalize_micrometer_range(issues, field, value)
}

fn normalize_micrometer_range(
    issues: &mut Vec<ValidationIssue>,
    field: &'static str,
    millimeters: f64,
) -> Option<i64> {
    let rounded_micrometers = (millimeters * MICROMETERS_PER_MILLIMETER).round();
    if rounded_micrometers < 1.0 {
        issues.push(ValidationIssue::RoundsToZeroMicrometers { field });
        return None;
    }
    if rounded_micrometers >= I64_MICROMETER_UPPER_EXCLUSIVE {
        issues.push(ValidationIssue::TooLargeForMicrometers { field });
        return None;
    }
    Some(rounded_micrometers as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_spec_is_versioned_valid_and_serializes_explicitly() {
        let spec = JobSpec::default();

        assert_eq!(spec.schema_version(), JOB_SPEC_VERSION);
        assert_eq!(spec.size.max_width_mm, Some(DEFAULT_MAX_WIDTH_MM));
        assert_eq!(spec.size.max_height_mm, None);
        assert_eq!(
            serde_json::to_value(&spec).expect("default spec serializes"),
            json!({
                "schema_version": 1,
                "size": {
                    "max_width_mm": 100.0,
                    "max_height_mm": null
                },
                "geometry": {
                    "backing_thickness_mm": 1.2,
                    "total_thickness_mm": 2.0,
                    "border_width_mm": 2.0,
                    "minimum_feature_width_mm": 0.4,
                    "contour_tolerance_mm": 0.05
                },
                "raster": {
                    "mm_per_pixel": 0.04,
                    "threshold": 255,
                    "invert_foreground": false
                }
            })
        );
        assert_eq!(spec.validate(), Ok(()));
    }

    #[test]
    fn default_json_round_trips_without_changing_meaning() {
        let original = JobSpec::default();
        let encoded = serde_json::to_string(&original).expect("spec serializes");
        let decoded: JobSpec = serde_json::from_str(&encoded).expect("spec deserializes");

        assert_eq!(decoded, original);
    }

    #[test]
    fn schema_version_is_required() {
        let error = serde_json::from_str::<JobSpec>(r#"{"size": {}}"#)
            .expect_err("documents without a version must fail");

        assert!(error.to_string().contains("schema_version"));
    }

    #[test]
    fn missing_v1_settings_receive_v1_defaults() {
        let spec: JobSpec =
            serde_json::from_str(r#"{"schema_version": 1, "raster": {"invert_foreground": true}}"#)
                .expect("partial version 1 document is valid JSON");

        assert_eq!(spec.size, SizeConstraints::default());
        assert_eq!(spec.geometry, GeometryOptions::default());
        assert_eq!(
            spec.raster,
            RasterOptions {
                invert_foreground: true,
                ..RasterOptions::default()
            }
        );
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(serde_json::from_str::<JobSpec>(r#"{"schema_version": 1, "widht": 100}"#).is_err());
        assert!(
            serde_json::from_str::<JobSpec>(
                r#"{"schema_version": 1, "geometry": {"border_width": 2}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn unsupported_versions_are_reported() {
        let spec = JobSpec {
            schema_version: JOB_SPEC_VERSION + 1,
            ..JobSpec::default()
        };

        assert_eq!(
            spec.validate()
                .expect_err("future version must fail")
                .issues(),
            &[ValidationIssue::UnsupportedSchemaVersion {
                found: 2,
                supported: 1
            }]
        );
    }

    #[test]
    fn validation_collects_independent_numeric_errors() {
        let mut spec = JobSpec::default();
        spec.size.max_width_mm = None;
        spec.size.max_height_mm = None;
        spec.geometry.backing_thickness_mm = f64::NAN;
        spec.geometry.total_thickness_mm = 0.0;
        spec.geometry.border_width_mm = -1.0;
        spec.geometry.minimum_feature_width_mm = 0.0;
        spec.geometry.contour_tolerance_mm = f64::INFINITY;
        spec.raster.mm_per_pixel = 0.0;

        assert_eq!(
            spec.validate()
                .expect_err("invalid values must fail")
                .issues(),
            &[
                ValidationIssue::MissingSizeConstraint,
                ValidationIssue::NotFinite {
                    field: "geometry.backing_thickness_mm"
                },
                ValidationIssue::MustBePositive {
                    field: "geometry.total_thickness_mm"
                },
                ValidationIssue::MustBeNonnegative {
                    field: "geometry.border_width_mm"
                },
                ValidationIssue::MustBePositive {
                    field: "geometry.minimum_feature_width_mm"
                },
                ValidationIssue::NotFinite {
                    field: "geometry.contour_tolerance_mm"
                },
                ValidationIssue::MustBePositive {
                    field: "raster.mm_per_pixel"
                }
            ]
        );
    }

    #[test]
    fn validation_enforces_cross_field_printability_constraints() {
        let mut spec = JobSpec::default();
        spec.geometry.total_thickness_mm = spec.geometry.backing_thickness_mm;
        spec.geometry.border_width_mm = 0.2;
        spec.geometry.contour_tolerance_mm = 0.21;
        spec.raster.mm_per_pixel = 0.21;

        assert_eq!(
            spec.validate()
                .expect_err("invalid relationships must fail")
                .issues(),
            &[
                ValidationIssue::TotalThicknessNotGreaterThanBacking,
                ValidationIssue::BorderBelowMinimumFeatureWidth,
                ValidationIssue::ContourToleranceTooLarge,
                ValidationIssue::RasterResolutionTooCoarse
            ]
        );
    }

    #[test]
    fn disabled_border_and_exact_sampling_boundaries_are_valid() {
        let mut spec = JobSpec::default();
        spec.geometry.border_width_mm = 0.0;
        spec.geometry.contour_tolerance_mm = spec.geometry.minimum_feature_width_mm / 2.0;
        spec.raster.mm_per_pixel = spec.geometry.minimum_feature_width_mm / 2.0;

        assert_eq!(spec.validate(), Ok(()));
    }

    #[test]
    fn border_must_fit_every_supplied_dimension() {
        let mut spec = JobSpec::default();
        spec.size.max_width_mm = Some(spec.geometry.border_width_mm * 2.0);
        spec.size.max_height_mm = Some(3.0);

        assert_eq!(
            spec.validate()
                .expect_err("border must fit target")
                .issues(),
            &[
                ValidationIssue::BorderDoesNotFit {
                    dimension: "size.max_width_mm"
                },
                ValidationIssue::BorderDoesNotFit {
                    dimension: "size.max_height_mm"
                }
            ]
        );
    }

    #[test]
    fn positive_lengths_must_survive_micrometer_conversion() {
        let mut spec = JobSpec::default();
        spec.size.max_width_mm = Some(0.000_4);
        spec.size.max_height_mm = Some(i64::MAX as f64);

        assert_eq!(
            spec.validate()
                .expect_err("unrepresentable lengths must fail")
                .issues(),
            &[
                ValidationIssue::RoundsToZeroMicrometers {
                    field: "size.max_width_mm"
                },
                ValidationIssue::TooLargeForMicrometers {
                    field: "size.max_height_mm"
                }
            ]
        );
    }

    #[test]
    fn normalization_exposes_an_owned_integer_snapshot() {
        let mut spec = JobSpec::default();
        spec.size.max_height_mm = Some(80.000_5);
        spec.raster.threshold = 123;
        spec.raster.invert_foreground = true;

        let normalized = spec.normalize().expect("default-derived spec normalizes");

        assert_eq!(normalized.schema_version(), JOB_SPEC_VERSION);
        assert_eq!(normalized.size().max_width_micrometers(), Some(100_000));
        assert_eq!(normalized.size().max_height_micrometers(), Some(80_001));
        assert_eq!(normalized.geometry().backing_thickness_micrometers(), 1_200);
        assert_eq!(normalized.geometry().total_thickness_micrometers(), 2_000);
        assert_eq!(normalized.geometry().border_width_micrometers(), 2_000);
        assert_eq!(
            normalized.geometry().minimum_feature_width_micrometers(),
            400
        );
        assert_eq!(normalized.geometry().contour_tolerance_micrometers(), 50);
        assert_eq!(normalized.raster().sampling_pitch_micrometers(), 40);
        assert_eq!(normalized.raster().threshold(), 123);
        assert!(normalized.raster().invert_foreground());

        spec.size.max_width_mm = Some(1.0);
        spec.geometry.backing_thickness_mm = 1.5;
        spec.raster.threshold = 0;

        assert_eq!(spec.size.max_width_mm, Some(1.0));
        assert_eq!(spec.geometry.backing_thickness_mm, 1.5);
        assert_eq!(spec.raster.threshold, 0);
        assert_eq!(normalized.size().max_width_micrometers(), Some(100_000));
        assert_eq!(normalized.geometry().backing_thickness_micrometers(), 1_200);
        assert_eq!(normalized.raster().threshold(), 123);
    }

    #[test]
    fn micrometer_conversion_has_explicit_ties_zero_and_range_boundaries() {
        let mut issues = Vec::new();

        assert_eq!(normalize_positive_mm(&mut issues, "small", 0.000_499), None);
        assert_eq!(
            issues,
            [ValidationIssue::RoundsToZeroMicrometers { field: "small" }]
        );

        issues.clear();
        assert_eq!(normalize_positive_mm(&mut issues, "half", 0.000_5), Some(1));
        assert_eq!(
            normalize_positive_mm(&mut issues, "one_and_a_half", 0.001_5),
            Some(2)
        );
        assert_eq!(normalize_nonnegative_mm(&mut issues, "zero", 0.0), Some(0));
        assert_eq!(
            normalize_nonnegative_mm(&mut issues, "negative_zero", -0.0),
            Some(0)
        );
        assert!(issues.is_empty());

        assert_eq!(
            normalize_positive_mm(&mut issues, "largest_tested", 9_223_372_036_854_774.0),
            Some(9_223_372_036_854_773_760)
        );
        assert_eq!(
            normalize_positive_mm(&mut issues, "upper_exclusive", 9_223_372_036_854_776.0),
            None
        );
        assert_eq!(
            issues,
            [ValidationIssue::TooLargeForMicrometers {
                field: "upper_exclusive"
            }]
        );
    }

    #[test]
    fn every_nonfinite_class_is_rejected_before_conversion() {
        let mut issues = Vec::new();

        assert_eq!(normalize_positive_mm(&mut issues, "nan", f64::NAN), None);
        assert_eq!(
            normalize_positive_mm(&mut issues, "positive", f64::INFINITY),
            None
        );
        assert_eq!(
            normalize_nonnegative_mm(&mut issues, "negative", f64::NEG_INFINITY),
            None
        );
        assert_eq!(
            issues,
            [
                ValidationIssue::NotFinite { field: "nan" },
                ValidationIssue::NotFinite { field: "positive" },
                ValidationIssue::NotFinite { field: "negative" }
            ]
        );
    }

    #[test]
    fn thickness_order_is_checked_after_normalization() {
        let mut spec = JobSpec::default();
        spec.geometry.backing_thickness_mm = 1.0;
        spec.geometry.total_thickness_mm = 1.000_4;

        assert_eq!(
            spec.validate()
                .expect_err("normalized thicknesses are equal")
                .issues(),
            &[ValidationIssue::TotalThicknessNotGreaterThanBacking]
        );
    }

    #[test]
    fn half_feature_limits_are_checked_after_normalization() {
        let mut spec = JobSpec::default();
        spec.geometry.minimum_feature_width_mm = 1.001_2;
        spec.geometry.contour_tolerance_mm = spec.geometry.minimum_feature_width_mm / 2.0;
        spec.raster.mm_per_pixel = spec.geometry.minimum_feature_width_mm / 2.0;

        assert_eq!(
            spec.validate()
                .expect_err("rounded halves exceed the normalized limit")
                .issues(),
            &[
                ValidationIssue::ContourToleranceTooLarge,
                ValidationIssue::RasterResolutionTooCoarse
            ]
        );
    }

    #[test]
    fn equivalent_normalized_feature_widths_are_valid() {
        let mut spec = JobSpec::default();
        spec.geometry.border_width_mm = 0.999_6;
        spec.geometry.minimum_feature_width_mm = 1.000_4;

        let normalized = spec
            .normalize()
            .expect("equal normalized feature widths are valid");

        assert_eq!(normalized.geometry().border_width_micrometers(), 1_000);
        assert_eq!(
            normalized.geometry().minimum_feature_width_micrometers(),
            1_000
        );
    }

    #[test]
    fn border_fit_is_checked_after_normalization() {
        let mut normalized_failure = JobSpec::default();
        normalized_failure.geometry.border_width_mm = 2.000_51;
        normalized_failure.size.max_width_mm = Some(4.001_1);

        assert_eq!(
            normalized_failure
                .validate()
                .expect_err("normalized border does not fit")
                .issues(),
            &[ValidationIssue::BorderDoesNotFit {
                dimension: "size.max_width_mm"
            }]
        );

        let mut normalized_success = JobSpec::default();
        normalized_success.geometry.border_width_mm = 2.000_49;
        normalized_success.size.max_width_mm = Some(4.000_8);

        assert_eq!(normalized_success.validate(), Ok(()));
    }

    #[test]
    fn invalid_scalars_suppress_dependent_relationship_errors() {
        let mut spec = JobSpec::default();
        spec.geometry.minimum_feature_width_mm = 0.000_4;
        spec.geometry.contour_tolerance_mm = 100.0;
        spec.raster.mm_per_pixel = 100.0;

        assert_eq!(
            spec.validate()
                .expect_err("invalid minimum feature width must fail")
                .issues(),
            &[ValidationIssue::RoundsToZeroMicrometers {
                field: "geometry.minimum_feature_width_mm"
            }]
        );
    }

    #[test]
    fn validate_and_normalize_report_identical_issues() {
        let valid = JobSpec::default();
        assert_eq!(valid.validate(), valid.normalize().map(drop));

        let mut invalid = JobSpec::default();
        invalid.size.max_width_mm = None;
        invalid.geometry.total_thickness_mm = invalid.geometry.backing_thickness_mm;
        assert_eq!(invalid.validate(), invalid.normalize().map(drop));
    }
}
