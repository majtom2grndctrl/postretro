// App-side screen-shake state for the engine-owned `screen.shake` surface.
// The screen-effects resolve pass (render/screen_effects.rs) consumes this slot
// each frame; at rest the slot is exact `[0, 0]` so the consumer collapses to identity.
// See: context/lib/ui.md §3 · context/lib/scripting.md §10.4

use std::f32::consts::TAU;

use crate::scripting::primitives::store::write_store_slot;
use postretro_entities::ctx::ScriptCtx;
use postretro_entities::slot_table::SlotValue;

/// Dotted name of the engine-owned screen-shake offset slot.
const SHAKE_SLOT: &str = "screen.shake";

/// At-rest shake offset (logical-reference px). Written when no shake is active
/// and as the oscillation's resting endpoint — exact zero so there is no
/// displacement and the compositor pass is identity at rest.
const REST: [f32; 2] = [0.0, 0.0];

/// Default oscillation frequency in Hz, applied HERE by the driver when a `start`
/// omits the frequency. 18 Hz reads as a sharp, percussive shake rather than a
/// slow sway.
const DEFAULT_FREQUENCY_HZ: f32 = 18.0;

/// One active shake: its peak amplitude (logical-reference px), oscillation
/// frequency, total duration, and how long it has run. Pure oscillation math; no
/// store, no GPU, no wall-clock — `elapsed_ms` is advanced from the injected
/// frame delta so the offset is deterministic and unit-testable. The x and y
/// axes oscillate on a quarter-period phase offset so the motion traces a
/// decaying figure rather than a straight diagonal line.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ActiveShake {
    /// Peak displacement at the start, in logical-reference px. Clamped to
    /// `[0, +∞)`.
    amplitude: f32,
    /// Oscillation frequency in Hz (cycles per second). Clamped to `[0, +∞)`.
    frequency_hz: f32,
    /// Total duration in milliseconds: amplitude ramps linearly to zero over this
    /// span. A non-positive duration collapses to an instantaneous one-frame
    /// shake.
    duration_ms: f32,
    /// Milliseconds elapsed since the shake started.
    elapsed_ms: f32,
}

impl ActiveShake {
    /// The current `[dx, dy]` offset: a sinusoidal oscillation whose amplitude
    /// decays linearly to zero over `duration_ms`. The x and y axes are a quarter
    /// period apart (sin vs. cos) so the shake traces a decaying orbit rather
    /// than a single diagonal axis. Reaches exact zero at `duration_ms`.
    fn current_offset(&self) -> [f32; 2] {
        let amplitude = self.amplitude.max(0.0);
        let remaining = if self.duration_ms > 0.0 {
            (1.0 - self.elapsed_ms / self.duration_ms).clamp(0.0, 1.0)
        } else {
            // Non-positive duration: full on the first frame (elapsed 0), then
            // rest — a single-frame spike rather than a divide-by-zero.
            if self.elapsed_ms <= 0.0 { 1.0 } else { 0.0 }
        };
        let decayed = amplitude * remaining;
        let phase = TAU * self.frequency_hz.max(0.0) * (self.elapsed_ms / 1000.0);
        [decayed * phase.sin(), decayed * phase.cos()]
    }

    /// Whether the shake has fully decayed (amplitude reached zero).
    fn is_finished(&self) -> bool {
        self.elapsed_ms >= self.duration_ms
    }
}

/// Owns the active shake and writes `screen.shake` each game-logic tick.
///
/// Holds a clone of `App`'s `ScriptCtx` (cheap `Rc` bump). A drained shake
/// command calls [`ShakeDecay::start`]; [`ShakeDecay::tick`] runs once per
/// game-logic tick (beside `FlashDecay::tick`) to advance and publish the
/// offset. A new `start` mid-flight replaces the active shake (latest wins),
/// matching how `FlashDecay` restarts on a fresh trigger.
pub(crate) struct ShakeDecay {
    ctx: ScriptCtx,
    active: Option<ActiveShake>,
    /// True once the slot has been written to rest after a shake ended, so an
    /// idle `ShakeDecay` writes the resting zero offset exactly once rather than
    /// every frame in the per-frame hot path.
    wrote_idle: bool,
}

