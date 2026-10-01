use postretro_level_format::cluster_directory::{CLUSTER_HINT_FLAG_PINNED, ClusterHintRecord};

use super::band_pool_sim::{BandPolicy, BandRun, BandSimInputs, simulate_band};
use super::brief_set::{
    BriefSetSources, Dilation, build_lead_map, check_against_direct, check_every_breakpoint,
    direct_set, pinned_cells, visible_sources,
};
use super::cell_blocks::{BlockDims, CellBlocks};
use super::dry_run_test_fixtures::{METER, bc6h_formats, chart, input, pair};
use super::inputs::pinned_clusters;
use super::mandatory::{Granularity, MandatoryContext};
use super::run_dry_run;
use super::visible_set_tests::u_turn_input;
use crate::cell_residency_bake::lead_map::{LeadMap, meters_fixed, portal_neighbours};
use crate::cell_residency_bake::portal_distance::{
    HubCell, HubPortal, Neighbors, PortalGraphInput,
};

/// `n` unit cubes chained along X by portals `k ↔ k + 1`.
fn chain_graph(n: u32) -> PortalGraphInput {
    let cube = |x: f32| HubCell {
        bounds_min: [x, 0.0, 0.0],
        bounds_max: [x + 1.0, 1.0, 1.0],
        solid: false,
    };
    PortalGraphInput {
        cells: (0..n).map(|k| cube(k as f32)).collect(),
        portals: (0..n.saturating_sub(1))
            .map(|k| HubPortal {
                front: k,
                back: k + 1,
                vertices: Vec::new(),
            })
            .collect(),
    }
}

fn lead_of(map: &LeadMap, camera: u32, cell: u32) -> Option<u32> {
    map.entries_of(camera)
        .iter()
        .find(|&&(entry, _)| entry == cell)
        .map(|&(_, lead)| lead)
}

fn sources<'a>(
    neighbors: &'a Neighbors,
    visible: &'a [Vec<u32>],
    pinned: &'a [u32],
) -> BriefSetSources<'a> {
    BriefSetSources {
        neighbors,
        visible,
        pinned,
        cell_count: visible.len(),
    }
}

#[test]
fn dilation_adds_exactly_the_one_hop_portal_neighbours() {
    // Chain 0-1-2-3-4-5. Cell 0 sees {0, 1}: one hop adds 2, never 3.
    let graph = chain_graph(6);
    let neighbours = portal_neighbours(&graph);
    assert_eq!(neighbours[2], vec![1, 3]);
    let mut pvs = vec![Vec::new(); 6];
    pvs[0] = vec![0, 1];
    pvs[4] = vec![4];
    let dilated = visible_sources(&pvs, &neighbours, Dilation::OneHop);
    assert_eq!(dilated[0], vec![0, 1, 2]);
    assert_eq!(dilated[4], vec![3, 4, 5]);
    assert!(dilated[1].is_empty(), "Dil of an empty PVS stays empty");
    assert_eq!(visible_sources(&pvs, &neighbours, Dilation::None), pvs);

    let no_pairs = Neighbors::from_pairs(6, &[]);
    for (visible, expected) in [(&dilated, vec![0, 1, 2]), (&pvs, vec![0, 1])] {
        let sources = sources(&no_pairs, visible, &[]);
        let map = build_lead_map(&sources, &[0], meters_fixed(32));
        assert_eq!(map.mandatory(0, 0, &[]), expected);
    }
}

#[test]
fn brief_set_has_no_camera_cluster_term() {
    // Cells 0 and 3 share cluster 0; cell 3 is neither visible from 0 nor
    // within any lead of it.
    let fixture = input(
        bc6h_formats(64, 1, false),
        &[0, 1, 2, 0],
        Vec::new(),
        vec![pair(0, 1, 5 * METER)],
    );
    let neighbors = Neighbors::from_pairs(4, &fixture.coupled_pairs);
    let pvs = vec![vec![0, 1], vec![1, 2], vec![2], vec![3]];
    let pinned = pinned_cells(&fixture);
    let sources = sources(&neighbors, &pvs, &pinned);
    let map = build_lead_map(&sources, &[0, 1, 2, 3], meters_fixed(32));
    for lead in [0, 16, 32] {
        let brief = map.mandatory(0, meters_fixed(lead), &pinned);
        assert!(!brief.contains(&3), "L={lead}: {brief:?}");
    }
    assert_eq!(map.mandatory(0, meters_fixed(16), &pinned), vec![0, 1, 2]);
    // The older cell-granular set drags the cluster-mate in.
    let mut context = MandatoryContext::new(&fixture);
    assert!(
        context
            .set_from_reached(0, &[0, 1], Granularity::Cell)
            .contains(&3)
    );
}

