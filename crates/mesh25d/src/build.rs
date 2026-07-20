//! Direct boundary construction, canonicalization, and invariant validation.

use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

use plaque3mf_planar::{PlanarPartition, PointMicrometers, Polygon, RectMicrometers, Ring};

use crate::{
    MAX_INPUT_CONTOUR_VERTICES, MAX_MESH_TRIANGLES, MAX_MESH_VERTICES, MeshError, MeshOptions,
    MeshPart, PartModel, Triangle, TriangleMesh, VertexMicrometers,
    triangulate::{WorkBudget, triangulate_rings},
};

pub(crate) fn build(
    partition: &PlanarPartition,
    options: MeshOptions,
) -> Result<PartModel, MeshError> {
    let backing = options.backing_thickness_micrometers();
    let total = options.total_thickness_micrometers();
    let upper_height = total - backing;
    let stats = collect_model_stats(partition)?;
    let footprint_ring = segmented_footprint(partition, stats.bridged_input_vertices()?)?;
    preflight_mesh(&stats, footprint_ring.len())?;

    let mut work = WorkBudget::new();
    let footprint_triangles = triangulate_rings(
        &footprint_ring,
        &[],
        partition.footprint().double_area(),
        &mut work,
    )?;
    let substrate_caps = partition
        .substrate_upper()
        .polygons()
        .iter()
        .map(|polygon| triangulate_polygon(polygon, &mut work))
        .collect::<Result<Vec<_>, _>>()?;
    let fill_caps = partition
        .fill_parts()
        .iter()
        .map(|polygon| triangulate_polygon(polygon, &mut work))
        .collect::<Result<Vec<_>, _>>()?;

    let substrate_expected = substrate_expected_volume(partition, backing, upper_height)?;
    let mut substrate = MeshBuilder::default();
    substrate.add_cap(&footprint_triangles, 0, false)?;
    substrate.add_ring_walls(&footprint_ring, 0, backing)?;
    for triangles in &fill_caps {
        substrate.add_cap(triangles, backing, true)?;
    }
    for (polygon, triangles) in partition
        .substrate_upper()
        .polygons()
        .iter()
        .zip(&substrate_caps)
    {
        substrate.add_cap(triangles, total, true)?;
        substrate.add_polygon_walls(polygon, backing, total)?;
    }
    let substrate = substrate.finish(MeshPart::Substrate, substrate_expected)?;

    let mut fill_parts = Vec::with_capacity(partition.fill_parts().len());
    for (index, (polygon, triangles)) in partition.fill_parts().iter().zip(&fill_caps).enumerate() {
        let expected = scaled_volume(polygon.double_area(), upper_height)?;
        let mut builder = MeshBuilder::default();
        builder.add_cap(triangles, backing, false)?;
        builder.add_cap(triangles, total, true)?;
        builder.add_polygon_walls(polygon, backing, total)?;
        fill_parts.push(builder.finish(MeshPart::Fill(index), expected)?);
    }

    let actual_combined =
        fill_parts
            .iter()
            .try_fold(substrate.signed_six_volume(), |volume, mesh| {
                volume
                    .checked_add(mesh.signed_six_volume())
                    .ok_or(MeshError::ArithmeticOverflow)
            })?;
    let expected_combined = scaled_volume(partition.footprint().double_area(), total)?;
    if actual_combined != expected_combined {
        return Err(MeshError::CombinedVolumeMismatch {
            expected: expected_combined,
            actual: actual_combined,
        });
    }

    Ok(PartModel::from_validated(substrate, fill_parts))
}

fn triangulate_polygon(
    polygon: &Polygon,
    work: &mut WorkBudget,
) -> Result<Vec<[PointMicrometers; 3]>, MeshError> {
    let holes = polygon.holes().iter().collect::<Vec<_>>();
    triangulate_rings(
        polygon.exterior().vertices(),
        &holes,
        polygon.double_area(),
        work,
    )
}

