//! Deterministic watertight 2.5D construction from planar partitions.
//!
//! Planar caps are triangulated once with exact integer predicates and reused
//! across contacting parts. The substrate is emitted as one boundary shell:
//! footprint bottom, lower outside walls, fill shelves, raised substrate caps,
//! and raised walls. No overlapping closed extrusions or 3D Boolean operations
//! are used.

mod build;
mod triangulate;
mod types;

pub use types::{
    MeshError, MeshOptions, MeshPart, PartModel, Triangle, TriangleMesh, VertexMicrometers,
};

use plaque3mf_planar::PlanarPartition;

/// Largest accepted vertical coordinate in integer micrometres.
pub const MAX_MESH_HEIGHT_MICROMETERS: i64 = 1_000_000_000;

/// Maximum bridged contour size accepted before triangulation.
pub const MAX_INPUT_CONTOUR_VERTICES: usize = 1_000_000;

/// Maximum total vertex estimate accepted for one part model.
pub const MAX_MESH_VERTICES: usize = 4_194_304;

/// Maximum total triangle estimate accepted for one part model.
pub const MAX_MESH_TRIANGLES: usize = 8_388_608;

/// Maximum deterministic predicate work across all cap triangulations.
pub const MAX_TRIANGULATION_WORK: u128 = 100_000_000;

/// Builds separately selectable, exact, watertight substrate and fill meshes.
///
/// Coordinates remain integer micrometres. Returned vertices and triangles are
/// canonicalized so repeated construction of the same partition is byte-for-
/// byte deterministic.
pub fn build_part_model(
    partition: &PlanarPartition,
    options: MeshOptions,
) -> Result<PartModel, MeshError> {
    build::build(partition, options)
}

#[cfg(test)]
mod tests;
