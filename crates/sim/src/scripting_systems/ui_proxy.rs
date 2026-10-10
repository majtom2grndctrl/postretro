// Player state publisher. Host publishes authoritative health at each impact seam and
// republishes health, ammo, heat/cell, and reload slots for HUD consumers after game logic;
// every role publishes local weapon names, spread, charging and resource-kind slots.
// See: context/lib/scripting.md §5 "Durable State Store"

use std::collections::HashSet;

use crate::scripting::primitives::store::write_store_slot;
use postretro_entities::AmmoReserve;
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::components::weapon::{ReloadFeedbackConsumer, WeaponComponent};
use postretro_entities::components::weapon_resource::WeaponResourceKind;
use postretro_entities::ctx::ScriptCtx;
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::registry::{EntityId, EntityRegistry};
use postretro_entities::slot_table::{SlotOwnership, SlotValue};
use postretro_foundation::ActivationToken;
use postretro_scripting_core::player_slots::{
    PlayerSlot, ReloadRead, player_slot_value, weapon_slot_value,
};

/// Read the current and maximum HP of the player pawn resolved by the local
/// player marker, with legacy fallback to the first entity carrying
/// `PlayerMovement`. Returns `None` when there is no pawn or the pawn carries no
/// `Health` component; the caller then skips the `player.*Health` writes and the
/// slots keep their last values (accepted slot-staleness contract).
///
/// Pure read against the registry: no slot table, no GPU, so it is unit-testable
/// without the publisher's `ScriptCtx`.
fn pawn_health_values(registry: &EntityRegistry) -> Option<(EntityId, f32, f32)> {
    let pawn = registry.local_player_movement_pawn()?;
    let number = |slot| match player_slot_value(registry, slot, pawn) {
        Some(SlotValue::Number(value)) => Some(value),
        _ => None,
    };
    Some((
        pawn,
        number(PlayerSlot::Health)?,
        number(PlayerSlot::MaxHealth)?,
    ))
}

/// The HUD samples reload through its own endpoint cursor.
const HUD_READ: ReloadRead = ReloadRead::Feedback(ReloadFeedbackConsumer::Hud);

/// One weapon slot's HUD number, through the shared per-pawn lookup.
fn hud_number(weapon: &WeaponComponent, reserve: Option<&AmmoReserve>, slot: PlayerSlot) -> Option<f32> {
    match weapon_slot_value(weapon, reserve, slot, HUD_READ) {
        Some(SlotValue::Number(value)) => Some(value),
        _ => None,
    }
}

fn hud_flag(weapon: &WeaponComponent, slot: PlayerSlot) -> bool {
    matches!(
        weapon_slot_value(weapon, None, slot, HUD_READ),
        Some(SlotValue::Boolean(true))
    )
}

/// The sampled active weapon's HUD facts. `sampled` is `None` with no pawn or
/// no live active weapon, and the other fields are then placeholders.
#[derive(Debug, Default, PartialEq)]
struct WeaponHudValues {
    sampled: Option<EntityId>,
    ammo: Option<(u32, u32)>,
    resource: ResourceHud,
    reload_progress: f32,
    reload_active: bool,
    effective_spread_degrees: f32,
    charging: Option<(ActivationToken, f32)>,
}

/// Heat and cell use the health pattern: raw value plus a companion max.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum ResourceHud {
    #[default]
    None,
    Ammo,
    Heat {
        heat: f32,
        overheat_at: f32,
        overheated: bool,
    },
    Cell {
        charge: f32,
        capacity: f32,
    },
}

impl ResourceHud {
    fn of(weapon: &WeaponComponent) -> Self {
        let number = |slot| hud_number(weapon, None, slot);
        match weapon.resource_kind() {
            WeaponResourceKind::None => Self::None,
            WeaponResourceKind::Ammo => Self::Ammo,
            WeaponResourceKind::Heat => number(PlayerSlot::Heat)
                .zip(number(PlayerSlot::OverheatAt))
                .map_or(Self::None, |(heat, overheat_at)| Self::Heat {
                    heat,
                    overheat_at,
                    overheated: hud_flag(weapon, PlayerSlot::Overheated),
                }),
            WeaponResourceKind::Cell => number(PlayerSlot::Cell)
                .zip(number(PlayerSlot::CellCapacity))
                .map_or(Self::None, |(charge, capacity)| Self::Cell { charge, capacity }),
        }
    }

