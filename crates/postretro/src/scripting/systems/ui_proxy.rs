// Slot producer for the HUD store slots. Publishes the live pawn HP into the
// readonly engine-owned `player.health` slot each frame. `player.ammo` is a
// proxy stand-in; `intro.flashColor` is driven by a timer animation this proxy
// owns — its real producer, not a placeholder.
// See: context/lib/scripting.md §5 "Durable State Store"

use crate::scripting::components::health::pawn_with_health;
use crate::scripting::ctx::ScriptCtx;
use crate::scripting::primitives::store::write_store_slot;
use crate::scripting::registry::EntityRegistry;
use crate::scripting::slot_table::SlotValue;

/// Stand-in demo ammo published into `player.ammo` every frame. The ammo
/// producer is a separate future task; this remains a proxy stand-in.
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

/// Read the current HP of the player pawn — the first entity carrying
/// `PlayerMovement` (entity_model.md: "a player by virtue of carrying
/// `PlayerMovement`"). Returns `None` when there is no pawn or the pawn carries
/// no `Health` component; the caller then skips the `player.health` write and
/// the slot keeps its last value (accepted slot-staleness contract).
///
/// Pure read against the registry: no slot table, no GPU, so it is unit-testable
/// without the proxy's `ScriptCtx`.
fn pawn_health_current(registry: &EntityRegistry) -> Option<f32> {
    pawn_with_health(registry).map(|(_, health)| health.current)
}

