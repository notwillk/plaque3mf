use std::collections::{BTreeMap, BTreeSet, VecDeque};

use plaque3mf_document::{BinaryMask, CanonicalArtwork, PhysicalSizeMicrometers};
use plaque3mf_planar::{PlanarOptions, PlanarPartition, RectMicrometers, partition_artwork};

use super::*;

const BACKING: i64 = 100;
const TOTAL: i64 = 250;

fn artwork(
    physical_width: i64,
    physical_height: i64,
    pixel_width: u32,
    pixel_height: u32,
    pixels: &[u8],
) -> CanonicalArtwork {
    CanonicalArtwork::new(
        PhysicalSizeMicrometers::new(physical_width, physical_height)
            .expect("test physical size is valid"),
        BinaryMask::new(pixel_width, pixel_height, pixels.to_vec()).expect("test mask is valid"),
    )
}

fn partition(
    physical_width: i64,
    physical_height: i64,
    pixel_width: u32,
    pixel_height: u32,
    pixels: &[u8],
    border: i64,
) -> PlanarPartition {
    let options = PlanarOptions::new(border, 1, 0).expect("test planar options are valid");
    partition_artwork(
        &artwork(
            physical_width,
            physical_height,
            pixel_width,
            pixel_height,
            pixels,
        ),
        options,
    )
    .expect("test artwork partitions")
}

fn mesh_options() -> MeshOptions {
    MeshOptions::new(BACKING, TOTAL).expect("test mesh options are valid")
}

fn build(partition: &PlanarPartition) -> PartModel {
    build_part_model(partition, mesh_options()).expect("test partition meshes")
}

#[derive(Debug, Default)]
struct EdgeIncidence {
    directed_uses: Vec<(usize, i8)>,
}

fn assert_model_invariants(partition: &PlanarPartition, options: MeshOptions, model: &PartModel) {
    assert_closed_outward_mesh(model.substrate(), 0, options.total_thickness_micrometers());
    assert_eq!(model.fill_parts().len(), partition.fill_parts().len());
    for mesh in model.fill_parts() {
        assert_closed_outward_mesh(
            mesh,
            options.backing_thickness_micrometers(),
            options.total_thickness_micrometers(),
        );
    }

    let backing = i128::from(options.backing_thickness_micrometers());
    let upper =
        i128::from(options.total_thickness_micrometers() - options.backing_thickness_micrometers());
    let expected_substrate = 3
        * (partition.footprint().double_area() * backing
            + partition.substrate_upper().double_area() * upper);
    assert_eq!(model.substrate().signed_six_volume(), expected_substrate);

    for (mesh, polygon) in model.fill_parts().iter().zip(partition.fill_parts()) {
        assert_eq!(mesh.signed_six_volume(), 3 * polygon.double_area() * upper);
    }

    let actual_combined = model
        .fill_parts()
        .iter()
        .fold(model.substrate().signed_six_volume(), |volume, mesh| {
            volume + mesh.signed_six_volume()
        });
    let expected_combined =
        3 * partition.footprint().double_area() * i128::from(options.total_thickness_micrometers());
    assert_eq!(actual_combined, expected_combined);

    assert_exact_interfaces(partition.footprint(), options, model);
}

