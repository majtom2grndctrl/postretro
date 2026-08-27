use std::cell::RefCell;
use std::rc::Rc;

use glam::{Quat, Vec3};

use crate::collision::CollisionWorld;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::weapon::{self, FireButtonState, WeaponFireAuthorization, WeaponFireCommand};
use postretro_entities::components::billboard_emitter::{BillboardEmitterComponent, LifetimeCurve};
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::light::{LightComponent, LightKind};
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::components::sprite_visual::SpriteVisual;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::components::wieldable_state::WieldableState;
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{EntityId, EntityRegistry, EntityTypeDescriptor, Transform};
use postretro_foundation::{
    FireMode, ProjectileBodyVisual, ResolutionMode, WeaponPlacementDescriptor,
};

use super::super::{
    OpenAuthorizedShot, PostMovementCommand, ReloadDelivery, RemotePawnCommand,
    RemoteProjectileFireRejection, RemoteProjectilePresentationLaunch,
};
use super::impact::apply_authorized_weapon_impact_damage;
use super::machine::tick_weapon_machine;
use super::state::{
    WieldableStateEvent, begin_raising, finish_lowering, transition_wieldable_state,
};

#[derive(Debug, Default)]
pub(in crate::sim) struct LocalWeaponCommandResult {
    pub(in crate::sim) reload_deliveries: Vec<ReloadDelivery>,
    pub(in crate::sim) weapon_events: Vec<&'static str>,
    pub(in crate::sim) repointed_pawn: Option<EntityId>,
    pub(in crate::sim) projectile_spawns: Vec<EntityId>,
    #[cfg(test)]
    pub(in crate::sim) weapon_impact_points: Vec<Vec3>,
}

