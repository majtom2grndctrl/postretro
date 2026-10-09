// Weapon activation consumer and resolution seam.
// See: context/lib/entity_model.md §4, §5 · context/lib/networking.md
use super::*;

#[allow(clippy::too_many_arguments)]
pub(in crate::sim) fn run_remote_weapon_commands(
    registry: &Rc<RefCell<EntityRegistry>>,
    remote_pawn_commands: &[RemotePawnCommand],
    descriptors: &[EntityTypeDescriptor],
    default_weapon_placement: Option<&WeaponPlacementDescriptor>,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    tick_dt: f32,
) -> RemoteWeaponCommandResult {
    let mut registry = registry.borrow_mut();
    let mut authorized = Vec::new();
    let mut activation_progress = Vec::new();
    let mut projectile_presentations = Vec::new();
    let mut rejected_projectile_fires = Vec::new();
    let mut reload_deliveries = Vec::new();
    let mut reload_emissions = Vec::new();
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
        let pose_available = (weapon_component.resolution != ResolutionMode::Projectile
            && weapon_component.knockback.is_none())
            || remote_projectile_aim(&registry, remote).is_some();
        let descriptor_available = weapon_component.resolution != ResolutionMode::Projectile
            || weapon_component.projectile.is_some();
        let pawn_alive = !crate::scripting_systems::health::is_terminally_committed_to_removal(
            &registry,
            remote.pawn,
        ) && !registry
            .get_component::<postretro_entities::components::health::HealthComponent>(remote.pawn)
            .is_ok_and(|health| health.current <= 0.0 || !health.current.is_finite());
        if !pawn_alive {
            weapon_component.cancel_activation();
        }
        let command = WeaponFireCommand {
            button: remote.command.fire_button,
            aim_origin: Vec3::ZERO,
            aim_direction: Vec3::Z,
            can_fire: remote.shot_id.is_some()
                && pose_available
                && descriptor_available
                && pawn_alive,
        };
        let activation = weapon::execution::ActivationCommand {
            tick: remote.fire_tick,
            pawn: remote.shot_id.map_or(0, |id| id.pawn),
            real_command: remote.real_command,
            input: remote.command.activation,
            controller_starts: false,
            primary: remote.command.fire_button,
            secondary: remote.command.secondary_button,
        };
        let mut machine = tick_weapon_machine_activation(
            &mut registry,
            Some(remote.pawn),
            weapon,
            &mut weapon_component,
            remote.command.reload,
            &command,
            false,
            tick_dt,
            activation,
            false,
        );
        if let Some(rejected) = remote.rejected_activation {
            machine.activation.rejected = Some(rejected);
        }
        let frozen = machine
            .activation
            .shot
            .clone()
            .map(|attempt| weapon::freeze_weapon_shot(&weapon_component, attempt));
        let shot_id = machine.activation.attempted;
        let projectile_fire_intended = machine.activation.attempted.is_some()
            && weapon_component.resolution == ResolutionMode::Projectile;
        activation_progress.push(super::super::super::RemoteActivationProgress {
            pawn: remote.pawn,
            owner_client_id: remote.owner_client_id,
            weapon,
            tick: remote.fire_tick,
            recovery_ms: weapon_component.cooldown_remaining_ms,
            advance: machine.activation.clone(),
        });
        reload_emissions.extend(
            machine
                .deliveries
                .iter()
                .map(|delivery| reload_emission(&registry, delivery)),
        );
        reload_deliveries.extend(machine.deliveries);
        let damage = frozen.as_ref().map_or(weapon_component.damage, |shot| {
            shot.activation.values.damage
        });
        let knockback = frozen
            .as_ref()
            .map_or(weapon_component.knockback, |shot| shot.knockback);
        let range = frozen
            .as_ref()
            .map_or(weapon_component.range, |shot| shot.activation.values.range);
        let pellet_count = frozen
            .as_ref()
            .map_or(weapon_component.pellet_count, |shot| shot.pellet_count)
            as usize;
        let credit_source = frozen.as_ref().map_or_else(
            || weapon_component.credit_source.clone(),
            |shot| shot.credit_source.clone(),
        );
        let resolution = frozen
            .as_ref()
            .map_or(weapon_component.resolution, |shot| shot.resolution);
        let projectile = frozen.as_ref().and_then(|shot| shot.projectile.clone());
        let splash = frozen.as_ref().and_then(|shot| shot.splash.clone());
        // Projectile fire origins deliberately read the live host component,
        // which is the host-spawned source for authored muzzle content.
        let muzzle_offset = weapon_component.muzzle_offset;
        let _ = registry.set_component(weapon, weapon_component);
        // A remote pawn's shot sounds from that pawn, with its weapon's sounds.
        let remote_emission = |address| WeaponEmission {
            sounds: frozen.as_ref().map(|shot| shot.sounds.clone()),
            action: if address == "activate" {
                frozen.as_ref().map(|shot| shot.action().clone())
            } else {
                None
            },
            shot_id,
            address,
            emitter: entity_emitter(&registry, remote.pawn),
            weapon: descriptor_name(&registry, weapon),
        };
        match machine.authorization {
            WeaponFireAuthorization::Accepted => {
                weapon_events.push(remote_emission("activate"));
                if machine.overheat {
                    weapon_events.push(remote_emission("overheat"));
                }
            }
            WeaponFireAuthorization::Empty => {
                weapon_events.push(remote_emission("dry_fire"));
                if projectile_fire_intended && let Some(shot_id) = shot_id {
                    rejected_projectile_fires.push(RemoteProjectileFireRejection {
                        owner_client_id: remote.owner_client_id,
                        shot_id,
                    });
                }
                continue;
            }
            WeaponFireAuthorization::Rejected => {
                if projectile_fire_intended && let Some(shot_id) = shot_id {
                    rejected_projectile_fires.push(RemoteProjectileFireRejection {
                        owner_client_id: remote.owner_client_id,
                        shot_id,
                    });
                }
                continue;
            }
        }
        let Some(shot_id) = shot_id else {
            continue;
        };
        let (
            is_projectile,
            fire_origin,
            projectile_direction,
            timeout_budget_ticks,
            projectile_presentation,
        ) = match resolution {
            ResolutionMode::Hitscan => {
                // Freeze direction's origin with the shot. The later declaration
                // still validates LOS/range from the live eye, but strafing after
                // FIRE must not rotate its knockback.
                let fire_origin = match remote_projectile_aim(&registry, remote) {
                    Some((eye, _)) => eye,
                    // Amount-only FIRE can still be authorized before a pawn
                    // has movement; existing HIT validation requires its eye.
                    None if knockback.is_none() => Vec3::ZERO,
                    None => continue,
                };
                (false, fire_origin, None, MAX_OPEN_SHOT_AGE_TICKS, None)
            }
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
                let Some((eye, direction)) = remote_projectile_aim(&registry, remote) else {
                    log::warn!(
                        "[Net] remote projectile fire has no valid live pawn aim; dropping shot"
                    );
                    rejected_projectile_fires.push(RemoteProjectileFireRejection {
                        owner_client_id: remote.owner_client_id,
                        shot_id,
                    });
                    continue;
                };
                let descriptor_class = registry
                    .get_component::<DescriptorProvenance>(weapon)
                    .ok()
                    .map(|provenance| provenance.canonical_name.clone())
                    .unwrap_or_default();
                let authored_placement = descriptors
                    .iter()
                    .find(|descriptor| {
                        descriptor.canonical_name.as_deref() == Some(descriptor_class.as_str())
                    })
                    .and_then(|descriptor| descriptor.weapon.as_ref())
                    .and_then(|weapon| weapon.placement.as_ref());
                let placement = postretro_foundation::resolve_weapon_placement(
                    default_weapon_placement,
                    None,
                    authored_placement,
                    None,
                );
                // Authorization, observer presentation, and host replay freeze
                // the pose reconstructed from the same authored rules as prediction.
                let (fire_origin, projectile_direction) = weapon::resolve_projectile_launch_pose(
                    Some(remote.pawn),
                    eye,
                    direction,
                    &placement,
                    muzzle_offset,
                    projectile.radius,
                    collision_world,
                    &registry,
                    hit_zone_store,
                    anim_time,
                    range,
                );
                let projectile_presentation =
                    (!descriptor_class.is_empty()).then_some(RemoteProjectilePresentationLaunch {
                        action: frozen.as_ref().map(|shot| shot.action().clone()),
                        model_scale: frozen
                            .as_ref()
                            .map_or(1.0, |shot| shot.projectile_model_scale),
                        owner_client_id: remote.owner_client_id,
                        shot_id,
                        origin: fire_origin,
                        direction: projectile_direction,
                        range,
                        descriptor_class,
                        projectile: projectile.clone(),
                    });
                (
                    true,
                    fire_origin,
                    Some(projectile_direction),
                    projectile_timeout_budget_ticks(
                        range,
                        projectile.speed,
                        projectile.lifetime_ms / 1000.0,
                        tick_dt,
                    ),
                    projectile_presentation,
                )
            }
        };
        let projectile_radius = if is_projectile {
            projectile.as_ref().map(|projectile| projectile.radius)
        } else {
            None
        };
        authorized.push(OpenAuthorizedShot {
            shot: AuthorizedShot {
                sounds: frozen.as_ref().map(|shot| shot.sounds.clone()),
                action: frozen.as_ref().map(|shot| shot.action().clone()),
                source_weapon: descriptor_name(&registry, weapon),
                shot_id,
                pawn: remote.pawn,
                weapon,
                fire_tick: remote.fire_tick,
                damage,
                knockback,
                range,
                pellet_count,
                credit_source,
                splash,
                projectile_radius,
                projectile_direction,
                projectile_speed: projectile.as_ref().map(|projectile| projectile.speed),
                projectile_lifetime_seconds: projectile
                    .as_ref()
                    .map(|projectile| projectile.lifetime_ms / 1_000.0),
                projectile_tick_seconds: is_projectile.then_some(tick_dt),
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
        activation_progress,
        authorized_shots: authorized,
        projectile_presentation_launches: projectile_presentations,
        rejected_projectile_fires,
        reload_deliveries,
        reload_emissions,
        weapon_events,
    }
}

fn remote_projectile_aim(
    registry: &EntityRegistry,
    remote: &RemotePawnCommand,
) -> Option<(Vec3, Vec3)> {
    let transform = registry.get_component::<Transform>(remote.pawn).ok()?;
    let movement = registry
        .get_component::<PlayerMovementComponent>(remote.pawn)
        .ok()?;
    // A delivered start fires along the aim its own command declared. An aim
    // failing the command checks reads as the delivering command's aim.
    let (yaw, pitch) = remote
        .start_aim
        .filter(|aim| aim.yaw.is_finite() && aim.pitch.is_finite())
        .map_or(
            (remote.command.movement.facing_yaw, remote.aim_pitch),
            |aim| (aim.yaw, aim.pitch),
        );
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
    Some((
        transform.position + Vec3::Y * movement.capsule.eye_height,
        direction / length_squared.sqrt(),
    ))
}
