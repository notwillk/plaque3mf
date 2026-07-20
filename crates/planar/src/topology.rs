use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

use crate::{
    MAX_BOUNDARY_EDGES, MAX_HOLE_CONTAINMENT_WORK, MAX_SIMPLIFICATION_VALIDATION_VERTICES,
    PlanarError, PointMicrometers, Polygon, RectMicrometers, RegionSet, Ring,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DirectedEdge {
    start: PointMicrometers,
    end: PointMicrometers,
}

impl DirectedEdge {
    const fn new(start: PointMicrometers, end: PointMicrometers) -> Self {
        Self { start, end }
    }

    const fn reversed(self) -> Self {
        Self::new(self.end, self.start)
    }
}

pub(crate) fn substrate_boundary_edges(
    x_edges: &[i64],
    y_edges: &[i64],
    solid: &[u8],
    width: usize,
    height: usize,
) -> Result<Vec<DirectedEdge>, PlanarError> {
    debug_assert_eq!(solid.len(), width * height);
    debug_assert_eq!(x_edges.len(), width + 1);
    debug_assert_eq!(y_edges.len(), height + 1);

    let mut edges = Vec::new();
    for y in 0..height {
        for x in 0..width {
            if solid[y * width + x] == 0 {
                continue;
            }

            let rectangle = CellRectangle {
                x0: x_edges[x],
                x1: x_edges[x + 1],
                y0: y_edges[y],
                y1: y_edges[y + 1],
            };
            let x = i64::try_from(x).map_err(|_| PlanarError::ArithmeticOverflow)?;
            let y = i64::try_from(y).map_err(|_| PlanarError::ArithmeticOverflow)?;
            let bottom_open = !solid_at(solid, width, height, x, y - 1);
            let right_open = !solid_at(solid, width, height, x + 1, y);
            let top_open = !solid_at(solid, width, height, x, y + 1);
            let left_open = !solid_at(solid, width, height, x - 1, y);
            let corners = [
                corner_points(
                    rectangle,
                    CellCorner::BottomLeft,
                    bottom_open && left_open && solid_at(solid, width, height, x - 1, y - 1),
                )?,
                corner_points(
                    rectangle,
                    CellCorner::BottomRight,
                    bottom_open && right_open && solid_at(solid, width, height, x + 1, y - 1),
                )?,
                corner_points(
                    rectangle,
                    CellCorner::TopRight,
                    top_open && right_open && solid_at(solid, width, height, x + 1, y + 1),
                )?,
                corner_points(
                    rectangle,
                    CellCorner::TopLeft,
                    top_open && left_open && solid_at(solid, width, height, x - 1, y + 1),
                )?,
            ];
            let open_sides = [bottom_open, right_open, top_open, left_open];

            for side in 0..4 {
                let next = (side + 1) % 4;
                if open_sides[side] {
                    push_edge(
                        &mut edges,
                        DirectedEdge::new(corners[side].outgoing, corners[next].incoming),
                    )?;
                }
                if corners[next].clipped {
                    push_edge(
                        &mut edges,
                        DirectedEdge::new(corners[next].incoming, corners[next].outgoing),
                    )?;
                }
            }
        }
    }
    Ok(edges)
}
#[derive(Debug, Clone, Copy)]
struct CellRectangle {
    x0: i64,
    x1: i64,
    y0: i64,
    y1: i64,
}

#[derive(Debug, Clone, Copy)]
enum CellCorner {
    BottomLeft,
    BottomRight,
    TopRight,
    TopLeft,
}

#[derive(Debug, Clone, Copy)]
struct CornerPoints {
    incoming: PointMicrometers,
    outgoing: PointMicrometers,
    clipped: bool,
}

fn corner_points(
    rectangle: CellRectangle,
    corner: CellCorner,
    clipped: bool,
) -> Result<CornerPoints, PlanarError> {
    let corner_point = match corner {
        CellCorner::BottomLeft => PointMicrometers::new(rectangle.x0, rectangle.y0),
        CellCorner::BottomRight => PointMicrometers::new(rectangle.x1, rectangle.y0),
        CellCorner::TopRight => PointMicrometers::new(rectangle.x1, rectangle.y1),
        CellCorner::TopLeft => PointMicrometers::new(rectangle.x0, rectangle.y1),
    };
    if !clipped {
        return Ok(CornerPoints {
            incoming: corner_point,
            outgoing: corner_point,
            clipped: false,
        });
    }

    let inset_x = (rectangle.x1 - rectangle.x0) / 2;
    let inset_y = (rectangle.y1 - rectangle.y0) / 2;
    if inset_x == 0 || inset_y == 0 {
        return Err(PlanarError::DiagonalContactTooFine {
            x: corner_point.x(),
            y: corner_point.y(),
        });
    }
    let (incoming, outgoing) = match corner {
        CellCorner::BottomLeft => (
            PointMicrometers::new(rectangle.x0, rectangle.y0 + inset_y),
            PointMicrometers::new(rectangle.x0 + inset_x, rectangle.y0),
        ),
        CellCorner::BottomRight => (
            PointMicrometers::new(rectangle.x1 - inset_x, rectangle.y0),
            PointMicrometers::new(rectangle.x1, rectangle.y0 + inset_y),
        ),
        CellCorner::TopRight => (
            PointMicrometers::new(rectangle.x1, rectangle.y1 - inset_y),
            PointMicrometers::new(rectangle.x1 - inset_x, rectangle.y1),
        ),
        CellCorner::TopLeft => (
            PointMicrometers::new(rectangle.x0 + inset_x, rectangle.y1),
            PointMicrometers::new(rectangle.x0, rectangle.y1 - inset_y),
        ),
    };
    Ok(CornerPoints {
        incoming,
        outgoing,
        clipped: true,
    })
}

fn solid_at(solid: &[u8], width: usize, height: usize, x: i64, y: i64) -> bool {
    let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y)) else {
        return false;
    };
    x < width && y < height && solid[y * width + x] == 1
}