    fn kind(self) -> WeaponResourceKind {
        match self {
            Self::None => WeaponResourceKind::None,
            Self::Ammo => WeaponResourceKind::Ammo,
            Self::Heat { .. } => WeaponResourceKind::Heat,
            Self::Cell { .. } => WeaponResourceKind::Cell,
        }
    }
}

fn weapon_hud_values(registry: &EntityRegistry) -> WeaponHudValues {
    let Some(pawn) = registry.local_player_movement_pawn() else {
        return WeaponHudValues::default();
    };
    let Ok(inventory) = registry.get_component::<Inventory>(pawn) else {
        return WeaponHudValues::default();
    };
    let Some(weapon_id) = inventory.active_wieldable() else {
        return WeaponHudValues::default();
    };
    let Ok(weapon) = registry.get_component::<WeaponComponent>(weapon_id) else {
        return WeaponHudValues::default();
    };
    let (horizontal_speed, run_speed) = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .ok()
        .map_or((0.0, 0.0), |movement| {
            let velocity = movement.velocity;
            (
                (velocity.x * velocity.x + velocity.z * velocity.z).sqrt(),
                movement.ground_params.speed.run,
            )
        });
    let reserve = registry.get_component::<AmmoReserve>(pawn).ok();
    let progress = hud_number(weapon, None, PlayerSlot::ReloadProgress).unwrap_or(0.0);
    let active = hud_flag(weapon, PlayerSlot::ReloadActive);
    // Integer-shaped HUD values: the lookup's `f32` round-trips them exactly
    // through 2^24 (scripting.md §5).
    let ammo = hud_number(weapon, reserve, PlayerSlot::Ammo)
        .zip(hud_number(weapon, reserve, PlayerSlot::AmmoReserve))
        .map(|(magazine, reserve)| (magazine as u32, reserve as u32));
    WeaponHudValues {
        sampled: Some(weapon_id),
        ammo,
        resource: ResourceHud::of(weapon),
        reload_progress: progress,
        reload_active: active,
        effective_spread_degrees: weapon.effective_spread_degrees(horizontal_speed, run_speed),
        charging: weapon_charge_values(registry, pawn, inventory, weapon),
    }
}

