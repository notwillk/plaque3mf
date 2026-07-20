//! Project-owned fixed-point planar geometry types.

use plaque3mf_document::Dimension;
use std::{error::Error, fmt};

/// A two-dimensional point in integer micrometres.
///
/// Points use the planar coordinate system: the origin is at the bottom-left
/// of the artwork, positive X points right, and positive Y points up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PointMicrometers {
    x: i64,
    y: i64,
}

impl PointMicrometers {
    /// Creates a point from its X and Y coordinates in micrometres.
    #[must_use]
    pub const fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }

    /// Returns the X coordinate in micrometres.
    #[must_use]
    pub const fn x(self) -> i64 {
        self.x
    }

    /// Returns the Y coordinate in micrometres.
    #[must_use]
    pub const fn y(self) -> i64 {
        self.y
    }
}

/// An axis-aligned rectangular footprint with its lower-left corner at zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectMicrometers {
    width: i64,
    height: i64,
}

impl RectMicrometers {
    /// Creates a rectangle from positive physical extents in micrometres.
    ///
    /// # Panics
    ///
    /// Panics if either extent is zero or negative. Artwork extents are
    /// validated by the document layer before planar processing, so this
    /// constructor records that established invariant.
    #[must_use]
    pub(crate) const fn from_positive_size(width: i64, height: i64) -> Self {
        assert!(width > 0, "rectangle width must be positive");
        assert!(height > 0, "rectangle height must be positive");
        Self { width, height }
    }

    /// Returns the rectangle width in micrometres.
    #[must_use]
    pub const fn width(self) -> i64 {
        self.width
    }

    /// Returns the rectangle height in micrometres.
    #[must_use]
    pub const fn height(self) -> i64 {
        self.height
    }

    /// Returns the lower-left corner.
    #[must_use]
    pub const fn bottom_left(self) -> PointMicrometers {
        PointMicrometers::new(0, 0)
    }

    /// Returns the lower-right corner.
    #[must_use]
    pub const fn bottom_right(self) -> PointMicrometers {
        PointMicrometers::new(self.width, 0)
    }

    /// Returns the upper-right corner.
    #[must_use]
    pub const fn top_right(self) -> PointMicrometers {
        PointMicrometers::new(self.width, self.height)
    }

    /// Returns the upper-left corner.
    #[must_use]
    pub const fn top_left(self) -> PointMicrometers {
        PointMicrometers::new(0, self.height)
    }

    /// Returns the corners in counter-clockwise order, starting at the origin.
    #[must_use]
    pub const fn corners(self) -> [PointMicrometers; 4] {
        [
            self.bottom_left(),
            self.bottom_right(),
            self.top_right(),
            self.top_left(),
        ]
    }

    /// Returns twice the rectangle area in square micrometres.
    ///
    /// Twice-area arithmetic avoids fractions and remains exact for all
    /// positive `i64` extents.
    #[must_use]
    pub const fn double_area(self) -> i128 {
        self.width as i128 * self.height as i128 * 2
    }
}

/// A simple closed polygon ring.
///
/// The first vertex is not repeated at the end. Rings produced by this crate
/// contain at least three vertices and have already been oriented and
/// canonicalized. Exterior rings are counter-clockwise and holes are
/// clockwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ring {
    vertices: Vec<PointMicrometers>,
}

impl Ring {
    /// Records a ring whose orientation and topology have already been checked.
    pub(crate) fn from_oriented_vertices(vertices: Vec<PointMicrometers>) -> Self {
        debug_assert!(vertices.len() >= 3);
        debug_assert_ne!(signed_double_area(&vertices), 0);
        Self { vertices }
    }

    /// Returns the ring vertices without a repeated closing vertex.
    #[must_use]
    pub fn vertices(&self) -> &[PointMicrometers] {
        &self.vertices
    }

    /// Returns the signed double area in square micrometres.
    ///
    /// A positive value means counter-clockwise orientation and a negative
    /// value means clockwise orientation.
    #[must_use]
    pub fn signed_double_area(&self) -> i128 {
        signed_double_area(&self.vertices)
    }

    /// Returns whether this ring is counter-clockwise.
    #[must_use]
    pub fn is_ccw(&self) -> bool {
        self.signed_double_area() > 0
    }
}

/// One connected filled polygon with zero or more holes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Polygon {
    exterior: Ring,
    holes: Vec<Ring>,
}

impl Polygon {
    /// Records an already-validated exterior ring and its holes.
    pub(crate) fn from_oriented_rings(exterior: Ring, holes: Vec<Ring>) -> Self {
        debug_assert!(exterior.is_ccw());
        debug_assert!(holes.iter().all(|hole| !hole.is_ccw()));
        Self { exterior, holes }
    }

