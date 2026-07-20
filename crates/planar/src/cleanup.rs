//! Deterministic cleanup of the canonical foreground mask.

use plaque3mf_document::BinaryMask;

use crate::{MAX_MORPHOLOGY_WORK, PlanarError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DiskRow {
    dy: i64,
    left: i64,
    right: i64,
}

impl DiskRow {
    const fn reflected(self) -> Self {
        Self {
            dy: -self.dy,
            left: -self.right,
            right: -self.left,
        }
    }
}

/// Opens the foreground with a digital ellipse whose physical diameter is the
/// configured minimum feature width.
///
/// Pixel cells form an anisotropic integer lattice using the same half-up
/// fixed-point breakpoints as contour tracing. Each kernel axis is the smallest
/// cell count for which every placement spans at least the configured physical
/// diameter. A non-divisible grid can therefore require one extra cell at some
/// placements, but cannot retain a feature because an average pitch was rounded
/// up. The resulting cell kernel is sampled as a digital ellipse. Even-sized
/// kernels use a deterministic lower/left anchor; dilation reflects that anchor
/// so the opening does not translate the artwork. Pixels outside the page are
/// background.
pub(crate) fn open_foreground_on_grid(
    mask: &BinaryMask,
    x_edges: &[i64],
    y_edges: &[i64],
    minimum_width_micrometers: i64,
) -> Result<Vec<u8>, PlanarError> {
    if minimum_width_micrometers <= 0 {
        return Err(PlanarError::NonPositiveMinimumFeatureWidth);
    }

    let dimensions = mask.dimensions();
    let width = dimensions.width() as usize;
    let height = dimensions.height() as usize;
    if x_edges.len() != width + 1 || y_edges.len() != height + 1 {
        return Err(PlanarError::InvalidTopology(
            "cleanup breakpoints do not match the raster dimensions",
        ));
    }
    let kernel_width = kernel_axis_length(x_edges, minimum_width_micrometers)?;
    let kernel_height = kernel_axis_length(y_edges, minimum_width_micrometers)?;

    if kernel_width == 1 && kernel_height == 1 {
        return Ok(mask.as_bytes().to_vec());
    }

    // No placement of the kernel fits inside the page. Since erosion treats
    // the exterior as background, the complete opening is known immediately.
    if kernel_width > width || kernel_height > height {
        return Ok(vec![0; dimensions.pixel_count()]);
    }

    ensure_work_within_limit(
        dimensions.pixel_count() as u128,
        kernel_height as u128,
        MAX_MORPHOLOGY_WORK,
    )?;
    let rows = disk_rows(kernel_width, kernel_height)?;

    let mut eroded = vec![1; dimensions.pixel_count()];
    for row in &rows {
        erode_with_row(mask.as_bytes(), &mut eroded, width, height, *row);
    }
    if !eroded.contains(&1) {
        return Ok(eroded);
    }

    let mut opened = vec![0; dimensions.pixel_count()];
    for row in &rows {
        dilate_with_row(&eroded, &mut opened, width, height, row.reflected());
    }
    Ok(opened)
}

fn kernel_axis_length(edges: &[i64], diameter: i64) -> Result<usize, PlanarError> {
    let intervals = edges
        .len()
        .checked_sub(1)
        .ok_or(PlanarError::InvalidTopology(
            "a cleanup axis has no breakpoints",
        ))?;
    if intervals == 0 || edges.windows(2).any(|window| window[0] >= window[1]) {
        return Err(PlanarError::InvalidTopology(
            "cleanup breakpoints are not strictly increasing",
        ));
    }

    let spans_diameter = |cells: usize| {
        (0..=intervals - cells).all(|start| {
            i128::from(edges[start + cells]) - i128::from(edges[start]) >= i128::from(diameter)
        })
    };
    if !spans_diameter(intervals) {
        return intervals
            .checked_add(1)
            .ok_or(PlanarError::ArithmeticOverflow);
    }

    let mut lower = 1_usize;
    let mut upper = intervals;
    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        if spans_diameter(middle) {
            upper = middle;
        } else {
            lower = middle + 1;
        }
    }
    Ok(lower)
}