#[derive(Debug, Clone, Copy)]
struct PolygonStats {
    vertices: usize,
    holes: usize,
    cap_triangles: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct ModelStats {
    substrate_vertices: usize,
    fill_vertices: usize,
    substrate_cap_triangles: usize,
    fill_cap_triangles: usize,
    hole_count: usize,
}

impl ModelStats {
    fn bridged_input_vertices(self) -> Result<usize, MeshError> {
        self.substrate_vertices
            .checked_add(self.fill_vertices)
            .and_then(|count| count.checked_add(self.hole_count.checked_mul(2)?))
            .ok_or(MeshError::ArithmeticOverflow)
    }
}

fn polygon_stats(polygon: &Polygon) -> Result<PolygonStats, MeshError> {
    let vertices =
        polygon
            .holes()
            .iter()
            .try_fold(polygon.exterior().vertices().len(), |count, hole| {
                count
                    .checked_add(hole.vertices().len())
                    .ok_or(MeshError::ArithmeticOverflow)
            })?;
    let holes = polygon.holes().len();
    let cap_triangles = vertices
        .checked_add(holes.checked_mul(2).ok_or(MeshError::ArithmeticOverflow)?)
        .and_then(|count| count.checked_sub(2))
        .ok_or(MeshError::ArithmeticOverflow)?;
    Ok(PolygonStats {
        vertices,
        holes,
        cap_triangles,
    })
}

fn collect_model_stats(partition: &PlanarPartition) -> Result<ModelStats, MeshError> {
    let mut model = ModelStats::default();
    for polygon in partition.substrate_upper().polygons() {
        let stats = polygon_stats(polygon)?;
        add_count(&mut model.substrate_vertices, stats.vertices)?;
        add_count(&mut model.substrate_cap_triangles, stats.cap_triangles)?;
        add_count(&mut model.hole_count, stats.holes)?;
    }
    for polygon in partition.fill_parts() {
        let stats = polygon_stats(polygon)?;
        add_count(&mut model.fill_vertices, stats.vertices)?;
        add_count(&mut model.fill_cap_triangles, stats.cap_triangles)?;
        add_count(&mut model.hole_count, stats.holes)?;
    }

    let required = model
        .bridged_input_vertices()?
        .checked_add(4)
        .ok_or(MeshError::ArithmeticOverflow)?;
    if required > MAX_INPUT_CONTOUR_VERTICES {
        return Err(MeshError::TooManyInputContourVertices {
            required,
            max: MAX_INPUT_CONTOUR_VERTICES,
        });
    }
    Ok(model)
}

fn add_count(total: &mut usize, amount: usize) -> Result<(), MeshError> {
    *total = total
        .checked_add(amount)
        .ok_or(MeshError::ArithmeticOverflow)?;
    Ok(())
}

fn preflight_mesh(stats: &ModelStats, footprint_vertices: usize) -> Result<(), MeshError> {
    let substrate_vertices = stats.substrate_vertices;
    let fill_vertices = stats.fill_vertices;
    let substrate_cap_triangles = stats.substrate_cap_triangles;
    let fill_cap_triangles = stats.fill_cap_triangles;

    let estimated_vertices = footprint_vertices
        .checked_mul(2)
        .and_then(|count| count.checked_add(substrate_vertices.checked_mul(2)?))
        .and_then(|count| count.checked_add(fill_vertices.checked_mul(3)?))
        .ok_or(MeshError::ArithmeticOverflow)?;
    if estimated_vertices > MAX_MESH_VERTICES {
        return Err(MeshError::TooManyMeshVertices {
            required: estimated_vertices,
            max: MAX_MESH_VERTICES,
        });
    }

    let footprint_cap = footprint_vertices
        .checked_sub(2)
        .ok_or(MeshError::ArithmeticOverflow)?;
    let substrate_triangles = footprint_cap
        .checked_add(
            footprint_vertices
                .checked_mul(2)
                .ok_or(MeshError::ArithmeticOverflow)?,
        )
        .and_then(|count| count.checked_add(fill_cap_triangles))
        .and_then(|count| count.checked_add(substrate_cap_triangles))
        .and_then(|count| count.checked_add(substrate_vertices.checked_mul(2)?))
        .ok_or(MeshError::ArithmeticOverflow)?;
    let fill_triangles = fill_cap_triangles
        .checked_mul(2)
        .and_then(|caps| caps.checked_add(fill_vertices.checked_mul(2)?))
        .ok_or(MeshError::ArithmeticOverflow)?;
    let estimated_triangles = substrate_triangles
        .checked_add(fill_triangles)
        .ok_or(MeshError::ArithmeticOverflow)?;
    if estimated_triangles > MAX_MESH_TRIANGLES {
        return Err(MeshError::TooManyMeshTriangles {
            required: estimated_triangles,
            max: MAX_MESH_TRIANGLES,
        });
    }
    Ok(())
}

fn segmented_footprint(
    partition: &PlanarPartition,
    bridged_input_vertices: usize,
) -> Result<Vec<PointMicrometers>, MeshError> {
    let footprint = partition.footprint();
    let mut bottom = BTreeSet::from([0, footprint.width()]);
    let mut right = BTreeSet::from([0, footprint.height()]);
    let mut top = bottom.clone();
    let mut left = right.clone();

    for polygon in partition
        .substrate_upper()
        .polygons()
        .iter()
        .chain(partition.fill_parts())
    {
        record_ring_breakpoints(
            polygon.exterior(),
            footprint,
            &mut bottom,
            &mut right,
            &mut top,
            &mut left,
            bridged_input_vertices,
        )?;
        for hole in polygon.holes() {
            record_ring_breakpoints(
                hole,
                footprint,
                &mut bottom,
                &mut right,
                &mut top,
                &mut left,
                bridged_input_vertices,
            )?;
        }
    }

    let mut ring = Vec::with_capacity(bottom.len() + right.len() + top.len() + left.len() - 4);
    ring.extend(bottom.iter().copied().map(|x| PointMicrometers::new(x, 0)));
    ring.extend(
        right
            .iter()
            .copied()
            .filter(|y| *y > 0)
            .map(|y| PointMicrometers::new(footprint.width(), y)),
    );
    ring.extend(
        top.iter()
            .rev()
            .copied()
            .filter(|x| *x < footprint.width())
            .map(|x| PointMicrometers::new(x, footprint.height())),
    );
    ring.extend(
        left.iter()
            .rev()
            .copied()
            .filter(|y| *y > 0 && *y < footprint.height())
            .map(|y| PointMicrometers::new(0, y)),
    );
    Ok(ring)
}

fn record_ring_breakpoints(
    ring: &Ring,
    footprint: RectMicrometers,
    bottom: &mut BTreeSet<i64>,
    right: &mut BTreeSet<i64>,
    top: &mut BTreeSet<i64>,
    left: &mut BTreeSet<i64>,
    bridged_input_vertices: usize,
) -> Result<(), MeshError> {
    for point in ring.vertices() {
        if point.y() == 0 {
            bottom.insert(point.x());
        }
        if point.x() == footprint.width() {
            right.insert(point.y());
        }
        if point.y() == footprint.height() {
            top.insert(point.x());
        }
        if point.x() == 0 {
            left.insert(point.y());
        }
        check_footprint_limit(bottom, right, top, left, bridged_input_vertices)?;
    }
    Ok(())
}

fn check_footprint_limit(
    bottom: &BTreeSet<i64>,
    right: &BTreeSet<i64>,
    top: &BTreeSet<i64>,
    left: &BTreeSet<i64>,
    bridged_input_vertices: usize,
) -> Result<(), MeshError> {
    let footprint_vertices = bottom
        .len()
        .checked_add(right.len())
        .and_then(|count| count.checked_add(top.len()))
        .and_then(|count| count.checked_add(left.len()))
        .and_then(|count| count.checked_sub(4))
        .ok_or(MeshError::ArithmeticOverflow)?;
    let required = bridged_input_vertices
        .checked_add(footprint_vertices)
        .ok_or(MeshError::ArithmeticOverflow)?;
    if required > MAX_INPUT_CONTOUR_VERTICES {
        return Err(MeshError::TooManyInputContourVertices {
            required,
            max: MAX_INPUT_CONTOUR_VERTICES,
        });
    }
    Ok(())
}

fn substrate_expected_volume(
    partition: &PlanarPartition,
    backing: i64,
    upper_height: i64,
) -> Result<i128, MeshError> {
    let base = partition
        .footprint()
        .double_area()
        .checked_mul(i128::from(backing))
        .ok_or(MeshError::ArithmeticOverflow)?;
    let upper = partition
        .substrate_upper()
        .double_area()
        .checked_mul(i128::from(upper_height))
        .ok_or(MeshError::ArithmeticOverflow)?;
    base.checked_add(upper)
        .and_then(|volume| volume.checked_mul(3))
        .ok_or(MeshError::ArithmeticOverflow)
}

fn scaled_volume(double_area: i128, height: i64) -> Result<i128, MeshError> {
    double_area
        .checked_mul(i128::from(height))
        .and_then(|volume| volume.checked_mul(3))
        .ok_or(MeshError::ArithmeticOverflow)
}

#[derive(Default)]
struct MeshBuilder {
    vertices: Vec<VertexMicrometers>,
    vertex_indices: BTreeMap<VertexMicrometers, u32>,
    triangles: Vec<[u32; 3]>,
}

impl MeshBuilder {
    fn add_cap(
        &mut self,
        triangles: &[[PointMicrometers; 3]],
        z: i64,
        upward: bool,
    ) -> Result<(), MeshError> {
        for triangle in triangles {
            let points = if upward {
                [triangle[0], triangle[1], triangle[2]]
            } else {
                [triangle[0], triangle[2], triangle[1]]
            };
            self.add_triangle([
                VertexMicrometers::new(points[0].x(), points[0].y(), z),
                VertexMicrometers::new(points[1].x(), points[1].y(), z),
                VertexMicrometers::new(points[2].x(), points[2].y(), z),
            ])?;
        }
        Ok(())
    }

