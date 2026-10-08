// Cell-block layout tests: chart containment, alignment, pool-layer and block-count limits, order.
// See: context/lib/build_pipeline.md §PRL section IDs (Lightmap id 22)

use std::collections::BTreeSet;

use glam::Vec3;
use postretro_level_format::lightmap::{LIGHTMAP_POOL_LAYER_EDGE, MAX_LIGHTMAP_BLOCKS};

use super::*;

fn chart(width: u32, height: u32, cell: u32) -> Chart {
    Chart {
        origin: Vec3::ZERO,
        u_axis: Vec3::X,
        v_axis: Vec3::Z,
        uv_min: [0.0, 0.0],
        uv_extent: [1.0, 1.0],
        normal: Vec3::Y,
        width_texels: width,
        height_texels: height,
        leaf_index: cell,
        window: None,
    }
}

/// Five cells of mixed, unaligned chart extents, including a 1×1
/// degenerate-face placeholder, interleaved in face order.
fn mixed_cells() -> Vec<Chart> {
    let mut charts = Vec::new();
    for i in 0..40u32 {
        let cell = [3, 0, 7, 3, 1][i as usize % 5];
        charts.push(chart(5 + (i * 13) % 61, 5 + (i * 29) % 47, cell));
    }
    charts.push(chart(1, 1, 7));
    charts.push(chart(301, 17, 1));
    charts
}

fn pack(charts: &[Chart], ordering: BlockOrdering<'_>) -> BlockedPack {
    pack_cell_blocks(charts, ordering, &BakeControl::unrestricted()).expect("fixture cells pack")
}

#[test]
fn cell_block_pack_holds_every_chart_inside_its_cell_block_without_overlap() {
    let charts = mixed_cells();
    for scale in [2, 8] {
        let pack = pack(&charts, BlockOrdering::by_cell_id(scale));
        let layout = &pack.layout;
        let align = layout.alignment();
        assert_eq!(align, if scale == 8 { 8 } else { 4 });
        assert_eq!(layout.blocks.len(), 4, "one block per charted cell");

        let mut occupied = BTreeSet::new();
        for (index, (chart, placement)) in charts.iter().zip(&pack.placements).enumerate() {
            let block = &layout.blocks[layout.chart_blocks[index] as usize];
            assert_eq!(
                block.cell_id, chart.leaf_index,
                "chart {index} sits in its cell's block"
            );
            assert!(
                block.contains(
                    placement.layer,
                    placement.x,
                    placement.y,
                    chart.width_texels,
                    chart.height_texels
                ),
                "chart {index} {placement:?} leaves block {block:?}"
            );
            for y in placement.y..placement.y + chart.height_texels {
                for x in placement.x..placement.x + chart.width_texels {
                    assert!(
                        occupied.insert((placement.layer, x, y)),
                        "charts overlap at layer {} texel {x},{y}",
                        placement.layer
                    );
                }
            }
        }

        for (id, block) in layout.blocks.iter().enumerate() {
            assert_eq!(block.width % align, 0, "block {id} width");
            assert_eq!(block.height % align, 0, "block {id} height");
            assert_eq!(block.width % 4, 0, "block {id} BC edge");
            assert_eq!(block.width % scale, 0, "block {id} direction scale");
            assert_eq!(block.x % align, 0, "block {id} origin x");
            assert_eq!(block.y % align, 0, "block {id} origin y");
            assert!(block.width <= LIGHTMAP_POOL_LAYER_EDGE);
            assert!(block.height <= LIGHTMAP_POOL_LAYER_EDGE);
            assert!(block.x + block.width <= pack.layer_dim);
            assert!(block.y + block.height <= pack.layer_dim);
            for other in &layout.blocks[id + 1..] {
                let apart = block.layer != other.layer
                    || block.x + block.width <= other.x
                    || other.x + other.width <= block.x
                    || block.y + block.height <= other.y
                    || other.y + other.height <= block.y;
                assert!(
                    apart,
                    "blocks {block:?} and {other:?} overlap in their bake layer"
                );
            }
        }
    }
}