fn ensure_work_within_limit(
    pixel_count: u128,
    kernel_rows: u128,
    max: u128,
) -> Result<(), PlanarError> {
    let required = pixel_count
        .checked_mul(kernel_rows)
        .and_then(|work| work.checked_mul(2))
        .ok_or(PlanarError::ArithmeticOverflow)?;
    if required > max {
        return Err(PlanarError::MorphologyWorkLimitExceeded { required, max });
    }
    Ok(())
}

fn disk_rows(width: usize, height: usize) -> Result<Vec<DiskRow>, PlanarError> {
    let width_i128 = i128::try_from(width).map_err(|_| PlanarError::ArithmeticOverflow)?;
    let height_i128 = i128::try_from(height).map_err(|_| PlanarError::ArithmeticOverflow)?;
    let width_squared = width_i128
        .checked_mul(width_i128)
        .ok_or(PlanarError::ArithmeticOverflow)?;
    let height_squared = height_i128
        .checked_mul(height_i128)
        .ok_or(PlanarError::ArithmeticOverflow)?;
    let limit = width_squared
        .checked_mul(height_squared)
        .ok_or(PlanarError::ArithmeticOverflow)?;
    let anchor_x = (width - 1) / 2;
    let anchor_y = (height - 1) / 2;
    let mut rows = Vec::with_capacity(height);

    for y in 0..height {
        let normalized_y = i128::try_from(y)
            .map_err(|_| PlanarError::ArithmeticOverflow)?
            .checked_mul(2)
            .and_then(|value| value.checked_add(1))
            .and_then(|value| value.checked_sub(height_i128))
            .ok_or(PlanarError::ArithmeticOverflow)?;
        let vertical = normalized_y
            .checked_mul(normalized_y)
            .and_then(|value| value.checked_mul(width_squared))
            .ok_or(PlanarError::ArithmeticOverflow)?;
        let mut first = None;
        let mut last = None;
        for x in 0..width {
            let normalized_x = i128::try_from(x)
                .map_err(|_| PlanarError::ArithmeticOverflow)?
                .checked_mul(2)
                .and_then(|value| value.checked_add(1))
                .and_then(|value| value.checked_sub(width_i128))
                .ok_or(PlanarError::ArithmeticOverflow)?;
            let horizontal = normalized_x
                .checked_mul(normalized_x)
                .and_then(|value| value.checked_mul(height_squared))
                .ok_or(PlanarError::ArithmeticOverflow)?;
            if horizontal
                .checked_add(vertical)
                .ok_or(PlanarError::ArithmeticOverflow)?
                <= limit
            {
                first.get_or_insert(x);
                last = Some(x);
            }
        }
        let (first, last) = match (first, last) {
            (Some(first), Some(last)) => (first, last),
            (None, None) => ((width - 1) / 2, width / 2),
            _ => {
                return Err(PlanarError::InvalidTopology(
                    "a digital cleanup disk row is only partially defined",
                ));
            }
        };
        rows.push(DiskRow {
            dy: offset_from_anchor(y, anchor_y)?,
            left: offset_from_anchor(first, anchor_x)?,
            right: offset_from_anchor(last, anchor_x)?,
        });
    }
    Ok(rows)
}

fn offset_from_anchor(index: usize, anchor: usize) -> Result<i64, PlanarError> {
    i64::try_from(index)
        .and_then(|index| i64::try_from(anchor).map(|anchor| index - anchor))
        .map_err(|_| PlanarError::ArithmeticOverflow)
}

fn erode_with_row(
    source: &[u8],
    destination: &mut [u8],
    width: usize,
    height: usize,
    disk_row: DiskRow,
) {
    for output_y in 0..height {
        let Some(source_y) = offset_index(output_y, disk_row.dy, height) else {
            destination[output_y * width..(output_y + 1) * width].fill(0);
            continue;
        };
        let source_row = &source[source_y * width..(source_y + 1) * width];
        let destination_row = &mut destination[output_y * width..(output_y + 1) * width];
        let left_margin =
            usize::try_from((-disk_row.left).max(0)).expect("cleanup offset fits usize");
        let right_margin =
            usize::try_from(disk_row.right.max(0)).expect("cleanup offset fits usize");
        let first_center = left_margin;
        let past_last_center = width - right_margin;
        destination_row[..first_center].fill(0);
        destination_row[past_last_center..].fill(0);

        let window_width = usize::try_from(disk_row.right - disk_row.left + 1)
            .expect("positive cleanup row width fits usize");
        let first_source = usize::try_from(first_center as i64 + disk_row.left)
            .expect("valid erosion window starts inside the row");
        let mut foreground = source_row[first_source..first_source + window_width]
            .iter()
            .map(|value| usize::from(*value))
            .sum::<usize>();
        for (center_x, destination) in destination_row
            .iter_mut()
            .enumerate()
            .take(past_last_center)
            .skip(first_center)
        {
            if foreground != window_width {
                *destination = 0;
            }
            if center_x + 1 < past_last_center {
                let leaving = usize::try_from(center_x as i64 + disk_row.left)
                    .expect("valid erosion window has a leaving sample");
                let entering = usize::try_from(center_x as i64 + 1 + disk_row.right)
                    .expect("valid erosion window has an entering sample");
                foreground -= usize::from(source_row[leaving]);
                foreground += usize::from(source_row[entering]);
            }
        }
    }
}

