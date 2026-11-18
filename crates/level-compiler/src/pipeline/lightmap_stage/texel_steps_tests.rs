use super::*;

const ATLAS_WIDTH: u32 = 48;

/// Per-texel lookup by binary search over the whole partition: the reference
/// [`ChartRows`] must match on every row.
fn searched_rows(
    texels: &[LayerTexel],
    placement: ChartPlacement,
    width: u32,
    height: u32,
) -> Vec<Vec<Option<f32>>> {
    (0..height)
        .map(|ty| {
            (0..width)
                .map(|tx| {
                    let (x, y) = placement.interior_texel(tx as i32, ty as i32);
                    let idx = y as u32 * ATLAS_WIDTH + x as u32;
                    texels
                        .binary_search_by_key(&idx, |texel| texel.idx)
                        .ok()
                        .map(|at| texels[at].raw_visibility)
                })
                .collect()
        })
        .collect()
}

fn streamed_rows(
    texels: &[LayerTexel],
    placement: ChartPlacement,
    width: u32,
    height: u32,
) -> Option<Vec<Vec<Option<f32>>>> {
    let mut rows = ChartRows::new(texels, placement, ATLAS_WIDTH, width, height)?;
    Some(
        (0..height)
            .map(|ty| {
                let mut row = vec![None; width as usize];
                rows.fill(ty, &mut row);
                row
            })
            .collect(),
    )
}

/// A sparse partition over the whole atlas: roughly two texels in three hold
/// a record, each with a value unique to its index, so neighbouring charts on
/// the same atlas rows contribute records a cursor must step past.
fn sparse_partition(atlas_height: u32) -> Vec<LayerTexel> {
    (0..ATLAS_WIDTH * atlas_height)
        .filter(|idx| (idx.wrapping_mul(2_654_435_761) >> 7) % 3 != 0)
        .map(|idx| LayerTexel {
            idx,
            raw_visibility: idx as f32 * 0.25,
        })
        .collect()
}

/// The padded atlas extent of a `width × height` interior.
fn placed_extent(placement: ChartPlacement, width: u32, height: u32) -> (u32, u32) {
    let padding = 2 * crate::chart_raster::CHART_PADDING_TEXELS;
    placement.placed_extent(width + padding, height + padding)
}

#[test]
fn chart_rows_match_per_texel_search_upright_and_transposed() {
    let atlas_height = 40;
    let texels = sparse_partition(atlas_height);
    for (width, height) in [(9, 5), (1, 1), (1, 7), (7, 1), (5, 9)] {
        for transposed in [false, true] {
            let oriented = ChartPlacement {
                x: 0,
                y: 0,
                layer: 0,
                transposed,
            };
            let (placed_w, placed_h) = placed_extent(oriented, width, height);
            // Corners, an interior spot, and the atlas's right and bottom
            // edges.
            for (x, y) in [
                (0, 0),
                (6, 4),
                (30, 20),
                (ATLAS_WIDTH - placed_w, 26),
                (ATLAS_WIDTH - placed_w, atlas_height - placed_h),
                (0, atlas_height - placed_h),
            ] {
                let placement = ChartPlacement { x, y, ..oriented };
                let label = format!("{width}x{height} at ({x}, {y}), transposed {transposed}");
                let expected = searched_rows(&texels, placement, width, height);
                match streamed_rows(&texels, placement, width, height) {
                    Some(streamed) => assert_eq!(streamed, expected, "{label}"),
                    None => assert!(
                        expected.iter().flatten().all(Option::is_none),
                        "{label}: skipped a chart with records"
                    ),
                }
            }
        }
    }
}

// A transposed chart's index range spans whole atlas rows, so it can hold
// records that all belong to a neighbour sharing those rows. Every row then
// reads empty.
#[test]
fn a_transposed_chart_spanning_only_a_neighbours_records_reads_empty() {
    let placement = ChartPlacement {
        x: 10,
        y: 6,
        layer: 0,
        transposed: true,
    };
    let (width, height) = (4, 7);
    let (placed_w, placed_h) = placed_extent(placement, width, height);
    // Records on every atlas row the chart covers, left and right of it.
    let texels: Vec<LayerTexel> = (placement.y..placement.y + placed_h)
        .flat_map(|y| {
            [
                y * ATLAS_WIDTH + 2,
                y * ATLAS_WIDTH + placement.x + placed_w + 3,
            ]
        })
        .map(|idx| LayerTexel {
            idx,
            raw_visibility: idx as f32,
        })
        .collect();
    let expected = searched_rows(&texels, placement, width, height);
    assert!(expected.iter().flatten().all(Option::is_none));
    let streamed = streamed_rows(&texels, placement, width, height)
        .expect("records inside the chart's index range");
    assert_eq!(streamed, expected);
}

#[test]
fn chart_rows_skip_a_chart_with_no_record_in_either_orientation() {
    // Records only outside the chart's placed rect, before and after it.
    let texels: Vec<LayerTexel> = [3, 4, 30 * ATLAS_WIDTH + 2]
        .into_iter()
        .map(|idx| LayerTexel {
            idx,
            raw_visibility: 1.0,
        })
        .collect();
    for transposed in [false, true] {
        let placement = ChartPlacement {
            x: 10,
            y: 6,
            layer: 0,
            transposed,
        };
        assert!(streamed_rows(&texels, placement, 7, 3).is_none());
    }
}
