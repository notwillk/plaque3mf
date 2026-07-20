use std::collections::{BTreeMap, BTreeSet};

use plaque3mf_document::{BinaryMask, CanonicalArtwork, Dimension, PhysicalSizeMicrometers};

use super::*;

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

fn options(border: i64, minimum_feature: i64, tolerance: i64) -> PlanarOptions {
    PlanarOptions::new(border, minimum_feature, tolerance).expect("test options are valid")
}

fn point(x: i64, y: i64) -> PointMicrometers {
    PointMicrometers::new(x, y)
}

#[test]
fn pixel_edges_use_half_up_rounding_and_reject_collapsed_cells() {
    assert_eq!(
        axis_edges(5, 2, Dimension::Width).expect("grid is representable"),
        [0, 3, 5]
    );
    assert_eq!(
        axis_edges(7, 4, Dimension::Height).expect("grid is representable"),
        [0, 2, 4, 5, 7]
    );
    assert_eq!(
        axis_edges(1, 2, Dimension::Width),
        Err(PlanarError::ResolutionTooFine {
            dimension: Dimension::Width,
            pixels: 2,
            extent: 1,
        })
    );
}

#[test]
fn top_left_raster_origin_maps_to_upper_left_physical_cell() {
    let partition = partition_artwork(&artwork(5, 7, 2, 2, &[1, 0, 0, 0]), options(0, 1, 0))
        .expect("artwork partitions");
    let substrate = partition.substrate_upper();

    assert_eq!(substrate.polygons().len(), 1);
    assert_eq!(
        substrate.polygons()[0].exterior().vertices(),
        [point(0, 4), point(3, 4), point(3, 7), point(0, 7)]
    );
    assert_eq!(substrate.double_area(), 18);
    assert_partition_area(&partition);
    assert_shared_edges_are_reversed(&partition);
}

#[test]
fn empty_and_full_masks_are_valid_extreme_partitions() {
    let empty = partition_artwork(&artwork(200, 100, 2, 1, &[0, 0]), options(0, 1, 0))
        .expect("empty mask partitions");
    assert!(empty.substrate_upper().is_empty());
    assert_eq!(empty.fill_parts().len(), 1);
    assert_eq!(empty.fill_parts()[0].double_area(), 40_000);

    let full = partition_artwork(&artwork(200, 100, 2, 1, &[1, 1]), options(0, 1, 0))
        .expect("full mask partitions");
    assert_eq!(full.substrate_upper().polygons().len(), 1);
    assert_eq!(full.substrate_upper().double_area(), 40_000);
    assert!(full.fill_parts().is_empty());
}

#[test]
fn exact_border_frame_is_unioned_before_complement_extraction() {
    let partition = partition_artwork(&artwork(1_000, 800, 1, 1, &[0]), options(100, 1, 0))
        .expect("border partitions");

    assert_eq!(partition.substrate_upper().polygons().len(), 1);
    let frame = &partition.substrate_upper().polygons()[0];
    assert_eq!(frame.holes().len(), 1);
    assert_eq!(frame.double_area(), 640_000);
    assert_eq!(partition.fill_parts().len(), 1);
    assert_eq!(partition.fill_parts()[0].double_area(), 960_000);
    assert_eq!(
        partition.fill_parts()[0].exterior().vertices(),
        [
            point(100, 100),
            point(900, 100),
            point(900, 700),
            point(100, 700),
        ]
    );
    assert_partition_area(&partition);
    assert_shared_edges_are_reversed(&partition);
}

#[test]
fn border_is_revalidated_against_actual_fitted_artwork() {
    let error = partition_artwork(&artwork(1_000, 100, 1, 1, &[0]), options(60, 1, 0))
        .expect_err("border cannot consume the fitted height");

    assert_eq!(
        error,
        PlanarError::BorderDoesNotFitArtwork {
            width: 1_000,
            height: 100,
            border: 60,
        }
    );
}

