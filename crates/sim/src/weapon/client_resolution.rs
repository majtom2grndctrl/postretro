// Frozen client shot resolution at the rendered pose.
// See: context/lib/entity_model.md §5, §7 · context/lib/networking.md
use super::*;

#[allow(clippy::too_many_arguments)]
pub fn resolve_client_shot(
    owner_pawn: Option<EntityId>,
    weapon: WeaponComponent,
    pellet_salt_name: &str,
    active_slot: usize,
    aim_origin: Vec3,
    aim_direction: Vec3,
    placement: &WeaponPlacementDescriptor,
    collision_world: &CollisionWorld,
    registry: &EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    shot: ResolvedWeaponShot,
) -> ClientFireResolution {
    let client_tick = shot.activation.shot_id.start_tick;
    let events = resolve_activation_shot_owned(
        registry,
        owner_pawn,
        weapon,
        pellet_salt_name,
        active_slot,
        &WeaponFireCommand {
            button: FireButtonState {
                pressed: false,
                active: true,
            },
            aim_origin,
            aim_direction,
            can_fire: true,
        },
        placement,
        collision_world,
        hit_zone_store,
        anim_time,
        shot,
    );
    let mut hits = Vec::new();
    let mut world_contacts = Vec::new();
    for impact in events.impacts {
        if let Some(target) = impact.target {
            hits.push(LocalHitRecord {
                target,
                point: impact.point,
                normal: impact.normal,
                zone: impact.zone,
            });
        } else {
            world_contacts.push(WorldContact {
                point: impact.point,
                normal: impact.normal,
            });
        }
    }
    ClientFireResolution {
        client_tick,
        hits,
        world_contacts,
        projectile_launch: events.projectile_launches.into_iter().next(),
    }
}

// Spatial fixtures use the actual fixed-tick advance/freeze/resolve chain.
// Production callers must supply every due snapshot explicitly.
#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::too_many_arguments)]
pub fn resolve_test_client_shot(
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
    _selected_shot_elapsed_ms: &[f32],
    _logical_tick_elapsed_ms: &[f32],
    collision_world: &CollisionWorld,
    registry: &EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    frame_dt: f32,
) -> Option<ClientFireResolution> {
    weapon.muzzle_offset = muzzle_offset;
    let result = activation_prediction::advance_predicted_weapon_tick(
        weapon,
        execution::ActivationCommand {
            tick: client_tick,
            pawn: 0,
            real_command: true,
            input: postretro_foundation::ActivationInput::default(),
            controller_starts: true,
            primary: button,
            secondary: FireButtonState {
                pressed: false,
                active: false,
            },
        },
        false,
        frame_dt.max(0.0) * 1000.0,
        true,
    );
    let shot = freeze_weapon_shot(weapon, result.shot?);
    Some(resolve_client_shot(
        owner_pawn,
        weapon.clone(),
        pellet_salt_name,
        active_slot,
        aim_origin,
        aim_direction,
        placement,
        collision_world,
        registry,
        hit_zone_store,
        anim_time,
        shot,
    ))
}