    fn add_polygon_walls(
        &mut self,
        polygon: &Polygon,
        low: i64,
        high: i64,
    ) -> Result<(), MeshError> {
        self.add_ring_walls(polygon.exterior().vertices(), low, high)?;
        for hole in polygon.holes() {
            self.add_ring_walls(hole.vertices(), low, high)?;
        }
        Ok(())
    }

    fn add_ring_walls(
        &mut self,
        ring: &[PointMicrometers],
        low: i64,
        high: i64,
    ) -> Result<(), MeshError> {
        for (start, end) in ring
            .iter()
            .copied()
            .zip(ring.iter().copied().cycle().skip(1))
            .take(ring.len())
        {
            let start_low = VertexMicrometers::new(start.x(), start.y(), low);
            let end_low = VertexMicrometers::new(end.x(), end.y(), low);
            let start_high = VertexMicrometers::new(start.x(), start.y(), high);
            let end_high = VertexMicrometers::new(end.x(), end.y(), high);
            if start < end {
                self.add_triangle([start_low, end_low, end_high])?;
                self.add_triangle([start_low, end_high, start_high])?;
            } else {
                self.add_triangle([start_low, end_low, start_high])?;
                self.add_triangle([end_low, end_high, start_high])?;
            }
        }
        Ok(())
    }

