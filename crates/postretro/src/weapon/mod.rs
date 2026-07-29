// Weapon fire tick, hitscan/local hit resolution, and client fire prediction: owns fire commands, local hit records, and predicted-shot reconciliation state.
// See: context/lib/entity_model.md §5, §7

use std::collections::HashMap;

use glam::Vec3;
use parry3d::math::{Point, Vector};
#[cfg(test)]
use postretro_entities::EntityTypeDescriptor;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::registry::{EntityId, EntityRegistry};
use postretro_foundation::{FireMode, ResolutionMode};

use crate::collision::{CollisionWorld, cast_ray};
use crate::scripting_systems::hit_zones::{EntityRayHit, HitZoneStore, nearest_entity_hit};
#[cfg(test)]
use crate::{
    camera::Camera,
    input::{Action, ActionSnapshot, ButtonState},
};

mod damage;
mod impact;

pub(crate) use damage::DamagePayload;
pub(crate) use impact::sprite_collection as impact_sprite_collection;
pub(crate) use impact::{lifetime as impact_lifetime, spawn_impact_effect_at};

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
pub(crate) struct ClientWeaponState {
    pub(crate) pawn: EntityId,
    pub(crate) cooldown_remaining_ms: f32,
    pub(crate) cooldown_ms: f32,
    pub(crate) cooldown_authority_generation: u64,
    pub(crate) fire_mode: FireMode,
    pub(crate) resolution: ResolutionMode,
    pub(crate) range: f32,
    pub(crate) shoot_press_consumed: bool,
}

impl ClientWeaponState {
    /// Build client prediction state from the host-resolved payload. Connected
    /// clients must not consult their local registry for these values: doing so
    /// would make the divergent peer predict with precisely the data replication
    /// was introduced to replace.
    pub(crate) fn from_host_tuning(
        pawn: EntityId,
        tuning: &crate::netcode::DefaultWeaponFirePayload,
    ) -> Self {
        Self {
            pawn,
            cooldown_remaining_ms: 0.0,
            cooldown_ms: tuning.cooldown_ms,
            cooldown_authority_generation: 0,
            fire_mode: tuning.fire_mode,
            resolution: tuning.resolution,
            range: tuning.range,
            shoot_press_consumed: false,
        }
    }

    /// Synchronize the descriptor-derived client prediction carrier with the
    /// latest host payload. Returns whether prediction history still belongs to
    /// the same active pawn and may be retained.
    pub(crate) fn sync_from_host_tuning(
        state: &mut Option<Self>,
        pawn: Option<EntityId>,
        tuning: Option<&crate::netcode::DefaultWeaponFirePayload>,
    ) -> bool {
        let (Some(pawn), Some(tuning)) = (pawn, tuning) else {
            *state = None;
            return false;
        };
        if let Some(state) = state.as_mut().filter(|state| state.pawn == pawn) {
            state.cooldown_ms = tuning.cooldown_ms;
            state.fire_mode = tuning.fire_mode;
            state.resolution = tuning.resolution;
            state.range = tuning.range;
            return true;
        }
        *state = Some(Self::from_host_tuning(pawn, tuning));
        false
    }

    #[cfg(test)]
    pub(crate) fn from_local_pawn_descriptor(
        pawn: EntityId,
        entity_class: &str,
        descriptors: &[EntityTypeDescriptor],
    ) -> Option<Self> {
        let Some(pawn_descriptor) = find_descriptor(descriptors, entity_class) else {
            log::warn!(
                "[Net] local pawn entity_class `{entity_class}` not registered; client weapon \
                 prediction stays inert for this pawn"
            );
            return None;
        };
        let default_weapon = pawn_descriptor.default_weapon.as_deref()?;
        let Some(weapon_descriptor) = find_descriptor(descriptors, default_weapon) else {
            log::warn!(
                "[Net] local pawn defaultWeapon `{default_weapon}` not registered; client weapon \
                 prediction stays inert for this pawn"
            );
            return None;
        };
        let Some(weapon) = weapon_descriptor.weapon.as_ref() else {
            log::warn!(
                "[Net] local pawn defaultWeapon `{default_weapon}` has no weapon component; \
                 client weapon prediction stays inert for this pawn"
            );
            return None;
        };

        Some(Self {
            pawn,
            cooldown_remaining_ms: 0.0,
            cooldown_ms: weapon.cooldown_ms,
            cooldown_authority_generation: 0,
            fire_mode: weapon.fire_mode,
            resolution: weapon.resolution,
            range: weapon.range,
            shoot_press_consumed: false,
        })
    }
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
}

