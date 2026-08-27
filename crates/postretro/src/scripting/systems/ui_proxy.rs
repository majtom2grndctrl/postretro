// Player state publisher. Host publishes authoritative health at each impact seam and
// republishes health, ammo, and reload slots for HUD consumers after game logic;
// every role publishes local display-only `player.weapon.*` slots.
// See: context/lib/scripting.md §5 "Durable State Store"

use std::collections::HashSet;

use crate::scripting::primitives::store::write_store_slot;
use postretro_entities::AmmoReserve;
use postretro_entities::components::health::pawn_with_health;
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::ctx::ScriptCtx;
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::registry::{EntityId, EntityRegistry};
use postretro_entities::slot_table::{SlotOwnership, SlotValue};

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
) -> (Option<EntityId>, Option<(u32, u32)>, f32, bool) {
    let Some(pawn) = registry.local_player_movement_pawn() else {
        return (None, None, 0.0, false);
    };
    let Some(weapon_id) = registry
        .get_component::<Inventory>(pawn)
        .ok()
        .and_then(Inventory::active_wieldable)
    else {
        return (None, None, 0.0, false);
    };
    let Ok(weapon) = registry.get_component::<WeaponComponent>(weapon_id) else {
        return (None, None, 0.0, false);
    };
    let (progress, active) = weapon.reload_status();
    let ammo = weapon.effective().ammo.map(|ammo| {
        let reserve = registry
            .get_component::<AmmoReserve>(pawn)
            .map_or(0, |reserve| reserve.available(ammo.ammo_type));
        (weapon.magazine, reserve)
    });
    (Some(weapon_id), ammo, progress, active)
}

/// Read the local display-only switching state from the owning pawn's inventory.
/// The committed active instance names the current weapon; a switch target means
/// the machine is in flight. Descriptor provenance preserves the canonical
/// archetype identity of spawned wieldables.
fn weapon_state_values(
    registry: &EntityRegistry,
    pending_slot: Option<usize>,
) -> (String, String, bool) {
    let Some(pawn) = registry.local_player_movement_pawn() else {
        return (String::new(), String::new(), false);
    };
    let Some(inventory) = registry.get_component::<Inventory>(pawn).ok() else {
        return (String::new(), String::new(), false);
    };
    let weapon_name = |slot: usize| {
        inventory
            .wieldables
            .get(slot)
            .copied()
            .flatten()
            .and_then(|weapon| registry.get_component::<DescriptorProvenance>(weapon).ok())
            .map(|provenance| provenance.canonical_name.clone())
            .unwrap_or_default()
    };
    let current = weapon_name(inventory.active_slot);
    let pending = pending_slot.map(weapon_name).unwrap_or_default();
    (current, pending, inventory.switch_target.is_some())
}

/// Engine-side producer for player slots consumed by impact policies and HUD.
pub(crate) struct PlayerHudStatePublisher {
    ctx: ScriptCtx,
    invalid_max_warned_for: Option<EntityId>,
    write_failure_warned_slots: HashSet<&'static str>,
    /// Input-layer cursor selection. It is local on every role and deliberately
    /// never enters `Inventory`, simulation, or replication.
    pending_weapon_slot: Option<usize>,
}

impl PlayerHudStatePublisher {
    /// Build a publisher holding a clone of the engine's `ScriptCtx`.
    pub(crate) fn new(ctx: ScriptCtx) -> Self {
        Self {
            ctx,
            invalid_max_warned_for: None,
            write_failure_warned_slots: HashSet::new(),
            pending_weapon_slot: None,
        }
    }

    /// Set the input layer's local pending cursor for this frame's HUD publish.
    pub(crate) fn set_pending_weapon_slot(&mut self, pending_weapon_slot: Option<usize>) {
        self.pending_weapon_slot = pending_weapon_slot;
    }

    fn write_hud_slot(&mut self, name: &'static str, value: SlotValue) -> bool {
        match write_store_slot(&self.ctx, name, value) {
            Ok(()) => true,
            Err(err) => {
                if self.write_failure_warned_slots.insert(name) {
                    log::warn!(
                        "[HUD] failed to publish built-in slot `{name}`; suppressing repeated warnings for this slot: {err}"
                    );
                }
                false
            }
        }
    }