    /// Returns the counter-clockwise exterior ring.
    #[must_use]
    pub const fn exterior(&self) -> &Ring {
        &self.exterior
    }

    /// Returns the clockwise hole rings.
    #[must_use]
    pub fn holes(&self) -> &[Ring] {
        &self.holes
    }

    /// Returns twice the filled polygon area in square micrometres.
    #[must_use]
    pub fn double_area(&self) -> i128 {
        self.holes
            .iter()
            .fold(self.exterior.signed_double_area(), |area, hole| {
                area.checked_add(hole.signed_double_area())
                    .expect("validated planar area fits into i128")
            })
    }
}

/// A deterministic collection of interior-disjoint polygons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionSet {
    polygons: Vec<Polygon>,
}

impl RegionSet {
    /// Records an already-validated, canonically ordered polygon collection.
    pub(crate) const fn from_polygons(polygons: Vec<Polygon>) -> Self {
        Self { polygons }
    }

    /// Returns all polygons in canonical order.
    #[must_use]
    pub fn polygons(&self) -> &[Polygon] {
        &self.polygons
    }

    /// Returns whether this set has no filled area.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.polygons.is_empty()
    }

    /// Returns twice the total filled area in square micrometres.
    #[must_use]
    pub fn double_area(&self) -> i128 {
        self.polygons.iter().fold(0, |area, polygon| {
            area.checked_add(polygon.double_area())
                .expect("validated planar area fits into i128")
        })
    }
}

/// Validated settings used to clean and partition canonical artwork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanarOptions {
    border_width_micrometers: i64,
    minimum_feature_width_micrometers: i64,
    contour_tolerance_micrometers: i64,
}

impl PlanarOptions {
    /// Creates planar settings expressed in integer micrometres.
    ///
    /// A zero border disables the generated border. A zero contour tolerance
    /// disables geometric simplification. Minimum feature width must always be
    /// positive and applies to foreground features only; complementary gaps
    /// are not closed during cleanup.
    pub const fn new(
        border_width_micrometers: i64,
        minimum_feature_width_micrometers: i64,
        contour_tolerance_micrometers: i64,
    ) -> Result<Self, PlanarError> {
        if border_width_micrometers < 0 {
            return Err(PlanarError::NegativeBorderWidth);
        }
        if minimum_feature_width_micrometers <= 0 {
            return Err(PlanarError::NonPositiveMinimumFeatureWidth);
        }
        if contour_tolerance_micrometers < 0 {
            return Err(PlanarError::NegativeContourTolerance);
        }
        if contour_tolerance_micrometers > crate::MAX_ARTWORK_EXTENT_MICROMETERS {
            return Err(PlanarError::ContourToleranceTooLarge {
                tolerance: contour_tolerance_micrometers,
                max: crate::MAX_ARTWORK_EXTENT_MICROMETERS,
            });
        }

        Ok(Self {
            border_width_micrometers,
            minimum_feature_width_micrometers,
            contour_tolerance_micrometers,
        })
    }

    /// Returns the generated border width in micrometres.
    #[must_use]
    pub const fn border_width_micrometers(self) -> i64 {
        self.border_width_micrometers
    }

    /// Returns the minimum retained foreground-feature width in micrometres.
    #[must_use]
    pub const fn minimum_feature_width_micrometers(self) -> i64 {
        self.minimum_feature_width_micrometers
    }

    /// Returns the maximum contour simplification error in micrometres.
    #[must_use]
    pub const fn contour_tolerance_micrometers(self) -> i64 {
        self.contour_tolerance_micrometers
    }
}

/// A complete, exact partition of one artwork footprint.
///
/// `substrate_upper` and `fill_parts` have disjoint interiors and together
/// cover `footprint`. Shared boundaries are derived from one arrangement and
/// therefore use identical fixed-point coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanarPartition {
    footprint: RectMicrometers,
    substrate_upper: RegionSet,
    fill_parts: Vec<Polygon>,
}

impl PlanarPartition {
    /// Records a partition after its coverage and topology have been checked.
    pub(crate) const fn from_validated_parts(
        footprint: RectMicrometers,
        substrate_upper: RegionSet,
        fill_parts: Vec<Polygon>,
    ) -> Self {
        Self {
            footprint,
            substrate_upper,
            fill_parts,
        }
    }

    /// Returns the full rectangular footprint.
    #[must_use]
    pub const fn footprint(&self) -> RectMicrometers {
        self.footprint
    }

    /// Returns the upper substrate region: cleaned foreground union border.
    #[must_use]
    pub const fn substrate_upper(&self) -> &RegionSet {
        &self.substrate_upper
    }

