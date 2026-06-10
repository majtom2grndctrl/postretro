// Engine-side static UI proxy: the stand-in producer that writes HUD store
// slots until real game logic (M10 entity health) feeds them. It owns a clone
// of `App`'s `ScriptCtx` and, each frame, writes the engine-owned `player.*`
// slots and a level-load-timed `intro.flashColor` through the store's engine
// write path (`write_store_slot`).
//
// This is the producer half of the Goal C UI-decoupling seam: HUD widgets bind
// to store slots like `player.health`; this proxy publishes demo values into
// those slots with no compile-time dependency on the widget side, and real
// game logic replaces it later without touching the widgets.
//
// See: context/lib/scripting.md §5 "Durable State Store"

use crate::scripting::ctx::ScriptCtx;
use crate::scripting::primitives::store::write_store_slot;
use crate::scripting::slot_table::SlotValue;

/// Stand-in demo health published into the readonly engine-owned
/// `player.health` slot every frame. Replaced by M10 entity health later.
const DEMO_HEALTH: f32 = 100.0;

/// Stand-in demo ammo published into `player.ammo` every frame.
const DEMO_AMMO: f32 = 50.0;

/// Dotted name of the mod-declared flash slot. Absent until the demo mod
/// (Task 5) declares the `intro` namespace; the proxy tolerates its absence.
const FLASH_SLOT: &str = "intro.flashColor";

/// Half-period of the flash toggle. The color alternates between the two
/// endpoints every 500 ms.
const FLASH_TOGGLE_MS: f32 = 500.0;

/// How long after level load the flash animates. Past this the color holds the
/// solid (non-flash) endpoint.
const FLASH_DURATION_MS: f32 = 3000.0;

/// Solid (non-flash) RGBA endpoint, held after the flash window ends and shown
/// on even toggle intervals. Subtle cyberpunk cyan.
const FLASH_SOLID: [f32; 4] = [0.0, 0.65, 0.75, 1.0];

/// Flash RGBA endpoint, shown on odd toggle intervals. Same hue as the solid
/// endpoint, only slightly brighter — a subtle pulse, not a strobe.
const FLASH_PULSE: [f32; 4] = [0.0, 0.80, 0.90, 1.0];

/// Compute the `intro.flashColor` RGBA for a given elapsed time since level
/// load. Pure: no store, no GPU, no wall-clock — the toggle/hold logic is
/// driven entirely by `elapsed_ms`, so it is deterministic and unit-testable.
///
/// For the first `FLASH_DURATION_MS` the color toggles between [`FLASH_SOLID`]
/// and [`FLASH_PULSE`] every `FLASH_TOGGLE_MS`, starting on the solid endpoint.
/// After the window it holds [`FLASH_SOLID`].
fn flash_color_at(elapsed_ms: f32) -> [f32; 4] {
    if elapsed_ms >= FLASH_DURATION_MS {
        return FLASH_SOLID;
    }
    // Even interval (0, 2, …) → solid; odd interval (1, 3, …) → pulse.
    let interval = (elapsed_ms / FLASH_TOGGLE_MS) as u32;
    if interval % 2 == 0 {
        FLASH_SOLID
    } else {
        FLASH_PULSE
    }
}

/// Engine-side stand-in producer for the HUD store slots.
///
/// Owns a clone of `App`'s `ScriptCtx` (cheap `Rc` bump) and an injected-dt
/// timer. Constructed during `App` setup; its timer is reset on every level
/// load via [`StaticUiProxy::reset_timer`].
pub(crate) struct StaticUiProxy {
    ctx: ScriptCtx,

    /// Milliseconds since the current level loaded. Advanced from the injected
    /// frame delta in [`StaticUiProxy::tick`], never wall-clock, so the flash
    /// animation is deterministic and testable. Reset to zero on level load.
    elapsed_ms: f32,

    /// Latches once the `intro.flashColor` write has failed (slot absent) so the
    /// warning is logged at most once, not every frame — the proxy runs in the
    /// per-frame hot path. See: development_guide.md §6.1.
    flash_warned: bool,
}

impl StaticUiProxy {
    /// Build a proxy holding a clone of the engine's `ScriptCtx`. The timer
    /// starts at zero; the first level load resets it.
    pub(crate) fn new(ctx: ScriptCtx) -> Self {
        Self {
            ctx,
            elapsed_ms: 0.0,
            flash_warned: false,
        }
    }

    /// Restart the level-load flash timer. Called from the level-load path so
    /// the flash animation replays from the start each time a level loads.
    pub(crate) fn reset_timer(&mut self) {
        self.elapsed_ms = 0.0;
    }