    fn add_triangle(&mut self, vertices: [VertexMicrometers; 3]) -> Result<(), MeshError> {
        let required = self
            .triangles
            .len()
            .checked_add(1)
            .ok_or(MeshError::ArithmeticOverflow)?;
        if required > MAX_MESH_TRIANGLES {
            return Err(MeshError::TooManyMeshTriangles {
                required,
                max: MAX_MESH_TRIANGLES,
            });
        }
        let mut indices = [0_u32; 3];
        for (slot, vertex) in indices.iter_mut().zip(vertices) {
            *slot = self.intern_vertex(vertex)?;
        }
        self.triangles.push(indices);
        Ok(())
    }

    fn intern_vertex(&mut self, vertex: VertexMicrometers) -> Result<u32, MeshError> {
        if let Some(index) = self.vertex_indices.get(&vertex) {
            return Ok(*index);
        }
        let required = self
            .vertices
            .len()
            .checked_add(1)
            .ok_or(MeshError::ArithmeticOverflow)?;
        if required > MAX_MESH_VERTICES {
            return Err(MeshError::TooManyMeshVertices {
                required,
                max: MAX_MESH_VERTICES,
            });
        }
        let index =
            u32::try_from(self.vertices.len()).map_err(|_| MeshError::TriangleIndexOverflow)?;
        self.vertices.push(vertex);
        self.vertex_indices.insert(vertex, index);
        Ok(index)
    }

    fn finish(self, part: MeshPart, expected_volume: i128) -> Result<TriangleMesh, MeshError> {
        let mut ordered = self.vertices.into_iter().enumerate().collect::<Vec<_>>();
        ordered.sort_by_key(|(_, vertex)| *vertex);
        let mut remap = vec![0_u32; ordered.len()];
        let mut vertices = Vec::with_capacity(ordered.len());
        for (new, (old, vertex)) in ordered.into_iter().enumerate() {
            remap[old] = u32::try_from(new).map_err(|_| MeshError::TriangleIndexOverflow)?;
            vertices.push(vertex);
        }

        let mut triangles = self
            .triangles
            .into_iter()
            .map(|indices| {
                let mut remapped = [
                    remap[indices[0] as usize],
                    remap[indices[1] as usize],
                    remap[indices[2] as usize],
                ];
                let minimum = remapped
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, index)| **index)
                    .map_or(0, |(position, _)| position);
                remapped.rotate_left(minimum);
                Triangle::new(remapped)
            })
            .collect::<Vec<_>>();
        triangles.sort_unstable();

