use std::collections::BTreeSet;

use super::band_pool_sim::{BandPolicy, BandSimInputs, simulate_band};
use super::block_pool_sim::{
    Eviction, SimInputs, run_walks, shelf_layers_from_scratch, simulate_fixed,
};
use super::camera_walks::{
    STALL_TELEPORT_STEPS, WalkKind, camera_adjacency, component_count, walk_path,
};
use super::cell_block_residency::static_maxrects_layers;
use super::cell_blocks::{BlockDims, CellBlocks};
use super::dry_run_test_fixtures::{bc6h_formats, chart, input, multi_block_input};
use super::mandatory::Granularity;
use super::visible_set_tests::u_turn_input;
use super::{ChartRect, ShadowmaskState, run_dry_run};
use crate::cell_residency_bake::portal_distance::{HubCell, HubPortal, PortalGraphInput};
use crate::lightmap_bake::{PackedBlock, pack_cell_block as pack_sizes};

/// The bake's packer over recovered chart rects.
fn pack_cell_block(charts: &[ChartRect], align: u32) -> Option<PackedBlock> {
    let sizes: Vec<(u32, u32)> = charts.iter().map(|c| (c.width, c.height)).collect();
    pack_sizes(&sizes, align)
}

#[test]
fn cell_block_holds_every_chart_of_its_cell_without_overlap() {
    let charts: Vec<_> = (0..23u32)
        .map(|i| chart(0, 0, 0, 0, 5 + (i * 13) % 41, 7 + (i * 29) % 37))
        .chain([chart(0, 0, 0, 0, 90, 9), chart(0, 0, 0, 0, 6, 70)])
        .collect();
    let block = pack_cell_block(&charts, 4).expect("charted cell packs");
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
    assert!(pack_cell_block(&[], 4).is_none());
}

#[test]
fn cell_block_ratio_is_at_least_one_and_a_single_aligned_chart_packs_exactly() {
    let exact = pack_cell_block(&[chart(0, 0, 0, 0, 64, 32)], 4).unwrap();
    assert_eq!((exact.width, exact.height), (64, 32));
    let unaligned = pack_cell_block(&[chart(0, 0, 0, 0, 61, 30)], 4).unwrap();
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
    assert!(
        blocks.cell_dims(2).is_empty(),
        "chartless cell has no block"
    );
    assert_eq!(blocks.set_bytes(&[2]), 0);
    assert_eq!(blocks.cells_spanning_stored_blocks, 0);
    let overhead = blocks.overhead();
    assert_eq!(overhead.blocks, 3);
    assert!(overhead.block_texels >= overhead.chart_texels);
    assert!(overhead.ratios.iter().all(|&(ratio, _)| ratio >= 1.0));
    // Block bytes never undercut the cell's texel-exact bytes.
    let rate = fixture.formats.bytes_per_texel();
    for cell in 0..fixture.cell_count() {
        let bytes = blocks.set_bytes(&[cell as u32]);
        assert!(bytes as f64 >= blocks.chart_texels[cell] as f64 * rate);
    }

    let mut split = fixture;
    split.charts[1].layer = 1;
    assert_eq!(CellBlocks::new(&split).cells_spanning_stored_blocks, 1);
}