#[test]
fn flagged_pin_joins_every_camera_cell_and_an_unflagged_hint_does_not() {
    // Cluster 2 (cell 4) is flagged pinned; cluster 3 (cell 5) carries only a
    // priority region's rank.
    let hints = [
        ClusterHintRecord {
            cluster_id: 3,
            flags: 0,
            priority: 7,
        },
        ClusterHintRecord {
            cluster_id: 2,
            flags: CLUSTER_HINT_FLAG_PINNED,
            priority: 0,
        },
    ];
    let mut fixture = input(
        bc6h_formats(64, 1, false),
        &[0, 0, 1, 1, 2, 3],
        Vec::new(),
        vec![pair(0, 1, 20 * METER), pair(2, 3, 20 * METER)],
    );
    fixture.pinned_clusters = pinned_clusters(&hints);
    assert_eq!(fixture.pinned_clusters, vec![2]);
    let pinned = pinned_cells(&fixture);
    assert_eq!(pinned, vec![4]);

    let neighbors = Neighbors::from_pairs(6, &fixture.coupled_pairs);
    let pvs: Vec<Vec<u32>> = (0..6).map(|cell| vec![cell]).collect();
    let sources = sources(&neighbors, &pvs, &pinned);
    let cameras = [0, 1, 2, 3, 4, 5];
    let map = build_lead_map(&sources, &cameras, meters_fixed(32));
    for camera in cameras {
        for lead in [0, 16, 32] {
            let set = map.mandatory(camera, meters_fixed(lead), &pinned);
            assert!(set.contains(&4), "camera {camera} L={lead}: {set:?}");
            assert_eq!(
                set.contains(&5),
                camera == 5,
                "camera {camera} L={lead}: an unflagged cluster joins only through W or PVS"
            );
            assert!(
                !map.band(camera, meters_fixed(lead), &pinned).contains(&4),
                "a pinned cell is always mandatory, never band"
            );
        }
    }
    // The map itself omits pins, as the wire format does.
    assert_eq!(lead_of(&map, 0, 4), None);
}

/// Chain 0..6 with hub distances 10 m apart; each cell sees itself and its
/// next cell, and cell 2 also sees cell 5.
fn lead_fixture() -> (Neighbors, Vec<Vec<u32>>) {
    let mut pairs = Vec::new();
    for a in 0..6u32 {
        for b in a + 1..6 {
            pairs.push(pair(a, b, (b - a) * 10 * METER));
        }
    }
    // Cell 1 sits exactly 16 m from cell 4 by a shortcut, and 16 m + 1 unit
    // from cell 3.
    for p in &mut pairs {
        match (p.cell_a, p.cell_b) {
            (1, 4) => p.distance = 16 * METER,
            (1, 3) => p.distance = 16 * METER + 1,
            _ => {}
        }
    }
    let neighbors = Neighbors::from_pairs(6, &pairs);
    let pvs = vec![
        vec![0, 1],
        vec![1, 2],
        vec![2, 3, 5],
        vec![3, 4],
        vec![4, 5],
        vec![5],
    ];
    (neighbors, pvs)
}

#[test]
fn camera_cell_leads_itself_at_zero() {
    let (neighbors, pvs) = lead_fixture();
    let sources = sources(&neighbors, &pvs, &[]);
    let cameras: Vec<u32> = (0..6).collect();
    let map = build_lead_map(&sources, &cameras, meters_fixed(32));
    for camera in cameras {
        assert_eq!(lead_of(&map, camera, camera), Some(0), "camera {camera}");
        assert_eq!(map.entries_of(camera)[0].1, 0);
    }
}

