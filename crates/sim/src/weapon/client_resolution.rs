// Weapon activation consumer and resolution seam.
// See: context/lib/entity_model.md §4, §5 · context/lib/networking.md
use super::*;

#[allow(clippy::too_many_arguments)] // mirrors the host/single-player hitscan inputs.
pub fn resolve_client_fire(
    owner_pawn: Option<EntityId>,
    weapon: &mut WeaponComponent,
    pellet_salt_name: &str,
    active_slot: usize,
    button: FireButtonState,
    aim_origin: Vec3,
    aim_direction: Vec3,
    placement: &WeaponPlacementDescriptor,
    muzzle_offset: Option<Vec3>,
    client_tick: u32,
    selected_shot_elapsed_ms: &[f32],
    logical_tick_elapsed_ms: &[f32],
    collision_world: &CollisionWorld,
    registry: &EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    frame_dt: f32,
) -> Option<ClientFireResolution> {
    let frame_dt_ms = (frame_dt.max(0.0)) * 1000.0;
    if !advance_client_fire_gate(weapon, button, frame_dt_ms) {
        tick_client_bloom_for_frame(weapon, frame_dt_ms, logical_tick_elapsed_ms);
        return None;
    }

    // As on the local path, consume one deterministic shell position only after
    // this frame has an authorized cast. A send failure deliberately does not
    // roll this back: the next shell must use the next fan.
    let shell_counter = weapon.shells_fired;
    weapon.shells_fired = weapon.shells_fired.wrapping_add(1);
    let (
        cooldown_ms,
        pellet_count,
        range,
        resolution,
        projectile,
        splash,
        damage,
        knockback,
        credit_source,
    ) = {
        let stats = weapon.effective();
        (
            stats.primary.recovery_ms,
            stats.pellet_count,
            stats.range,
            stats.resolution,
            stats.projectile.cloned(),
            stats.splash.cloned(),
            stats.damage,
            stats.knockback,
            stats.credit_source.to_string(),
        )
    };
    let mut replayed_bloom_until_ms = 0.0;
    let (spread_radians, hitscan_direction) = if resolution == ResolutionMode::Hitscan {
        // The post-loop path casts one rendered-pose ray, but the host has run
        // every selected logical tick. Replay bloom through the first selected
        // tick before sampling its cone; the remaining selected ticks advance
        // below without consuming a client ray, shell position, or RNG fan.
        replayed_bloom_until_ms = selected_shot_elapsed_ms
            .first()
            .copied()
            .unwrap_or(frame_dt_ms)
            .clamp(0.0, frame_dt_ms);
        let mut previous_logical_tick_ms = 0.0;
        for &elapsed_ms in logical_tick_elapsed_ms {
            let elapsed_ms = elapsed_ms.clamp(previous_logical_tick_ms, frame_dt_ms);
            if elapsed_ms > replayed_bloom_until_ms {
                break;
            }
            weapon.tick_bloom(elapsed_ms - previous_logical_tick_ms);
            previous_logical_tick_ms = elapsed_ms;
        }
        weapon.tick_bloom(replayed_bloom_until_ms - previous_logical_tick_ms);
        composed_hitscan_cone(registry, owner_pawn, weapon, aim_direction)
    } else {
        weapon.tick_bloom(frame_dt_ms);
        (weapon.spread_degrees.to_radians(), aim_direction)
    };
    weapon.cooldown_remaining_ms = cooldown_ms;
    let ((hits, world_contacts), projectile_launch) = match resolution {
        ResolutionMode::Hitscan => (
            resolve_client_hitscan(
                owner_pawn,
                aim_origin,
                hitscan_direction,
                collision_world,
                registry,
                hit_zone_store,
                anim_time,
                pellet_count,
                spread_radians,
                range,
                resolution,
                shell_counter,
                pellet_salt_name,
                active_slot,
            ),
            None,
        ),
        ResolutionMode::Projectile => {
            let projectile = projectile?;
            let (origin, direction) = resolve_projectile_launch_pose(
                owner_pawn,
                aim_origin,
                aim_direction,
                placement,
                muzzle_offset,
                projectile.radius,
                collision_world,
                registry,
                hit_zone_store,
                anim_time,
                range,
            );
            (
                (Vec::new(), Vec::new()),
                Some(ProjectileLaunch {
                    action: None,
                    shot_id: None,
                    model_scale: 1.0,
                    knockback_impulse: knockback.map_or(Vec3::ZERO, |push| {
                        postretro_foundation::knockback_impulse(
                            push.speed,
                            push.upward_bias,
                            direction,
                        )
                    }),
                    origin,
                    direction,
                    speed: projectile.speed,
                    radius: projectile.radius,
                    range,
                    lifetime: projectile.lifetime_ms / 1000.0,
                    damage,
                    credit_source,
                    descriptor: projectile,
                    splash,
                }),
            )
        }
    };
    if resolution == ResolutionMode::Hitscan {
        // Each trailing selected shot runs after the intervening logical-tick
        // bloom decay. It has an empty declaration, so no client ray, shell
        // position, or RNG fan is consumed for it.
        weapon.apply_bloom_shot();
        let first_selected_shot_ms = replayed_bloom_until_ms;
        let mut logical_ticks = logical_tick_elapsed_ms
            .iter()
            .copied()
            .map(|elapsed_ms| elapsed_ms.clamp(first_selected_shot_ms, frame_dt_ms))
            .peekable();
        while logical_ticks
            .peek()
            .is_some_and(|elapsed_ms| *elapsed_ms <= replayed_bloom_until_ms)
        {
            let _ = logical_ticks.next();
        }
        let mut trailing_selected_shots =
            selected_shot_elapsed_ms.iter().copied().skip(1).peekable();
        for elapsed_ms in logical_ticks {
            weapon.tick_bloom(elapsed_ms - replayed_bloom_until_ms);
            replayed_bloom_until_ms = elapsed_ms;
            while trailing_selected_shots
                .peek()
                .is_some_and(|selected_ms| *selected_ms <= elapsed_ms)
            {
                weapon.apply_bloom_shot();
                let _ = trailing_selected_shots.next();
            }
        }
        for elapsed_ms in trailing_selected_shots {
            let elapsed_ms = elapsed_ms.clamp(replayed_bloom_until_ms, frame_dt_ms);
            weapon.tick_bloom(elapsed_ms - replayed_bloom_until_ms);
            weapon.apply_bloom_shot();
            replayed_bloom_until_ms = elapsed_ms;
        }
        // Preserve decay after the final selected fire tick until the rendered
        // frame ends, including a partial fixed-tick remainder.
        weapon.tick_bloom(frame_dt_ms - replayed_bloom_until_ms);
    }
    Some(ClientFireResolution {
        client_tick,
        hits,
        world_contacts,
        projectile_launch,
    })
}

