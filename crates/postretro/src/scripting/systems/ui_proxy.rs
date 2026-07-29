// Player HUD state publisher. Publishes authoritative pawn health and active
// weapon ammo/reload state into readonly engine-owned slots each frame.
// See: context/lib/scripting.md §5 "Durable State Store"

use std::collections::HashSet;

use crate::scripting::primitives::store::write_store_slot;
use postretro_entities::AmmoReserve;
use postretro_entities::components::health::pawn_with_health;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::ctx::ScriptCtx;
use postretro_entities::registry::{EntityId, EntityRegistry};
use postretro_entities::slot_table::SlotValue;

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

fn weapon_hud_values(
    registry: &EntityRegistry,
    active_wieldable: Option<EntityId>,
) -> (Option<(u32, u32)>, f32, bool) {
    let Some(pawn) = registry.local_player_movement_pawn() else {
        return (None, 0.0, false);
    };
    let Some(weapon_id) = active_wieldable else {
        return (None, 0.0, false);
    };
    let Ok(weapon) = registry.get_component::<WeaponComponent>(weapon_id) else {
        return (None, 0.0, false);
    };
    let (progress, active) = weapon.reload_status();
    let ammo = weapon.effective().ammo.map(|ammo| {
        let reserve = registry
            .get_component::<AmmoReserve>(pawn)
            .map_or(0, |reserve| reserve.available(ammo.ammo_type));
        (weapon.magazine, reserve)
    });
    (ammo, progress, active)
}

/// Engine-side producer for the HUD store slots.
pub(crate) struct PlayerHudStatePublisher {
    ctx: ScriptCtx,
    invalid_max_warned_for: Option<EntityId>,
    write_failure_warned_slots: HashSet<&'static str>,
}

impl PlayerHudStatePublisher {
    /// Build a publisher holding a clone of the engine's `ScriptCtx`.
    pub(crate) fn new(ctx: ScriptCtx) -> Self {
        Self {
            ctx,
            invalid_max_warned_for: None,
            write_failure_warned_slots: HashSet::new(),
        }
    }

    fn write_hud_slot(&mut self, name: &'static str, value: SlotValue) {
        if let Err(err) = write_store_slot(&self.ctx, name, value)
            && self.write_failure_warned_slots.insert(name)
        {
            log::warn!(
                "[HUD] failed to publish built-in slot `{name}`; suppressing repeated warnings for this slot: {err}"
            );
        }
    }

