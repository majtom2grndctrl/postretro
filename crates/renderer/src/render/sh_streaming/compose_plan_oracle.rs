//! Test oracle: the previous push-stamping compose planner, plus equivalence
//! and scaling tests against the pull planner. Two deliberate changes from the
//! original. An empty full-repair scan clears the repair latch; the original
//! cleared it only on commit, and the renderer never commits an empty plan.
//! Pass A queues Pass-B retry work only for rows Pass B holds; the original
//! queued every Pass-A row, which on a level without id-45 only churned.

use std::collections::{BTreeMap, BTreeSet};

use super::compose_plan::{
    ComposeControlSnapshot, ComposeFramePlan, ComposePass, ComposePlannerFrame, PassMembership,
    RowMembership, StreamedComposePlanner,
};

/// Resident rows and the subset whose contents depend on a pass's inputs.
#[derive(Clone, Copy)]
struct OraclePassRows<'a> {
    resident: &'a [u32],
    contributing: &'a [u32],
}

struct OracleFrame<'a> {
    records_compose: bool,
    force_full_resident: bool,
    gated_rows: &'a [u32],
    indirect_rows: OraclePassRows<'a>,
    static_direct_rows: OraclePassRows<'a>,
    animated_direct_rows: OraclePassRows<'a>,
    indirect_active: bool,
    animated_direct_active: bool,
    effective_static_weights: &'a [f32],
    effective_animated_weights: &'a [f32],
    controls: ComposeControlSnapshot,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct OraclePassPlan {
    rows: Vec<u32>,
    required_generations: Vec<u64>,
    lagged_rows: usize,
    full_repair: bool,
}

#[derive(Default)]
struct OracleStaleness {
    next_generation: u64,
    required: BTreeMap<u32, u64>,
    composed: BTreeMap<u32, u64>,
    pending: BTreeSet<u32>,
    full_repair_pending: bool,
}

