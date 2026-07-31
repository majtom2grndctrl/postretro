use std::cell::RefCell;
use std::rc::Rc;

use glam::Vec3;

use crate::collision::CollisionWorld;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::weapon::{self, FireButtonState, WeaponFireAuthorization, WeaponFireCommand};
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{EntityId, EntityRegistry};

use super::super::{OpenAuthorizedShot, PostMovementCommand, ReloadDelivery, RemotePawnCommand};
use super::impact::apply_weapon_impact_damage;
use super::machine::tick_weapon_machine;
use super::state::{
    WieldableStateEvent, begin_raising, finish_lowering, transition_wieldable_state,
};

pub(in crate::sim) fn weapon_fire_command(
    button: FireButtonState,
    post_movement: PostMovementCommand,
) -> WeaponFireCommand {
    // The aim normalization and `can_fire` gate below are degenerate-input guards.
    // `camera.aim_ray()` already returns normalized, finite values in normal operation;
    // these checks protect against NaN/zero vectors from headless or mocked callers.
    if post_movement.aim_origin.is_finite()
        && let Some(aim_direction) = normalize_aim_direction(post_movement.aim_direction)
    {
        return WeaponFireCommand {
            button,
            aim_origin: post_movement.aim_origin,
            aim_direction,
            can_fire: true,
        };
    }

    WeaponFireCommand {
        button,
        aim_origin: Vec3::ZERO,
        aim_direction: Vec3::Z,
        can_fire: false,
    }
}

fn normalize_aim_direction(direction: Vec3) -> Option<Vec3> {
    if !direction.is_finite() {
        return None;
    }
    let length_squared = direction.length_squared();
    if !length_squared.is_finite() || length_squared <= 1.0e-12 {
        return None;
    }
    Some(direction / length_squared.sqrt())
}

pub(in crate::sim) fn run_remote_weapon_commands(
    registry: &Rc<RefCell<EntityRegistry>>,
    remote_pawn_commands: &[RemotePawnCommand],
    tick_dt: f32,
) -> (
    Vec<OpenAuthorizedShot>,
    Vec<ReloadDelivery>,
    Vec<&'static str>,
) {
    let mut registry = registry.borrow_mut();
    let mut authorized = Vec::new();
    let mut reload_deliveries = Vec::new();
    let mut weapon_events = Vec::new();

    for remote in remote_pawn_commands {
        // Remote authorization requires live ownership. A delayed command for a
        // despawned pawn must not mutate its former weapon or mint an open shot.
        if !registry.exists(remote.pawn) {
            continue;
        }
        let Some(weapon) = remote.weapon else {
            continue;
        };
        let Ok(mut weapon_component) = registry.get_component::<WeaponComponent>(weapon).cloned()
        else {
            continue;
        };
        let command = WeaponFireCommand {
            button: remote.command.fire_button,
            aim_origin: Vec3::ZERO,
            aim_direction: Vec3::Z,
            // Repurposes `can_fire` (elsewhere "aim valid") to mean "pawn has a NetworkId";
            // the real fire gate is `button` -> `wants_fire`. The host casts no local aim ray.
            can_fire: remote.shot_id.is_some(),
        };
        let machine = tick_weapon_machine(
            &mut registry,
            Some(remote.pawn),
            weapon,
            &mut weapon_component,
            remote.command.reload,
            &command,
            tick_dt,
        );
        reload_deliveries.extend(machine.deliveries);
        let effective = weapon_component.effective();
        let damage = effective.damage;
        let range = effective.range;
        let credit_source = effective.credit_source.to_string();
        let _ = registry.set_component(weapon, weapon_component);
        match machine.authorization {
            WeaponFireAuthorization::Accepted => weapon_events.push("activate"),
            WeaponFireAuthorization::Empty => {
                weapon_events.push("dry_fire");
                continue;
            }
            WeaponFireAuthorization::Rejected => continue,
        }
        let Some(shot_id) = remote.shot_id else {
            continue;
        };
        authorized.push(OpenAuthorizedShot {
            shot: super::super::AuthorizedShot {
                shot_id,
                pawn: remote.pawn,
                weapon,
                fire_tick: remote.fire_tick,
                damage,
                range,
                pellet_count: 1,
                credit_source,
            },
            owner_client_id: remote.owner_client_id,
        });
    }

    (authorized, reload_deliveries, weapon_events)
}