impl ShakeDecay {
    /// Build a shake-decay state holding a clone of the engine's `ScriptCtx`.
    pub(crate) fn new(ctx: ScriptCtx) -> Self {
        Self {
            ctx,
            active: None,
            wrote_idle: false,
        }
    }

    /// Start (or restart) a shake: `amplitude` (logical-reference px) decaying to
    /// zero over `duration_ms`, oscillating at `frequency_hz` if supplied, else
    /// the engine default (18 Hz). The default is applied here in the driver, not
    /// by an arg deserializer, so the omitted-frequency case is one behavior
    /// regardless of how the reaction surface parses its args. The new shake
    /// replaces any in-flight one so the latest trigger wins.
    pub(crate) fn start(&mut self, amplitude: f32, duration_ms: f32, frequency_hz: Option<f32>) {
        self.active = Some(ActiveShake {
            amplitude,
            frequency_hz: frequency_hz.unwrap_or(DEFAULT_FREQUENCY_HZ),
            duration_ms,
            elapsed_ms: 0.0,
        });
        self.wrote_idle = false;
    }

    /// Clear any active shake and reset the idle latch. Called on level load so a
    /// shake never bleeds across levels.
    pub(crate) fn reset(&mut self) {
        self.active = None;
        self.wrote_idle = false;
    }

    /// Advance the active shake by the injected frame delta (seconds) and publish
    /// `screen.shake`. With no active shake, writes the resting zero offset once,
    /// then short-circuits subsequent idle frames.
    ///
    /// Runs at the game-logic stage (beside `FlashDecay::tick`), after game logic
    /// and before the UI read-snapshot build, so the snapshot freezes the shake
    /// offset this same frame.
    pub(crate) fn tick(&mut self, dt: f32) {
        let Some(shake) = self.active.as_mut() else {
            // Idle: write exact-zero rest once so the compositor is identity, then
            // stop touching the slot every frame.
            if !self.wrote_idle {
                self.write(REST);
                self.wrote_idle = true;
            }
            return;
        };

        shake.elapsed_ms += dt * 1000.0;
        let finished = shake.is_finished();
        // At rest the offset must be EXACT zero, not a near-zero sample of the
        // sinusoid at the final phase — so once finished, publish the rest value
        // directly rather than the computed offset.
        let offset = if finished {
            REST
        } else {
            shake.current_offset()
        };
        self.write(offset);

        if finished {
            self.active = None;
            self.wrote_idle = true;
        }
    }

