//! Deterministic fixed-point contours and complementary planar partitioning.
//!
//! Canonical raster pixels are treated as physical cells. Their edges are
//! mapped to integer micrometres, foreground is cleaned before tracing, and an
//! exact rectangular border is added. Substrate and fill boundaries are then
//! derived from the same directed edge arrangement so their shared coordinates
//! cannot drift independently.

mod cleanup;
mod topology;
mod types;

pub use types::{
    PlanarError, PlanarOptions, PlanarPartition, PointMicrometers, Polygon, RectMicrometers,
    RegionSet, Ring,
};

use plaque3mf_document::{CanonicalArtwork, Dimension};

use cleanup::open_foreground_on_grid;
use topology::{
    assemble_region, complementary_boundary_edges, simplify_shared_rings, substrate_boundary_edges,
    trace_rings,
};

/// Largest physical extent accepted by deterministic planar arithmetic.
///
/// One billion micrometres is one kilometre, far beyond a printable plaque,
/// while keeping squared-distance and cross-product checks comfortably inside
/// `i128`.
pub const MAX_ARTWORK_EXTENT_MICROMETERS: i64 = 1_000_000_000;

/// Maximum number of elementary rectangles in one planar arrangement.
pub const MAX_PLANAR_CELLS: usize = 67_108_864;
/// Maximum number of raster intervals retained on either coordinate axis.
pub const MAX_AXIS_INTERVALS: usize = 1_000_000;

/// Maximum number of directed edges accepted by contour tracing.
pub const MAX_BOUNDARY_EDGES: usize = 4_194_304;

/// Maximum deterministic work units used by foreground morphology.
pub const MAX_MORPHOLOGY_WORK: u128 = 1_000_000_000;

/// Largest unique contour set on which quadratic topology checks are run.
pub const MAX_SIMPLIFICATION_VALIDATION_VERTICES: usize = 4_096;

/// Maximum point-in-polygon work used to assign holes to exterior rings.
pub const MAX_HOLE_CONTAINMENT_WORK: u128 = 16_777_216;

/// Cleans and partitions canonical artwork into complementary upper regions.
///
/// Coordinates use a bottom-left origin and integer micrometres. Raster row
/// zero is therefore mapped to the top of the physical footprint. Foreground
/// is opened without closing complementary gaps. Diagonal foreground contacts
/// are separated with deterministic chamfers, giving foreground four-connectivity
/// and fill eight-connectivity without point-touching polygons.
pub fn partition_artwork(
    artwork: &CanonicalArtwork,
    options: PlanarOptions,
) -> Result<PlanarPartition, PlanarError> {
    let physical = artwork.physical_size();
    let width = physical.width();
    let height = physical.height();
    validate_extent(width, height)?;
    validate_border(width, height, options.border_width_micrometers())?;

    let dimensions = artwork.foreground().dimensions();
    if dimensions.pixel_count() > MAX_PLANAR_CELLS {
        return Err(PlanarError::TooManyCells {
            required: dimensions.pixel_count(),
            max: MAX_PLANAR_CELLS,
        });
    }
    for (dimension, required) in [
        (Dimension::Width, dimensions.width() as usize),
        (Dimension::Height, dimensions.height() as usize),
    ] {
        if required > MAX_AXIS_INTERVALS {
            return Err(PlanarError::TooManyAxisIntervals {
                dimension,
                required,
                max: MAX_AXIS_INTERVALS,
            });
        }
    }

    let source_x_edges = axis_edges(width, dimensions.width(), Dimension::Width)?;
    let source_y_edges = axis_edges(height, dimensions.height(), Dimension::Height)?;
    let x_edges = with_border_edges(
        source_x_edges.clone(),
        width,
        options.border_width_micrometers(),
    );
    let y_edges = with_border_edges(
        source_y_edges.clone(),
        height,
        options.border_width_micrometers(),
    );
    let grid_width = x_edges.len() - 1;
    let grid_height = y_edges.len() - 1;
    let cell_count = grid_width
        .checked_mul(grid_height)
        .ok_or(PlanarError::ArithmeticOverflow)?;
    if cell_count > MAX_PLANAR_CELLS {
        return Err(PlanarError::TooManyCells {
            required: cell_count,
            max: MAX_PLANAR_CELLS,
        });
    }

    let cleaned = open_foreground_on_grid(
        artwork.foreground(),
        &source_x_edges,
        &source_y_edges,
        options.minimum_feature_width_micrometers(),
    )?;

    let x_sources = source_intervals(&source_x_edges, &x_edges)?;
    let y_sources = source_intervals(&source_y_edges, &y_edges)?;
    let solid = classify_cells(
        &x_edges,
        &y_edges,
        &x_sources,
        &y_sources,
        dimensions.width() as usize,
        dimensions.height() as usize,
        &cleaned,
        width,
        height,
        options.border_width_micrometers(),
    );

    let footprint = RectMicrometers::from_positive_size(width, height);
    let substrate_edges =
        substrate_boundary_edges(&x_edges, &y_edges, &solid, grid_width, grid_height)?;
    let fill_edges = complementary_boundary_edges(footprint, &x_edges, &y_edges, &substrate_edges)?;
    let substrate_rings = trace_rings(substrate_edges)?;
    let fill_rings = trace_rings(fill_edges)?;
    let (substrate_rings, fill_rings) = simplify_shared_rings(
        substrate_rings,
        fill_rings,
        footprint,
        options.contour_tolerance_micrometers(),
    );
    let substrate_upper = assemble_region(substrate_rings)?;
    let fill = assemble_region(fill_rings)?;

    let combined_area = substrate_upper
        .double_area()
        .checked_add(fill.double_area())
        .ok_or(PlanarError::ArithmeticOverflow)?;
    if combined_area != footprint.double_area() {
        return Err(PlanarError::InvalidTopology(
            "substrate and fill do not exactly cover the footprint",
        ));
    }

    Ok(PlanarPartition::from_validated_parts(
        footprint,
        substrate_upper,
        fill.polygons().to_vec(),
    ))
}

