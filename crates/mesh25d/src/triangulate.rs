//! Exact, deterministic polygon triangulation.
//!
//! Holes are connected to the exterior with visibility-tested bridges.  The
//! resulting weakly-simple ring is then ear clipped without discarding any
//! original boundary vertex; retaining those vertices is required so caps and
//! extrusion walls have exactly the same boundary segmentation.

use std::collections::BTreeMap;

use plaque3mf_planar::{PointMicrometers, Ring};

use crate::{MAX_TRIANGULATION_WORK, MeshError};

/// Shared deterministic work meter for all cap triangulations in one model.
#[derive(Debug, Default)]
pub(crate) struct WorkBudget {
    used: u128,
}

impl WorkBudget {
    /// Starts an unused triangulation work budget.
    pub(crate) const fn new() -> Self {
        Self { used: 0 }
    }

    fn charge(&mut self, amount: u128) -> Result<(), MeshError> {
        let required = self.used.saturating_add(amount);
        if required > MAX_TRIANGULATION_WORK {
            return Err(MeshError::TriangulationWorkLimitExceeded {
                required,
                max: MAX_TRIANGULATION_WORK,
            });
        }
        self.used = required;
        Ok(())
    }
}

/// Triangulates one counter-clockwise exterior and its clockwise holes.
pub(crate) fn triangulate_rings(
    exterior: &[PointMicrometers],
    holes: &[&Ring],
    expected_double_area: i128,
    budget: &mut WorkBudget,
) -> Result<Vec<[PointMicrometers; 3]>, MeshError> {
    let hole_vertices = holes.iter().map(|hole| hole.vertices()).collect::<Vec<_>>();
    triangulate_slices(exterior, &hole_vertices, expected_double_area, budget)
}

fn triangulate_slices(
    exterior: &[PointMicrometers],
    holes: &[&[PointMicrometers]],
    expected_double_area: i128,
    budget: &mut WorkBudget,
) -> Result<Vec<[PointMicrometers; 3]>, MeshError> {
    validate_input(exterior, holes, expected_double_area)?;

    let mut merged = exterior.to_vec();
    let mut bridges = Vec::with_capacity(holes.len());
    let mut hole_order = (0..holes.len()).collect::<Vec<_>>();
    hole_order.sort_by(|&first, &second| {
        let first_anchor = rightmost(holes[first]);
        let second_anchor = rightmost(holes[second]);
        second_anchor
            .cmp(&first_anchor)
            .then_with(|| holes[first].cmp(holes[second]))
    });

    for hole_index in hole_order {
        let hole = holes[hole_index];
        let (outer_index, hole_vertex_index) =
            find_bridge(&merged, hole, exterior, holes, &bridges, budget)?;
        let outer = merged[outer_index];
        let inner = hole[hole_vertex_index];
        merged = splice_hole(&merged, outer_index, hole, hole_vertex_index)?;
        bridges.push((outer, inner));
    }

    if signed_double_area(&merged)? != expected_double_area {
        return Err(MeshError::TriangulationFailed(
            "hole bridges changed the polygon area",
        ));
    }

    let triangles = ear_clip(&merged, budget)?;
    validate_triangulation(exterior, holes, &triangles, expected_double_area)?;
    Ok(triangles)
}

fn validate_input(
    exterior: &[PointMicrometers],
    holes: &[&[PointMicrometers]],
    expected_double_area: i128,
) -> Result<(), MeshError> {
    if exterior.len() < 3 || expected_double_area <= 0 {
        return Err(MeshError::TriangulationFailed(
            "a polygon exterior must have positive area",
        ));
    }
    if signed_double_area(exterior)? <= 0 {
        return Err(MeshError::TriangulationFailed(
            "the polygon exterior is not counter-clockwise",
        ));
    }

    let mut actual_area = signed_double_area(exterior)?;
    for hole in holes {
        if hole.len() < 3 || signed_double_area(hole)? >= 0 {
            return Err(MeshError::TriangulationFailed(
                "a polygon hole is not clockwise",
            ));
        }
        actual_area = actual_area.checked_add(signed_double_area(hole)?).ok_or(
            MeshError::TriangulationFailed("polygon area arithmetic overflowed"),
        )?;
    }
    if actual_area != expected_double_area {
        return Err(MeshError::TriangulationFailed(
            "the supplied polygon area does not match its rings",
        ));
    }
    Ok(())
}

