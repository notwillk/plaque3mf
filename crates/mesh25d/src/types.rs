//! Project-owned fixed-point mesh and part-model types.

use std::{error::Error, fmt};

/// Validated vertical dimensions for 2.5D construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshOptions {
    backing_thickness_micrometers: i64,
    total_thickness_micrometers: i64,
}

impl MeshOptions {
    /// Creates mesh settings expressed in integer micrometres.
    pub const fn new(
        backing_thickness_micrometers: i64,
        total_thickness_micrometers: i64,
    ) -> Result<Self, MeshError> {
        if backing_thickness_micrometers <= 0 {
            return Err(MeshError::NonPositiveBackingThickness);
        }
        if total_thickness_micrometers <= 0 {
            return Err(MeshError::NonPositiveTotalThickness);
        }
        if total_thickness_micrometers <= backing_thickness_micrometers {
            return Err(MeshError::TotalThicknessNotGreaterThanBacking);
        }
        if total_thickness_micrometers > crate::MAX_MESH_HEIGHT_MICROMETERS {
            return Err(MeshError::ThicknessTooLarge {
                total: total_thickness_micrometers,
                max: crate::MAX_MESH_HEIGHT_MICROMETERS,
            });
        }
        Ok(Self {
            backing_thickness_micrometers,
            total_thickness_micrometers,
        })
    }

    /// Returns the continuous backing thickness.
    #[must_use]
    pub const fn backing_thickness_micrometers(self) -> i64 {
        self.backing_thickness_micrometers
    }

    /// Returns the finished part thickness.
    #[must_use]
    pub const fn total_thickness_micrometers(self) -> i64 {
        self.total_thickness_micrometers
    }
}

/// One exact 3D vertex in integer micrometres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VertexMicrometers {
    x: i64,
    y: i64,
    z: i64,
}

impl VertexMicrometers {
    pub(crate) const fn new(x: i64, y: i64, z: i64) -> Self {
        Self { x, y, z }
    }

    /// Returns the X coordinate.
    #[must_use]
    pub const fn x(self) -> i64 {
        self.x
    }

    /// Returns the Y coordinate.
    #[must_use]
    pub const fn y(self) -> i64 {
        self.y
    }

    /// Returns the Z coordinate.
    #[must_use]
    pub const fn z(self) -> i64 {
        self.z
    }
}

/// One consistently oriented indexed triangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Triangle {
    indices: [u32; 3],
}

impl Triangle {
    pub(crate) const fn new(indices: [u32; 3]) -> Self {
        Self { indices }
    }

    /// Returns the three vertex indices in winding order.
    #[must_use]
    pub const fn indices(self) -> [u32; 3] {
        self.indices
    }
}

/// One validated, closed, consistently oriented triangle mesh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriangleMesh {
    vertices: Vec<VertexMicrometers>,
    triangles: Vec<Triangle>,
    signed_six_volume: i128,
}

impl TriangleMesh {
    pub(crate) const fn from_validated(
        vertices: Vec<VertexMicrometers>,
        triangles: Vec<Triangle>,
        signed_six_volume: i128,
    ) -> Self {
        Self {
            vertices,
            triangles,
            signed_six_volume,
        }
    }

    /// Returns canonical lexicographically ordered vertices.
    #[must_use]
    pub fn vertices(&self) -> &[VertexMicrometers] {
        &self.vertices
    }

    /// Returns canonical oriented triangles.
    #[must_use]
    pub fn triangles(&self) -> &[Triangle] {
        &self.triangles
    }

    /// Returns six times the exact signed volume in cubic micrometres.
    #[must_use]
    pub const fn signed_six_volume(&self) -> i128 {
        self.signed_six_volume
    }
}

/// Separately selectable substrate and fill meshes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartModel {
    substrate: TriangleMesh,
    fill_parts: Vec<TriangleMesh>,
}

impl PartModel {
    pub(crate) const fn from_validated(
        substrate: TriangleMesh,
        fill_parts: Vec<TriangleMesh>,
    ) -> Self {
        Self {
            substrate,
            fill_parts,
        }
    }

    /// Returns the single backing, border, and foreground mesh.
    #[must_use]
    pub const fn substrate(&self) -> &TriangleMesh {
        &self.substrate
    }

    /// Returns fill meshes in the planar partition's canonical component order.
    #[must_use]
    pub fn fill_parts(&self) -> &[TriangleMesh] {
        &self.fill_parts
    }
}

/// Identifies a leaf mesh when reporting an invariant failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshPart {
    /// The continuous backing and raised substrate.
    Substrate,
    /// A zero-based fill component.
    Fill(usize),
}