fn assert_closed_outward_mesh(mesh: &TriangleMesh, bottom: i64, top: i64) {
    assert!(!mesh.vertices().is_empty());
    assert!(!mesh.triangles().is_empty());
    assert!(mesh.vertices().windows(2).all(|pair| pair[0] < pair[1]));
    assert!(mesh.triangles().windows(2).all(|pair| pair[0] < pair[1]));

    let mut referenced = vec![false; mesh.vertices().len()];
    let mut faces = BTreeSet::new();
    let mut edges = BTreeMap::<(u32, u32), EdgeIncidence>::new();
    let mut recomputed_volume = 0_i128;

    for (triangle_index, triangle) in mesh.triangles().iter().copied().enumerate() {
        let indices = triangle.indices();
        assert!(indices[0] <= indices[1] && indices[0] <= indices[2]);
        for index in indices {
            let referenced = referenced
                .get_mut(index as usize)
                .unwrap_or_else(|| panic!("triangle {triangle_index} has invalid index {index}"));
            *referenced = true;
        }

        let mut face = indices;
        face.sort_unstable();
        assert!(
            faces.insert(face),
            "triangle {triangle_index} duplicates a face"
        );

        let vertices = triangle_vertices(mesh, triangle);
        let cross = triangle_cross(vertices);
        assert_ne!(cross, [0, 0, 0], "triangle {triangle_index} is degenerate");
        assert!(
            vertices
                .iter()
                .all(|vertex| vertex.z() >= bottom && vertex.z() <= top),
            "triangle {triangle_index} leaves the part's vertical bounds"
        );
        if vertices.iter().all(|vertex| vertex.z() == vertices[0].z()) {
            if vertices[0].z() == bottom {
                assert!(cross[2] < 0, "bottom cap triangle points upward");
            } else {
                assert!(cross[2] > 0, "upper cap triangle points downward");
            }
        }

        for (start, end) in [
            (indices[0], indices[1]),
            (indices[1], indices[2]),
            (indices[2], indices[0]),
        ] {
            let (key, direction) = if start < end {
                ((start, end), 1)
            } else {
                ((end, start), -1)
            };
            edges
                .entry(key)
                .or_default()
                .directed_uses
                .push((triangle_index, direction));
        }

        recomputed_volume += signed_six_tetrahedron(vertices);
    }

    assert!(referenced.into_iter().all(|used| used));
    assert!(recomputed_volume > 0, "mesh orientation must be outward");
    assert_eq!(mesh.signed_six_volume(), recomputed_volume);

    let mut adjacency = vec![Vec::new(); mesh.triangles().len()];
    for (edge, incidence) in edges {
        assert_eq!(
            incidence.directed_uses.len(),
            2,
            "edge {edge:?} is not two-manifold"
        );
        let first = incidence.directed_uses[0];
        let second = incidence.directed_uses[1];
        assert_eq!(
            i16::from(first.1) + i16::from(second.1),
            0,
            "edge {edge:?} has inconsistent winding"
        );
        adjacency[first.0].push(second.0);
        adjacency[second.0].push(first.0);
    }

    let mut visited = vec![false; mesh.triangles().len()];
    let mut queue = VecDeque::from([0]);
    visited[0] = true;
    while let Some(triangle) = queue.pop_front() {
        for adjacent in adjacency[triangle].iter().copied() {
            if !visited[adjacent] {
                visited[adjacent] = true;
                queue.push_back(adjacent);
            }
        }
    }
    assert!(
        visited.into_iter().all(|seen| seen),
        "mesh has disconnected shells"
    );
}

fn triangle_vertices(mesh: &TriangleMesh, triangle: Triangle) -> [VertexMicrometers; 3] {
    triangle
        .indices()
        .map(|index| mesh.vertices()[index as usize])
}

fn triangle_cross(vertices: [VertexMicrometers; 3]) -> [i128; 3] {
    let [a, b, c] = vertices;
    let ab = [
        i128::from(b.x() - a.x()),
        i128::from(b.y() - a.y()),
        i128::from(b.z() - a.z()),
    ];
    let ac = [
        i128::from(c.x() - a.x()),
        i128::from(c.y() - a.y()),
        i128::from(c.z() - a.z()),
    ];
    [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ]
}

fn signed_six_tetrahedron(vertices: [VertexMicrometers; 3]) -> i128 {
    let [a, b, c] = vertices;
    let [ax, ay, az] = [i128::from(a.x()), i128::from(a.y()), i128::from(a.z())];
    let [bx, by, bz] = [i128::from(b.x()), i128::from(b.y()), i128::from(b.z())];
    let [cx, cy, cz] = [i128::from(c.x()), i128::from(c.y()), i128::from(c.z())];
    ax * (by * cz - bz * cy) - ay * (bx * cz - bz * cx) + az * (bx * cy - by * cx)
}

type FaceKey = [VertexMicrometers; 3];

fn face_key(mut vertices: [VertexMicrometers; 3]) -> FaceKey {
    vertices.sort_unstable();
    vertices
}

fn face_map(mesh: &TriangleMesh) -> BTreeMap<FaceKey, [VertexMicrometers; 3]> {
    mesh.triangles()
        .iter()
        .copied()
        .map(|triangle| {
            let vertices = triangle_vertices(mesh, triangle);
            (face_key(vertices), vertices)
        })
        .collect()
}

fn has_opposite_orientation(first: [VertexMicrometers; 3], second: [VertexMicrometers; 3]) -> bool {
    (0..3).any(|offset| {
        first[0] == second[offset]
            && first[1] == second[(offset + 2) % 3]
            && first[2] == second[(offset + 1) % 3]
    })
}