#[derive(Debug, Default)]
pub(in crate::sim) struct RemoteWeaponCommandResult {
    pub(in crate::sim) authorized_shots: Vec<OpenAuthorizedShot>,
    pub(in crate::sim) projectile_presentation_launches: Vec<RemoteProjectilePresentationLaunch>,
    pub(in crate::sim) rejected_projectile_fires: Vec<RemoteProjectileFireRejection>,
    pub(in crate::sim) reload_deliveries: Vec<ReloadDelivery>,
    pub(in crate::sim) weapon_events: Vec<&'static str>,
}

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
) -> RemoteWeaponCommandResult {
    let mut registry = registry.borrow_mut();
    let mut authorized = Vec::new();
    let mut projectile_presentations = Vec::new();
    let mut rejected_projectile_fires = Vec::new();
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
        let projectile_fire_intended = {
            let effective = weapon_component.effective();
            matches!(effective.resolution, ResolutionMode::Projectile)
                && match effective.fire_mode {
                    FireMode::Semi => remote.command.fire_button.pressed,
                    FireMode::Auto => remote.command.fire_button.active,
                }
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
            false,
            tick_dt,
        );
        reload_deliveries.extend(machine.deliveries);
        let effective = weapon_component.effective();
        let damage = effective.damage;
        let range = effective.range;
        let pellet_count = effective.pellet_count as usize;
        let credit_source = effective.credit_source.to_string();
        let resolution = effective.resolution;
        let projectile = effective.projectile.cloned();
        let projectile_presentation = matches!(effective.resolution, ResolutionMode::Projectile)
            .then(|| {
                let projectile = projectile.clone()?;
                let transform = registry.get_component::<Transform>(remote.pawn).ok()?;
                let movement = registry
                    .get_component::<postretro_foundation::PlayerMovementComponent>(remote.pawn)
                    .ok()?;
                let yaw = remote.command.movement.facing_yaw;
                let pitch = remote.aim_pitch;
                if !yaw.is_finite() || !pitch.is_finite() {
                    return None;
                }
                let direction = Vec3::new(
                    -yaw.sin() * pitch.cos(),
                    pitch.sin(),
                    -yaw.cos() * pitch.cos(),
                );
                let length_squared = direction.length_squared();
                if !length_squared.is_finite() || length_squared <= 1.0e-12 {
                    return None;
                }
                let descriptor_class = registry
                    .get_component::<DescriptorProvenance>(weapon)
                    .ok()?
                    .canonical_name
                    .clone();
                (!descriptor_class.is_empty()).then_some(RemoteProjectilePresentationLaunch {
                    owner_client_id: remote.owner_client_id,
                    shot_id: remote.shot_id?,
                    origin: transform.position + Vec3::Y * movement.capsule.eye_height,
                    direction: direction / length_squared.sqrt(),
                    range,
                    descriptor_class,
                    projectile,
                })
            })
            .flatten();
        let _ = registry.set_component(weapon, weapon_component);
        match machine.authorization {
            WeaponFireAuthorization::Accepted => weapon_events.push("activate"),
            WeaponFireAuthorization::Empty => {
                weapon_events.push("dry_fire");
                if projectile_fire_intended && let Some(shot_id) = remote.shot_id {
                    rejected_projectile_fires.push(RemoteProjectileFireRejection {
                        owner_client_id: remote.owner_client_id,
                        shot_id,
                    });
                }
                continue;
            }
            WeaponFireAuthorization::Rejected => {
                if projectile_fire_intended && let Some(shot_id) = remote.shot_id {
                    rejected_projectile_fires.push(RemoteProjectileFireRejection {
                        owner_client_id: remote.owner_client_id,
                        shot_id,
                    });
                }
                continue;
            }
        }
        let Some(shot_id) = remote.shot_id else {
            continue;
        };
        let (is_projectile, fire_origin, timeout_budget_ticks) = match resolution {
            ResolutionMode::Hitscan => (false, Vec3::ZERO, crate::netcode::MAX_OPEN_SHOT_AGE_TICKS),
            ResolutionMode::Projectile => {
                let Some(projectile) = projectile.as_ref() else {
                    log::warn!(
                        "[Net] authorized projectile weapon has no projectile descriptor; dropping shot"
                    );
                    rejected_projectile_fires.push(RemoteProjectileFireRejection {
                        owner_client_id: remote.owner_client_id,
                        shot_id,
                    });
                    continue;
                };
                let Some(fire_origin) = remote_fire_origin(&registry, remote.pawn) else {
                    log::warn!("[Net] remote projectile fire has no live pawn eye; dropping shot");
                    rejected_projectile_fires.push(RemoteProjectileFireRejection {
                        owner_client_id: remote.owner_client_id,
                        shot_id,
                    });
                    continue;
                };
                (
                    true,
                    fire_origin,
                    crate::netcode::projectile_timeout_budget_ticks(
                        range,
                        projectile.speed,
                        projectile.lifetime_ms / 1000.0,
                        tick_dt,
                    ),
                )
            }
        };
        authorized.push(OpenAuthorizedShot {
            shot: super::super::AuthorizedShot {
                shot_id,
                pawn: remote.pawn,
                weapon,
                fire_tick: remote.fire_tick,
                damage,
                range,
                pellet_count,
                credit_source,
                is_projectile,
                fire_origin,
                timeout_budget_ticks,
            },
            owner_client_id: remote.owner_client_id,
        });
        if let Some(presentation) = projectile_presentation {
            projectile_presentations.push(presentation);
        }
    }

    RemoteWeaponCommandResult {
        authorized_shots: authorized,
        projectile_presentation_launches: projectile_presentations,
        rejected_projectile_fires,
        reload_deliveries,
        weapon_events,
    }
}

fn remote_fire_origin(registry: &EntityRegistry, pawn: EntityId) -> Option<Vec3> {
    let transform = registry.get_component::<Transform>(pawn).ok()?;
    let movement = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .ok()?;
    Some(transform.position + Vec3::Y * movement.capsule.eye_height)
}

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
    )
}