pub fn advance_client_fire_state(
    weapon: &mut WeaponComponent,
    button: FireButtonState,
    frame_dt: f32,
    logical_tick_elapsed_ms: &[f32],
) -> bool {
    let dt_ms = (frame_dt.max(0.0)) * 1000.0;
    tick_client_bloom_for_frame(weapon, dt_ms, logical_tick_elapsed_ms);
    advance_client_fire_gate(weapon, button, dt_ms)
}

fn tick_client_bloom_for_frame(
    weapon: &mut WeaponComponent,
    frame_dt_ms: f32,
    logical_tick_elapsed_ms: &[f32],
) {
    let mut previous_elapsed_ms = 0.0;
    for &elapsed_ms in logical_tick_elapsed_ms {
        let elapsed_ms = elapsed_ms.clamp(previous_elapsed_ms, frame_dt_ms);
        weapon.tick_bloom(elapsed_ms - previous_elapsed_ms);
        previous_elapsed_ms = elapsed_ms;
    }
    weapon.tick_bloom(frame_dt_ms - previous_elapsed_ms);
}

fn advance_client_fire_gate(
    weapon: &mut WeaponComponent,
    button: FireButtonState,
    dt_ms: f32,
) -> bool {
    weapon.cooldown_remaining_ms = (weapon.cooldown_remaining_ms - dt_ms).max(0.0);

    let trigger = weapon.primary.trigger;
    let wants_fire = match trigger {
        postretro_foundation::ActivationTrigger::Press => {
            button.pressed && !weapon.shoot_press_consumed
        }
        postretro_foundation::ActivationTrigger::Hold => button.active,
    };
    if trigger == postretro_foundation::ActivationTrigger::Press && button.pressed {
        weapon.shoot_press_consumed = true;
    } else if !button.active {
        weapon.shoot_press_consumed = false;
    }

    if !weapon.state.allows_fire() || !wants_fire || weapon.cooldown_remaining_ms > 0.0 {
        return false;
    }
    true
}

#[allow(clippy::too_many_arguments)] // mirrors the local fire query inputs without a throwaway struct.
fn resolve_client_hitscan(
    owner_pawn: Option<EntityId>,
    origin: Vec3,
    direction: Vec3,
    collision_world: &CollisionWorld,
    registry: &EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    pellet_count: u32,
    spread_radians: f32,
    range: f32,
    resolution: ResolutionMode,
    shell_counter: u32,
    pellet_salt_name: &str,
    active_slot: usize,
) -> (Vec<LocalHitRecord>, Vec<WorldContact>) {
    match resolution {
        ResolutionMode::Hitscan => {
            let mut hits = Vec::with_capacity(pellet_count as usize);
            let mut world_contacts = Vec::new();
            let mut pellet_rng = spread::PelletRng::new(spread::pellet_rng_seed(
                shell_counter,
                pellet_salt_name,
                active_slot,
            ));
            for _ in 0..pellet_count {
                let pellet_direction = spread::sample_cone_direction(
                    direction,
                    spread_radians,
                    pellet_rng.next_f32(),
                    pellet_rng.next_f32(),
                );
                // An entity hit is a damage claim; a nearer world hit is a
                // presentation-only contact the client still plays and declares.
                match resolve_nearest_hit(NearestHitQuery {
                    owner_pawn,
                    origin,
                    direction: pellet_direction,
                    collision_world,
                    registry,
                    hit_zone_store,
                    anim_time,
                    range,
                }) {
                    Some(NearestHit::Entity(entity)) => hits.push(local_hit_record(entity)),
                    Some(NearestHit::World(world)) => world_contacts.push(WorldContact {
                        point: world.point,
                        normal: world.normal,
                    }),
                    None => {}
                }
            }
            (hits, world_contacts)
        }
        // Projectile flight is materialized by the connected client's mutable
        // post-loop path. This ray-resolution helper emits no same-frame hit;
        // the projectile declares its later collision or expiry instead.
        ResolutionMode::Projectile => (Vec::new(), Vec::new()),
    }
}
