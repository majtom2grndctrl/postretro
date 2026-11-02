//! Teammate assist: the fixed-point hold conversion, the accumulator's pure
//! step and target choice, and the session-owned assist state. The host pass
//! that drives the step each tick lives in `assist_pass`.
//! See: context/lib/scripting.md §12 (Assist)
//!
//! Progress is a u64 in units of [`PROGRESS_UNIT`] (2³²), never summed
//! fractions. A hold of n required ticks advances by [`increment`]`(n)` per
//! advancing tick and completes on the tick whose progress reaches one whole;
//! for n ≤ 3600 (the 60 s cap), n(n − 1) < 2³², so a constant n completes on
//! exactly the n-th advancing tick. Host and client use the same functions, and
//! the step takes `required_ticks`, never `holdMs`, so a client fed the host's
//! integer computes the host's increment bit for bit.

use glam::Vec3;
use postretro_entities::{EntityId, ScriptCtx};

use super::assist_pairs::{AssistPairTable, PairValues};

/// One whole hold in fixed-point progress units.
pub const PROGRESS_UNIT: u64 = 1 << 32;

/// The required tick count for a hold of `hold_ms`: `ceil(holdMs × 1000 /
/// TICK_DURATION µs)` in f64, at least one — the conversion `wait`
/// enrollment uses. `hold_ms` is a finite value already clamped to
/// `[0, ASSIST_MAX_HOLD_MS]` by the caller; anything else saturates.
pub fn required_count(hold_ms: f32) -> u32 {
    let micros = postretro_foundation::TICK_DURATION.as_micros() as f64;
    let ticks = (f64::from(hold_ms) * 1000.0 / micros).ceil();
    if ticks.is_nan() || ticks <= 1.0 {
        1
    } else if ticks >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        ticks as u32
    }
}

/// One advancing tick's progress for a hold of `required_ticks`:
/// `ceil(2³² / n)`. Zero for `required_ticks` 0, the "kept, not advanced"
/// sentinel a faulted or negative `holdMs` produces.
pub fn increment(required_ticks: u32) -> u64 {
    if required_ticks == 0 {
        return 0;
    }
    PROGRESS_UNIT.div_ceil(u64::from(required_ticks))
}

/// Which machine runs [`step`]: the host (and single player) authoritatively,
/// or a connected client predicting its own pawn's helper bar. They differ only
/// in rows 3 and 6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistRunner {
    Host,
    Client,
}

/// A pawn's life state as one pass sees it. `elapsed` is
/// `life_state_elapsed_ticks`, or `None` where the runner retains no elapsed
/// (the client's remote pawns): the reset check then compares ids alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PawnLife {
    pub life_state: u8,
    pub elapsed: Option<u32>,
}

impl PawnLife {
    /// Whether this pawn changed or re-entered its life state since
    /// `previous`, the value stored at the last pass: a different id, or, in a
    /// declared state, an elapsed not greater than the stored one (a respawn
    /// into the same state, a leave and re-entry between passes). A pawn with
    /// no stored value is new to the accumulator and counts as changed. An
    /// undeclared pawn's elapsed stays zero, so only its id is compared.
    pub fn changed_since(self, previous: Option<PawnLife>) -> bool {
        let Some(previous) = previous else {
            return true;
        };
        if previous.life_state != self.life_state {
            return true;
        }
        if self.life_state == postretro_foundation::UNDECLARED_LIFE_STATE {
            return false;
        }
        match (self.elapsed, previous.elapsed) {
            (Some(elapsed), Some(stored)) => elapsed <= stored,
            _ => false,
        }
    }
}

/// The helper side of one step: identity, origin, whether its state permits
/// `use`, and its life state now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AssistHelper {
    pub id: EntityId,
    pub position: Vec3,
    pub permits_use: bool,
    pub life: PawnLife,
}

/// One other pawn a helper may assist. `pair` is the pass's evaluated values
/// for (helper, this pawn); `None` when the pass evaluated no pair for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AssistCandidate {
    pub id: EntityId,
    pub position: Vec3,
    pub life: PawnLife,
    pub assistable: bool,
    pub pair: Option<PairValues>,
}

/// A helper's progress between passes: fixed-point progress, the target it
/// accumulated on, and the latch marking an ended hold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HelperProgress {
    pub progress: u64,
    pub target: Option<EntityId>,
    pub latched: bool,
}

