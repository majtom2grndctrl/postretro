//! GPU-free trigger, staleness, and retry planning for streamed SH compose.
//! See: context/lib/rendering_pipeline.md §4 "Sampled-row compose"

use postretro_render_cpu::frame_uniforms::LightTermMask;

use super::compose_staleness::{PassStaleness, PassTriggers};
use super::{AnimatedDirectShDebugOverride, DirectShDebugOverride};

/// The three streamed passes in their required encode order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ComposePass {
    Indirect,
    StaticDirect,
    AnimatedDirect,
}

/// One row's membership in one pass's change sources.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct PassMembership {
    pub(super) resident: bool,
    /// The row carries the pass's own streamed contribution.
    pub(super) contributing: bool,
    /// Pass B only: the row carries a Pass-A (id-41) contribution, so its
    /// promotion term changes when static weights do.
    pub(super) upstream: bool,
}

/// A row's membership in every pass, as of the frame being planned.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct RowMembership {
    pub(super) indirect: PassMembership,
    pub(super) static_direct: PassMembership,
    pub(super) animated_direct: PassMembership,
}

/// Input values that force a full-resident repair when they change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ComposeControlSnapshot {
    pub(super) light_term_mask: LightTermMask,
    pub(super) promotion_override: DirectShDebugOverride,
    pub(super) animated_override: AnimatedDirectShDebugOverride,
}

/// One logical frame's complete planner input.
pub(super) struct ComposePlannerFrame<'a> {
    /// False when no command encoder can record compose this frame. Input
    /// changes are still observed so gated rows become stale for a later frame.
    pub(super) records_compose: bool,
    /// Exactness/debug bypass: compose every resident row in every pass.
    pub(super) force_full_resident: bool,
    pub(super) gated_rows: &'a [u32],
    /// Current membership of every row whose residency or contribution may
    /// have changed since the previous plan. Rows may repeat or be unchanged.
    pub(super) membership_changes: &'a [(u32, RowMembership)],
    pub(super) indirect_active: bool,
    pub(super) animated_direct_active: bool,
    /// Exact f32 values uploaded by Pass A, after cache-layer zeroing.
    pub(super) effective_static_weights: &'a [f32],
    /// Exact complementary promotion scales uploaded by Pass B.
    pub(super) effective_animated_weights: &'a [f32],
    pub(super) controls: ComposeControlSnapshot,
}

/// Work for one pass. State changes only when this plan is committed after a
/// successful encode.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ComposePassPlan {
    pub(super) pass: ComposePass,
    pub(super) rows: Vec<u32>,
    /// Planner generation the rows become current at when committed.
    pub(super) generation: u64,
    pub(super) lagged_rows: usize,
    pub(super) full_repair: bool,
}

impl ComposePassPlan {
    #[cfg(test)]
    pub(super) fn pass(&self) -> ComposePass {
        self.pass
    }

    pub(super) fn rows(&self) -> &[u32] {
        &self.rows
    }

    pub(super) fn lagged_rows(&self) -> usize {
        self.lagged_rows
    }

    #[cfg(test)]
    pub(super) fn full_repair(&self) -> bool {
        self.full_repair
    }

    #[cfg(test)]
    pub(super) fn test_plan(pass: ComposePass, rows: Vec<u32>, lagged_rows: usize) -> Self {
        Self {
            pass,
            rows,
            generation: 0,
            lagged_rows,
            full_repair: false,
        }
    }

    fn empty(pass: ComposePass) -> Self {
        Self {
            pass,
            rows: Vec::new(),
            generation: 0,
            lagged_rows: 0,
            full_repair: false,
        }
    }

    pub(super) fn reset(&mut self, pass: ComposePass, generation: u64, full_repair: bool) {
        self.pass = pass;
        self.rows.clear();
        self.generation = generation;
        self.lagged_rows = 0;
        self.full_repair = full_repair;
    }
}

/// Current-frame work in encode order. Pass B is planned after Pass A and is
/// guaranteed to contain every row Pass A rewrites that Pass B holds.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ComposeFramePlan {
    pub(super) indirect: ComposePassPlan,
    pub(super) static_direct: ComposePassPlan,
    pub(super) animated_direct: ComposePassPlan,
}

