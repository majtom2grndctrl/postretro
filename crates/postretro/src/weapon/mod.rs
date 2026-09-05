// Weapon fire tick, hitscan/local hit resolution, and client fire prediction: owns fire commands, local hit records, and predicted-shot reconciliation state.
// See: context/lib/entity_model.md §5, §7

use std::collections::HashMap;

use glam::Vec3;
use parry3d::math::{Point, Vector};
use postretro_entities::components::weapon::{UNKNOWN_WEAPON_CREDIT_SOURCE, WeaponComponent};
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::registry::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_foundation::{
    FireMode, ProjectileDescriptor, ResolutionMode, WeaponPlacementDescriptor,
};

use crate::collision::{CollisionWorld, cast_ray};
#[cfg(test)]
use crate::scripting_systems::hit_zones::nearest_entity_hit;
use crate::scripting_systems::hit_zones::{
    EntityRayHit, HitZoneStore, nearest_entity_hit_ignoring,
};
#[cfg(test)]
use crate::{
    camera::Camera,
    input::{Action, ActionSnapshot, ButtonState},
};

mod damage;
mod impact;
pub(crate) mod spread;

pub(crate) use damage::DamagePayload;
pub(crate) use impact::sprite_collection as impact_sprite_collection;
pub(crate) use impact::{
    lifetime as impact_lifetime, spawn_impact_effect_at, spawn_projectile_impact_light,
};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ActivationOutcome {
    Hit(DamagePayload),
    Effect,
    Spawned(EntityId),
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WeaponActivation {
    pub(crate) origin: Vec3,
    pub(crate) direction: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FireButtonState {
    pub(crate) pressed: bool,
    pub(crate) active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WeaponFireCommand {
    pub(crate) button: FireButtonState,
    pub(crate) aim_origin: Vec3,
    pub(crate) aim_direction: Vec3,
    pub(crate) can_fire: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LocalHitRecord {
    pub(crate) target: EntityId,
    pub(crate) point: Vec3,
    pub(crate) zone: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ClientFireResolution {
    pub(crate) client_tick: u32,
    pub(crate) hits: Vec<LocalHitRecord>,
    /// A projectile launch is deferred to the connected client's post-loop
    /// presentation path. It must not produce a same-frame hit declaration.
    pub(crate) projectile_launch: Option<ProjectileLaunch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PredictedShotStatus {
    Pending,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PredictedShotRecord {
    pub(crate) shot_id: u64,
    pub(crate) client_tick: u32,
    pub(crate) weapon: EntityId,
    pub(crate) cooldown_before_ms: f32,
    pub(crate) cooldown_after_ms: f32,
    pub(crate) cooldown_authority_generation: u64,
    pub(crate) muzzle_fx_visible: bool,
    pub(crate) hitmarker_visible: bool,
    pub(crate) status: PredictedShotStatus,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct ClientPredictedShots {
    shots: HashMap<u64, PredictedShotRecord>,
    cooldown_authority_generation: HashMap<EntityId, u64>,
}

impl ClientPredictedShots {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn clear(&mut self) {
        self.shots.clear();
        self.cooldown_authority_generation.clear();
    }

    pub(crate) fn predict(
        &mut self,
        shot_id: u64,
        weapon: EntityId,
        resolution: &ClientFireResolution,
        cooldown_before_ms: f32,
        cooldown_after_ms: f32,
    ) {
        self.shots.insert(
            shot_id,
            PredictedShotRecord {
                shot_id,
                client_tick: resolution.client_tick,
                weapon,
                cooldown_before_ms,
                cooldown_after_ms,
                cooldown_authority_generation: self
                    .cooldown_authority_generation
                    .get(&weapon)
                    .copied()
                    .unwrap_or_default(),
                muzzle_fx_visible: true,
                hitmarker_visible: !resolution.hits.is_empty(),
                status: PredictedShotStatus::Pending,
            },
        );
    }

    pub(crate) fn reconcile_cooldown(
        &mut self,
        weapon_id: EntityId,
        weapon: &mut WeaponComponent,
        authoritative_cooldown_ms: f32,
    ) {
        if authoritative_cooldown_ms.is_finite() {
            weapon.cooldown_remaining_ms = authoritative_cooldown_ms.max(0.0);
            let generation = self
                .cooldown_authority_generation
                .entry(weapon_id)
                .or_default();
            *generation = generation.wrapping_add(1);
        }
    }

    pub(crate) fn apply_verdict(
        &mut self,
        registry: &mut EntityRegistry,
        shot_id: u64,
        fire_accepted: bool,
        hit_accepted: bool,
    ) -> Option<PredictedShotRecord> {
        // A per-shot verdict is terminal: the record is reconciled exactly once,
        // so a stored record is always `Pending`. Apply the rollback effects,
        // then prune it — the map would otherwise grow unbounded across a session
        // (unlike the age-pruned host mirror). A duplicate or late verdict finds
        // nothing and is a harmless no-op.
        let record = self.shots.get_mut(&shot_id)?;
        if fire_accepted {
            record.status = PredictedShotStatus::Accepted;
            record.hitmarker_visible &= hit_accepted;
        } else {
            if self
                .cooldown_authority_generation
                .get(&record.weapon)
                .copied()
                .unwrap_or_default()
                == record.cooldown_authority_generation
                && let Ok(mut weapon) = registry
                    .get_component::<WeaponComponent>(record.weapon)
                    .cloned()
            {
                weapon.cooldown_remaining_ms = record.cooldown_before_ms.max(0.0);
                let _ = registry.set_component(record.weapon, weapon);
            }
            record.muzzle_fx_visible = false;
            record.hitmarker_visible = false;
            record.status = PredictedShotStatus::Rejected;

            // A rejected FIRE has no host authority to resolve later. Remove only
            // this client's matching predicted flight; remote observer entities
            // carry no ProjectileComponent and other local shots keep their ids.
            let rejected_projectiles = registry
                .iter_with_kind(ComponentKind::Projectile)
                .filter_map(|(id, value)| {
                    let ComponentValue::Projectile(projectile) = value else {
                        return None;
                    };
                    (projectile.predicted_shot_id == Some(shot_id)).then_some(id)
                })
                .collect::<Vec<_>>();
            for projectile in rejected_projectiles {
                let _ = registry.despawn(projectile);
            }
        }
        self.shots.remove(&shot_id)
    }

    /// A predicted projectile only knows whether it hit after its later
    /// frame-driven sweep. The verdict remains the authority that keeps or
    /// clears this local presentation state.
    pub(crate) fn mark_hitmarker(&mut self, shot_id: u64) {
        if let Some(record) = self.shots.get_mut(&shot_id)
            && record.status == PredictedShotStatus::Pending
        {
            record.hitmarker_visible = true;
        }
    }

    #[cfg(test)]
    fn get(&self, shot_id: u64) -> Option<&PredictedShotRecord> {
        self.shots.get(&shot_id)
    }
}

// Not `Copy`: `zone: Option<String>` carries a heap-backed tag for skeletal
// hit-zone hits, so `WeaponImpact` (and `WeaponFireEvents`, which owns a list of
// them) move/borrow rather than copy. Audited call sites: `fire_hitscan`
// constructs the per-pellet literals, and the sim weapon stage consumes the
// list in order while retaining a pre-policy cast-point record for determinism.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WeaponImpact {
    pub(crate) point: Vec3,
    pub(crate) normal: Vec3,
    /// The entity struck, when the nearest hit along the ray is an entity
    /// hitbox rather than world geometry. `None` for a world-only hit or when
    /// no targetable entity lies along the ray within range. Spatial targeting
    /// rides here, beside the payload — never inside [`DamagePayload`]. The sim
    /// weapon stage consumes this to route `apply_damage_with_context` before
    /// the death sweep handles zero-HP entities.
    pub(crate) target: Option<EntityId>,
    /// The authored skeletal hit-zone tag the shot landed on (e.g. "head"),
    /// surfaced for an entity hit that struck a bone-posed capsule. `None` for a
    /// world hit or an authored-AABB entity hit. The zone-multiplier damage
    /// routing site reads this to scale the payload; here it is only surfaced.
    pub(crate) zone: Option<String>,
    pub(crate) outcome: ActivationOutcome,
}

/// Immutable fire-time data the mutable weapon stage materializes as an entity.
/// `fire_hitscan` deliberately returns this rather than spawning while it holds
/// only an immutable registry borrow.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProjectileLaunch {
    pub(crate) origin: Vec3,
    pub(crate) direction: Vec3,
    pub(crate) speed: f32,
    pub(crate) radius: f32,
    pub(crate) range: f32,
    pub(crate) lifetime: f32,
    pub(crate) damage: f32,
    pub(crate) credit_source: String,
    pub(crate) descriptor: ProjectileDescriptor,
}

const MUZZLE_DIRECTION_EPSILON_SQUARED: f32 = 1.0e-12;

/// Compose a model-local muzzle point through steady viewmodel placement and
/// the gameplay aim basis. Render-rate sway and bob intentionally do not enter
/// this authoritative origin.
pub(crate) fn muzzle_world_origin(
    eye: Vec3,
    aim_direction: Vec3,
    placement: &WeaponPlacementDescriptor,
    muzzle_local: Vec3,
) -> Vec3 {
    let (placement_offset, placement_rotation) = placement.camera_space();
    let camera_space = placement_rotation * muzzle_local + placement_offset;

    let forward_length_squared = aim_direction.length_squared();
    let forward = if aim_direction.is_finite()
        && forward_length_squared.is_finite()
        && forward_length_squared > MUZZLE_DIRECTION_EPSILON_SQUARED
    {
        aim_direction
    } else {
        Vec3::NEG_Z
    };
    let right_candidate = forward.cross(Vec3::Y);
    let right = if right_candidate.length_squared() > MUZZLE_DIRECTION_EPSILON_SQUARED {
        right_candidate.normalize()
    } else {
        // A remote wire aim may be exactly vertical, even though the local
        // camera pitch clamp normally keeps it short of this pole.
        forward.cross(Vec3::Z).normalize()
    };
    let up = right.cross(forward);

    eye + right * camera_space.x + up * camera_space.y + forward * -camera_space.z
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct WeaponFireEvents {
    pub(crate) activate: Option<WeaponActivation>,
    pub(crate) impacts: Vec<WeaponImpact>,
    pub(crate) projectile_launches: Vec<ProjectileLaunch>,
    /// Filled only by the mutable caller after it materializes a launch intent.
    pub(crate) spawned: Vec<ActivationOutcome>,
    pub(crate) dry_fire: bool,
}

impl WeaponFireEvents {
    pub(crate) fn event_names(&self) -> Vec<&'static str> {
        let mut names = Vec::with_capacity(3);
        if self.dry_fire {
            names.push("dry_fire");
        }
        if self.activate.is_some() {
            names.push("activate");
        }
        if !self.impacts.is_empty() {
            names.push("impact");
        }
        if !self.spawned.is_empty() {
            names.push("spawned");
        }
        names
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum WeaponFireAuthorization {
    Accepted,
    Rejected,
    Empty,
}

#[allow(clippy::too_many_arguments)] // weapon fire genuinely needs all of these inputs.
#[cfg(test)]
pub(crate) fn tick(
    registry: &mut EntityRegistry,
    active_wieldable: Option<EntityId>,
    snapshot: &ActionSnapshot,
    camera: &Camera,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    fire: WeaponFireAuthorization,
) -> WeaponFireEvents {
    let shoot = snapshot.button(Action::Shoot);
    let (aim_origin, aim_direction) = camera.aim_ray();
    let command = WeaponFireCommand {
        button: FireButtonState {
            pressed: shoot == ButtonState::Pressed,
            active: shoot.is_active(),
        },
        aim_origin,
        aim_direction,
        can_fire: true,
    };
    tick_resolved(
        registry,
        active_wieldable,
        &command,
        collision_world,
        hit_zone_store,
        anim_time,
        fire,
    )
}

#[allow(clippy::too_many_arguments)] // weapon fire genuinely needs all of these inputs.
#[cfg(test)]
pub(crate) fn tick_resolved(
    registry: &mut EntityRegistry,
    active_wieldable: Option<EntityId>,
    command: &WeaponFireCommand,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    fire: WeaponFireAuthorization,
) -> WeaponFireEvents {
    let Some(weapon_id) = active_wieldable else {
        return WeaponFireEvents::default();
    };

    let Ok(existing) = registry.get_component::<WeaponComponent>(weapon_id) else {
        return WeaponFireEvents::default();
    };
    let mut weapon = existing.clone();
    let pellet_salt_name = pellet_salt_name(registry, weapon_id, &weapon);

    let events = tick_resolved_component(
        registry,
        None,
        &mut weapon,
        &pellet_salt_name,
        0,
        command,
        &WeaponPlacementDescriptor::default(),
        collision_world,
        hit_zone_store,
        anim_time,
        fire,
    );

    let _ = registry.set_component(weapon_id, weapon);
    events
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn tick_resolved_component(
    registry: &EntityRegistry,
    owner_pawn: Option<EntityId>,
    weapon: &mut WeaponComponent,
    pellet_salt_name: &str,
    active_slot: usize,
    command: &WeaponFireCommand,
    placement: &WeaponPlacementDescriptor,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    fire: WeaponFireAuthorization,
) -> WeaponFireEvents {
    let stats = weapon.effective();
    let damage = stats.damage;
    let pellet_count = stats.pellet_count;
    let spread_radians = stats.spread_degrees.to_radians();
    let range = stats.range;
    let resolution = stats.resolution;
    let projectile = stats.projectile.cloned();
    let muzzle_offset = stats.muzzle_offset;
    let credit_source = stats.credit_source.to_string();
    match fire {
        WeaponFireAuthorization::Accepted => {
            // A shell position is consumed whether it is a single exact-axis ray
            // or a spread fan, preserving future spread changes' deterministic
            // sequence. Only a resolved shell advances this instance-local state.
            let shell_counter = weapon.shells_fired;
            weapon.shells_fired = weapon.shells_fired.wrapping_add(1);
            let (origin, direction) = if resolution == ResolutionMode::Projectile {
                resolve_projectile_launch_pose(
                    owner_pawn,
                    command.aim_origin,
                    command.aim_direction,
                    placement,
                    muzzle_offset,
                    collision_world,
                    registry,
                    hit_zone_store,
                    anim_time,
                    range,
                )
            } else {
                (command.aim_origin, command.aim_direction)
            };
            fire_hitscan(
                owner_pawn,
                origin,
                direction,
                collision_world,
                registry,
                hit_zone_store,
                anim_time,
                damage,
                pellet_count,
                spread_radians,
                range,
                resolution,
                projectile.as_ref(),
                &credit_source,
                shell_counter,
                pellet_salt_name,
                active_slot,
            )
        }
        WeaponFireAuthorization::Empty => WeaponFireEvents {
            dry_fire: true,
            ..WeaponFireEvents::default()
        },
        WeaponFireAuthorization::Rejected => WeaponFireEvents::default(),
    }
}

#[allow(clippy::too_many_arguments)] // weapon fire genuinely needs all of these inputs.
fn fire_hitscan(
    owner_pawn: Option<EntityId>,
    origin: Vec3,
    direction: Vec3,
    collision_world: &CollisionWorld,
    registry: &EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    damage: f32,
    pellet_count: u32,
    spread_radians: f32,
    range: f32,
    resolution: ResolutionMode,
    projectile: Option<&ProjectileDescriptor>,
    credit_source: &str,
    shell_counter: u32,
    pellet_salt_name: &str,
    active_slot: usize,
) -> WeaponFireEvents {
    let mut events = WeaponFireEvents {
        activate: Some(WeaponActivation { origin, direction }),
        impacts: Vec::with_capacity(pellet_count as usize),
        projectile_launches: Vec::new(),
        spawned: Vec::new(),
        dry_fire: false,
    };

    match resolution {
        ResolutionMode::Hitscan => {
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
                let impact = match resolve_nearest_hit(
                    owner_pawn,
                    origin,
                    pellet_direction,
                    collision_world,
                    registry,
                    hit_zone_store,
                    anim_time,
                    range,
                ) {
                    Some(NearestHit::Entity(entity)) => impact_from_entity(entity, damage),
                    Some(NearestHit::World(world)) => WeaponImpact {
                        point: world.point,
                        normal: world.normal,
                        target: None,
                        zone: None,
                        outcome: ActivationOutcome::Hit(DamagePayload { amount: damage }),
                    },
                    None => continue,
                };
                events.impacts.push(impact);
            }
        }
        ResolutionMode::Projectile => {
            let Some(projectile) = projectile else {
                log::warn!(
                    "[Weapon] projectile resolution has no projectile descriptor; dropping launch"
                );
                return events;
            };
            events.projectile_launches.push(ProjectileLaunch {
                origin,
                direction,
                speed: projectile.speed,
                radius: projectile.radius,
                range,
                lifetime: projectile.lifetime_ms / 1000.0,
                damage,
                credit_source: credit_source.to_string(),
                descriptor: projectile.clone(),
            });
        }
    }

    events
}

#[allow(clippy::too_many_arguments)] // mirrors the host/single-player hitscan inputs.
pub(crate) fn resolve_client_fire(
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
    collision_world: &CollisionWorld,
    registry: &EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    frame_dt: f32,
) -> Option<ClientFireResolution> {
    if !advance_client_fire_state(weapon, button, frame_dt) {
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
        spread_radians,
        range,
        resolution,
        projectile,
        damage,
        credit_source,
    ) = {
        let stats = weapon.effective();
        (
            stats.cooldown_ms,
            stats.pellet_count,
            stats.spread_degrees.to_radians(),
            stats.range,
            stats.resolution,
            stats.projectile.cloned(),
            stats.damage,
            stats.credit_source.to_string(),
        )
    };
    weapon.cooldown_remaining_ms = cooldown_ms;
    let (hits, projectile_launch) = match resolution {
        ResolutionMode::Hitscan => (
            resolve_client_hitscan(
                owner_pawn,
                aim_origin,
                aim_direction,
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
                collision_world,
                registry,
                hit_zone_store,
                anim_time,
                range,
            );
            (
                Vec::new(),
                Some(ProjectileLaunch {
                    origin,
                    direction,
                    speed: projectile.speed,
                    radius: projectile.radius,
                    range,
                    lifetime: projectile.lifetime_ms / 1000.0,
                    damage,
                    credit_source,
                    descriptor: projectile,
                }),
            )
        }
    };
    Some(ClientFireResolution {
        client_tick,
        hits,
        projectile_launch,
    })
}

pub(crate) fn advance_client_fire_state(
    weapon: &mut WeaponComponent,
    button: FireButtonState,
    frame_dt: f32,
) -> bool {
    let dt_ms = (frame_dt.max(0.0)) * 1000.0;
    weapon.cooldown_remaining_ms = (weapon.cooldown_remaining_ms - dt_ms).max(0.0);

    let fire_mode = weapon.effective().fire_mode;
    let wants_fire = match fire_mode {
        FireMode::Semi => button.pressed && !weapon.shoot_press_consumed,
        FireMode::Auto => button.active,
    };
    if fire_mode == FireMode::Semi && button.pressed {
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
) -> Vec<LocalHitRecord> {
    match resolution {
        ResolutionMode::Hitscan => {
            let mut hits = Vec::with_capacity(pellet_count as usize);
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
                // Only an entity hit produces a local hit record; a nearer world
                // hit (or no hit) yields none — the client owns no world-impact
                // record.
                if let Some(NearestHit::Entity(entity)) = resolve_nearest_hit(
                    owner_pawn,
                    origin,
                    pellet_direction,
                    collision_world,
                    registry,
                    hit_zone_store,
                    anim_time,
                    range,
                ) {
                    hits.push(local_hit_record(entity));
                }
            }
            hits
        }
        // Projectile flight is materialized by the connected client's mutable
        // post-loop path. This ray-resolution helper emits no same-frame hit;
        // the projectile declares its later collision or expiry instead.
        ResolutionMode::Projectile => Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_projectile_launch_pose(
    owner_pawn: Option<EntityId>,
    aim_origin: Vec3,
    aim_direction: Vec3,
    placement: &WeaponPlacementDescriptor,
    muzzle_offset: Option<Vec3>,
    collision_world: &CollisionWorld,
    registry: &EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    range: f32,
) -> (Vec3, Vec3) {
    let Some(muzzle_local) = muzzle_offset else {
        // This arm preserves the historical eye-origin launch exactly.
        return (aim_origin, aim_direction);
    };

    let muzzle = muzzle_world_origin(aim_origin, aim_direction, placement, muzzle_local);
    let convergence = resolve_nearest_hit(
        owner_pawn,
        aim_origin,
        aim_direction,
        collision_world,
        registry,
        hit_zone_store,
        anim_time,
        range,
    )
    .map_or(aim_origin + aim_direction * range, |hit| match hit {
        NearestHit::World(hit) => hit.point,
        NearestHit::Entity(hit) => hit.point,
    });
    let muzzle_to_convergence = convergence - muzzle;
    let length_squared = muzzle_to_convergence.length_squared();
    if !muzzle_to_convergence.is_finite()
        || !length_squared.is_finite()
        || length_squared <= MUZZLE_DIRECTION_EPSILON_SQUARED
        || muzzle_to_convergence.dot(aim_direction) <= 0.0
    {
        return (muzzle, aim_direction);
    }

    (muzzle, muzzle_to_convergence / length_squared.sqrt())
}

/// The deterministic pellet salt chooses a canonical descriptor identity first,
/// then the live component's credit source, and finally the shared unknown
/// source. Never use allocation-ordered entity/network ids here: spawn-order
/// reversal replays must preserve the sampled fan.
pub(crate) fn pellet_salt_name(
    registry: &EntityRegistry,
    weapon_id: EntityId,
    weapon: &WeaponComponent,
) -> String {
    registry
        .get_component::<DescriptorProvenance>(weapon_id)
        .ok()
        .map(|provenance| provenance.canonical_name.as_str())
        .filter(|name| !name.is_empty())
        .or_else(|| (!weapon.credit_source.is_empty()).then_some(weapon.credit_source.as_str()))
        .unwrap_or(UNKNOWN_WEAPON_CREDIT_SOURCE)
        .to_owned()
}

fn local_hit_record(entity: EntityRayHit) -> LocalHitRecord {
    LocalHitRecord {
        target: entity.target,
        point: entity.point,
        zone: entity.zone,
    }
}

/// A resolved world-geometry point along the fire ray. `toi` is the ray
/// parameter (distance, since `direction` is unit length) used to pick the
/// nearest of world vs. entity. Entity hits are resolved by the hit-zone
/// facility, which owns the AABB/capsule narrow phases and returns its own type.
#[derive(Debug, Clone, Copy)]
struct WorldHit {
    toi: f32,
    point: Vec3,
    normal: Vec3,
}

/// The winner of the world-vs-entity nearest-of resolution along a fire ray.
enum NearestHit {
    World(WorldHit),
    Entity(EntityRayHit),
}

/// Cast the fire ray against world geometry and the nearest targetable entity,
/// both clamped to `range`, and return whichever is nearer. On a tie (entity toi
/// == world toi) the wall wins (`entity.toi < world.toi`); an entity behind a
/// wall is never reached because its toi exceeds the wall's. Both the sim fire
/// path (`fire_hitscan`) and the client prediction path (`resolve_client_hitscan`)
/// resolve through here so the tie-break lives in one place.
fn resolve_nearest_hit(
    owner_pawn: Option<EntityId>,
    origin: Vec3,
    direction: Vec3,
    collision_world: &CollisionWorld,
    registry: &EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    range: f32,
) -> Option<NearestHit> {
    // World geometry hit — parry returns the nearest triangle intersection.
    let world_hit = cast_ray(
        collision_world,
        Point::new(origin.x, origin.y, origin.z),
        Vector::new(direction.x, direction.y, direction.z),
        range,
    )
    .map(|hit| WorldHit {
        toi: hit.time_of_impact,
        point: origin + direction * hit.time_of_impact,
        normal: Vec3::new(hit.normal.x, hit.normal.y, hit.normal.z),
    });

    // Nearest entity hit (authored AABB or bone-posed capsule), resolved entirely
    // by the standalone hit-zone facility.
    let entity_hit = nearest_entity_hit_ignoring(
        registry,
        hit_zone_store,
        anim_time,
        origin,
        direction,
        range,
        0.0,
        |candidate| owner_pawn == Some(candidate),
    );

    match (world_hit, entity_hit) {
        (Some(world), Some(entity)) if entity.toi < world.toi => Some(NearestHit::Entity(entity)),
        (Some(world), _) => Some(NearestHit::World(world)),
        (None, Some(entity)) => Some(NearestHit::Entity(entity)),
        (None, None) => None,
    }
}

/// Build a [`WeaponImpact`] from a facility entity hit, attaching the damage
/// payload and carrying the struck zone tag (if any) through to the caller.
fn impact_from_entity(entity: EntityRayHit, damage: f32) -> WeaponImpact {
    WeaponImpact {
        point: entity.point,
        normal: entity.normal,
        target: Some(entity.target),
        zone: entity.zone,
        outcome: ActivationOutcome::Hit(DamagePayload { amount: damage }),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::input::{Binding, InputSystem, PhysicalInput};
    use parry3d::math::Isometry;
    use parry3d::shape::TriMesh;
    use postretro_entities::components::health::{HealthComponent, Hitbox};
    use postretro_entities::components::projectile::ProjectileComponent;
    use postretro_entities::registry::{ComponentKind, Transform};
    use postretro_foundation::{
        AmmoResource, ProjectileBodyVisual, ProjectileVisual, ReloadStyle, WeaponDescriptor,
        WeaponResource,
    };
    use winit::event::MouseButton;

    const EPSILON: f32 = 1.0e-5;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < EPSILON
    }

    fn assert_vec3_approx(actual: Vec3, expected: Vec3) {
        assert!(
            approx_eq(actual.x, expected.x)
                && approx_eq(actual.y, expected.y)
                && approx_eq(actual.z, expected.z),
            "expected ({:.5}, {:.5}, {:.5}), got ({:.5}, {:.5}, {:.5})",
            expected.x,
            expected.y,
            expected.z,
            actual.x,
            actual.y,
            actual.z,
        );
    }

    fn assert_vec3_bits_eq(actual: Vec3, expected: Vec3) {
        assert_eq!(actual.x.to_bits(), expected.x.to_bits());
        assert_eq!(actual.y.to_bits(), expected.y.to_bits());
        assert_eq!(actual.z.to_bits(), expected.z.to_bits());
    }

    fn only_impact(events: &WeaponFireEvents) -> &WeaponImpact {
        let [impact] = events.impacts.as_slice() else {
            panic!("expected exactly one impact, got {}", events.impacts.len());
        };
        impact
    }

    pub(crate) fn weapon_component(fire_mode: FireMode, cooldown_ms: f32) -> WeaponComponent {
        WeaponComponent::from_descriptor(&WeaponDescriptor {
            damage: 25.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            range: 10.0,
            cooldown_ms,
            fire_mode,
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
        })
    }

    pub(crate) fn ammo_weapon_component(
        fire_mode: FireMode,
        cooldown_ms: f32,
        magazine: u32,
        cost_per_shot: u32,
    ) -> WeaponComponent {
        let mut descriptor = weapon_descriptor(fire_mode, cooldown_ms);
        descriptor.resource = Some(WeaponResource::Ammo(AmmoResource {
            ammo_type: "bullets.light".to_string(),
            magazine,
            cost_per_shot,
            reserve: 48,
            reload_ms: 900,
            reload_style: ReloadStyle::Magazine,
        }));
        WeaponComponent::from_descriptor(&descriptor)
    }

    fn weapon_descriptor(fire_mode: FireMode, cooldown_ms: f32) -> WeaponDescriptor {
        WeaponDescriptor {
            damage: 25.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            range: 10.0,
            cooldown_ms,
            fire_mode,
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
        }
    }

    fn projectile_weapon_component(muzzle_offset: Option<Vec3>) -> WeaponComponent {
        let mut descriptor = weapon_descriptor(FireMode::Semi, 100.0);
        descriptor.resolution = ResolutionMode::Projectile;
        descriptor.muzzle_offset = muzzle_offset.map(|offset| offset.to_array());
        descriptor.projectile = Some(ProjectileDescriptor {
            speed: 20.0,
            radius: 0.1,
            lifetime_ms: 1_000.0,
            visual: ProjectileVisual {
                body: ProjectileBodyVisual::Sprite {
                    sprite: "sprites/projectiles/test.png".to_string(),
                    size: 0.2,
                    opacity: 1.0,
                    rotation: 0.0,
                    tint: [1.0, 1.0, 1.0],
                    emissive: 0.0,
                    frame_duration_ms: None,
                },
                trail: None,
                light: None,
                impact_light: None,
            },
        });
        WeaponComponent::from_descriptor(&descriptor)
    }

    /// Run a weapon `tick` with an EMPTY hit-zone store and a zero animation
    /// clock — the no-skeletal-zones configuration, so these tests exercise the
    /// authored-AABB path exactly as before the facility landed (byte-identical
    /// behavior: an empty store routes every health+hitbox entity through the
    /// AABB narrow phase). Keeps the existing test bodies a one-word rename.
    pub(crate) fn fire_tick(
        registry: &mut EntityRegistry,
        active_wieldable: Option<EntityId>,
        snapshot: &ActionSnapshot,
        camera: &Camera,
        world: &CollisionWorld,
        _tick_dt: f32,
    ) -> WeaponFireEvents {
        let store = HitZoneStore::new();
        tick(
            registry,
            active_wieldable,
            snapshot,
            camera,
            world,
            &store,
            0.0,
            WeaponFireAuthorization::Accepted,
        )
    }

    pub(crate) fn spawn_weapon(
        registry: &mut EntityRegistry,
        component: WeaponComponent,
    ) -> EntityId {
        let id = registry.spawn(Transform::default());
        registry
            .set_component(id, component)
            .expect("weapon component should attach");
        id
    }

    /// Spawn a `Health` entity carrying a hitbox at a world position. Default
    /// `half_extents` make a unit cube (0.5 in each axis); `offset` defaults to
    /// zero so the AABB centers on `position`.
    fn spawn_hitbox_entity(
        registry: &mut EntityRegistry,
        position: Vec3,
        half_extents: Vec3,
        offset: Vec3,
    ) -> EntityId {
        let id = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry
            .set_component(
                id,
                HealthComponent {
                    max: 100.0,
                    current: 100.0,
                    hitbox: Some(Hitbox {
                        half_extents,
                        offset,
                    }),
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: std::collections::HashMap::new(),
                    contributor_ledger: Default::default(),
                },
            )
            .expect("health component should attach");
        id
    }

    pub(crate) fn input_system() -> InputSystem {
        InputSystem::new(vec![Binding::new(
            PhysicalInput::MouseButton(MouseButton::Left),
            Action::Shoot,
        )])
    }

    pub(crate) fn shoot_snapshot(input: &mut InputSystem, active: bool) -> ActionSnapshot {
        input.set_physical_input(PhysicalInput::MouseButton(MouseButton::Left), active);
        input.snapshot()
    }

    pub(crate) fn wall_world() -> CollisionWorld {
        let points = vec![
            Point::new(-1.0, -1.0, -5.0),
            Point::new(1.0, -1.0, -5.0),
            Point::new(1.0, 1.0, -5.0),
            Point::new(-1.0, 1.0, -5.0),
        ];
        let triangles = vec![[0u32, 1, 2], [0, 2, 3]];
        CollisionWorld {
            mesh: TriMesh::new(points, triangles),
            isometry: Isometry::identity(),
        }
    }

    #[test]
    fn muzzle_world_origin_composes_model_offset_through_each_placement_rotation() {
        let eye = Vec3::new(2.0, 3.0, 4.0);
        let aim = Vec3::NEG_Z;
        let muzzle_local = Vec3::new(0.2, -0.4, -0.7);
        let neutral = WeaponPlacementDescriptor::default();
        let canted = WeaponPlacementDescriptor {
            offset: postretro_foundation::PlacementOffset {
                right: 0.35,
                up: -0.15,
                forward: 0.6,
            },
            rotation: postretro_foundation::PlacementRotation {
                yaw: 25.0,
                pitch: -15.0,
                roll: 35.0,
            },
        };

        let neutral_origin = muzzle_world_origin(eye, aim, &neutral, muzzle_local);
        let canted_origin = muzzle_world_origin(eye, aim, &canted, muzzle_local);
        let (offset, rotation) = canted.camera_space();

        assert_vec3_approx(neutral_origin, eye + muzzle_local);
        assert_vec3_approx(canted_origin, eye + rotation * muzzle_local + offset);
        assert_ne!(
            neutral_origin, canted_origin,
            "placement must affect the muzzle"
        );
    }

    #[test]
    fn muzzle_world_origin_tracks_pitched_and_near_vertical_aim_without_nan() {
        let placement = WeaponPlacementDescriptor {
            offset: postretro_foundation::PlacementOffset {
                right: 0.1,
                up: 0.2,
                forward: 0.3,
            },
            rotation: postretro_foundation::PlacementRotation {
                yaw: 10.0,
                pitch: 20.0,
                roll: -30.0,
            },
        };
        let muzzle = Vec3::new(0.25, -0.5, -0.75);

        for aim in [
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::new(0.000_001, 1.0, 0.0).normalize(),
        ] {
            let origin = muzzle_world_origin(Vec3::ZERO, aim, &placement, muzzle);
            assert!(
                origin.is_finite(),
                "near-vertical aim must retain a finite basis"
            );
        }
    }

    #[test]
    fn projectile_launch_pose_converges_from_muzzle_on_hit_or_far_eye_ray() {
        let placement = WeaponPlacementDescriptor::default();
        let muzzle = Some(Vec3::new(0.5, 0.0, -0.8));
        let registry = EntityRegistry::new();
        let zones = HitZoneStore::new();

        let (far_origin, far_direction) = resolve_projectile_launch_pose(
            None,
            Vec3::ZERO,
            Vec3::NEG_Z,
            &placement,
            muzzle,
            &CollisionWorld::new(),
            &registry,
            &zones,
            0.0,
            10.0,
        );
        assert_vec3_approx(far_origin, Vec3::new(0.5, 0.0, -0.8));
        assert_vec3_approx(
            far_direction,
            (Vec3::new(0.0, 0.0, -10.0) - far_origin).normalize(),
        );

        let (hit_origin, hit_direction) = resolve_projectile_launch_pose(
            None,
            Vec3::ZERO,
            Vec3::NEG_Z,
            &placement,
            muzzle,
            &wall_world(),
            &registry,
            &zones,
            0.0,
            10.0,
        );
        assert_vec3_approx(hit_origin, far_origin);
        assert_vec3_approx(
            hit_direction,
            (Vec3::new(0.0, 0.0, -5.0) - hit_origin).normalize(),
        );
    }

    #[test]
    fn projectile_launch_pose_keeps_aim_when_convergence_is_behind_or_degenerate() {
        let placement = WeaponPlacementDescriptor::default();
        let registry = EntityRegistry::new();
        let zones = HitZoneStore::new();
        for muzzle in [Vec3::new(0.0, 0.0, -6.0), Vec3::new(0.0, 0.0, -5.0)] {
            let (origin, direction) = resolve_projectile_launch_pose(
                None,
                Vec3::ZERO,
                Vec3::NEG_Z,
                &placement,
                Some(muzzle),
                &wall_world(),
                &registry,
                &zones,
                0.0,
                10.0,
            );
            assert_vec3_approx(origin, muzzle);
            assert_vec3_approx(direction, Vec3::NEG_Z);
        }
    }

    #[test]
    fn projectile_without_muzzle_keeps_legacy_eye_launch_bits() {
        let mut weapon = projectile_weapon_component(None);
        let placement = WeaponPlacementDescriptor {
            offset: postretro_foundation::PlacementOffset {
                right: 0.7,
                up: -0.3,
                forward: 0.5,
            },
            rotation: postretro_foundation::PlacementRotation {
                yaw: 20.0,
                pitch: -10.0,
                roll: 5.0,
            },
        };
        let eye = Vec3::new(1.0, 2.0, 3.0);
        let aim = Vec3::new(0.2, -0.1, -0.97).normalize();
        let resolution = resolve_client_fire(
            None,
            &mut weapon,
            "weapon.unknown",
            0,
            FireButtonState {
                pressed: true,
                active: true,
            },
            eye,
            aim,
            &placement,
            None,
            1,
            &CollisionWorld::new(),
            &EntityRegistry::new(),
            &HitZoneStore::new(),
            0.0,
            0.0,
        )
        .expect("projectile fire resolves");
        let launch = resolution.projectile_launch.expect("projectile launch");
        assert_vec3_bits_eq(launch.origin, eye);
        assert_vec3_bits_eq(launch.direction, aim);
    }

    #[test]
    fn authoritative_projectile_launch_uses_the_same_composed_muzzle_origin() {
        let muzzle_local = Vec3::new(0.25, -0.1, -0.6);
        let placement = WeaponPlacementDescriptor {
            offset: postretro_foundation::PlacementOffset {
                right: 0.3,
                up: -0.2,
                forward: 0.7,
            },
            rotation: postretro_foundation::PlacementRotation {
                yaw: 20.0,
                pitch: -10.0,
                roll: 15.0,
            },
        };
        let command = WeaponFireCommand {
            button: FireButtonState {
                pressed: true,
                active: true,
            },
            aim_origin: Vec3::new(1.0, 2.0, 3.0),
            aim_direction: Vec3::NEG_Z,
            can_fire: true,
        };
        let registry = EntityRegistry::new();
        let mut weapon = projectile_weapon_component(Some(muzzle_local));
        let events = tick_resolved_component(
            &registry,
            None,
            &mut weapon,
            "weapon.unknown",
            0,
            &command,
            &placement,
            &CollisionWorld::new(),
            &HitZoneStore::new(),
            0.0,
            WeaponFireAuthorization::Accepted,
        );
        let [launch] = events.projectile_launches.as_slice() else {
            panic!("expected one projectile launch");
        };
        assert_vec3_approx(
            launch.origin,
            muzzle_world_origin(
                command.aim_origin,
                command.aim_direction,
                &placement,
                muzzle_local,
            ),
        );
        assert_eq!(launch.range, 10.0, "remaining range stays descriptor range");
    }

    #[test]
    fn client_fire_path_gates_held_trigger_while_cooling() {
        let mut registry = EntityRegistry::new();
        let target = spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let mut state = weapon_component(FireMode::Auto, 100.0);
        let world = CollisionWorld::new();
        let store = HitZoneStore::new();
        let button = FireButtonState {
            pressed: true,
            active: true,
        };

        let first = resolve_client_fire(
            None,
            &mut state,
            "weapon.unknown",
            0,
            button,
            Vec3::ZERO,
            Vec3::NEG_Z,
            &WeaponPlacementDescriptor::default(),
            None,
            7,
            &world,
            &registry,
            &store,
            0.0,
            0.0,
        )
        .expect("first fire passes");
        assert_eq!(first.client_tick, 7);
        assert_eq!(first.hits.len(), 1);
        assert_eq!(first.hits[0].target, target);
        assert_eq!(
            state.shells_fired, 1,
            "the resolved client shell advances once"
        );

        let blocked = resolve_client_fire(
            None,
            &mut state,
            "weapon.unknown",
            0,
            FireButtonState {
                pressed: false,
                active: true,
            },
            Vec3::ZERO,
            Vec3::NEG_Z,
            &WeaponPlacementDescriptor::default(),
            None,
            8,
            &world,
            &registry,
            &store,
            0.0,
            0.016,
        );
        assert!(blocked.is_none());
    }

    #[test]
    fn hitscan_world_hit_returns_impact_point_normal_and_damage_payload() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = wall_world();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        assert_eq!(events.event_names(), vec!["activate", "impact"]);
        let impact = only_impact(&events);
        assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, -5.0));
        assert_vec3_approx(impact.normal, Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(
            impact.outcome,
            ActivationOutcome::Hit(DamagePayload { amount: 25.0 })
        );
    }

    #[test]
    fn legacy_single_pellet_keeps_exact_axis_and_one_impact_event() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let (_, aim_direction) = camera.aim_ray();
        let world = wall_world();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        assert_eq!(events.impacts.len(), 1);
        assert_eq!(events.event_names(), vec!["activate", "impact"]);
        let activation = events.activate.expect("resolved shell activates once");
        assert_vec3_bits_eq(activation.direction, aim_direction);
        assert_vec3_approx(only_impact(&events).point, Vec3::new(0.0, 0.0, -5.0));
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_id)
                .expect("weapon remains attached")
                .shells_fired,
            1,
            "even a legacy exact-axis shell consumes one deterministic position"
        );
    }

    #[test]
    fn eight_zero_spread_pellets_resolve_eight_exact_axis_impacts() {
        let mut registry = EntityRegistry::new();
        let mut component = weapon_component(FireMode::Semi, 100.0);
        component.pellet_count = 8;
        component.spread_degrees = 0.0;
        let weapon_id = spawn_weapon(&mut registry, component);
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = wall_world();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        assert_eq!(events.impacts.len(), 8);
        assert_eq!(events.event_names(), vec!["activate", "impact"]);
        let exact_axis_point = events.impacts[0].point;
        for impact in &events.impacts {
            assert_vec3_bits_eq(impact.point, exact_axis_point);
            assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, -5.0));
            assert_vec3_approx(impact.normal, Vec3::new(0.0, 0.0, 1.0));
        }
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_id)
                .expect("weapon remains attached")
                .shells_fired,
            1,
            "one multi-pellet shell increments once"
        );
    }

    #[test]
    fn client_all_pellets_miss_returns_valid_empty_declaration_and_advances_once() {
        let registry = EntityRegistry::new();
        let mut weapon = weapon_component(FireMode::Auto, 100.0);
        weapon.pellet_count = 8;
        weapon.spread_degrees = 4.0;

        let resolution = resolve_client_fire(
            None,
            &mut weapon,
            "weapon.unknown",
            0,
            FireButtonState {
                pressed: true,
                active: true,
            },
            Vec3::ZERO,
            Vec3::NEG_Z,
            &WeaponPlacementDescriptor::default(),
            None,
            7,
            &CollisionWorld::new(),
            &registry,
            &HitZoneStore::new(),
            0.0,
            0.0,
        )
        .expect("an off-cooldown shot still declares an all-miss shell");

        assert!(
            resolution.hits.is_empty(),
            "an empty list is a valid miss declaration"
        );
        assert_eq!(weapon.shells_fired, 1);
    }

    #[test]
    fn client_zero_radius_pellets_keep_the_legacy_entity_query_results() {
        let mut registry = EntityRegistry::new();
        let target = spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let mut weapon = weapon_component(FireMode::Auto, 100.0);
        weapon.pellet_count = 8;
        weapon.spread_degrees = 0.0;
        let legacy = nearest_entity_hit(
            &registry,
            &HitZoneStore::new(),
            0.0,
            Vec3::ZERO,
            Vec3::NEG_Z,
            10.0,
            0.0,
        )
        .expect("the legacy r = 0 entity ray has a target");

        let resolution = resolve_client_fire(
            None,
            &mut weapon,
            "weapon.unknown",
            0,
            FireButtonState {
                pressed: true,
                active: true,
            },
            Vec3::ZERO,
            Vec3::NEG_Z,
            &WeaponPlacementDescriptor::default(),
            None,
            7,
            &CollisionWorld::new(),
            &registry,
            &HitZoneStore::new(),
            0.0,
            0.0,
        )
        .expect("an off-cooldown client shell resolves");

        assert_eq!(resolution.hits.len(), 8);
        for hit in resolution.hits {
            assert_eq!(hit.target, target);
            assert_vec3_approx(hit.point, Vec3::new(0.0, 0.0, -4.5));
            assert_vec3_bits_eq(hit.point, legacy.point);
            assert_eq!(hit.zone, legacy.zone);
        }
        assert_eq!(weapon.shells_fired, 1);
    }

    #[test]
    fn player_sized_hitbox_is_targetable_but_owner_fire_skips_it() {
        let mut registry = EntityRegistry::new();
        // This is the authored player body: its eye-origin is inside y=[0, 1.6].
        let player = spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.8, 0.0),
            Vec3::new(0.2, 0.8, 0.2),
            Vec3::ZERO,
        );
        let target = spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.8, -5.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let zones = HitZoneStore::new();

        let player_hit = nearest_entity_hit(
            &registry,
            &zones,
            0.0,
            Vec3::new(0.0, 0.8, 2.0),
            Vec3::NEG_Z,
            10.0,
            0.0,
        )
        .expect("the player body is targetable from outside");
        assert_eq!(player_hit.target, player);

        let command = WeaponFireCommand {
            button: FireButtonState {
                pressed: true,
                active: true,
            },
            aim_origin: Vec3::new(0.0, 0.8, 0.0),
            aim_direction: Vec3::NEG_Z,
            can_fire: true,
        };
        let mut authoritative_hitscan = weapon_component(FireMode::Auto, 0.0);
        authoritative_hitscan.pellet_count = 8;
        let authoritative_events = tick_resolved_component(
            &registry,
            Some(player),
            &mut authoritative_hitscan,
            "weapon.player",
            0,
            &command,
            &WeaponPlacementDescriptor::default(),
            &CollisionWorld::new(),
            &zones,
            0.0,
            WeaponFireAuthorization::Accepted,
        );
        assert_eq!(authoritative_events.impacts.len(), 8);
        assert!(
            authoritative_events
                .impacts
                .iter()
                .all(|impact| impact.target == Some(target))
        );

        let mut hitscan = weapon_component(FireMode::Auto, 0.0);
        hitscan.pellet_count = 8;
        let hits = resolve_client_fire(
            Some(player),
            &mut hitscan,
            "weapon.player",
            0,
            FireButtonState {
                pressed: true,
                active: true,
            },
            Vec3::new(0.0, 0.8, 0.0),
            Vec3::NEG_Z,
            &WeaponPlacementDescriptor::default(),
            None,
            1,
            &CollisionWorld::new(),
            &registry,
            &zones,
            0.0,
            0.0,
        )
        .expect("the pellet shell resolves");
        assert_eq!(hits.hits.len(), 8);
        assert!(hits.hits.iter().all(|hit| hit.target == target));

        let mut projectile = projectile_weapon_component(Some(Vec3::new(0.5, 0.0, -0.4)));
        let launch = resolve_client_fire(
            Some(player),
            &mut projectile,
            "weapon.player.projectile",
            0,
            FireButtonState {
                pressed: true,
                active: true,
            },
            Vec3::new(0.0, 0.8, 0.0),
            Vec3::NEG_Z,
            &WeaponPlacementDescriptor::default(),
            Some(Vec3::new(0.5, 0.0, -0.4)),
            2,
            &CollisionWorld::new(),
            &registry,
            &zones,
            0.0,
            0.0,
        )
        .expect("the projectile shell resolves")
        .projectile_launch
        .expect("the projectile launch is deferred");
        assert!(
            launch.direction.x < -0.05,
            "muzzle convergence aims at the other target, not the owner's interior hitbox"
        );
    }

    #[test]
    fn client_fire_gate_does_not_advance_shell_counter_without_a_resolved_shell() {
        let registry = EntityRegistry::new();
        let mut weapon = weapon_component(FireMode::Auto, 100.0);
        weapon.shells_fired = 9;
        weapon.cooldown_remaining_ms = 1.0;

        let resolution = resolve_client_fire(
            None,
            &mut weapon,
            "weapon.unknown",
            0,
            FireButtonState {
                pressed: false,
                active: true,
            },
            Vec3::ZERO,
            Vec3::NEG_Z,
            &WeaponPlacementDescriptor::default(),
            None,
            7,
            &CollisionWorld::new(),
            &registry,
            &HitZoneStore::new(),
            0.0,
            0.0,
        );

        assert!(resolution.is_none());
        assert_eq!(weapon.shells_fired, 9);
    }

    #[test]
    fn no_tick_client_fire_state_advance_does_not_consume_a_shell_position() {
        let mut weapon = weapon_component(FireMode::Auto, 100.0);
        weapon.shells_fired = 9;

        assert!(advance_client_fire_state(
            &mut weapon,
            FireButtonState {
                pressed: false,
                active: true,
            },
            0.0,
        ));
        assert_eq!(weapon.shells_fired, 9);
    }

    #[test]
    fn pellet_salt_name_prefers_provenance_then_credit_source_then_unknown() {
        let mut registry = EntityRegistry::new();

        let mut provenance_component = weapon_component(FireMode::Semi, 100.0);
        provenance_component.credit_source = "weapon.credit".to_string();
        let provenance_weapon = spawn_weapon(&mut registry, provenance_component);
        registry
            .set_component(
                provenance_weapon,
                DescriptorProvenance {
                    canonical_name: "weapon.canonical".to_string(),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: postretro_entities::provenance::DescriptorSpawnPath::DefaultWeapon,
                },
            )
            .expect("weapon provenance attaches");

        let mut credit_component = weapon_component(FireMode::Semi, 100.0);
        credit_component.credit_source = "weapon.credit".to_string();
        let credit_weapon = spawn_weapon(&mut registry, credit_component);

        let mut unknown_component = weapon_component(FireMode::Semi, 100.0);
        unknown_component.credit_source.clear();
        let unknown_weapon = spawn_weapon(&mut registry, unknown_component);

        assert_eq!(
            pellet_salt_name(
                &registry,
                provenance_weapon,
                registry
                    .get_component::<WeaponComponent>(provenance_weapon)
                    .expect("weapon component remains attached"),
            ),
            "weapon.canonical"
        );
        assert_eq!(
            pellet_salt_name(
                &registry,
                credit_weapon,
                registry
                    .get_component::<WeaponComponent>(credit_weapon)
                    .expect("weapon component remains attached"),
            ),
            "weapon.credit"
        );
        assert_eq!(
            pellet_salt_name(
                &registry,
                unknown_weapon,
                registry
                    .get_component::<WeaponComponent>(unknown_weapon)
                    .expect("weapon component remains attached"),
            ),
            UNKNOWN_WEAPON_CREDIT_SOURCE
        );
    }

    #[test]
    fn inactive_or_missing_wieldable_does_not_fire() {
        let mut registry = EntityRegistry::new();
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = CollisionWorld::new();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick(&mut registry, None, &pressed, &camera, &world, 1.0 / 60.0);
        assert!(events.event_names().is_empty());

        let non_weapon = registry.spawn(Transform::default());
        let events = fire_tick(
            &mut registry,
            Some(non_weapon),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );
        assert!(events.event_names().is_empty());
        assert!(
            registry
                .iter_with_kind(ComponentKind::Weapon)
                .next()
                .is_none()
        );
    }

    // The AABB slab test and the entity-hit walk relocated to the hit-zone
    // facility (`scripting/systems/hit_zones.rs`) along with `ray_aabb_slab` /
    // `nearest_entity_hit`; their unit tests live there now. The weapon-level
    // tests below cover the delegation + world-vs-entity nearest-of resolution.

    #[test]
    fn entity_hit_reported_through_weapon_impact() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let target = spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -4.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        // Empty world: no wall, so the entity is the only contender.
        let world = CollisionWorld::new();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        let impact = only_impact(&events);
        assert_eq!(
            impact.target,
            Some(target),
            "spatial target rides beside payload"
        );
        assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, -3.5));
        assert_vec3_approx(impact.normal, Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(
            impact.outcome,
            ActivationOutcome::Hit(DamagePayload { amount: 25.0 })
        );
    }

    #[test]
    fn world_wins_when_wall_is_nearer_than_entity() {
        // Wall sits at z = -5; entity box behind it at z = -8. The wall is
        // nearer, so it is selected and no entity target is reported.
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -8.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = wall_world();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        let impact = only_impact(&events);
        assert_eq!(impact.target, None, "wall wins; no entity target");
        assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, -5.0));
    }

    #[test]
    fn entity_wins_when_nearer_than_wall() {
        // Entity box at z = -3, in front of the wall at z = -5. The entity is
        // nearer and is selected over the wall.
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let target = spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -3.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = wall_world();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        let impact = only_impact(&events);
        assert_eq!(impact.target, Some(target), "nearer entity beats the wall");
        assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, -2.5));
    }

    #[test]
    fn entity_beyond_range_is_not_targeted() {
        // Weapon range is 10.0 (see `weapon_component`). The entity sits at
        // z = -12, beyond range, and there is no wall: nothing is hit.
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -12.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = CollisionWorld::new();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        assert!(
            events.impacts.is_empty(),
            "entity beyond weapon range is not targeted"
        );
    }

    #[test]
    fn near_miss_resolves_to_wall_behind() {
        // A hitbox entity sits just off the ray (a near miss) while the wall
        // lies behind it; the shot passes the entity and strikes the wall.
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        spawn_hitbox_entity(
            &mut registry,
            Vec3::new(2.0, 0.0, -3.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = wall_world();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        let impact = only_impact(&events);
        assert_eq!(impact.target, None, "near miss falls through to the wall");
        assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, -5.0));
    }

    // Regression: a zero-HP entity stays targetable until an authored lifecycle
    // action makes it terminally inert, so a downed target can receive a later
    // impact (for example, the zombie gib policy).
    #[test]
    fn zero_hp_entity_on_ray_remains_targetable_before_terminal_removal() {
        // Entity with current == 0.0 sits directly on the ray in front of the
        // wall. Zero HP alone does not let the wall win the ray.
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let corpse = spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -3.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        // Drive health to zero to simulate a downed entity before its authored
        // lifecycle decides whether it resurrects or despawns.
        let mut health = registry
            .get_component::<HealthComponent>(corpse)
            .expect("health component should exist")
            .clone();
        health.current = 0.0;
        registry
            .set_component(corpse, health)
            .expect("health component update should succeed");

        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = wall_world();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            1.0 / 60.0,
        );

        let impact = only_impact(&events);
        assert_eq!(impact.target, Some(corpse));
        assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, -2.5));
    }

    // --- Skeletal hit-zone delegation ---------------------------------------

    use crate::scripting_systems::hit_zones::ModelHitZones;
    use postretro_entities::components::mesh::MeshComponent;
    use postretro_model::skeleton::{Joint, RestLocal, Skeleton};
    use postretro_render_data::cone_frustum::Aabb;
    use std::sync::Arc;

    /// Build a store holding one model with a single TAGGED LEAF joint at the
    /// model origin — a sphere of `radius`. The derived bound is the sphere's box
    /// so the broad phase admits it. Static (no clip), so any anim_time poses the
    /// joint to the origin.
    fn head_zone_store(
        handle: &str,
        radius: f32,
    ) -> crate::scripting_systems::hit_zones::HitZoneStore {
        let skeleton = Skeleton {
            joints: vec![Joint {
                parent: None,
                inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
                rest_local: RestLocal::default(),
            }],
        };
        let model = ModelHitZones {
            skeleton: Arc::new(skeleton),
            clips: Arc::new(vec![]),
            joint_zones: vec![Some(postretro_model::gltf_loader::JointZone {
                tag: "head".to_string(),
                radius: Some(radius),
            })],
            sockets: std::collections::HashMap::new(),
            derived_bound: Some(Aabb {
                min: Vec3::splat(-radius),
                max: Vec3::splat(radius),
            }),
            legs: Vec::new(),
            pose_stack: Arc::new(postretro_model::pose_modifier::PoseModifierStack::default()),
        };
        let mut store = HitZoneStore::new();
        store.insert_for_test(postretro_model::ModelHandle::from(handle), model);
        store
    }

    /// Run `tick` with a populated hit-zone store and animation clock.
    #[allow(clippy::too_many_arguments)] // test harness threads the full tick context
    pub(crate) fn fire_tick_with(
        registry: &mut EntityRegistry,
        active_wieldable: Option<EntityId>,
        snapshot: &ActionSnapshot,
        camera: &Camera,
        world: &CollisionWorld,
        store: &HitZoneStore,
        anim_time: f64,
        _tick_dt: f32,
    ) -> WeaponFireEvents {
        tick(
            registry,
            active_wieldable,
            snapshot,
            camera,
            world,
            store,
            anim_time,
            WeaponFireAuthorization::Accepted,
        )
    }

    /// Spawn a health + stateless-mesh entity that uses a zone-bearing model.
    fn spawn_zone_entity(registry: &mut EntityRegistry, model: &str, position: Vec3) -> EntityId {
        let id = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry
            .set_component(
                id,
                HealthComponent {
                    max: 100.0,
                    current: 100.0,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: std::collections::HashMap::new(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
        registry
            .set_component(id, MeshComponent::stateless(model.to_string()))
            .unwrap();
        id
    }

    /// Spawn the client-side presentation shape of a remote enemy: mesh only, no
    /// local Health, so local hits can produce hitmarker/FX but cannot apply damage.
    fn spawn_mesh_only_zone_entity(
        registry: &mut EntityRegistry,
        model: &str,
        position: Vec3,
    ) -> EntityId {
        let id = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry
            .set_component(id, MeshComponent::stateless(model.to_string()))
            .unwrap();
        id
    }

    #[test]
    fn client_fire_resolves_remote_enemy_at_presentation_pose_without_health() {
        let mut registry = EntityRegistry::new();
        let store = head_zone_store("mob", 0.5);
        let target = spawn_mesh_only_zone_entity(&mut registry, "mob", Vec3::new(5.0, 0.0, -4.0));
        let mut state = weapon_component(FireMode::Semi, 100.0);

        // Remote interpolation has already sampled the network buffer and written the
        // rendered pose into the registry before the client fire path runs. The host's
        // present pose would be off the ray; the presentation pose is directly ahead.
        let rendered_pose = Transform {
            position: Vec3::new(0.0, 0.0, -4.0),
            ..Transform::default()
        };
        registry
            .set_presentation_transform(target, rendered_pose)
            .expect("remote interpolation writes the rendered pose");
        assert_vec3_approx(
            registry
                .interpolated_transform(target, 0.5)
                .unwrap()
                .position,
            rendered_pose.position,
        );
        assert_eq!(
            registry.has_component_kind(target, ComponentKind::Health),
            Ok(false),
            "remote client enemies carry no local Health before firing"
        );

        let resolution = resolve_client_fire(
            None,
            &mut state,
            "weapon.unknown",
            0,
            FireButtonState {
                pressed: true,
                active: true,
            },
            Vec3::ZERO,
            Vec3::NEG_Z,
            &WeaponPlacementDescriptor::default(),
            None,
            77,
            &CollisionWorld::new(),
            &registry,
            &store,
            0.0,
            0.0,
        )
        .expect("off-cooldown client fire resolves");

        assert_eq!(resolution.client_tick, 77);
        assert_eq!(resolution.hits.len(), 1);
        assert_eq!(resolution.hits[0].target, target);
        assert_eq!(resolution.hits[0].zone.as_deref(), Some("head"));
        assert_eq!(
            registry.has_component_kind(target, ComponentKind::Health),
            Ok(false),
            "local hit detection must not attach or mutate client-side Health"
        );
    }

    /// A zone hit through the full weapon path surfaces its zone tag on the
    /// impact (the zone-multiplier damage routing site reads `impact.zone`; here we only surface it).
    #[test]
    fn zone_hit_reports_zone_tag_through_weapon_impact() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        // Head sphere (r=0.5) at the entity, placed on the -Z ray at z=-4.
        let store = head_zone_store("mob", 0.5);
        let target = spawn_zone_entity(&mut registry, "mob", Vec3::new(0.0, 0.0, -4.0));
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = CollisionWorld::new(); // empty world: the zone is the only contender
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick_with(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            &store,
            0.0,
            1.0 / 60.0,
        );

        let impact = only_impact(&events);
        assert_eq!(impact.target, Some(target), "zone entity is targeted");
        assert_eq!(
            impact.zone.as_deref(),
            Some("head"),
            "the struck zone tag rides on the impact"
        );
    }

    /// A wall in front of a zone-bearing entity still wins the nearest-of: the
    /// world hit is nearer, so no entity target / zone is reported.
    #[test]
    fn wall_in_front_of_zone_still_wins() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let store = head_zone_store("mob", 0.5);
        // Zone entity BEHIND the wall (wall at z=-5; entity at z=-8).
        spawn_zone_entity(&mut registry, "mob", Vec3::new(0.0, 0.0, -8.0));
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = wall_world();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick_with(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            &store,
            0.0,
            1.0 / 60.0,
        );

        let impact = only_impact(&events);
        assert_eq!(impact.target, None, "wall wins; no zone entity targeted");
        assert_eq!(impact.zone, None, "no zone tag for a world hit");
        assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, -5.0));
    }

    /// The facility, called directly with an arbitrary ray (no weapon, no
    /// camera), reports the SAME nearest entity hit the weapon path reports for
    /// that ray — proving the weapon merely delegates.
    #[test]
    fn facility_direct_call_matches_weapon_entity_hit() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let store = head_zone_store("mob", 0.5);
        let target = spawn_zone_entity(&mut registry, "mob", Vec3::new(0.0, 0.0, -4.0));

        // The weapon fires straight down -Z (camera at origin, yaw/pitch 0).
        let origin = Vec3::ZERO;
        let direction = Vec3::new(0.0, 0.0, -1.0);

        // Direct facility call with the same ray + range (weapon range = 10).
        let direct = nearest_entity_hit(&registry, &store, 0.0, origin, direction, 10.0, 0.0)
            .expect("facility resolves the entity directly");

        // The weapon path for the same ray.
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = CollisionWorld::new();
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);
        let events = fire_tick_with(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            &store,
            0.0,
            1.0 / 60.0,
        );
        let impact = only_impact(&events);

        assert_eq!(Some(direct.target), impact.target, "same target");
        assert_eq!(direct.zone, impact.zone, "same zone tag");
        assert_vec3_approx(direct.point, impact.point);
        assert_eq!(direct.target, target);
    }

    fn client_weapon_registry() -> (EntityRegistry, EntityId) {
        let mut registry = EntityRegistry::new();
        let weapon = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        (registry, weapon)
    }

    fn set_client_cooldown(registry: &mut EntityRegistry, weapon: EntityId, value: f32) {
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        component.cooldown_remaining_ms = value;
        registry.set_component(weapon, component).unwrap();
    }

    fn client_cooldown(registry: &EntityRegistry, weapon: EntityId) -> f32 {
        registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .cooldown_remaining_ms
    }

    #[test]
    fn predicted_shot_records_local_presentation_markers() {
        let target = EntityId::from_raw(2);
        let resolution = ClientFireResolution {
            client_tick: 9,
            hits: vec![LocalHitRecord {
                target,
                point: Vec3::new(1.0, 2.0, 3.0),
                zone: Some("head".to_string()),
            }],
            projectile_launch: None,
        };
        let mut predicted = ClientPredictedShots::new();

        predicted.predict(0xA, EntityId::from_raw(1), &resolution, 0.0, 100.0);

        let record = predicted.get(0xA).expect("shot should be recorded");
        assert_eq!(record.client_tick, 9);
        assert!(record.muzzle_fx_visible);
        assert!(record.hitmarker_visible);
        assert_eq!(record.status, PredictedShotStatus::Pending);
    }

    #[test]
    fn predicted_projectile_impact_marks_hitmarker_after_later_resolution() {
        let resolution = ClientFireResolution {
            client_tick: 9,
            hits: Vec::new(),
            projectile_launch: None,
        };
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(0xA, EntityId::from_raw(1), &resolution, 0.0, 100.0);

        assert!(
            !predicted
                .get(0xA)
                .expect("projectile fire is pending")
                .hitmarker_visible
        );
        predicted.mark_hitmarker(0xA);
        assert!(
            predicted
                .get(0xA)
                .expect("projectile remains pending before verdict")
                .hitmarker_visible
        );
    }

    #[test]
    fn shot_verdict_accept_confirms_predicted_markers() {
        let resolution = ClientFireResolution {
            client_tick: 9,
            hits: vec![LocalHitRecord {
                target: EntityId::from_raw(2),
                point: Vec3::ZERO,
                zone: None,
            }],
            projectile_launch: None,
        };
        let (mut registry, weapon) = client_weapon_registry();
        set_client_cooldown(&mut registry, weapon, 100.0);
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(0xA, weapon, &resolution, 0.0, 100.0);

        let record = predicted
            .apply_verdict(&mut registry, 0xA, true, true)
            .expect("verdict should match a predicted shot");

        assert!(record.muzzle_fx_visible);
        assert!(record.hitmarker_visible);
        assert_eq!(record.status, PredictedShotStatus::Accepted);
        assert!(approx_eq(client_cooldown(&registry, weapon), 100.0));
        assert!(
            predicted.get(0xA).is_none(),
            "a terminal verdict prunes the record"
        );
    }

    #[test]
    fn shot_verdict_authorized_miss_keeps_fire_state_and_retracts_hitmarker() {
        let resolution = ClientFireResolution {
            client_tick: 9,
            hits: vec![LocalHitRecord {
                target: EntityId::from_raw(2),
                point: Vec3::ZERO,
                zone: None,
            }],
            projectile_launch: None,
        };
        let (mut registry, weapon) = client_weapon_registry();
        set_client_cooldown(&mut registry, weapon, 100.0);
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(0xA, weapon, &resolution, 25.0, 100.0);

        let record = predicted
            .apply_verdict(&mut registry, 0xA, true, false)
            .expect("verdict should match a predicted shot");

        assert!(record.muzzle_fx_visible);
        assert!(!record.hitmarker_visible);
        assert_eq!(record.status, PredictedShotStatus::Accepted);
        assert!(approx_eq(client_cooldown(&registry, weapon), 100.0));
        assert!(
            predicted.get(0xA).is_none(),
            "a terminal verdict prunes the record"
        );
    }

    #[test]
    fn shot_verdict_reject_rolls_back_local_presentation_and_cooldown() {
        let resolution = ClientFireResolution {
            client_tick: 9,
            hits: vec![LocalHitRecord {
                target: EntityId::from_raw(2),
                point: Vec3::ZERO,
                zone: None,
            }],
            projectile_launch: None,
        };
        let (mut registry, weapon) = client_weapon_registry();
        set_client_cooldown(&mut registry, weapon, 100.0);
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(0xA, weapon, &resolution, 25.0, 100.0);

        let record = predicted
            .apply_verdict(&mut registry, 0xA, false, false)
            .expect("verdict should match a predicted shot");

        assert!(!record.muzzle_fx_visible);
        assert!(!record.hitmarker_visible);
        assert_eq!(record.status, PredictedShotStatus::Rejected);
        assert!(approx_eq(client_cooldown(&registry, weapon), 25.0));
        assert!(
            predicted.get(0xA).is_none(),
            "a terminal verdict prunes the record"
        );
    }

    // Regression: a host-rejected projectile kept flying locally until its later
    // impact/expiry declaration, despite FIRE already having no authority.
    #[test]
    fn shot_verdict_reject_removes_only_matching_predicted_projectile() {
        let resolution = ClientFireResolution {
            client_tick: 9,
            hits: Vec::new(),
            projectile_launch: None,
        };
        let (mut registry, weapon) = client_weapon_registry();
        let owner = registry.spawn(Transform::default());
        let remote_target = registry.spawn(Transform::default());
        registry
            .set_component(
                remote_target,
                HealthComponent {
                    max: 100.0,
                    current: 100.0,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .expect("remote target health attaches");
        let spawn_predicted = |registry: &mut EntityRegistry, shot_id| {
            let projectile = registry.spawn(Transform::default());
            registry
                .set_component(
                    projectile,
                    ProjectileComponent {
                        direction: Vec3::NEG_Z.to_array(),
                        speed: 10.0,
                        radius: 0.1,
                        remaining_range: 100.0,
                        remaining_lifetime: 10.0,
                        damage: 25.0,
                        credit_source: "weapon.test.projectile".to_string(),
                        owner_pawn: owner,
                        owner_weapon: weapon,
                        spawned: false,
                        predicted_shot_id: Some(shot_id),
                        elapsed_flight_age: 0.0,
                        flipbook_active: false,
                        impact_light: None,
                    },
                )
                .expect("predicted projectile state attaches");
            projectile
        };
        let rejected = spawn_predicted(&mut registry, 0xA);
        let other = spawn_predicted(&mut registry, 0xB);
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(0xA, weapon, &resolution, 25.0, 100.0);

        let record = predicted
            .apply_verdict(&mut registry, 0xA, false, false)
            .expect("prompt rejection matches the predicted fire");

        assert_eq!(record.status, PredictedShotStatus::Rejected);
        assert!(!registry.exists(rejected));
        assert!(
            registry.exists(other),
            "a different local shot identity keeps flying"
        );
        assert_eq!(
            registry
                .get_component::<HealthComponent>(remote_target)
                .expect("remote health remains host-owned")
                .current,
            100.0
        );
    }

    #[test]
    fn duplicate_or_late_reject_does_not_undo_accepted_predicted_shot() {
        let resolution = ClientFireResolution {
            client_tick: 9,
            hits: vec![LocalHitRecord {
                target: EntityId::from_raw(2),
                point: Vec3::ZERO,
                zone: None,
            }],
            projectile_launch: None,
        };
        let (mut registry, weapon) = client_weapon_registry();
        set_client_cooldown(&mut registry, weapon, 100.0);
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(0xA, weapon, &resolution, 25.0, 100.0);

        let accepted = predicted
            .apply_verdict(&mut registry, 0xA, true, true)
            .expect("accept should match");
        assert_eq!(accepted.status, PredictedShotStatus::Accepted);
        assert!(accepted.muzzle_fx_visible);
        assert!(accepted.hitmarker_visible);

        // The terminal accept pruned the record, so a late reject finds nothing
        // and cannot undo the accepted shot's cooldown or presentation.
        assert!(
            predicted
                .apply_verdict(&mut registry, 0xA, false, false)
                .is_none()
        );
        assert!(predicted.get(0xA).is_none());
        assert!(approx_eq(client_cooldown(&registry, weapon), 100.0));
    }

    #[test]
    fn stale_reject_does_not_overwrite_fresh_authoritative_cooldown() {
        let resolution = ClientFireResolution {
            client_tick: 9,
            hits: vec![LocalHitRecord {
                target: EntityId::from_raw(2),
                point: Vec3::ZERO,
                zone: None,
            }],
            projectile_launch: None,
        };
        let (mut registry, weapon) = client_weapon_registry();
        set_client_cooldown(&mut registry, weapon, 100.0);
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(0xA, weapon, &resolution, 25.0, 100.0);

        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        predicted.reconcile_cooldown(weapon, &mut component, 12.0);
        registry.set_component(weapon, component).unwrap();
        let record = predicted
            .apply_verdict(&mut registry, 0xA, false, false)
            .expect("reject should match");

        assert_eq!(record.status, PredictedShotStatus::Rejected);
        assert!(!record.muzzle_fx_visible);
        assert!(!record.hitmarker_visible);
        assert!(
            approx_eq(client_cooldown(&registry, weapon), 12.0),
            "fresh owner-private cooldown must win over stale rollback"
        );
        assert!(
            predicted.get(0xA).is_none(),
            "a terminal verdict prunes the record"
        );
    }

    #[test]
    fn owner_private_cooldown_reconciles_live_client_weapon() {
        let (mut registry, weapon) = client_weapon_registry();
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        component.cooldown_remaining_ms = 100.0;
        let mut predicted = ClientPredictedShots::new();

        predicted.reconcile_cooldown(weapon, &mut component, 42.0);
        registry.set_component(weapon, component).unwrap();

        assert!(approx_eq(client_cooldown(&registry, weapon), 42.0));
        assert_eq!(
            predicted
                .cooldown_authority_generation
                .get(&weapon)
                .copied(),
            Some(1)
        );
    }
}