/// Local authoritative command with the descriptor context required to resolve
/// steady placement for a projectile muzzle. The legacy wrapper above keeps
/// headless test fixtures and the fire-suppressed client equip pass on their
/// explicit no-content path.
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
    let placement =
        crate::resolve_weapon_placement(default_weapon_placement, None, authored_placement, None);
    let pellet_salt_name = weapon::pellet_salt_name(&registry, weapon_id, &weapon_component);
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
        && super::super::reload::timer_expires_this_tick(&weapon_component, tick_dt);
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
    let mut machine = tick_weapon_machine(
        &mut registry,
        pawn,
        weapon_id,
        &mut weapon_component,
        reload_pressed,
        command,
        begin_lower,
        tick_dt,
    );
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
    let mut events = weapon::tick_resolved_component(
        &registry,
        &mut weapon_component,
        &pellet_salt_name,
        active_slot,
        command,
        &placement,
        collision_world,
        hit_zone_store,
        anim_time,
        machine.authorization,
    );
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
    } else if begin_lower {
        if let (Some(pawn), Some(inventory)) = (pawn, inventory) {
            let _ = registry.set_component(pawn, inventory);
        }
    }
    let _ = registry.set_component(weapon_id, weapon_component);
    let mut projectile_spawns = Vec::new();
    if let Some(pawn) = pawn {
        for launch in std::mem::take(&mut events.projectile_launches) {
            if let Some(projectile_id) =
                spawn_projectile(&mut registry, pawn, weapon_id, launch, None)
            {
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
            // Match the host's per-record liveness check. A policy run for an
            // earlier pellet may have despawned either endpoint, in which case
            // the cast still gets its FX but no damage or later policy fire.
            if !pawn.is_some_and(|pawn| registry.exists(pawn)) {
                continue;
            }
            if !registry.exists(target)
                || registry.get_component::<HealthComponent>(target).is_err()
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
        weapon_events: events.event_names(),
        repointed_pawn,
        projectile_spawns,
        #[cfg(test)]
        weapon_impact_points,
    }
}

pub(crate) fn spawn_projectile(
    registry: &mut EntityRegistry,
    owner_pawn: EntityId,
    owner_weapon: EntityId,
    launch: weapon::ProjectileLaunch,
    predicted_shot_id: Option<u64>,
) -> Option<EntityId> {
    // Resolve every hit-time visual before moving the body out of the descriptor
    // below. Impact resolution must not consult the owner weapon, which can be
    // gone before a long-lived projectile makes contact.
    let impact_light = launch.descriptor.visual.impact_light.clone();
    let Some(projectile_id) = registry.try_spawn(
        Transform {
            position: launch.origin,
            rotation: projectile_model_body_rotation(
                &launch.descriptor.visual.body,
                launch.direction,
            ),
            ..Transform::default()
        },
        &[],
    ) else {
        log::warn!("[Weapon] entity registry exhausted; dropping projectile launch");
        return None;
    };

    let component = ProjectileComponent {
        direction: launch.direction.to_array(),
        speed: launch.speed,
        radius: launch.radius,
        remaining_range: launch.range,
        remaining_lifetime: launch.lifetime,
        damage: launch.damage,
        credit_source: launch.credit_source,
        owner_pawn,
        owner_weapon,
        spawned: true,
        predicted_shot_id,
        elapsed_flight_age: 0.0,
        flipbook_active: matches!(
            &launch.descriptor.visual.body,
            ProjectileBodyVisual::Sprite {
                frame_duration_ms: Some(_),
                ..
            }
        ),
        impact_light,
    };
    let _ = registry.set_component(projectile_id, component);

    if let Some(light) = launch.descriptor.visual.light.clone() {
        let _ = registry.set_component(
            projectile_id,
            LightComponent {
                origin: launch.origin.to_array(),
                light_type: LightKind::Point,
                intensity: light.intensity,
                color: light.color,
                falloff_model: light.falloff_model,
                falloff_range: light.falloff_range,
                cone_angle_inner: None,
                cone_angle_outer: None,
                cone_direction: None,
                is_dynamic: true,
                animated_slot: None,
                follow_transform: true,
                carrier: None,
                animation: None,
            },
        );
    }

    match launch.descriptor.visual.body {
        ProjectileBodyVisual::Sprite {
            sprite,
            size,
            opacity,
            rotation,
            tint,
            ..
        } => {
            let _ = registry.set_component(
                projectile_id,
                SpriteVisual {
                    sprite,
                    size,
                    opacity,
                    rotation,
                    tint,
                },
            );
        }
        ProjectileBodyVisual::Model { model } => {
            let _ = registry.set_component(projectile_id, MeshComponent::stateless(model));
        }
    }
    if let Some(trail) = launch.descriptor.visual.trail {
        let _ = registry.set_component(
            projectile_id,
            BillboardEmitterComponent {
                rate: trail.rate,
                burst: trail.burst,
                spread: trail.spread,
                lifetime: trail.lifetime,
                velocity: trail.velocity,
                buoyancy: trail.buoyancy,
                drag: trail.drag,
                size_over_lifetime: LifetimeCurve::from(trail.size_over_lifetime),
                opacity_over_lifetime: LifetimeCurve::from(trail.opacity_over_lifetime),
                color: trail.color,
                sprite: trail.sprite,
                spin_rate: trail.spin_rate,
                spin_animation: trail.spin_animation.map(|animation| {
                    postretro_entities::components::billboard_emitter::SpinAnimation {
                        duration: animation.duration,
                        rate_curve: animation.rate_curve,
                    }
                }),
            },
        );
    }

    Some(projectile_id)
}

/// The renderer applies entity rotation directly to rigid glTF geometry. Projectile
/// models therefore use the same authored mesh-forward convention as other meshes:
/// local `+Z` faces their travel direction. Sprite bodies remain camera-facing and
/// intentionally retain the identity transform rotation.
pub(crate) fn projectile_model_body_rotation(body: &ProjectileBodyVisual, direction: Vec3) -> Quat {
    if !matches!(body, ProjectileBodyVisual::Model { .. }) || !direction.is_finite() {
        return Quat::IDENTITY;
    }

    let length_squared = direction.length_squared();
    if !length_squared.is_finite() || length_squared <= 1.0e-12 {
        return Quat::IDENTITY;
    }

    Quat::from_rotation_arc(Vec3::Z, direction / length_squared.sqrt())
}

pub(crate) fn normalize_inventory_liveness(
    registry: &mut EntityRegistry,
    pawn: EntityId,
) -> Option<(Inventory, bool)> {
    let mut inventory = registry.get_component::<Inventory>(pawn).ok()?.clone();
    let original_active = inventory.active_wieldable();
    let mut changed = false;
    for wieldable in &mut inventory.wieldables {
        if wieldable.is_some_and(|id| {
            !registry.exists(id)
                || registry.has_component_kind(id, postretro_entities::ComponentKind::Weapon)
                    != Ok(true)
        }) {
            *wieldable = None;
            changed = true;
        }
    }

    let active_is_live = inventory
        .wieldables
        .get(inventory.active_slot)
        .copied()
        .flatten()
        .is_some();
    let target_is_live = inventory
        .switch_target
        .is_none_or(|slot| inventory.wieldables.get(slot).copied().flatten().is_some());
    if !active_is_live || !target_is_live {
        inventory.active_slot = inventory
            .wieldables
            .iter()
            .position(Option::is_some)
            .unwrap_or_default();
        inventory.switch_target = None;
        inventory.switch_origin = None;
        for weapon in inventory.wieldables.iter().flatten().copied() {
            let Ok(mut component) = registry.get_component::<WeaponComponent>(weapon).cloned()
            else {
                continue;
            };
            if matches!(
                component.state,
                WieldableState::Lowering | WieldableState::Raising
            ) {
                finish_lowering(&mut component);
                let _ = registry.set_component(weapon, component);
            }
        }
        changed = true;
    }

    if changed {
        let _ = registry.set_component(pawn, inventory.clone());
    }
    let active_changed = inventory.active_wieldable() != original_active;
    Some((inventory, active_changed))
}

/// Normalize every live pawn inventory independently of command arrival. Host-owned
/// remote pawns can go several ticks without a command, but a despawned sibling must
/// still abandon its equip transition and update presentation in that interval.
pub(in crate::sim) fn normalize_all_inventory_liveness(
    registry: &mut EntityRegistry,
) -> Vec<EntityId> {
    let pawns = registry
        .iter_with_kind(postretro_entities::ComponentKind::Inventory)
        .map(|(pawn, _)| pawn)
        .collect::<Vec<_>>();
    pawns
        .into_iter()
        .filter(|pawn| {
            normalize_inventory_liveness(registry, *pawn)
                .is_some_and(|(_, active_changed)| active_changed)
        })
        .collect()
}

/// Apply a host refusal to the locally-running switch machine. A refusal can
/// arrive after the local lower already repointed, so the inventory retains the
/// original slot until this path settles it. Equip state is presentation-only on
/// the client and must not survive the correction as a second visible transition.
pub(crate) fn refuse_local_switch(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    refused_slot: usize,
    rollback_slot: usize,
) -> bool {
    let Ok(mut inventory) = registry.get_component::<Inventory>(pawn).cloned() else {
        return false;
    };

    let refused_in_flight = inventory.switch_target == Some(refused_slot);
    let refused_after_repoint = inventory.active_slot == refused_slot;
    if !refused_in_flight && !refused_after_repoint {
        return false;
    }
    if inventory
        .wieldables
        .get(rollback_slot)
        .copied()
        .flatten()
        .is_none()
    {
        return false;
    }
    inventory.active_slot = rollback_slot;
    inventory.switch_target = None;
    inventory.switch_origin = None;

    for weapon in inventory.wieldables.iter().flatten().copied() {
        let Ok(mut component) = registry.get_component::<WeaponComponent>(weapon).cloned() else {
            continue;
        };
        if matches!(
            component.state,
            WieldableState::Lowering | WieldableState::Raising
        ) {
            component.state = WieldableState::Idle;
            component.state_total_ms = 0;
            component.state_remaining_ms = 0;
            component.state_elapsed_sub_ms = 0.0;
            let _ = registry.set_component(weapon, component);
        }
    }

    let _ = registry.set_component(pawn, inventory);
    true
}

#[cfg(test)]
mod projectile_spawn_tests {
    use super::*;
    use postretro_foundation::{
        ProjectileBodyVisual, ProjectileDescriptor, ProjectileImpactLight, ProjectileLight,
        ProjectileTrailSpinAnimation, ProjectileTrailVisual, ProjectileVisual,
    };

    fn launch(visual: ProjectileVisual) -> weapon::ProjectileLaunch {
        weapon::ProjectileLaunch {
            origin: Vec3::new(1.0, 2.0, 3.0),
            direction: Vec3::NEG_Z,
            speed: 40.0,
            radius: 0.2,
            range: 64.0,
            lifetime: 2.0,
            damage: 25.0,
            credit_source: "plasma.primary".to_string(),
            descriptor: ProjectileDescriptor {
                speed: 40.0,
                radius: 0.2,
                lifetime_ms: 2000.0,
                visual,
            },
        }
    }

    #[test]
    fn projectile_spawn_attaches_sprite_body_and_optional_trail() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let visual = ProjectileVisual {
            body: ProjectileBodyVisual::Sprite {
                sprite: "sprites/plasma.png".to_string(),
                size: 0.4,
                opacity: 0.9,
                rotation: 0.25,
                tint: [0.2, 0.8, 1.0],
                emissive: 0.0,
                frame_duration_ms: None,
            },
            trail: Some(ProjectileTrailVisual {
                sprite: "sprites/trail.png".to_string(),
                rate: 60.0,
                lifetime: 0.5,
                burst: None,
                spread: 0.1,
                velocity: [0.0, 0.0, 0.0],
                buoyancy: 0.0,
                drag: 0.0,
                size_over_lifetime: vec![0.2, 0.0],
                opacity_over_lifetime: vec![1.0, 0.0],
                color: [1.0, 1.0, 1.0],
                spin_rate: 0.0,
                spin_animation: Some(ProjectileTrailSpinAnimation {
                    duration: 0.75,
                    rate_curve: vec![0.0, 2.0, -1.0],
                }),
            }),
            light: None,
            impact_light: None,
        };

        let projectile = spawn_projectile(&mut registry, pawn, weapon, launch(visual), None)
            .expect("projectile spawns");
        assert!(
            registry
                .get_component::<ProjectileComponent>(projectile)
                .is_ok()
        );
        assert_eq!(
            registry
                .get_component::<SpriteVisual>(projectile)
                .expect("sprite body attaches")
                .sprite,
            "sprites/plasma.png"
        );
        let trail = registry
            .get_component::<BillboardEmitterComponent>(projectile)
            .expect("trail emitter attaches");
        assert_eq!(trail.sprite, "sprites/trail.png");
        let animation = trail
            .spin_animation
            .as_ref()
            .expect("trail spin animation materializes locally");
        assert!((animation.duration - 0.75).abs() <= f32::EPSILON);
        assert_eq!(animation.rate_curve, [0.0, 2.0, -1.0]);
    }

    #[test]
    fn projectile_spawn_attaches_rigid_mesh_body() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let visual = ProjectileVisual {
            body: ProjectileBodyVisual::Model {
                model: "models/rocket.gltf".to_string(),
            },
            trail: None,
            light: None,
            impact_light: None,
        };
        let mut launch = launch(visual);
        launch.direction = Vec3::new(3.0, 2.0, -4.0).normalize();

        let projectile = spawn_projectile(&mut registry, pawn, weapon, launch.clone(), None)
            .expect("projectile spawns");
        let mesh = registry
            .get_component::<MeshComponent>(projectile)
            .expect("rigid mesh body attaches");
        assert_eq!(mesh.model, "models/rocket.gltf");
        assert!(mesh.animation.is_none(), "projectile mesh is rigid");
        let transform = registry
            .get_component::<Transform>(projectile)
            .expect("projectile carries its launch transform");
        let rendered_forward = transform.rotation * Vec3::Z;
        assert!(
            rendered_forward.distance(launch.direction) <= 1.0e-6,
            "a rigid projectile model faces the aim direction captured at fire time"
        );
    }

    #[test]
    fn projectile_spawn_attaches_a_following_dynamic_point_light() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let visual = ProjectileVisual {
            body: ProjectileBodyVisual::Sprite {
                sprite: "sprites/plasma.png".to_string(),
                size: 0.4,
                opacity: 0.9,
                rotation: 0.25,
                tint: [0.2, 0.8, 1.0],
                emissive: 0.0,
                frame_duration_ms: None,
            },
            trail: None,
            light: Some(ProjectileLight {
                color: [0.2, 0.7, 1.0],
                intensity: 2.5,
                falloff_range: 6.0,
                falloff_model: postretro_foundation::FalloffKind::InverseDistance,
            }),
            impact_light: None,
        };

        let projectile = spawn_projectile(&mut registry, pawn, weapon, launch(visual), None)
            .expect("projectile spawns");
        let light = registry
            .get_component::<LightComponent>(projectile)
            .expect("descriptor light materializes with its projectile");
        assert_eq!(light.light_type, LightKind::Point);
        assert!(light.is_dynamic);
        assert!(light.follow_transform);
        assert!(Vec3::from_array(light.origin).distance(Vec3::new(1.0, 2.0, 3.0)) <= 1.0e-6);
        assert!(Vec3::from_array(light.color).distance(Vec3::new(0.2, 0.7, 1.0)) <= 1.0e-6);
        assert!((light.intensity - 2.5).abs() <= f32::EPSILON);
        assert!((light.falloff_range - 6.0).abs() <= f32::EPSILON);
        assert_eq!(
            light.falloff_model,
            postretro_foundation::FalloffKind::InverseDistance
        );
    }

    #[test]
    fn projectile_spawn_retains_resolved_impact_light_before_moving_body_visual() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let impact_light = ProjectileImpactLight {
            color: [0.5, 0.8, 1.0],
            intensity: 4.0,
            radius: 5.0,
            peak_radius: Some(10.0),
            fade_ms: 200.0,
        };
        let visual = ProjectileVisual {
            body: ProjectileBodyVisual::Model {
                model: "models/rocket.gltf".to_string(),
            },
            trail: None,
            light: None,
            impact_light: Some(impact_light.clone()),
        };

        let projectile = spawn_projectile(&mut registry, pawn, weapon, launch(visual), None)
            .expect("projectile spawns");
        assert_eq!(
            registry
                .get_component::<ProjectileComponent>(projectile)
                .expect("projectile state survives body materialization")
                .impact_light,
            Some(impact_light),
            "the later contact path never has to resolve the owner weapon"
        );
    }

    #[test]
    fn projectile_model_rotation_handles_vertical_and_invalid_directions() {
        let body = ProjectileBodyVisual::Model {
            model: "models/rocket.gltf".to_string(),
        };
        let vertical = projectile_model_body_rotation(&body, Vec3::Y);
        assert!(vertical.is_finite());
        assert!(
            (vertical * Vec3::Z).distance(Vec3::Y) <= 1.0e-6,
            "vertical aim remains a valid model orientation"
        );

        for invalid in [Vec3::ZERO, Vec3::new(f32::NAN, 0.0, 1.0)] {
            assert_eq!(
                projectile_model_body_rotation(&body, invalid),
                Quat::IDENTITY,
                "invalid launch direction preserves the safe identity fallback"
            );
        }
    }
}
