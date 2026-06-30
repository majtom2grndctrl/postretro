// App-side flash-decay state for the engine-owned `screen.flash` surface.
// A drained `FlashScreen` system-reaction command starts a flash (color +
// duration); each game-logic tick this writes `screen.flash` via the engine
// write path, fading the color's alpha to transparent over `duration_ms`. A
// full-screen panel bound to `screen.flash` renders the flash.
// See: context/lib/ui.md §3 · context/lib/scripting.md §10.4

use crate::scripting::primitives::store::write_store_slot;
use postretro_entities::ctx::ScriptCtx;
use postretro_entities::slot_table::SlotValue;

/// Dotted name of the engine-owned flash surface slot.
const FLASH_SLOT: &str = "screen.flash";

/// Fully transparent flash (linear RGBA). Written when no flash is active and as
/// the decay's resting endpoint.
const TRANSPARENT: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

/// One active flash: its start color (linear RGBA), total decay duration, and
/// how long it has been running. Pure decay math; no store, no GPU, no
/// wall-clock — `elapsed_ms` is advanced from the injected frame delta so the
/// fade is deterministic and unit-testable.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ActiveFlash {
    /// Linear RGBA at the moment the flash started (full intensity).
    start_color: [f32; 4],
    /// Total fade duration in milliseconds. A non-positive duration collapses to
    /// an instantaneous one-frame flash.
    duration_ms: f32,
    /// Milliseconds elapsed since the flash started.
    elapsed_ms: f32,
}

impl ActiveFlash {
    /// The current linear RGBA: the start color with its alpha scaled by the
    /// remaining fraction of the duration. Reaches transparent at `duration_ms`.
    fn current_color(&self) -> [f32; 4] {
        let remaining = if self.duration_ms > 0.0 {
            (1.0 - self.elapsed_ms / self.duration_ms).clamp(0.0, 1.0)
        } else {
            // Non-positive duration: full on the first frame (elapsed 0), then
            // transparent — a single-frame spike rather than a divide-by-zero.
            if self.elapsed_ms <= 0.0 { 1.0 } else { 0.0 }
        };
        [
            self.start_color[0],
            self.start_color[1],
            self.start_color[2],
            self.start_color[3] * remaining,
        ]
    }

    /// Whether the flash has fully decayed (alpha reached transparent).
    fn is_finished(&self) -> bool {
        self.elapsed_ms >= self.duration_ms
    }
}

/// Owns the active flash and writes `screen.flash` each game-logic tick.
///
/// Holds a clone of `App`'s `ScriptCtx` (cheap `Rc` bump). A drained
/// `FlashScreen` command calls [`FlashDecay::start`]; [`FlashDecay::tick`] runs
/// once per game-logic tick (beside the static UI proxy) to advance and publish
/// the decay. A new `start` mid-flight replaces the active flash (latest flash
/// wins), matching how an alarm flash should restart on a fresh trigger.
pub(crate) struct FlashDecay {
    ctx: ScriptCtx,
    active: Option<ActiveFlash>,
    /// True once the slot has been written to transparent after a flash ended,
    /// so an idle `FlashDecay` writes the resting transparent value exactly once
    /// rather than every frame in the per-frame hot path.
    wrote_idle: bool,
}

impl FlashDecay {
    /// Build a flash-decay state holding a clone of the engine's `ScriptCtx`.
    pub(crate) fn new(ctx: ScriptCtx) -> Self {
        Self {
            ctx,
            active: None,
            wrote_idle: false,
        }
    }

    /// Start (or restart) a flash from `color` decaying over `duration_ms`. The
    /// new flash replaces any in-flight flash so the latest trigger wins.
    pub(crate) fn start(&mut self, color: [f32; 4], duration_ms: f32) {
        self.active = Some(ActiveFlash {
            start_color: color,
            duration_ms,
            elapsed_ms: 0.0,
        });
        self.wrote_idle = false;
    }

    /// Clear any active flash and reset the idle latch. Called on level load so a
    /// flash never bleeds across levels.
    pub(crate) fn reset(&mut self) {
        self.active = None;
        self.wrote_idle = false;
    }

    /// Advance the active flash by the injected frame delta (seconds) and publish
    /// `screen.flash`. With no active flash, writes the resting transparent value
    /// once, then short-circuits subsequent idle frames.
    ///
    /// Runs at the game-logic stage (beside `ui_proxy.tick`), after game logic
    /// and before the UI read-snapshot build, so the snapshot freezes the flash
    /// color this same frame.
    pub(crate) fn tick(&mut self, dt: f32) {
        let Some(flash) = self.active.as_mut() else {
            // Idle: write transparent exactly once so the panel renders nothing,
            // then stop touching the slot every frame.
            if !self.wrote_idle {
                self.write(TRANSPARENT);
                self.wrote_idle = true;
            }
            return;
        };

        flash.elapsed_ms += dt * 1000.0;
        let color = flash.current_color();
        let finished = flash.is_finished();
        self.write(color);

        if finished {
            // The final write above already published transparent; drop the
            // active flash and mark idle so the next idle frame is a no-op.
            self.active = None;
            self.wrote_idle = true;
        }
    }