#[test]
fn cell_block_order_is_cluster_major_then_cell_id() {
    let charts = mixed_cells();
    // Cells 0, 1, 3, 7 in clusters 2, 0, 2, 0.
    let clusters = [2, 0, 9, 2, 9, 9, 9, 0];
    let clustered = pack(
        &charts,
        BlockOrdering {
            direction_texel_scale: 2,
            cell_clusters: &clusters,
        },
    );
    let order: Vec<u32> = clustered.layout.blocks.iter().map(|b| b.cell_id).collect();
    assert_eq!(order, [1, 7, 0, 3]);
    let by_cell = pack(&charts, BlockOrdering::by_cell_id(2));
    let order: Vec<u32> = by_cell.layout.blocks.iter().map(|b| b.cell_id).collect();
    assert_eq!(order, [0, 1, 3, 7]);
}

#[test]
fn cell_block_pack_accepts_a_block_at_the_pool_layer_edge() {
    let at_edge = [chart(LIGHTMAP_POOL_LAYER_EDGE, 12, 0), chart(9, 9, 1)];
    let pack = pack(&at_edge, BlockOrdering::by_cell_id(2));
    assert_eq!(pack.layout.blocks.len(), 2, "one block per fitting cell");
    assert_eq!(pack.layout.blocks[0].width, LIGHTMAP_POOL_LAYER_EDGE);
}

/// Charts of each block, in block-id order.
fn block_members(pack: &BlockedPack) -> Vec<Vec<usize>> {
    let mut members = vec![Vec::new(); pack.layout.blocks.len()];
    for (chart, &block) in pack.layout.chart_blocks.iter().enumerate() {
        members[block as usize].push(chart);
    }
    members
}

#[test]
fn oversized_cell_packs_into_several_blocks_each_within_the_pool_edge() {
    // Each of cell 5's charts fits a pool layer on its own; together they
    // cover more than a layer's area, so no single block can hold them.
    let side = LIGHTMAP_POOL_LAYER_EDGE * 3 / 4;
    let charts = [
        chart(9, 9, 0),
        chart(side, side, 5),
        chart(side, side, 5),
        chart(side, side, 5),
        chart(40, 24, 5),
    ];
    assert!(
        charts
            .iter()
            .all(|c| c.width_texels <= LIGHTMAP_POOL_LAYER_EDGE
                && c.height_texels <= LIGHTMAP_POOL_LAYER_EDGE),
        "every chart fits a pool layer on its own"
    );
    let pack = pack(&charts, BlockOrdering::by_cell_id(2));
    let layout = &pack.layout;
    let cells: Vec<u32> = layout.blocks.iter().map(|b| b.cell_id).collect();
    assert_eq!(cells, [0, 5, 5, 5], "cell 5 splits into three blocks");
    for block in &layout.blocks {
        assert!(block.width <= LIGHTMAP_POOL_LAYER_EDGE);
        assert!(block.height <= LIGHTMAP_POOL_LAYER_EDGE);
    }
    // Every chart lands in exactly one block, of its own cell, inside it.
    assert_eq!(layout.chart_blocks.len(), charts.len());
    for (index, (chart, placement)) in charts.iter().zip(&pack.placements).enumerate() {
        let block = &layout.blocks[layout.chart_blocks[index] as usize];
        assert_eq!(block.cell_id, chart.leaf_index);
        assert!(block.contains(
            placement.layer,
            placement.x,
            placement.y,
            chart.width_texels,
            chart.height_texels
        ));
    }
    let members = block_members(&pack);
    assert!(members.iter().all(|m| !m.is_empty()), "no empty block");
    assert_eq!(members.iter().map(Vec::len).sum::<usize>(), charts.len());
}

#[test]
fn fitting_cells_pack_one_block_each_exactly_as_the_single_block_packer() {
    let charts = mixed_cells();
    let pack = pack(&charts, BlockOrdering::by_cell_id(2));
    let align = pack.layout.alignment();
    for (block_id, members) in block_members(&pack).iter().enumerate() {
        let block = &pack.layout.blocks[block_id];
        let sizes: Vec<(u32, u32)> = members
            .iter()
            .map(|&i| (charts[i].width_texels, charts[i].height_texels))
            .collect();
        let single = super::super::cell_blocks::pack_cell_block(&sizes, align)
            .expect("a charted cell packs");
        assert_eq!((block.width, block.height), (single.width, single.height));
        let local: Vec<(u32, u32)> = members
            .iter()
            .map(|&i| pack.layout.local_placement(i, &pack.placements[i]))
            .collect();
        assert_eq!(local, single.placements, "block {block_id} placements");
    }
}