#[allow(clippy::too_many_arguments)]
pub(in crate::sim) fn run_local_weapon_command(
    registry: &Rc<RefCell<EntityRegistry>>,
    pawn: Option<EntityId>,
    active_wieldable: Option<EntityId>,
    select_slot: Option<usize>,
    command: &WeaponFireCommand,
    reload_pressed: bool,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    tick_dt: f32,
    on_impact: &mut impl FnMut(&mut EntityRegistry),
) -> (Vec<ReloadDelivery>, Vec<&'static str>) {
    let mut registry = registry.borrow_mut();
    // Task 3 retires the temporary App-held active handle. Keeping it as a
    // fallback here preserves the split driver's focused legacy fixtures
    // until then; production player composition always supplies Inventory.
    let mut inventory =
        pawn.and_then(|pawn| registry.get_component::<Inventory>(pawn).ok().cloned());
    let weapon_id = inventory
        .as_ref()
        .and_then(Inventory::active_wieldable)
        .or(active_wieldable);
    let Some(weapon_id) = weapon_id else {
        return (Vec::new(), Vec::new());
    };
    let begin_lower = inventory.as_ref().is_some_and(|inventory| {
        select_slot.is_some_and(|slot| {
            slot < inventory.wieldables.len()
                && slot != inventory.active_slot
                && inventory.wieldables[slot].is_some()
                && inventory.switch_target != Some(slot)
        })
    });
    if begin_lower {
        if let Some(inventory) = inventory.as_mut() {
            inventory.switch_target = select_slot;
        }
    }
    let Ok(mut weapon_component) = registry
        .get_component::<WeaponComponent>(weapon_id)
        .cloned()
    else {
        return (Vec::new(), Vec::new());
    };
    if begin_lower {
        let lower_ms = weapon_component.lower_ms;
        let _ = transition_wieldable_state(
            &mut weapon_component,
            WieldableStateEvent::BeginLower {
                duration_ms: lower_ms,
            },
            None,
        );
    }
    let machine = tick_weapon_machine(
        &mut registry,
        pawn,
        weapon_id,
        &mut weapon_component,
        reload_pressed,
        command,
        tick_dt,
    );
    let events = weapon::tick_resolved_component(
        &registry,
        &mut weapon_component,
        command,
        collision_world,
        hit_zone_store,
        anim_time,
        machine.authorization,
    );
    if machine.lowered {
        if let (Some(pawn), Some(inventory)) = (pawn, inventory.as_mut())
            && let Some(target_slot) = inventory.switch_target
            && let Some(incoming_id) = inventory.wieldables[target_slot]
            && let Ok(mut incoming) = registry
                .get_component::<WeaponComponent>(incoming_id)
                .cloned()
        {
            finish_lowering(&mut weapon_component);
            incoming.reload_press_consumed = reload_pressed;
            incoming.cooldown_remaining_ms =
                incoming.cooldown_remaining_ms.max(incoming.raise_ms as f32);
            begin_raising(&mut incoming);
            inventory.active_slot = target_slot;
            inventory.switch_target = None;
            let _ = registry.set_component(incoming_id, incoming);
            let _ = registry.set_component(pawn, inventory.clone());
        }
    } else if begin_lower {
        if let (Some(pawn), Some(inventory)) = (pawn, inventory) {
            let _ = registry.set_component(pawn, inventory);
        }
    }
    let _ = registry.set_component(weapon_id, weapon_component);
    if let Some(impact) = events.impact.as_ref() {
        weapon::spawn_impact_effect_at(&mut registry, impact.point, impact.normal);
        apply_weapon_impact_damage(&mut registry, Some(weapon_id), pawn, impact);
        on_impact(&mut registry);
    }
    (machine.deliveries, events.event_names())
}