#[test]
fn disk_opening_removes_subminimum_foreground_before_tracing() {
    let partition = partition_artwork(
        &artwork(
            300,
            300,
            3,
            3,
            &[
                0, 0, 0, //
                0, 1, 0, //
                0, 0, 0,
            ],
        ),
        options(0, 200, 0),
    )
    .expect("small feature is cleaned");

    assert!(partition.substrate_upper().is_empty());
    assert_eq!(partition.fill_parts().len(), 1);
    assert_partition_area(&partition);
}

#[test]
fn holes_and_diagonal_contacts_have_deterministic_topology() {
    let hole = partition_artwork(
        &artwork(
            300,
            300,
            3,
            3,
            &[
                1, 1, 1, //
                1, 0, 1, //
                1, 1, 1,
            ],
        ),
        options(0, 1, 0),
    )
    .expect("hole partitions");
    assert_eq!(hole.substrate_upper().polygons().len(), 1);
    assert_eq!(hole.substrate_upper().polygons()[0].holes().len(), 1);
    assert_eq!(hole.fill_parts().len(), 1);
    assert_shared_edges_are_reversed(&hole);

    let diagonal = partition_artwork(&artwork(200, 200, 2, 2, &[1, 0, 0, 1]), options(0, 1, 0))
        .expect("checkerboard partitions");
    assert_eq!(diagonal.substrate_upper().polygons().len(), 2);
    assert_eq!(diagonal.fill_parts().len(), 1);
    let first_vertices = diagonal.substrate_upper().polygons()[0]
        .exterior()
        .vertices()
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let second_vertices = diagonal.substrate_upper().polygons()[1]
        .exterior()
        .vertices()
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert!(first_vertices.is_disjoint(&second_vertices));
    let actual_rings = diagonal
        .substrate_upper()
        .polygons()
        .iter()
        .map(|polygon| polygon.exterior().vertices().to_vec())
        .collect::<BTreeSet<_>>();
    let expected_rings = [
        vec![
            point(0, 100),
            point(50, 100),
            point(100, 150),
            point(100, 200),
            point(0, 200),
        ],
        vec![
            point(100, 0),
            point(200, 0),
            point(200, 100),
            point(150, 100),
            point(100, 50),
        ],
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert_eq!(actual_rings, expected_rings);
    assert!(
        diagonal
            .substrate_upper()
            .polygons()
            .iter()
            .all(|polygon| polygon.double_area() == 17_500)
    );
    assert_eq!(diagonal.fill_parts()[0].double_area(), 45_000);
    assert_partition_area(&diagonal);
    assert_shared_edges_are_reversed(&diagonal);
}

#[test]
fn diagonal_contact_requires_room_for_an_integer_chamfer() {
    let error = partition_artwork(&artwork(2, 4, 2, 2, &[1, 0, 0, 1]), options(0, 1, 0))
        .expect_err("a one-micrometre cell axis cannot be chamfered");

    assert_eq!(error, PlanarError::DiagonalContactTooFine { x: 1, y: 2 });
}

#[test]
fn enabled_simplification_reuses_boundaries_and_freezes_the_footprint() {
    let input = artwork(
        700,
        700,
        7,
        7,
        &[
            0, 0, 0, 0, 0, 0, 0, //
            0, 0, 1, 1, 0, 0, 0, //
            0, 0, 1, 1, 1, 0, 0, //
            0, 0, 1, 1, 1, 1, 0, //
            0, 0, 0, 0, 0, 0, 0, //
            0, 0, 0, 0, 0, 0, 0, //
            0, 0, 0, 0, 0, 0, 0,
        ],
    );
    let original =
        partition_artwork(&input, options(0, 1, 0)).expect("unsimplified partition succeeds");
    let simplified =
        partition_artwork(&input, options(0, 1, 100)).expect("simplified partition succeeds");
    let repeated =
        partition_artwork(&input, options(0, 1, 100)).expect("repeated simplification succeeds");

    assert!(
        simplified.substrate_upper().polygons()[0]
            .exterior()
            .vertices()
            .len()
            < original.substrate_upper().polygons()[0]
                .exterior()
                .vertices()
                .len()
    );
    assert_eq!(simplified, repeated);
    assert_eq!(
        simplified.fill_parts()[0].exterior().vertices(),
        simplified.footprint().corners()
    );
    assert_partition_area(&simplified);
    assert_shared_edges_are_reversed(&simplified);
    assert_canonical_geometry(&simplified);
}

#[test]
fn foreground_opening_never_closes_a_narrow_complementary_gap() {
    let partition = partition_artwork(
        &artwork(
            500,
            500,
            5,
            5,
            &[
                1, 1, 0, 1, 1, //
                1, 1, 0, 1, 1, //
                1, 1, 0, 1, 1, //
                1, 1, 0, 1, 1, //
                1, 1, 0, 1, 1,
            ],
        ),
        options(0, 200, 0),
    )
    .expect("narrow gap remains a valid complement");

    assert_eq!(partition.substrate_upper().polygons().len(), 2);
    assert_eq!(partition.fill_parts().len(), 1);
    assert_eq!(
        partition.fill_parts()[0].exterior().vertices(),
        [
            point(200, 0),
            point(300, 0),
            point(300, 500),
            point(200, 500),
        ]
    );
    assert_partition_area(&partition);
    assert_shared_edges_are_reversed(&partition);
}

#[test]
fn nested_ring_and_island_assign_holes_to_the_smallest_exterior() {
    let partition = partition_artwork(
        &artwork(
            700,
            700,
            7,
            7,
            &[
                0, 0, 0, 0, 0, 0, 0, //
                0, 1, 1, 1, 1, 1, 0, //
                0, 1, 0, 0, 0, 1, 0, //
                0, 1, 0, 1, 0, 1, 0, //
                0, 1, 0, 0, 0, 1, 0, //
                0, 1, 1, 1, 1, 1, 0, //
                0, 0, 0, 0, 0, 0, 0,
            ],
        ),
        options(0, 1, 0),
    )
    .expect("nested components partition");

    assert_eq!(partition.substrate_upper().polygons().len(), 2);
    assert_eq!(partition.substrate_upper().polygons()[0].holes().len(), 1);
    assert!(partition.substrate_upper().polygons()[1].holes().is_empty());
    assert_eq!(partition.fill_parts().len(), 2);
    assert!(
        partition
            .fill_parts()
            .iter()
            .all(|polygon| polygon.holes().len() == 1)
    );
    assert_partition_area(&partition);
    assert_shared_edges_are_reversed(&partition);
    assert_canonical_geometry(&partition);
}

#[test]
fn public_resource_preflights_return_typed_errors() {
    let intervals = MAX_AXIS_INTERVALS + 1;
    let wide_pixels = vec![0; intervals];
    let wide_error = partition_artwork(
        &artwork(intervals as i64, 1, intervals as u32, 1, &wide_pixels),
        options(0, 1, 0),
    )
    .expect_err("oversized axis is rejected");

    assert_eq!(
        wide_error,
        PlanarError::TooManyAxisIntervals {
            dimension: Dimension::Width,
            required: intervals,
            max: MAX_AXIS_INTERVALS,
        }
    );

    const TALL_PIXELS: u32 = 32_768;
    let tall_pixels = vec![1; TALL_PIXELS as usize];
    let morphology_error = partition_artwork(
        &artwork(
            i64::from(TALL_PIXELS),
            i64::from(TALL_PIXELS),
            1,
            TALL_PIXELS,
            &tall_pixels,
        ),
        options(0, i64::from(TALL_PIXELS), 0),
    )
    .expect_err("oversized morphology work is rejected");

    assert_eq!(
        morphology_error,
        PlanarError::MorphologyWorkLimitExceeded {
            required: 2_147_483_648,
            max: MAX_MORPHOLOGY_WORK,
        }
    );
}

#[test]
fn every_three_by_three_mask_is_an_exact_complementary_partition() {
    for bits in 0_u16..512 {
        let pixels = (0..9)
            .map(|index| u8::from(bits & (1 << index) != 0))
            .collect::<Vec<_>>();
        let first = partition_artwork(&artwork(300, 300, 3, 3, &pixels), options(0, 1, 0))
            .unwrap_or_else(|error| panic!("mask {bits:#05x} failed: {error}"));
        let second = partition_artwork(&artwork(300, 300, 3, 3, &pixels), options(0, 1, 0))
            .expect("repeated partition succeeds");

        assert_eq!(first, second, "mask {bits:#05x} is nondeterministic");
        assert_partition_area(&first);
        assert_shared_edges_are_reversed(&first);
        assert_canonical_geometry(&first);
    }
}

#[test]
fn oversized_artwork_is_rejected_before_fixed_point_products() {
    let error = partition_artwork(
        &artwork(MAX_ARTWORK_EXTENT_MICROMETERS + 1, 1, 1, 1, &[0]),
        options(0, 1, 0),
    )
    .expect_err("oversized geometry is rejected");

    assert!(matches!(error, PlanarError::ArtworkExtentTooLarge { .. }));
}

fn assert_partition_area(partition: &PlanarPartition) {
    let fill_area = partition
        .fill_parts()
        .iter()
        .map(Polygon::double_area)
        .sum::<i128>();
    assert_eq!(
        partition.substrate_upper().double_area() + fill_area,
        partition.footprint().double_area()
    );
}

fn assert_shared_edges_are_reversed(partition: &PlanarPartition) {
    let mut substrate_edges = BTreeMap::new();
    for polygon in partition.substrate_upper().polygons() {
        add_polygon_edges(&mut substrate_edges, polygon);
    }
    let mut fill_edges = BTreeMap::new();
    for polygon in partition.fill_parts() {
        add_polygon_edges(&mut fill_edges, polygon);
    }

    for ((start, end), count) in &substrate_edges {
        if !is_footprint_edge(*start, *end, partition.footprint()) {
            assert_eq!(
                fill_edges.get(&(*end, *start)),
                Some(count),
                "substrate edge {start:?}->{end:?} is not shared in reverse"
            );
        }
    }
    for ((start, end), count) in &fill_edges {
        if !is_footprint_edge(*start, *end, partition.footprint()) {
            assert_eq!(
                substrate_edges.get(&(*end, *start)),
                Some(count),
                "fill edge {start:?}->{end:?} is not shared in reverse"
            );
        }
    }
}

fn add_polygon_edges(
    edges: &mut BTreeMap<(PointMicrometers, PointMicrometers), usize>,
    polygon: &Polygon,
) {
    for ring in std::iter::once(polygon.exterior()).chain(polygon.holes()) {
        for (start, end) in ring
            .vertices()
            .iter()
            .copied()
            .zip(ring.vertices().iter().copied().cycle().skip(1))
            .take(ring.vertices().len())
        {
            *edges.entry((start, end)).or_default() += 1;
        }
    }
}

fn is_footprint_edge(
    start: PointMicrometers,
    end: PointMicrometers,
    footprint: RectMicrometers,
) -> bool {
    (start.x() == 0 && end.x() == 0)
        || (start.y() == 0 && end.y() == 0)
        || (start.x() == footprint.width() && end.x() == footprint.width())
        || (start.y() == footprint.height() && end.y() == footprint.height())
}

fn assert_canonical_geometry(partition: &PlanarPartition) {
    for polygon in partition
        .substrate_upper()
        .polygons()
        .iter()
        .chain(partition.fill_parts())
    {
        assert!(polygon.exterior().is_ccw());
        assert_eq!(
            polygon.exterior().vertices().first(),
            polygon.exterior().vertices().iter().min()
        );
        for hole in polygon.holes() {
            assert!(!hole.is_ccw());
            assert_eq!(hole.vertices().first(), hole.vertices().iter().min());
        }
    }
}