        let signed_six_volume = validate_mesh(part, &vertices, &triangles)?;
        if signed_six_volume != expected_volume {
            return Err(MeshError::VolumeMismatch {
                part,
                expected: expected_volume,
                actual: signed_six_volume,
            });
        }
        Ok(TriangleMesh::from_validated(
            vertices,
            triangles,
            signed_six_volume,
        ))
    }
}

#[derive(Debug, Clone, Copy)]
struct EdgeUse {
    uses: u32,
    balance: i32,
    first_triangle: usize,
    second_triangle: Option<usize>,
}

fn validate_mesh(
    part: MeshPart,
    vertices: &[VertexMicrometers],
    triangles: &[Triangle],
) -> Result<i128, MeshError> {
    if triangles.is_empty() {
        return Err(MeshError::DisconnectedMesh { part });
    }

    let mut first_incident = vec![None; vertices.len()];
    let mut incident_counts = vec![0_usize; vertices.len()];
    let mut faces = BTreeSet::new();
    let mut edges = BTreeMap::<(u32, u32), EdgeUse>::new();
    let mut sets = DisjointSets::new(triangles.len());
    let mut volume = 0_i128;

    for (triangle_index, triangle) in triangles.iter().copied().enumerate() {
        let indices = triangle.indices();
        for index in indices {
            let Some(first) = first_incident.get_mut(index as usize) else {
                return Err(MeshError::InvalidTriangleIndex {
                    part,
                    triangle: triangle_index,
                    index,
                    vertex_count: vertices.len(),
                });
            };
            first.get_or_insert(triangle_index);
            incident_counts[index as usize] = incident_counts[index as usize]
                .checked_add(1)
                .ok_or(MeshError::ArithmeticOverflow)?;
        }
        if indices[0] == indices[1] || indices[1] == indices[2] || indices[2] == indices[0] {
            return Err(MeshError::DegenerateTriangle {
                part,
                triangle: triangle_index,
            });
        }
        let a = vertices[indices[0] as usize];
        let b = vertices[indices[1] as usize];
        let c = vertices[indices[2] as usize];
        if cross_is_zero(a, b, c) {
            return Err(MeshError::DegenerateTriangle {
                part,
                triangle: triangle_index,
            });
        }

        let mut face = indices;
        face.sort_unstable();
        if !faces.insert(face) {
            return Err(MeshError::DuplicateTriangle {
                part,
                triangle: triangle_index,
            });
        }

        for (start, end) in [
            (indices[0], indices[1]),
            (indices[1], indices[2]),
            (indices[2], indices[0]),
        ] {
            let (key, delta) = if start < end {
                ((start, end), 1_i32)
            } else {
                ((end, start), -1_i32)
            };
            match edges.entry(key) {
                Entry::Vacant(entry) => {
                    entry.insert(EdgeUse {
                        uses: 1,
                        balance: delta,
                        first_triangle: triangle_index,
                        second_triangle: None,
                    });
                }
                Entry::Occupied(mut entry) => {
                    let edge = entry.get_mut();
                    edge.uses = edge
                        .uses
                        .checked_add(1)
                        .ok_or(MeshError::ArithmeticOverflow)?;
                    edge.balance = edge
                        .balance
                        .checked_add(delta)
                        .ok_or(MeshError::ArithmeticOverflow)?;
                    if edge.second_triangle.is_none() {
                        edge.second_triangle = Some(triangle_index);
                    }
                    sets.union(edge.first_triangle, triangle_index);
                }
            }
        }

        volume = volume
            .checked_add(signed_six_tetrahedron(a, b, c))
            .ok_or(MeshError::ArithmeticOverflow)?;
    }

    for (&(first, second), edge) in &edges {
        if edge.uses != 2 || edge.balance != 0 {
            return Err(MeshError::NonManifoldEdge {
                part,
                first,
                second,
                uses: edge.uses,
                balance: edge.balance,
            });
        }
    }
    for (index, first) in first_incident.into_iter().enumerate() {
        let vertex = u32::try_from(index).map_err(|_| MeshError::TriangleIndexOverflow)?;
        let Some(first) = first else {
            return Err(MeshError::UnreferencedVertex { part, vertex });
        };
        validate_vertex_link(
            part,
            vertex,
            first,
            incident_counts[index],
            triangles,
            &edges,
        )?;
    }
    let root = sets.find(0);
    if (1..triangles.len()).any(|triangle| sets.find(triangle) != root) {
        return Err(MeshError::DisconnectedMesh { part });
    }
    Ok(volume)
}

