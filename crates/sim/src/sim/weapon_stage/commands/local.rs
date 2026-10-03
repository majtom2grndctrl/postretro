// Weapon activation consumer and resolution seam.
// See: context/lib/entity_model.md §4, §5 · context/lib/networking.md
use super::*;

#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::too_many_arguments)]
pub(in crate::sim) fn run_local_weapon_command(
    registry: &Rc<RefCell<EntityRegistry>>,
    pawn: Option<EntityId>,
    mod_block_during_reload: bool,
    select_slot: Option<usize>,
    command: &WeaponFireCommand,
    reload_pressed: bool,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    tick_dt: f32,
    on_impact: &mut impl FnMut(&mut EntityRegistry),
) -> LocalWeaponCommandResult {
    run_local_weapon_command_with_content(
        registry,
        pawn,
        mod_block_during_reload,
        &[],
        None,
        select_slot,
        command,
        reload_pressed,
        collision_world,
        hit_zone_store,
        anim_time,
        tick_dt,
        on_impact,
        None,
    )
}

/// Local authoritative command with the descriptor context required to resolve
/// steady placement for a projectile muzzle. The wrapper above keeps headless
/// test fixtures on their explicit no-content path.
#[allow(clippy::too_many_arguments)]
pub(in crate::sim) fn run_local_weapon_command_with_content(
    registry: &Rc<RefCell<EntityRegistry>>,
    pawn: Option<EntityId>,
    mod_block_during_reload: bool,
    descriptors: &[EntityTypeDescriptor],
    default_weapon_placement: Option<&WeaponPlacementDescriptor>,
    select_slot: Option<usize>,
    command: &WeaponFireCommand,
    reload_pressed: bool,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    tick_dt: f32,
    on_impact: &mut impl FnMut(&mut EntityRegistry),
    activation: Option<weapon::execution::ActivationCommand>,
) -> LocalWeaponCommandResult {
    run_local_weapon_command_inner(
        registry,
        pawn,
        mod_block_during_reload,
        descriptors,
        default_weapon_placement,
        select_slot,
        command,
        reload_pressed,
        collision_world,
        hit_zone_store,
        anim_time,
        tick_dt,
        on_impact,
        activation,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::sim) fn run_client_weapon_equip(
    registry: &Rc<RefCell<EntityRegistry>>,
    pawn: Option<EntityId>,
    mod_block_during_reload: bool,
    select_slot: Option<usize>,
    command: &WeaponFireCommand,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    tick_dt: f32,
) -> LocalWeaponCommandResult {
    run_local_weapon_command_inner(
        registry,
        pawn,
        mod_block_during_reload,
        &[],
        None,
        select_slot,
        command,
        false,
        collision_world,
        hit_zone_store,
        anim_time,
        tick_dt,
        &mut |_| {},
        None,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_local_weapon_command_inner(
    registry: &Rc<RefCell<EntityRegistry>>,
    pawn: Option<EntityId>,
    mod_block_during_reload: bool,
    descriptors: &[EntityTypeDescriptor],
    default_weapon_placement: Option<&WeaponPlacementDescriptor>,
    select_slot: Option<usize>,
    command: &WeaponFireCommand,
    reload_pressed: bool,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    tick_dt: f32,
    on_impact: &mut impl FnMut(&mut EntityRegistry),
    activation: Option<weapon::execution::ActivationCommand>,
    equip_only: bool,
) -> LocalWeaponCommandResult {
    let mut registry = registry.borrow_mut();
    let mut inventory = pawn.and_then(|pawn| {
        normalize_inventory_liveness(&mut registry, pawn).map(|(inventory, _)| inventory)
    });
    let weapon_id = inventory.as_ref().and_then(Inventory::active_wieldable);
    let Some(weapon_id) = weapon_id else {
        return LocalWeaponCommandResult::default();
    };
    let Ok(mut weapon_component) = registry
        .get_component::<WeaponComponent>(weapon_id)
        .cloned()
    else {
        return LocalWeaponCommandResult::default();
    };
    let pawn_alive = pawn.is_some_and(|pawn| {
        registry.exists(pawn)
            && !crate::scripting_systems::health::is_terminally_committed_to_removal(
                &registry, pawn,
            )
            && !registry
                .get_component::<postretro_entities::components::health::HealthComponent>(pawn)
                .is_ok_and(|health| health.current <= 0.0 || !health.current.is_finite())
    });
    if !pawn_alive {
        weapon_component.cancel_activation();
    }
    let mut checked_command = *command;
    checked_command.can_fire &= pawn_alive;
    let command = &checked_command;
    let active_slot = inventory
        .as_ref()
        .map_or(0, |inventory| inventory.active_slot);
    let authored_placement = registry
        .get_component::<DescriptorProvenance>(weapon_id)
        .ok()
        .and_then(|provenance| {
            descriptors.iter().find(|descriptor| {
                descriptor.canonical_name.as_deref() == Some(provenance.canonical_name.as_str())
            })
        })
        .and_then(|descriptor| descriptor.weapon.as_ref())
        .and_then(|weapon| weapon.placement.as_ref());
    // Resolve from the pre-switch active weapon captured above. A same-tick
    // switch may repoint inventory later, but it cannot change this shot.
    let placement = postretro_foundation::resolve_weapon_placement(
        default_weapon_placement,
        None,
        authored_placement,
        None,
    );
    let pellet_salt_name =
        (!equip_only).then(|| weapon::pellet_salt_name(&registry, weapon_id, &weapon_component));
    // The descriptor override stays unresolved in the component. Only this
    // App-fed local input gate resolves it against the mod-global policy.
    let block_during_reload = weapon_component
        .block_during_reload
        .unwrap_or(mod_block_during_reload);
    let begin_lower = inventory.as_ref().is_some_and(|inventory| {
        select_slot.is_some_and(|slot| {
            slot < inventory.wieldables.len()
                && slot != inventory.active_slot
                && inventory.wieldables[slot].is_some()
                && inventory.switch_target != Some(slot)
                && !(block_during_reload && weapon_component.state.is_reload_activity())
        })
    });
    // An atomic reload already due this tick resolves before the accepted switch
    // owns the state machine. This preserves its credit and terminal delivery;
    // non-expired reloads still take the normal preempt-to-lower path below.
    let complete_reload_before_lower = begin_lower
        && weapon_component.state == WieldableState::Reloading
        && crate::sim::reload::timer_expires_this_tick(&weapon_component, tick_dt);
    if begin_lower && let Some(inventory) = inventory.as_mut() {
        // Each accepted declaration supersedes the rollback origin retained for
        // the prior one. Correlated refusals ignore the older declaration.
        inventory.switch_origin = Some(inventory.active_slot);
        inventory.switch_target = select_slot;
    }
    if begin_lower && !complete_reload_before_lower {
        let lower_ms = weapon_component.lower_ms;
        let _ = transition_wieldable_state(
            &mut weapon_component,
            WieldableStateEvent::BeginLower {
                duration_ms: lower_ms,
            },
            None,
        );
    }
    let mut machine = if equip_only {
        super::super::machine::tick_weapon_equip_only(
            &mut registry,
            pawn,
            weapon_id,
            &mut weapon_component,
            tick_dt,
        )
    } else if let Some(activation) = activation {
        tick_weapon_machine_activation(
            &mut registry,
            pawn,
            weapon_id,
            &mut weapon_component,
            reload_pressed,
            command,
            begin_lower,
            tick_dt,
            activation,
            false,
        )
    } else {
        tick_weapon_machine(
            &mut registry,
            pawn,
            weapon_id,
            &mut weapon_component,
            reload_pressed,
            command,
            begin_lower,
            tick_dt,
        )
    };
    // Credit belongs to the weapon that passed the firing state machine, even
    // when this same tick completes a lower or an impact policy repoints the
    // inventory before later pellets land.
    let fire_snapshot = (
        weapon_id,
        weapon_component.effective().credit_source.to_string(),
    );
    if complete_reload_before_lower {
        let lower_ms = weapon_component.lower_ms;
        let _ = transition_wieldable_state(
            &mut weapon_component,
            WieldableStateEvent::BeginLower {
                duration_ms: lower_ms,
            },
            None,
        );
        // The outgoing instance has not been ticked as Lowering yet. A zero
        // lower therefore resolves exactly once here, without a second machine
        // pass that would advance cooldown or fire input a second time.
        machine.lowered = lower_ms == 0;
    }
    let mut events = if let Some(attempt) = machine.activation.shot.clone() {
        let shot = weapon::freeze_weapon_shot(&weapon_component, attempt);
        weapon::resolve_activation_shot(
            &registry,
            pawn,
            &mut weapon_component,
            pellet_salt_name.as_deref().unwrap_or("weapon.unknown"),
            active_slot,
            command,
            &placement,
            collision_world,
            hit_zone_store,
            anim_time,
            shot,
        )
    } else {
        weapon::tick_resolved_component(
            &registry,
            pawn,
            &mut weapon_component,
            pellet_salt_name.as_deref().unwrap_or("weapon.unknown"),
            active_slot,
            command,
            &placement,
            collision_world,
            hit_zone_store,
            anim_time,
            machine.authorization,
        )
    };
    events.overheat = machine.overheat;
    #[cfg(test)]
    // Determinism tests compare the cast set, including pellets a policy makes
    // inapplicable. Capture it before the first policy runs.
    let weapon_impact_points = events.impacts.iter().map(|impact| impact.point).collect();
    let mut repointed_pawn = None;
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
            repointed_pawn = Some(pawn);
        }
    } else if begin_lower && let (Some(pawn), Some(inventory)) = (pawn, inventory) {
        let _ = registry.set_component(pawn, inventory);
    }
    let _ = registry.set_component(weapon_id, weapon_component);
    // Fire, dry fire and spawn sound from the firing pawn (the weapon itself
    // when no pawn holds it); impacts carry their contacts instead.
    let shooter = entity_emitter(&registry, pawn.unwrap_or(weapon_id));
    let weapon_name = descriptor_name(&registry, weapon_id);
    let reload_emissions = machine
        .deliveries
        .iter()
        .map(|delivery| reload_emission(&registry, delivery))
        .collect();
    let mut projectile_spawns = Vec::new();
    if let Some(pawn) = pawn {
        let mut activation = None;
        for launch in std::mem::take(&mut events.projectile_launches) {
            let source = ProjectileSource {
                weapon: weapon_name.clone(),
                activation,
            };
            if let Some(projectile_id) =
                spawn_projectile(&mut registry, pawn, weapon_id, launch, None, source)
            {
                activation.get_or_insert(projectile_id);
                events
                    .spawned
                    .push(weapon::ActivationOutcome::Spawned(projectile_id));
                projectile_spawns.push(projectile_id);
            }
        }
    }
    for impact in &events.impacts {
        weapon::spawn_impact_effect_at(&mut registry, impact.point, impact.normal);

        if let Some(target) = impact.target {
            // Match the host's per-record target check. A policy run for an
            // earlier pellet may have removed the shooter or committed the
            // target to removal; the cast keeps its FX but does no later damage
            // or policy work.
            if !pawn.is_some_and(|pawn| registry.exists(pawn)) {
                continue;
            }
            if !registry.exists(target)
                || registry.get_component::<HealthComponent>(target).is_err()
                || crate::scripting_systems::health::is_terminally_committed_to_removal(
                    &registry, target,
                )
            {
                continue;
            }
        }
        if let weapon::ActivationOutcome::Hit(payload) = impact.outcome {
            apply_authorized_weapon_impact_damage(
                &mut registry,
                fire_snapshot.0,
                pawn,
                impact,
                fire_snapshot.1.clone(),
                payload.amount,
            );
        }
        on_impact(&mut registry);
    }
    LocalWeaponCommandResult {
        reload_deliveries: machine.deliveries,
        reload_emissions,
        weapon_events: events.emissions(&shooter, weapon_name),
        repointed_pawn,
        projectile_spawns,
        #[cfg(test)]
        weapon_impact_points,
    }
}
