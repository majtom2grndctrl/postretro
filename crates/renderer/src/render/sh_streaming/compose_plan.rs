//! GPU-free trigger, staleness, and retry planning for streamed SH compose.
//! See: context/lib/rendering_pipeline.md §4 "Sampled-row compose"

use std::collections::{BTreeMap, BTreeSet};

use postretro_render_cpu::frame_uniforms::LightTermMask;

use super::{AnimatedDirectShDebugOverride, DirectShDebugOverride};

/// The three streamed passes in their required encode order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ComposePass {
    Indirect,
    StaticDirect,
    AnimatedDirect,
}

/// Resident rows and the subset whose contents depend on a pass's inputs.
#[derive(Clone, Copy)]
pub(super) struct ComposePassRows<'a> {
    pub(super) resident: &'a BTreeSet<u32>,
    pub(super) contributing: &'a BTreeSet<u32>,
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
    pub(super) gated_rows: &'a BTreeSet<u32>,
    pub(super) indirect_rows: ComposePassRows<'a>,
    pub(super) static_direct_rows: ComposePassRows<'a>,
    pub(super) animated_direct_rows: ComposePassRows<'a>,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ComposePassPlan {
    pass: ComposePass,
    rows: Vec<u32>,
    required_generations: Vec<u64>,
    lagged_rows: usize,
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
    pub(super) fn test_plan(pass: ComposePass, rows: Vec<u32>, lagged_rows: usize) -> Self {
        Self {
            required_generations: vec![0; rows.len()],
            pass,
            rows,
            lagged_rows,
        }
    }
}

/// Current-frame work in encode order. Pass B is planned after Pass A and is
/// guaranteed to contain every row Pass A rewrites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ComposeFramePlan {
    pub(super) indirect: ComposePassPlan,
    pub(super) static_direct: ComposePassPlan,
    pub(super) animated_direct: ComposePassPlan,
}

impl ComposeFramePlan {
    fn empty() -> Self {
        Self {
            indirect: ComposePassPlan::empty(ComposePass::Indirect),
            static_direct: ComposePassPlan::empty(ComposePass::StaticDirect),
            animated_direct: ComposePassPlan::empty(ComposePass::AnimatedDirect),
        }
    }

    #[cfg(test)]
    pub(super) fn ordered(&self) -> [&ComposePassPlan; 3] {
        [&self.indirect, &self.static_direct, &self.animated_direct]
    }
}

impl ComposePassPlan {
    fn empty(pass: ComposePass) -> Self {
        Self {
            pass,
            rows: Vec::new(),
            required_generations: Vec::new(),
            lagged_rows: 0,
        }
    }
}

#[derive(Default)]
struct PassStaleness {
    next_generation: u64,
    required: BTreeMap<u32, u64>,
    composed: BTreeMap<u32, u64>,
    pending: BTreeSet<u32>,
}

impl PassStaleness {
    fn mark_changed<'a>(&mut self, rows: impl IntoIterator<Item = &'a u32>) {
        self.next_generation = self.next_generation.checked_add(1).unwrap_or_else(|| {
            // Generation overflow is practically unreachable, but rebasing
            // preserves the ordering contract instead of wrapping stale rows
            // into apparently-current ones.
            let lagging: BTreeSet<_> = self
                .required
                .iter()
                .filter_map(|(&row, &required)| {
                    (self.composed.get(&row).copied().unwrap_or(0) < required).then_some(row)
                })
                .collect();
            self.required.clear();
            self.composed.clear();
            for row in lagging {
                self.required.insert(row, 1);
            }
            1
        });
        for &row in rows {
            self.required.insert(row, self.next_generation);
        }
    }

    fn retain_resident(&mut self, resident: &BTreeSet<u32>) {
        self.required.retain(|row, _| resident.contains(row));
        self.composed.retain(|row, _| resident.contains(row));
        self.pending.retain(|row| resident.contains(row));
    }

    fn plan(
        &self,
        pass: ComposePass,
        resident: &BTreeSet<u32>,
        gated: &BTreeSet<u32>,
    ) -> ComposePassPlan {
        let mut rows: Vec<u32> = self.pending.intersection(resident).copied().collect();
        rows.extend(gated.intersection(resident).copied().filter(|row| {
            self.composed.get(row).copied().unwrap_or(0)
                < self.required.get(row).copied().unwrap_or(0)
        }));
        rows.sort_unstable();
        rows.dedup();
        let lagged_rows = rows
            .iter()
            .filter(|row| {
                self.composed.get(row).copied().unwrap_or(0)
                    < self.required.get(row).copied().unwrap_or(0)
            })
            .count();
        let required_generations = rows
            .iter()
            .map(|row| self.required.get(row).copied().unwrap_or(0))
            .collect();
        ComposePassPlan {
            pass,
            rows,
            required_generations,
            lagged_rows,
        }
    }

    fn commit(&mut self, plan: &ComposePassPlan) {
        for (&row, &generation) in plan.rows.iter().zip(&plan.required_generations) {
            self.composed.insert(row, generation);
            self.pending.remove(&row);
        }
    }

    fn lagging_count(&self, resident: &BTreeSet<u32>) -> usize {
        self.required
            .iter()
            .filter(|(row, required)| {
                resident.contains(row) && self.composed.get(row).copied().unwrap_or(0) < **required
            })
            .count()
    }
}