fn validate_vertex_link(
    part: MeshPart,
    vertex: u32,
    first_triangle: usize,
    incident_count: usize,
    triangles: &[Triangle],
    edges: &BTreeMap<(u32, u32), EdgeUse>,
) -> Result<(), MeshError> {
    let mut previous = None;
    let mut current = first_triangle;
    let mut traversed = 0_usize;

    loop {
        if traversed >= incident_count {
            return Err(MeshError::NonManifoldVertex { part, vertex });
        }
        traversed += 1;
        let indices = triangles[current].indices();
        let Some(position) = indices.iter().position(|index| *index == vertex) else {
            return Err(MeshError::NonManifoldVertex { part, vertex });
        };
        let neighbors = [
            adjacent_triangle(part, vertex, indices[(position + 1) % 3], current, edges)?,
            adjacent_triangle(part, vertex, indices[(position + 2) % 3], current, edges)?,
        ];
        if neighbors[0] == neighbors[1] {
            return Err(MeshError::NonManifoldVertex { part, vertex });
        }
        let next = match previous {
            None => neighbors[0],
            Some(previous) if neighbors[0] == previous => neighbors[1],
            Some(previous) if neighbors[1] == previous => neighbors[0],
            Some(_) => return Err(MeshError::NonManifoldVertex { part, vertex }),
        };
        previous = Some(current);
        current = next;
        if current == first_triangle {
            break;
        }
    }

    if traversed != incident_count {
        return Err(MeshError::NonManifoldVertex { part, vertex });
    }
    Ok(())
}

fn adjacent_triangle(
    part: MeshPart,
    vertex: u32,
    other: u32,
    current: usize,
    edges: &BTreeMap<(u32, u32), EdgeUse>,
) -> Result<usize, MeshError> {
    let key = if vertex < other {
        (vertex, other)
    } else {
        (other, vertex)
    };
    let Some(edge) = edges.get(&key) else {
        return Err(MeshError::NonManifoldVertex { part, vertex });
    };
    if edge.first_triangle == current {
        return edge
            .second_triangle
            .ok_or(MeshError::NonManifoldVertex { part, vertex });
    }
    if edge.second_triangle == Some(current) {
        return Ok(edge.first_triangle);
    }
    Err(MeshError::NonManifoldVertex { part, vertex })
}

fn cross_is_zero(a: VertexMicrometers, b: VertexMicrometers, c: VertexMicrometers) -> bool {
    let ab = [
        i128::from(b.x()) - i128::from(a.x()),
        i128::from(b.y()) - i128::from(a.y()),
        i128::from(b.z()) - i128::from(a.z()),
    ];
    let ac = [
        i128::from(c.x()) - i128::from(a.x()),
        i128::from(c.y()) - i128::from(a.y()),
        i128::from(c.z()) - i128::from(a.z()),
    ];
    let cross = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    cross == [0, 0, 0]
}

fn signed_six_tetrahedron(
    a: VertexMicrometers,
    b: VertexMicrometers,
    c: VertexMicrometers,
) -> i128 {
    let ax = i128::from(a.x());
    let ay = i128::from(a.y());
    let az = i128::from(a.z());
    let bx = i128::from(b.x());
    let by = i128::from(b.y());
    let bz = i128::from(b.z());
    let cx = i128::from(c.x());
    let cy = i128::from(c.y());
    let cz = i128::from(c.z());
    ax * (by * cz - bz * cy) - ay * (bx * cz - bz * cx) + az * (bx * cy - by * cx)
}

struct DisjointSets {
    parents: Vec<usize>,
    ranks: Vec<u8>,
}

impl DisjointSets {
    fn new(count: usize) -> Self {
        Self {
            parents: (0..count).collect(),
            ranks: vec![0; count],
        }
    }

    fn find(&mut self, value: usize) -> usize {
        let parent = self.parents[value];
        if parent != value {
            self.parents[value] = self.find(parent);
        }
        self.parents[value]
    }

    fn union(&mut self, first: usize, second: usize) {
        let mut first_root = self.find(first);
        let mut second_root = self.find(second);
        if first_root == second_root {
            return;
        }
        if self.ranks[first_root] < self.ranks[second_root] {
            std::mem::swap(&mut first_root, &mut second_root);
        }
        self.parents[second_root] = first_root;
        if self.ranks[first_root] == self.ranks[second_root] {
            self.ranks[first_root] += 1;
        }
    }
}
