// Player HUD state publisher. Publishes live pawn HP into the readonly
// engine-owned `player.health` and `player.maxHealth` slots each frame.
// See: context/lib/scripting.md §5 "Durable State Store"

use crate::scripting::components::health::pawn_with_health;
use crate::scripting::ctx::ScriptCtx;
use crate::scripting::primitives::store::write_store_slot;
use crate::scripting::registry::{EntityId, EntityRegistry};
use crate::scripting::slot_table::SlotValue;

/// Read the current and maximum HP of the player pawn resolved by the local
/// player marker, with legacy fallback to the first entity carrying
/// `PlayerMovement`. Returns `None` when there is no pawn or the pawn carries no
/// `Health` component; the caller then skips the `player.*Health` writes and the
/// slots keep their last values (accepted slot-staleness contract).
///
/// Pure read against the registry: no slot table, no GPU, so it is unit-testable
/// without the publisher's `ScriptCtx`.
fn pawn_health_values(registry: &EntityRegistry) -> Option<(EntityId, f32, f32)> {
    pawn_with_health(registry).map(|(id, health)| (id, health.current, health.max))
}

/// Engine-side producer for the HUD store slots.
pub(crate) struct PlayerHudStatePublisher {
    ctx: ScriptCtx,
    invalid_max_warned_for: Option<EntityId>,
}

impl PlayerHudStatePublisher {
    /// Build a publisher holding a clone of the engine's `ScriptCtx`.
    pub(crate) fn new(ctx: ScriptCtx) -> Self {
        Self {
            ctx,
            invalid_max_warned_for: None,
        }
    }

    /// Republish the player HUD store slots for this frame unless this endpoint is
    /// a connected client.
    ///
    /// M15 Phase 3.5 Task 4: `player.health` / `player.maxHealth` are owner-private
    /// replicated slots. On a connected client the server writes them through the
    /// state-slot apply path, so the local (non-authoritative) publisher must not
    /// overwrite them. The host and single-player keep publishing as before. The
    /// `is_connected_client` decision is owned by the `main.rs` call site (the
    /// `NetEndpoint` role lives there); this method keeps the gate testable without
    /// an `App`.
    pub(crate) fn tick_for_role(&mut self, is_connected_client: bool) {
        if is_connected_client {
            return;
        }
        self.tick();
    }

    /// Republish the player HUD store slots for this frame.
    ///
    /// Publishes the live pawn HP into `player.health` and max HP into
    /// `player.maxHealth` when a pawn with a `Health` component exists; with no
    /// pawn or no health component the writes are skipped and the slots keep
    /// their last values (accepted slot-staleness contract). If corrupt live
    /// data carries an invalid max, current HP is still published but max HP is
    /// skipped so the store's `[1, +∞)` range never silently repairs it.
    ///
    /// Runs in the frame loop after game logic and before the UI read-snapshot
    /// build, so the snapshot picks up these values the same frame.
    pub(crate) fn tick(&mut self) {
        // `player.health`/`player.maxHealth` mirror the live pawn HP. No pawn /
        // no health component → skip; the readonly slots retain their previous
        // values. The registry borrow is scoped to the read so it drops before
        // the `write_store_slot` calls (which borrow the slot table, a separate
        // cell).
        let pawn_health = pawn_health_values(&self.ctx.registry.borrow());
        if let Some((pawn, current, max)) = pawn_health {
            // Engine-owned and always declared, so this write succeeds; an error
            // here would be a real bug, hence no skip-with-warn.
            if let Err(err) =
                write_store_slot(&self.ctx, "player.health", SlotValue::Number(current))
            {
                log::warn!("[HUD] failed to write player.health: {err}");
            }

            if max.is_finite() && max >= 1.0 {
                if let Err(err) =
                    write_store_slot(&self.ctx, "player.maxHealth", SlotValue::Number(max))
                {
                    log::warn!("[HUD] failed to write player.maxHealth: {err}");
                }
            } else if self.invalid_max_warned_for != Some(pawn) {
                log::warn!(
                    "[HUD] skipping player.maxHealth for pawn {pawn}: invalid max health {max}"
                );
                self.invalid_max_warned_for = Some(pawn);
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
            zone_multipliers: std::collections::HashMap::new(),
        });
        health.current = current;
        registry.set_component(id, health).unwrap();
        id
    }

    #[test]
    fn tick_publishes_live_pawn_health_and_max_health() {
        use crate::scripting::primitives::store::read_store_slot;

        let ctx = ScriptCtx::new();
        spawn_pawn_with_health(&ctx, 73.0);
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());

