// App-side vignette-decay state for the engine-owned `screen.vignette` surface.
// The screen-effects resolve pass (render/screen_effects.rs) consumes this slot
// each frame; at rest the slot is `[0,0,0,0]` so the consumer collapses to identity.
// See: context/lib/ui.md §3 · context/lib/scripting.md §10.4

use crate::scripting::primitives::store::write_store_slot;
use postretro_entities::ctx::ScriptCtx;
use postretro_entities::slot_table::SlotValue;

/// Dotted name of the engine-owned vignette surface slot.
const VIGNETTE_SLOT: &str = "screen.vignette";

/// At-rest vignette (linear RGBA, `a` = strength). Written when no vignette is
/// active and as the envelope's resting endpoint so the GPU consumer is identity.
const REST: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

/// One active vignette envelope: its tint (linear RGB), peak strength, the
/// rise/decay shape, and how long it has been running. Pure envelope math; no
/// store, no GPU, no wall-clock — `elapsed_ms` is advanced from the injected
/// frame delta so the envelope is deterministic and unit-testable.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ActiveVignette {
    /// Linear RGB tint held constant across the envelope.
    tint: [f32; 3],
    /// Strength at the peak (the envelope's `a` maximum). Clamped to `[0, +∞)`.
    peak: f32,
    /// Rise duration in milliseconds: strength ramps `0 → peak` over this span. A
    /// non-positive rise snaps to peak on the first frame.
    rise_ms: f32,
    /// Decay duration in milliseconds: strength ramps `peak → 0` after the rise. A
    /// non-positive decay collapses the envelope to its rise (then rest).
    decay_ms: f32,
    /// Milliseconds elapsed since the envelope started.
    elapsed_ms: f32,
}

impl ActiveVignette {
    /// The current strength (the RGBA `a`) sampled from the rise/peak/decay
    /// envelope. Rises linearly `0 → peak` over `rise_ms`, then decays linearly
    /// `peak → 0` over `decay_ms`, reaching exactly zero at the total duration.
    fn current_strength(&self) -> f32 {
        let peak = self.peak.max(0.0);
        if self.elapsed_ms < self.rise_ms {
            // Rising. (rise_ms > 0 here, since elapsed_ms >= 0 cannot be < a
            // non-positive rise_ms.)
            let t = (self.elapsed_ms / self.rise_ms).clamp(0.0, 1.0);
            peak * t
        } else {
            // At or past the peak: decay from peak to zero.
            let into_decay = self.elapsed_ms - self.rise_ms;
            if self.decay_ms > 0.0 {
                let t = (into_decay / self.decay_ms).clamp(0.0, 1.0);
                peak * (1.0 - t)
            } else {
                // No decay span: a single-frame peak spike, then rest.
                if into_decay <= 0.0 { peak } else { 0.0 }
            }
        }
    }

    /// The current linear RGBA: the constant tint with the enveloped strength as
    /// alpha.
    fn current_color(&self) -> [f32; 4] {
        let a = self.current_strength();
        [self.tint[0], self.tint[1], self.tint[2], a]
    }

    /// Whether the envelope has fully decayed (strength reached zero).
    fn is_finished(&self) -> bool {
        self.elapsed_ms >= self.rise_ms + self.decay_ms
    }
}

/// Owns the active vignette envelope and writes `screen.vignette` each
/// game-logic tick.
///
/// Holds a clone of `App`'s `ScriptCtx` (cheap `Rc` bump). A drained vignette
/// command calls [`VignetteDecay::start`]; [`VignetteDecay::tick`] runs once per
/// game-logic tick (beside `FlashDecay::tick`) to advance and publish the
/// envelope. A new `start` mid-flight replaces the active envelope (latest wins),
/// matching how `FlashDecay` restarts on a fresh trigger.
pub(crate) struct VignetteDecay {
    ctx: ScriptCtx,
    active: Option<ActiveVignette>,
    /// True once the slot has been written to rest after an envelope ended, so an
    /// idle `VignetteDecay` writes the resting value exactly once rather than
    /// every frame in the per-frame hot path.
    wrote_idle: bool,
}

