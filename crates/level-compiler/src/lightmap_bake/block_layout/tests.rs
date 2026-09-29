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
fn cell_block_pack_rejects_a_block_past_the_pool_layer_edge_and_accepts_one_at_it() {
    let at_edge = [chart(LIGHTMAP_POOL_LAYER_EDGE, 12, 0), chart(9, 9, 1)];
    let pack = pack(&at_edge, BlockOrdering::by_cell_id(2));
    assert_eq!(pack.layout.blocks[0].width, LIGHTMAP_POOL_LAYER_EDGE);

    let past_edge = [chart(9, 9, 0), chart(12, LIGHTMAP_POOL_LAYER_EDGE + 1, 5)];
    let error = pack_cell_blocks(
        &past_edge,
        BlockOrdering::by_cell_id(2),
        &BakeControl::unrestricted(),
    )
    .expect_err("a cell block taller than a pool layer must fail the build");
    match error {
        LightmapBakeError::BlockTooLarge {
            cell_id,
            height,
            max,
            largest_chart_face,
            ..
        } => {
            assert_eq!(
                (cell_id, max, largest_chart_face),
                (5, LIGHTMAP_POOL_LAYER_EDGE, 1)
            );
            assert!(height > LIGHTMAP_POOL_LAYER_EDGE);
        }
        other => panic!("expected BlockTooLarge, got {other}"),
    }
}

#[test]
fn block_limits_reject_an_extent_past_the_pool_layer_on_either_axis() {
    let extent = |width, height| BlockExtent {
        cell_id: 4,
        width,
        height,
        largest_chart_face: 0,
    };
    let edge = LIGHTMAP_POOL_LAYER_EDGE;
    assert!(check_block_limits(1, [extent(edge, edge)]).is_ok());
    for past in [extent(edge + 4, 4), extent(4, edge + 4)] {
        let message = check_block_limits(1, [past])
            .expect_err("oversize block must fail")
            .to_string();
        assert!(message.contains("cell 4"), "{message}");
        assert!(message.contains(&format!("{edge}x{edge}")), "{message}");
    }
}

#[test]
fn block_count_limit_rejects_one_past_the_vertex_id_limit_and_accepts_the_limit() {
    // Synthetic counts drive the chokepoint directly: packing 65,535 cells is
    // not what this proves.
    assert!(check_block_limits(MAX_LIGHTMAP_BLOCKS as usize, []).is_ok());
    let error = check_block_limits(MAX_LIGHTMAP_BLOCKS as usize + 1, [])
        .expect_err("one block past the vertex id limit must fail the build");
    assert!(matches!(
        error,
        LightmapBakeError::BlockCountOverflow { count, max }
            if count == MAX_LIGHTMAP_BLOCKS as usize + 1 && max == MAX_LIGHTMAP_BLOCKS
    ));
    assert!(error.to_string().contains(&MAX_LIGHTMAP_BLOCKS.to_string()));
}