    /// Republish the player HUD store slots for this frame unless this endpoint is
    /// a connected client.
    ///
    /// Owner-private `player.*` HUD slots are replicated. On a connected client
    /// the server writes them through the
    /// state-slot apply path, so the local (non-authoritative) publisher must not
    /// overwrite them. The host and single-player keep publishing as before. The
    /// `is_connected_client` decision is owned by the `main.rs` call site (the
    /// `NetEndpoint` role lives there); this method keeps the gate testable without
    /// an `App`.
    pub(crate) fn tick_for_role(
        &mut self,
        is_connected_client: bool,
        active_wieldable: Option<EntityId>,
    ) {
        if is_connected_client {
            return;
        }
        self.tick(active_wieldable);
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
    pub(crate) fn tick(&mut self, active_wieldable: Option<EntityId>) {
        // `player.health`/`player.maxHealth` mirror the live pawn HP. No pawn /
        // no health component → skip; the readonly slots retain their previous
        // values. The registry borrow is scoped to the read so it drops before
        // the `write_store_slot` calls (which borrow the slot table, a separate
        // cell).
        let pawn_health = pawn_health_values(&self.ctx.registry.borrow());
        if let Some((pawn, current, max)) = pawn_health {
            // Engine-owned and always declared, so a write error is a real bug
            // and must be surfaced rather than silently skipped.
            self.write_hud_slot("player.health", SlotValue::Number(current));

            if max.is_finite() && max >= 1.0 {
                self.write_hud_slot("player.maxHealth", SlotValue::Number(max));
            } else if self.invalid_max_warned_for != Some(pawn) {
                log::warn!(
                    "[HUD] skipping player.maxHealth for pawn {pawn}: invalid max health {max}"
                );
                self.invalid_max_warned_for = Some(pawn);
            }
        }

        let (ammo, reload_progress, reload_active) =
            weapon_hud_values(&self.ctx.registry.borrow(), active_wieldable);
        if let Some((magazine, reserve)) = ammo {
            self.write_hud_slot("player.ammo", SlotValue::Number(magazine as f32));
            self.write_hud_slot("player.ammoReserve", SlotValue::Number(reserve as f32));
        }
        self.write_hud_slot("player.reloadProgress", SlotValue::Number(reload_progress));
        self.write_hud_slot("player.reloadActive", SlotValue::Boolean(reload_active));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::components::player_movement::PlayerMovementComponent;
    use postretro_entities::components::weapon::ReloadFeedback;
    use postretro_entities::components::wieldable_state::WieldableState;
    use postretro_entities::registry::{EntityId, Transform};
    use postretro_scripting_core::data_descriptors::{
        AirParams, AmmoResource, CapsuleParams, FallParams, FireMode, GroundParams,
        HealthDescriptor, PlayerMovementDescriptor, ReloadStyle, ResolutionMode, SpeedParams,
        WeaponDescriptor, WeaponResource,
    };

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

    fn spawn_movement_pawn(ctx: &ScriptCtx) -> EntityId {
        let mut registry = ctx.registry.borrow_mut();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                PlayerMovementComponent::from_descriptor(&movement_descriptor()),
            )
            .unwrap();
        id
    }

    /// Spawn a pawn (carries `PlayerMovement`) with a `Health` component whose
    /// `current` HP is `current`. Returns the pawn id.
    fn spawn_pawn_with_health(ctx: &ScriptCtx, current: f32) -> EntityId {
        let id = spawn_movement_pawn(ctx);
        let mut health = HealthComponent::from_descriptor(&HealthDescriptor {
            max: 100.0,
            hitbox: None,
            zone_multipliers: std::collections::HashMap::new(),
        });
        health.current = current;
        let mut registry = ctx.registry.borrow_mut();
        registry.set_component(id, health).unwrap();
        id
    }

    fn spawn_ammo_weapon(ctx: &ScriptCtx, pawn: EntityId) -> EntityId {
        let descriptor = WeaponDescriptor {
            damage: 10.0,
            range: 64.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            credit_source: None,
            third_person_model: None,
            viewmodel: None,
            resource: Some(WeaponResource::Ammo(AmmoResource {
                ammo_type: "bullets.light".to_string(),
                magazine: 12,
                cost_per_shot: 1,
                reserve: 48,
                reload_ms: 500,
                reload_style: ReloadStyle::Magazine,
            })),
        };
        let mut weapon = WeaponComponent::from_descriptor(&descriptor);
        weapon.magazine = 5;
        weapon.state = WieldableState::Reloading;
        weapon.state_remaining_ms = 250;
        weapon.state_total_ms = 500;
        let mut reserve = AmmoReserve::new();
        reserve.credit("bullets.light", 20);
        let mut registry = ctx.registry.borrow_mut();
        registry.set_component(pawn, reserve).unwrap();
        let id = registry.spawn(Transform::default());
        registry.set_component(id, weapon).unwrap();
        id
    }

    #[test]
    fn tick_publishes_ammo_reserve_and_reload_then_resets_idle_reload_slots() {
        use crate::scripting::primitives::store::read_store_slot;

        let ctx = ScriptCtx::new();
        let pawn = spawn_pawn_with_health(&ctx, 100.0);
        let weapon_id = spawn_ammo_weapon(&ctx, pawn);
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());

        publisher.tick(Some(weapon_id));
        assert_eq!(
            read_store_slot(&ctx, "player.ammo").unwrap(),
            SlotValue::Number(5.0)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.ammoReserve").unwrap(),
            SlotValue::Number(20.0)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.reloadProgress").unwrap(),
            SlotValue::Number(0.5)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.reloadActive").unwrap(),
            SlotValue::Boolean(true)
        );

        let mut weapon = ctx
            .registry
            .borrow()
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        weapon.reload_feedback = Some(ReloadFeedback::Started);
        ctx.registry
            .borrow_mut()
            .set_component(weapon_id, weapon)
            .unwrap();
        publisher.tick(Some(weapon_id));
        assert_eq!(
            read_store_slot(&ctx, "player.reloadProgress").unwrap(),
            SlotValue::Number(0.0)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.reloadActive").unwrap(),
            SlotValue::Boolean(true)
        );

        let mut weapon = ctx
            .registry
            .borrow()
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        weapon.reload_feedback = Some(ReloadFeedback::Completed);
        weapon.state_remaining_ms = 0;
        ctx.registry
            .borrow_mut()
            .set_component(weapon_id, weapon)
            .unwrap();
        publisher.tick(Some(weapon_id));
        assert_eq!(
            read_store_slot(&ctx, "player.reloadProgress").unwrap(),
            SlotValue::Number(1.0)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.reloadActive").unwrap(),
            SlotValue::Boolean(true)
        );

        let mut weapon = ctx
            .registry
            .borrow()
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        weapon.state_remaining_ms = 0;
        weapon.reload_feedback = None;
        weapon.state = WieldableState::Idle;
        ctx.registry
            .borrow_mut()
            .set_component(weapon_id, weapon)
            .unwrap();
        publisher.tick(Some(weapon_id));
        assert_eq!(
            read_store_slot(&ctx, "player.reloadProgress").unwrap(),
            SlotValue::Number(0.0)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.reloadActive").unwrap(),
            SlotValue::Boolean(false)
        );

        let mut weapon = ctx
            .registry
            .borrow()
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        weapon.state_remaining_ms = 10;
        weapon.state_total_ms = 0;
        weapon.state = WieldableState::Reloading;
        ctx.registry
            .borrow_mut()
            .set_component(weapon_id, weapon)
            .unwrap();
        publisher.tick(Some(weapon_id));
        assert_eq!(
            read_store_slot(&ctx, "player.reloadProgress").unwrap(),
            SlotValue::Number(0.0)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.reloadActive").unwrap(),
            SlotValue::Boolean(true)
        );
    }