impl VignetteDecay {
    /// Build a vignette-decay state holding a clone of the engine's `ScriptCtx`.
    pub(crate) fn new(ctx: ScriptCtx) -> Self {
        Self {
            ctx,
            active: None,
            wrote_idle: false,
        }
    }

    /// Start (or restart) a vignette envelope: a `tint` (linear RGB) at `peak`
    /// strength, rising over `rise_ms` then decaying over `decay_ms`. The new
    /// envelope replaces any in-flight one so the latest trigger wins.
    pub(crate) fn start(&mut self, tint: [f32; 3], peak: f32, rise_ms: f32, decay_ms: f32) {
        self.active = Some(ActiveVignette {
            tint,
            peak,
            rise_ms,
            decay_ms,
            elapsed_ms: 0.0,
        });
        self.wrote_idle = false;
    }

    /// Clear any active vignette and reset the idle latch. Called on level load so
    /// a vignette never bleeds across levels.
    pub(crate) fn reset(&mut self) {
        self.active = None;
        self.wrote_idle = false;
    }

    /// Advance the active envelope by the injected frame delta (seconds) and
    /// publish `screen.vignette`. With no active envelope, writes the resting
    /// value once, then short-circuits subsequent idle frames.
    ///
    /// Runs at the game-logic stage (beside `FlashDecay::tick`), after game logic
    /// and before the UI read-snapshot build, so the snapshot freezes the
    /// vignette color this same frame.
    pub(crate) fn tick(&mut self, dt: f32) {
        let Some(vignette) = self.active.as_mut() else {
            // Idle: write rest exactly once so the GPU consumer is identity, then
            // stop touching the slot every frame.
            if !self.wrote_idle {
                self.write(REST);
                self.wrote_idle = true;
            }
            return;
        };

        vignette.elapsed_ms += dt * 1000.0;
        let color = vignette.current_color();
        let finished = vignette.is_finished();
        self.write(color);

        if finished {
            // The final write above already published rest (strength zero); drop
            // the active envelope and mark idle so the next idle frame is a no-op.
            self.active = None;
            self.wrote_idle = true;
        }
    }