/// The step table's rows, in match order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistRow {
    /// 1: released on a real command. Progress 0, latch cleared.
    Release = 1,
    /// 2: not held on a command that is not real (a neutral after the hold
    /// grace, or no resolved command). Progress 0, latch kept.
    Neutral = 2,
    /// 3: held while latched. Progress does not advance.
    Latched = 3,
    /// 4: held with no target. Progress 0.
    NoTarget = 4,
    /// 5: held on a gap-held command. Progress kept, not advanced.
    GapHeld = 5,
    /// 6: held on a real command with a target. Progress advances.
    Advance = 6,
}

/// Everything one helper's step reads.
#[derive(Debug, Clone, Copy)]
pub struct AssistStepInput<'a> {
    pub runner: AssistRunner,
    pub helper: AssistHelper,
    /// The command's `assist_held` level.
    pub held: bool,
    /// Whether this tick's command is real (not gap-held or synthesized).
    pub real_command: bool,
    /// The other pawns, in `players()` order (ties go to the earlier).
    pub candidates: &'a [AssistCandidate],
    pub previous: HelperProgress,
    /// Each pawn's life state at the previous pass. A pawn absent here is new.
    pub previous_lives: &'a [(EntityId, PawnLife)],
}

/// One step's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssistStep {
    pub progress: u64,
    pub latched: bool,
    /// The chosen target; `None` for rows 1–4.
    pub target: Option<EntityId>,
    pub row: AssistRow,
    /// The step's own reset fired: rows 1, 2 and 4, or the reset before the
    /// table (target switch, or either pawn changing or re-entering a state).
    /// The host's completion reset in row 6 is not one.
    pub step_reset: bool,
    pub completed: bool,
    /// The chosen pair's `required_ticks`; 0 with no target.
    pub required_ticks: u32,
}

/// The nearest other pawn, by origin distance, whose `assistable` bool is set
/// and whose pair is in range; ties go to the earlier candidate (`players()`
/// order). The helper's own id is skipped, and a candidate with no pair
/// values, an unassistable pair or one out of range is never chosen. A helper
/// whose state forbids `use` chooses nothing.
pub fn assist_candidate<'a>(
    helper: &AssistHelper,
    candidates: &'a [AssistCandidate],
) -> Option<&'a AssistCandidate> {
    if !helper.permits_use {
        return None;
    }
    let mut best: Option<(&AssistCandidate, f32)> = None;
    for candidate in candidates {
        if candidate.id == helper.id || !candidate.assistable {
            continue;
        }
        let Some(pair) = candidate.pair else {
            continue;
        };
        let distance = helper.position.distance(candidate.position);
        if !pair.in_range(distance) {
            continue;
        }
        if best.is_none_or(|(_, nearest)| distance < nearest) {
            best = Some((candidate, distance));
        }
    }
    best.map(|(candidate, _)| candidate)
}

/// One helper's accumulator step: the one rule both runners share. With
/// `assist` held and the latch clear it chooses a target
/// ([`assist_candidate`]) and resets progress when that target differs from
/// the previous one or either pawn changed or re-entered a life state; then
/// the first matching row applies (see [`AssistRow`]). Contains no IR.
pub fn step(input: &AssistStepInput<'_>) -> AssistStep {
    let previous = input.previous;
    let outcome = |row, progress, latched, step_reset| AssistStep {
        progress,
        latched,
        target: None,
        row,
        step_reset,
        completed: false,
        required_ticks: 0,
    };
    if !input.held {
        return if input.real_command {
            outcome(AssistRow::Release, 0, false, true)
        } else {
            outcome(AssistRow::Neutral, 0, previous.latched, true)
        };
    }
    if previous.latched {
        let progress = match input.runner {
            AssistRunner::Host => 0,
            AssistRunner::Client => previous.progress,
        };
        return outcome(AssistRow::Latched, progress, true, false);
    }
    let Some(target) = assist_candidate(&input.helper, input.candidates) else {
        return outcome(AssistRow::NoTarget, 0, false, true);
    };
    let stored = |id: EntityId| {
        input
            .previous_lives
            .iter()
            .find(|(pawn, _)| *pawn == id)
            .map(|(_, life)| *life)
    };
    let step_reset = previous.target != Some(target.id)
        || input.helper.life.changed_since(stored(input.helper.id))
        || target.life.changed_since(stored(target.id));
    let mut progress = if step_reset { 0 } else { previous.progress };
    let required_ticks = target.pair.map_or(0, |pair| pair.required_ticks);
    let mut result = AssistStep {
        progress,
        latched: false,
        target: Some(target.id),
        row: AssistRow::GapHeld,
        step_reset,
        completed: false,
        required_ticks,
    };
    if !input.real_command {
        return result;
    }
    result.row = AssistRow::Advance;
    progress = progress.saturating_add(increment(required_ticks));
    if required_ticks > 0 && progress >= PROGRESS_UNIT {
        result.completed = true;
        result.latched = true;
        progress = match input.runner {
            AssistRunner::Host => 0,
            AssistRunner::Client => PROGRESS_UNIT,
        };
    }
    result.progress = progress;
    result
}