    #[test]
    fn tick_publishes_a_per_shell_boundary_before_the_next_step_ramp() {
        use crate::scripting::primitives::store::read_store_slot;

        let ctx = ScriptCtx::new();
        let pawn = spawn_pawn_with_health(&ctx, 100.0);
        let weapon_id = spawn_ammo_weapon(&ctx, pawn);
        let mut weapon = ctx
            .registry
            .borrow()
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        weapon.state = WieldableState::ShellLoading;
        weapon.state_remaining_ms = 100;
        weapon.state_total_ms = 100;
        weapon.reload_feedback = Some(ReloadFeedback::Completed);
        ctx.registry
            .borrow_mut()
            .set_component(weapon_id, weapon)
            .unwrap();
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());

        publisher.tick(Some(weapon_id));
        assert_eq!(
            read_store_slot(&ctx, "player.reloadProgress").unwrap(),
            SlotValue::Number(1.0)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.reloadActive").unwrap(),
            SlotValue::Boolean(true)
        );

        // The local publisher observes the endpoint before the frame clear. The
        // next frame can then publish the active next-step ramp.
        crate::sim::clear_reload_feedback_for_weapon(&mut ctx.registry.borrow_mut(), weapon_id);
        let mut weapon = ctx
            .registry
            .borrow()
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        weapon.state_remaining_ms = 50;
        ctx.registry
            .borrow_mut()
            .set_component(weapon_id, weapon)
            .unwrap();