/// Whether a `w × h` rect fits anywhere in the free texels of a
/// `width × height` block holding `occupied` rects `(x, y, w, h)`. Brute
/// force over a summed-area table, independent of the packer it checks.
fn fits_free_space(
    width: u32,
    height: u32,
    occupied: &[(u32, u32, u32, u32)],
    w: u32,
    h: u32,
) -> bool {
    if w > width || h > height {
        return false;
    }
    let (bw, bh) = (width as usize, height as usize);
    let mut grid = vec![0u32; bw * bh];
    for &(x, y, rw, rh) in occupied {
        for yy in y..y + rh {
            for xx in x..x + rw {
                grid[yy as usize * bw + xx as usize] = 1;
            }
        }
    }
    let stride = bw + 1;
    let mut sat = vec![0u32; stride * (bh + 1)];
    for y in 0..bh {
        for x in 0..bw {
            sat[(y + 1) * stride + x + 1] =
                grid[y * bw + x] + sat[y * stride + x + 1] + sat[(y + 1) * stride + x]
                    - sat[y * stride + x];
        }
    }
    let (w, h) = (w as usize, h as usize);
    (0..=bh - h).any(|y| {
        (0..=bw - w).any(|x| {
            sat[(y + h) * stride + x + w] + sat[y * stride + x]
                == sat[y * stride + x + w] + sat[(y + h) * stride + x]
        })
    })
}

/// Deterministic pseudo-random chart extents in `[low, high]`.
fn scattered_charts(count: u32, low: u32, high: u32, cell: u32, seed: u32) -> Vec<Chart> {
    let span = high - low + 1;
    (0..count)
        .map(|i| {
            let a = (i.wrapping_mul(2_654_435_761) ^ seed).rotate_left(7);
            let b = (i.wrapping_mul(40_503) ^ seed.rotate_left(13)).wrapping_mul(2_246_822_519);
            chart(low + a % span, low + (b >> 7) % span, cell)
        })
        .collect()
}

#[test]
fn oversized_cell_blocks_are_trimmed_and_no_later_chart_fits_an_earlier_block() {
    let pool_edge = 256;
    let mut charts = scattered_charts(90, 9, 120, 2, 17);
    charts.extend(scattered_charts(20, 4, 60, 1, 5));
    let pack = pack_cell_blocks_within(
        &charts,
        BlockOrdering::by_cell_id(2),
        pool_edge,
        &BakeControl::unrestricted(),
    )
    .expect("fixture cells pack");
    let layout = &pack.layout;
    let align = layout.alignment();
    let members = block_members(&pack);
    let cell_two: Vec<usize> = (0..layout.blocks.len())
        .filter(|&b| layout.blocks[b].cell_id == 2)
        .collect();
    assert!(cell_two.len() >= 3, "cell 2 must split: {}", cell_two.len());

    for &block_id in &cell_two {
        let block = &layout.blocks[block_id];
        let rects: Vec<(u32, u32, u32, u32)> = members[block_id]
            .iter()
            .map(|&i| {
                let (x, y) = layout.local_placement(i, &pack.placements[i]);
                (x, y, charts[i].width_texels, charts[i].height_texels)
            })
            .collect();
        // Trimmed: the block is the aligned bounding box of its charts.
        let right = rects.iter().map(|r| r.0 + r.2).max().unwrap();
        let bottom = rects.iter().map(|r| r.1 + r.3).max().unwrap();
        assert!(block.width <= pool_edge && block.height <= pool_edge);
        assert_eq!(
            block.width,
            right.div_ceil(align) * align,
            "block {block_id} width"
        );
        assert_eq!(
            block.height,
            bottom.div_ceil(align) * align,
            "block {block_id} height"
        );
        // Fill rule: no chart of a later block of this cell fits here.
        for &later in cell_two.iter().filter(|&&b| b > block_id) {
            for &i in &members[later] {
                assert!(
                    !fits_free_space(
                        block.width,
                        block.height,
                        &rects,
                        charts[i].width_texels,
                        charts[i].height_texels
                    ),
                    "chart {i} in block {later} fits block {block_id}'s free space"
                );
            }
        }
    }
}