pub(crate) fn complementary_boundary_edges(
    footprint: RectMicrometers,
    x_edges: &[i64],
    y_edges: &[i64],
    substrate_edges: &[DirectedEdge],
) -> Result<Vec<DirectedEdge>, PlanarError> {
    let perimeter_edges = x_edges
        .len()
        .checked_sub(1)
        .and_then(|horizontal| {
            y_edges
                .len()
                .checked_sub(1)
                .and_then(|vertical| horizontal.checked_add(vertical))
        })
        .and_then(|half_perimeter| half_perimeter.checked_mul(2))
        .ok_or(PlanarError::ArithmeticOverflow)?;
    if perimeter_edges > MAX_BOUNDARY_EDGES {
        return Err(PlanarError::TooManyBoundaryEdges {
            required: perimeter_edges,
            max: MAX_BOUNDARY_EDGES,
        });
    }

    let mut segments = BTreeMap::<(PointMicrometers, PointMicrometers), i8>::new();

    for window in x_edges.windows(2) {
        add_or_cancel(
            &mut segments,
            DirectedEdge::new(
                PointMicrometers::new(window[0], 0),
                PointMicrometers::new(window[1], 0),
            ),
        )?;
    }
    for window in y_edges.windows(2) {
        add_or_cancel(
            &mut segments,
            DirectedEdge::new(
                PointMicrometers::new(footprint.width(), window[0]),
                PointMicrometers::new(footprint.width(), window[1]),
            ),
        )?;
    }
    for window in x_edges.windows(2).rev() {
        add_or_cancel(
            &mut segments,
            DirectedEdge::new(
                PointMicrometers::new(window[1], footprint.height()),
                PointMicrometers::new(window[0], footprint.height()),
            ),
        )?;
    }
    for window in y_edges.windows(2).rev() {
        add_or_cancel(
            &mut segments,
            DirectedEdge::new(
                PointMicrometers::new(0, window[1]),
                PointMicrometers::new(0, window[0]),
            ),
        )?;
    }
    for edge in substrate_edges {
        add_or_cancel(&mut segments, edge.reversed())?;
    }

    if segments.len() > MAX_BOUNDARY_EDGES {
        return Err(PlanarError::TooManyBoundaryEdges {
            required: segments.len(),
            max: MAX_BOUNDARY_EDGES,
        });
    }

    segments
        .into_iter()
        .filter_map(|((first, second), winding)| match winding {
            1 => Some(Ok(DirectedEdge::new(first, second))),
            -1 => Some(Ok(DirectedEdge::new(second, first))),
            0 => None,
            _ => Some(Err(PlanarError::InvalidTopology(
                "a partition edge has inconsistent multiplicity",
            ))),
        })
        .collect()
}