    fn clear_hud_slot(&mut self, name: &'static str) -> bool {
        let mut slots = self.ctx.slot_table.borrow_mut();
        let Some(record) = slots.get_mut(name) else {
            drop(slots);
            if self.write_failure_warned_slots.insert(name) {
                log::warn!("[HUD] failed to clear missing built-in slot `{name}`");
            }
            return false;
        };
        record.write_value(None);
        true
    }

    /// Republish the player HUD store slots for this frame.
    #[cfg(test)]
    pub(crate) fn tick_for_role(
        &mut self,
        is_connected_client: bool,
        _legacy_active_wieldable: Option<EntityId>,
    ) {
        let _ = self.tick_for_role_and_report_sampled_weapon(is_connected_client, None);
    }

    pub(crate) fn tick_for_role_and_report_sampled_weapon(
        &mut self,
        is_connected_client: bool,
        _legacy_active_wieldable: Option<EntityId>,
    ) -> Option<EntityId> {
        // A connected client suppresses host-authoritative HUD writes, but still
        // samples its locally-owned active component so the HUD feedback consumer
        // can acknowledge and drain its stream.
        if is_connected_client {
            // These switching display slots are local on every role: their inventory
            // source is locally owned, so no host projection exists to replicate.
            self.publish_local_weapon_state();
            return weapon_hud_values(&self.ctx.registry.borrow()).0;
        }
        self.tick_and_report_sampled_weapon()
    }