#[test]
fn lead_map_matches_direct_evaluation_at_every_lead() {
    let (neighbors, pvs) = lead_fixture();
    let graph = chain_graph(6);
    let cameras: Vec<u32> = (0..6).collect();
    for dilation in Dilation::ALL {
        let visible = visible_sources(&pvs, &portal_neighbours(&graph), dilation);
        let sources = sources(&neighbors, &visible, &[]);
        let map = build_lead_map(&sources, &cameras, meters_fixed(32));
        let mut leads: Vec<u32> = (0..=32).map(meters_fixed).collect();
        leads.extend([16 * METER - 1, 16 * METER + 1, 32 * METER - 1]);
        assert_eq!(
            check_against_direct(&map, &sources, &cameras, &leads),
            (cameras.len() * leads.len(), 0),
            "{dilation:?}"
        );
        let every = check_every_breakpoint(&map, &sources, &cameras);
        assert_eq!(every.mismatched, 0, "{dilation:?}");
        assert!(every.checked >= cameras.len(), "{every:?}");
        for camera in &cameras {
            let entries = map.entries_of(*camera);
            assert!(
                entries
                    .windows(2)
                    .all(|w| (w[0].1, w[0].0) < (w[1].1, w[1].0)),
                "sorted by lead then cell, no duplicates: {entries:?}"
            );
            assert!(entries.iter().all(|&(_, lead)| lead <= 32 * METER));
        }
        // CSR over every cell id: a non-camera cell has an empty range.
        assert_eq!(map.offsets.len(), 7);
    }
    let sources = sources(&neighbors, &pvs, &[]);
    let map = build_lead_map(&sources, &cameras, meters_fixed(32));
    // Cell 4 is reached at exactly 16 m, so it is mandatory at L = 16.
    assert_eq!(lead_of(&map, 1, 4), Some(16 * METER));
    assert!(map.mandatory(1, meters_fixed(16), &[]).contains(&4));
    // Cell 3 is seen from cell 2 at 10 m before cell 3 itself is reached.
    assert_eq!(lead_of(&map, 1, 3), Some(10 * METER));
    // Cell 5: seen from cell 2 (10 m), not from cell 4 (16 m) or itself.
    assert_eq!(lead_of(&map, 1, 5), Some(10 * METER));
    assert_eq!(direct_set(&sources, 1, 0), vec![1, 2]);
    // Cell 0 is 10 m away and nobody nearer sees it.
    assert_eq!(lead_of(&map, 1, 0), Some(10 * METER));
}

/// A hand-built lead map over cells 0–4: camera A = 0, camera B = 1.
fn band_lead_map() -> LeadMap {
    let a = [(0, 0), (2, 10 * METER), (3, 12 * METER)];
    let b = [(1, 0), (4, 16 * METER), (2, 32 * METER)];
    let entries: Vec<(u32, u32)> = a.into_iter().chain(b).collect();
    LeadMap {
        max_lead_fixed: 32 * METER,
        offsets: vec![0, 3, 6, 6, 6, 6],
        entries,
    }
}

#[test]
fn band_retention_keeps_a_block_exactly_in_the_band() {
    let map = band_lead_map();
    let fixed = meters_fixed(16);
    // Lead exactly 16 m is mandatory; exactly the 32 m maximum is band.
    assert_eq!(map.mandatory(1, fixed, &[]), vec![1, 4]);
    assert_eq!(map.band(1, fixed, &[]), vec![2]);
    assert!(map.band(0, fixed, &[]).is_empty());

    let fixture = input(
        bc6h_formats(2048, 1, true),
        &[0, 1, 2, 3, 4],
        (0..5).map(|cell| chart(cell, 0, 0, 0, 64, 64)).collect(),
        Vec::new(),
    );
    let blocks = CellBlocks::new(&fixture);
    let mandatory: Vec<Vec<u32>> = (0..2).map(|c| map.mandatory(c, fixed, &[])).collect();
    let band: Vec<Vec<u32>> = (0..2).map(|c| map.band(c, fixed, &[])).collect();
    let inputs = BandSimInputs {
        blocks: &blocks,
        mandatory: &mandatory,
        band: &band,
    };
    // A, B, A: cell 2 leaves M(A) for B's band, then re-enters M(A).
    let path = [0, 1, 0];
    let retain = simulate_band(&inputs, &path, None, BandPolicy::BandRetain);
    let immediate = simulate_band(&inputs, &path, None, BandPolicy::ImmediateFree);
    // Entering after the first step: 1 and 4 at B; 0, 2 and 3 back at A.
    assert_eq!(retain.entering_blocks, 5);
    assert_eq!(retain.entering_resident, 1, "cell 2 stayed resident");
    assert_eq!(
        retain.thrash_reads, 2,
        "cells 0 and 3 were freed and re-read"
    );
    assert_eq!(immediate.entering_blocks, 5);
    assert_eq!(immediate.entering_resident, 0);
    assert_eq!(immediate.thrash_reads, 3, "cell 2 is re-read too");
    assert_eq!(retain.prefetch_reads, 0, "cell 2 was already resident at B");
    for run in [retain, immediate] {
        assert_eq!((run.growth_steps, run.repack_steps), (0, 0));
    }
}