fn push_edge(edges: &mut Vec<DirectedEdge>, edge: DirectedEdge) -> Result<(), PlanarError> {
    if edge.start == edge.end {
        return Ok(());
    }
    let required = edges
        .len()
        .checked_add(1)
        .ok_or(PlanarError::ArithmeticOverflow)?;
    if required > MAX_BOUNDARY_EDGES {
        return Err(PlanarError::TooManyBoundaryEdges {
            required,
            max: MAX_BOUNDARY_EDGES,
        });
    }
    edges.push(edge);
    Ok(())
}

fn add_or_cancel(
    segments: &mut BTreeMap<(PointMicrometers, PointMicrometers), i8>,
    edge: DirectedEdge,
) -> Result<(), PlanarError> {
    if edge.start == edge.end {
        return Err(PlanarError::InvalidTopology(
            "a boundary edge has zero length",
        ));
    }
    let (key, delta) = if edge.start < edge.end {
        ((edge.start, edge.end), 1_i8)
    } else {
        ((edge.end, edge.start), -1_i8)
    };
    let at_capacity = segments.len() >= MAX_BOUNDARY_EDGES;
    match segments.entry(key) {
        Entry::Vacant(entry) => {
            if at_capacity {
                return Err(PlanarError::TooManyBoundaryEdges {
                    required: MAX_BOUNDARY_EDGES + 1,
                    max: MAX_BOUNDARY_EDGES,
                });
            }
            entry.insert(delta);
        }
        Entry::Occupied(mut entry) => {
            let winding = entry
                .get()
                .checked_add(delta)
                .ok_or(PlanarError::ArithmeticOverflow)?;
            if winding == 0 {
                entry.remove();
            } else {
                *entry.get_mut() = winding;
            }
        }
    }
    Ok(())
}

/// Stitches cell boundaries with a deterministic left-turn preference.
///
/// At a checkerboard saddle this keeps four-connected cells in separate rings;
/// the choice is independent of insertion or map iteration order.
pub(crate) fn trace_rings(
    mut edges: Vec<DirectedEdge>,
) -> Result<Vec<Vec<PointMicrometers>>, PlanarError> {
    edges.sort_unstable();
    let mut used = vec![false; edges.len()];
    let mut rings = Vec::new();

    for first_index in 0..edges.len() {
        if used[first_index] {
            continue;
        }

        let first = edges[first_index];
        let start = first.start;
        let mut vertices = vec![start];
        let mut current_index = first_index;
        let mut steps = 0_usize;

        loop {
            if used[current_index] {
                return Err(PlanarError::InvalidTopology(
                    "a boundary edge was reached more than once",
                ));
            }
            used[current_index] = true;
            steps = steps
                .checked_add(1)
                .ok_or(PlanarError::ArithmeticOverflow)?;
            if steps > edges.len() {
                return Err(PlanarError::InvalidTopology(
                    "a boundary walk did not close",
                ));
            }

            let current = edges[current_index];
            if current.end == start {
                break;
            }
            vertices.push(current.end);

            let lower = edges.partition_point(|edge| edge.start < current.end);
            let upper = edges.partition_point(|edge| edge.start <= current.end);
            let mut candidates = (lower..upper).filter(|candidate| !used[*candidate]);
            let first = candidates.next().ok_or(PlanarError::InvalidTopology(
                "a boundary vertex has no unused outgoing edge",
            ))?;
            let next = if let Some(second) = candidates.next() {
                let incoming = direction(current)?;
                let mut best = first;
                let mut best_key = (
                    turn_rank(incoming, direction(edges[first])?),
                    edges[first].end,
                );
                for candidate in std::iter::once(second).chain(candidates) {
                    let key = (
                        turn_rank(incoming, direction(edges[candidate])?),
                        edges[candidate].end,
                    );
                    if key < best_key {
                        best = candidate;
                        best_key = key;
                    }
                }
                best
            } else {
                first
            };
            current_index = next;
        }

        let vertices = remove_collinear_vertices(vertices)?;
        rings.push(canonicalize_oriented(vertices));
    }

    rings.sort();
    Ok(rings)
}