    /// Publish the owning local pawn's health slots from a registry the fixed-tick
    /// producer already borrows. Impact evaluation calls this after damage and
    /// before freezing ambient engine-state reads.
    pub(crate) fn publish_health_from_registry(&mut self, registry: &EntityRegistry) {
        let pawn_health = pawn_health_values(registry);
        self.publish_health_values(pawn_health);
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
    #[cfg(test)]
    pub(crate) fn tick(&mut self, _legacy_active_wieldable: Option<EntityId>) {
        let _ = self.tick_and_report_sampled_weapon();
    }

    fn tick_and_report_sampled_weapon(&mut self) -> Option<EntityId> {
        self.publish_local_per_owner_mod_slots();
        self.publish_local_weapon_state();
        // `player.health`/`player.maxHealth` mirror the live pawn HP. No pawn /
        // no health component → skip; the readonly slots retain their previous
        // values. The registry borrow is scoped to the read so it drops before
        // the `write_store_slot` calls (which borrow the slot table, a separate
        // cell).
        let pawn_health = pawn_health_values(&self.ctx.registry.borrow());
        self.publish_health_values(pawn_health);

        let (sampled_weapon, ammo, reload_progress, reload_active) =
            weapon_hud_values(&self.ctx.registry.borrow());
        match (sampled_weapon, ammo) {
            (_, Some((magazine, reserve))) => {
                self.write_hud_slot("player.ammo", SlotValue::Number(magazine as f32));
                self.write_hud_slot("player.ammoReserve", SlotValue::Number(reserve as f32));
            }
            (Some(_), None) => {
                // A live resourceless weapon is an authoritative absence, unlike a
                // missing pawn. Clear the outgoing weapon's values at the repoint.
                self.clear_hud_slot("player.ammo");
                self.clear_hud_slot("player.ammoReserve");
            }
            (None, None) => {}
        }
        let reload_progress_written =
            self.write_hud_slot("player.reloadProgress", SlotValue::Number(reload_progress));
        let reload_active_written =
            self.write_hud_slot("player.reloadActive", SlotValue::Boolean(reload_active));
        if reload_progress_written && reload_active_written {
            sampled_weapon
        } else {
            None
        }
    }

    fn publish_health_values(&mut self, pawn_health: Option<(EntityId, f32, f32)>) {
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
    }

    fn publish_local_weapon_state(&mut self) {
        let (current, pending, switching) =
            weapon_state_values(&self.ctx.registry.borrow(), self.pending_weapon_slot);
        self.write_hud_slot("player.weapon.current", SlotValue::String(current));
        self.write_hud_slot("player.weapon.pending", SlotValue::String(pending));
        self.write_hud_slot("player.weapon.switching", SlotValue::Boolean(switching));
    }

    /// Refresh unaddressed HUD reads of mod-owned per-owner slots from the
    /// local pawn's seat. A missing local pawn, seat, or declared default leaves
    /// the prior scalar projection intact, matching the existing publisher's
    /// absence behavior.
    fn publish_local_per_owner_mod_slots(&mut self) {
        let local_seat = {
            let registry = self.ctx.registry.borrow();
            registry
                .local_player_pawn()
                .and_then(|pawn| registry.seat_for_pawn(pawn))
        };
        let Some(local_seat) = local_seat else {
            return;
        };

        let mut slots = self.ctx.slot_table.borrow_mut();
        for (_, record) in slots.iter_mut() {
            if record.schema.ownership != SlotOwnership::Mod || !record.schema.per_owner {
                continue;
            }
            let Some(value) = record.per_seat_value(local_seat).cloned() else {
                continue;
            };
            if record.value.as_ref() != Some(&value) {
                record.write_value(Some(value));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::components::player_movement::PlayerMovementComponent;
    use postretro_entities::components::weapon::ReloadFeedback;
    use postretro_entities::components::wieldable_state::WieldableState;
    use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
    use postretro_entities::registry::{EntityId, Transform};
    use postretro_entities::slot_table::{
        ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType,
    };
    use postretro_foundation::Seat;
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

    fn per_owner_number_slot(default: f32) -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: SlotType::Number,
            default: Some(SlotValue::Number(default)),
            range: None,
            persist: false,
            readonly: false,
            ownership: SlotOwnership::Mod,
            network: ReplicationScope::None,
            per_owner: true,
            accumulate: None,
        })
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
            pellet_count: 1,
            spread_degrees: 0.0,
            range: 64.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            projectile: None,
            credit_source: None,
            third_person_model: None,
            viewmodel: None,
            placement: None,
            muzzle_offset: None,
            resource: Some(WeaponResource::Ammo(AmmoResource {
                ammo_type: "bullets.light".to_string(),
                magazine: 12,
                cost_per_shot: 1,
                reserve: 48,
                reload_ms: 500,
                reload_style: ReloadStyle::Magazine,
            })),
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
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
        registry
            .set_component(
                id,
                DescriptorProvenance {
                    canonical_name: "reference_pistol".to_string(),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::DefaultWeapon,
                },
            )
            .unwrap();
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(id);
        registry.set_component(pawn, inventory).unwrap();
        id
    }

    #[test]
    fn per_owner_mod_slot_publishes_only_the_local_seat_projection() {
        let ctx = ScriptCtx::new();
        let local = spawn_movement_pawn(&ctx);
        let remote = spawn_movement_pawn(&ctx);
        {
            let mut registry = ctx.registry.borrow_mut();
            registry.mark_local_player_pawn(local).unwrap();
            registry.bind_pawn_seat(local, Seat(0));
            registry.bind_pawn_seat(remote, Seat(1));
        }
        {
            let mut slots = ctx.slot_table.borrow_mut();
            slots
                .insert_namespace(
                    "currency",
                    vec![("xp".to_string(), per_owner_number_slot(5.0))],
                )
                .unwrap();
            let xp = slots.get_mut("currency.xp").unwrap();
            xp.set_per_seat_value(Seat(0), SlotValue::Number(17.0));
            xp.set_per_seat_value(Seat(1), SlotValue::Number(31.0));
            xp.write_value(Some(SlotValue::Number(99.0)));
        }

        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
        publisher.tick(None);

        assert_eq!(
            ctx.slot_table.borrow().get("currency.xp").unwrap().value,
            Some(SlotValue::Number(17.0)),
            "the HUD projection reads the local seat, never a remote owner's value"
        );

        let generation = ctx
            .slot_table
            .borrow()
            .get("currency.xp")
            .unwrap()
            .write_generation();
        publisher.tick(None);
        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("currency.xp")
                .unwrap()
                .write_generation(),
            generation,
            "an unchanged local-seat projection must not emit a spurious write notification"
        );
    }

