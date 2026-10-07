//! Compose membership narrowed to entry-carrying sparse rows, driven through
//! the real install, frame-planning, and commit path on synthetic maps.
//! See: context/lib/rendering_pipeline.md §4 "Sampled-row compose"

use super::compose_plan::{ComposePass, PassMembership};
use super::install_tests::SyntheticMap;
use super::*;
use crate::render::{ShSampleRegion, ShSampleRegionSets};

/// Four bricks, two per cluster: cluster 0 owns rows 0–1, cluster 1 rows 2–3.
///
/// | row | id 27 | id 41 | id 45 |
/// |-----|-------|-------|-------|
/// | 0   | entry | entry | —     |
/// | 1   | —     | —     | —     |
/// | 2   | —     | —     | —     |
/// | 3   | entry | —     | entry |
fn mixed_map() -> SyntheticMap {
    SyntheticMap::new([16, 4, 4], [2, 1, 1])
        .with_empty_rows(INDIRECT_DELTA_ID, &[1, 2])
        .with_empty_rows(DIRECT_DELTA_ID, &[1, 2, 3])
        .with_empty_rows(ANIMATED_DIRECT_DELTA_ID, &[0, 1, 2])
}

const ALL_ROWS: [u32; 4] = [0, 1, 2, 3];
const PASSES: [ComposePass; 3] = [
    ComposePass::Indirect,
    ComposePass::StaticDirect,
    ComposePass::AnimatedDirect,
];

#[derive(Clone)]
enum Gate {
    /// Fog reach draws everything: every resident row is gated.
    All,
    /// Only these rows, through one small mover region per brick.
    Rows(Vec<u32>),
}

#[derive(Clone)]
struct Frame {
    gate: Gate,
    records: bool,
    force_full: bool,
    indirect_active: bool,
    animated_active: bool,
    static_weight: f32,
    mask: LightTermMask,
}

impl Frame {
    fn idle() -> Self {
        Self {
            gate: Gate::All,
            records: true,
            force_full: false,
            indirect_active: false,
            animated_active: false,
            static_weight: 0.0,
            mask: LightTermMask::ALL,
        }
    }

    /// Both animated triggers fire; promotion weights hold.
    fn per_source() -> Self {
        Self {
            indirect_active: true,
            animated_active: true,
            ..Self::idle()
        }
    }

    fn gate(mut self, rows: &[u32]) -> Self {
        self.gate = Gate::Rows(rows.to_vec());
        self
    }

    fn skipped(mut self) -> Self {
        self.records = false;
        self
    }
}

/// Rows each pass planned, in [`PASSES`] order.
type Planned = [Vec<u32>; 3];

struct Harness {
    map: SyntheticMap,
    state: ShResidencyState,
}

impl Harness {
    fn new(map: SyntheticMap) -> Self {
        let state = map.state();
        Self { map, state }
    }

    fn install(&mut self, cluster_id: u32) {
        let prepared = self.map.prepared(&self.state, cluster_id);
        self.state.install(None, &prepared).unwrap();
    }

    fn evict(&mut self, cluster_id: u32) {
        self.state
            .evict(&mut StagedUploads::default(), cluster_id)
            .unwrap();
    }

    /// Plan one frame and, when it records, commit every non-empty pass as
    /// a successful encode would.
    fn frame(&mut self, frame: Frame) -> Planned {
        let planned = self.plan(frame.clone());
        if frame.records {
            self.commit();
        }
        planned
    }

    fn plan(&mut self, frame: Frame) -> Planned {
        let regions: Vec<ShSampleRegion> = match &frame.gate {
            Gate::All => Vec::new(),
            Gate::Rows(rows) => rows
                .iter()
                .map(|&row| {
                    // A point in the middle of brick `row`; its sampler
                    // dilation stays inside the brick.
                    let point = glam::Vec3::new(row as f32 * 4.0 + 1.5, 1.5, 1.5);
                    ShSampleRegion::new(point, point)
                })
                .collect(),
        };
        self.state
            .prepare_compose_frame(
                ShSampleRegionSets {
                    visible_cells: &postretro_visibility::VisibleCells::DrawAll,
                    fog_cells: &[],
                    movers: &regions,
                },
                None,
                false,
                matches!(frame.gate, Gate::All),
                frame.records,
                frame.force_full,
                frame.indirect_active,
                frame.animated_active,
                frame.mask,
                DirectShDebugOverride::default(),
                AnimatedDirectShDebugOverride::default(),
                &[frame.static_weight],
                &[],
            )
            .unwrap();
        let plan = self.state.compose_frame_plan.as_ref().unwrap();
        [
            plan.indirect.rows().to_vec(),
            plan.static_direct.rows().to_vec(),
            plan.animated_direct.rows().to_vec(),
        ]
    }