#[test]
fn quarter_edge_oversized_cell_occupies_at_most_one_layer_past_its_texel_area() {
    let edge = LIGHTMAP_POOL_LAYER_EDGE;
    let charts = scattered_charts(150, 40, edge / 4, 0, 99);
    let area: u64 = charts
        .iter()
        .map(|c| u64::from(c.width_texels) * u64::from(c.height_texels))
        .sum();
    let layer_area = u64::from(edge) * u64::from(edge);
    assert!(
        area > layer_area && area <= 4 * layer_area,
        "fixture area {area}"
    );
    let pack = pack(&charts, BlockOrdering::by_cell_id(2));
    assert!(pack.layout.blocks.len() >= 2);
    assert_eq!(pack.layer_dim, edge);
    let layers: BTreeSet<u32> = pack.layout.blocks.iter().map(|b| b.layer).collect();
    let needed = area.div_ceil(layer_area);
    assert!(
        layers.len() as u64 <= needed + 1,
        "{} layers for {needed} layers of texel area",
        layers.len()
    );
}

#[test]
fn multi_block_cells_are_contiguous_in_cluster_cell_sub_block_order() {
    let pool_edge = 128;
    let mut charts = scattered_charts(30, 20, 90, 4, 3);
    charts.extend(scattered_charts(30, 20, 90, 1, 8));
    charts.extend(scattered_charts(3, 5, 20, 2, 1));
    // Cells 1, 2, 4 in clusters 1, 0, 0.
    let clusters = [9, 1, 0, 9, 0];
    let pack = pack_cell_blocks_within(
        &charts,
        BlockOrdering {
            direction_texel_scale: 2,
            cell_clusters: &clusters,
        },
        pool_edge,
        &BakeControl::unrestricted(),
    )
    .expect("fixture cells pack");
    let order: Vec<u32> = pack.layout.blocks.iter().map(|b| b.cell_id).collect();
    let mut runs = order.clone();
    runs.dedup();
    assert_eq!(
        runs,
        [2, 4, 1],
        "cluster-major, then cell, each cell contiguous"
    );
    assert!(order.iter().filter(|&&c| c == 4).count() >= 2);
    assert!(order.iter().filter(|&&c| c == 1).count() >= 2);
}

#[test]
fn multi_block_pack_is_identical_with_one_worker_and_many() {
    use std::sync::Arc;

    use crate::governor::Governor;
    use crate::reporter::StageProgress;

    let mut charts = scattered_charts(80, 9, 100, 3, 21);
    charts.extend(scattered_charts(80, 9, 100, 0, 22));
    // Fitting cells too; every chart must fit the 256 test pool edge.
    charts.extend(mixed_cells().into_iter().filter(|c| c.width_texels <= 256));
    let run = |workers: usize| {
        let progress = StageProgress::indeterminate();
        let control = BakeControl::new(Arc::new(Governor::new(workers, false)), &progress);
        rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .expect("worker pool")
            .install(|| {
                pack_cell_blocks_within(&charts, BlockOrdering::by_cell_id(2), 256, &control)
                    .expect("fixture cells pack")
            })
    };
    let one = run(1);
    let many = run(4);
    assert!(one.layout.blocks.len() > 6, "fixture must split cells");
    assert_eq!(one.layout, many.layout);
    assert_eq!(one.placements, many.placements);
    assert_eq!(
        (one.layer_dim, one.layer_count),
        (many.layer_dim, many.layer_count)
    );
}

#[test]
fn block_count_limit_rejects_one_past_the_vertex_id_limit_and_accepts_the_limit() {
    // P11: synthetic counts drive the chokepoint directly, which
    // `pack_cell_blocks_within` feeds every block of every multi-block cell.
    assert!(check_block_limits(MAX_LIGHTMAP_BLOCKS as usize).is_ok());
    let error = check_block_limits(MAX_LIGHTMAP_BLOCKS as usize + 1)
        .expect_err("one block past the vertex id limit must fail the build");
    assert!(matches!(
        error,
        LightmapBakeError::BlockCountOverflow { count, max }
            if count == MAX_LIGHTMAP_BLOCKS as usize + 1 && max == MAX_LIGHTMAP_BLOCKS
    ));
    assert!(error.to_string().contains(&MAX_LIGHTMAP_BLOCKS.to_string()));
}