    #[test]
    fn per_owner_mod_slot_projection_skips_a_remote_seat_without_a_marked_local_pawn() {
        let ctx = ScriptCtx::new();
        let remote = spawn_movement_pawn(&ctx);
        ctx.registry.borrow_mut().bind_pawn_seat(remote, Seat(1));
        {
            let mut slots = ctx.slot_table.borrow_mut();
            slots
                .insert_namespace(
                    "currency",
                    vec![("xp".to_string(), per_owner_number_slot(5.0))],
                )
                .unwrap();
            slots
                .get_mut("currency.xp")
                .unwrap()
                .set_per_seat_value(Seat(1), SlotValue::Number(31.0));
            slots
                .get_mut("currency.xp")
                .unwrap()
                .write_value(Some(SlotValue::Number(77.0)));
        }

        PlayerHudStatePublisher::new(ctx.clone()).tick(None);

        assert_eq!(
            ctx.slot_table.borrow().get("currency.xp").unwrap().value,
            Some(SlotValue::Number(77.0)),
            "an unmarked local pawn never falls back to a remote seat projection"
        );
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
        let feedback_tick = weapon.begin_reload_feedback_tick();
        weapon.publish_reload_feedback(ReloadFeedback::Started, feedback_tick);
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
        weapon.reload_feedback = Default::default();
        let feedback_tick = weapon.begin_reload_feedback_tick();
        weapon.publish_reload_feedback(ReloadFeedback::Completed, feedback_tick);
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
        weapon.reload_feedback = Default::default();
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
        let feedback_tick = weapon.begin_reload_feedback_tick();
        weapon.publish_reload_feedback(ReloadFeedback::Completed, feedback_tick);
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
    fn tick_publishes_started_then_completed_after_fixed_tick_catch_up() {
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
        weapon.state = WieldableState::Idle;
        weapon.state_remaining_ms = 0;
        weapon.state_total_ms = 0;
        let start_tick = weapon.begin_reload_feedback_tick();
        weapon.publish_reload_feedback(ReloadFeedback::Started, start_tick);
        let completed_tick = weapon.begin_reload_feedback_tick();
        weapon.publish_reload_feedback(ReloadFeedback::Completed, completed_tick);
        ctx.registry
            .borrow_mut()
            .set_component(weapon_id, weapon)
            .unwrap();
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());

        // Regression: catch-up overwrote Started with Completed before the HUD sampled.
        publisher.tick(Some(weapon_id));
        assert_eq!(
            read_store_slot(&ctx, "player.reloadProgress").unwrap(),
            SlotValue::Number(0.0)
        );
        crate::sim::clear_reload_feedback_for_weapon(&mut ctx.registry.borrow_mut(), weapon_id);

        publisher.tick(Some(weapon_id));
        assert_eq!(
            read_store_slot(&ctx, "player.reloadProgress").unwrap(),
            SlotValue::Number(1.0)
        );
        crate::sim::clear_reload_feedback_for_weapon(&mut ctx.registry.borrow_mut(), weapon_id);

        publisher.tick(Some(weapon_id));
        assert_eq!(
            read_store_slot(&ctx, "player.reloadProgress").unwrap(),
            SlotValue::Number(0.0)
        );
        assert_eq!(
            read_store_slot(&ctx, "player.reloadActive").unwrap(),
            SlotValue::Boolean(false)
        );
    }

    #[test]
    fn weapon_hud_values_use_movement_pawn_without_requiring_health() {
        let ctx = ScriptCtx::new();
        let pawn = spawn_movement_pawn(&ctx);
        let _weapon = spawn_ammo_weapon(&ctx, pawn);

        assert_eq!(pawn_health_values(&ctx.registry.borrow()), None);
        assert_eq!(
            weapon_hud_values(&ctx.registry.borrow()).1,
            Some((5, 20)),
            "ammo HUD identity is independent of the Health component"
        );
    }