/// Engine-side producer for the HUD store slots.
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
    /// Publishes the live pawn HP into `player.health` when a pawn with a
    /// `Health` component exists; with no pawn or no health component the write
    /// is skipped and the slot keeps its last value (accepted slot-staleness
    /// contract). Always writes `player.ammo` (engine-owned demo stand-in).
    /// Writes `intro.flashColor` only when the slot exists; when the demo mod
    /// has not declared it the write returns `Err` and is skipped with a single
    /// warning.
    ///
    /// Runs in the frame loop after game logic and before the UI read-snapshot
    /// build, so the snapshot picks up these values the same frame.
    pub(crate) fn tick(&mut self, dt: f32) {
        self.elapsed_ms += dt * 1000.0;

        // `player.health` mirrors the live pawn HP. No pawn / no health
        // component → skip; the readonly slot retains its previous value. The
        // registry borrow is scoped to the read so it drops before the
        // `write_store_slot` (which borrows the slot table, a separate cell).
        let pawn_hp = pawn_health_current(&self.ctx.registry.borrow());
        if let Some(current) = pawn_hp {
            // Engine-owned and always declared, so this write succeeds; an error
            // here would be a real bug, hence no skip-with-warn.
            if let Err(err) =
                write_store_slot(&self.ctx, "player.health", SlotValue::Number(current))
            {
                log::warn!("[Proxy] failed to write player.health: {err}");
            }
        }
        // `player.ammo` is engine-owned and always declared, so this write
        // succeeds; an error here would be a real bug, hence no skip-with-warn.
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
    use crate::scripting::components::health::HealthComponent;
    use crate::scripting::components::player_movement::PlayerMovementComponent;
    use crate::scripting::data_descriptors::{
        AirParams, CapsuleParams, FallParams, GroundParams, HealthDescriptor,
        PlayerMovementDescriptor, SpeedParams,
    };
    use crate::scripting::registry::{EntityId, Transform};

    /// A minimal movement descriptor so a spawned entity qualifies as the pawn
    /// (carries `PlayerMovement`). Only the fields `from_descriptor` reads need
    /// to be sane for this test's purpose.
    fn movement_descriptor() -> PlayerMovementDescriptor {
        PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.35,
                half_height: 0.9,
                eye_height: 1.1,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                    crouch: 3.0,
                },
                accel: 12.0,
                step_height: 0.35,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 0.3,
                accel: 2.0,
                max_control_speed: 4.0,
                bunny_hop: true,
                jumps: 1,
                jump_velocity: 5.0,
                jump_ceiling: 2.0,
            },
            fall: FallParams {
                terminal_velocity: 50.0,
            },
            stuck_stop_enabled: true,
            stuck_stop_threshold: 0.001,
            dash: None,
            forgiveness: None,
            crouch: None,
            view_feel: None,
        }
    }

    /// Spawn a pawn (carries `PlayerMovement`) with a `Health` component whose
    /// `current` HP is `current`. Returns the pawn id.
    fn spawn_pawn_with_health(ctx: &ScriptCtx, current: f32) -> EntityId {
        let mut registry = ctx.registry.borrow_mut();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                PlayerMovementComponent::from_descriptor(&movement_descriptor()),
            )
            .unwrap();
        let mut health = HealthComponent::from_descriptor(&HealthDescriptor {
            max: 100.0,
            hitbox: None,
        });
        health.current = current;
        registry.set_component(id, health).unwrap();
        id
    }

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
    fn tick_advances_timer_and_publishes_live_pawn_health() {
        use crate::scripting::primitives::store::read_store_slot;

        let ctx = ScriptCtx::new();
        spawn_pawn_with_health(&ctx, 73.0);
        let mut proxy = StaticUiProxy::new(ctx.clone());

        // player.* start with no value; one tick publishes the live pawn HP and
        // the demo ammo constant.
        proxy.tick(0.016);
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(73.0)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.ammo").unwrap(),
            SlotValue::Number(DEMO_AMMO)
        );

        // dt is in seconds; the timer accumulates milliseconds.
        assert!((proxy.elapsed_ms - 16.0).abs() < 1e-3);
    }

    #[test]
    fn tick_tracks_pawn_hp_frame_over_frame() {
        use crate::scripting::primitives::store::read_store_slot;

        // The producer republishes the live pawn HP each frame, so a damage
        // mutation between ticks shows up in the slot the next frame (the M13
        // HUD readout would then show the new value).
        let ctx = ScriptCtx::new();
        let pawn = spawn_pawn_with_health(&ctx, 100.0);
        let mut proxy = StaticUiProxy::new(ctx.clone());

        proxy.tick(0.016);
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(100.0)
        );

        // Mutate the live HP, then tick again: the slot follows.
        {
            let mut registry = ctx.registry.borrow_mut();
            let mut health = *registry.get_component::<HealthComponent>(pawn).unwrap();
            health.current = 40.0;
            registry.set_component(pawn, health).unwrap();
        }
        proxy.tick(0.016);
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(40.0)
        );
    }

    #[test]
    fn tick_skips_health_write_with_no_pawn_keeping_last_value() {
        use crate::scripting::primitives::store::read_store_slot;
        use crate::scripting::primitives::store::write_store_slot;

        // Slot-staleness contract: with no pawn the producer skips the health
        // write entirely, so the slot keeps whatever value it last held.
        let ctx = ScriptCtx::new();
        write_store_slot(&ctx, "player.health", SlotValue::Number(55.0)).unwrap();
        let mut proxy = StaticUiProxy::new(ctx.clone());

        proxy.tick(0.016);
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(55.0),
            "no pawn → health slot unchanged"
        );
    }

    #[test]
    fn pawn_health_current_none_without_pawn_or_health_component() {
        // No entities at all → None.
        let empty = ScriptCtx::new();
        assert_eq!(pawn_health_current(&empty.registry.borrow()), None);

        // A pawn without a Health component → None.
        let no_health = ScriptCtx::new();
        {
            let mut registry = no_health.registry.borrow_mut();
            let id = registry.spawn(Transform::default());
            registry
                .set_component(
                    id,
                    PlayerMovementComponent::from_descriptor(&movement_descriptor()),
                )
                .unwrap();
        }
        assert_eq!(pawn_health_current(&no_health.registry.borrow()), None);

        // A pawn carrying Health → reads its current HP.
        let with_health = ScriptCtx::new();
        spawn_pawn_with_health(&with_health, 88.0);
        assert_eq!(
            pawn_health_current(&with_health.registry.borrow()),
            Some(88.0)
        );
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
        spawn_pawn_with_health(&ctx, 64.0);
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
            SlotValue::Number(64.0)
        );
        assert!(read_store_slot(&ctx, "intro.flashColor").is_err());
    }
}
