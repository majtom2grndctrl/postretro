use std::collections::BTreeSet;

use super::block_allocator::{BlockPool, ShelfLayer};
use super::block_pool_sim::{Eviction, SimInputs, run_walks, simulate_fixed};
use super::camera_walks::{WalkKind, camera_adjacency, walk_path};
use super::cell_block_residency::static_maxrects_layers;
use super::cell_blocks::{BlockDims, CellBlocks, POOL_LAYER_EDGE, pack_cell_block};
use super::dry_run_test_fixtures::{bc6h_formats, chart, input};
use super::mandatory::Granularity;
use super::portal_distance::{HubCell, HubPortal, PortalGraphInput};
use super::run_dry_run;
use super::visible_set_tests::u_turn_input;

#[test]
fn cell_block_holds_every_chart_of_its_cell_without_overlap() {
    let charts: Vec<_> = (0..23u32)
        .map(|i| chart(0, 0, 0, 0, 5 + (i * 13) % 41, 7 + (i * 29) % 37))
        .chain([chart(0, 0, 0, 0, 90, 9), chart(0, 0, 0, 0, 6, 70)])
        .collect();
    let block = pack_cell_block(&charts).expect("charted cell packs");
    assert_eq!(block.width % 4, 0, "BC-aligned width");
    assert_eq!(block.height % 4, 0, "BC-aligned height");
    assert_eq!(block.placements.len(), charts.len());
    let mut occupied = BTreeSet::new();
    for (rect, &(x, y)) in charts.iter().zip(&block.placements) {
        assert!(x + rect.width <= block.width && y + rect.height <= block.height);
        for ty in y..y + rect.height {
            for tx in x..x + rect.width {
                assert!(occupied.insert((tx, ty)), "overlap at {tx},{ty}");
            }
        }
    }
    let chart_area: u64 = charts.iter().map(|c| c.area()).sum();
    assert_eq!(occupied.len() as u64, chart_area);
    assert!(block.area() >= chart_area);
    assert!(pack_cell_block(&[]).is_none());
}

#[test]
fn cell_block_ratio_is_at_least_one_and_a_single_aligned_chart_packs_exactly() {
    let exact = pack_cell_block(&[chart(0, 0, 0, 0, 64, 32)]).unwrap();
    assert_eq!((exact.width, exact.height), (64, 32));
    let unaligned = pack_cell_block(&[chart(0, 0, 0, 0, 61, 30)]).unwrap();
    assert_eq!((unaligned.width, unaligned.height), (64, 32));

    let fixture = input(
        bc6h_formats(256, 2, true),
        &[0, 0, 1, 2],
        vec![
            chart(0, 0, 0, 0, 30, 18),
            chart(0, 0, 30, 0, 9, 9),
            chart(1, 1, 39, 0, 25, 14),
            chart(3, 0, 0, 18, 50, 21),
            chart(3, 0, 50, 18, 5, 40),
        ],
        Vec::new(),
    );
    let blocks = CellBlocks::new(&fixture);
    assert_eq!(blocks.dims[2], None, "chartless cell has no block");
    assert_eq!(blocks.block_bytes[2], 0);
    assert_eq!(blocks.multi_layer_cells, 0);
    let overhead = blocks.overhead();
    assert_eq!(overhead.blocks, 3);
    assert!(overhead.block_texels >= overhead.chart_texels);
    assert!(overhead.ratios.iter().all(|&(ratio, _)| ratio >= 1.0));
    // Block bytes never undercut the cell's texel-exact bytes.
    let rate = fixture.formats.bytes_per_texel();
    for cell in 0..fixture.cell_count() {
        assert!(blocks.block_bytes[cell] as f64 >= blocks.chart_texels[cell] as f64 * rate);
    }

    let mut split = fixture;
    split.charts[1].layer = 1;
    assert_eq!(CellBlocks::new(&split).multi_layer_cells, 1);
}

#[test]
fn freeing_a_block_makes_its_space_reusable() {
    let mut pool = BlockPool::new(POOL_LAYER_EDGE, Some(1));
    let whole = pool
        .allocate(2048, 2048)
        .expect("empty layer holds a full block");
    assert!(pool.allocate(4, 4).is_none(), "the one layer is full");
    pool.free(whole);
    assert_eq!(pool.extent(), 0);
    assert!(pool.allocate(2048, 2048).is_some());

    // Two half-height shelves merge back into one full-height shelf.
    let mut layer = ShelfLayer::new(2048, 2048);
    let top = layer.allocate(2048, 1024).unwrap();
    let bottom = layer.allocate(2048, 1024).unwrap();
    assert!(layer.allocate(4, 4).is_none());
    layer.free(top.0, top.1);
    layer.free(bottom.0, bottom.1);
    assert!(layer.is_empty());
    assert_eq!(layer.allocate(2048, 2048), Some((0, 0)));

    // Freed spans merge within a shelf whichever order they free in.
    let mut layer = ShelfLayer::new(2048, 2048);
    let spans: Vec<_> = (0..3).map(|_| layer.allocate(680, 2048).unwrap()).collect();
    assert!(layer.allocate(700, 4).is_none());
    for &i in &[1, 0, 2] {
        layer.free(spans[i].0, spans[i].1);
    }
    assert_eq!(layer.allocate(2048, 2048), Some((0, 0)));
}