/// Reported progress: progress over one whole, capped at 1.
pub fn reported(progress: u64) -> f32 {
    (progress.min(PROGRESS_UNIT) as f64 / PROGRESS_UNIT as f64) as f32
}

/// One host completion, recorded into `TickEvents::assist_completions` with
/// the target's life state and elapsed at record, which the player-event pass
/// re-checks before it applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssistCompletion {
    pub helper: EntityId,
    pub target: EntityId,
    pub life_state: u8,
    pub life_state_elapsed_ticks: u32,
}

/// A helper's row-6 record from its last pass: whether that pass ran row 6
/// without completing, and the chosen pair's `required_ticks` at that pass
/// whichever row it ran (0 with no target). A helper the pass did not see
/// reads the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Row6Record {
    pub advanced_without_completing: bool,
    pub required_ticks: u32,
}

/// One helper's cross-tick accumulator state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HelperRecord {
    pub progress: HelperProgress,
    pub row6: Row6Record,
}

/// Session-owned assist state, beside `touch_system`: the host's bound pair
/// table and the accumulator's cross-tick state. A connected client keeps the
/// default, empty table: it never evaluates pair IR.
#[derive(Default)]
pub struct AssistState {
    pair_table: AssistPairTable,
    /// Per-helper progress, target, latch and row-6 record, in the last
    /// pass's `players()` order.
    pub(super) helpers: Vec<(EntityId, HelperRecord)>,
    /// Each pawn's life state at the last pass, in `players()` order.
    pub(super) lives: Vec<(EntityId, PawnLife)>,
    /// The last pass's evaluated pair values, as (helper, target, values).
    pub(super) pair_values: Vec<(EntityId, EntityId, PairValues)>,
}

impl AssistState {
    /// The bound `assistable` table the accumulator pass and the snapshot
    /// build evaluate pairs through.
    pub fn pair_table(&self) -> &AssistPairTable {
        &self.pair_table
    }

    /// Rebind every `assistable` profile over the active descriptors,
    /// replacing the whole table and re-arming its fault warnings. Runs at
    /// every level install and recompose; a connected client binds nothing.
    pub fn rebind(&mut self, ctx: &ScriptCtx) {
        self.pair_table = AssistPairTable::bind(ctx);
    }

    /// The values the last accumulator pass evaluated for (helper, target),
    /// or `None` for a pair that pass did not evaluate. Host-side consumers
    /// read pair values here and never evaluate pair IR themselves.
    pub fn pair_values(&self, helper: EntityId, target: EntityId) -> Option<PairValues> {
        self.pair_values
            .iter()
            .find(|(h, t, _)| *h == helper && *t == target)
            .map(|(_, _, values)| *values)
    }

    /// A helper's cross-tick state from the last pass, or `None` for a pawn
    /// that pass did not see.
    pub fn helper(&self, helper: EntityId) -> Option<&HelperRecord> {
        self.helpers
            .iter()
            .find(|(id, _)| *id == helper)
            .map(|(_, record)| record)
    }

    /// Every helper's record from the last pass, in its `players()` order.
    pub fn helpers(&self) -> impl Iterator<Item = (EntityId, &HelperRecord)> {
        self.helpers.iter().map(|(id, record)| (*id, record))
    }

    /// A helper's row-6 record from the last pass; the default for a pawn
    /// that pass did not see.
    pub fn row6_record(&self, helper: EntityId) -> Row6Record {
        self.helper(helper)
            .map(|record| record.row6)
            .unwrap_or_default()
    }

    /// Level-scoped clear: progress, latches, row-6 records, the last pass's
    /// pair values and the stored per-pawn life states go; the bound table
    /// survives until the next install rebinds it. Suspend never calls this.
    pub fn clear_for_level_unload(&mut self) {
        self.helpers.clear();
        self.lives.clear();
        self.pair_values.clear();
    }
}

#[cfg(test)]
#[path = "assist_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "assist_step_tests.rs"]
mod step_tests;