    #[test]
    fn weapon_state_slots_follow_committed_inventory_and_publish_on_clients() {
        use crate::scripting::primitives::store::read_store_slot;

        let ctx = ScriptCtx::new();
        let pawn = spawn_movement_pawn(&ctx);
        let outgoing = spawn_ammo_weapon(&ctx, pawn);
        let incoming = {
            let mut registry = ctx.registry.borrow_mut();
            let id = registry.spawn(Transform::default());
            registry
                .set_component(
                    id,
                    DescriptorProvenance {
                        canonical_name: "reference_shotgun".to_string(),
                        owned_components: Default::default(),
                        map_overrides: Default::default(),
                        spawn_path: DescriptorSpawnPath::DefaultWeapon,
                    },
                )
                .unwrap();
            let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
            inventory.wieldables[1] = Some(id);
            inventory.switch_target = Some(1);
            registry.set_component(pawn, inventory).unwrap();
            id
        };
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
        publisher.set_pending_weapon_slot(Some(1));

        publisher.tick_for_role_and_report_sampled_weapon(true, None);
        assert_eq!(
            read_store_slot(&ctx, "player.weapon.current").unwrap(),
            SlotValue::String("reference_pistol".to_string()),
            "current remains the outgoing instance while lowering"
        );
        assert_eq!(
            read_store_slot(&ctx, "player.weapon.pending").unwrap(),
            SlotValue::String("reference_shotgun".to_string()),
            "pending projects the input-layer cursor before any inventory repoint"
        );
        assert_eq!(
            read_store_slot(&ctx, "player.weapon.switching").unwrap(),
            SlotValue::Boolean(true)
        );

        let mut inventory = ctx
            .registry
            .borrow()
            .get_component::<Inventory>(pawn)
            .unwrap()
            .clone();
        inventory.active_slot = 1;
        inventory.switch_target = None;
        ctx.registry
            .borrow_mut()
            .set_component(pawn, inventory)
            .unwrap();
        publisher.tick_for_role_and_report_sampled_weapon(true, None);
        assert_eq!(
            read_store_slot(&ctx, "player.weapon.current").unwrap(),
            SlotValue::String("reference_shotgun".to_string()),
            "current flips at the active-slot repoint, not switch acceptance"
        );
        assert_eq!(
            read_store_slot(&ctx, "player.weapon.switching").unwrap(),
            SlotValue::Boolean(false)
        );
        assert_ne!(outgoing, incoming);
    }

    #[test]
    fn o39_o40_one_frame_publish_observes_only_a_completed_short_switch() {
        use crate::scripting::primitives::store::read_store_slot;

        let ctx = ScriptCtx::new();
        let pawn = spawn_movement_pawn(&ctx);
        let first = spawn_ammo_weapon(&ctx, pawn);
        let second = {
            let mut registry = ctx.registry.borrow_mut();
            let id = registry.spawn(Transform::default());
            registry
                .set_component(
                    id,
                    DescriptorProvenance {
                        canonical_name: "instant_weapon".to_string(),
                        owned_components: Default::default(),
                        map_overrides: Default::default(),
                        spawn_path: DescriptorSpawnPath::DefaultWeapon,
                    },
                )
                .unwrap();
            let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
            inventory.wieldables[1] = Some(id);
            // The zero-duration lower/repoint/raise completed during the frame's
            // fixed-tick loop before the once-per-frame publisher runs.
            inventory.active_slot = 1;
            inventory.switch_target = None;
            inventory.switch_origin = None;
            registry.set_component(pawn, inventory).unwrap();
            id
        };
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());

        publisher.tick_for_role(false, None);

