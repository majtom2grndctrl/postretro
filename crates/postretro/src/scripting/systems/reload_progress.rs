// Dev-only reload-progress producer for the HUD demonstrator.
// See: context/lib/input.md §2 · context/lib/scripting.md §5

use crate::input::{Action, ActionSnapshot, ButtonState};
use crate::scripting::primitives::store::write_store_slot;
use postretro_entities::ctx::ScriptCtx;
use postretro_entities::slot_table::SlotValue;

/// Dotted name of the engine-owned HUD slot this demonstrator drives.
const RELOAD_PROGRESS_SLOT: &str = "player.reloadProgress";

/// Fixed half-second ramp used only by the dev HUD demonstrator. Reload gameplay
/// will replace this producer with its own authored timing state.
const FIXED_DEV_DURATION_SECS: f32 = 0.5;

/// Dev-only producer for the `player.reloadProgress` HUD slot.
///
/// A fresh reload-button press starts or restarts the short ramp. Once the slot
/// has published `1.0` for one frame, the following tick publishes its resting
/// `0.0`. The progress accumulator is intentionally retained on this small
/// system so the input-to-store seam is inspectable without a renderer.
pub(crate) struct DevReloadProgressDriver {
    ctx: ScriptCtx,
    progress: f32,
    active: bool,
}

impl DevReloadProgressDriver {
    /// Build the dev-only driver with a clone of the engine's scripting context.
    pub(crate) fn new(ctx: ScriptCtx) -> Self {
        Self {
            ctx,
            progress: 0.0,
            active: false,
        }
    }

    /// Advance the reload meter from the fixed-gameplay input snapshot and the
    /// render-frame delta. `None` is a zero-fixed-tick frame: an active meter
    /// keeps advancing, but no new reload press is observed until the input latch
    /// yields its gameplay snapshot.
    pub(crate) fn tick(&mut self, gameplay_snapshot: Option<&ActionSnapshot>, frame_dt: f32) {
        let reload_pressed = gameplay_snapshot.is_some_and(|snapshot| {
            matches!(snapshot.button(Action::Reload), ButtonState::Pressed)
        });

        if reload_pressed {
            self.progress = 0.0;
            self.active = true;
        }

        if !self.active {
            return;
        }

        if self.progress >= 1.0 {
            self.progress = 0.0;
            self.active = false;
            self.write_progress();
            return;
        }

        self.progress = (self.progress + frame_dt / FIXED_DEV_DURATION_SECS).clamp(0.0, 1.0);
        self.write_progress();
    }

    /// Write through the engine path, which intentionally bypasses script-side
    /// readonly gating while still applying the slot's numeric validation.
    fn write_progress(&self) {
        if let Err(err) = write_store_slot(
            &self.ctx,
            RELOAD_PROGRESS_SLOT,
            SlotValue::Number(self.progress),
        ) {
            log::warn!("[ReloadProgress] failed to write `{RELOAD_PROGRESS_SLOT}`: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::primitives::store::read_store_slot;

    fn reload_progress(ctx: &ScriptCtx) -> f32 {
        match read_store_slot(ctx, RELOAD_PROGRESS_SLOT).unwrap() {
            SlotValue::Number(progress) => progress,
            other => panic!("{RELOAD_PROGRESS_SLOT} should be a Number, got {other:?}"),
        }
    }

    fn assert_approx_eq(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= 0.000_001,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn reload_press_ramps_to_full_then_resets_the_store_slot() {
        let ctx = ScriptCtx::new();
        let mut driver = DevReloadProgressDriver::new(ctx.clone());
        let pressed = ActionSnapshot::with_button_state(Action::Reload, ButtonState::Pressed);
        let held = ActionSnapshot::with_button_state(Action::Reload, ButtonState::Held);

        driver.tick(Some(&pressed), FIXED_DEV_DURATION_SECS * 0.25);
        assert_approx_eq(reload_progress(&ctx), 0.25);
        assert_approx_eq(driver.progress, 0.25);

        // Held is a level signal, not a new ramp trigger: the remaining delta
        // completes the existing ramp instead of restarting it.
        driver.tick(Some(&held), FIXED_DEV_DURATION_SECS * 0.75);
        assert_approx_eq(reload_progress(&ctx), 1.0);
        assert_approx_eq(driver.progress, 1.0);

        // Keep the full value visible for one frame, then return the HUD slot to
        // its default resting value on the following tick.
        driver.tick(Some(&held), 0.0);
        assert_approx_eq(reload_progress(&ctx), 0.0);
        assert_approx_eq(driver.progress, 0.0);
    }

    #[test]
    fn fresh_reload_press_restarts_an_active_ramp() {
        let ctx = ScriptCtx::new();
        let mut driver = DevReloadProgressDriver::new(ctx.clone());
        let pressed = ActionSnapshot::with_button_state(Action::Reload, ButtonState::Pressed);

        driver.tick(Some(&pressed), FIXED_DEV_DURATION_SECS * 0.5);
        assert_approx_eq(reload_progress(&ctx), 0.5);

        driver.tick(Some(&pressed), FIXED_DEV_DURATION_SECS * 0.25);
        assert_approx_eq(reload_progress(&ctx), 0.25);
        assert_approx_eq(driver.progress, 0.25);
    }
}