fn direction(edge: DirectedEdge) -> Result<u8, PlanarError> {
    match (
        edge.end.x().cmp(&edge.start.x()),
        edge.end.y().cmp(&edge.start.y()),
    ) {
        (std::cmp::Ordering::Greater, std::cmp::Ordering::Equal) => Ok(0),
        (std::cmp::Ordering::Equal, std::cmp::Ordering::Greater) => Ok(1),
        (std::cmp::Ordering::Less, std::cmp::Ordering::Equal) => Ok(2),
        (std::cmp::Ordering::Equal, std::cmp::Ordering::Less) => Ok(3),
        _ => Err(PlanarError::InvalidTopology(
            "a traced cell edge is not axis-aligned",
        )),
    }
}

fn turn_rank(incoming: u8, outgoing: u8) -> u8 {
    match (outgoing + 4 - incoming) % 4 {
        1 => 0, // left
        0 => 1, // straight
        3 => 2, // right
        _ => 3, // reverse
    }
}

fn remove_collinear_vertices(
    mut vertices: Vec<PointMicrometers>,
) -> Result<Vec<PointMicrometers>, PlanarError> {
    vertices.dedup();
    if vertices.first() == vertices.last() {
        vertices.pop();
    }

    loop {
        if vertices.len() < 3 {
            return Err(PlanarError::InvalidTopology(
                "a contour has fewer than three vertices",
            ));
        }
        let mut keep = vec![true; vertices.len()];
        let mut removed = false;
        for index in 0..vertices.len() {
            let previous = vertices[(index + vertices.len() - 1) % vertices.len()];
            let current = vertices[index];
            let next = vertices[(index + 1) % vertices.len()];
            if is_forward_collinear(previous, current, next) {
                keep[index] = false;
                removed = true;
            }
        }
        if !removed {
            break;
        }
        vertices = vertices
            .into_iter()
            .zip(keep)
            .filter_map(|(vertex, keep)| keep.then_some(vertex))
            .collect();
    }

    if signed_double_area(&vertices) == 0 {
        return Err(PlanarError::InvalidTopology(
            "a contour has zero signed area",
        ));
    }
    Ok(vertices)
}

fn is_forward_collinear(
    previous: PointMicrometers,
    current: PointMicrometers,
    next: PointMicrometers,
) -> bool {
    let first_x = i128::from(current.x()) - i128::from(previous.x());
    let first_y = i128::from(current.y()) - i128::from(previous.y());
    let second_x = i128::from(next.x()) - i128::from(current.x());
    let second_y = i128::from(next.y()) - i128::from(current.y());
    first_x * second_y - first_y * second_x == 0 && first_x * second_x + first_y * second_y >= 0
}

fn canonicalize_oriented(mut vertices: Vec<PointMicrometers>) -> Vec<PointMicrometers> {
    let start = vertices
        .iter()
        .enumerate()
        .min_by_key(|(_, point)| **point)
        .map_or(0, |(index, _)| index);
    vertices.rotate_left(start);
    vertices
}

fn canonical_undirected(vertices: &[PointMicrometers]) -> Vec<PointMicrometers> {
    let forward = canonicalize_oriented(vertices.to_vec());
    let reverse = canonicalize_oriented(vertices.iter().rev().copied().collect());
    forward.min(reverse)
}

pub(crate) fn simplify_shared_rings(
    substrate: Vec<Vec<PointMicrometers>>,
    fill: Vec<Vec<PointMicrometers>>,
    footprint: RectMicrometers,
    tolerance: i64,
) -> (Vec<Vec<PointMicrometers>>, Vec<Vec<PointMicrometers>>) {
    if tolerance == 0 {
        return (substrate, fill);
    }

    let mut unique = BTreeSet::new();
    let mut examined_vertices = 0_usize;
    for ring in substrate.iter().chain(&fill) {
        examined_vertices = match examined_vertices.checked_add(ring.len()) {
            Some(total) if total <= MAX_SIMPLIFICATION_VALIDATION_VERTICES => total,
            _ => return (substrate, fill),
        };
        unique.insert(canonical_undirected(ring));
    }

    let originals = unique.into_iter().collect::<Vec<_>>();
    let candidates = originals
        .iter()
        .map(|ring| {
            if touches_footprint(ring, footprint) {
                ring.clone()
            } else {
                simplify_closed_ring(ring, tolerance)
            }
        })
        .collect::<Vec<_>>();

    let replacements = if validate_simplification(&originals, &candidates) {
        originals
            .iter()
            .cloned()
            .zip(candidates)
            .collect::<BTreeMap<_, _>>()
    } else {
        BTreeMap::new()
    };

    let apply = |rings: Vec<Vec<PointMicrometers>>| {
        rings
            .into_iter()
            .map(|ring| {
                let desired_area = signed_double_area(&ring);
                let key = canonical_undirected(&ring);
                let mut replacement = replacements.get(&key).cloned().unwrap_or(key);
                if signed_double_area(&replacement).signum() != desired_area.signum() {
                    replacement.reverse();
                }
                canonicalize_oriented(replacement)
            })
            .collect()
    };

    (apply(substrate), apply(fill))
}