/// Sample only the installed active lane's fixed-tick cursor. Repeated rendered
/// frames cannot advance this value, and switching/death clears it before the
/// simulation has had another opportunity to cancel the component.
fn weapon_charge_values(
    registry: &EntityRegistry,
    pawn: EntityId,
    inventory: &Inventory,
    weapon: &WeaponComponent,
) -> Option<(ActivationToken, f32)> {
    if inventory.switch_target.is_some()
        || crate::scripting_systems::health::is_terminally_committed_to_removal(registry, pawn)
        || registry
            .get_component::<postretro_entities::components::health::HealthComponent>(pawn)
            .is_ok_and(|health| health.current <= 0.0 || !health.current.is_finite())
    {
        return None;
    }
    let postretro_entities::components::wieldable_state::WieldableState::Charging(cursor) =
        weapon.state
    else {
        return None;
    };
    let timing = crate::weapon::execution::action_program(weapon, cursor.token.lane)?
        .timing
        .charge?;
    let fixed_tick = cursor.last_advanced_tick.unwrap_or(cursor.accepted_tick);
    let elapsed_ticks = fixed_tick.wrapping_sub(cursor.accepted_tick);
    Some((
        cursor.token,
        elapsed_ticks.min(timing.full_ticks) as f32 / timing.full_ticks as f32,
    ))
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
pub struct PlayerHudStatePublisher {
    ctx: ScriptCtx,
    invalid_max_warned_for: Option<EntityId>,
    write_failure_warned_slots: HashSet<&'static str>,
    /// Input-layer cursor selection. It is local on every role and deliberately
    /// never enters `Inventory`, simulation, or replication.
    pending_weapon_slot: Option<usize>,
    /// Presentation cancellation persists across zero-tick focus-return frames.
    /// Instance and token matching leaves a newer activation visible.
    charge_presentation_suppression: Option<(EntityId, ActivationToken)>,
}

impl PlayerHudStatePublisher {
    /// Build a publisher holding a clone of the engine's `ScriptCtx`.
    pub fn new(ctx: ScriptCtx) -> Self {
        Self {
            ctx,
            invalid_max_warned_for: None,
            write_failure_warned_slots: HashSet::new(),
            pending_weapon_slot: None,
            charge_presentation_suppression: None,
        }
    }

    /// Set the input layer's local pending cursor for this frame's HUD publish.
    pub fn set_pending_weapon_slot(&mut self, pending_weapon_slot: Option<usize>) {
        self.pending_weapon_slot = pending_weapon_slot;
    }

    /// Hide only the captured activation until fixed-tick cancellation settles.
    /// This changes HUD presentation without advancing or mutating execution.
    pub fn set_charge_presentation_suppression(
        &mut self,
        suppression: Option<(EntityId, ActivationToken)>,
    ) {
        self.charge_presentation_suppression = suppression;
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

    pub fn tick_for_role_and_report_sampled_weapon(
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
            let values = weapon_hud_values(&self.ctx.registry.borrow());
            self.publish_local_weapon_state(&values);
            return values.sampled;
        }
        self.tick_and_report_sampled_weapon()
    }

    /// Publish the owning local pawn's health slots from a registry the fixed-tick
    /// producer already borrows. Impact evaluation calls this after damage and
    /// before freezing ambient engine-state reads.
    pub fn publish_health_from_registry(&mut self, registry: &EntityRegistry) {
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
        let values = weapon_hud_values(&self.ctx.registry.borrow());
        self.publish_local_weapon_state(&values);
        let WeaponHudValues {
            sampled: sampled_weapon,
            ammo,
            resource,
            reload_progress,
            reload_active,
            ..
        } = values;
        // `player.health`/`player.maxHealth` mirror the live pawn HP. No pawn /
        // no health component → skip; the readonly slots retain their previous
        // values. The registry borrow is scoped to the read so it drops before
        // the `write_store_slot` calls (which borrow the slot table, a separate
        // cell).
        let pawn_health = pawn_health_values(&self.ctx.registry.borrow());
        self.publish_health_values(pawn_health);

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
        if sampled_weapon.is_some() {
            self.publish_resource_values(resource);
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

    /// Publish the live active weapon's heat/cell slots. Like the ammo pair, a
    /// weapon of another kind is an authoritative absence: its number slots
    /// clear and the latch reads false.
    fn publish_resource_values(&mut self, resource: ResourceHud) {
        match resource {
            ResourceHud::Heat {
                heat,
                overheat_at,
                overheated,
            } => {
                self.write_hud_slot("player.heat", SlotValue::Number(heat));
                self.write_hud_slot("player.overheatAt", SlotValue::Number(overheat_at));
                self.write_hud_slot("player.overheated", SlotValue::Boolean(overheated));
            }
            ResourceHud::None | ResourceHud::Ammo | ResourceHud::Cell { .. } => {
                self.clear_hud_slot("player.heat");
                self.clear_hud_slot("player.overheatAt");
                self.write_hud_slot("player.overheated", SlotValue::Boolean(false));
            }
        }
        match resource {
            ResourceHud::Cell { charge, capacity } => {
                self.write_hud_slot("player.cell", SlotValue::Number(charge));
                self.write_hud_slot("player.cellCapacity", SlotValue::Number(capacity));
            }
            ResourceHud::None | ResourceHud::Ammo | ResourceHud::Heat { .. } => {
                self.clear_hud_slot("player.cell");
                self.clear_hud_slot("player.cellCapacity");
            }
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

    fn publish_local_weapon_state(&mut self, values: &WeaponHudValues) {
        let (current, pending, switching) =
            weapon_state_values(&self.ctx.registry.borrow(), self.pending_weapon_slot);
        self.write_hud_slot("player.weapon.current", SlotValue::String(current));
        self.write_hud_slot("player.weapon.pending", SlotValue::String(pending));
        self.write_hud_slot("player.weapon.switching", SlotValue::Boolean(switching));

        // Spread is local predicted state, so every role publishes it from its
        // own active component before a connected client returns early.
        self.write_hud_slot(
            "player.spread",
            SlotValue::Number(values.effective_spread_degrees),
        );
        let charging = values.charging.filter(|(token, _)| {
            self.charge_presentation_suppression != values.sampled.map(|weapon| (weapon, *token))
        });
        self.write_hud_slot(
            "player.weaponCharging",
            SlotValue::Boolean(charging.is_some()),
        );
        self.write_hud_slot(
            "player.weaponChargeProgress",
            SlotValue::Number(charging.map_or(0.0, |(_, progress)| progress)),
        );
        // The kind follows the local active weapon, like the weapon name: the
        // host's owner-private projection has no per-pawn source for it. With
        // no live active weapon the prior kind stands (staleness contract).
        if values.sampled.is_some() {
            self.write_hud_slot(
                "player.weaponResource",
                SlotValue::Enum(values.resource.kind().as_str().to_string()),
            );
        }
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
        AirParams, AmmoResource, CapsuleParams, FallParams, GroundParams, HealthDescriptor,
        PlayerMovementDescriptor, ReloadStyle, ResolutionMode, SpeedParams, WeaponDescriptor,
        WeaponResource,
    };

    /// A minimal movement descriptor so a spawned entity qualifies as the pawn
    /// (carries `PlayerMovement`). Only the fields `from_descriptor` reads need
    /// to be sane for this test's purpose.
    fn movement_descriptor() -> PlayerMovementDescriptor {
        PlayerMovementDescriptor {
            sounds: None,
            knockback: Default::default(),
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
            slide: None,
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
            sounds: None,
            knockback: None,
            damage: 10.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            bloom_per_shot_degrees: 0.0,
            bloom_max_degrees: 0.0,
            bloom_decay_degrees_per_second: 0.0,
            bloom_decay_delay_ms: 0.0,
            movement_spread_degrees: 0.0,
            spread_vertical_bias: 0.0,
            range: 64.0,
            primary: postretro_foundation::WeaponActivationDescriptor::single(
                postretro_foundation::ActivationTrigger::Press,
                100.0,
            ),
            secondary: None,
            resolution: ResolutionMode::Hitscan,
            projectile: None,
            splash: None,
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
            weapon_hud_values(&ctx.registry.borrow()).ammo,
            Some((5, 20)),
            "ammo HUD identity is independent of the Health component"
        );
    }

    #[test]
    fn player_spread_publishes_local_effective_spread_on_every_role() {
        use crate::scripting::primitives::store::read_store_slot;

        let ctx = ScriptCtx::new();
        let pawn = spawn_movement_pawn(&ctx);
        let weapon_id = spawn_ammo_weapon(&ctx, pawn);
        {
            let mut registry = ctx.registry.borrow_mut();
            let mut movement = registry
                .get_component::<PlayerMovementComponent>(pawn)
                .unwrap()
                .clone();
            movement.velocity = glam::Vec3::new(5.5, 0.0, 0.0);
            registry.set_component(pawn, movement).unwrap();

            let mut weapon = registry
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .clone();
            weapon.spread_degrees = 2.0;
            weapon.bloom_accumulator_degrees = 1.0;
            weapon.movement_spread_degrees = 4.0;
            registry.set_component(weapon_id, weapon).unwrap();
        }
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());

        publisher.tick_for_role(true, None);
        let SlotValue::Number(client_spread) = read_store_slot(&ctx, "player.spread").unwrap()
        else {
            panic!("player.spread must be a number");
        };
        assert!(
            (client_spread - 5.0).abs() < 1e-6,
            "a connected client publishes its own predicted active-weapon spread"
        );

        publisher.tick_for_role(false, None);
        let SlotValue::Number(host_spread) = read_store_slot(&ctx, "player.spread").unwrap() else {
            panic!("player.spread must be a number");
        };
        assert!(
            (host_spread - 5.0).abs() < 1e-6,
            "the host uses the same local active-weapon projection"
        );

        let mut inventory = ctx
            .registry
            .borrow()
            .get_component::<Inventory>(pawn)
            .unwrap()
            .clone();
        inventory.wieldables[inventory.active_slot] = None;
        ctx.registry
            .borrow_mut()
            .set_component(pawn, inventory)
            .unwrap();

        publisher.tick_for_role(true, None);
        assert_eq!(
            read_store_slot(&ctx, "player.spread").unwrap(),
            SlotValue::Number(0.0),
            "no active weapon resets the local spread presentation to zero"
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
                        sounds: None,
                        knockback: None,
                        damage: 10.0,
                        pellet_count: 1,
                        spread_degrees: 0.0,
                        bloom_per_shot_degrees: 0.0,
                        bloom_max_degrees: 0.0,
                        bloom_decay_degrees_per_second: 0.0,
                        bloom_decay_delay_ms: 0.0,
                        movement_spread_degrees: 0.0,
                        spread_vertical_bias: 0.0,
                        range: 64.0,
                        primary: postretro_foundation::WeaponActivationDescriptor::single(
                            postretro_foundation::ActivationTrigger::Press,
                            100.0,
                        ),
                        secondary: None,
                        resolution: ResolutionMode::Hitscan,
                        projectile: None,
                        splash: None,
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

    fn equip_resource_weapon(ctx: &ScriptCtx, pawn: EntityId, resource: serde_json::Value) {
        let mut descriptor: WeaponDescriptor = serde_json::from_value(serde_json::json!({
            "damage": 10.0,
            "range": 64.0,
            "primary": { "trigger": "hold", "recoveryMs": 100.0, "steps": [{ "kind": "shot" }] },

            "resolution": "hitscan",
        }))
        .unwrap();
        descriptor.resource =
            (!resource.is_null()).then(|| serde_json::from_value(resource).unwrap());
        let weapon = WeaponComponent::from_descriptor(&descriptor);
        let mut registry = ctx.registry.borrow_mut();
        let id = registry.spawn(Transform::default());
        registry.set_component(id, weapon).unwrap();
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(id);
        registry.set_component(pawn, inventory).unwrap();
    }

    fn slot(ctx: &ScriptCtx, name: &str) -> Option<SlotValue> {
        ctx.slot_table.borrow().get(name).unwrap().value.clone()
    }

    fn equip_charged_weapon(ctx: &ScriptCtx, pawn: EntityId) -> EntityId {
        let descriptor: WeaponDescriptor = serde_json::from_value(serde_json::json!({
            "damage": 10.0,
            "range": 64.0,
            "resolution": "hitscan",
            "primary": {
                "trigger": "press", "recoveryMs": 100.0,
                "charge": { "minMs": 200.0, "fullMs": 1000.0 },
                "steps": [{ "kind": "shot" }]
            },
            "secondary": {
                "trigger": "press", "recoveryMs": 200.0,
                "charge": { "minMs": 200.0, "fullMs": 2000.0 },
                "steps": [{ "kind": "shot" }]
            }
        }))
        .unwrap();
        let mut registry = ctx.registry.borrow_mut();
        let weapon = registry.spawn(Transform::default());
        registry
            .set_component(weapon, WeaponComponent::from_descriptor(&descriptor))
            .unwrap();
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(weapon);
        registry.set_component(pawn, inventory).unwrap();
        weapon
    }

    fn advance_charge_for_hud(
        ctx: &ScriptCtx,
        weapon: EntityId,
        tick: u32,
        input: postretro_foundation::ActivationInput,
    ) -> crate::weapon::execution::WeaponActivationAdvance {
        use crate::weapon::FireButtonState;
        use crate::weapon::execution::ActivationCommand;
        use postretro_foundation::ActivationLane;

        let mut registry = ctx.registry.borrow_mut();
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        let active_lane = input.initiation.map(|token| token.lane).or_else(|| {
            component
                .state
                .activation_cursor()
                .map(|cursor| cursor.token.lane)
        });
        let button = |lane| FireButtonState {
            pressed: input.initiation.is_some_and(|token| token.lane == lane),
            active: active_lane == Some(lane) && input.release.is_none() && input.cancel.is_none(),
        };
        let result = crate::weapon::activation_prediction::advance_predicted_weapon_tick(
            &mut component,
            ActivationCommand {
                tick,
                pawn: 1,
                real_command: true,
                input,
                controller_starts: false,
                primary: button(ActivationLane::Primary),
                secondary: button(ActivationLane::Secondary),
            },
            false,
            1000.0 / 60.0,
            true,
        );
        registry.set_component(weapon, component).unwrap();
        result
    }

    fn hold_charge_for_hud(
        ctx: &ScriptCtx,
        weapon: EntityId,
        token: ActivationToken,
        held_ticks: u32,
    ) {
        for elapsed in 0..=held_ticks {
            advance_charge_for_hud(
                ctx,
                weapon,
                token.start_tick.wrapping_add(elapsed),
                postretro_foundation::ActivationInput {
                    initiation: (elapsed == 0).then_some(token),
                    ..Default::default()
                },
            );
        }
    }

    fn assert_charge_hud(ctx: &ScriptCtx, charging: bool, progress: f32) {
        assert_eq!(
            slot(ctx, "player.weaponCharging"),
            Some(SlotValue::Boolean(charging))
        );
        let Some(SlotValue::Number(actual)) = slot(ctx, "player.weaponChargeProgress") else {
            panic!("charge progress must be a number");
        };
        assert!((actual - progress).abs() < 1e-6, "{actual} != {progress}");
    }

    #[test]
    fn weapon_charge_hud_tracks_fixed_ticks_and_clears_after_release() {
        use postretro_foundation::{ActivationInput, ActivationLane, ActivationRelease};

        let ctx = ScriptCtx::new();
        let pawn = spawn_pawn_with_health(&ctx, 100.0);
        let weapon = equip_charged_weapon(&ctx, pawn);
        let token = ActivationToken {
            start_tick: 10,
            lane: ActivationLane::Primary,
        };
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
        hold_charge_for_hud(&ctx, weapon, token, 30);
        publisher.tick(None);
        assert_charge_hud(&ctx, true, 0.5);

        for tick in 41..=75 {
            advance_charge_for_hud(&ctx, weapon, tick, ActivationInput::default());
        }
        publisher.tick(None);
        assert_charge_hud(&ctx, true, 1.0);
        let released = advance_charge_for_hud(
            &ctx,
            weapon,
            76,
            ActivationInput {
                release: Some(ActivationRelease {
                    token,
                    release_tick: 76,
                }),
                ..Default::default()
            },
        );
        assert!(
            released.shot.is_some(),
            "a valid release executes through the shared machine"
        );
        publisher.tick(None);
        assert_charge_hud(&ctx, false, 0.0);
    }

    #[test]
    fn weapon_charge_hud_connected_client_reads_secondary_lane_and_clears_cancel() {
        use postretro_foundation::{ActivationInput, ActivationLane};

        let ctx = ScriptCtx::new();
        let pawn = spawn_pawn_with_health(&ctx, 100.0);
        let weapon = equip_charged_weapon(&ctx, pawn);
        let token = ActivationToken {
            start_tick: 0,
            lane: ActivationLane::Secondary,
        };
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
        hold_charge_for_hud(&ctx, weapon, token, 60);
        publisher.tick_for_role(true, None);
        assert_charge_hud(&ctx, true, 0.5);
        assert_eq!(
            slot(&ctx, "player.health"),
            None,
            "connected client still suppresses authoritative health writes"
        );

        let cancelled = advance_charge_for_hud(
            &ctx,
            weapon,
            61,
            ActivationInput {
                cancel: Some(token),
                ..Default::default()
            },
        );
        assert!(cancelled.shot.is_none());
        publisher.tick_for_role(true, None);
        assert_charge_hud(&ctx, false, 0.0);
    }

    #[test]
    fn weapon_charge_hud_clears_on_switch_unarmed_death_and_early_release() {
        use postretro_foundation::{ActivationInput, ActivationLane, ActivationRelease};

        for transition in ["switch", "unarmed", "death", "early-release"] {
            let ctx = ScriptCtx::new();
            let pawn = spawn_pawn_with_health(&ctx, 100.0);
            let weapon = equip_charged_weapon(&ctx, pawn);
            let token = ActivationToken {
                start_tick: 0,
                lane: ActivationLane::Primary,
            };
            let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
            hold_charge_for_hud(&ctx, weapon, token, 5);
            publisher.tick(None);
            assert_charge_hud(&ctx, true, 5.0 / 60.0);
            match transition {
                "switch" => {
                    let mut registry = ctx.registry.borrow_mut();
                    let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
                    inventory.switch_target = Some(1);
                    registry.set_component(pawn, inventory).unwrap();
                }
                "unarmed" => {
                    let mut registry = ctx.registry.borrow_mut();
                    let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
                    inventory.wieldables[0] = None;
                    registry.set_component(pawn, inventory).unwrap();
                }
                "death" => {
                    let mut registry = ctx.registry.borrow_mut();
                    let mut health = registry
                        .get_component::<HealthComponent>(pawn)
                        .unwrap()
                        .clone();
                    health.current = 0.0;
                    registry.set_component(pawn, health).unwrap();
                }
                "early-release" => {
                    let result = advance_charge_for_hud(
                        &ctx,
                        weapon,
                        6,
                        ActivationInput {
                            release: Some(ActivationRelease {
                                token,
                                release_tick: 6,
                            }),
                            ..Default::default()
                        },
                    );
                    assert!(result.shot.is_none());
                }
                _ => unreachable!(),
            }
            publisher.tick(None);
            assert_charge_hud(&ctx, false, 0.0);
        }
    }

    // Regression: focus return without a fixed tick must not reveal cancelled charge.
    #[test]
    fn weapon_charge_hud_zero_tick_suppression_persists_but_new_token_is_visible() {
        use postretro_foundation::{ActivationInput, ActivationLane};

        let ctx = ScriptCtx::new();
        let pawn = spawn_pawn_with_health(&ctx, 100.0);
        let weapon = equip_charged_weapon(&ctx, pawn);
        let token = ActivationToken {
            start_tick: 0,
            lane: ActivationLane::Primary,
        };
        hold_charge_for_hud(&ctx, weapon, token, 30);
        let before = ctx
            .registry
            .borrow()
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .state;
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
        publisher.tick_for_role(true, None);
        assert_charge_hud(&ctx, true, 0.5);
        publisher.set_charge_presentation_suppression(Some((weapon, token)));
        for _ in 0..3 {
            publisher.tick_for_role(true, None);
            assert_charge_hud(&ctx, false, 0.0);
        }
        assert_eq!(
            ctx.registry
                .borrow()
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .state,
            before,
            "HUD publication does not advance or mutate the cursor"
        );

        advance_charge_for_hud(
            &ctx,
            weapon,
            31,
            ActivationInput {
                cancel: Some(token),
                ..Default::default()
            },
        );
        let newer = ActivationToken {
            start_tick: 32,
            ..token
        };
        hold_charge_for_hud(&ctx, weapon, newer, 30);
        publisher.tick_for_role(true, None);
        assert_charge_hud(&ctx, true, 0.5);

        let replacement = equip_charged_weapon(&ctx, pawn);
        publisher.set_charge_presentation_suppression(Some((weapon, newer)));
        hold_charge_for_hud(&ctx, replacement, newer, 30);
        publisher.tick_for_role(true, None);
        assert_charge_hud(&ctx, true, 0.5);
    }

    #[test]
    fn weapon_charge_hud_uses_wrap_safe_fixed_ticks() {
        use postretro_foundation::ActivationLane;

        let ctx = ScriptCtx::new();
        let pawn = spawn_pawn_with_health(&ctx, 100.0);
        let weapon = equip_charged_weapon(&ctx, pawn);
        let token = ActivationToken {
            start_tick: u32::MAX - 15,
            lane: ActivationLane::Primary,
        };
        hold_charge_for_hud(&ctx, weapon, token, 30);
        PlayerHudStatePublisher::new(ctx.clone()).tick(None);
        assert_charge_hud(&ctx, true, 0.5);
    }

    fn heat_resource() -> serde_json::Value {
        serde_json::json!({
            "kind": "heat", "heatPerShot": 10.0, "overheatAt": 80.0, "coolPerSecond": 20.0
        })
    }

    fn cell_resource() -> serde_json::Value {
        serde_json::json!({
            "kind": "cell", "capacity": 40.0, "costPerShot": 4.0, "regenPerSecond": 8.0
        })
    }

    fn set_live_heat(ctx: &ScriptCtx, pawn: EntityId, heat: f32, overheated: bool) {
        let mut registry = ctx.registry.borrow_mut();
        let weapon = registry
            .get_component::<Inventory>(pawn)
            .unwrap()
            .active_wieldable()
            .unwrap();
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        let live = component.heat.as_mut().unwrap();
        live.heat = heat;
        live.overheated = overheated;
        registry.set_component(weapon, component).unwrap();
    }

    #[test]
    fn heat_weapon_publishes_heat_max_and_latch_and_clears_cell_and_ammo() {
        let ctx = ScriptCtx::new();
        let pawn = spawn_movement_pawn(&ctx);
        let _ammo = spawn_ammo_weapon(&ctx, pawn);
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
        publisher.tick(None);
        assert_eq!(
            slot(&ctx, "player.weaponResource"),
            Some(SlotValue::Enum("ammo".into()))
        );

        equip_resource_weapon(&ctx, pawn, cell_resource());
        publisher.tick(None);
        equip_resource_weapon(&ctx, pawn, heat_resource());
        set_live_heat(&ctx, pawn, 80.0, true);
        publisher.tick(None);

        assert_eq!(
            slot(&ctx, "player.weaponResource"),
            Some(SlotValue::Enum("heat".into()))
        );
        assert_eq!(slot(&ctx, "player.heat"), Some(SlotValue::Number(80.0)));
        assert_eq!(
            slot(&ctx, "player.overheatAt"),
            Some(SlotValue::Number(80.0))
        );
        assert_eq!(
            slot(&ctx, "player.overheated"),
            Some(SlotValue::Boolean(true))
        );
        assert_eq!(slot(&ctx, "player.cell"), None);
        assert_eq!(slot(&ctx, "player.cellCapacity"), None);
        assert_eq!(slot(&ctx, "player.ammo"), None);
        assert_eq!(slot(&ctx, "player.ammoReserve"), None);
    }

    #[test]
    fn cell_weapon_publishes_charge_and_capacity_and_clears_heat_and_latch() {
        let ctx = ScriptCtx::new();
        let pawn = spawn_movement_pawn(&ctx);
        equip_resource_weapon(&ctx, pawn, heat_resource());
        set_live_heat(&ctx, pawn, 40.0, true);
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
        publisher.tick(None);
        assert_eq!(
            slot(&ctx, "player.overheated"),
            Some(SlotValue::Boolean(true))
        );

        equip_resource_weapon(&ctx, pawn, cell_resource());
        publisher.tick(None);

        assert_eq!(
            slot(&ctx, "player.weaponResource"),
            Some(SlotValue::Enum("cell".into()))
        );
        assert_eq!(slot(&ctx, "player.cell"), Some(SlotValue::Number(40.0)));
        assert_eq!(
            slot(&ctx, "player.cellCapacity"),
            Some(SlotValue::Number(40.0))
        );
        assert_eq!(slot(&ctx, "player.heat"), None);
        assert_eq!(slot(&ctx, "player.overheatAt"), None);
        assert_eq!(
            slot(&ctx, "player.overheated"),
            Some(SlotValue::Boolean(false))
        );
    }

    #[test]
    fn heat_cell_slots_clear_for_ammo_and_resourceless_weapons_and_hold_without_one() {
        for kind in ["none", "ammo"] {
            let ctx = ScriptCtx::new();
            let pawn = spawn_movement_pawn(&ctx);
            equip_resource_weapon(&ctx, pawn, cell_resource());
            let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
            publisher.tick(None);
            equip_resource_weapon(&ctx, pawn, heat_resource());
            set_live_heat(&ctx, pawn, 30.0, true);
            publisher.tick(None);

            if kind == "ammo" {
                let _ = spawn_ammo_weapon(&ctx, pawn);
            } else {
                equip_resource_weapon(&ctx, pawn, serde_json::Value::Null);
            }
            publisher.tick(None);
            assert_eq!(
                slot(&ctx, "player.weaponResource"),
                Some(SlotValue::Enum(kind.into()))
            );
            for cleared in [
                "player.heat",
                "player.overheatAt",
                "player.cell",
                "player.cellCapacity",
            ] {
                assert_eq!(slot(&ctx, cleared), None, "{kind}: {cleared}");
            }
            assert_eq!(
                slot(&ctx, "player.overheated"),
                Some(SlotValue::Boolean(false))
            );
        }

        // No active weapon keeps the last published values (staleness contract).
        let ctx = ScriptCtx::new();
        let pawn = spawn_movement_pawn(&ctx);
        equip_resource_weapon(&ctx, pawn, cell_resource());
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());
        publisher.tick(None);
        ctx.registry
            .borrow_mut()
            .set_component(pawn, Inventory::default())
            .unwrap();
        publisher.tick(None);
        assert_eq!(
            slot(&ctx, "player.weaponResource"),
            Some(SlotValue::Enum("cell".into()))
        );
        assert_eq!(slot(&ctx, "player.cell"), Some(SlotValue::Number(40.0)));
    }

    #[test]
    fn connected_client_publishes_its_own_resource_kind_but_never_the_values() {
        let ctx = ScriptCtx::new();
        let pawn = spawn_movement_pawn(&ctx);
        equip_resource_weapon(&ctx, pawn, cell_resource());
        let mut publisher = PlayerHudStatePublisher::new(ctx.clone());

        publisher.tick_for_role(true, None);
        assert_eq!(
            slot(&ctx, "player.weaponResource"),
            Some(SlotValue::Enum("cell".into())),
            "the kind comes from the client's own active weapon"
        );
        for replicated in [
            "player.heat",
            "player.overheatAt",
            "player.cell",
            "player.cellCapacity",
        ] {
            assert_eq!(
                slot(&ctx, replicated),
                None,
                "{replicated} reaches a client only through replication"
            );
        }
        assert_eq!(
            slot(&ctx, "player.overheated"),
            Some(SlotValue::Boolean(false)),
            "the latch keeps its catalog default until replicated"
        );

        equip_resource_weapon(&ctx, pawn, heat_resource());
        publisher.tick_for_role(true, None);
        assert_eq!(
            slot(&ctx, "player.weaponResource"),
            Some(SlotValue::Enum("heat".into()))
        );

        ctx.registry
            .borrow_mut()
            .set_component(pawn, Inventory::default())
            .unwrap();
        publisher.tick_for_role(true, None);
        assert_eq!(
            slot(&ctx, "player.weaponResource"),
            Some(SlotValue::Enum("heat".into())),
            "no active weapon keeps the last kind"
        );
    }
}