    /// Write `screen.shake` via the engine write path (bypasses readonly,
    /// validates the Array). An error here would be an engine bug (the slot is
    /// engine-owned and always declared), so it is logged rather than skipped.
    fn write(&self, offset: [f32; 2]) {
        if let Err(err) = write_store_slot(&self.ctx, SHAKE_SLOT, SlotValue::Array(offset.to_vec()))
        {
            log::warn!("[ShakeDecay] failed to write `{SHAKE_SLOT}`: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::primitives::store::read_store_slot;

    fn read_shake(ctx: &ScriptCtx) -> [f32; 2] {
        match read_store_slot(ctx, SHAKE_SLOT).unwrap() {
            SlotValue::Array(values) => {
                let mut out = [0.0; 2];
                out.copy_from_slice(&values[..2]);
                out
            }
            other => panic!("screen.shake should be an Array, got {other:?}"),
        }
    }

    fn approx(a: [f32; 2], b: [f32; 2]) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn shake_oscillates_then_returns_to_exact_zero() {
        // A shake command starts a decaying oscillation; each tick advances the
        // sinusoid while the envelope decays to zero. At the duration the offset
        // is EXACT zero so the compositor collapses to identity.
        let ctx = ScriptCtx::new();
        let mut decay = ShakeDecay::new(ctx.clone());

        decay.start(10.0, 200.0, Some(10.0));

        // First tick (dt = 0): phase zero ⇒ sin component zero, cos component at
        // full amplitude (the orbit starts on the y axis).
        decay.tick(0.0);
        assert!(
            approx(read_shake(&ctx), [0.0, 10.0]),
            "phase-zero offset is full amplitude on y, got {:?}",
            read_shake(&ctx)
        );

        // Mid-shake: the offset is bounded by the decayed amplitude.
        decay.tick(0.05);
        let mid = read_shake(&ctx);
        let mag = (mid[0] * mid[0] + mid[1] * mid[1]).sqrt();
        assert!(
            mag <= 10.0 + 1e-4,
            "mid-shake magnitude within decayed amplitude, got {mag}"
        );

        // Past the duration: exact zero offset (not a near-zero sinusoid sample).
        decay.tick(0.2);
        assert_eq!(
            read_shake(&ctx),
            [0.0, 0.0],
            "shake returns to exact zero at the duration"
        );
    }

    #[test]
    fn shake_magnitude_is_non_increasing_envelope() {
        // Property: the oscillation envelope (decayed amplitude) is non-increasing,
        // so sampling at quarter-period peaks yields a non-increasing magnitude.
        let ctx = ScriptCtx::new();
        let mut decay = ShakeDecay::new(ctx.clone());
        // 5 Hz ⇒ 200 ms period; quarter period = 50 ms lands on sin/cos peaks.
        decay.start(8.0, 1000.0, Some(5.0));

        decay.tick(0.0);
        let mut prev = {
            let o = read_shake(&ctx);
            (o[0] * o[0] + o[1] * o[1]).sqrt()
        };
        for _ in 0..8 {
            decay.tick(0.05);
            let o = read_shake(&ctx);
            let cur = (o[0] * o[0] + o[1] * o[1]).sqrt();
            assert!(
                cur <= prev + 1e-4,
                "shake envelope magnitude must be non-increasing: prev {prev}, cur {cur}"
            );
            prev = cur;
        }
    }

    #[test]
    fn omitted_frequency_uses_engine_default() {
        // The driver applies the 18 Hz default when start omits the frequency.
        // A 1/18 s span is one full cycle, returning the orbit to its start phase
        // (sin 0, cos full) modulo the envelope decay — proving the default rate.
        let ctx = ScriptCtx::new();
        let mut decay = ShakeDecay::new(ctx.clone());
        // Long duration so the envelope barely decays over one cycle.
        decay.start(10.0, 100_000.0, None);

        decay.tick(0.0);
        let start = read_shake(&ctx);
        // One full period at 18 Hz.
        decay.tick(1.0 / 18.0);
        let after_cycle = read_shake(&ctx);
        // x (sin) returns to ~0; y (cos) returns to ~full amplitude.
        assert!(
            after_cycle[0].abs() < 1e-2,
            "after one default-rate cycle the sin axis is back near zero, got {after_cycle:?}"
        );
        assert!(
            (after_cycle[1] - start[1]).abs() < 0.05,
            "after one default-rate cycle the cos axis is back near the start, got {after_cycle:?} vs {start:?}"
        );
    }

    #[test]
    fn restart_mid_flight_replaces_active_shake() {
        let ctx = ScriptCtx::new();
        let mut decay = ShakeDecay::new(ctx.clone());
        decay.start(10.0, 200.0, Some(10.0));
        decay.tick(0.0);
        decay.tick(0.1); // half-decayed

        decay.start(5.0, 200.0, Some(10.0));
        decay.tick(0.0);
        // Restart: phase zero, full new amplitude on the y axis.
        assert!(
            approx(read_shake(&ctx), [0.0, 5.0]),
            "restart publishes the new amplitude at phase zero, got {:?}",
            read_shake(&ctx)
        );
    }

    #[test]
    fn idle_tick_writes_zero_once_then_short_circuits() {
        let ctx = ScriptCtx::new();
        let mut decay = ShakeDecay::new(ctx.clone());

        decay.tick(0.016);
        assert_eq!(read_shake(&ctx), [0.0, 0.0]);

        // Poke the slot directly; an idle tick must NOT overwrite it again.
        write_store_slot(&ctx, SHAKE_SLOT, SlotValue::Array(vec![3.0, 4.0])).unwrap();
        decay.tick(0.016);
        assert!(
            approx(read_shake(&ctx), [3.0, 4.0]),
            "idle tick short-circuits after the first zero write"
        );
    }

    #[test]
    fn reset_clears_active_shake() {
        let ctx = ScriptCtx::new();
        let mut decay = ShakeDecay::new(ctx.clone());
        decay.start(10.0, 1000.0, Some(10.0));
        decay.tick(0.0);
        decay.reset();
        // After reset, the next tick is the idle path (exact zero).
        decay.tick(0.016);
        assert_eq!(read_shake(&ctx), [0.0, 0.0]);
    }
}