impl ClientPredictedShots {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn clear(&mut self) {
        self.shots.clear();
    }

    pub(crate) fn predict(
        &mut self,
        shot_id: u64,
        resolution: &ClientFireResolution,
        cooldown_before_ms: f32,
        cooldown_after_ms: f32,
        cooldown_authority_generation: u64,
    ) {
        self.shots.insert(
            shot_id,
            PredictedShotRecord {
                shot_id,
                client_tick: resolution.client_tick,
                cooldown_before_ms,
                cooldown_after_ms,
                cooldown_authority_generation,
                muzzle_fx_visible: true,
                hitmarker_visible: !resolution.hits.is_empty(),
                status: PredictedShotStatus::Pending,
            },
        );
    }

    pub(crate) fn reconcile_cooldown(
        state: &mut ClientWeaponState,
        authoritative_cooldown_ms: f32,
    ) {
        if authoritative_cooldown_ms.is_finite() {
            state.cooldown_remaining_ms = authoritative_cooldown_ms.max(0.0);
            state.cooldown_authority_generation =
                state.cooldown_authority_generation.wrapping_add(1);
        }
    }

    pub(crate) fn apply_verdict(
        &mut self,
        state: &mut ClientWeaponState,
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
            if state.cooldown_authority_generation == record.cooldown_authority_generation {
                state.cooldown_remaining_ms = record.cooldown_before_ms.max(0.0);
            }
            record.muzzle_fx_visible = false;
            record.hitmarker_visible = false;
            record.status = PredictedShotStatus::Rejected;
        }
        self.shots.remove(&shot_id)
    }

    #[cfg(test)]
    fn get(&self, shot_id: u64) -> Option<&PredictedShotRecord> {
        self.shots.get(&shot_id)
    }
}