        publisher.tick(Some(weapon_id));
        assert_eq!(
            read_store_slot(&ctx, "player.reloadProgress").unwrap(),
            SlotValue::Number(0.5)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.reloadActive").unwrap(),
            SlotValue::Boolean(true)
        );
    }

    #[test]
    fn weapon_hud_values_use_movement_pawn_without_requiring_health() {
        let ctx = ScriptCtx::new();
        let pawn = spawn_movement_pawn(&ctx);
        let weapon = spawn_ammo_weapon(&ctx, pawn);

        assert_eq!(pawn_health_values(&ctx.registry.borrow()), None);
        assert_eq!(
            weapon_hud_values(&ctx.registry.borrow(), Some(weapon)).0,
            Some((5, 20)),
            "ammo HUD identity is independent of the Health component"
        );
    }

    #[test]
    fn tick_keeps_reload_active_when_hot_refresh_removes_ammo_tuning() {
        use crate::scripting::primitives::store::{read_store_slot, write_store_slot};

        let ctx = ScriptCtx::new();
        let pawn = spawn_pawn_with_health(&ctx, 100.0);
        let weapon_id = spawn_ammo_weapon(&ctx, pawn);
        let mut weapon = ctx
            .registry
            .borrow()
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        weapon.ammo = None;
        ctx.registry
            .borrow_mut()
            .set_component(weapon_id, weapon)
            .unwrap();
        write_store_slot(&ctx, "player.ammo", SlotValue::Number(7.0)).unwrap();
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());

        publisher.tick(Some(weapon_id));

        assert_eq!(
            read_store_slot(&ctx, "player.ammo").unwrap(),
            SlotValue::Number(7.0),
            "removed tuning still suppresses ammo publication"
        );
        assert_eq!(
            read_store_slot(&ctx, "player.reloadProgress").unwrap(),
            SlotValue::Number(0.5)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.reloadActive").unwrap(),
            SlotValue::Boolean(true)
        );
    }

    #[test]
    fn tick_skips_ammo_but_clears_reload_without_pawn_weapon_or_resource() {
        use crate::scripting::primitives::store::{read_store_slot, write_store_slot};

        let ctx = ScriptCtx::new();
        write_store_slot(&ctx, "player.ammo", SlotValue::Number(7.0)).unwrap();
        write_store_slot(&ctx, "player.ammoReserve", SlotValue::Number(31.0)).unwrap();
        write_store_slot(&ctx, "player.reloadProgress", SlotValue::Number(0.8)).unwrap();
        write_store_slot(&ctx, "player.reloadActive", SlotValue::Boolean(true)).unwrap();
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());

        publisher.tick(None);
        assert_eq!(
            read_store_slot(&ctx, "player.ammo").unwrap(),
            SlotValue::Number(7.0)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.ammoReserve").unwrap(),
            SlotValue::Number(31.0)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.reloadProgress").unwrap(),
            SlotValue::Number(0.0)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.reloadActive").unwrap(),
            SlotValue::Boolean(false)
        );

        let pawn = spawn_pawn_with_health(&ctx, 100.0);
        let weapon_id = {
            let mut registry = ctx.registry.borrow_mut();
            let id = registry.spawn(Transform::default());
            registry
                .set_component(
                    id,
                    WeaponComponent::from_descriptor(&WeaponDescriptor {
                        damage: 10.0,
                        range: 64.0,
                        cooldown_ms: 100.0,
                        fire_mode: FireMode::Semi,
                        resolution: ResolutionMode::Hitscan,
                        credit_source: None,
                        third_person_model: None,
                        viewmodel: None,
                        resource: None,
                    }),
                )
                .unwrap();
            id
        };
        let _ = pawn;
        publisher.tick(Some(weapon_id));
        assert_eq!(
            read_store_slot(&ctx, "player.ammo").unwrap(),
            SlotValue::Number(7.0)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.reloadActive").unwrap(),
            SlotValue::Boolean(false)
        );
    }

    #[test]
    fn tick_publishes_live_pawn_health_and_max_health() {
        use crate::scripting::primitives::store::read_store_slot;

        let ctx = ScriptCtx::new();
        spawn_pawn_with_health(&ctx, 73.0);
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());

        publisher.tick(None);
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

        publisher.tick_for_role(true, None);
        assert_eq!(
            read_store_slot(&ctx, "player.health").ok(),
            None,
            "connected client does not publish player.health",
        );

        // Host / single-player (is_connected_client == false) still publishes.
        publisher.tick_for_role(false, None);
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

        publisher.tick(None);
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
        publisher.tick(None);
        assert_eq!(
            read_store_slot(&ctx, "player.health").unwrap(),
            SlotValue::Number(40.0)
        );
    }

    #[test]
    fn publisher_write_is_visible_to_same_frame_crossing_detection() {
        use crate::scripting::primitives::store::read_store_slot;
        use postretro_scripting_core::data_descriptors::{CrossingCondition, CrossingDescriptor};
        use postretro_scripting_core::state_crossings::CrossingDetector;

        let ctx = ScriptCtx::new();
        let pawn = spawn_pawn_with_health(&ctx, 100.0);
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
        publisher.tick(None);

        ctx.data_registry.borrow_mut().populate_level(
            Vec::new(),
            vec![CrossingDescriptor {
                slot: Some("player.health".to_string()),
                condition: CrossingCondition::Below { threshold: 0.2 },
                max: 100.0,
                edge: None,
                fire: vec!["lowHealth".to_string()],
            }],
            &[],
        );
        let mut detector = CrossingDetector::new();
        detector.initialize(&ctx.data_registry.borrow(), &ctx.slot_table.borrow(), &ctx);

        {
            let mut registry = ctx.registry.borrow_mut();
            let mut health = registry
                .get_component::<HealthComponent>(pawn)
                .unwrap()
                .clone();
            health.current = 10.0;
            registry.set_component(pawn, health).unwrap();
        }

        publisher.tick(None);
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

        publisher.tick(None);
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

        publisher.tick(None);

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

        publisher.tick(None);
        assert_eq!(publisher.invalid_max_warned_for, Some(first));
        publisher.tick(None);
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

        publisher.tick(None);
        assert_eq!(
            publisher.invalid_max_warned_for,
            Some(second),
            "new pawn lifetime can emit one warning"
        );
    }

    #[test]
    fn persistent_write_failures_latch_once_per_distinct_slot() {
        use postretro_entities::slot_table::SlotType;

        let ctx = ScriptCtx::new();
        {
            let mut slots = ctx.slot_table.borrow_mut();
            slots
                .get_mut("player.reloadProgress")
                .unwrap()
                .schema
                .slot_type = SlotType::Boolean;
            slots
                .get_mut("player.reloadActive")
                .unwrap()
                .schema
                .slot_type = SlotType::Number;
        }
        let mut publisher = PlayerHudStatePublisher::new(ctx);

        publisher.tick(None);
        publisher.tick(None);

        assert_eq!(
            publisher.write_failure_warned_slots,
            HashSet::from(["player.reloadActive", "player.reloadProgress"]),
            "persistent failures stay latched while distinct slots remain visible"
        );
    }
}