impl OracleStaleness {
    fn mark_changed<'a>(&mut self, rows: impl IntoIterator<Item = &'a u32>) {
        self.next_generation = self.next_generation.checked_add(1).unwrap_or_else(|| {
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

    fn retain_resident(&mut self, resident: &[u32]) {
        self.required
            .retain(|row, _| resident.binary_search(row).is_ok());
        self.composed
            .retain(|row, _| resident.binary_search(row).is_ok());
        self.pending
            .retain(|row| resident.binary_search(row).is_ok());
    }

    fn stale(&self, row: u32) -> bool {
        self.composed.get(&row).copied().unwrap_or(0)
            < self.required.get(&row).copied().unwrap_or(0)
    }

    fn plan_into(
        &mut self,
        resident: &[u32],
        gated: &[u32],
        force_full_resident: bool,
        plan: &mut OraclePassPlan,
    ) {
        *plan = OraclePassPlan {
            full_repair: self.full_repair_pending,
            ..OraclePassPlan::default()
        };
        plan.rows.extend(
            self.pending
                .iter()
                .copied()
                .filter(|row| resident.binary_search(row).is_ok()),
        );
        if force_full_resident || self.full_repair_pending {
            let planned = plan.rows.len();
            plan.rows
                .extend(resident.iter().copied().filter(|&row| self.stale(row)));
            if plan.rows.len() == planned {
                self.full_repair_pending = false;
            }
        } else {
            plan.rows.extend(
                gated
                    .iter()
                    .copied()
                    .filter(|&row| resident.binary_search(&row).is_ok() && self.stale(row)),
            );
        }
        plan.rows.sort_unstable();
        plan.rows.dedup();
        plan.lagged_rows = plan.rows.iter().filter(|&&row| self.stale(row)).count();
        plan.required_generations.extend(
            plan.rows
                .iter()
                .map(|row| self.required.get(row).copied().unwrap_or(0)),
        );
    }

    fn commit(&mut self, plan: &OraclePassPlan) {
        for (&row, &generation) in plan.rows.iter().zip(&plan.required_generations) {
            self.composed.insert(row, generation);
            self.pending.remove(&row);
        }
        if plan.full_repair {
            self.full_repair_pending = false;
        }
    }

    fn lagging_count(&self, resident: &[u32]) -> usize {
        self.required
            .iter()
            .filter(|(row, required)| {
                resident.binary_search(row).is_ok()
                    && self.composed.get(row).copied().unwrap_or(0) < **required
            })
            .count()
    }
}

#[derive(Default)]
struct OraclePlanner {
    passes: [OracleStaleness; 3],
    indirect_was_active: bool,
    animated_direct_was_active: bool,
    static_weight_bits: Option<Vec<u32>>,
    animated_weight_bits: Option<Vec<u32>>,
    controls: Option<ComposeControlSnapshot>,
}

impl OraclePlanner {
    fn plan_frame(&mut self, frame: OracleFrame<'_>) -> [OraclePassPlan; 3] {
        let mut plans: [OraclePassPlan; 3] = Default::default();
        let [indirect, static_direct, animated_direct] = &mut self.passes;
        indirect.retain_resident(frame.indirect_rows.resident);
        static_direct.retain_resident(frame.static_direct_rows.resident);
        animated_direct.retain_resident(frame.animated_direct_rows.resident);

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
            indirect.full_repair_pending = true;
            static_direct.full_repair_pending = true;
            animated_direct.full_repair_pending = true;
            indirect.mark_changed(frame.indirect_rows.resident.iter());
            static_direct.mark_changed(frame.static_direct_rows.resident.iter());
            animated_direct.mark_changed(frame.animated_direct_rows.resident.iter());
        } else {
            if indirect_changed {
                indirect.mark_changed(frame.indirect_rows.contributing.iter());
            }
            if static_changed {
                static_direct.mark_changed(frame.static_direct_rows.contributing.iter());
            }
            if animated_changed {
                animated_direct.mark_changed(frame.animated_direct_rows.contributing.iter());
            }
            if static_weights_changed {
                animated_direct.mark_changed(frame.static_direct_rows.contributing.iter());
            }
        }
        if frame.force_full_resident {
            indirect.mark_changed(frame.indirect_rows.resident.iter());
            static_direct.mark_changed(frame.static_direct_rows.resident.iter());
            animated_direct.mark_changed(frame.animated_direct_rows.resident.iter());
        }
        self.indirect_was_active = frame.indirect_active;
        self.animated_direct_was_active = frame.animated_direct_active;
        if !frame.records_compose {
            return plans;
        }
        indirect.plan_into(
            frame.indirect_rows.resident,
            frame.gated_rows,
            frame.force_full_resident,
            &mut plans[0],
        );
        static_direct.plan_into(
            frame.static_direct_rows.resident,
            frame.gated_rows,
            frame.force_full_resident,
            &mut plans[1],
        );
        animated_direct
            .pending
            .extend(plans[1].rows.iter().copied().filter(|row| {
                frame
                    .animated_direct_rows
                    .resident
                    .binary_search(row)
                    .is_ok()
            }));
        animated_direct.plan_into(
            frame.animated_direct_rows.resident,
            frame.gated_rows,
            frame.force_full_resident,
            &mut plans[2],
        );
        plans
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

mod tests {
    use super::*;
    use crate::render::sh_streaming::{AnimatedDirectShDebugOverride, DirectShDebugOverride};
    use postretro_render_cpu::frame_uniforms::LightTermMask;

    const PASSES: [ComposePass; 3] = [
        ComposePass::Indirect,
        ComposePass::StaticDirect,
        ComposePass::AnimatedDirect,
    ];

    fn controls() -> ComposeControlSnapshot {
        ComposeControlSnapshot {
            light_term_mask: LightTermMask::ALL,
            promotion_override: DirectShDebugOverride::default(),
            animated_override: AnimatedDirectShDebugOverride::default(),
        }
    }

    /// Per-pass membership of one row, in the shape both planners consume.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    struct ModelRow {
        indirect_resident: bool,
        indirect_contributing: bool,
        static_resident: bool,
        static_contributing: bool,
        animated_resident: bool,
        animated_contributing: bool,
    }

    impl ModelRow {
        fn membership(self) -> RowMembership {
            RowMembership {
                indirect: PassMembership {
                    resident: self.indirect_resident,
                    contributing: self.indirect_contributing,
                    upstream: false,
                },
                static_direct: PassMembership {
                    resident: self.static_resident,
                    contributing: self.static_contributing,
                    upstream: false,
                },
                animated_direct: PassMembership {
                    resident: self.animated_resident,
                    contributing: self.animated_contributing,
                    upstream: self.static_contributing,
                },
            }
        }

        /// The residency invariant the renderer maintains: each pass's
        /// contributing rows are resident for that pass, and Pass B is resident
        /// wherever Pass A is (when the level has id-45 data).
        fn installed(indirect: bool, static_direct: bool, animated: bool) -> Self {
            Self {
                indirect_resident: true,
                indirect_contributing: indirect,
                static_resident: true,
                static_contributing: static_direct,
                animated_resident: true,
                animated_contributing: animated,
            }
        }
    }

    /// Scripted world driving both planners with identical inputs. Membership
    /// is delivered to the pull planner only for rows touched since its last
    /// plan, exactly as the renderer's touched-row list does.
    struct Harness {
        rows: Vec<ModelRow>,
        touched: Vec<u32>,
        gated: Vec<u32>,
        static_weights: Vec<f32>,
        animated_weights: Vec<f32>,
        controls: ComposeControlSnapshot,
        pull: StreamedComposePlanner,
        oracle: OraclePlanner,
        plan: ComposeFramePlan,
        frame_index: usize,
    }

    #[derive(Clone, Copy, Default)]
    struct Step {
        records: bool,
        force_full: bool,
        indirect_active: bool,
        animated_active: bool,
    }

    impl Step {
        fn recorded() -> Self {
            Self {
                records: true,
                ..Self::default()
            }
        }
        fn indirect(mut self, active: bool) -> Self {
            self.indirect_active = active;
            self
        }
        fn animated(mut self, active: bool) -> Self {
            self.animated_active = active;
            self
        }
        fn forced(mut self) -> Self {
            self.force_full = true;
            self
        }
        fn skipped(mut self) -> Self {
            self.records = false;
            self
        }
    }

    /// Which passes a scripted frame commits after planning. Empty plans are
    /// never committed.
    #[derive(Clone, Copy)]
    enum Commit {
        All,
        None,
        Only(&'static [usize]),
    }

    impl Harness {
        fn new(row_count: usize) -> Self {
            Self {
                rows: vec![ModelRow::default(); row_count],
                touched: Vec::new(),
                gated: Vec::new(),
                static_weights: vec![0.0],
                animated_weights: vec![1.0],
                controls: controls(),
                pull: StreamedComposePlanner::with_row_capacity(row_count),
                oracle: OraclePlanner::default(),
                plan: ComposeFramePlan::default(),
                frame_index: 0,
            }
        }

        fn set_row(&mut self, row: u32, value: ModelRow) {
            self.rows[row as usize] = value;
            self.touched.push(row);
        }

        fn evict(&mut self, row: u32) {
            self.set_row(row, ModelRow::default());
        }

        /// An install marks the row pending for every pass, as the renderer's
        /// dirty-row sets do.
        fn install(&mut self, row: u32, value: ModelRow) {
            self.set_row(row, value);
            self.mark_pending(row, &[0, 1, 2]);
        }

        fn mark_pending(&mut self, row: u32, passes: &[usize]) {
            for &pass in passes {
                self.pull.mark_residency_rows(PASSES[pass], [row]);
                self.oracle.passes[pass].pending.insert(row);
            }
        }

        fn gate(&mut self, rows: &[u32]) {
            self.gated = rows.to_vec();
        }

        fn lists(&self) -> [Vec<u32>; 6] {
            let collect = |predicate: fn(&ModelRow) -> bool| -> Vec<u32> {
                self.rows
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| predicate(row))
                    .map(|(index, _)| index as u32)
                    .collect()
            };
            [
                collect(|row| row.indirect_resident),
                collect(|row| row.indirect_contributing),
                collect(|row| row.static_resident),
                collect(|row| row.static_contributing),
                collect(|row| row.animated_resident),
                collect(|row| row.animated_contributing),
            ]
        }

        fn step(&mut self, step: Step, commit: Commit) {
            let [ir, ic, sr, sc, ar, ac] = self.lists();
            let oracle_plans = self.oracle.plan_frame(OracleFrame {
                records_compose: step.records,
                force_full_resident: step.force_full,
                gated_rows: &self.gated,
                indirect_rows: OraclePassRows {
                    resident: &ir,
                    contributing: &ic,
                },
                static_direct_rows: OraclePassRows {
                    resident: &sr,
                    contributing: &sc,
                },
                animated_direct_rows: OraclePassRows {
                    resident: &ar,
                    contributing: &ac,
                },
                indirect_active: step.indirect_active,
                animated_direct_active: step.animated_active,
                effective_static_weights: &self.static_weights,
                effective_animated_weights: &self.animated_weights,
                controls: self.controls,
            });

            let changes: Vec<(u32, RowMembership)> = self
                .touched
                .drain(..)
                .map(|row| (row, self.rows[row as usize].membership()))
                .collect();
            self.pull.plan_frame_into(
                ComposePlannerFrame {
                    records_compose: step.records,
                    force_full_resident: step.force_full,
                    gated_rows: &self.gated,
                    membership_changes: &changes,
                    indirect_active: step.indirect_active,
                    animated_direct_active: step.animated_active,
                    effective_static_weights: &self.static_weights,
                    effective_animated_weights: &self.animated_weights,
                    controls: self.controls,
                },
                &mut self.plan,
            );

            let residents = [&ir, &sr, &ar];
            for (index, pull_plan) in self.plan.ordered().into_iter().enumerate() {
                let oracle_plan = &oracle_plans[index];
                assert_eq!(
                    pull_plan.rows(),
                    oracle_plan.rows.as_slice(),
                    "frame {} pass {:?} planned rows",
                    self.frame_index,
                    PASSES[index]
                );
                assert_eq!(
                    pull_plan.full_repair(),
                    oracle_plan.full_repair,
                    "frame {} pass {:?} full repair",
                    self.frame_index,
                    PASSES[index]
                );
                assert_eq!(
                    pull_plan.lagged_rows(),
                    oracle_plan.lagged_rows,
                    "frame {} pass {:?} lagged rows",
                    self.frame_index,
                    PASSES[index]
                );
            }
            self.assert_lag(&residents, "after plan");

            let committed: &[usize] = match commit {
                Commit::All => &[0, 1, 2],
                Commit::None => &[],
                Commit::Only(passes) => passes,
            };
            for &pass in committed {
                // The renderer encodes, and therefore commits, only non-empty
                // plans.
                if oracle_plans[pass].rows.is_empty() {
                    continue;
                }
                self.pull.commit_pass(self.plan.ordered()[pass]);
                self.oracle.passes[pass].commit(&oracle_plans[pass]);
            }
            self.assert_lag(&residents, "after commit");
            self.frame_index += 1;
        }

        fn assert_lag(&self, residents: &[&Vec<u32>; 3], phase: &str) {
            for (index, pass) in PASSES.into_iter().enumerate() {
                assert_eq!(
                    self.pull.lagging_rows(pass),
                    self.oracle.passes[index].lagging_count(residents[index]),
                    "frame {} pass {pass:?} lag {phase}",
                    self.frame_index
                );
            }
        }

        /// A session clear, as the renderer performs it: residency, pending
        /// work, and planner history all drop together.
        fn clear_session(&mut self) {
            self.rows.fill(ModelRow::default());
            self.touched.clear();
            self.pull.reset_generation();
            self.pull.clear_membership();
            self.oracle = OraclePlanner::default();
        }

        fn set_generation(&mut self, generation: u64) {
            for (index, pass) in PASSES.into_iter().enumerate() {
                self.pull.set_generation_for_test(pass, generation);
                let oracle = &mut self.oracle.passes[index];
                // Preserve the oracle's staleness relation while moving its
                // counter: shift every stamp by the same offset.
                let offset = generation.saturating_sub(oracle.next_generation);
                oracle.next_generation += offset;
                for value in oracle.required.values_mut() {
                    *value += offset;
                }
                for value in oracle.composed.values_mut() {
                    if *value > 0 {
                        *value += offset;
                    }
                }
            }
        }
    }

    fn all_installed(harness: &mut Harness, rows: &[u32]) {
        for &row in rows {
            harness.install(row, ModelRow::installed(true, true, true));
        }
    }

    #[test]
    fn equivalence_install_evict_and_slot_reuse() {
        let mut h = Harness::new(16);
        all_installed(&mut h, &[1, 2, 3, 4]);
        h.gate(&[1, 2]);
        h.step(Step::recorded(), Commit::All);
        // Idle frame: nothing stale, nothing pending.
        h.step(Step::recorded(), Commit::All);

        // Evict row 3, reinstall into the same row in a later frame.
        h.evict(3);
        h.mark_pending(2, &[0]);
        h.step(Step::recorded(), Commit::All);
        h.install(3, ModelRow::installed(true, false, true));
        h.step(Step::recorded(), Commit::All);

        // Evict and reinstall between two plans (slot reuse into the same row).
        h.evict(4);
        h.install(4, ModelRow::installed(false, true, false));
        h.step(Step::recorded(), Commit::All);

        // A partial eviction: row keeps base residency, loses its id-27 data.
        h.set_row(1, ModelRow::installed(false, true, true));
        h.mark_pending(1, &[0]);
        h.step(Step::recorded().indirect(true), Commit::All);
        h.step(Step::recorded(), Commit::All);
    }

    #[test]
    fn equivalence_animation_start_stop_and_tail() {
        let mut h = Harness::new(16);
        all_installed(&mut h, &[0, 1, 2, 3, 4, 5]);
        h.gate(&[0, 1, 2]);
        h.step(Step::recorded(), Commit::All);
        for _ in 0..3 {
            h.step(Step::recorded().indirect(true).animated(true), Commit::All);
        }
        // Stop: the one-frame tail fires on the next frame only.
        h.step(Step::recorded(), Commit::All);
        h.step(Step::recorded(), Commit::All);
        // Tail on a non-recording frame waits for the next recorded one.
        h.step(Step::recorded().animated(true), Commit::All);
        h.step(Step::recorded().skipped(), Commit::None);
        h.step(Step::recorded(), Commit::All);
        // Stale rows outside the gate come into view later.
        h.gate(&[3, 4, 5]);
        h.step(Step::recorded(), Commit::All);
        h.gate(&[0, 1, 2, 3, 4, 5]);
        h.step(Step::recorded(), Commit::All);
    }

    #[test]
    fn equivalence_promotion_ramp_and_cross_pass() {
        let mut h = Harness::new(16);
        h.install(0, ModelRow::installed(false, true, false));
        h.install(1, ModelRow::installed(false, false, true));
        h.install(2, ModelRow::installed(true, true, true));
        h.install(3, ModelRow::installed(false, true, true));
        h.gate(&[0, 1, 2]);
        h.step(Step::recorded(), Commit::All);
        for step in 1..=5 {
            h.static_weights[0] = step as f32 * 0.2;
            h.animated_weights[0] = 1.0 - h.static_weights[0];
            h.step(Step::recorded(), Commit::All);
        }
        // Two changes in one frame: static weights and animation start.
        h.static_weights[0] = 0.1;
        h.step(Step::recorded().animated(true).indirect(true), Commit::All);
        // Pass B fails while Pass A commits; Pass B retries alone.
        h.static_weights[0] = 0.3;
        h.step(Step::recorded(), Commit::Only(&[0, 1]));
        h.step(Step::recorded(), Commit::All);
        h.gate(&[3]);
        h.step(Step::recorded(), Commit::All);
    }

    #[test]
    fn equivalence_control_change_force_full_and_zero_gate() {
        let mut h = Harness::new(16);
        all_installed(&mut h, &[2, 5, 7, 9]);
        h.gate(&[]);
        h.step(Step::recorded(), Commit::All);
        // Zero gated rows: only pending work plans.
        h.step(Step::recorded().indirect(true).animated(true), Commit::All);
        h.controls.promotion_override.enabled = true;
        h.step(Step::recorded().skipped(), Commit::None);
        h.step(Step::recorded(), Commit::Only(&[0, 1]));
        h.step(Step::recorded(), Commit::All);
        h.controls.light_term_mask = LightTermMask::AMBIENT_FLOOR;
        h.gate(&[5]);
        h.step(Step::recorded().indirect(true), Commit::All);
        h.step(Step::recorded().forced(), Commit::All);
        h.step(Step::recorded().forced().animated(true), Commit::Only(&[2]));
        h.step(Step::recorded(), Commit::All);
    }

    #[test]
    fn equivalence_gate_sides_and_reset() {
        let mut h = Harness::new(16);
        all_installed(&mut h, &[1, 2, 3]);
        h.gate(&[1]);
        h.step(Step::recorded(), Commit::All);
        // Stale outside the gate is not planned; stale inside is.
        h.step(Step::recorded().indirect(true), Commit::All);
        assert_eq!(h.plan.indirect.rows(), &[1]);
        // A pending row outside the gate is planned.
        h.mark_pending(3, &[0]);
        h.step(Step::recorded(), Commit::All);
        assert_eq!(h.plan.indirect.rows(), &[1, 3]);
        // Row 2 left the gate stale and composes on re-entry.
        h.gate(&[2]);
        h.step(Step::recorded(), Commit::All);
        assert_eq!(h.plan.indirect.rows(), &[2]);

        // Lag and pending work left uncommitted do not survive a session
        // clear; reinstalled rows compose as fresh installs.
        h.mark_pending(1, &[0, 1, 2]);
        h.step(Step::recorded().indirect(true), Commit::None);
        h.clear_session();
        h.step(Step::recorded(), Commit::All);
        assert!(h.plan.ordered().iter().all(|plan| plan.rows().is_empty()));
        all_installed(&mut h, &[1, 2, 3]);
        h.step(Step::recorded(), Commit::All);
        assert_eq!(h.plan.indirect.rows(), &[1, 2, 3]);
        h.step(Step::recorded().animated(true), Commit::All);
    }

    #[test]
    fn equivalence_control_change_with_empty_pass_retires_repair() {
        let mut h = Harness::new(16);
        // No id-45 data: Pass B has no resident rows.
        let no_animated = ModelRow {
            animated_resident: false,
            animated_contributing: false,
            ..ModelRow::installed(true, true, false)
        };
        for row in [1, 2, 3] {
            h.install(row, no_animated);
        }
        h.gate(&[1]);
        h.step(Step::recorded(), Commit::All);

        h.controls.light_term_mask = LightTermMask::AMBIENT_FLOOR;
        h.step(Step::recorded(), Commit::All);
        assert_eq!(h.plan.indirect.rows(), &[1, 2, 3]);
        assert!(h.plan.animated_direct.full_repair());
        assert!(h.plan.animated_direct.rows().is_empty());
        h.step(Step::recorded(), Commit::All);
        assert!(h.plan.ordered().iter().all(|plan| !plan.full_repair()));

        // A row joining Pass B after the repair is not swept in by a stuck
        // latch: stale outside the gate, it waits for gate entry.
        h.set_row(5, ModelRow::installed(true, true, true));
        h.step(Step::recorded().animated(true), Commit::All);
        assert!(h.plan.animated_direct.rows().is_empty());
        h.gate(&[1, 5]);
        h.step(Step::recorded(), Commit::All);
        assert_eq!(h.plan.animated_direct.rows(), &[5]);
    }

    #[test]
    fn equivalence_membership_joins_after_change_epoch() {
        let mut h = Harness::new(16);
        // Row 4 is base-resident only while animation is active, then gains
        // id-27/id-45 contributions without a new trigger.
        h.install(4, ModelRow::installed(false, false, false));
        h.install(5, ModelRow::installed(true, true, true));
        h.gate(&[4, 5]);
        h.step(Step::recorded(), Commit::All);
        h.step(Step::recorded().indirect(true).animated(true), Commit::All);
        // The one-frame tail fires before row 4 contributes.
        h.step(Step::recorded(), Commit::All);
        h.set_row(4, ModelRow::installed(true, true, true));
        h.step(Step::recorded(), Commit::All);
        assert!(h.plan.ordered().iter().all(|plan| plan.rows().is_empty()));
        h.step(Step::recorded(), Commit::All);
        // Leave the contributing set while stale and outside the gate, then
        // rejoin: the carried lag composes on gate re-entry.
        h.gate(&[]);
        h.step(Step::recorded().indirect(true), Commit::All);
        h.set_row(5, ModelRow::installed(false, true, true));
        h.step(Step::recorded(), Commit::All);
        h.set_row(5, ModelRow::installed(true, true, true));
        h.step(Step::recorded(), Commit::All);
        h.gate(&[5]);
        h.step(Step::recorded(), Commit::All);
    }

    #[test]
    fn equivalence_generation_overflow_rebase() {
        let mut h = Harness::new(8);
        all_installed(&mut h, &[0, 1, 2, 3]);
        h.gate(&[0, 1]);
        h.step(Step::recorded(), Commit::All);
        h.step(Step::recorded().indirect(true).animated(true), Commit::All);
        h.set_generation(u64::MAX - 2);
        for frame in 0..8 {
            h.static_weights[0] = frame as f32;
            h.step(
                Step::recorded()
                    .indirect(frame % 3 != 0)
                    .animated(frame % 2 == 0),
                if frame % 4 == 1 {
                    Commit::None
                } else {
                    Commit::All
                },
            );
        }
        h.gate(&[0, 1, 2, 3]);
        h.step(Step::recorded(), Commit::All);
    }

    #[test]
    fn equivalence_overflow_preserves_lag_from_quiet_sources() {
        let mut h = Harness::new(8);
        h.install(2, ModelRow::installed(true, true, true));
        h.install(3, ModelRow::installed(false, false, false));
        h.gate(&[2]);
        h.step(Step::recorded(), Commit::All);
        // Row 3 lags only through the control-change (resident) source.
        h.controls.light_term_mask = LightTermMask::AMBIENT_FLOOR;
        h.step(Step::recorded().skipped(), Commit::None);
        h.set_generation(u64::MAX - 1);
        // Contributing triggers overflow the counter while the repair is
        // still uncommitted for Pass 0.
        for _ in 0..3 {
            h.step(Step::recorded().indirect(true), Commit::Only(&[1, 2]));
        }
        assert!(h.plan.indirect.rows().contains(&3));
        h.step(Step::recorded(), Commit::All);
    }

    /// Deterministic xorshift so the sequence test needs no seed plumbing.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn chance(&mut self, one_in: u64) -> bool {
            self.next().is_multiple_of(one_in)
        }
        fn below(&mut self, bound: u64) -> u64 {
            self.next() % bound
        }
    }

    fn random_row(rng: &mut Rng, invariant: bool) -> ModelRow {
        if invariant {
            if rng.chance(4) {
                return ModelRow::default();
            }
            let row = ModelRow::installed(rng.chance(2), rng.chance(2), rng.chance(2));
            if rng.chance(5) {
                // A level without id-45 data: Pass B is never resident.
                return ModelRow {
                    animated_resident: false,
                    animated_contributing: false,
                    ..row
                };
            }
            return row;
        }
        // No residency invariant: every membership bit is independent.
        let bits = rng.next();
        ModelRow {
            indirect_resident: bits & 1 != 0,
            indirect_contributing: bits & 2 != 0,
            static_resident: bits & 4 != 0,
            static_contributing: bits & 8 != 0,
            animated_resident: bits & 16 != 0,
            animated_contributing: bits & 32 != 0,
        }
    }

    fn random_sequence(seed: u64, invariant: bool) {
        const ROWS: u32 = 24;
        let mut rng = Rng(seed);
        let mut h = Harness::new(ROWS as usize);
        let (mut planned_frames, mut repair_frames, mut lagging_frames) = (0, 0, 0);
        for frame in 0..400 {
            for _ in 0..rng.below(4) {
                let row = rng.below(u64::from(ROWS)) as u32;
                let value = random_row(&mut rng, invariant);
                if rng.chance(2) {
                    h.install(row, value);
                } else {
                    h.set_row(row, value);
                }
            }
            if rng.chance(6) {
                let row = rng.below(u64::from(ROWS)) as u32;
                h.mark_pending(row, &[rng.below(3) as usize]);
            }
            if rng.chance(3) {
                let gated: Vec<u32> = (0..ROWS).filter(|_| rng.chance(3)).collect();
                h.gate(&gated);
            }
            if rng.chance(5) {
                h.static_weights[0] = rng.below(4) as f32 * 0.25;
            }
            if rng.chance(7) {
                h.animated_weights[0] = rng.below(4) as f32 * 0.25;
            }
            if rng.chance(25) {
                h.controls.promotion_override.enabled = !h.controls.promotion_override.enabled;
            }
            if rng.chance(150) {
                h.clear_session();
            }
            if frame == 200 {
                h.set_generation(u64::MAX - 3);
            }
            let step = Step {
                records: !rng.chance(8),
                force_full: rng.chance(20),
                indirect_active: rng.chance(3),
                animated_active: rng.chance(3),
            };
            let commit = match rng.below(6) {
                0 => Commit::None,
                1 => Commit::Only(&[0, 1]),
                2 => Commit::Only(&[1]),
                _ => Commit::All,
            };
            h.step(step, commit);
            let [indirect, static_direct, animated] = h.plan.ordered();
            planned_frames += usize::from(
                !(indirect.rows().is_empty()
                    && static_direct.rows().is_empty()
                    && animated.rows().is_empty()),
            );
            repair_frames += usize::from(indirect.full_repair());
            lagging_frames += usize::from(h.pull.lagging_rows(ComposePass::Indirect) > 0);
        }
        // Guard against a vacuous sequence: both planners must actually plan,
        // repair, and carry lag across frames.
        assert!(planned_frames > 100, "planned frames {planned_frames}");
        assert!(repair_frames > 0, "repair frames {repair_frames}");
        assert!(lagging_frames > 50, "lagging frames {lagging_frames}");
    }

    #[test]
    fn equivalence_randomized_sequences_with_residency_invariant() {
        for seed in 1..=24 {
            random_sequence(0x9E37_79B9_7F4A_7C15 ^ seed, true);
        }
    }

    #[test]
    fn equivalence_randomized_sequences_without_residency_invariant() {
        for seed in 1..=24 {
            random_sequence(0xD1B5_4A32_D192_ED03 ^ seed, false);
        }
    }

    fn scaling_frame<'a>(
        membership_changes: &'a [(u32, RowMembership)],
        gated_rows: &'a [u32],
        static_weights: &'a [f32],
        animated_weights: &'a [f32],
    ) -> ComposePlannerFrame<'a> {
        ComposePlannerFrame {
            records_compose: true,
            force_full_resident: false,
            gated_rows,
            membership_changes,
            indirect_active: true,
            animated_direct_active: true,
            effective_static_weights: static_weights,
            effective_animated_weights: animated_weights,
            controls: controls(),
        }
    }

    /// Commits only the passes the renderer would encode.
    fn commit_encoded(planner: &mut StreamedComposePlanner, plan: &ComposeFramePlan) {
        for pass in plan.ordered() {
            if !pass.rows().is_empty() {
                planner.commit_pass(pass);
            }
        }
    }

    #[test]
    fn control_repair_on_empty_pass_does_not_rescan_every_frame() {
        const ROWS: u32 = 1_000_000;
        let mut planner = StreamedComposePlanner::with_row_capacity(ROWS as usize);
        // Neither direct pass has resident rows, as on a level without
        // direct SH.
        let indirect_only = ModelRow {
            indirect_resident: true,
            indirect_contributing: true,
            ..ModelRow::default()
        }
        .membership();
        let resident: Vec<u32> = (0..1_000).map(|index| index * 997).collect();
        let install: Vec<(u32, RowMembership)> =
            resident.iter().map(|&row| (row, indirect_only)).collect();
        let gated: Vec<u32> = resident.iter().copied().step_by(10).collect();
        let static_weights = [0.0];
        let animated_weights = [1.0];
        let mut plan = ComposeFramePlan::default();
        planner.plan_frame_into(
            scaling_frame(&install, &[], &static_weights, &animated_weights),
            &mut plan,
        );
        commit_encoded(&mut planner, &plan);

        fn changed_frame<'a>(
            gated_rows: &'a [u32],
            static_weights: &'a [f32],
            animated_weights: &'a [f32],
        ) -> ComposePlannerFrame<'a> {
            let mut frame = scaling_frame(&[], gated_rows, static_weights, animated_weights);
            frame.controls.light_term_mask = LightTermMask::AMBIENT_FLOOR;
            frame
        }
        planner.plan_frame_into(
            changed_frame(&gated, &static_weights, &animated_weights),
            &mut plan,
        );
        assert_eq!(plan.indirect.rows(), resident.as_slice());
        for pass in [&plan.static_direct, &plan.animated_direct] {
            assert!(pass.full_repair());
            assert!(pass.rows().is_empty());
        }
        commit_encoded(&mut planner, &plan);

        for frame_index in 0..8 {
            planner.reset_visited_for_test();
            planner.plan_frame_into(
                changed_frame(&gated, &static_weights, &animated_weights),
                &mut plan,
            );
            commit_encoded(&mut planner, &plan);
            assert!(plan.ordered().iter().all(|pass| !pass.full_repair()));
            assert_eq!(plan.indirect.rows(), gated.as_slice());
            let bound = 3 * 4 * gated.len();
            let visited = planner.visited_for_test();
            assert!(
                visited <= bound,
                "frame {frame_index} touched {visited} rows; bound {bound}"
            );
        }
    }

    #[test]
    fn per_frame_planning_scales_with_gated_and_pending_rows() {
        const ROWS: u32 = 1_000_000;
        let mut planner = StreamedComposePlanner::with_row_capacity(ROWS as usize);
        let everything = ModelRow::installed(true, true, true).membership();
        let install: Vec<(u32, RowMembership)> = (0..ROWS).map(|row| (row, everything)).collect();
        let gated: Vec<u32> = (0..100).map(|index| 5_000 + index * 9_973).collect();
        let static_weights = [0.0];
        let animated_weights = [1.0];
        let mut plan = ComposeFramePlan::default();
        // Level-load residency: a one-time O(resident) membership install.
        planner.plan_frame_into(
            scaling_frame(&install, &[], &static_weights, &animated_weights),
            &mut plan,
        );
        commit_encoded(&mut planner, &plan);
        assert_eq!(planner.lagging_rows(ComposePass::Indirect), ROWS as usize);

        for frame_index in 0..16u32 {
            // A streaming drain adds a couple of pending rows per frame.
            let pending = [frame_index * 31, frame_index * 31 + 1];
            for pass in PASSES {
                planner.mark_residency_rows(pass, pending);
            }
            planner.reset_visited_for_test();
            planner.plan_frame_into(
                scaling_frame(&[], &gated, &static_weights, &animated_weights),
                &mut plan,
            );
            commit_encoded(&mut planner, &plan);
            let bound = 3 * 4 * (gated.len() + pending.len());
            let visited = planner.visited_for_test();
            assert!(
                visited <= bound,
                "frame {frame_index} touched {visited} rows; bound {bound}"
            );
            assert_eq!(plan.indirect.rows().len(), gated.len() + pending.len());
            assert_eq!(
                plan.animated_direct.rows().len(),
                gated.len() + pending.len()
            );
            assert_eq!(plan.static_direct.rows(), pending.as_slice());
            assert_eq!(
                planner.lagging_rows(ComposePass::Indirect),
                ROWS as usize - gated.len() - pending.len()
            );
        }
    }
}