fn touches_footprint(vertices: &[PointMicrometers], footprint: RectMicrometers) -> bool {
    vertices.iter().any(|point| {
        point.x() == 0
            || point.y() == 0
            || point.x() == footprint.width()
            || point.y() == footprint.height()
    })
}

fn simplify_closed_ring(vertices: &[PointMicrometers], tolerance: i64) -> Vec<PointMicrometers> {
    if vertices.len() <= 3 {
        return vertices.to_vec();
    }

    let anchor = vertices[0];
    let opposite = vertices
        .iter()
        .enumerate()
        .skip(1)
        .max_by_key(|(_, point)| squared_distance(anchor, **point))
        .map_or(1, |(index, _)| index);

    let first = simplify_open_chain(&vertices[..=opposite], tolerance);
    let mut second_chain = vertices[opposite..].to_vec();
    second_chain.push(anchor);
    let second = simplify_open_chain(&second_chain, tolerance);

    let mut result = first;
    result.pop();
    result.extend(second);
    result.pop();
    canonical_undirected(&result)
}

fn simplify_open_chain(points: &[PointMicrometers], tolerance: i64) -> Vec<PointMicrometers> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0_usize, points.len() - 1)];

    while let Some((start, end)) = stack.pop() {
        if end <= start + 1 {
            continue;
        }
        let candidate = (start + 1..end)
            .map(|index| {
                (
                    index,
                    perpendicular_distance_key(points[index], points[start], points[end]),
                )
            })
            .max_by_key(|(index, distance)| (*distance, std::cmp::Reverse(*index)));
        if let Some((index, distance)) = candidate {
            let segment_length = squared_distance(points[start], points[end]);
            let tolerance_squared = i128::from(tolerance) * i128::from(tolerance);
            if distance > tolerance_squared * segment_length {
                keep[index] = true;
                stack.push((index, end));
                stack.push((start, index));
            }
        }
    }

    points
        .iter()
        .copied()
        .zip(keep)
        .filter_map(|(point, keep)| keep.then_some(point))
        .collect()
}

fn perpendicular_distance_key(
    point: PointMicrometers,
    start: PointMicrometers,
    end: PointMicrometers,
) -> i128 {
    let dx = i128::from(end.x()) - i128::from(start.x());
    let dy = i128::from(end.y()) - i128::from(start.y());
    let px = i128::from(point.x()) - i128::from(start.x());
    let py = i128::from(point.y()) - i128::from(start.y());
    let cross = dx * py - dy * px;
    cross * cross
}

fn squared_distance(first: PointMicrometers, second: PointMicrometers) -> i128 {
    let dx = i128::from(second.x()) - i128::from(first.x());
    let dy = i128::from(second.y()) - i128::from(first.y());
    dx * dx + dy * dy
}

fn validate_simplification(
    originals: &[Vec<PointMicrometers>],
    candidates: &[Vec<PointMicrometers>],
) -> bool {
    if candidates
        .iter()
        .any(|ring| ring.len() < 3 || signed_double_area(ring) == 0 || ring_self_intersects(ring))
    {
        return false;
    }
    for first in 0..candidates.len() {
        for second in first + 1..candidates.len() {
            if rings_intersect(&candidates[first], &candidates[second]) {
                return false;
            }
            let original_relation = point_location(originals[first][0], &originals[second]);
            let candidate_relation = point_location(candidates[first][0], &candidates[second]);
            if original_relation != candidate_relation {
                return false;
            }
            let original_relation = point_location(originals[second][0], &originals[first]);
            let candidate_relation = point_location(candidates[second][0], &candidates[first]);
            if original_relation != candidate_relation {
                return false;
            }
        }
    }
    true
}

