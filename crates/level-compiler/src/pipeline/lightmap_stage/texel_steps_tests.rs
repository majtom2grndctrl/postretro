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

#[test]
fn chart_rows_match_per_texel_search_upright_and_transposed() {
    let texels = sparse_partition(40);
    let (width, height) = (9, 5);
    for transposed in [false, true] {
        for (x, y) in [(0, 0), (6, 4), (30, 20), (ATLAS_WIDTH - 13, 26)] {
            let placement = ChartPlacement {
                x,
                y,
                layer: 0,
                transposed,
            };
            let expected = searched_rows(&texels, placement, width, height);
            let streamed = streamed_rows(&texels, placement, width, height)
                .expect("a sparse partition has records inside every chart");
            assert_eq!(
                streamed, expected,
                "placement ({x}, {y}), transposed {transposed}"
            );
            assert!(expected.iter().flatten().any(Option::is_none));
            assert!(expected.iter().flatten().any(Option::is_some));
        }
    }
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