fn on_footprint_wall(vertices: [VertexMicrometers; 3], footprint: RectMicrometers) -> bool {
    vertices.iter().all(|vertex| vertex.x() == 0)
        || vertices
            .iter()
            .all(|vertex| vertex.x() == footprint.width())
        || vertices.iter().all(|vertex| vertex.y() == 0)
        || vertices
            .iter()
            .all(|vertex| vertex.y() == footprint.height())
}

fn assert_exact_interfaces(footprint: RectMicrometers, options: MeshOptions, model: &PartModel) {
    let substrate_faces = face_map(model.substrate());
    for fill in model.fill_parts() {
        let mut shared_faces = 0_usize;
        for triangle in fill.triangles().iter().copied() {
            let vertices = triangle_vertices(fill, triangle);
            let cross = triangle_cross(vertices);
            let is_bottom = vertices
                .iter()
                .all(|vertex| vertex.z() == options.backing_thickness_micrometers());
            let is_vertical = cross[2] == 0;
            let should_be_shared =
                is_bottom || (is_vertical && !on_footprint_wall(vertices, footprint));
            let matching = substrate_faces.get(&face_key(vertices));

            if should_be_shared {
                let substrate =
                    matching.expect("substrate is missing an exact fill interface face");
                assert!(
                    has_opposite_orientation(*substrate, vertices),
                    "shared interface face has matching rather than opposing winding"
                );
                shared_faces += 1;
            } else {
                assert!(
                    matching.is_none(),
                    "external fill face overlaps the substrate"
                );
            }
        }
        assert!(shared_faces > 0, "every fill must rest on the backing");
    }
}

fn mesh_contains_edge(
    mesh: &TriangleMesh,
    first: VertexMicrometers,
    second: VertexMicrometers,
) -> bool {
    mesh.triangles().iter().copied().any(|triangle| {
        let vertices = triangle_vertices(mesh, triangle);
        [
            (vertices[0], vertices[1]),
            (vertices[1], vertices[2]),
            (vertices[2], vertices[0]),
        ]
        .into_iter()
        .any(|(start, end)| (start == first && end == second) || (start == second && end == first))
    })
}

#[test]
fn mesh_options_reject_every_invalid_thickness_relationship() {
    assert_eq!(
        MeshOptions::new(0, 1),
        Err(MeshError::NonPositiveBackingThickness)
    );
    assert_eq!(
        MeshOptions::new(-1, 1),
        Err(MeshError::NonPositiveBackingThickness)
    );
    assert_eq!(
        MeshOptions::new(1, 0),
        Err(MeshError::NonPositiveTotalThickness)
    );
    assert_eq!(
        MeshOptions::new(1, -1),
        Err(MeshError::NonPositiveTotalThickness)
    );
    assert_eq!(
        MeshOptions::new(10, 10),
        Err(MeshError::TotalThicknessNotGreaterThanBacking)
    );
    assert_eq!(
        MeshOptions::new(11, 10),
        Err(MeshError::TotalThicknessNotGreaterThanBacking)
    );
    assert_eq!(
        MeshOptions::new(1, MAX_MESH_HEIGHT_MICROMETERS + 1),
        Err(MeshError::ThicknessTooLarge {
            total: MAX_MESH_HEIGHT_MICROMETERS + 1,
            max: MAX_MESH_HEIGHT_MICROMETERS,
        })
    );

    let maximum = MeshOptions::new(MAX_MESH_HEIGHT_MICROMETERS - 1, MAX_MESH_HEIGHT_MICROMETERS)
        .expect("the exact maximum is accepted");
    assert_eq!(
        maximum.backing_thickness_micrometers(),
        MAX_MESH_HEIGHT_MICROMETERS - 1
    );
    assert_eq!(
        maximum.total_thickness_micrometers(),
        MAX_MESH_HEIGHT_MICROMETERS
    );
}

#[test]
fn empty_and_full_masks_produce_exact_complementary_extremes() {
    let empty_partition = partition(200, 100, 1, 1, &[0], 0);
    let empty = build(&empty_partition);
    assert_eq!(empty.substrate().vertices().len(), 8);
    assert_eq!(empty.substrate().triangles().len(), 12);
    assert_eq!(empty.substrate().signed_six_volume(), 12_000_000);
    assert_eq!(empty.fill_parts().len(), 1);
    assert_eq!(empty.fill_parts()[0].vertices().len(), 8);
    assert_eq!(empty.fill_parts()[0].triangles().len(), 12);
    assert_eq!(empty.fill_parts()[0].signed_six_volume(), 18_000_000);
    assert_model_invariants(&empty_partition, mesh_options(), &empty);

    let full_partition = partition(200, 100, 1, 1, &[1], 0);
    let full = build(&full_partition);
    assert_eq!(full.substrate().signed_six_volume(), 30_000_000);
    assert!(full.fill_parts().is_empty());
    assert_model_invariants(&full_partition, mesh_options(), &full);
}