    /// Write `screen.flash` via the engine write path (bypasses readonly,
    /// validates the Array). An error here would be an engine bug (the slot is
    /// engine-owned and always declared), so it is logged rather than skipped.
    fn write(&self, color: [f32; 4]) {
        if let Err(err) = write_store_slot(&self.ctx, FLASH_SLOT, SlotValue::Array(color.to_vec()))
        {
            log::warn!("[FlashDecay] failed to write `{FLASH_SLOT}`: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::primitives::store::read_store_slot;

    fn read_flash(ctx: &ScriptCtx) -> [f32; 4] {
        match read_store_slot(ctx, FLASH_SLOT).unwrap() {
            SlotValue::Array(values) => {
                let mut out = [0.0; 4];
                out.copy_from_slice(&values[..4]);
                out
            }
            other => panic!("screen.flash should be an Array, got {other:?}"),
        }
    }

    fn approx(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn start_then_tick_writes_full_color_then_decays_to_transparent() {
        // A flashScreen command starts the flash; each tick fades the alpha to
        // transparent over duration_ms. Sample start, midpoint, and end.
        let ctx = ScriptCtx::new();
        let mut decay = FlashDecay::new(ctx.clone());
        let color = [1.0, 0.0, 0.0, 0.8];

        decay.start(color, 200.0);

        // First tick (dt = 0): the flash is at full intensity.
        decay.tick(0.0);
        assert!(
            approx(read_flash(&ctx), [1.0, 0.0, 0.0, 0.8]),
            "start of flash is the full color, got {:?}",
            read_flash(&ctx)
        );

        // Halfway through the 200 ms decay: alpha at half.
        decay.tick(0.1);
        assert!(
            approx(read_flash(&ctx), [1.0, 0.0, 0.0, 0.4]),
            "midpoint alpha is half, got {:?}",
            read_flash(&ctx)
        );

        // Past the duration: alpha decays to zero (fully transparent). The decay
        // fades only the alpha, so the RGB stays the start hue — a zero alpha
        // means the panel renders nothing regardless of the RGB channels.
        decay.tick(0.1);
        assert!(
            read_flash(&ctx)[3].abs() < 1e-4,
            "flash alpha decays to transparent at the duration, got {:?}",
            read_flash(&ctx)
        );
    }

    #[test]
    fn alpha_decreases_monotonically_over_the_decay() {
        let ctx = ScriptCtx::new();
        let mut decay = FlashDecay::new(ctx.clone());
        decay.start([0.2, 0.4, 0.6, 1.0], 300.0);

        decay.tick(0.0);
        let mut prev = read_flash(&ctx)[3];
        for _ in 0..6 {
            decay.tick(0.05);
            let cur = read_flash(&ctx)[3];
            assert!(
                cur <= prev + 1e-6,
                "alpha must be non-increasing: prev {prev}, cur {cur}"
            );
            prev = cur;
        }
        assert!(prev < 1e-4, "alpha settles at transparent, got {prev}");
    }

    #[test]
    fn restart_mid_flight_replaces_active_flash() {
        // A second flashScreen mid-decay restarts from the new color at full
        // intensity (latest trigger wins).
        let ctx = ScriptCtx::new();
        let mut decay = FlashDecay::new(ctx.clone());
        decay.start([1.0, 0.0, 0.0, 1.0], 200.0);
        decay.tick(0.0);
        decay.tick(0.1); // half-decayed

        decay.start([0.0, 1.0, 0.0, 1.0], 200.0);
        decay.tick(0.0);
        assert!(
            approx(read_flash(&ctx), [0.0, 1.0, 0.0, 1.0]),
            "restart publishes the new color at full intensity, got {:?}",
            read_flash(&ctx)
        );
    }

    #[test]
    fn idle_tick_writes_transparent_once_then_short_circuits() {
        // With no active flash the resting value is transparent, written once so
        // the per-frame hot path does not re-touch the slot each idle frame.
        let ctx = ScriptCtx::new();
        let mut decay = FlashDecay::new(ctx.clone());

        decay.tick(0.016);
        assert!(approx(read_flash(&ctx), [0.0, 0.0, 0.0, 0.0]));

        // Poke the slot directly; an idle tick must NOT overwrite it again.
        write_store_slot(&ctx, FLASH_SLOT, SlotValue::Array(vec![0.5, 0.5, 0.5, 0.5])).unwrap();
        decay.tick(0.016);
        assert!(
            approx(read_flash(&ctx), [0.5, 0.5, 0.5, 0.5]),
            "idle tick short-circuits after the first transparent write"
        );
    }

    #[test]
    fn reset_clears_active_flash() {
        let ctx = ScriptCtx::new();
        let mut decay = FlashDecay::new(ctx.clone());
        decay.start([1.0, 0.0, 0.0, 1.0], 1000.0);
        decay.tick(0.0);
        decay.reset();
        // After reset, the next tick is the idle path (transparent).
        decay.tick(0.016);
        assert!(approx(read_flash(&ctx), [0.0, 0.0, 0.0, 0.0]));
    }
}