    /// The CPU half of each pass's dispatch, which runs after its encode.
    fn commit(&mut self) {
        let plan = self.state.compose_frame_plan.take().unwrap();
        if !plan.indirect.rows().is_empty() {
            self.state
                .commit_indirect_compose(&plan.indirect, 1)
                .unwrap();
        }
        if !plan.static_direct.rows().is_empty() {
            self.state
                .commit_static_direct_compose(&plan.static_direct, 1);
        }
        if !plan.animated_direct.rows().is_empty() {
            self.state.commit_animated_direct_compose(
                &plan.animated_direct,
                &plan.static_direct,
                1,
            );
        }
        self.state.compose_frame_plan = Some(plan);
    }

    /// Commit only Pass A, as when Pass B's encode fails after it.
    fn commit_pass_a_only(&mut self) {
        let plan = self.state.compose_frame_plan.take().unwrap();
        self.state
            .commit_static_direct_compose(&plan.static_direct, 1);
        self.state.compose_frame_plan = Some(plan);
    }

    /// Install every cluster and compose their rows once, with no trigger.
    fn settled(map: SyntheticMap) -> Self {
        let mut harness = Self::new(map);
        for cluster_id in 0..harness.map.cluster_count() {
            harness.install(cluster_id);
        }
        harness.frame(Frame::idle());
        assert_eq!(harness.frame(Frame::idle()), Planned::default());
        harness
    }

    fn lagging(&self, pass: ComposePass) -> usize {
        self.state.compose_planner.lagging_rows(pass)
    }

    fn stale(&self, pass: ComposePass, row: u32) -> bool {
        self.state.compose_planner.is_stale(pass, row)
    }
}

fn pass_membership(state: &ShResidencyState, pass: ComposePass, row: u32) -> PassMembership {
    let membership = state.compose_row_membership(row);
    match pass {
        ComposePass::Indirect => membership.indirect,
        ComposePass::StaticDirect => membership.static_direct,
        ComposePass::AnimatedDirect => membership.animated_direct,
    }
}

#[test]
fn membership_follows_installed_entry_counts() {
    let map = mixed_map();
    let mut harness = Harness::new(mixed_map());
    harness.install(0);
    harness.install(1);
    // Observe the install through the planner, as a frame would.
    harness.plan(Frame::idle().skipped());

    let state = &harness.state;
    for row in ALL_ROWS {
        let entries = |section_id| map.entry_count(section_id, row) > 0;
        let expected = [
            (ComposePass::Indirect, entries(INDIRECT_DELTA_ID), false),
            (ComposePass::StaticDirect, entries(DIRECT_DELTA_ID), false),
            (
                ComposePass::AnimatedDirect,
                entries(ANIMATED_DIRECT_DELTA_ID),
                entries(DIRECT_DELTA_ID),
            ),
        ];
        for (pass, contributing, upstream) in expected {
            let membership = PassMembership {
                resident: true,
                contributing,
                upstream,
            };
            assert_eq!(
                pass_membership(state, pass, row),
                membership,
                "{pass:?} row {row}"
            );
            assert_eq!(
                state.compose_planner.observed_membership(pass, row),
                membership,
                "{pass:?} row {row} as the planner observed it"
            );
        }
    }
    // Every sparse family installs a row for every covered brick, empty or
    // not, so each pass's ref table still names all four rows.
    for refs in [
        &state.indirect_delta_row_refs,
        &state.direct_promotion_row_refs,
        &state.direct_animated_row_refs,
    ] {
        assert_eq!(refs.keys().copied().collect::<Vec<_>>(), ALL_ROWS);
    }
}