    /// Advance the timer by the injected frame delta (seconds) and republish the
    /// store slots for this frame.
    ///
    /// Always writes `player.health` / `player.ammo` (engine-owned, always
    /// declared). Writes `intro.flashColor` only when the slot exists; when the
    /// demo mod has not declared it the write returns `Err` and is skipped with
    /// a single warning.
    ///
    /// Runs in the frame loop after game logic and before the UI read-snapshot
    /// build, so the snapshot picks up these values the same frame.
    pub(crate) fn tick(&mut self, dt: f32) {
        self.elapsed_ms += dt * 1000.0;

        // `player.*` are engine-owned and always declared, so these writes
        // succeed; an error here would be a real bug, hence no skip-with-warn.
        if let Err(err) =
            write_store_slot(&self.ctx, "player.health", SlotValue::Number(DEMO_HEALTH))
        {
            log::warn!("[Proxy] failed to write player.health: {err}");
        }
        if let Err(err) = write_store_slot(&self.ctx, "player.ammo", SlotValue::Number(DEMO_AMMO)) {
            log::warn!("[Proxy] failed to write player.ammo: {err}");
        }

        let flash = flash_color_at(self.elapsed_ms);
        if let Err(err) = write_store_slot(&self.ctx, FLASH_SLOT, SlotValue::Array(flash.to_vec()))
        {
            // Expected until the demo mod declares `intro` (Task 5). Warn once
            // so the hot path does not spam the log every frame.
            if !self.flash_warned {
                log::warn!(
                    "[Proxy] skipping `{FLASH_SLOT}` writes; slot not declared (demo mod absent): {err}"
                );
                self.flash_warned = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flash_starts_on_solid_endpoint() {
        assert_eq!(flash_color_at(0.0), FLASH_SOLID);
    }

    #[test]
    fn flash_toggles_every_500ms_within_window() {
        // Sample the middle of each 500 ms interval across the 3000 ms window:
        // even intervals hold solid, odd intervals hold the pulse endpoint.
        for interval in 0..6 {
            let mid = interval as f32 * FLASH_TOGGLE_MS + FLASH_TOGGLE_MS / 2.0;
            let expected = if interval % 2 == 0 {
                FLASH_SOLID
            } else {
                FLASH_PULSE
            };
            assert_eq!(
                flash_color_at(mid),
                expected,
                "interval {interval} at {mid}ms"
            );
        }
    }

    #[test]
    fn flash_switches_exactly_at_toggle_boundaries() {
        // Just before 500 ms is still the solid (first) interval; at 500 ms the
        // toggle crosses to the pulse endpoint.
        assert_eq!(flash_color_at(FLASH_TOGGLE_MS - 1.0), FLASH_SOLID);
        assert_eq!(flash_color_at(FLASH_TOGGLE_MS), FLASH_PULSE);
        assert_eq!(flash_color_at(2.0 * FLASH_TOGGLE_MS), FLASH_SOLID);
    }

    #[test]
    fn flash_holds_solid_after_window() {
        // At and after 3000 ms the animation stops on the solid endpoint, even
        // on what would have been an odd (pulse) interval.
        assert_eq!(flash_color_at(FLASH_DURATION_MS), FLASH_SOLID);
        assert_eq!(
            flash_color_at(FLASH_DURATION_MS + FLASH_TOGGLE_MS),
            FLASH_SOLID
        );
        assert_eq!(flash_color_at(10_000.0), FLASH_SOLID);
    }

    #[test]
    fn endpoints_are_valid_linear_rgba() {
        for endpoint in [FLASH_SOLID, FLASH_PULSE] {
            assert_eq!(endpoint.len(), 4);
            assert!(endpoint.iter().all(|c| (0.0..=1.0).contains(c)));
        }
    }

    #[test]
    fn endpoints_share_hue_and_differ_subtly() {
        // Same-hue subtle flash: red and alpha match; green/blue differ only
        // slightly between the two endpoints.
        assert_eq!(FLASH_SOLID[0], FLASH_PULSE[0]);
        assert_eq!(FLASH_SOLID[3], FLASH_PULSE[3]);
        for channel in 1..=2 {
            let delta = (FLASH_SOLID[channel] - FLASH_PULSE[channel]).abs();
            assert!(
                delta > 0.0 && delta <= 0.2,
                "channel {channel} delta {delta}"
            );
        }
    }

    #[test]
    fn tick_advances_timer_and_writes_player_slots() {
        use crate::scripting::primitives::store::read_store_slot;

        let ctx = ScriptCtx::new();
        let mut proxy = StaticUiProxy::new(ctx.clone());

        // player.* start with no value; one tick publishes the demo constants.
        proxy.tick(0.016);
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(DEMO_HEALTH)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.ammo").unwrap(),
            SlotValue::Number(DEMO_AMMO)
        );

        // dt is in seconds; the timer accumulates milliseconds.
        assert!((proxy.elapsed_ms - 16.0).abs() < 1e-3);
    }

    #[test]
    fn reset_timer_replays_flash_from_start() {
        let ctx = ScriptCtx::new();
        let mut proxy = StaticUiProxy::new(ctx);
        // Advance past the flash window.
        proxy.tick(4.0);
        assert!(proxy.elapsed_ms >= FLASH_DURATION_MS);
        proxy.reset_timer();
        assert_eq!(proxy.elapsed_ms, 0.0);
    }

    #[test]
    fn missing_flash_slot_warns_once_and_keeps_writing_player_slots() {
        use crate::scripting::primitives::store::read_store_slot;

        // Default ScriptCtx has no `intro` namespace, so flashColor writes fail.
        let ctx = ScriptCtx::new();
        let mut proxy = StaticUiProxy::new(ctx.clone());

        proxy.tick(0.016);
        assert!(
            proxy.flash_warned,
            "first failed flash write latches the warn"
        );

        // Subsequent ticks must not re-arm the latch, and player.* keep updating.
        proxy.tick(0.016);
        assert!(proxy.flash_warned);
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(DEMO_HEALTH)
        );
        assert!(read_store_slot(&ctx, "intro.flashColor").is_err());
    }
}