impl ComposeFramePlan {
    fn new() -> Self {
        Self {
            indirect: ComposePassPlan::empty(ComposePass::Indirect),
            static_direct: ComposePassPlan::empty(ComposePass::StaticDirect),
            animated_direct: ComposePassPlan::empty(ComposePass::AnimatedDirect),
        }
    }

    fn clear(&mut self) {
        self.indirect.reset(ComposePass::Indirect, 0, false);
        self.static_direct
            .reset(ComposePass::StaticDirect, 0, false);
        self.animated_direct
            .reset(ComposePass::AnimatedDirect, 0, false);
    }

    #[cfg(test)]
    fn capacities(&self) -> [usize; 3] {
        self.ordered().map(|plan| plan.rows.capacity())
    }

    #[cfg(test)]
    pub(super) fn ordered(&self) -> [&ComposePassPlan; 3] {
        [&self.indirect, &self.static_direct, &self.animated_direct]
    }
}

impl Default for ComposeFramePlan {
    fn default() -> Self {
        Self::new()
    }
}

/// Persistent planner state. Observing a frame may make rows stale; only
/// `commit_pass` makes planned rows current or consumes pending residency work.
/// Per-frame cost scales with gated, pending, and membership-changed rows;
/// only a control-change repair and forced full-resident compose visit every
/// row.
pub(super) struct StreamedComposePlanner {
    indirect: PassStaleness,
    static_direct: PassStaleness,
    animated_direct: PassStaleness,
    indirect_was_active: bool,
    animated_direct_was_active: bool,
    static_weight_bits: Option<Vec<u32>>,
    animated_weight_bits: Option<Vec<u32>>,
    controls: Option<ComposeControlSnapshot>,
}

impl StreamedComposePlanner {
    /// `rows` bounds every affinity row id the level can name.
    pub(super) fn with_row_capacity(rows: usize) -> Self {
        Self {
            indirect: PassStaleness::with_row_capacity(rows),
            static_direct: PassStaleness::with_row_capacity(rows),
            animated_direct: PassStaleness::with_row_capacity(rows),
            indirect_was_active: false,
            animated_direct_was_active: false,
            static_weight_bits: None,
            animated_weight_bits: None,
            controls: None,
        }
    }

    /// Forget trigger history, staleness, and pending work. Observed
    /// membership survives; `clear_membership` drops it with the residency
    /// mirrors.
    pub(super) fn reset_generation(&mut self) {
        self.indirect.reset_generation();
        self.static_direct.reset_generation();
        self.animated_direct.reset_generation();
        self.indirect_was_active = false;
        self.animated_direct_was_active = false;
        self.static_weight_bits = None;
        self.animated_weight_bits = None;
        self.controls = None;
    }

    pub(super) fn clear_membership(&mut self) {
        self.indirect.clear_membership();
        self.static_direct.clear_membership();
        self.animated_direct.clear_membership();
    }

    pub(super) fn mark_residency_rows(
        &mut self,
        pass: ComposePass,
        rows: impl IntoIterator<Item = u32>,
    ) {
        let staleness = self.pass_mut(pass);
        for row in rows {
            staleness.mark_pending(row);
        }
    }

    #[cfg(test)]
    pub(super) fn plan_frame(&mut self, frame: ComposePlannerFrame<'_>) -> ComposeFramePlan {
        let mut plan = ComposeFramePlan::default();
        self.plan_frame_into(frame, &mut plan);
        plan
    }