#[test]
fn half_footprint_has_exact_individual_and_total_volumes() {
    let partition = partition(200, 100, 2, 1, &[1, 0], 0);
    let model = build(&partition);

    assert_eq!(partition.substrate_upper().double_area(), 20_000);
    assert_eq!(partition.fill_parts()[0].double_area(), 20_000);
    assert_eq!(model.substrate().signed_six_volume(), 21_000_000);
    assert_eq!(model.fill_parts()[0].signed_six_volume(), 9_000_000);
    assert_eq!(
        model.substrate().signed_six_volume() + model.fill_parts()[0].signed_six_volume(),
        30_000_000
    );
    assert_model_invariants(&partition, mesh_options(), &model);
}

#[test]
fn border_frame_with_a_hole_meshes_without_losing_the_fill_interface() {
    let partition = partition(1_000, 800, 1, 1, &[0], 100);
    let model = build(&partition);

    assert_eq!(partition.substrate_upper().polygons().len(), 1);
    assert_eq!(partition.substrate_upper().polygons()[0].holes().len(), 1);
    assert_eq!(partition.fill_parts().len(), 1);
    assert_eq!(model.substrate().signed_six_volume(), 768_000_000);
    assert_eq!(model.fill_parts()[0].signed_six_volume(), 432_000_000);
    assert_model_invariants(&partition, mesh_options(), &model);
}

#[test]
fn footprint_walls_are_segmented_at_upper_part_ownership_changes() {
    let partition = partition(300, 100, 3, 1, &[1, 0, 1], 0);
    let model = build(&partition);

    for x in [100, 200] {
        for y in [0, 100] {
            assert!(
                mesh_contains_edge(
                    model.substrate(),
                    VertexMicrometers::new(x, y, 0),
                    VertexMicrometers::new(x, y, BACKING),
                ),
                "missing lower footprint-wall segmentation edge at ({x}, {y})"
            );
        }
    }
    assert_model_invariants(&partition, mesh_options(), &model);
}

#[test]
fn nested_ring_hole_and_island_produce_closed_exact_parts() {
    let pixels = [
        0, 0, 0, 0, 0, 0, 0, //
        0, 1, 1, 1, 1, 1, 0, //
        0, 1, 0, 0, 0, 1, 0, //
        0, 1, 0, 1, 0, 1, 0, //
        0, 1, 0, 0, 0, 1, 0, //
        0, 1, 1, 1, 1, 1, 0, //
        0, 0, 0, 0, 0, 0, 0,
    ];
    let partition = partition(700, 700, 7, 7, &pixels, 0);
    let model = build(&partition);

    assert_eq!(partition.substrate_upper().polygons().len(), 2);
    assert_eq!(partition.fill_parts().len(), 2);
    assert_eq!(model.fill_parts().len(), 2);
    assert_model_invariants(&partition, mesh_options(), &model);
}

#[test]
fn repeated_complex_construction_is_byte_for_byte_deterministic() {
    let pixels = [
        1, 0, 1, 0, //
        1, 1, 0, 0, //
        0, 1, 0, 1, //
        1, 0, 1, 1,
    ];
    let partition = partition(400, 400, 4, 4, &pixels, 25);
    let expected = build(&partition);

    for _ in 0..5 {
        assert_eq!(build(&partition), expected);
    }
    assert_model_invariants(&partition, mesh_options(), &expected);
}

#[test]
fn every_three_by_three_mask_builds_an_exact_deterministic_manifold_model() {
    for bits in 0_u16..512 {
        let pixels = (0..9)
            .map(|index| u8::from(bits & (1 << index) != 0))
            .collect::<Vec<_>>();
        let partition = partition(300, 300, 3, 3, &pixels, 0);
        let first = build_part_model(&partition, mesh_options())
            .unwrap_or_else(|error| panic!("mask {bits:#05x} failed to mesh: {error}"));
        let second = build_part_model(&partition, mesh_options())
            .unwrap_or_else(|error| panic!("mask {bits:#05x} failed on repetition: {error}"));

        assert_eq!(first, second, "mask {bits:#05x} is nondeterministic");
        assert_model_invariants(&partition, mesh_options(), &first);
    }
}