        publisher.tick();
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(73.0)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.maxHealth").unwrap(),
            SlotValue::Number(100.0)
        );
    }

    #[test]
    fn tick_for_role_skips_player_slots_on_connected_client() {
        use crate::scripting::primitives::store::read_store_slot;

        // M15 Phase 3.5 Task 4: a connected client must NOT publish the player
        // slots — the server replicates them through the state-slot apply path.
        // With a live pawn present, the gated tick still writes nothing, so the
        // engine-owned slots keep their (unset) value.
        let ctx = ScriptCtx::new();
        spawn_pawn_with_health(&ctx, 73.0);
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());

        publisher.tick_for_role(true);
        assert_eq!(
            read_store_slot(&ctx, "player.health").ok(),
            None,
            "connected client does not publish player.health",
        );

        // Host / single-player (is_connected_client == false) still publishes.
        publisher.tick_for_role(false);
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(73.0),
            "host / single-player still publishes player.health",
        );
    }

    #[test]
    fn tick_tracks_pawn_hp_frame_over_frame() {
        use crate::scripting::primitives::store::read_store_slot;

        // The producer republishes the live pawn HP each frame, so a damage
        // mutation between ticks shows up in the slot the next frame (the M13
        // HUD readout would then show the new value).
        let ctx = ScriptCtx::new();
        let pawn = spawn_pawn_with_health(&ctx, 100.0);
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());

        publisher.tick();
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(100.0)
        );

        // Mutate the live HP, then tick again: the slot follows.
        {
            let mut registry = ctx.registry.borrow_mut();
            let mut health = registry
                .get_component::<HealthComponent>(pawn)
                .unwrap()
                .clone();
            health.current = 40.0;
            registry.set_component(pawn, health).unwrap();
        }
        publisher.tick();
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(40.0)
        );
    }

    #[test]
    fn publisher_write_is_visible_to_same_frame_crossing_detection() {
        use crate::scripting::data_descriptors::{
            CrossingCondition, CrossingDescriptor, LevelManifest,
        };
        use crate::scripting::primitives::store::read_store_slot;
        use crate::scripting::state_crossings::CrossingDetector;

        let ctx = ScriptCtx::new();
        let pawn = spawn_pawn_with_health(&ctx, 100.0);
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
        publisher.tick();

        ctx.data_registry.borrow_mut().populate_from_manifest(
            LevelManifest {
                reactions: Vec::new(),
                ui_trees: Vec::new(),
                crossings: vec![CrossingDescriptor {
                    slot: "player.health".to_string(),
                    condition: CrossingCondition::Below { threshold: 0.2 },
                    max: 100.0,
                    fire: vec!["lowHealth".to_string()],
                }],
            },
            &[],
        );
        let mut detector = CrossingDetector::new();
        detector.initialize(&ctx.data_registry.borrow(), &ctx.slot_table.borrow());

        {
            let mut registry = ctx.registry.borrow_mut();
            let mut health = registry
                .get_component::<HealthComponent>(pawn)
                .unwrap()
                .clone();
            health.current = 10.0;
            registry.set_component(pawn, health).unwrap();
        }

        publisher.tick();
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(10.0)
        );
        assert_eq!(
            detector.detect(&ctx.slot_table.borrow()),
            vec!["lowHealth".to_string()],
            "crossing detection must observe the publisher's same-frame write"
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
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());

        publisher.tick();
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(55.0),
            "no pawn → health slot unchanged"
        );
    }

    #[test]
    fn pawn_health_values_none_without_pawn_or_health_component() {
        // No entities at all → None.
        let empty = ScriptCtx::new();
        assert_eq!(pawn_health_values(&empty.registry.borrow()), None);

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
        assert_eq!(pawn_health_values(&no_health.registry.borrow()), None);

        // A pawn carrying Health → reads its current HP.
        let with_health = ScriptCtx::new();
        spawn_pawn_with_health(&with_health, 88.0);
        assert_eq!(
            pawn_health_values(&with_health.registry.borrow()),
            Some((EntityId::from_raw(0), 88.0, 100.0))
        );
    }

    #[test]
    fn invalid_live_max_publishes_current_and_skips_max_without_repairing() {
        use crate::scripting::primitives::store::read_store_slot;

        let ctx = ScriptCtx::new();
        let pawn = spawn_pawn_with_health(&ctx, 64.0);
        write_store_slot(&ctx, "player.maxHealth", SlotValue::Number(100.0)).unwrap();
        {
            let mut registry = ctx.registry.borrow_mut();
            let mut health = registry
                .get_component::<HealthComponent>(pawn)
                .unwrap()
                .clone();
            health.max = 0.5;
            registry.set_component(pawn, health).unwrap();
        }
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());

        publisher.tick();

        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(64.0)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.maxHealth").unwrap(),
            SlotValue::Number(100.0),
            "invalid max is skipped instead of clamped by the store range"
        );
        assert_eq!(publisher.invalid_max_warned_for, Some(pawn));
    }

    #[test]
    fn invalid_live_max_warning_latches_per_pawn_lifetime() {
        let ctx = ScriptCtx::new();
        let first = spawn_pawn_with_health(&ctx, 64.0);
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
        {
            let mut registry = ctx.registry.borrow_mut();
            let mut health = registry
                .get_component::<HealthComponent>(first)
                .unwrap()
                .clone();
            health.max = f32::NAN;
            registry.set_component(first, health).unwrap();
        }

        publisher.tick();
        assert_eq!(publisher.invalid_max_warned_for, Some(first));
        publisher.tick();
        assert_eq!(
            publisher.invalid_max_warned_for,
            Some(first),
            "same pawn lifetime stays latched"
        );

        {
            let mut registry = ctx.registry.borrow_mut();
            registry.despawn(first).unwrap();
        }
        let second = spawn_pawn_with_health(&ctx, 32.0);
        {
            let mut registry = ctx.registry.borrow_mut();
            let mut health = registry
                .get_component::<HealthComponent>(second)
                .unwrap()
                .clone();
            health.max = 0.0;
            registry.set_component(second, health).unwrap();
        }

        publisher.tick();
        assert_eq!(
            publisher.invalid_max_warned_for,
            Some(second),
            "new pawn lifetime can emit one warning"
        );
    }
}