/// One chart per cell at each `(width, height)`, so each block is exactly
/// that size.
fn sized_blocks(sizes: &[(u32, u32)]) -> CellBlocks {
    let cells: Vec<u32> = (0..sizes.len() as u32).collect();
    let charts = sizes
        .iter()
        .zip(&cells)
        .map(|(&(width, height), &cell)| chart(cell, 0, 0, 0, width, height))
        .collect();
    let blocks = CellBlocks::new(&input(
        bc6h_formats(2048, 1, true),
        &cells,
        charts,
        Vec::new(),
    ));
    for (cell, &(width, height)) in sizes.iter().enumerate() {
        assert_eq!(blocks.cell_dims(cell as u32), [BlockDims { width, height }]);
    }
    blocks
}

/// A band-retain walk over hand-picked per-camera sets.
fn band_walk(
    blocks: &CellBlocks,
    mandatory: &[Vec<u32>],
    band: &[Vec<u32>],
    path: &[u32],
    cap: u32,
) -> BandRun {
    let inputs = BandSimInputs {
        blocks,
        mandatory,
        band,
    };
    simulate_band(&inputs, path, Some(cap), BandPolicy::BandRetain)
}

#[test]
fn mandatory_blocks_grow_past_a_cap_and_prefetch_stays_under_it() {
    let map = band_lead_map();
    let fixed = meters_fixed(16);
    let blocks = sized_blocks(&[(64, 64); 5]);
    let mandatory = vec![Vec::new(), map.mandatory(1, fixed, &[])];
    let band = vec![Vec::new(), map.band(1, fixed, &[])];
    // A zero-layer cap holds nothing: both mandatory blocks grow past it, and
    // no band block is placed past the cap.
    let run = band_walk(&blocks, &mandatory, &band, &[1], 0);
    assert_eq!((run.growth_steps, run.over_cap_steps), (1, 1));
    assert_eq!(
        (run.demand_reads, run.prefetch_reads),
        (2, 0),
        "cells 1 and 4, not band cell 2"
    );
    // A one-layer cap fits all three.
    let run = band_walk(&blocks, &mandatory, &band, &[1], 1);
    assert_eq!(
        (run.growth_steps, run.demand_reads, run.prefetch_reads),
        (0, 2, 1)
    );

    // Camera 0 needs three layers against a two-layer cap: 0 and 1 fill
    // layers 0 and most of 1, and 2 opens layer 2. Band cell 3 still fits
    // the rest of layer 1, under the cap, though the pool is over it.
    let blocks = sized_blocks(&[(2048, 2048), (2048, 1536), (2048, 1024), (64, 64)]);
    let mandatory = vec![vec![0, 1, 2], vec![0, 1]];
    let band = vec![vec![3], vec![3, 2]];
    let run = band_walk(&blocks, &mandatory, &band, &[0], 2);
    assert_eq!((run.peak_layers, run.growth_steps), (3, 1));
    assert_eq!(
        run.prefetch_reads, 1,
        "band cell 3 prefetched under the cap"
    );
    // At camera 1, cell 2 drops into the band while sitting past the cap: it
    // is freed, so the pool falls back under the cap and stays there.
    let run = band_walk(&blocks, &mandatory, &band, &[0, 1, 1, 1], 2);
    assert_eq!(run.growth_steps, 1);
    assert_eq!(run.over_cap_steps, 1, "only the step that grew ends over");
    assert_eq!(
        (run.demand_reads, run.prefetch_reads),
        (3, 1),
        "cell 2 finds no room under the cap to come back"
    );
}