impl fmt::Display for MeshPart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Substrate => formatter.write_str("substrate"),
            Self::Fill(index) => write!(formatter, "fill component {index}"),
        }
    }
}

/// A failure while converting a planar partition into exact triangle meshes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MeshError {
    NonPositiveBackingThickness,
    NonPositiveTotalThickness,
    TotalThicknessNotGreaterThanBacking,
    ThicknessTooLarge {
        total: i64,
        max: i64,
    },
    TooManyInputContourVertices {
        required: usize,
        max: usize,
    },
    TooManyMeshVertices {
        required: usize,
        max: usize,
    },
    TooManyMeshTriangles {
        required: usize,
        max: usize,
    },
    TriangulationWorkLimitExceeded {
        required: u128,
        max: u128,
    },
    TriangulationFailed(&'static str),
    TriangleIndexOverflow,
    InvalidTriangleIndex {
        part: MeshPart,
        triangle: usize,
        index: u32,
        vertex_count: usize,
    },
    DegenerateTriangle {
        part: MeshPart,
        triangle: usize,
    },
    DuplicateTriangle {
        part: MeshPart,
        triangle: usize,
    },
    UnreferencedVertex {
        part: MeshPart,
        vertex: u32,
    },
    NonManifoldEdge {
        part: MeshPart,
        first: u32,
        second: u32,
        uses: u32,
        balance: i32,
    },
    NonManifoldVertex {
        part: MeshPart,
        vertex: u32,
    },
    DisconnectedMesh {
        part: MeshPart,
    },
    VolumeMismatch {
        part: MeshPart,
        expected: i128,
        actual: i128,
    },
    CombinedVolumeMismatch {
        expected: i128,
        actual: i128,
    },
    ArithmeticOverflow,
}

impl fmt::Display for MeshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonPositiveBackingThickness => {
                formatter.write_str("backing thickness must be positive")
            }
            Self::NonPositiveTotalThickness => {
                formatter.write_str("total thickness must be positive")
            }
            Self::TotalThicknessNotGreaterThanBacking => {
                formatter.write_str("total thickness must be greater than backing thickness")
            }
            Self::ThicknessTooLarge { total, max } => write!(
                formatter,
                "total thickness {total} micrometres exceeds the supported maximum of {max}"
            ),
            Self::TooManyInputContourVertices { required, max } => write!(
                formatter,
                "mesh input requires {required} contour vertices; the limit is {max}"
            ),
            Self::TooManyMeshVertices { required, max } => write!(
                formatter,
                "mesh construction requires {required} vertices; the limit is {max}"
            ),
            Self::TooManyMeshTriangles { required, max } => write!(
                formatter,
                "mesh construction requires {required} triangles; the limit is {max}"
            ),
            Self::TriangulationWorkLimitExceeded { required, max } => write!(
                formatter,
                "cap triangulation requires {required} work units; the limit is {max}"
            ),
            Self::TriangulationFailed(reason) => {
                write!(formatter, "cap triangulation failed: {reason}")
            }
            Self::TriangleIndexOverflow => {
                formatter.write_str("mesh vertex index exceeds the u32 range")
            }
            Self::InvalidTriangleIndex {
                part,
                triangle,
                index,
                vertex_count,
            } => write!(
                formatter,
                "{part} triangle {triangle} references vertex {index}, but only {vertex_count} vertices exist"
            ),
            Self::DegenerateTriangle { part, triangle } => {
                write!(formatter, "{part} triangle {triangle} is degenerate")
            }
            Self::DuplicateTriangle { part, triangle } => {
                write!(
                    formatter,
                    "{part} triangle {triangle} duplicates an existing face"
                )
            }
            Self::UnreferencedVertex { part, vertex } => {
                write!(formatter, "{part} vertex {vertex} is not referenced")
            }
            Self::NonManifoldEdge {
                part,
                first,
                second,
                uses,
                balance,
            } => write!(
                formatter,
                "{part} edge {first}-{second} has {uses} incidences and direction balance {balance}"
            ),
            Self::NonManifoldVertex { part, vertex } => {
                write!(
                    formatter,
                    "{part} vertex {vertex} does not have one connected triangle fan"
                )
            }
            Self::DisconnectedMesh { part } => {
                write!(formatter, "{part} contains disconnected triangle shells")
            }
            Self::VolumeMismatch {
                part,
                expected,
                actual,
            } => write!(
                formatter,
                "{part} signed six-volume is {actual}, expected {expected}"
            ),
            Self::CombinedVolumeMismatch { expected, actual } => write!(
                formatter,
                "combined signed six-volume is {actual}, expected {expected}"
            ),
            Self::ArithmeticOverflow => {
                formatter.write_str("fixed-point mesh arithmetic overflowed")
            }
        }
    }
}

impl Error for MeshError {}