fn rightmost(vertices: &[PointMicrometers]) -> PointMicrometers {
    vertices
        .iter()
        .copied()
        .max_by_key(|point| (point.x(), point.y()))
        .expect("validated ring has vertices")
}

fn find_bridge(
    merged: &[PointMicrometers],
    hole: &[PointMicrometers],
    exterior: &[PointMicrometers],
    holes: &[&[PointMicrometers]],
    bridges: &[(PointMicrometers, PointMicrometers)],
    budget: &mut WorkBudget,
) -> Result<(usize, usize), MeshError> {
    let mut best: Option<(i128, PointMicrometers, PointMicrometers, usize, usize)> = None;

    for (hole_index, &inner) in hole.iter().enumerate() {
        for (outer_index, &outer) in merged.iter().enumerate() {
            if inner == outer || !bridge_is_visible(inner, outer, exterior, holes, bridges, budget)?
            {
                continue;
            }

            let candidate = (
                squared_distance(inner, outer),
                inner,
                outer,
                outer_index,
                hole_index,
            );
            if best.as_ref().is_none_or(|current| candidate < *current) {
                best = Some(candidate);
            }
        }
    }

    best.map(|(_, _, _, outer_index, hole_index)| (outer_index, hole_index))
        .ok_or(MeshError::TriangulationFailed(
            "no non-intersecting hole bridge exists",
        ))
}

fn bridge_is_visible(
    inner: PointMicrometers,
    outer: PointMicrometers,
    exterior: &[PointMicrometers],
    holes: &[&[PointMicrometers]],
    bridges: &[(PointMicrometers, PointMicrometers)],
    budget: &mut WorkBudget,
) -> Result<bool, MeshError> {
    for ring in std::iter::once(exterior).chain(holes.iter().copied()) {
        for (start, end) in ring_edges(ring) {
            if segments_have_disallowed_intersection(inner, outer, start, end, budget)? {
                return Ok(false);
            }
        }
    }
    for &(start, end) in bridges {
        if segments_have_disallowed_intersection(inner, outer, start, end, budget)? {
            return Ok(false);
        }
    }

    let midpoint_x2 = i128::from(inner.x()) + i128::from(outer.x());
    let midpoint_y2 = i128::from(inner.y()) + i128::from(outer.y());
    if point_in_ring_scaled(exterior, midpoint_x2, midpoint_y2, budget)? != PointLocation::Inside {
        return Ok(false);
    }
    for hole in holes {
        if point_in_ring_scaled(hole, midpoint_x2, midpoint_y2, budget)? != PointLocation::Outside {
            return Ok(false);
        }
    }
    Ok(true)
}

fn splice_hole(
    merged: &[PointMicrometers],
    outer_index: usize,
    hole: &[PointMicrometers],
    hole_index: usize,
) -> Result<Vec<PointMicrometers>, MeshError> {
    let capacity = merged
        .len()
        .checked_add(hole.len())
        .and_then(|length| length.checked_add(2))
        .ok_or(MeshError::TriangulationFailed(
            "bridged polygon vertex count overflowed",
        ))?;
    let mut result = Vec::with_capacity(capacity);
    result.extend_from_slice(&merged[..=outer_index]);
    for offset in 0..hole.len() {
        result.push(hole[(hole_index + offset) % hole.len()]);
    }
    result.push(hole[hole_index]);
    result.push(merged[outer_index]);
    result.extend_from_slice(&merged[outer_index + 1..]);
    Ok(result)
}

fn ear_clip(
    vertices: &[PointMicrometers],
    budget: &mut WorkBudget,
) -> Result<Vec<[PointMicrometers; 3]>, MeshError> {
    if vertices.len() < 3 {
        return Err(MeshError::TriangulationFailed(
            "a bridged polygon has fewer than three vertices",
        ));
    }
    let triangle_count = vertices
        .len()
        .checked_sub(2)
        .ok_or(MeshError::TriangulationFailed(
            "triangulation size underflowed",
        ))?;
    let mut triangles = Vec::with_capacity(triangle_count);
    let mut active = (0..vertices.len()).collect::<Vec<_>>();

    while active.len() > 3 {
        let mut ear = None;
        for position in 0..active.len() {
            if is_ear(vertices, &active, position, budget)? {
                ear = Some(position);
                break;
            }
        }
        let Some(position) = ear else {
            return Err(MeshError::TriangulationFailed(
                "ear clipping stalled on a weakly-simple polygon",
            ));
        };
        let previous = active[(position + active.len() - 1) % active.len()];
        let current = active[position];
        let next = active[(position + 1) % active.len()];
        triangles.push([vertices[previous], vertices[current], vertices[next]]);
        active.remove(position);
    }

    let final_triangle = [
        vertices[active[0]],
        vertices[active[1]],
        vertices[active[2]],
    ];
    budget.charge(1)?;
    if triangle_cross(final_triangle) <= 0 {
        return Err(MeshError::TriangulationFailed(
            "the final ear is degenerate or reversed",
        ));
    }
    triangles.push(final_triangle);
    Ok(triangles)
}