        assert_eq!(
            read_store_slot(&ctx, "player.weapon.current").unwrap(),
            SlotValue::String("instant_weapon".to_string())
        );
        assert_eq!(
            read_store_slot(&ctx, "player.weapon.switching").unwrap(),
            SlotValue::Boolean(false),
            "a sub-publish or multi-tick-frame switch exposes only its final state"
        );
        assert_ne!(first, second);
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
            read_store_slot(&ctx, "player.ammo").ok(),
            None,
            "a live weapon with no ammo resource clears stale ammo presentation"
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
    fn o38_resourceless_incoming_weapon_clears_outgoing_ammo_and_reserve_slots() {
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
                        pellet_count: 1,
                        spread_degrees: 0.0,
                        range: 64.0,
                        cooldown_ms: 100.0,
                        fire_mode: FireMode::Semi,
                        resolution: ResolutionMode::Hitscan,
                        projectile: None,
                        credit_source: None,
                        third_person_model: None,
                        viewmodel: None,
                        placement: None,
                        muzzle_offset: None,
                        resource: None,
                        lower_ms: 0,
                        raise_ms: 0,
                        block_during_reload: None,
                    }),
                )
                .unwrap();
            id
        };
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(weapon_id);
        ctx.registry
            .borrow_mut()
            .set_component(pawn, inventory)
            .unwrap();
        publisher.tick(Some(weapon_id));
        assert_eq!(
            read_store_slot(&ctx, "player.ammo").ok(),
            None,
            "incoming resourceless weapon cannot retain outgoing magazine"
        );
        assert_eq!(
            read_store_slot(&ctx, "player.ammoReserve").ok(),
            None,
            "incoming resourceless weapon cannot retain outgoing reserve"
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
    fn connected_client_skips_authoritative_slots_but_samples_inventory_feedback() {
        use crate::scripting::primitives::store::read_store_slot;
        use postretro_entities::components::weapon::ReloadFeedbackConsumer;

        // M15 Phase 3.5 Task 4: a connected client must NOT publish the player
        // slots — the server replicates them through the state-slot apply path.
        // With a live pawn present, the gated tick still writes nothing, so the
        // engine-owned slots keep their (unset) value.
        let ctx = ScriptCtx::new();
        let pawn = spawn_pawn_with_health(&ctx, 73.0);
        let weapon_id = spawn_ammo_weapon(&ctx, pawn);
        let mut weapon = ctx
            .registry
            .borrow()
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        let feedback_tick = weapon.begin_reload_feedback_tick();
        weapon.publish_reload_feedback(ReloadFeedback::Completed, feedback_tick);
        ctx.registry
            .borrow_mut()
            .set_component(weapon_id, weapon)
            .unwrap();
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());

        assert_eq!(
            publisher.tick_for_role_and_report_sampled_weapon(true, None),
            Some(weapon_id),
            "connected clients resolve the local inventory weapon for feedback acknowledgement"
        );
        assert_eq!(
            read_store_slot(&ctx, "player.health").ok(),
            None,
            "connected client does not publish player.health",
        );
        crate::sim::clear_reload_feedback_for_weapon(&mut ctx.registry.borrow_mut(), weapon_id);
        assert!(
            ctx.registry
                .borrow()
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .reload_feedback_sample(ReloadFeedbackConsumer::Hud)
                .endpoint
                .is_none(),
            "the sampled client feedback endpoint is acknowledged rather than accumulating"
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

    #[test]
    fn hud_feedback_acknowledgement_waits_for_both_reload_slot_writes() {
        use crate::scripting::primitives::store::read_store_slot;
        use postretro_entities::slot_table::SlotType;

        let ctx = ScriptCtx::new();
        let pawn = spawn_pawn_with_health(&ctx, 100.0);
        let weapon_id = spawn_ammo_weapon(&ctx, pawn);
        let mut weapon = ctx
            .registry
            .borrow()
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        let feedback_tick = weapon.begin_reload_feedback_tick();
        weapon.publish_reload_feedback(ReloadFeedback::Completed, feedback_tick);
        ctx.registry
            .borrow_mut()
            .set_component(weapon_id, weapon)
            .unwrap();

        {
            let mut slots = ctx.slot_table.borrow_mut();
            slots
                .get_mut("player.reloadActive")
                .unwrap()
                .schema
                .slot_type = SlotType::Number;
        }
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());

        // Regression: a failed reload-slot write advanced the HUD cursor and
        // discarded an endpoint that never reached the complete HUD surface.
        assert_eq!(
            publisher.tick_for_role_and_report_sampled_weapon(false, Some(weapon_id)),
            None
        );
        assert_eq!(
            read_store_slot(&ctx, "player.reloadProgress").unwrap(),
            SlotValue::Number(1.0),
            "the valid half of the projection still writes"
        );
        assert_eq!(
            ctx.registry
                .borrow()
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .reload_status(),
            (1.0, true),
            "the endpoint remains pending while either reload-slot write fails"
        );

        ctx.slot_table
            .borrow_mut()
            .get_mut("player.reloadActive")
            .unwrap()
            .schema
            .slot_type = SlotType::Boolean;
        assert_eq!(
            publisher.tick_for_role_and_report_sampled_weapon(false, Some(weapon_id)),
            Some(weapon_id),
            "successful retry reports the weapon for acknowledgement"
        );
        assert_eq!(
            read_store_slot(&ctx, "player.reloadActive").unwrap(),
            SlotValue::Boolean(true)
        );

        crate::sim::clear_reload_feedback_for_weapon(&mut ctx.registry.borrow_mut(), weapon_id);
        assert_eq!(
            ctx.registry
                .borrow()
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .reload_status(),
            (0.5, true),
            "acknowledgement advances to the live reload sample after projection"
        );
    }
}