fn ring_self_intersects(ring: &[PointMicrometers]) -> bool {
    for first in 0..ring.len() {
        let first_next = (first + 1) % ring.len();
        for second in first + 1..ring.len() {
            let second_next = (second + 1) % ring.len();
            if first == second
                || first == second_next
                || first_next == second
                || first_next == second_next
            {
                continue;
            }
            if segments_intersect(
                ring[first],
                ring[first_next],
                ring[second],
                ring[second_next],
            ) {
                return true;
            }
        }
    }
    false
}

fn rings_intersect(first: &[PointMicrometers], second: &[PointMicrometers]) -> bool {
    first
        .iter()
        .copied()
        .zip(first.iter().copied().cycle().skip(1))
        .take(first.len())
        .any(|(a, b)| {
            second
                .iter()
                .copied()
                .zip(second.iter().copied().cycle().skip(1))
                .take(second.len())
                .any(|(c, d)| segments_intersect(a, b, c, d))
        })
}

fn segments_intersect(
    a: PointMicrometers,
    b: PointMicrometers,
    c: PointMicrometers,
    d: PointMicrometers,
) -> bool {
    let ab_c = orientation(a, b, c);
    let ab_d = orientation(a, b, d);
    let cd_a = orientation(c, d, a);
    let cd_b = orientation(c, d, b);
    (ab_c == 0 && point_on_segment(c, a, b))
        || (ab_d == 0 && point_on_segment(d, a, b))
        || (cd_a == 0 && point_on_segment(a, c, d))
        || (cd_b == 0 && point_on_segment(b, c, d))
        || (ab_c.signum() != ab_d.signum() && cd_a.signum() != cd_b.signum())
}

fn orientation(a: PointMicrometers, b: PointMicrometers, c: PointMicrometers) -> i128 {
    (i128::from(b.x()) - i128::from(a.x())) * (i128::from(c.y()) - i128::from(a.y()))
        - (i128::from(b.y()) - i128::from(a.y())) * (i128::from(c.x()) - i128::from(a.x()))
}