fn is_ear(
    vertices: &[PointMicrometers],
    active: &[usize],
    position: usize,
    budget: &mut WorkBudget,
) -> Result<bool, MeshError> {
    let previous = active[(position + active.len() - 1) % active.len()];
    let current = active[position];
    let next = active[(position + 1) % active.len()];
    let triangle = [vertices[previous], vertices[current], vertices[next]];
    budget.charge(1)?;
    if triangle_cross(triangle) <= 0 {
        return Ok(false);
    }

    for edge_position in 0..active.len() {
        let edge_start = active[edge_position];
        let edge_end = active[(edge_position + 1) % active.len()];
        if (edge_start == previous && edge_end == next)
            || (edge_start == next && edge_end == previous)
        {
            continue;
        }
        if segments_have_disallowed_intersection(
            vertices[previous],
            vertices[next],
            vertices[edge_start],
            vertices[edge_end],
            budget,
        )? {
            return Ok(false);
        }
    }

    for &candidate_index in active {
        if candidate_index == previous || candidate_index == current || candidate_index == next {
            continue;
        }
        let candidate = vertices[candidate_index];
        if triangle.contains(&candidate) {
            continue;
        }
        if point_in_or_on_triangle(candidate, triangle, budget)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validate_triangulation(
    exterior: &[PointMicrometers],
    holes: &[&[PointMicrometers]],
    triangles: &[[PointMicrometers; 3]],
    expected_double_area: i128,
) -> Result<(), MeshError> {
    let mut area = 0_i128;
    let mut output_edges = BTreeMap::<EdgeKey, EdgeUse>::new();
    for &triangle in triangles {
        let triangle_area = triangle_cross(triangle);
        if triangle_area <= 0 {
            return Err(MeshError::TriangulationFailed(
                "triangulation contains a degenerate or reversed triangle",
            ));
        }
        area = area
            .checked_add(triangle_area)
            .ok_or(MeshError::TriangulationFailed(
                "triangle area arithmetic overflowed",
            ))?;
        add_edge(&mut output_edges, triangle[0], triangle[1])?;
        add_edge(&mut output_edges, triangle[1], triangle[2])?;
        add_edge(&mut output_edges, triangle[2], triangle[0])?;
    }
    if area != expected_double_area {
        return Err(MeshError::TriangulationFailed(
            "triangles do not exactly cover the polygon area",
        ));
    }

    let mut boundary_edges = BTreeMap::<EdgeKey, EdgeUse>::new();
    for ring in std::iter::once(exterior).chain(holes.iter().copied()) {
        for (start, end) in ring_edges(ring) {
            add_edge(&mut boundary_edges, start, end)?;
        }
    }
    if boundary_edges
        .values()
        .any(|usage| usage.forward + usage.reverse != 1)
    {
        return Err(MeshError::TriangulationFailed(
            "polygon boundary contains a repeated edge",
        ));
    }

    for (key, usage) in &output_edges {
        if let Some(boundary) = boundary_edges.get(key) {
            if usage != boundary {
                return Err(MeshError::TriangulationFailed(
                    "a cap does not retain its exact boundary segmentation",
                ));
            }
        } else if usage.forward != 1 || usage.reverse != 1 {
            return Err(MeshError::TriangulationFailed(
                "an internal triangulation edge is not paired in reverse",
            ));
        }
    }
    if boundary_edges
        .keys()
        .any(|key| !output_edges.contains_key(key))
    {
        return Err(MeshError::TriangulationFailed(
            "triangulation omitted a polygon boundary edge",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct EdgeKey(PointMicrometers, PointMicrometers);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct EdgeUse {
    forward: usize,
    reverse: usize,
}

fn add_edge(
    edges: &mut BTreeMap<EdgeKey, EdgeUse>,
    start: PointMicrometers,
    end: PointMicrometers,
) -> Result<(), MeshError> {
    if start == end {
        return Err(MeshError::TriangulationFailed(
            "triangulation contains a zero-length edge",
        ));
    }
    let (key, forward) = if start < end {
        (EdgeKey(start, end), true)
    } else {
        (EdgeKey(end, start), false)
    };
    let usage = edges.entry(key).or_default();
    let count = if forward {
        &mut usage.forward
    } else {
        &mut usage.reverse
    };
    *count = count.checked_add(1).ok_or(MeshError::TriangulationFailed(
        "triangulation edge multiplicity overflowed",
    ))?;
    Ok(())
}

fn ring_edges(
    ring: &[PointMicrometers],
) -> impl Iterator<Item = (PointMicrometers, PointMicrometers)> + '_ {
    ring.iter()
        .copied()
        .zip(ring.iter().copied().cycle().skip(1))
        .take(ring.len())
}

fn signed_double_area(vertices: &[PointMicrometers]) -> Result<i128, MeshError> {
    ring_edges(vertices).try_fold(0_i128, |area, (start, end)| {
        let cross = i128::from(start.x()) * i128::from(end.y())
            - i128::from(start.y()) * i128::from(end.x());
        area.checked_add(cross)
            .ok_or(MeshError::TriangulationFailed(
                "polygon area arithmetic overflowed",
            ))
    })
}

fn squared_distance(first: PointMicrometers, second: PointMicrometers) -> i128 {
    let dx = i128::from(second.x()) - i128::from(first.x());
    let dy = i128::from(second.y()) - i128::from(first.y());
    dx * dx + dy * dy
}

fn triangle_cross(triangle: [PointMicrometers; 3]) -> i128 {
    orientation(triangle[0], triangle[1], triangle[2])
}

fn orientation(first: PointMicrometers, second: PointMicrometers, third: PointMicrometers) -> i128 {
    let first_x = i128::from(second.x()) - i128::from(first.x());
    let first_y = i128::from(second.y()) - i128::from(first.y());
    let second_x = i128::from(third.x()) - i128::from(first.x());
    let second_y = i128::from(third.y()) - i128::from(first.y());
    first_x * second_y - first_y * second_x
}

fn segments_have_disallowed_intersection(
    first_start: PointMicrometers,
    first_end: PointMicrometers,
    second_start: PointMicrometers,
    second_end: PointMicrometers,
    budget: &mut WorkBudget,
) -> Result<bool, MeshError> {
    budget.charge(4)?;
    let first_side_start = orientation(first_start, first_end, second_start);
    let first_side_end = orientation(first_start, first_end, second_end);
    let second_side_start = orientation(second_start, second_end, first_start);
    let second_side_end = orientation(second_start, second_end, first_end);

    let intersects = opposite_signs(first_side_start, first_side_end)
        && opposite_signs(second_side_start, second_side_end)
        || first_side_start == 0 && point_on_segment(second_start, first_start, first_end)
        || first_side_end == 0 && point_on_segment(second_end, first_start, first_end)
        || second_side_start == 0 && point_on_segment(first_start, second_start, second_end)
        || second_side_end == 0 && point_on_segment(first_end, second_start, second_end);
    if !intersects {
        return Ok(false);
    }

    let shared_endpoint = first_start == second_start
        || first_start == second_end
        || first_end == second_start
        || first_end == second_end;
    let collinear = first_side_start == 0
        && first_side_end == 0
        && second_side_start == 0
        && second_side_end == 0;
    if collinear {
        return Ok(collinear_overlap_has_positive_length(
            first_start,
            first_end,
            second_start,
            second_end,
        ));
    }
    Ok(!shared_endpoint)
}

fn collinear_overlap_has_positive_length(
    first_start: PointMicrometers,
    first_end: PointMicrometers,
    second_start: PointMicrometers,
    second_end: PointMicrometers,
) -> bool {
    if first_start.x() != first_end.x() {
        first_start
            .x()
            .min(first_end.x())
            .max(second_start.x().min(second_end.x()))
            < first_start
                .x()
                .max(first_end.x())
                .min(second_start.x().max(second_end.x()))
    } else {
        first_start
            .y()
            .min(first_end.y())
            .max(second_start.y().min(second_end.y()))
            < first_start
                .y()
                .max(first_end.y())
                .min(second_start.y().max(second_end.y()))
    }
}

fn opposite_signs(first: i128, second: i128) -> bool {
    first < 0 && second > 0 || first > 0 && second < 0
}

fn point_on_segment(
    point: PointMicrometers,
    start: PointMicrometers,
    end: PointMicrometers,
) -> bool {
    point.x() >= start.x().min(end.x())
        && point.x() <= start.x().max(end.x())
        && point.y() >= start.y().min(end.y())
        && point.y() <= start.y().max(end.y())
}

fn point_in_or_on_triangle(
    point: PointMicrometers,
    triangle: [PointMicrometers; 3],
    budget: &mut WorkBudget,
) -> Result<bool, MeshError> {
    budget.charge(3)?;
    Ok(orientation(triangle[0], triangle[1], point) >= 0
        && orientation(triangle[1], triangle[2], point) >= 0
        && orientation(triangle[2], triangle[0], point) >= 0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointLocation {
    Outside,
    Inside,
    Boundary,
}

fn point_in_ring_scaled(
    ring: &[PointMicrometers],
    point_x2: i128,
    point_y2: i128,
    budget: &mut WorkBudget,
) -> Result<PointLocation, MeshError> {
    let mut winding = 0_i32;
    for (start, end) in ring_edges(ring) {
        budget.charge(1)?;
        let start_x2 = i128::from(start.x()) * 2;
        let start_y2 = i128::from(start.y()) * 2;
        let end_x2 = i128::from(end.x()) * 2;
        let end_y2 = i128::from(end.y()) * 2;
        let side = (i128::from(end.x()) - i128::from(start.x())) * (point_y2 - start_y2)
            - (i128::from(end.y()) - i128::from(start.y())) * (point_x2 - start_x2);

        if side == 0
            && point_x2 >= start_x2.min(end_x2)
            && point_x2 <= start_x2.max(end_x2)
            && point_y2 >= start_y2.min(end_y2)
            && point_y2 <= start_y2.max(end_y2)
        {
            return Ok(PointLocation::Boundary);
        }

        if start_y2 <= point_y2 {
            if end_y2 > point_y2 && side > 0 {
                winding += 1;
            }
        } else if end_y2 <= point_y2 && side < 0 {
            winding -= 1;
        }
    }
    Ok(if winding == 0 {
        PointLocation::Outside
    } else {
        PointLocation::Inside
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: i64, y: i64) -> PointMicrometers {
        PointMicrometers::new(x, y)
    }

    #[test]
    fn triangulates_concave_ring_without_dropping_collinear_vertices() {
        let exterior = [
            point(0, 0),
            point(2, 0),
            point(4, 0),
            point(4, 4),
            point(2, 2),
            point(0, 4),
        ];
        let mut budget = WorkBudget::new();
        let triangles = triangulate_slices(&exterior, &[], 24, &mut budget)
            .expect("concave polygon triangulates");

        assert_eq!(triangles.len(), exterior.len() - 2);
    }

    #[test]
    fn collinear_endpoint_touch_does_not_block_a_valid_ear() {
        let exterior = [
            point(0, 100),
            point(100, 100),
            point(100, 0),
            point(200, 0),
            point(200, 200),
            point(300, 200),
            point(300, 300),
            point(100, 300),
            point(100, 200),
            point(0, 200),
        ];
        let triangles = triangulate_slices(&exterior, &[], 100_000, &mut WorkBudget::new())
            .expect("collinear endpoint contact is not an overlap");

        assert_eq!(triangles.len(), exterior.len() - 2);
    }

    #[test]
    fn triangulates_a_hole_with_exact_bridge_pairing() {
        let exterior = [point(0, 0), point(10, 0), point(10, 10), point(0, 10)];
        let hole = [point(3, 3), point(3, 7), point(7, 7), point(7, 3)];
        let mut budget = WorkBudget::new();
        let triangles = triangulate_slices(&exterior, &[&hole], 168, &mut budget)
            .expect("polygon with a hole triangulates");

        assert_eq!(triangles.len(), exterior.len() + hole.len());
    }

    #[test]
    fn rejects_an_incorrect_expected_area() {
        let exterior = [point(0, 0), point(4, 0), point(4, 4), point(0, 4)];
        let error = triangulate_slices(&exterior, &[], 31, &mut WorkBudget::new())
            .expect_err("mismatched area must fail");

        assert!(matches!(error, MeshError::TriangulationFailed(_)));
    }
}