    /// Returns the connected fill components in canonical order.
    #[must_use]
    pub fn fill_parts(&self) -> &[Polygon] {
        &self.fill_parts
    }
}

/// A failure while converting canonical artwork into a planar partition.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlanarError {
    /// Border width must be zero or positive.
    NegativeBorderWidth,
    /// Minimum printable feature width must be positive.
    NonPositiveMinimumFeatureWidth,
    /// Contour tolerance must be zero or positive.
    NegativeContourTolerance,
    /// Contour tolerance exceeds the supported deterministic geometry range.
    ContourToleranceTooLarge {
        /// Requested tolerance in micrometres.
        tolerance: i64,
        /// Largest supported tolerance in micrometres.
        max: i64,
    },
    /// Artwork extents exceed the supported deterministic geometry range.
    ArtworkExtentTooLarge {
        /// Artwork width in micrometres.
        width: i64,
        /// Artwork height in micrometres.
        height: i64,
        /// Largest supported extent in micrometres.
        max: i64,
    },
    /// The inset border leaves no interior in at least one dimension.
    BorderDoesNotFitArtwork {
        /// Artwork width in micrometres.
        width: i64,
        /// Artwork height in micrometres.
        height: i64,
        /// Requested border width in micrometres.
        border: i64,
    },
    /// More pixel boundaries exist than integer micrometres can represent.
    ResolutionTooFine {
        /// Axis whose mapped pixel edges would collapse.
        dimension: Dimension,
        /// Pixel count along that axis.
        pixels: u32,
        /// Physical axis extent in micrometres.
        extent: i64,
    },
    /// One raster axis exceeds its deterministic breakpoint limit.
    TooManyAxisIntervals {
        /// Axis with too many intervals.
        dimension: Dimension,
        /// Number of requested intervals.
        required: usize,
        /// Maximum supported interval count.
        max: usize,
    },
    /// A diagonal contact cannot be separated on the integer-micrometre grid.
    DiagonalContactTooFine {
        /// X coordinate of the ambiguous contact.
        x: i64,
        /// Y coordinate of the ambiguous contact.
        y: i64,
    },
    /// The planar cell grid exceeds its deterministic resource limit.
    TooManyCells {
        /// Number of cells the operation requires.
        required: usize,
        /// Maximum supported cell count.
        max: usize,
    },
    /// Contour tracing would exceed its deterministic edge limit.
    TooManyBoundaryEdges {
        /// Number of boundary edges the operation requires.
        required: usize,
        /// Maximum supported boundary-edge count.
        max: usize,
    },
    /// Feature cleanup would exceed its deterministic work limit.
    MorphologyWorkLimitExceeded {
        /// Work units the operation requires.
        required: u128,
        /// Maximum supported work units.
        max: u128,
    },
    /// Hole-to-exterior assignment would exceed its deterministic work limit.
    HoleContainmentWorkLimitExceeded {
        /// Point-in-polygon work units the operation requires.
        required: u128,
        /// Maximum supported work units.
        max: u128,
    },
    /// Checked fixed-point arithmetic overflowed.
    ArithmeticOverflow,
    /// An intermediate or final polygon arrangement was invalid.
    InvalidTopology(&'static str),
}

impl fmt::Display for PlanarError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NegativeBorderWidth => formatter.write_str("border width must not be negative"),
            Self::NonPositiveMinimumFeatureWidth => {
                formatter.write_str("minimum feature width must be positive")
            }
            Self::NegativeContourTolerance => {
                formatter.write_str("contour tolerance must not be negative")
            }
            Self::ContourToleranceTooLarge { tolerance, max } => write!(
                formatter,
                "contour tolerance {tolerance} micrometres exceeds the supported maximum of {max} micrometres"
            ),
            Self::ArtworkExtentTooLarge { width, height, max } => write!(
                formatter,
                "artwork extent {width}x{height} micrometres exceeds the supported maximum of {max} micrometres per axis"
            ),
            Self::BorderDoesNotFitArtwork {
                width,
                height,
                border,
            } => write!(
                formatter,
                "border width {border} micrometres does not fit inside artwork extent {width}x{height} micrometres"
            ),
            Self::ResolutionTooFine {
                dimension,
                pixels,
                extent,
            } => write!(
                formatter,
                "{pixels} {dimension} pixels cannot be represented across {extent} integer micrometres"
            ),
            Self::TooManyAxisIntervals {
                dimension,
                required,
                max,
            } => write!(
                formatter,
                "{dimension} axis requires {required} intervals; the limit is {max}"
            ),
            Self::DiagonalContactTooFine { x, y } => write!(
                formatter,
                "diagonal contact at ({x}, {y}) cannot be separated on the integer-micrometre grid"
            ),
            Self::TooManyCells { required, max } => write!(
                formatter,
                "planar processing requires {required} cells; the limit is {max}"
            ),
            Self::TooManyBoundaryEdges { required, max } => write!(
                formatter,
                "contour tracing requires {required} boundary edges; the limit is {max}"
            ),
            Self::MorphologyWorkLimitExceeded { required, max } => write!(
                formatter,
                "feature cleanup requires {required} work units; the limit is {max}"
            ),
            Self::HoleContainmentWorkLimitExceeded { required, max } => write!(
                formatter,
                "hole assignment requires {required} work units; the limit is {max}"
            ),
            Self::ArithmeticOverflow => {
                formatter.write_str("fixed-point planar arithmetic overflowed")
            }
            Self::InvalidTopology(reason) => write!(formatter, "invalid planar topology: {reason}"),
        }
    }
}