fn dilate_with_row(
    source: &[u8],
    destination: &mut [u8],
    width: usize,
    height: usize,
    disk_row: DiskRow,
) {
    for output_y in 0..height {
        let Some(source_y) = offset_index(output_y, disk_row.dy, height) else {
            continue;
        };
        let source_row = &source[source_y * width..(source_y + 1) * width];
        let destination_row = &mut destination[output_y * width..(output_y + 1) * width];
        let mut foreground = (disk_row.left..=disk_row.right)
            .filter_map(|offset| usize::try_from(offset).ok())
            .filter(|index| *index < width)
            .map(|index| usize::from(source_row[index]))
            .sum::<usize>();

        for (center_x, destination) in destination_row.iter_mut().enumerate().take(width) {
            if foreground != 0 {
                *destination = 1;
            }
            let center = center_x as i64;
            let leaving = center + disk_row.left;
            if let Ok(index) = usize::try_from(leaving) {
                if index < width {
                    foreground -= usize::from(source_row[index]);
                }
            }
            let entering = center + 1 + disk_row.right;
            if let Ok(index) = usize::try_from(entering) {
                if index < width {
                    foreground += usize::from(source_row[index]);
                }
            }
        }
    }
}

fn offset_index(index: usize, offset: i64, limit: usize) -> Option<usize> {
    let shifted = i128::try_from(index)
        .ok()?
        .checked_add(i128::from(offset))?;
    let shifted = usize::try_from(shifted).ok()?;
    (shifted < limit).then_some(shifted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use plaque3mf_document::{BinaryMask, Dimension, PhysicalSizeMicrometers};

    fn mask(width: u32, height: u32, pixels: &[u8]) -> BinaryMask {
        BinaryMask::new(width, height, pixels.to_vec()).expect("test mask is valid")
    }

    fn physical(width: i64, height: i64) -> PhysicalSizeMicrometers {
        PhysicalSizeMicrometers::new(width, height).expect("test dimensions are positive")
    }

    fn open_foreground(
        mask: &BinaryMask,
        physical_size: PhysicalSizeMicrometers,
        minimum_width_micrometers: i64,
    ) -> Result<Vec<u8>, PlanarError> {
        let dimensions = mask.dimensions();
        let x_edges =
            crate::axis_edges(physical_size.width(), dimensions.width(), Dimension::Width)?;
        let y_edges = crate::axis_edges(
            physical_size.height(),
            dimensions.height(),
            Dimension::Height,
        )?;
        open_foreground_on_grid(mask, &x_edges, &y_edges, minimum_width_micrometers)
    }

    #[test]
    fn one_cell_minimum_is_the_identity() {
        let input = mask(3, 2, &[1, 0, 1, 0, 1, 0]);
        let opened =
            open_foreground(&input, physical(300, 200), 100).expect("identity opening succeeds");

        assert_eq!(opened, input.as_bytes());
    }

    #[test]
    fn opening_rejects_a_nonpositive_width() {
        let input = mask(1, 1, &[1]);
        assert_eq!(
            open_foreground(&input, physical(100, 100), 0),
            Err(PlanarError::NonPositiveMinimumFeatureWidth)
        );
    }

    #[test]
    fn digital_disk_removes_below_minimum_and_retains_exact_width() {
        let narrow = mask(
            4,
            4,
            &[
                0, 1, 0, 0, //
                0, 1, 0, 0, //
                0, 1, 0, 0, //
                0, 1, 0, 0,
            ],
        );
        let exact = mask(
            4,
            4,
            &[
                0, 1, 1, 0, //
                0, 1, 1, 0, //
                0, 1, 1, 0, //
                0, 1, 1, 0,
            ],
        );

        assert_eq!(
            open_foreground(&narrow, physical(400, 400), 200).expect("opening succeeds"),
            vec![0; 16]
        );
        assert_eq!(
            open_foreground(&exact, physical(400, 400), 200).expect("opening succeeds"),
            exact.as_bytes()
        );
    }

    #[test]
    fn nondivisible_anisotropic_axes_use_actual_breakpoint_spans() {
        let narrow = mask(2, 1, &[0, 1]);
        let full = mask(2, 1, &[1, 1]);

        assert_eq!(
            open_foreground(&narrow, physical(5, 4), 3).expect("conservative opening succeeds"),
            [0, 0]
        );
        assert_eq!(
            open_foreground(&full, physical(5, 4), 3).expect("full-width opening succeeds"),
            full.as_bytes()
        );
    }

    #[test]
    fn nondivisible_grid_preserves_a_physically_wide_full_page() {
        let pixels = vec![1; 100];
        let input = mask(100, 1, &pixels);

        assert_eq!(
            open_foreground(&input, physical(199, 200), 150)
                .expect("physical breakpoint opening succeeds"),
            input.as_bytes()
        );
    }

    #[test]
    fn extreme_anisotropic_disk_has_defined_tips() {
        let rows = disk_rows(2, 100).expect("narrow digital ellipse is representable");
        assert_eq!(rows.len(), 100);
        assert_eq!(
            rows.first(),
            Some(&DiskRow {
                dy: -49,
                left: 0,
                right: 1,
            })
        );
        assert_eq!(
            rows.last(),
            Some(&DiskRow {
                dy: 50,
                left: 0,
                right: 1,
            })
        );

        let pixels = vec![1; 200];
        let input = mask(2, 100, &pixels);
        assert_eq!(
            open_foreground(&input, physical(100, 100), 100).expect("anisotropic opening succeeds"),
            input.as_bytes()
        );
    }

    #[test]
    fn reflected_even_kernel_does_not_translate_a_full_page() {
        let input = mask(3, 3, &[1; 9]);
        let opened = open_foreground(&input, physical(300, 300), 200).expect("opening succeeds");

        assert_eq!(opened, input.as_bytes());
    }

    #[test]
    fn digital_ellipse_rows_are_deterministic_for_even_axes() {
        assert_eq!(
            disk_rows(4, 2).expect("disk is representable"),
            [
                DiskRow {
                    dy: 0,
                    left: -1,
                    right: 2,
                },
                DiskRow {
                    dy: 1,
                    left: -1,
                    right: 2,
                },
            ]
        );
    }

    #[test]
    fn opening_is_idempotent_and_anti_extensive_for_every_three_by_three_mask() {
        for bits in 0_u16..512 {
            let pixels = (0..9)
                .map(|index| u8::from(bits & (1 << index) != 0))
                .collect::<Vec<_>>();
            let input = mask(3, 3, &pixels);
            let opened =
                open_foreground(&input, physical(300, 300), 200).expect("first opening succeeds");
            let opened_mask = mask(3, 3, &opened);
            let repeated = open_foreground(&opened_mask, physical(300, 300), 200)
                .expect("second opening succeeds");

            assert_eq!(
                opened, repeated,
                "opening is not idempotent for {bits:#05x}"
            );
            assert!(
                opened
                    .iter()
                    .zip(&pixels)
                    .all(|(result, source)| result <= source),
                "opening added foreground for {bits:#05x}"
            );
        }
    }

    #[test]
    fn opening_work_is_bounded_before_processing() {
        assert!(matches!(
            ensure_work_within_limit(10, 3, 59),
            Err(PlanarError::MorphologyWorkLimitExceeded {
                required: 60,
                max: 59,
            })
        ));
    }

    #[test]
    fn opening_work_overflow_is_reported() {
        assert_eq!(
            ensure_work_within_limit(u128::MAX, 2, u128::MAX),
            Err(PlanarError::ArithmeticOverflow)
        );
    }
}
