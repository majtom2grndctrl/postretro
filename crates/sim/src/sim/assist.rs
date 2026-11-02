//! Teammate assist: the fixed-point hold conversion and the session-owned
//! assist state.
//! See: context/lib/scripting.md §12 (Assist)
//!
//! Progress is a u64 in units of [`PROGRESS_UNIT`] (2³²), never summed
//! fractions. A hold of n required ticks advances by [`increment`]`(n)` per
//! advancing tick and completes on the tick whose progress reaches one whole;
//! for n ≤ 3600 (the 60 s cap), n(n − 1) < 2³², so a constant n completes on
//! exactly the n-th advancing tick. Host and client use the same functions, and
//! the step takes `required_ticks`, never `holdMs`, so a client fed the host's
//! integer computes the host's increment bit for bit.

use postretro_entities::ScriptCtx;

use super::assist_pairs::AssistPairTable;

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

/// Session-owned assist state, beside `touch_system`. It holds the host's
/// bound pair table; the accumulator's cross-tick progress joins it later.
/// A connected client keeps the default, empty table: it never evaluates
/// pair IR.
#[derive(Default)]
pub struct AssistState {
    pair_table: AssistPairTable,
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
}

#[cfg(test)]
#[path = "assist_tests.rs"]
mod tests;