    /// Write `screen.vignette` via the engine write path (bypasses readonly,
    /// validates the Array). An error here would be an engine bug (the slot is
    /// engine-owned and always declared), so it is logged rather than skipped.
    fn write(&self, color: [f32; 4]) {
        if let Err(err) =
            write_store_slot(&self.ctx, VIGNETTE_SLOT, SlotValue::Array(color.to_vec()))
        {
            log::warn!("[VignetteDecay] failed to write `{VIGNETTE_SLOT}`: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::primitives::store::read_store_slot;

    fn read_vignette(ctx: &ScriptCtx) -> [f32; 4] {
        match read_store_slot(ctx, VIGNETTE_SLOT).unwrap() {
            SlotValue::Array(values) => {
                let mut out = [0.0; 4];
                out.copy_from_slice(&values[..4]);
                out
            }
            other => panic!("screen.vignette should be an Array, got {other:?}"),
        }
    }

    fn approx(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn envelope_rises_to_peak_then_decays_to_rest() {
        // The envelope ramps strength 0 → peak over rise_ms, then peak → 0 over
        // decay_ms. Sample start, peak, and end. Tint stays constant; only the
        // alpha (strength) moves.
        let ctx = ScriptCtx::new();
        let mut decay = VignetteDecay::new(ctx.clone());
        let tint = [0.1, 0.0, 0.2];

        decay.start(tint, 0.8, 100.0, 200.0);

        // First tick (dt = 0): start of the rise, strength zero.
        decay.tick(0.0);
        assert!(
            approx(read_vignette(&ctx), [0.1, 0.0, 0.2, 0.0]),
            "start of rise has zero strength, got {:?}",
            read_vignette(&ctx)
        );

        // End of the 100 ms rise: strength at peak.
        decay.tick(0.1);
        assert!(
            approx(read_vignette(&ctx), [0.1, 0.0, 0.2, 0.8]),
            "rise reaches peak strength, got {:?}",
            read_vignette(&ctx)
        );

        // Halfway through the 200 ms decay: strength at half peak.
        decay.tick(0.1);
        assert!(
            approx(read_vignette(&ctx), [0.1, 0.0, 0.2, 0.4]),
            "decay midpoint is half peak, got {:?}",
            read_vignette(&ctx)
        );

        // Past the total duration: strength settles to rest (zero).
        decay.tick(0.1);
        assert!(
            read_vignette(&ctx)[3].abs() < 1e-4,
            "envelope settles to zero strength, got {:?}",
            read_vignette(&ctx)
        );
    }

    #[test]
    fn strength_is_non_increasing_after_the_peak() {
        // Property: once past the peak, the enveloped strength never rises again.
        let ctx = ScriptCtx::new();
        let mut decay = VignetteDecay::new(ctx.clone());
        decay.start([0.2, 0.4, 0.6], 1.0, 50.0, 300.0);

        // Advance to the peak.
        decay.tick(0.0);
        decay.tick(0.05);
        let mut prev = read_vignette(&ctx)[3];
        assert!((prev - 1.0).abs() < 1e-4, "should be at peak, got {prev}");

        for _ in 0..8 {
            decay.tick(0.05);
            let cur = read_vignette(&ctx)[3];
            assert!(
                cur <= prev + 1e-6,
                "strength must be non-increasing after peak: prev {prev}, cur {cur}"
            );
            prev = cur;
        }
        assert!(prev < 1e-4, "strength settles at rest, got {prev}");
    }

    #[test]
    fn restart_mid_flight_replaces_active_envelope() {
        // A second vignette mid-envelope restarts from the new tint/peak (latest
        // trigger wins).
        let ctx = ScriptCtx::new();
        let mut decay = VignetteDecay::new(ctx.clone());
        decay.start([1.0, 0.0, 0.0], 1.0, 0.0, 200.0);
        decay.tick(0.0); // snaps to peak (zero rise)
        decay.tick(0.1); // half-decayed

        decay.start([0.0, 1.0, 0.0], 0.5, 0.0, 200.0);
        decay.tick(0.0);
        assert!(
            approx(read_vignette(&ctx), [0.0, 1.0, 0.0, 0.5]),
            "restart publishes the new envelope at peak, got {:?}",
            read_vignette(&ctx)
        );
    }

    #[test]
    fn idle_tick_writes_rest_once_then_short_circuits() {
        // With no active envelope the resting value is [0,0,0,0], written once so
        // the per-frame hot path does not re-touch the slot each idle frame.
        let ctx = ScriptCtx::new();
        let mut decay = VignetteDecay::new(ctx.clone());

        decay.tick(0.016);
        assert!(approx(read_vignette(&ctx), [0.0, 0.0, 0.0, 0.0]));

        // Poke the slot directly; an idle tick must NOT overwrite it again.
        write_store_slot(
            &ctx,
            VIGNETTE_SLOT,
            SlotValue::Array(vec![0.5, 0.5, 0.5, 0.5]),
        )
        .unwrap();
        decay.tick(0.016);
        assert!(
            approx(read_vignette(&ctx), [0.5, 0.5, 0.5, 0.5]),
            "idle tick short-circuits after the first rest write"
        );
    }

    #[test]
    fn reset_clears_active_envelope() {
        let ctx = ScriptCtx::new();
        let mut decay = VignetteDecay::new(ctx.clone());
        decay.start([1.0, 0.0, 0.0], 1.0, 100.0, 1000.0);
        decay.tick(0.0);
        decay.reset();
        // After reset, the next tick is the idle path (rest).
        decay.tick(0.016);
        assert!(approx(read_vignette(&ctx), [0.0, 0.0, 0.0, 0.0]));
    }
}