// Not `Copy`: `zone: Option<String>` carries a heap-backed tag for skeletal
// hit-zone hits, so `WeaponImpact` (and `WeaponFireEvents`, which embeds it)
// move/borrow rather than copy. Audited call sites: `fire_hitscan` constructs it
// (the sole literal site, production), and the sim weapon stage borrows
// `events.impact` rather than copying it out.
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

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct WeaponFireEvents {
    pub(crate) activate: Option<WeaponActivation>,
    pub(crate) impact: Option<WeaponImpact>,
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
        if self.impact.is_some() {
            names.push("impact");
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
    tick_dt: f32,
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
        tick_dt,
        false,
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
    tick_dt: f32,
    reload_started_this_tick: bool,
) -> WeaponFireEvents {
    let Some(weapon_id) = active_wieldable else {
        return WeaponFireEvents::default();
    };

    let Ok(existing) = registry.get_component::<WeaponComponent>(weapon_id) else {
        return WeaponFireEvents::default();
    };
    let mut weapon = existing.clone();

    let events = tick_resolved_component(
        registry,
        &mut weapon,
        command,
        collision_world,
        hit_zone_store,
        anim_time,
        tick_dt,
        reload_started_this_tick,
    );

    let _ = registry.set_component(weapon_id, weapon);
    events
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn tick_resolved_component(
    registry: &EntityRegistry,
    weapon: &mut WeaponComponent,
    command: &WeaponFireCommand,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    tick_dt: f32,
    reload_started_this_tick: bool,
) -> WeaponFireEvents {
    let stats = weapon.effective();
    let damage = stats.damage;
    let range = stats.range;
    let resolution = stats.resolution;
    let fire_mode = stats.fire_mode;
    let cooldown_ms = stats.cooldown_ms;
    let cost_per_shot = stats.ammo.as_ref().map(|ammo| ammo.cost_per_shot);
    let fire = apply_weapon_fire_state(
        weapon,
        command,
        fire_mode,
        cooldown_ms,
        cost_per_shot,
        tick_dt,
        reload_started_this_tick,
    );
    match fire {
        WeaponFireAuthorization::Accepted => fire_hitscan(
            command.aim_origin,
            command.aim_direction,
            collision_world,
            registry,
            hit_zone_store,
            anim_time,
            damage,
            range,
            resolution,
        ),
        WeaponFireAuthorization::Empty => WeaponFireEvents {
            dry_fire: true,
            ..WeaponFireEvents::default()
        },
        WeaponFireAuthorization::Rejected => WeaponFireEvents::default(),
    }
}

#[cfg(test)]
pub(crate) fn tick_state_only(
    registry: &mut EntityRegistry,
    active_wieldable: Option<EntityId>,
    command: &WeaponFireCommand,
    tick_dt: f32,
    reload_started_this_tick: bool,
) -> WeaponFireAuthorization {
    let Some(weapon_id) = active_wieldable else {
        return WeaponFireAuthorization::Rejected;
    };

    let Ok(existing) = registry.get_component::<WeaponComponent>(weapon_id) else {
        return WeaponFireAuthorization::Rejected;
    };
    let mut weapon = existing.clone();
    let result = tick_state_only_component(&mut weapon, command, tick_dt, reload_started_this_tick);
    let _ = registry.set_component(weapon_id, weapon);
    result
}

pub(crate) fn tick_state_only_component(
    weapon: &mut WeaponComponent,
    command: &WeaponFireCommand,
    tick_dt: f32,
    reload_started_this_tick: bool,
) -> WeaponFireAuthorization {
    let stats = weapon.effective();
    let fire_mode = stats.fire_mode;
    let cooldown_ms = stats.cooldown_ms;
    let cost_per_shot = stats.ammo.as_ref().map(|ammo| ammo.cost_per_shot);
    apply_weapon_fire_state(
        weapon,
        command,
        fire_mode,
        cooldown_ms,
        cost_per_shot,
        tick_dt,
        reload_started_this_tick,
    )
}

fn apply_weapon_fire_state(
    weapon: &mut WeaponComponent,
    command: &WeaponFireCommand,
    fire_mode: FireMode,
    cooldown_ms: f32,
    cost_per_shot: Option<u32>,
    tick_dt: f32,
    reload_started_this_tick: bool,
) -> WeaponFireAuthorization {
    let dt_ms = (tick_dt.max(0.0)) * 1000.0;
    weapon.cooldown_remaining_ms = (weapon.cooldown_remaining_ms - dt_ms).max(0.0);

    let wants_fire = match fire_mode {
        FireMode::Semi => command.button.pressed && !weapon.shoot_press_consumed,
        FireMode::Auto => command.button.active,
    };
    if fire_mode == FireMode::Semi && command.button.pressed {
        weapon.shoot_press_consumed = true;
    } else if !command.button.active {
        weapon.shoot_press_consumed = false;
    }

    if !command.can_fire || !wants_fire || weapon.cooldown_remaining_ms > 0.0 {
        return WeaponFireAuthorization::Rejected;
    }

    // Starting a reload owns the entire tick even when its duration is no
    // longer than this tick and the atomic transfer already completed.
    if reload_started_this_tick || weapon.reload_remaining_ms > 0 {
        return WeaponFireAuthorization::Rejected;
    }

    if let Some(cost_per_shot) = cost_per_shot {
        if weapon.magazine < cost_per_shot {
            weapon.cooldown_remaining_ms = cooldown_ms;
            return WeaponFireAuthorization::Empty;
        }
        weapon.magazine -= cost_per_shot;
    }

    weapon.cooldown_remaining_ms = cooldown_ms;
    WeaponFireAuthorization::Accepted
}

#[allow(clippy::too_many_arguments)] // weapon fire genuinely needs all of these inputs.
fn fire_hitscan(
    origin: Vec3,
    direction: Vec3,
    collision_world: &CollisionWorld,
    registry: &EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    damage: f32,
    range: f32,
    resolution: ResolutionMode,
) -> WeaponFireEvents {
    let mut events = WeaponFireEvents {
        activate: Some(WeaponActivation { origin, direction }),
        impact: None,
        dry_fire: false,
    };

    match resolution {
        ResolutionMode::Hitscan => {
            let impact = match resolve_nearest_hit(
                origin,
                direction,
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
                None => return events,
            };
            events.impact = Some(impact);
        }
    }

    events
}

#[allow(clippy::too_many_arguments)] // mirrors the host/single-player hitscan inputs.
pub(crate) fn resolve_client_fire(
    state: &mut ClientWeaponState,
    button: FireButtonState,
    aim_origin: Vec3,
    aim_direction: Vec3,
    client_tick: u32,
    collision_world: &CollisionWorld,
    registry: &EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    frame_dt: f32,
) -> Option<ClientFireResolution> {
    if !advance_client_fire_state(state, button, frame_dt) {
        return None;
    }

    state.cooldown_remaining_ms = state.cooldown_ms;
    let hits = resolve_client_hitscan(
        aim_origin,
        aim_direction,
        collision_world,
        registry,
        hit_zone_store,
        anim_time,
        state.range,
        state.resolution,
    );
    Some(ClientFireResolution { client_tick, hits })
}

pub(crate) fn advance_client_fire_state(
    state: &mut ClientWeaponState,
    button: FireButtonState,
    frame_dt: f32,
) -> bool {
    let dt_ms = (frame_dt.max(0.0)) * 1000.0;
    state.cooldown_remaining_ms = (state.cooldown_remaining_ms - dt_ms).max(0.0);

    let wants_fire = match state.fire_mode {
        FireMode::Semi => button.pressed && !state.shoot_press_consumed,
        FireMode::Auto => button.active,
    };
    if state.fire_mode == FireMode::Semi && button.pressed {
        state.shoot_press_consumed = true;
    } else if !button.active {
        state.shoot_press_consumed = false;
    }

    if !wants_fire || state.cooldown_remaining_ms > 0.0 {
        return false;
    }
    true
}

#[allow(clippy::too_many_arguments)] // mirrors the local fire query inputs without a throwaway struct.
fn resolve_client_hitscan(
    origin: Vec3,
    direction: Vec3,
    collision_world: &CollisionWorld,
    registry: &EntityRegistry,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    range: f32,
    resolution: ResolutionMode,
) -> Vec<LocalHitRecord> {
    match resolution {
        ResolutionMode::Hitscan => match resolve_nearest_hit(
            origin,
            direction,
            collision_world,
            registry,
            hit_zone_store,
            anim_time,
            range,
        ) {
            // Only an entity hit produces a local hit record; a nearer world hit
            // (or no hit) yields none — the client owns no world-impact record.
            Some(NearestHit::Entity(entity)) => vec![local_hit_record(entity)],
            _ => Vec::new(),
        },
    }
}

fn local_hit_record(entity: EntityRayHit) -> LocalHitRecord {
    LocalHitRecord {
        target: entity.target,
        point: entity.point,
        zone: entity.zone,
    }
}

#[cfg(test)]
fn find_descriptor<'a>(
    descriptors: &'a [EntityTypeDescriptor],
    name: &str,
) -> Option<&'a EntityTypeDescriptor> {
    descriptors
        .iter()
        .find(|desc| desc.canonical_name.as_deref() == Some(name))
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
    let entity_hit = nearest_entity_hit(
        registry,
        hit_zone_store,
        anim_time,
        origin,
        direction,
        range,
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
mod tests {
    use super::*;
    use crate::input::{Binding, InputSystem, PhysicalInput};
    use parry3d::math::Isometry;
    use parry3d::shape::TriMesh;
    use postretro_entities::components::health::{HealthComponent, Hitbox};
    use postretro_entities::registry::{ComponentKind, Transform};
    use postretro_entities::{AmmoReserve, EntityTypeDescriptor, MeshDescriptor};
    use postretro_foundation::{AmmoResource, WeaponDescriptor, WeaponResource};
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

    fn weapon_component(fire_mode: FireMode, cooldown_ms: f32) -> WeaponComponent {
        WeaponComponent::from_descriptor(&WeaponDescriptor {
            damage: 25.0,
            range: 10.0,
            cooldown_ms,
            fire_mode,
            resolution: ResolutionMode::Hitscan,
            credit_source: None,
            third_person_model: None,
            viewmodel: None,
            resource: None,
        })
    }

    fn ammo_weapon_component(
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
        }));
        WeaponComponent::from_descriptor(&descriptor)
    }

    fn weapon_descriptor(fire_mode: FireMode, cooldown_ms: f32) -> WeaponDescriptor {
        WeaponDescriptor {
            damage: 25.0,
            range: 10.0,
            cooldown_ms,
            fire_mode,
            resolution: ResolutionMode::Hitscan,
            credit_source: None,
            third_person_model: None,
            viewmodel: None,
            resource: None,
        }
    }

    fn descriptor_table(default_weapon: Option<&str>) -> Vec<EntityTypeDescriptor> {
        vec![
            EntityTypeDescriptor {
                canonical_name: Some("player".to_string()),
                default_weapon: default_weapon.map(str::to_string),
                light: None,
                emitter: None,
                movement: None,
                weapon: None,
                mesh: None,
                health: None,
                behavior: None,
            },
            EntityTypeDescriptor {
                canonical_name: Some("pistol".to_string()),
                default_weapon: None,
                light: None,
                emitter: None,
                movement: None,
                weapon: Some(weapon_descriptor(FireMode::Semi, 100.0)),
                mesh: None::<MeshDescriptor>,
                health: None,
                behavior: None,
            },
        ]
    }

    /// Run a weapon `tick` with an EMPTY hit-zone store and a zero animation
    /// clock — the no-skeletal-zones configuration, so these tests exercise the
    /// authored-AABB path exactly as before the facility landed (byte-identical
    /// behavior: an empty store routes every health+hitbox entity through the
    /// AABB narrow phase). Keeps the existing test bodies a one-word rename.
    fn fire_tick(
        registry: &mut EntityRegistry,
        active_wieldable: Option<EntityId>,
        snapshot: &ActionSnapshot,
        camera: &Camera,
        world: &CollisionWorld,
        tick_dt: f32,
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
            tick_dt,
        )
    }

    fn spawn_weapon(registry: &mut EntityRegistry, component: WeaponComponent) -> EntityId {
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

    fn input_system() -> InputSystem {
        InputSystem::new(vec![Binding::new(
            PhysicalInput::MouseButton(MouseButton::Left),
            Action::Shoot,
        )])
    }

    fn shoot_snapshot(input: &mut InputSystem, active: bool) -> ActionSnapshot {
        input.set_physical_input(PhysicalInput::MouseButton(MouseButton::Left), active);
        input.snapshot()
    }

    fn wall_world() -> CollisionWorld {
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
    fn client_weapon_state_seeds_from_local_pawn_default_weapon() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let state = ClientWeaponState::from_local_pawn_descriptor(
            pawn,
            "player",
            &descriptor_table(Some("pistol")),
        )
        .expect("player default weapon resolves");

        assert_eq!(state.pawn, pawn);
        assert_eq!(state.cooldown_remaining_ms, 0.0);
        assert_eq!(state.cooldown_ms, 100.0);
        assert_eq!(state.fire_mode, FireMode::Semi);
        assert_eq!(state.resolution, ResolutionMode::Hitscan);
        assert_eq!(state.range, 10.0);
    }

    #[test]
    fn client_weapon_state_none_for_weaponless_pawn() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());

        assert!(
            ClientWeaponState::from_local_pawn_descriptor(pawn, "player", &descriptor_table(None))
                .is_none()
        );
    }

    // Regression: a host retune from default_weapon Some to None left the
    // previously seeded client weapon prediction carrier active.
    #[test]
    fn client_weapon_state_clears_stale_state_when_host_tuning_is_none() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let tuning = crate::netcode::DefaultWeaponFirePayload {
            range: 10.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
        };
        let mut state = Some(ClientWeaponState::from_host_tuning(pawn, &tuning));

        let preserve_prediction_history =
            ClientWeaponState::sync_from_host_tuning(&mut state, Some(pawn), None);

        assert!(state.is_none());
        assert!(!preserve_prediction_history);
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
        let pawn = registry.spawn(Transform::default());
        let mut state = ClientWeaponState {
            pawn,
            cooldown_remaining_ms: 0.0,
            cooldown_ms: 100.0,
            cooldown_authority_generation: 0,
            fire_mode: FireMode::Auto,
            resolution: ResolutionMode::Hitscan,
            range: 10.0,
            shoot_press_consumed: false,
        };
        let world = CollisionWorld::new();
        let store = HitZoneStore::new();
        let button = FireButtonState {
            pressed: true,
            active: true,
        };

        let first = resolve_client_fire(
            &mut state,
            button,
            Vec3::ZERO,
            Vec3::NEG_Z,
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

        let blocked = resolve_client_fire(
            &mut state,
            FireButtonState {
                pressed: false,
                active: true,
            },
            Vec3::ZERO,
            Vec3::NEG_Z,
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
    fn semi_weapon_fires_once_per_press() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
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
        assert_eq!(events.event_names(), vec!["activate"]);

        let same_pressed_snapshot = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            0.2,
        );
        assert!(same_pressed_snapshot.event_names().is_empty());

        let held = shoot_snapshot(&mut input, true);
        let events = fire_tick(&mut registry, Some(weapon_id), &held, &camera, &world, 0.2);
        assert!(events.event_names().is_empty());

        let _released = shoot_snapshot(&mut input, false);
        let inactive = shoot_snapshot(&mut input, false);
        let _ = fire_tick(
            &mut registry,
            Some(weapon_id),
            &inactive,
            &camera,
            &world,
            0.2,
        );

        let pressed_again = shoot_snapshot(&mut input, true);
        let events = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed_again,
            &camera,
            &world,
            1.0 / 60.0,
        );
        assert_eq!(events.event_names(), vec!["activate"]);
    }

    #[test]
    fn auto_weapon_fires_repeatedly_when_held_after_cooldown() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Auto, 30.0));
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let world = CollisionWorld::new();
        let mut input = input_system();

        let pressed = shoot_snapshot(&mut input, true);
        let first = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &world,
            0.016,
        );
        assert_eq!(first.event_names(), vec!["activate"]);

        let held = shoot_snapshot(&mut input, true);
        let blocked = fire_tick(
            &mut registry,
            Some(weapon_id),
            &held,
            &camera,
            &world,
            0.016,
        );
        assert!(blocked.event_names().is_empty());

        let still_held = shoot_snapshot(&mut input, true);
        let second = fire_tick(
            &mut registry,
            Some(weapon_id),
            &still_held,
            &camera,
            &world,
            0.016,
        );
        assert_eq!(second.event_names(), vec!["activate"]);
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
        let impact = events.impact.expect("world hit should emit impact");
        assert_vec3_approx(impact.point, Vec3::new(0.0, 0.0, -5.0));
        assert_vec3_approx(impact.normal, Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(
            impact.outcome,
            ActivationOutcome::Hit(DamagePayload { amount: 25.0 })
        );
    }

    #[test]
    fn open_space_shot_consumes_cooldown_without_impact() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
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

        assert_eq!(events.event_names(), vec!["activate"]);
        assert!(events.impact.is_none());
        let weapon = registry
            .get_component::<WeaponComponent>(weapon_id)
            .expect("weapon component should still exist");
        assert!(approx_eq(weapon.cooldown_remaining_ms, 100.0));
    }

    #[test]
    fn ammo_shot_consumes_effective_cost_once_and_resolves_normally() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(
            &mut registry,
            ammo_weapon_component(FireMode::Semi, 100.0, 12, 2),
        );
        let pawn = registry.spawn(Transform::default());
        let mut reserve = AmmoReserve::new();
        reserve.credit("bullets.light", 48);
        registry.set_component(pawn, reserve).unwrap();
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
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .magazine,
            10
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            48
        );
    }

    #[test]
    fn ammo_shot_spends_cost_on_open_space_miss() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(
            &mut registry,
            ammo_weapon_component(FireMode::Semi, 100.0, 12, 2),
        );
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &CollisionWorld::new(),
            1.0 / 60.0,
        );

        assert_eq!(events.event_names(), vec!["activate"]);
        assert!(events.impact.is_none());
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .magazine,
            10
        );
    }

    #[test]
    fn below_cost_is_empty_at_state_seam_and_emits_only_dry_fire() {
        let command = WeaponFireCommand {
            button: FireButtonState {
                pressed: true,
                active: true,
            },
            aim_origin: Vec3::ZERO,
            aim_direction: Vec3::NEG_Z,
            can_fire: true,
        };
        let mut component = ammo_weapon_component(FireMode::Semi, 100.0, 2, 3);
        let stats = component.effective();
        let fire_mode = stats.fire_mode;
        let cooldown_ms = stats.cooldown_ms;
        let cost_per_shot = stats.ammo.as_ref().map(|ammo| ammo.cost_per_shot);
        assert_eq!(
            apply_weapon_fire_state(
                &mut component,
                &command,
                fire_mode,
                cooldown_ms,
                cost_per_shot,
                1.0 / 60.0,
                false,
            ),
            WeaponFireAuthorization::Empty
        );
        assert_eq!(component.magazine, 2);
        assert!(approx_eq(component.cooldown_remaining_ms, 100.0));

        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(
            &mut registry,
            ammo_weapon_component(FireMode::Semi, 100.0, 2, 3),
        );
        let target = spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let before_health = registry
            .get_component::<HealthComponent>(target)
            .unwrap()
            .current;
        let events = tick_resolved(
            &mut registry,
            Some(weapon_id),
            &command,
            &CollisionWorld::new(),
            &HitZoneStore::new(),
            0.0,
            1.0 / 60.0,
            false,
        );

        assert_eq!(events.event_names(), vec!["dry_fire"]);
        assert!(events.activate.is_none());
        assert!(events.impact.is_none());
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .magazine,
            2
        );
        assert_eq!(
            registry
                .get_component::<HealthComponent>(target)
                .unwrap()
                .current,
            before_health
        );
    }

    // Regression: a held Auto trigger emitted dry_fire on every fixed tick.
    #[test]
    fn empty_auto_weapon_emits_once_per_fire_interval() {
        let registry = EntityRegistry::new();
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut weapon = ammo_weapon_component(FireMode::Auto, 100.0, 1, 1);
        weapon.magazine = 0;
        let pressed = WeaponFireCommand {
            button: FireButtonState {
                pressed: true,
                active: true,
            },
            aim_origin: Vec3::ZERO,
            aim_direction: Vec3::NEG_Z,
            can_fire: true,
        };

        let first = tick_resolved_component(
            &registry,
            &mut weapon,
            &pressed,
            &world,
            &hit_zones,
            0.0,
            0.0,
            false,
        );
        assert_eq!(first.event_names(), vec!["dry_fire"]);
        assert!(approx_eq(weapon.cooldown_remaining_ms, 100.0));
        assert_eq!(weapon.magazine, 0);

        let held = WeaponFireCommand {
            button: FireButtonState {
                pressed: false,
                active: true,
            },
            ..pressed
        };
        let cooling = tick_resolved_component(
            &registry,
            &mut weapon,
            &held,
            &world,
            &hit_zones,
            0.0,
            0.04,
            false,
        );
        assert!(cooling.event_names().is_empty());
        assert!(approx_eq(weapon.cooldown_remaining_ms, 60.0));

        let ready = tick_resolved_component(
            &registry,
            &mut weapon,
            &held,
            &world,
            &hit_zones,
            0.0,
            0.061,
            false,
        );
        assert_eq!(ready.event_names(), vec!["dry_fire"]);
        assert!(ready.activate.is_none());
        assert!(ready.impact.is_none());
        assert!(approx_eq(weapon.cooldown_remaining_ms, 100.0));
        assert_eq!(weapon.magazine, 0);
    }

    #[test]
    fn reload_in_flight_silently_blocks_without_cancelling_or_spending() {
        let mut registry = EntityRegistry::new();
        let mut component = ammo_weapon_component(FireMode::Semi, 100.0, 12, 2);
        component.reload_remaining_ms = 450;
        component.reload_total_ms = 900;
        let weapon_id = spawn_weapon(&mut registry, component);
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
        let weapon = registry
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap();

        assert!(events.event_names().is_empty());
        assert_eq!(weapon.magazine, 12);
        assert_eq!(weapon.reload_remaining_ms, 450);
        assert_eq!(weapon.reload_total_ms, 900);
        assert_eq!(weapon.cooldown_remaining_ms, 0.0);
    }

    #[test]
    fn resourceless_weapon_fires_without_magazine_gating_or_consumption() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
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

        assert_eq!(events.event_names(), vec!["activate"]);
        let weapon = registry
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap();
        assert!(weapon.ammo.is_none());
        assert_eq!(weapon.magazine, 0);
    }

    #[test]
    fn state_only_fire_advances_cooldown_without_hitscan_events() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let command = WeaponFireCommand {
            button: FireButtonState {
                pressed: true,
                active: true,
            },
            aim_origin: Vec3::ZERO,
            aim_direction: Vec3::NEG_Z,
            can_fire: true,
        };

        let result = tick_state_only(&mut registry, Some(weapon_id), &command, 1.0 / 60.0, false);

        assert_eq!(result, WeaponFireAuthorization::Accepted);
        let weapon = registry
            .get_component::<WeaponComponent>(weapon_id)
            .expect("weapon component should still exist");
        assert!(approx_eq(weapon.cooldown_remaining_ms, 100.0));
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

        let impact = events.impact.expect("entity hit should emit impact");
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

        let impact = events.impact.expect("wall hit should emit impact");
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

        let impact = events.impact.expect("entity hit should emit impact");
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
            events.impact.is_none(),
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

        let impact = events.impact.expect("wall hit should emit impact");
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

        let impact = events.impact.expect("downed target should emit impact");
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
    fn fire_tick_with(
        registry: &mut EntityRegistry,
        active_wieldable: Option<EntityId>,
        snapshot: &ActionSnapshot,
        camera: &Camera,
        world: &CollisionWorld,
        store: &HitZoneStore,
        anim_time: f64,
        tick_dt: f32,
    ) -> WeaponFireEvents {
        tick(
            registry,
            active_wieldable,
            snapshot,
            camera,
            world,
            store,
            anim_time,
            tick_dt,
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
        let pawn = registry.spawn(Transform::default());
        let mut state = ClientWeaponState {
            pawn,
            cooldown_remaining_ms: 0.0,
            cooldown_ms: 100.0,
            cooldown_authority_generation: 0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            range: 10.0,
            shoot_press_consumed: false,
        };

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
            &mut state,
            FireButtonState {
                pressed: true,
                active: true,
            },
            Vec3::ZERO,
            Vec3::NEG_Z,
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

        let impact = events.impact.expect("zone hit should emit impact");
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

        let impact = events.impact.expect("wall hit should emit impact");
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
        let direct = nearest_entity_hit(&registry, &store, 0.0, origin, direction, 10.0)
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
        let impact = events.impact.expect("weapon reports the entity hit");

        assert_eq!(Some(direct.target), impact.target, "same target");
        assert_eq!(direct.zone, impact.zone, "same zone tag");
        assert_vec3_approx(direct.point, impact.point);
        assert_eq!(direct.target, target);
    }

    fn client_weapon_state() -> ClientWeaponState {
        ClientWeaponState {
            pawn: EntityId::from_raw(1),
            cooldown_remaining_ms: 0.0,
            cooldown_ms: 100.0,
            cooldown_authority_generation: 0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            range: 10.0,
            shoot_press_consumed: false,
        }
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
        };
        let mut predicted = ClientPredictedShots::new();

        predicted.predict(0xA, &resolution, 0.0, 100.0, 0);

        let record = predicted.get(0xA).expect("shot should be recorded");
        assert_eq!(record.client_tick, 9);
        assert!(record.muzzle_fx_visible);
        assert!(record.hitmarker_visible);
        assert_eq!(record.status, PredictedShotStatus::Pending);
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
        };
        let mut state = client_weapon_state();
        state.cooldown_remaining_ms = 100.0;
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(
            0xA,
            &resolution,
            0.0,
            100.0,
            state.cooldown_authority_generation,
        );

        let record = predicted
            .apply_verdict(&mut state, 0xA, true, true)
            .expect("verdict should match a predicted shot");

        assert!(record.muzzle_fx_visible);
        assert!(record.hitmarker_visible);
        assert_eq!(record.status, PredictedShotStatus::Accepted);
        assert!(approx_eq(state.cooldown_remaining_ms, 100.0));
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
        };
        let mut state = client_weapon_state();
        state.cooldown_remaining_ms = 100.0;
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(
            0xA,
            &resolution,
            25.0,
            100.0,
            state.cooldown_authority_generation,
        );

        let record = predicted
            .apply_verdict(&mut state, 0xA, true, false)
            .expect("verdict should match a predicted shot");

        assert!(record.muzzle_fx_visible);
        assert!(!record.hitmarker_visible);
        assert_eq!(record.status, PredictedShotStatus::Accepted);
        assert!(approx_eq(state.cooldown_remaining_ms, 100.0));
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
        };
        let mut state = client_weapon_state();
        state.cooldown_remaining_ms = 100.0;
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(
            0xA,
            &resolution,
            25.0,
            100.0,
            state.cooldown_authority_generation,
        );

        let record = predicted
            .apply_verdict(&mut state, 0xA, false, false)
            .expect("verdict should match a predicted shot");

        assert!(!record.muzzle_fx_visible);
        assert!(!record.hitmarker_visible);
        assert_eq!(record.status, PredictedShotStatus::Rejected);
        assert!(approx_eq(state.cooldown_remaining_ms, 25.0));
        assert!(
            predicted.get(0xA).is_none(),
            "a terminal verdict prunes the record"
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
        };
        let mut state = client_weapon_state();
        state.cooldown_remaining_ms = 100.0;
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(
            0xA,
            &resolution,
            25.0,
            100.0,
            state.cooldown_authority_generation,
        );

        let accepted = predicted
            .apply_verdict(&mut state, 0xA, true, true)
            .expect("accept should match");
        assert_eq!(accepted.status, PredictedShotStatus::Accepted);
        assert!(accepted.muzzle_fx_visible);
        assert!(accepted.hitmarker_visible);

        // The terminal accept pruned the record, so a late reject finds nothing
        // and cannot undo the accepted shot's cooldown or presentation.
        assert!(
            predicted
                .apply_verdict(&mut state, 0xA, false, false)
                .is_none()
        );
        assert!(predicted.get(0xA).is_none());
        assert!(approx_eq(state.cooldown_remaining_ms, 100.0));
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
        };
        let mut state = client_weapon_state();
        state.cooldown_remaining_ms = 100.0;
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(
            0xA,
            &resolution,
            25.0,
            100.0,
            state.cooldown_authority_generation,
        );

        ClientPredictedShots::reconcile_cooldown(&mut state, 12.0);
        let record = predicted
            .apply_verdict(&mut state, 0xA, false, false)
            .expect("reject should match");

        assert_eq!(record.status, PredictedShotStatus::Rejected);
        assert!(!record.muzzle_fx_visible);
        assert!(!record.hitmarker_visible);
        assert!(
            approx_eq(state.cooldown_remaining_ms, 12.0),
            "fresh owner-private cooldown must win over stale rollback"
        );
        assert!(
            predicted.get(0xA).is_none(),
            "a terminal verdict prunes the record"
        );
    }

    #[test]
    fn owner_private_cooldown_reconciles_client_weapon_state() {
        let mut state = client_weapon_state();
        state.cooldown_remaining_ms = 100.0;

        ClientPredictedShots::reconcile_cooldown(&mut state, 42.0);

        assert!(approx_eq(state.cooldown_remaining_ms, 42.0));
        assert_eq!(state.cooldown_authority_generation, 1);
    }
}