fn validate_extent(width: i64, height: i64) -> Result<(), PlanarError> {
    if width > MAX_ARTWORK_EXTENT_MICROMETERS || height > MAX_ARTWORK_EXTENT_MICROMETERS {
        return Err(PlanarError::ArtworkExtentTooLarge {
            width,
            height,
            max: MAX_ARTWORK_EXTENT_MICROMETERS,
        });
    }
    Ok(())
}

fn validate_border(width: i64, height: i64, border: i64) -> Result<(), PlanarError> {
    if border > 0
        && (i128::from(border) * 2 >= i128::from(width)
            || i128::from(border) * 2 >= i128::from(height))
    {
        return Err(PlanarError::BorderDoesNotFitArtwork {
            width,
            height,
            border,
        });
    }
    Ok(())
}

fn axis_edges(extent: i64, pixels: u32, dimension: Dimension) -> Result<Vec<i64>, PlanarError> {
    if i64::from(pixels) > extent {
        return Err(PlanarError::ResolutionTooFine {
            dimension,
            pixels,
            extent,
        });
    }

    let divisor = i128::from(pixels);
    let mut edges = Vec::with_capacity(pixels as usize + 1);
    for index in 0..=pixels {
        let numerator = i128::from(index) * i128::from(extent);
        let quotient = numerator / divisor;
        let remainder = numerator % divisor;
        let rounded = quotient + i128::from(remainder * 2 >= divisor);
        edges.push(i64::try_from(rounded).map_err(|_| PlanarError::ArithmeticOverflow)?);
    }
    if edges.windows(2).any(|window| window[0] >= window[1]) {
        return Err(PlanarError::ResolutionTooFine {
            dimension,
            pixels,
            extent,
        });
    }
    Ok(edges)
}

fn with_border_edges(mut edges: Vec<i64>, extent: i64, border: i64) -> Vec<i64> {
    if border > 0 {
        edges.push(border);
        edges.push(extent - border);
        edges.sort_unstable();
        edges.dedup();
    }
    edges
}

fn source_intervals(original: &[i64], augmented: &[i64]) -> Result<Vec<usize>, PlanarError> {
    let mut result = Vec::with_capacity(augmented.len() - 1);
    let mut source = 0_usize;
    for window in augmented.windows(2) {
        while source + 1 < original.len() - 1 && window[0] >= original[source + 1] {
            source += 1;
        }
        if window[0] < original[source] || window[1] > original[source + 1] {
            return Err(PlanarError::InvalidTopology(
                "an augmented cell crosses a raster-cell boundary",
            ));
        }
        result.push(source);
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn classify_cells(
    x_edges: &[i64],
    y_edges: &[i64],
    x_sources: &[usize],
    y_sources: &[usize],
    source_width: usize,
    source_height: usize,
    cleaned: &[u8],
    physical_width: i64,
    physical_height: i64,
    border: i64,
) -> Vec<u8> {
    let grid_width = x_sources.len();
    let grid_height = y_sources.len();
    let mut solid = Vec::with_capacity(grid_width * grid_height);
    for y in 0..grid_height {
        let source_y = source_height - 1 - y_sources[y];
        let y0 = y_edges[y];
        let y1 = y_edges[y + 1];
        for x in 0..grid_width {
            let x0 = x_edges[x];
            let x1 = x_edges[x + 1];
            let in_border = border > 0
                && (x0 < border
                    || x1 > physical_width - border
                    || y0 < border
                    || y1 > physical_height - border);
            let source_index = source_y * source_width + x_sources[x];
            solid.push(u8::from(in_border || cleaned[source_index] == 1));
        }
    }
    solid
}

#[cfg(test)]
mod tests;