fn point_on_segment(
    point: PointMicrometers,
    start: PointMicrometers,
    end: PointMicrometers,
) -> bool {
    orientation(start, end, point) == 0
        && point.x() >= start.x().min(end.x())
        && point.x() <= start.x().max(end.x())
        && point.y() >= start.y().min(end.y())
        && point.y() <= start.y().max(end.y())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointLocation {
    Outside,
    Inside,
    Boundary,
}

fn point_location(point: PointMicrometers, ring: &[PointMicrometers]) -> PointLocation {
    let mut inside = false;
    for (start, end) in ring
        .iter()
        .copied()
        .zip(ring.iter().copied().cycle().skip(1))
        .take(ring.len())
    {
        if point_on_segment(point, start, end) {
            return PointLocation::Boundary;
        }
        if (start.y() > point.y()) != (end.y() > point.y()) {
            let cross = orientation(start, end, point);
            if (end.y() > start.y() && cross > 0) || (end.y() < start.y() && cross < 0) {
                inside = !inside;
            }
        }
    }
    if inside {
        PointLocation::Inside
    } else {
        PointLocation::Outside
    }
}

fn ensure_hole_containment_work(
    hole_count: usize,
    exterior_vertex_count: usize,
    max: u128,
) -> Result<(), PlanarError> {
    let required = (hole_count as u128)
        .checked_mul(exterior_vertex_count as u128)
        .ok_or(PlanarError::ArithmeticOverflow)?;
    if required > max {
        return Err(PlanarError::HoleContainmentWorkLimitExceeded { required, max });
    }
    Ok(())
}

pub(crate) fn assemble_region(rings: Vec<Vec<PointMicrometers>>) -> Result<RegionSet, PlanarError> {
    let mut exteriors = Vec::<(Vec<PointMicrometers>, Vec<Vec<PointMicrometers>>)>::new();
    let mut holes = Vec::new();

    for ring in rings {
        match signed_double_area(&ring).signum() {
            1 => exteriors.push((canonicalize_oriented(ring), Vec::new())),
            -1 => holes.push(canonicalize_oriented(ring)),
            _ => {
                return Err(PlanarError::InvalidTopology(
                    "a traced ring has zero signed area",
                ));
            }
        }
    }

    let exterior_vertex_count = exteriors.iter().try_fold(0_usize, |total, (exterior, _)| {
        total
            .checked_add(exterior.len())
            .ok_or(PlanarError::ArithmeticOverflow)
    })?;
    ensure_hole_containment_work(
        holes.len(),
        exterior_vertex_count,
        MAX_HOLE_CONTAINMENT_WORK,
    )?;

    for hole in holes {
        let sample = hole[0];
        let owner = exteriors
            .iter()
            .enumerate()
            .filter(|(_, (exterior, _))| {
                matches!(
                    point_location(sample, exterior),
                    PointLocation::Inside | PointLocation::Boundary
                )
            })
            .min_by_key(|(_, (exterior, _))| signed_double_area(exterior).abs())
            .map(|(index, _)| index)
            .ok_or(PlanarError::InvalidTopology(
                "a hole is not contained by an exterior ring",
            ))?;
        exteriors[owner].1.push(hole);
    }

    let mut polygons = exteriors
        .into_iter()
        .map(|(exterior, mut holes)| {
            holes.sort();
            Polygon::from_oriented_rings(
                Ring::from_oriented_vertices(exterior),
                holes
                    .into_iter()
                    .map(Ring::from_oriented_vertices)
                    .collect(),
            )
        })
        .collect::<Vec<_>>();
    polygons.sort_by(|first, second| {
        first
            .exterior()
            .vertices()
            .cmp(second.exterior().vertices())
            .then_with(|| first.holes().len().cmp(&second.holes().len()))
    });
    Ok(RegionSet::from_polygons(polygons))
}

fn signed_double_area(vertices: &[PointMicrometers]) -> i128 {
    vertices
        .iter()
        .copied()
        .zip(vertices.iter().copied().cycle().skip(1))
        .take(vertices.len())
        .map(|(start, end)| {
            i128::from(start.x()) * i128::from(end.y())
                - i128::from(start.y()) * i128::from(end.x())
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: i64, y: i64) -> PointMicrometers {
        PointMicrometers::new(x, y)
    }

    #[test]
    fn left_turn_rule_separates_diagonal_cells() {
        let rings = trace_rings(vec![
            DirectedEdge::new(point(0, 0), point(1, 0)),
            DirectedEdge::new(point(1, 0), point(1, 1)),
            DirectedEdge::new(point(1, 1), point(0, 1)),
            DirectedEdge::new(point(0, 1), point(0, 0)),
            DirectedEdge::new(point(1, 1), point(2, 1)),
            DirectedEdge::new(point(2, 1), point(2, 2)),
            DirectedEdge::new(point(2, 2), point(1, 2)),
            DirectedEdge::new(point(1, 2), point(1, 1)),
        ])
        .expect("diagonal contours trace");

        assert_eq!(rings.len(), 2);
        assert!(rings.iter().all(|ring| signed_double_area(ring) == 2));
    }

    #[test]
    fn complement_cancels_footprint_edges() {
        let footprint = RectMicrometers::from_positive_size(10, 10);
        let substrate =
            substrate_boundary_edges(&[0, 10], &[0, 10], &[1], 1, 1).expect("boundary builds");
        let fill = complementary_boundary_edges(footprint, &[0, 10], &[0, 10], &substrate)
            .expect("complement builds");

        assert!(fill.is_empty());
    }

    #[test]
    fn hole_containment_work_is_bounded_before_assignment() {
        assert_eq!(ensure_hole_containment_work(3, 5, 15), Ok(()));
        assert_eq!(
            ensure_hole_containment_work(3, 5, 14),
            Err(PlanarError::HoleContainmentWorkLimitExceeded {
                required: 15,
                max: 14,
            })
        );
    }

    #[test]
    fn guarded_rdp_reuses_one_internal_boundary() {
        let ccw = vec![
            point(2, 2),
            point(4, 2),
            point(4, 3),
            point(5, 3),
            point(5, 5),
            point(2, 5),
        ];
        let clockwise = ccw.iter().rev().copied().collect::<Vec<_>>();
        let (substrate, fill) = simplify_shared_rings(
            vec![ccw],
            vec![clockwise],
            RectMicrometers::from_positive_size(10, 10),
            1,
        );

        assert_eq!(
            canonical_undirected(&substrate[0]),
            canonical_undirected(&fill[0])
        );
        assert!(substrate[0].len() <= 6);
    }
}