    pub(super) fn plan_frame_into(
        &mut self,
        frame: ComposePlannerFrame<'_>,
        plan: &mut ComposeFramePlan,
    ) {
        plan.clear();
        for &(row, membership) in frame.membership_changes {
            self.indirect.observe_membership(row, membership.indirect);
            self.static_direct
                .observe_membership(row, membership.static_direct);
            self.animated_direct
                .observe_membership(row, membership.animated_direct);
        }
        self.indirect.retain_resident_pending();
        self.static_direct.retain_resident_pending();
        self.animated_direct.retain_resident_pending();

        let controls_changed = self
            .controls
            .replace(frame.controls)
            .is_some_and(|previous| previous != frame.controls);
        let static_weights_changed =
            snapshot_changed(&mut self.static_weight_bits, frame.effective_static_weights);
        let animated_weights_changed = snapshot_changed(
            &mut self.animated_weight_bits,
            frame.effective_animated_weights,
        );

        let indirect_changed = frame.indirect_active || self.indirect_was_active;
        let animated_changed = frame.animated_direct_active
            || self.animated_direct_was_active
            || animated_weights_changed
            || static_weights_changed;

        if controls_changed {
            self.indirect.full_repair_pending = true;
            self.static_direct.full_repair_pending = true;
            self.animated_direct.full_repair_pending = true;
        }
        // A control change or the exactness switch stales every resident row;
        // otherwise each pass stales only rows carrying its changed input.
        let resident = controls_changed || frame.force_full_resident;
        let per_source = !controls_changed;
        self.indirect.fire(PassTriggers {
            resident,
            contributing: per_source && indirect_changed,
            upstream: false,
        });
        self.static_direct.fire(PassTriggers {
            resident,
            contributing: per_source && static_weights_changed,
            upstream: false,
        });
        // Pass B reads Pass A's intermediate. Even a row with no id-45
        // contribution becomes stale when its promotion term changes.
        self.animated_direct.fire(PassTriggers {
            resident,
            contributing: per_source && animated_changed,
            upstream: per_source && static_weights_changed,
        });

        self.indirect_was_active = frame.indirect_active;
        self.animated_direct_was_active = frame.animated_direct_active;

        if !frame.records_compose {
            return;
        }

        self.indirect.plan_into(
            ComposePass::Indirect,
            frame.gated_rows,
            frame.force_full_resident,
            &mut plan.indirect,
        );
        self.static_direct.plan_into(
            ComposePass::StaticDirect,
            frame.gated_rows,
            frame.force_full_resident,
            &mut plan.static_direct,
        );
        // Planning Pass A creates durable Pass-B retry work before encoding.
        // Membership is already observed, so a row Pass B does not hold (a
        // level without id-45) would only be dropped as non-resident later.
        for &row in &plan.static_direct.rows {
            if self.animated_direct.resident(row) {
                self.animated_direct.mark_pending(row);
            }
        }
        self.animated_direct.plan_into(
            ComposePass::AnimatedDirect,
            frame.gated_rows,
            frame.force_full_resident,
            &mut plan.animated_direct,
        );
    }

    pub(super) fn commit_pass(&mut self, plan: &ComposePassPlan) {
        self.pass_mut(plan.pass).commit(plan);
    }

    /// Resident rows still lagging for `pass`. O(1).
    pub(super) fn lagging_rows(&self, pass: ComposePass) -> usize {
        self.pass(pass).lagging_count()
    }

    fn pass(&self, pass: ComposePass) -> &PassStaleness {
        match pass {
            ComposePass::Indirect => &self.indirect,
            ComposePass::StaticDirect => &self.static_direct,
            ComposePass::AnimatedDirect => &self.animated_direct,
        }
    }

    fn pass_mut(&mut self, pass: ComposePass) -> &mut PassStaleness {
        match pass {
            ComposePass::Indirect => &mut self.indirect,
            ComposePass::StaticDirect => &mut self.static_direct,
            ComposePass::AnimatedDirect => &mut self.animated_direct,
        }
    }

    #[cfg(test)]
    pub(super) fn is_stale(&self, pass: ComposePass, row: u32) -> bool {
        self.pass(pass).is_stale(row)
    }

    #[cfg(test)]
    pub(super) fn is_resident(&self, pass: ComposePass, row: u32) -> bool {
        self.pass(pass).resident(row)
    }

    #[cfg(test)]
    pub(super) fn observed_membership(&self, pass: ComposePass, row: u32) -> PassMembership {
        self.pass(pass).observed_membership(row)
    }

    #[cfg(test)]
    pub(super) fn reset_visited_for_test(&mut self) {
        self.indirect.visited = 0;
        self.static_direct.visited = 0;
        self.animated_direct.visited = 0;
    }

    #[cfg(test)]
    pub(super) fn visited_for_test(&self) -> usize {
        self.indirect.visited + self.static_direct.visited + self.animated_direct.visited
    }

    #[cfg(test)]
    pub(super) fn set_generation_for_test(&mut self, pass: ComposePass, generation: u64) {
        self.pass_mut(pass).set_generation_for_test(generation);
    }
}