impl Error for PlanarError {}

fn signed_double_area(vertices: &[PointMicrometers]) -> i128 {
    if vertices.len() < 3 {
        return 0;
    }

    vertices
        .iter()
        .zip(vertices.iter().cycle().skip(1))
        .take(vertices.len())
        .fold(0_i128, |area, (start, end)| {
            let cross = i128::from(start.x()) * i128::from(end.y())
                - i128::from(start.y()) * i128::from(end.x());
            area.checked_add(cross)
                .expect("validated planar area fits into i128")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(vertices: &[(i64, i64)]) -> Ring {
        Ring::from_oriented_vertices(
            vertices
                .iter()
                .map(|&(x, y)| PointMicrometers::new(x, y))
                .collect(),
        )
    }

    #[test]
    fn points_order_lexicographically() {
        let mut points = [
            PointMicrometers::new(1, 0),
            PointMicrometers::new(0, 2),
            PointMicrometers::new(0, 1),
        ];
        points.sort();

        assert_eq!(
            points,
            [
                PointMicrometers::new(0, 1),
                PointMicrometers::new(0, 2),
                PointMicrometers::new(1, 0),
            ]
        );
    }

    #[test]
    fn rectangle_corners_and_area_are_exact() {
        let rectangle = RectMicrometers::from_positive_size(10, 20);

        assert_eq!(rectangle.width(), 10);
        assert_eq!(rectangle.height(), 20);
        assert_eq!(
            rectangle.corners(),
            [
                PointMicrometers::new(0, 0),
                PointMicrometers::new(10, 0),
                PointMicrometers::new(10, 20),
                PointMicrometers::new(0, 20),
            ]
        );
        assert_eq!(rectangle.double_area(), 400);
    }

    #[test]
    fn ring_area_reports_orientation() {
        let counter_clockwise = ring(&[(0, 0), (10, 0), (10, 10), (0, 10)]);
        let clockwise = ring(&[(0, 0), (0, 10), (10, 10), (10, 0)]);

        assert_eq!(counter_clockwise.signed_double_area(), 200);
        assert!(counter_clockwise.is_ccw());
        assert_eq!(clockwise.signed_double_area(), -200);
        assert!(!clockwise.is_ccw());
    }

    #[test]
    fn polygon_and_region_area_subtract_holes() {
        let exterior = ring(&[(0, 0), (10, 0), (10, 10), (0, 10)]);
        let hole = ring(&[(2, 2), (2, 4), (4, 4), (4, 2)]);
        let polygon = Polygon::from_oriented_rings(exterior, vec![hole]);
        let regions = RegionSet::from_polygons(vec![polygon.clone()]);

        assert_eq!(polygon.double_area(), 192);
        assert_eq!(regions.double_area(), 192);
        assert!(!regions.is_empty());
    }

    #[test]
    fn planar_options_validate_scalar_domains() {
        assert_eq!(
            PlanarOptions::new(-1, 1, 0),
            Err(PlanarError::NegativeBorderWidth)
        );
        assert_eq!(
            PlanarOptions::new(0, 0, 0),
            Err(PlanarError::NonPositiveMinimumFeatureWidth)
        );
        assert_eq!(
            PlanarOptions::new(0, 1, -1),
            Err(PlanarError::NegativeContourTolerance)
        );
        assert!(matches!(
            PlanarOptions::new(0, 1, i64::MAX),
            Err(PlanarError::ContourToleranceTooLarge { .. })
        ));

        let options = PlanarOptions::new(2_000, 400, 0).expect("settings are valid");
        assert_eq!(options.border_width_micrometers(), 2_000);
        assert_eq!(options.minimum_feature_width_micrometers(), 400);
        assert_eq!(options.contour_tolerance_micrometers(), 0);
    }
}