/// Persistent planner state. Observing a frame may make rows stale; only
/// `commit_pass` makes planned rows current or consumes pending residency work.
#[derive(Default)]
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
    pub(super) fn reset_generation(&mut self) {
        *self = Self::default();
    }

    pub(super) fn mark_residency_rows(
        &mut self,
        pass: ComposePass,
        rows: impl IntoIterator<Item = u32>,
    ) {
        self.pass_mut(pass).pending.extend(rows);
    }

    pub(super) fn plan_frame(&mut self, frame: ComposePlannerFrame<'_>) -> ComposeFramePlan {
        self.indirect.retain_resident(frame.indirect_rows.resident);
        self.static_direct
            .retain_resident(frame.static_direct_rows.resident);
        self.animated_direct
            .retain_resident(frame.animated_direct_rows.resident);

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

        let indirect_changed =
            frame.indirect_active || self.indirect_was_active || controls_changed;
        let static_changed = static_weights_changed || controls_changed;
        let animated_changed = frame.animated_direct_active
            || self.animated_direct_was_active
            || animated_weights_changed
            || static_weights_changed
            || controls_changed;

        if controls_changed {
            self.indirect
                .mark_changed(frame.indirect_rows.resident.iter());
            self.static_direct
                .mark_changed(frame.static_direct_rows.resident.iter());
            self.animated_direct
                .mark_changed(frame.animated_direct_rows.resident.iter());
        } else {
            if indirect_changed {
                self.indirect
                    .mark_changed(frame.indirect_rows.contributing.iter());
            }
            if static_changed {
                self.static_direct
                    .mark_changed(frame.static_direct_rows.contributing.iter());
            }
            if animated_changed {
                self.animated_direct
                    .mark_changed(frame.animated_direct_rows.contributing.iter());
            }
            // Pass B reads Pass A's intermediate. Even a row with no id-45
            // contribution becomes stale when its promotion term changes.
            if static_weights_changed {
                self.animated_direct
                    .mark_changed(frame.static_direct_rows.contributing.iter());
            }
        }

        if frame.force_full_resident {
            self.indirect
                .mark_changed(frame.indirect_rows.resident.iter());
            self.static_direct
                .mark_changed(frame.static_direct_rows.resident.iter());
            self.animated_direct
                .mark_changed(frame.animated_direct_rows.resident.iter());
        }

        self.indirect_was_active = frame.indirect_active;
        self.animated_direct_was_active = frame.animated_direct_active;

        if !frame.records_compose {
            return ComposeFramePlan::empty();
        }

        let indirect = self.indirect.plan(
            ComposePass::Indirect,
            frame.indirect_rows.resident,
            if frame.force_full_resident {
                frame.indirect_rows.resident
            } else {
                frame.gated_rows
            },
        );
        let static_direct = self.static_direct.plan(
            ComposePass::StaticDirect,
            frame.static_direct_rows.resident,
            if frame.force_full_resident {
                frame.static_direct_rows.resident
            } else {
                frame.gated_rows
            },
        );
        // Planning Pass A creates durable Pass-B retry work before encoding.
        self.animated_direct
            .pending
            .extend(static_direct.rows.iter().copied());
        let animated_direct = self.animated_direct.plan(
            ComposePass::AnimatedDirect,
            frame.animated_direct_rows.resident,
            if frame.force_full_resident {
                frame.animated_direct_rows.resident
            } else {
                frame.gated_rows
            },
        );

        ComposeFramePlan {
            indirect,
            static_direct,
            animated_direct,
        }
    }

    pub(super) fn commit_pass(&mut self, plan: &ComposePassPlan) {
        self.pass_mut(plan.pass).commit(plan);
    }

    pub(super) fn lagging_rows(&self, pass: ComposePass, resident: &BTreeSet<u32>) -> usize {
        self.pass(pass).lagging_count(resident)
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

    fn set(rows: &[u32]) -> BTreeSet<u32> {
        rows.iter().copied().collect()
    }

    fn controls() -> ComposeControlSnapshot {
        ComposeControlSnapshot {
            light_term_mask: LightTermMask::ALL,
            promotion_override: DirectShDebugOverride::default(),
            animated_override: AnimatedDirectShDebugOverride::default(),
        }
    }

    struct Fixture {
        gated: BTreeSet<u32>,
        indirect_resident: BTreeSet<u32>,
        indirect_contributing: BTreeSet<u32>,
        static_resident: BTreeSet<u32>,
        static_contributing: BTreeSet<u32>,
        animated_resident: BTreeSet<u32>,
        animated_contributing: BTreeSet<u32>,
        static_weights: Vec<f32>,
        animated_weights: Vec<f32>,
    }

    impl Fixture {
        fn all(rows: &[u32]) -> Self {
            let all = set(rows);
            Self {
                gated: all.clone(),
                indirect_resident: all.clone(),
                indirect_contributing: all.clone(),
                static_resident: all.clone(),
                static_contributing: all.clone(),
                animated_resident: all.clone(),
                animated_contributing: all,
                static_weights: vec![0.0],
                animated_weights: vec![1.0],
            }
        }

        fn frame(
            &self,
            records_compose: bool,
            indirect: bool,
            animated: bool,
        ) -> ComposePlannerFrame<'_> {
            ComposePlannerFrame {
                records_compose,
                force_full_resident: false,
                gated_rows: &self.gated,
                indirect_rows: ComposePassRows {
                    resident: &self.indirect_resident,
                    contributing: &self.indirect_contributing,
                },
                static_direct_rows: ComposePassRows {
                    resident: &self.static_resident,
                    contributing: &self.static_contributing,
                },
                animated_direct_rows: ComposePassRows {
                    resident: &self.animated_resident,
                    contributing: &self.animated_contributing,
                },
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
        let mut planner = StreamedComposePlanner::default();
        let mut fixture = Fixture::all(&[1, 2, 3, 4]);
        fixture.gated = set(&[1, 2, 4]);
        fixture.indirect_contributing = set(&[2, 3]);
        fixture.animated_contributing = set(&[1, 3]);

        let plan = planner.plan_frame(fixture.frame(true, true, true));
        assert_eq!(plan.indirect.rows(), &[2]);
        assert!(plan.static_direct.rows().is_empty());
        assert_eq!(plan.animated_direct.rows(), &[1]);
    }

    #[test]
    fn idle_plan_contains_only_pending_residency_rows() {
        let mut planner = StreamedComposePlanner::default();
        let fixture = Fixture::all(&[1, 2, 3]);
        planner.mark_residency_rows(ComposePass::Indirect, [2]);
        planner.mark_residency_rows(ComposePass::StaticDirect, [3]);

        let plan = planner.plan_frame(fixture.frame(true, false, false));
        assert_eq!(plan.indirect.rows(), &[2]);
        assert_eq!(plan.static_direct.rows(), &[3]);
        assert_eq!(plan.animated_direct.rows(), &[3]);
    }

    #[test]
    fn effective_uploaded_promotion_weights_drive_static_trigger() {
        let mut planner = StreamedComposePlanner::default();
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
        let mut planner = StreamedComposePlanner::default();
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
        let mut planner = StreamedComposePlanner::default();
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
        let mut planner = StreamedComposePlanner::default();
        let fixture = Fixture::all(&[9]);
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
        let mut planner = StreamedComposePlanner::default();
        let fixture = Fixture::all(&[1, 2]);
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
    fn force_full_resident_plans_all_resident_rows() {
        let mut planner = StreamedComposePlanner::default();
        let mut fixture = Fixture::all(&[1, 2, 3]);
        fixture.gated = set(&[2]);
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
        let mut planner = StreamedComposePlanner::default();
        let mut fixture = Fixture::all(&[1, 2]);
        fixture.gated = set(&[1]);
        let first = planner.plan_frame(fixture.frame(true, true, false));
        planner.commit_pass(&first.indirect);

        // Consume the deactivation tail for row 1 while row 2 remains out of
        // gate and therefore stale.
        let tail = planner.plan_frame(fixture.frame(true, false, false));
        planner.commit_pass(&tail.indirect);

        fixture.gated = set(&[2]);
        let reentry = planner.plan_frame(fixture.frame(true, false, false));
        assert_eq!(reentry.indirect.rows(), &[2]);
        planner.commit_pass(&reentry.indirect);

        fixture.gated = set(&[1, 2]);
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
        let mut planner = StreamedComposePlanner::default();
        let mut fixture = Fixture::all(&[5]);
        fixture.gated.clear();
        assert!(
            planner
                .plan_frame(fixture.frame(true, true, false))
                .indirect
                .rows()
                .is_empty()
        );

        fixture.gated.insert(5);
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
        let mut planner = StreamedComposePlanner::default();
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
        fixture.gated.insert(6);
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
        let mut planner = StreamedComposePlanner::default();
        let mut fixture = Fixture::all(&[3]);
        let initial = planner.plan_frame(fixture.frame(true, false, false));
        commit_all(&mut planner, &initial);
        fixture.gated.clear();
        fixture.static_weights[0] = 0.75;
        let hidden = planner.plan_frame(fixture.frame(true, false, false));
        assert!(hidden.static_direct.rows().is_empty());
        fixture.gated.insert(3);
        let reentry = planner.plan_frame(fixture.frame(true, false, false));
        assert_eq!(reentry.static_direct.rows(), &[3]);
        assert_eq!(reentry.animated_direct.rows(), &[3]);
    }

    #[test]
    fn camera_cut_rows_compose_before_sampling() {
        let mut planner = StreamedComposePlanner::default();
        let mut fixture = Fixture::all(&[11]);
        fixture.gated.clear();
        planner.plan_frame(fixture.frame(true, true, false));
        fixture.gated.insert(11);
        let cut = planner.plan_frame(fixture.frame(true, false, false));
        assert_eq!(cut.indirect.rows(), &[11]);
    }

    #[test]
    fn eviction_and_generation_reset_preserve_lag_contract() {
        let mut planner = StreamedComposePlanner::default();
        let mut fixture = Fixture::all(&[1, 2]);
        fixture.gated.clear();
        planner.plan_frame(fixture.frame(true, true, false));
        fixture.indirect_resident.remove(&1);
        fixture.indirect_contributing.remove(&1);
        planner.mark_residency_rows(ComposePass::Indirect, [1, 2]);
        fixture.gated.insert(2);
        let eviction = planner.plan_frame(fixture.frame(true, false, false));
        assert_eq!(eviction.indirect.rows(), &[2]);

        planner.reset_generation();
        assert_eq!(
            planner.lagging_rows(ComposePass::Indirect, &fixture.indirect_resident),
            0
        );
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
        let mut planner = StreamedComposePlanner::default();
        let mut fixture = Fixture::all(&[8]);
        fixture.gated.clear();
        planner.mark_residency_rows(ComposePass::Indirect, [8]);
        let install = planner.plan_frame(fixture.frame(true, false, false));
        assert_eq!(install.indirect.rows(), &[8]);
        planner.commit_pass(&install.indirect);
        fixture.gated.insert(8);
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
        let mut planner = StreamedComposePlanner::default();
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
        let mut planner = StreamedComposePlanner::default();
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
            let mut planner = StreamedComposePlanner::default();
            let mut previous_resident = fixture.indirect_resident.clone();

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
                fixture.indirect_resident = resident.clone();
                fixture.static_resident = resident.clone();
                fixture.animated_resident = resident.clone();
                fixture.indirect_contributing = resident.clone();
                fixture.static_contributing = resident.clone();
                fixture.animated_contributing = resident;
                fixture.gated = rows
                    .iter()
                    .copied()
                    .filter(|row| gate_bits & (1u16 << row) != 0)
                    .collect();
                fixture.static_weights[0] = f32::from(weight) / 255.0;
                fixture.animated_weights[0] = 1.0 - fixture.static_weights[0];

                let plan = planner.plan_frame(fixture.frame(records, indirect_active, animated_active));
                if records {
                    commit_all(&mut planner, &plan);
                    for pass in [ComposePass::Indirect, ComposePass::StaticDirect, ComposePass::AnimatedDirect] {
                        let resident = match pass {
                            ComposePass::Indirect => &fixture.indirect_resident,
                            ComposePass::StaticDirect => &fixture.static_resident,
                            ComposePass::AnimatedDirect => &fixture.animated_resident,
                        };
                        let gated_resident: BTreeSet<_> = fixture.gated.intersection(resident).copied().collect();
                        prop_assert_eq!(planner.pass(pass).lagging_count(&gated_resident), 0);
                    }
                }
            }
        }
    }
}