#[test]
fn a_repack_moves_a_resident_band_block_without_a_read() {
    // One layer. Camera 0 stacks 0 (768 high) and 1 (512 high) and prefetches
    // band cell 3 below them. At camera 1, cell 0 leaves, and 2 (1280 high)
    // fits no shelf even with 3 evicted: a repack places 2 and 1, then moves
    // 3 back into the space left under the cap.
    let blocks = sized_blocks(&[(2048, 768), (2048, 512), (2048, 1280), (64, 64)]);
    let mandatory = vec![vec![0, 1], vec![1, 2]];
    let band = vec![vec![3], vec![3]];
    let run = band_walk(&blocks, &mandatory, &band, &[0, 1], 1);
    assert_eq!(run.repack_steps, 1);
    assert_eq!(run.demand_reads, 3, "cells 0 and 1, then 2");
    assert_eq!(run.prefetch_reads, 1, "cell 3 is read once and then moved");
    assert_eq!((run.band_evictions, run.thrash_reads), (0, 0));
    assert_eq!(run.peak_layers, 1);
}

#[test]
fn a_victim_is_not_read_back_in_the_step_that_evicted_it() {
    // One layer. Camera 0 places 0 (1024 high) and prefetches band cells 1
    // and 2 onto one 64-high shelf. At camera 1, cell 3 (1088 high) fits only
    // once both are evicted; the space left would hold them again.
    let blocks = sized_blocks(&[(2048, 1024), (64, 64), (64, 64), (2048, 1088)]);
    let mandatory = vec![vec![0], vec![3]];
    let band = vec![vec![1, 2], vec![1, 2]];
    let run = band_walk(&blocks, &mandatory, &band, &[0, 1], 1);
    assert_eq!((run.repack_steps, run.band_evictions), (0, 2));
    assert_eq!(run.prefetch_reads, 2, "no read back in the evicting step");
    assert_eq!(run.thrash_reads, 0);
    // The next step reads both back, as thrash.
    let run = band_walk(&blocks, &mandatory, &band, &[0, 1, 1], 1);
    assert_eq!((run.prefetch_reads, run.thrash_reads), (4, 2));
}

#[test]
fn u_turn_brief_set_drops_the_cluster_term_and_dilates_one_hop() {
    // Sampled PVS: 0 -> {0, 1}, 1 -> {0, 1, 2}, 2 -> {1, 2}; solid cell 3
    // shares cluster 2 with cell 2 and has no portal.
    let report = run_dry_run(&u_turn_input());
    let visible = report
        .visible_set
        .as_ref()
        .expect("world and graph present");
    let brief = &visible.brief_set;
    let [dilated, undilated] = &brief.variants[..] else {
        panic!("one variant per dilation");
    };
    assert_eq!(
        (dilated.dilation, undilated.dilation),
        (Dilation::OneHop, Dilation::None)
    );
    for variant in [dilated, undilated] {
        assert_eq!(variant.consistency.mismatched, 0, "{:?}", variant.dilation);
        assert!(
            variant.consistency.checked >= 3,
            "{:?}",
            variant.consistency
        );
    }
    assert_eq!(undilated.leads[0].set_cells, vec![2, 3, 2]);
    assert_eq!(dilated.leads[0].set_cells, vec![3, 3, 3]);
    // The cell-granular set adds cell 2's cluster-mate, solid cell 3.
    let old = &visible.dense[0];
    assert_eq!(old.granularity, Granularity::Cell);
    assert_eq!(old.leads[0].cells[2].1.cell_count, 3);
    // One-layer sets: the shelf p95 and worst caps merge; 125% rounds to 2.
    let caps: Vec<(u32, &str)> = brief
        .walks
        .caps
        .iter()
        .map(|cap| (cap.layers, cap.basis.as_str()))
        .collect();
    assert_eq!(
        caps,
        vec![(1, "shelf p95 = 100% shelf worst"), (2, "125% shelf worst")]
    );
    assert!(brief.walks.walks.iter().all(|walk| walk.runs.len() == 6));
    let rendered = report.render();
    assert!(rendered.contains("-- brief set:"));
    assert!(rendered.contains("dilated 0 mismatched"));
    assert!(rendered.contains("would-be cell residency section"));
    assert!(rendered.contains("-- brief-set pool walks:"));
    assert_eq!(rendered, run_dry_run(&u_turn_input()).render());
}