#[test]
fn cell_blocks_align_to_the_direction_scale_above_the_bc_block_edge() {
    // Direction at 1/8 of irradiance: a 4-aligned 36-texel edge would not map
    // to whole direction texels and `layer_bytes_at` would refuse it.
    let mut formats = bc6h_formats(2048, 1, true);
    formats.direction_texel_scale = 8;
    formats.blocks[0].direction_bytes = 256 * 256 * 2;
    assert_eq!(formats.block_alignment(), 8);
    assert_eq!(bc6h_formats(2048, 1, true).block_alignment(), 4);

    let fixture = input(
        formats,
        &[0, 1],
        vec![chart(0, 0, 0, 0, 33, 21), chart(1, 0, 40, 0, 61, 35)],
        Vec::new(),
    );
    let blocks = CellBlocks::new(&fixture);
    assert_eq!(blocks.alignment, 8);
    assert_eq!(
        blocks.cell_dims(0),
        [BlockDims {
            width: 40,
            height: 24
        }]
    );
    for dims in &blocks.dims {
        assert_eq!((dims.width % 8, dims.height % 8), (0, 0), "{dims:?}");
    }
    assert!(blocks.block_bytes.iter().all(|&bytes| bytes > 0));
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
        .map(|set| shelf_layers_from_scratch(&blocks, set))
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
    assert_eq!(first.camera_components, 1);
    assert_eq!(first.walks.len(), WalkKind::ALL.len());
    for walk in &first.walks {
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
    // The repack is the shelf from-scratch packing: a pool at the shelf worst
    // may defragment but never hard-fails.
    for eviction in [Eviction::Immediate, Eviction::Lru] {
        let at_worst = simulate_fixed(&sim(7), &path, worst, eviction);
        assert_eq!(at_worst.hard_fail_steps, 0, "{eviction:?}");
    }
}

#[test]
fn random_walk_leaves_a_disconnected_pair_once_it_stalls() {
    // Cells 0-1 are an isolated pair; 2-9 a chain. Seed 0 starts in the pair.
    let n = 10u32;
    let mut adjacency = vec![Vec::new(); n as usize];
    for (a, b) in [(0, 1)].into_iter().chain((2..n - 1).map(|c| (c, c + 1))) {
        adjacency[a as usize].push(b);
        adjacency[b as usize].push(a);
    }
    assert_eq!(component_count(&adjacency), 2);
    let seed = (0..)
        .find(|&seed| walk_path(WalkKind::Random, &adjacency, 1, seed).0[0] < 2)
        .unwrap();
    let steps = STALL_TELEPORT_STEPS * 3;
    let (path, teleports) = walk_path(WalkKind::Random, &adjacency, steps, seed);
    assert!(teleports >= 1, "stalled in the pair and never left");
    let left_at = path
        .iter()
        .position(|&c| c >= 2)
        .expect("reached the chain");
    assert!(left_at > STALL_TELEPORT_STEPS, "left only after stalling");
    assert_eq!(path.len(), steps);
    let again = walk_path(WalkKind::Random, &adjacency, steps, seed);
    assert_eq!(again, (path, teleports), "deterministic per seed");
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
    assert_eq!(residency.walks.walks.len(), WalkKind::ALL.len());
    for walk in &residency.walks.walks {
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
    assert!(rendered.contains("1 portal components"));
    assert_eq!(rendered, run_dry_run(&u_turn_input()).render());
}

#[test]
fn block_bytes_and_their_exact_floor_both_charge_an_omitted_for_width_shadowmask() {
    let mut fixture = u_turn_input();
    fixture.formats.shadowmask = ShadowmaskState::OmittedForWidth;
    let report = run_dry_run(&fixture);
    let visible = report.visible_set.as_ref().unwrap();
    let exact = &visible.dense[0];
    assert_eq!(exact.granularity, Granularity::Cell);
    for (lead, exact_lead) in visible.cell_blocks.leads.iter().zip(&exact.leads) {
        for (&bytes, (cell, exact)) in lead.bytes.iter().zip(&exact_lead.cells) {
            // Blocks this narrow carry id 42, so the matching floor must too.
            let floor = exact.texel_exact_charging_mask();
            assert!(floor > exact.texel_exact, "cell {cell}");
            assert!(bytes as f64 >= floor, "cell {cell}");
        }
    }
    assert!(
        report
            .render()
            .contains("exact columns include the omitted id 42")
    );
}

#[test]
fn cell_blocks_count_every_block_of_a_cell_past_one_pool_layer() {
    let fixture = multi_block_input();
    let formats = &fixture.formats;
    let blocks = CellBlocks::new(&fixture);
    let dims = |width, height| BlockDims { width, height };
    assert_eq!(blocks.blocks_of_cell(0), 0..2);
    assert_eq!(blocks.cell_dims(0), [dims(2000, 1500), dims(1500, 1500)]);
    assert_eq!(blocks.cell_dims(1), [dims(64, 64)]);
    assert!(blocks.dims.iter().all(|d| d.fits_pool_layer()));
    assert_eq!(blocks.cell_of_block, [0, 0, 1]);
    let cell0_bytes = formats.layer_bytes_at(2000, 1500) + formats.layer_bytes_at(1500, 1500);
    assert_eq!(blocks.set_bytes(&[0]), cell0_bytes);
    assert_eq!(blocks.set_dims(&[0, 1]).count(), 3);
    let overhead = blocks.overhead();
    assert_eq!(overhead.blocks, 3);
    assert_eq!(overhead.multi_block_cells, [0]);
    assert_eq!(overhead.largest, Some((0, dims(2000, 1500))));
    let rendered = run_dry_run(&fixture).render();
    assert!(
        rendered.contains(
            "cells packed into several blocks: 1
    cell 0@"
        ),
        "{rendered}"
    );
    assert!(rendered.contains(": 2 blocks (2000x1500, 1500x1500)"));

    // The shelf pool places both of cell 0's blocks: neither leaves room for
    // the other in one layer.
    assert_eq!(shelf_layers_from_scratch(&blocks, &[0]), 2);
    let graph = PortalGraphInput {
        cells: Vec::new(),
        portals: Vec::new(),
    };
    let sets = vec![vec![0]];
    let sim = SimInputs {
        blocks: &blocks,
        camera_cells: &[0],
        sets: &sets,
        static_layers: &[2],
        graph: &graph,
        static_worst_layers: 2,
        steps: 1,
        seed: 0,
    };
    let one_layer = simulate_fixed(&sim, &[0], 1, Eviction::Immediate);
    assert_eq!(
        (one_layer.hard_fail_steps, one_layer.hard_fail_blocks),
        (1, 1),
        "the second block of cell 0 finds no room"
    );
    let two_layers = simulate_fixed(&sim, &[0], 2, Eviction::Immediate);
    assert_eq!(
        (two_layers.defrag_steps, two_layers.hard_fail_steps),
        (0, 0)
    );

    // Cell 0 joining M(c) brings both of its blocks: each is an entering
    // block and a demand read.
    let mandatory = vec![vec![1], vec![0, 1]];
    let band = vec![Vec::new(), Vec::new()];
    let inputs = BandSimInputs {
        blocks: &blocks,
        mandatory: &mandatory,
        band: &band,
    };
    let run = simulate_band(&inputs, &[0, 1], None, BandPolicy::ImmediateFree);
    assert_eq!((run.entering_blocks, run.entering_resident), (2, 0));
    assert_eq!(run.demand_reads, 3);
    assert_eq!(run.read_bytes, blocks.set_bytes(&[0, 1]));
    assert_eq!(run.peak_layers, 2);
}