#[test]
fn zero_entry_rows_join_resident_unions_and_dirty_sets() {
    let mut harness = Harness::new(mixed_map());
    harness.install(0);
    let state = &harness.state;
    // Row 1 has no entry in any family, yet it is resident and dirty in
    // every pass like its entry-carrying neighbour.
    for row in [0, 1] {
        assert!(state.indirect_resident_rows.contains(&row));
        assert!(state.direct_promotion_resident_rows.contains(&row));
        assert!(state.direct_animated_resident_rows.contains(&row));
        assert!(state.indirect_dirty_rows.contains(&row));
        assert!(state.direct_promotion_dirty_rows.contains(&row));
        assert!(state.direct_animated_dirty_rows.contains(&row));
        for section_id in [INDIRECT_DELTA_ID, DIRECT_DELTA_ID, ANIMATED_DIRECT_DELTA_ID] {
            assert!(state.dirty_rows.contains(&(section_id, row)));
        }
    }
    assert_eq!(state.resident_rows(), state.rebuilt_resident_rows());

    harness.install(1);
    harness.frame(Frame::idle());
    harness.evict(0);
    let state = &harness.state;
    for row in [0, 1] {
        assert!(!state.indirect_resident_rows.contains(&row));
        assert!(!state.direct_promotion_resident_rows.contains(&row));
        assert!(!state.direct_animated_resident_rows.contains(&row));
        assert!(state.indirect_dirty_rows.contains(&row));
    }
    assert_eq!(state.resident_rows(), state.rebuilt_resident_rows());
    assert_eq!(
        state.resident_rows(),
        [
            BTreeSet::from([2, 3]),
            BTreeSet::from([2, 3]),
            BTreeSet::from([2, 3])
        ]
    );
}

#[test]
fn per_source_trigger_plans_only_entry_carrying_rows() {
    let mut harness = Harness::settled(mixed_map());
    // Activity fires the indirect and Pass B per-source triggers; the
    // promotion weights hold, so Pass A has no trigger.
    let planned = harness.frame(Frame::per_source());
    assert_eq!(planned, [vec![0, 3], vec![], vec![3]]);
    // Activity stays on: the same rows, every frame.
    assert_eq!(harness.frame(Frame::per_source()), planned);
    assert_eq!(
        harness
            .state
            .indirect_compose_diagnostics
            .entry_rows_composed,
        2
    );
    assert_eq!(
        harness
            .state
            .animated_direct_compose_diagnostics
            .entry_rows_composed,
        1
    );
}

#[test]
fn zero_payload_records_keep_their_rows_contributing() {
    let map = mixed_map();
    // Every synthetic entry carries an all-zero payload, as a retained
    // script-mutable record (ids 27, 45) or id-41's last kept promotion
    // record does. Entry presence, not payload, decides membership.
    let state = map.state();
    let prepared = map.prepared(&state, 1);
    for block in prepared
        .chunk
        .blocks
        .iter()
        .filter(|block| block.kind == SPARSE_ROWS_BLOCK)
    {
        for row in parse_sparse_rows(1, prepared.chunk.block_bytes(block)).unwrap() {
            assert!(row.tile_f16.iter().all(|&half| half == 0));
        }
    }

    let mut harness = Harness::settled(mixed_map());
    let planned = harness.frame(Frame::per_source());
    assert!(planned[0].contains(&3), "id-27 zero-payload row");
    assert!(planned[2].contains(&3), "id-45 zero-payload row");
    let promotion = harness.frame(Frame {
        static_weight: 0.5,
        ..Frame::idle()
    });
    assert_eq!(promotion[1], [0], "id-41 zero-payload row");
}

#[test]
fn pass_b_upstream_follows_id41_entries() {
    let mut harness = Harness::settled(mixed_map());
    // Only the promotion weights change. That fires Pass A's trigger and
    // both of Pass B's: upstream (id-41 rows) and contributing (id-45 rows).
    let planned = harness.frame(Frame {
        static_weight: 0.5,
        ..Frame::idle()
    });
    assert_eq!(planned[0], Vec::<u32>::new());
    assert_eq!(planned[1], [0]);
    // Row 0 has id-41 entries and no id-45; rows 1 and 2 have neither.
    assert_eq!(planned[2], [0, 3]);

    // Outside the gate, the id-41 row lags in both direct passes and the
    // no-entry rows lag in neither.
    let hidden = harness.frame(
        Frame {
            static_weight: 0.75,
            ..Frame::idle()
        }
        .gate(&[]),
    );
    assert_eq!(hidden, Planned::default());
    assert!(harness.stale(ComposePass::StaticDirect, 0));
    assert!(harness.stale(ComposePass::AnimatedDirect, 0));
    for row in [1, 2] {
        assert!(!harness.stale(ComposePass::StaticDirect, row));
        assert!(!harness.stale(ComposePass::AnimatedDirect, row));
    }
    assert_eq!(harness.lagging(ComposePass::StaticDirect), 1);
    assert_eq!(harness.lagging(ComposePass::AnimatedDirect), 2);

    // On re-entry Pass A recomposes the row, then Pass B.
    let reentry = harness.frame(Frame {
        static_weight: 0.75,
        ..Frame::idle()
    });
    assert_eq!(reentry, [vec![], vec![0], vec![0, 3]]);
}