/// Twelve camera cells in a portal ring, one tall chart each; M(c) is the
/// cell and its two neighbours either side.
fn ring_sim_fixture() -> (
    CellBlocks,
    Vec<u32>,
    Vec<Vec<u32>>,
    Vec<u32>,
    PortalGraphInput,
) {
    let n = 12u32;
    let charts = (0..n)
        .map(|cell| chart(cell, 0, 0, 0, 600 + 40 * (cell % 4), 700 + 150 * (cell % 5)))
        .collect();
    let fixture = input(
        bc6h_formats(2048, 1, true),
        &(0..n).collect::<Vec<_>>(),
        charts,
        Vec::new(),
    );
    let blocks = CellBlocks::new(&fixture);
    let camera: Vec<u32> = (0..n).collect();
    let sets: Vec<Vec<u32>> = (0..n)
        .map(|c| {
            let mut set: Vec<u32> = (0..5).map(|k| (c + n + k - 2) % n).collect();
            set.sort_unstable();
            set
        })
        .collect();
    let static_layers = sets
        .iter()
        .map(|set| static_maxrects_layers(blocks.set_dims(set).map(|(_, d)| d)).0)
        .collect();
    let cube = HubCell {
        bounds_min: [0.0; 3],
        bounds_max: [1.0; 3],
        solid: false,
    };
    let graph = PortalGraphInput {
        cells: vec![cube; n as usize],
        portals: (0..n)
            .map(|c| HubPortal {
                front: c,
                back: (c + 1) % n,
                vertices: Vec::new(),
            })
            .collect(),
    };
    (blocks, camera, sets, static_layers, graph)
}

#[test]
fn pool_simulation_is_deterministic_for_a_seed() {
    let (blocks, camera, sets, static_layers, graph) = ring_sim_fixture();
    let worst = *static_layers.iter().max().unwrap();
    let sim = |seed| SimInputs {
        blocks: &blocks,
        camera_cells: &camera,
        sets: &sets,
        static_layers: &static_layers,
        graph: &graph,
        static_worst_layers: worst,
        steps: 400,
        seed,
    };
    let first = run_walks(&sim(7));
    assert_eq!(first, run_walks(&sim(7)));
    assert_eq!(first.len(), WalkKind::ALL.len());
    for walk in &first {
        assert_eq!(walk.steps, 400);
        assert_eq!(walk.teleports, 0, "the ring is connected");
        assert_eq!(walk.distinct_cells, 12, "{:?}", walk.kind);
        // The pool never needs fewer layers than the cells it visits pack into.
        assert!(walk.unbounded.peak_layers as u32 >= walk.unbounded.walk_static_peak);
        assert_eq!(walk.fixed.len(), 4);
    }
    let adjacency = camera_adjacency(&graph, &camera);
    let (a, _) = walk_path(WalkKind::Random, &adjacency, 400, 7);
    let (b, _) = walk_path(WalkKind::Random, &adjacency, 400, 8);
    assert_ne!(a, b, "the seed drives the walk");
    for path in [a, walk_path(WalkKind::FarPointTour, &adjacency, 400, 7).0] {
        for step in path.windows(2) {
            assert!(adjacency[step[0] as usize].contains(&step[1]), "{step:?}");
        }
    }

    // A one-layer pool cannot hold a two-layer set: every step fails, even
    // after the repack.
    let (path, _) = walk_path(WalkKind::Random, &adjacency, 50, 7);
    let tight = simulate_fixed(&sim(7), &path, 1, Eviction::Lru);
    if worst > 1 {
        assert!(tight.hard_fail_steps > 0);
        assert!(tight.defrag_steps >= tight.hard_fail_steps);
    }
    let roomy = simulate_fixed(&sim(7), &path, 12, Eviction::Immediate);
    assert_eq!((roomy.defrag_steps, roomy.hard_fail_steps), (0, 0));
}

#[test]
fn static_pool_fit_counts_layers_and_leaves_out_oversize_blocks() {
    let dims = |width, height| BlockDims { width, height };
    assert_eq!(static_maxrects_layers(std::iter::empty()), (0, 0));
    let quarters = [dims(1024, 1024); 4];
    assert_eq!(static_maxrects_layers(quarters.into_iter()), (1, 0));
    let five = [dims(1024, 1024); 5];
    assert_eq!(static_maxrects_layers(five.into_iter()), (2, 0));
    let oversize = [dims(2052, 16), dims(8, 8)];
    assert_eq!(static_maxrects_layers(oversize.into_iter()), (1, 1));
}

#[test]
fn cell_block_residency_covers_every_lead_and_never_undercuts_texel_exact() {
    let report = run_dry_run(&u_turn_input());
    let visible = report
        .visible_set
        .as_ref()
        .expect("world and graph present");
    let exact = visible
        .dense
        .iter()
        .find(|r| r.granularity == Granularity::Cell)
        .unwrap();
    let residency = &visible.cell_blocks;
    assert_eq!(residency.leads.len(), exact.leads.len());
    for (lead, exact_lead) in residency.leads.iter().zip(&exact.leads) {
        assert_eq!(lead.lead_meters, exact_lead.lead_meters);
        for (&bytes, (cell, exact)) in lead.bytes.iter().zip(&exact_lead.cells) {
            assert!(bytes as f64 >= exact.texel_exact, "cell {cell}");
        }
        // Three small blocks share one 2048² layer.
        assert!(lead.static_layers.iter().all(|&l| l == 1));
    }
    assert_eq!(residency.walks.len(), WalkKind::ALL.len());
    for walk in &residency.walks {
        assert_eq!(walk.unbounded.peak_layers, 1);
        assert!(
            walk.fixed
                .iter()
                .all(|run| run.defrag_steps == 0 && run.hard_fail_steps == 0)
        );
    }
    let rendered = report.render();
    assert!(rendered.contains("-- cell blocks:"));
    assert!(rendered.contains("cell-block mandatory bytes and static pool"));
    assert!(rendered.contains("cell-block pool walks"));
    assert_eq!(rendered, run_dry_run(&u_turn_input()).render());
}