fn snapshot_changed(snapshot: &mut Option<Vec<u32>>, values: &[f32]) -> bool {
    let Some(previous) = snapshot.as_mut() else {
        *snapshot = Some(values.iter().map(|value| value.to_bits()).collect());
        return false;
    };
    let changed = previous.len() != values.len()
        || previous
            .iter()
            .zip(values)
            .any(|(&bits, value)| bits != value.to_bits());
    previous.clear();
    previous.extend(values.iter().map(|value| value.to_bits()));
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeSet;

    fn list(rows: &[u32]) -> Vec<u32> {
        rows.to_vec()
    }

    fn controls() -> ComposeControlSnapshot {
        ComposeControlSnapshot {
            light_term_mask: LightTermMask::ALL,
            promotion_override: DirectShDebugOverride::default(),
            animated_override: AnimatedDirectShDebugOverride::default(),
        }
    }

    struct Fixture {
        gated: Vec<u32>,
        indirect_resident: Vec<u32>,
        indirect_contributing: Vec<u32>,
        static_resident: Vec<u32>,
        static_contributing: Vec<u32>,
        animated_resident: Vec<u32>,
        animated_contributing: Vec<u32>,
        static_weights: Vec<f32>,
        animated_weights: Vec<f32>,
        membership: Vec<(u32, RowMembership)>,
    }

    /// Row space for these fixtures; membership is re-observed for every row
    /// each frame, which the planner treats idempotently.
    const FIXTURE_ROWS: u32 = 64;

    fn planner() -> StreamedComposePlanner {
        StreamedComposePlanner::with_row_capacity(FIXTURE_ROWS as usize)
    }

    impl Fixture {
        fn all(rows: &[u32]) -> Self {
            Self {
                gated: rows.to_vec(),
                indirect_resident: rows.to_vec(),
                indirect_contributing: rows.to_vec(),
                static_resident: rows.to_vec(),
                static_contributing: rows.to_vec(),
                animated_resident: rows.to_vec(),
                animated_contributing: rows.to_vec(),
                static_weights: vec![0.0],
                animated_weights: vec![1.0],
                membership: Vec::new(),
            }
        }

        fn frame(
            &mut self,
            records_compose: bool,
            indirect: bool,
            animated: bool,
        ) -> ComposePlannerFrame<'_> {
            let has = |rows: &[u32], row: u32| rows.contains(&row);
            self.membership = (0..FIXTURE_ROWS)
                .map(|row| {
                    (
                        row,
                        RowMembership {
                            indirect: PassMembership {
                                resident: has(&self.indirect_resident, row),
                                contributing: has(&self.indirect_contributing, row),
                                upstream: false,
                            },
                            static_direct: PassMembership {
                                resident: has(&self.static_resident, row),
                                contributing: has(&self.static_contributing, row),
                                upstream: false,
                            },
                            animated_direct: PassMembership {
                                resident: has(&self.animated_resident, row),
                                contributing: has(&self.animated_contributing, row),
                                upstream: has(&self.static_contributing, row),
                            },
                        },
                    )
                })
                .collect();
            ComposePlannerFrame {
                records_compose,
                force_full_resident: false,
                gated_rows: &self.gated,
                membership_changes: &self.membership,
                indirect_active: indirect,
                animated_direct_active: animated,
                effective_static_weights: &self.static_weights,
                effective_animated_weights: &self.animated_weights,
                controls: controls(),
            }
        }
    }

    fn commit_all(planner: &mut StreamedComposePlanner, plan: &ComposeFramePlan) {
        for pass in plan.ordered() {
            planner.commit_pass(pass);
        }
    }

    #[test]
    fn trigger_selects_only_gated_contributing_rows_per_pass() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[1, 2, 3, 4]);
        fixture.gated = list(&[1, 2, 4]);
        fixture.indirect_contributing = list(&[2, 3]);
        fixture.animated_contributing = list(&[1, 3]);

        let plan = planner.plan_frame(fixture.frame(true, true, true));
        assert_eq!(plan.indirect.rows(), &[2]);
        assert!(plan.static_direct.rows().is_empty());
        assert_eq!(plan.animated_direct.rows(), &[1]);
    }

    #[test]
    fn idle_plan_contains_only_pending_residency_rows() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[1, 2, 3]);
        planner.mark_residency_rows(ComposePass::Indirect, [2]);
        planner.mark_residency_rows(ComposePass::StaticDirect, [3]);

        let plan = planner.plan_frame(fixture.frame(true, false, false));
        assert_eq!(plan.indirect.rows(), &[2]);
        assert_eq!(plan.static_direct.rows(), &[3]);
        assert_eq!(plan.animated_direct.rows(), &[3]);
    }

    #[test]
    fn effective_uploaded_promotion_weights_drive_static_trigger() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[4]);
        let initial = planner.plan_frame(fixture.frame(true, false, false));
        commit_all(&mut planner, &initial);

        // Requested promotion may have changed elsewhere, but the planner
        // sees only the post-cache-zeroed value actually uploaded.
        let unchanged = planner.plan_frame(fixture.frame(true, false, false));
        assert!(unchanged.static_direct.rows().is_empty());
        fixture.static_weights[0] = 0.5;
        let changed = planner.plan_frame(fixture.frame(true, false, false));
        assert_eq!(changed.static_direct.rows(), &[4]);
        assert_eq!(changed.animated_direct.rows(), &[4]);
    }

    #[test]
    fn effective_uploaded_animated_weights_drive_only_pass_b() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[4]);
        let initial = planner.plan_frame(fixture.frame(true, false, false));
        commit_all(&mut planner, &initial);

        fixture.animated_weights[0] = 0.5;
        let changed = planner.plan_frame(fixture.frame(true, false, false));
        assert!(changed.static_direct.rows().is_empty());
        assert_eq!(changed.animated_direct.rows(), &[4]);
    }

    #[test]
    fn direct_plan_splits_activity_and_follows_static_rewrite() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[7]);
        let initial = planner.plan_frame(fixture.frame(true, false, false));
        commit_all(&mut planner, &initial);

        let animated = planner.plan_frame(fixture.frame(true, false, true));
        assert!(animated.static_direct.rows().is_empty());
        assert_eq!(animated.animated_direct.rows(), &[7]);
        planner.commit_pass(&animated.animated_direct);

        fixture.static_weights[0] = 0.25;
        let promotion = planner.plan_frame(fixture.frame(true, false, true));
        assert_eq!(promotion.ordered()[1].pass(), ComposePass::StaticDirect);
        assert_eq!(promotion.ordered()[2].pass(), ComposePass::AnimatedDirect);
        assert_eq!(promotion.static_direct.rows(), &[7]);
        assert_eq!(promotion.animated_direct.rows(), &[7]);
    }

    #[test]
    fn deactivation_tail_waits_for_next_recorded_compose() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[9]);
        let active = planner.plan_frame(fixture.frame(true, true, true));
        commit_all(&mut planner, &active);

        let skipped = planner.plan_frame(fixture.frame(false, false, false));
        assert!(skipped.indirect.rows().is_empty());
        assert!(skipped.animated_direct.rows().is_empty());
        let tail = planner.plan_frame(fixture.frame(true, false, false));
        assert_eq!(tail.indirect.rows(), &[9]);
        assert_eq!(tail.animated_direct.rows(), &[9]);
        planner.commit_pass(&tail.indirect);
        planner.commit_pass(&tail.animated_direct);
        assert!(
            planner
                .plan_frame(fixture.frame(true, false, false))
                .indirect
                .rows()
                .is_empty()
        );
    }

    #[test]
    fn mask_or_override_change_forces_all_passes_in_order() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[1, 2]);
        let initial = planner.plan_frame(fixture.frame(true, false, false));
        commit_all(&mut planner, &initial);

        let mut frame = fixture.frame(true, false, false);
        frame.controls.promotion_override.enabled = true;
        let plan = planner.plan_frame(frame);
        assert_eq!(
            plan.ordered().map(ComposePassPlan::pass),
            [
                ComposePass::Indirect,
                ComposePass::StaticDirect,
                ComposePass::AnimatedDirect,
            ]
        );
        for pass in plan.ordered() {
            assert_eq!(pass.rows(), &[1, 2]);
        }
    }

    #[test]
    fn control_change_bypasses_partial_or_empty_gate_until_committed() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[1, 2, 3]);
        let initial = planner.plan_frame(fixture.frame(true, false, false));
        commit_all(&mut planner, &initial);

        fixture.gated = list(&[2]);
        let mut changed = fixture.frame(true, false, false);
        changed.controls.light_term_mask = LightTermMask::AMBIENT_FLOOR;
        let partial = planner.plan_frame(changed);
        for pass in partial.ordered() {
            assert_eq!(pass.rows(), &[1, 2, 3]);
        }

        fixture.gated.clear();
        let mut retry = fixture.frame(true, false, false);
        retry.controls.light_term_mask = LightTermMask::AMBIENT_FLOOR;
        let empty_gate_retry = planner.plan_frame(retry);
        for pass in empty_gate_retry.ordered() {
            assert_eq!(pass.rows(), &[1, 2, 3]);
        }
        commit_all(&mut planner, &empty_gate_retry);
        fixture.gated = list(&[1, 2, 3]);
        let mut clean_frame = fixture.frame(true, false, false);
        clean_frame.controls.light_term_mask = LightTermMask::AMBIENT_FLOOR;
        let clean = planner.plan_frame(clean_frame);
        assert!(clean.ordered().iter().all(|plan| plan.rows().is_empty()));
    }

    #[test]
    fn control_change_on_nonrecording_frame_repairs_full_resident_later() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[4, 5]);
        let initial = planner.plan_frame(fixture.frame(true, false, false));
        commit_all(&mut planner, &initial);
        fixture.gated.clear();

        let mut skipped = fixture.frame(false, false, false);
        skipped.controls.animated_override.enabled = true;
        assert!(
            planner
                .plan_frame(skipped)
                .ordered()
                .iter()
                .all(|plan| plan.rows().is_empty())
        );

        let mut recorded = fixture.frame(true, false, false);
        recorded.controls.animated_override.enabled = true;
        let repair = planner.plan_frame(recorded);
        for pass in repair.ordered() {
            assert_eq!(pass.rows(), &[4, 5]);
        }
    }

    #[test]
    fn force_full_resident_plans_all_resident_rows() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[1, 2, 3]);
        fixture.gated = list(&[2]);
        let initial = planner.plan_frame(fixture.frame(true, false, false));
        commit_all(&mut planner, &initial);

        let mut frame = fixture.frame(true, false, false);
        frame.force_full_resident = true;
        let forced = planner.plan_frame(frame);
        for pass in forced.ordered() {
            assert_eq!(pass.rows(), &[1, 2, 3]);
        }
        commit_all(&mut planner, &forced);

        let mut next = fixture.frame(true, false, false);
        next.force_full_resident = true;
        let forced_again = planner.plan_frame(next);
        for pass in forced_again.ordered() {
            assert_eq!(pass.rows(), &[1, 2, 3]);
        }
    }

    #[test]
    fn only_lagging_rows_compose_on_idle_reentry() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[1, 2]);
        fixture.gated = list(&[1]);
        let first = planner.plan_frame(fixture.frame(true, true, false));
        planner.commit_pass(&first.indirect);

        // Consume the deactivation tail for row 1 while row 2 remains out of
        // gate and therefore stale.
        let tail = planner.plan_frame(fixture.frame(true, false, false));
        planner.commit_pass(&tail.indirect);

        fixture.gated = list(&[2]);
        let reentry = planner.plan_frame(fixture.frame(true, false, false));
        assert_eq!(reentry.indirect.rows(), &[2]);
        planner.commit_pass(&reentry.indirect);

        fixture.gated = list(&[1, 2]);
        assert!(
            planner
                .plan_frame(fixture.frame(true, false, false))
                .indirect
                .rows()
                .is_empty()
        );
    }

    #[test]
    fn empty_contributing_gate_advances_generation_without_dispatch() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[5]);
        fixture.gated.clear();
        let empty = planner.plan_frame(fixture.frame(true, true, false));
        assert!(empty.indirect.rows().is_empty());
        assert_eq!(planner.lagging_rows(ComposePass::Indirect), 1);

        fixture.gated.push(5);
        assert_eq!(
            planner
                .plan_frame(fixture.frame(true, false, false))
                .indirect
                .rows(),
            &[5]
        );
    }

    #[test]
    fn deactivation_outside_gate_is_repaired_on_reentry() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[6]);
        let active = planner.plan_frame(fixture.frame(true, true, false));
        planner.commit_pass(&active.indirect);
        fixture.gated.clear();
        assert!(
            planner
                .plan_frame(fixture.frame(true, false, false))
                .indirect
                .rows()
                .is_empty()
        );
        fixture.gated.push(6);
        assert_eq!(
            planner
                .plan_frame(fixture.frame(true, false, false))
                .indirect
                .rows(),
            &[6]
        );
    }

    #[test]
    fn promotion_change_outside_gate_repairs_both_direct_passes() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[3]);
        let initial = planner.plan_frame(fixture.frame(true, false, false));
        commit_all(&mut planner, &initial);
        fixture.gated.clear();
        fixture.static_weights[0] = 0.75;
        let hidden = planner.plan_frame(fixture.frame(true, false, false));
        assert!(hidden.static_direct.rows().is_empty());
        fixture.gated.push(3);
        let reentry = planner.plan_frame(fixture.frame(true, false, false));
        assert_eq!(reentry.static_direct.rows(), &[3]);
        assert_eq!(reentry.animated_direct.rows(), &[3]);
    }

    #[test]
    fn camera_cut_rows_compose_before_sampling() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[11]);
        fixture.gated.clear();
        planner.plan_frame(fixture.frame(true, true, false));
        fixture.gated.push(11);
        let cut = planner.plan_frame(fixture.frame(true, false, false));
        assert_eq!(cut.indirect.rows(), &[11]);
    }

    #[test]
    fn eviction_and_generation_reset_preserve_lag_contract() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[1, 2]);
        fixture.gated.clear();
        planner.plan_frame(fixture.frame(true, true, false));
        fixture.indirect_resident.retain(|row| *row != 1);
        fixture.indirect_contributing.retain(|row| *row != 1);
        planner.mark_residency_rows(ComposePass::Indirect, [1, 2]);
        fixture.gated.push(2);
        let eviction = planner.plan_frame(fixture.frame(true, false, false));
        assert_eq!(eviction.indirect.rows(), &[2]);

        planner.reset_generation();
        assert_eq!(planner.lagging_rows(ComposePass::Indirect), 0);
        assert!(
            planner
                .plan_frame(fixture.frame(true, false, false))
                .indirect
                .rows()
                .is_empty()
        );
    }

    #[test]
    fn install_and_slot_reuse_bypass_gate_before_promotion() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[8]);
        fixture.gated.clear();
        planner.mark_residency_rows(ComposePass::Indirect, [8]);
        let install = planner.plan_frame(fixture.frame(true, false, false));
        assert_eq!(install.indirect.rows(), &[8]);
        planner.commit_pass(&install.indirect);
        fixture.gated.push(8);
        assert!(
            planner
                .plan_frame(fixture.frame(true, false, false))
                .indirect
                .rows()
                .is_empty()
        );
    }

    #[test]
    fn failed_encode_commits_no_compose_state() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[12]);
        planner.mark_residency_rows(ComposePass::Indirect, [12]);
        let failed = planner.plan_frame(fixture.frame(true, true, false));
        assert_eq!(failed.indirect.rows(), &[12]);
        // No commit: both pending residency and lag must retry even after the
        // activity trigger turns off.
        fixture.gated.clear();
        let retry = planner.plan_frame(fixture.frame(true, false, false));
        assert_eq!(retry.indirect.rows(), &[12]);
    }

    #[test]
    fn failed_pass_b_retries_without_rewriting_committed_pass_a() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[13]);
        planner.plan_frame(fixture.frame(true, false, false));
        fixture.static_weights[0] = 0.5;
        let first = planner.plan_frame(fixture.frame(true, false, false));
        planner.commit_pass(&first.static_direct);
        // Pass B fails and is deliberately not committed.

        let retry = planner.plan_frame(fixture.frame(true, false, false));
        assert!(retry.static_direct.rows().is_empty());
        assert_eq!(retry.animated_direct.rows(), &[13]);
    }

    #[test]
    fn failed_control_repair_pass_b_keeps_its_full_retry_only() {
        let mut planner = planner();
        let mut fixture = Fixture::all(&[13, 14]);
        let initial = planner.plan_frame(fixture.frame(true, false, false));
        commit_all(&mut planner, &initial);
        fixture.gated = list(&[13]);

        let mut changed = fixture.frame(true, false, false);
        changed.controls.promotion_override.enabled = true;
        let first = planner.plan_frame(changed);
        assert_eq!(first.static_direct.rows(), &[13, 14]);
        assert_eq!(first.animated_direct.rows(), &[13, 14]);
        planner.commit_pass(&first.static_direct);
        // Pass B fails and retains its independent full-repair latch.

        fixture.gated.clear();
        let mut retry_frame = fixture.frame(true, false, false);
        retry_frame.controls.promotion_override.enabled = true;
        let retry = planner.plan_frame(retry_frame);
        assert!(retry.static_direct.rows().is_empty());
        assert_eq!(retry.animated_direct.rows(), &[13, 14]);
    }

    #[test]
    fn compose_planner_reuses_warmed_pass_vectors() {
        let rows = (0..64).collect::<Vec<_>>();
        let mut fixture = Fixture::all(&rows);
        let mut planner = planner();
        let mut plan = ComposeFramePlan::default();
        let mut first = fixture.frame(true, true, true);
        first.force_full_resident = true;
        planner.plan_frame_into(first, &mut plan);
        let warmed = plan.capacities();

        for _ in 0..32 {
            commit_all(&mut planner, &plan);
            let mut frame = fixture.frame(true, true, true);
            frame.force_full_resident = true;
            planner.plan_frame_into(frame, &mut plan);
            assert_eq!(plan.capacities(), warmed);
        }
    }

    proptest! {
        #[test]
        fn recorded_compose_clears_all_gated_lag(
            frames in prop::collection::vec(
                (
                    any::<u16>(),
                    any::<u16>(),
                    any::<bool>(),
                    any::<bool>(),
                    any::<bool>(),
                    any::<u8>(),
                ),
                1..32,
            ),
        ) {
            let rows: Vec<u32> = (0..16).collect();
            let mut fixture = Fixture::all(&rows);
            let mut planner = planner();
            let mut previous_resident: BTreeSet<u32> =
                fixture.indirect_resident.iter().copied().collect();

            for (gate_bits, resident_bits, records, indirect_active, animated_active, weight) in frames {
                let resident: BTreeSet<u32> = rows
                    .iter()
                    .copied()
                    .filter(|row| resident_bits & (1u16 << row) != 0)
                    .collect();
                let newly_resident: Vec<_> = resident.difference(&previous_resident).copied().collect();
                for pass in [ComposePass::Indirect, ComposePass::StaticDirect, ComposePass::AnimatedDirect] {
                    planner.mark_residency_rows(pass, newly_resident.iter().copied());
                }
                previous_resident = resident.clone();
                fixture.indirect_resident = resident.iter().copied().collect();
                fixture.static_resident = resident.iter().copied().collect();
                fixture.animated_resident = resident.iter().copied().collect();
                fixture.indirect_contributing = resident.iter().copied().collect();
                fixture.static_contributing = resident.iter().copied().collect();
                fixture.animated_contributing = resident.iter().copied().collect();
                fixture.gated = rows
                    .iter()
                    .copied()
                    .filter(|row| gate_bits & (1u16 << row) != 0)
                    .collect();
                fixture.static_weights[0] = f32::from(weight) / 255.0;
                fixture.animated_weights[0] = 1.0 - fixture.static_weights[0];

                let plan = planner.plan_frame(fixture.frame(records, indirect_active, animated_active));
                if records {
                    for (pass, pass_plan) in [
                        ComposePass::Indirect,
                        ComposePass::StaticDirect,
                        ComposePass::AnimatedDirect,
                    ]
                    .into_iter()
                    .zip(plan.ordered())
                    {
                        let resident = match pass {
                            ComposePass::Indirect => fixture.indirect_resident.as_slice(),
                            ComposePass::StaticDirect => fixture.static_resident.as_slice(),
                            ComposePass::AnimatedDirect => fixture.animated_resident.as_slice(),
                        };
                        prop_assert!(pass_plan.rows().windows(2).all(|rows| rows[0] < rows[1]));
                        prop_assert!(pass_plan
                            .rows()
                            .iter()
                            .all(|row| resident.binary_search(row).is_ok()));
                    }
                    commit_all(&mut planner, &plan);
                    for pass in [ComposePass::Indirect, ComposePass::StaticDirect, ComposePass::AnimatedDirect] {
                        let resident = match pass {
                            ComposePass::Indirect => fixture.indirect_resident.as_slice(),
                            ComposePass::StaticDirect => fixture.static_resident.as_slice(),
                            ComposePass::AnimatedDirect => fixture.animated_resident.as_slice(),
                        };
                        let gated_resident: Vec<_> = fixture
                            .gated
                            .iter()
                            .copied()
                            .filter(|row| resident.binary_search(row).is_ok())
                            .collect();
                        let current = gated_resident.iter().all(|&row| {
                            planner.is_resident(pass, row) && !planner.is_stale(pass, row)
                        });
                        prop_assert!(current);
                    }
                }
            }
        }
    }
}