#[test]
fn pass_a_rewrite_reaches_pass_b_without_id45_entries() {
    let mut harness = Harness::settled(mixed_map());
    let planned = harness.plan(Frame {
        static_weight: 0.5,
        ..Frame::idle()
    });
    assert_eq!(planned[1], [0]);
    // Pass B fails after Pass A committed its rewrite of row 0, which has
    // no id-45 entry. The rewrite left durable Pass B work: it retries even
    // with an empty gate and no trigger.
    harness.commit_pass_a_only();
    let held = Frame {
        static_weight: 0.5,
        ..Frame::idle()
    };
    let retry = harness.frame(held.clone().gate(&[]));
    assert_eq!(retry, [vec![], vec![], vec![0]]);
    // Row 3's failed Pass B work is ordinary lag: it waits for the gate.
    assert_eq!(harness.frame(held.clone()), [vec![], vec![], vec![3]]);
    assert_eq!(harness.frame(held), Planned::default());
}

#[test]
fn trigger_over_only_zero_entry_rows_plans_nothing() {
    let all_empty = SyntheticMap::new([16, 4, 4], [2, 1, 1])
        .with_empty_rows(INDIRECT_DELTA_ID, &ALL_ROWS)
        .with_empty_rows(DIRECT_DELTA_ID, &ALL_ROWS)
        .with_empty_rows(ANIMATED_DIRECT_DELTA_ID, &ALL_ROWS);
    let mut harness = Harness::settled(all_empty);
    for frame in [
        Frame::per_source(),
        Frame {
            static_weight: 0.5,
            ..Frame::per_source()
        },
        Frame::idle(),
    ] {
        assert_eq!(harness.frame(frame), Planned::default());
        for pass in PASSES {
            assert_eq!(harness.lagging(pass), 0, "{pass:?}");
        }
        for diagnostics in [
            harness.state.indirect_compose_diagnostics,
            harness.state.static_direct_compose_diagnostics,
            harness.state.animated_direct_compose_diagnostics,
        ] {
            assert_eq!(diagnostics, ShComposePassDiagnostics::default());
        }
    }
}

#[test]
fn residency_changes_plan_zero_entry_rows_once() {
    // Install on a frame where the per-source trigger fires.
    let mut harness = Harness::new(mixed_map());
    harness.install(0);
    let install = harness.frame(Frame::per_source().gate(&[]));
    assert_eq!(install, [vec![0, 1], vec![0, 1], vec![0, 1]]);
    let next = harness.frame(Frame::per_source());
    assert_eq!(next, [vec![0], vec![], vec![]]);

    // Slot reuse: cluster 0's rows leave and cluster 1's reuse their slots
    // in the same drain, again under the per-source trigger.
    let freed = harness.state.sparse_pools[&INDIRECT_DELTA_ID].row_pairs[0];
    harness.evict(0);
    harness.install(1);
    assert_eq!(
        harness.state.sparse_pools[&INDIRECT_DELTA_ID].row_pairs[3], freed,
        "row 3's entry reuses row 0's freed range"
    );
    let reuse = harness.frame(Frame::per_source().gate(&[]));
    assert_eq!(reuse, [vec![2, 3], vec![2, 3], vec![2, 3]]);
    assert_eq!(harness.lagging(ComposePass::Indirect), 0);
    let next = harness.frame(Frame::per_source());
    assert_eq!(next, [vec![3], vec![], vec![3]]);
}

#[test]
fn partial_eviction_plans_its_rows_regardless_of_gate() {
    // Cross-owned rows make one eviction partial for two rows. Row 1 is
    // zero-entry: its base belongs to cluster 0, its sparse rows to cluster
    // 1. Row 2 carries an id-27 entry: its sparse rows belong to cluster 0,
    // its base to cluster 1.
    let map = SyntheticMap::new([16, 4, 4], [2, 1, 1])
        .with_empty_rows(INDIRECT_DELTA_ID, &[1])
        .with_empty_rows(DIRECT_DELTA_ID, &[1, 2, 3])
        .with_empty_rows(ANIMATED_DIRECT_DELTA_ID, &[0, 1, 2])
        .with_sparse_row_owner(1, 1)
        .with_sparse_row_owner(2, 0);
    let mut harness = Harness::settled(map);

    // Row 2 leaves the gate and goes stale.
    let hidden = harness.frame(Frame::per_source().gate(&[0, 1, 3]));
    assert_eq!(hidden[0], [0, 3]);
    assert!(harness.stale(ComposePass::Indirect, 2));

    // Evicting cluster 0 leaves rows 1 and 2 resident through their other
    // owner. Both are planned while still outside the gate.
    harness.evict(0);
    assert_eq!(harness.state.resident_rows()[0], BTreeSet::from([1, 2, 3]));
    let eviction = harness.frame(Frame::per_source().gate(&[]));
    for (pass, rows) in PASSES.into_iter().zip(&eviction) {
        assert!(rows.contains(&1) && rows.contains(&2), "{pass:?}: {rows:?}");
        assert!(!harness.stale(pass, 1) && !harness.stale(pass, 2));
    }

    // Neither row carries an entry now, so the trigger alone skips them.
    let next = harness.frame(Frame::per_source());
    assert_eq!(next, [vec![3], vec![], vec![3]]);
}

#[test]
fn control_change_and_force_full_plan_zero_entry_rows() {
    let mut harness = Harness::settled(mixed_map());
    let mut mask = LightTermMask::ALL;
    mask.set_enabled(LightTermMask::SPECULAR, false);
    let changed = harness.frame(Frame {
        mask,
        ..Frame::idle().gate(&[])
    });
    assert_eq!(
        changed,
        [ALL_ROWS.to_vec(), ALL_ROWS.to_vec(), ALL_ROWS.to_vec()]
    );
    assert_eq!(
        harness.frame(Frame {
            mask,
            ..Frame::idle()
        }),
        Planned::default()
    );

    let forced = harness.frame(Frame {
        mask,
        force_full: true,
        ..Frame::idle().gate(&[])
    });
    assert_eq!(
        forced,
        [ALL_ROWS.to_vec(), ALL_ROWS.to_vec(), ALL_ROWS.to_vec()]
    );
}

#[test]
fn zero_entry_row_reenters_gate_current() {
    let mut harness = Harness::settled(mixed_map());
    // Every row leaves the gate while current, and activity runs for three
    // frames, then stops (the deactivation tail fires once more).
    for _ in 0..3 {
        assert_eq!(
            harness.frame(Frame::per_source().gate(&[])),
            Planned::default()
        );
    }
    assert_eq!(harness.frame(Frame::idle().gate(&[])), Planned::default());
    // Only the entry-carrying rows lag: rows 0 and 3 in indirect, row 3 in
    // Pass B. Zero-entry rows 1 and 2 never count.
    assert_eq!(harness.lagging(ComposePass::Indirect), 2);
    assert_eq!(harness.lagging(ComposePass::StaticDirect), 0);
    assert_eq!(harness.lagging(ComposePass::AnimatedDirect), 1);
    for pass in PASSES {
        assert!(!harness.stale(pass, 1) && !harness.stale(pass, 2));
    }

    // Re-entry on a frame with no trigger recomposes only the stale rows.
    let reentry = harness.frame(Frame::idle());
    assert_eq!(reentry, [vec![0, 3], vec![], vec![3]]);
    for pass in PASSES {
        assert_eq!(harness.lagging(pass), 0);
    }
}

#[test]
fn deactivation_tail_plans_only_entry_carrying_rows() {
    let entry_rows: Planned = [vec![0, 3], vec![], vec![3]];

    // P5: activity returns to zero on a recording frame.
    let mut harness = Harness::settled(mixed_map());
    assert_eq!(harness.frame(Frame::per_source()), entry_rows);
    assert_eq!(harness.frame(Frame::idle()), entry_rows);
    assert_eq!(harness.frame(Frame::idle()), Planned::default());

    // P6: it returns to zero on a frame that records no compose.
    let mut harness = Harness::settled(mixed_map());
    harness.frame(Frame::per_source());
    harness.frame(Frame::idle().skipped());
    assert_eq!(harness.frame(Frame::idle()), entry_rows);
    assert_eq!(harness.frame(Frame::idle()), Planned::default());

    // P7: 1 → 0 → 1 over three recorded frames.
    let mut harness = Harness::settled(mixed_map());
    for frame in [Frame::per_source(), Frame::idle(), Frame::per_source()] {
        assert_eq!(harness.frame(frame), entry_rows);
    }
}
