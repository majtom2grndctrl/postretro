// Weapon fire tick, hitscan/local hit resolution, and client fire prediction: owns fire commands, local hit records, and predicted-shot reconciliation state.
// See: context/lib/entity_model.md §5, §7

#[cfg(test)]
use postretro_foundation::FireMode;
use std::collections::HashMap;

use glam::Vec3;
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::components::weapon::{UNKNOWN_WEAPON_CREDIT_SOURCE, WeaponComponent};
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::registry::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_entities::{Emitter, ImpactContact, WeaponEmission};
use postretro_foundation::{
    KnockbackDescriptor, ProjectileDescriptor, ResolutionMode, SplashDescriptor,
    WeaponPlacementDescriptor,
};

use crate::collision::{CollisionWorld, cast_ray, cast_sphere_exact};
#[cfg(test)]
use crate::scripting_systems::hit_zones::nearest_entity_hit;
use crate::scripting_systems::hit_zones::{
    EntityRayHit, HitZoneStore, nearest_entity_hit_ignoring,
};

pub mod activation_prediction;
pub mod execution;
#[cfg(test)]
mod execution_tests;
mod shot;
pub use shot::{ResolvedWeaponShot, freeze_weapon_shot};
mod client_resolution;
pub use client_resolution::resolve_client_shot;
#[cfg(any(test, feature = "test-support"))]
pub use client_resolution::resolve_test_client_shot;
mod client_pull;
mod damage;
mod impact;
pub mod spread;

pub use client_pull::{
    ClientPullEffects, ClientPullPresentation, ClientShotDeclaration, ReplicatedWeaponProjection,
    SlotSample, client_pull_effects, client_pull_presentation, client_shot_presentation,
};

pub use damage::DamagePayload;
pub use impact::{
    collection_id as impact_collection_id, emissive as impact_emissive,
    spec_exponent as impact_spec_exponent, spec_intensity as impact_spec_intensity,
    sprite_collection as impact_sprite_collection,
};
pub use impact::{
    lifetime as impact_lifetime, spawn_impact_effect_at, spawn_projectile_impact_light,
};

/// Minimal weapon construction used by binary-owned gameplay orchestration tests.
#[cfg(feature = "test-support")]
pub mod test_fixtures {
    use postretro_entities::components::weapon::WeaponComponent;
    use postretro_foundation::{ActivationTrigger, ResolutionMode, WeaponDescriptor};

    pub fn weapon_component(trigger: ActivationTrigger, cooldown_ms: f32) -> WeaponComponent {
        WeaponComponent::from_descriptor(&WeaponDescriptor {
            sounds: None,
            knockback: None,
            damage: 25.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            bloom_per_shot_degrees: 0.0,
            bloom_max_degrees: 0.0,
            bloom_decay_degrees_per_second: 0.0,
            bloom_decay_delay_ms: 0.0,
            movement_spread_degrees: 0.0,
            spread_vertical_bias: 0.0,
            range: 10.0,
            primary: postretro_foundation::WeaponActivationDescriptor::single(trigger, cooldown_ms),
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
        })
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActivationOutcome {
    Hit(DamagePayload),
    Effect,
    Spawned(EntityId),
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeaponActivation {
    pub origin: Vec3,
    pub direction: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FireButtonState {
    pub pressed: bool,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeaponFireCommand {
    pub button: FireButtonState,
    pub aim_origin: Vec3,
    pub aim_direction: Vec3,
    pub can_fire: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalHitRecord {
    pub target: EntityId,
    pub point: Vec3,
    pub zone: Option<String>,
    /// Surface normal at `point`, declared so the host holds the same contact.
    pub normal: Vec3,
}

/// A predicted pellet that struck world geometry. It has no damage target; it
/// is declared as a presentation-only contact and plays its impact locally.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldContact {
    pub point: Vec3,
    pub normal: Vec3,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientFireResolution {
    pub client_tick: u32,
    /// Predicted entity hits; these alone drive the hitmarker and damage claims.
    pub hits: Vec<LocalHitRecord>,
    /// Predicted world contacts, declared alongside `hits` so the host sees
    /// every contact of the shot.
    pub world_contacts: Vec<WorldContact>,
    /// A projectile launch is deferred to the connected client's post-loop
    /// presentation path. It must not produce a same-frame hit declaration.
    pub projectile_launch: Option<ProjectileLaunch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictedShotStatus {
    Pending,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PredictedShotRecord {
    pub(crate) shot_id: postretro_foundation::ShotId,
    pub(crate) client_tick: u32,
    pub(crate) weapon: EntityId,
    pub(crate) cooldown_before_ms: f32,
    pub(crate) cooldown_after_ms: f32,
    pub(crate) cooldown_authority_generation: u64,
    /// Presentation bookkeeping: whether this shot showed its muzzle FX. Only
    /// a [`ClientPullPresentation::Fire`] does. A rejecting verdict clears it;
    /// nothing raises it after the pull.
    pub(crate) muzzle_fx_visible: bool,
    /// Presentation bookkeeping: whether this shot shows a hit. Only a fire
    /// that predicted an entity hit, or a fired projectile that later struck
    /// one, marks it; a dry or silent pull never does, even when its
    /// declaration carries a hit. The verdict only retracts it.
    pub(crate) hitmarker_visible: bool,
    pub(crate) status: PredictedShotStatus,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ClientPredictedShots {
    shots: HashMap<postretro_foundation::ShotId, PredictedShotRecord>,
    cooldown_authority_generation: HashMap<EntityId, u64>,
}

impl ClientPredictedShots {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.shots.clear();
        self.cooldown_authority_generation.clear();
    }

    /// Record a predicted shot for reconcile. Every shot the fire gate passes
    /// is predicted, whatever it presents; only a [`ClientPullPresentation::Fire`]
    /// shows its muzzle FX or a hitmarker. A dry click on an enemy marks no hit
    /// before the verdict, since it presented no shot.
    pub fn predict(
        &mut self,
        shot_id: postretro_foundation::ShotId,
        weapon: EntityId,
        resolution: &ClientFireResolution,
        cooldown_before_ms: f32,
        cooldown_after_ms: f32,
        presentation: ClientPullPresentation,
    ) {
        let shows_fire = presentation == ClientPullPresentation::Fire;
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
                muzzle_fx_visible: shows_fire,
                hitmarker_visible: shows_fire && !resolution.hits.is_empty(),
                status: PredictedShotStatus::Pending,
            },
        );
    }

    pub fn reconcile_cooldown(
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

    pub fn apply_verdict(
        &mut self,
        registry: &mut EntityRegistry,
        shot_id: postretro_foundation::ShotId,
        fire_accepted: bool,
        hit_accepted: bool,
    ) -> Option<PredictedShotRecord> {
        if !fire_accepted {
            // HIT refusal can already have retired the cosmetic record. The
            // flight still belongs to this exact shot and must obey real FIRE
            // denial independently of that bookkeeping.
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
        // A per-shot verdict is terminal: the record is reconciled exactly once,
        // so a stored record is always `Pending`. Apply the rollback effects,
        // then prune it — the map would otherwise grow unbounded across a session
        // (unlike the age-pruned host mirror). A duplicate or late verdict finds
        // nothing and changes no cosmetic record or recovery.
        let record = self.shots.get_mut(&shot_id)?;
        if fire_accepted {
            record.status = PredictedShotStatus::Accepted;
            record.hitmarker_visible &= hit_accepted;
        } else {
            // Recovery is correlated by activation token and captured instance.
            // A per-shot verdict retracts cosmetics/flight only: restoring an
            // old pre-shot cooldown could erase a newer activation's recovery.
            record.muzzle_fx_visible = false;
            record.hitmarker_visible = false;
            record.status = PredictedShotStatus::Rejected;
        }
        self.shots.remove(&shot_id)
    }

    /// Pending HIT intake can refuse a declaration before FIRE is decided.
    /// Retire its hit feedback record without deciding FIRE, changing recovery,
    /// or removing a still-live predicted projectile.
    pub fn refuse_hit(
        &mut self,
        shot_id: postretro_foundation::ShotId,
    ) -> Option<PredictedShotRecord> {
        let mut record = self.shots.remove(&shot_id)?;
        record.hitmarker_visible = false;
        Some(record)
    }

    /// A predicted projectile only knows whether it hit after its later
    /// frame-driven sweep. The verdict remains the authority that keeps or
    /// clears this local presentation state.
    pub fn mark_hitmarker(&mut self, shot_id: postretro_foundation::ShotId) {
        if let Some(record) = self.shots.get_mut(&shot_id)
            && record.status == PredictedShotStatus::Pending
        {
            record.hitmarker_visible = true;
        }
    }

    #[cfg(test)]
    fn get(&self, shot_id: postretro_foundation::ShotId) -> Option<&PredictedShotRecord> {
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
pub struct WeaponImpact {
    pub point: Vec3,
    pub normal: Vec3,
    /// The entity struck, when the nearest hit along the ray is an entity
    /// hitbox rather than world geometry. `None` for a world-only hit or when
    /// no targetable entity lies along the ray within range. Spatial targeting
    /// rides here, beside the payload — never inside [`DamagePayload`]. The sim
    /// weapon stage consumes this to route `apply_damage_with_context` before
    /// the death sweep handles zero-HP entities.
    pub target: Option<EntityId>,
    /// The authored skeletal hit-zone tag the shot landed on (e.g. "head"),
    /// surfaced for an entity hit that struck a bone-posed capsule. `None` for a
    /// world hit or an authored-AABB entity hit. The zone-multiplier damage
    /// routing site reads this to scale the payload; here it is only surfaced.
    pub zone: Option<String>,
    pub outcome: ActivationOutcome,
}

/// Immutable fire-time data the mutable weapon stage materializes as an entity.
/// `fire_hitscan` deliberately returns this rather than spawning while it holds
/// only an immutable registry borrow.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectileLaunch {
    pub sounds: Option<std::sync::Arc<postretro_foundation::ActivationSounds>>,
    pub action: Option<std::sync::Arc<postretro_foundation::WeaponActivationDescriptor>>,
    pub shot_id: Option<postretro_foundation::ShotId>,
    pub model_scale: f32,
    pub origin: Vec3,
    pub direction: Vec3,
    pub speed: f32,
    pub radius: f32,
    pub range: f32,
    pub lifetime: f32,
    pub damage: f32,
    pub knockback_impulse: Vec3,
    pub credit_source: String,
    pub descriptor: ProjectileDescriptor,
    /// Impact-composed radial damage snapshot, independent from projectile
    /// travel tuning and retained by the spawned projectile.
    pub splash: Option<SplashDescriptor>,
}

impl ProjectileLaunch {
    /// Construct immutable launch facts for cross-crate simulation harnesses.
    /// Production launch construction stays coupled to weapon resolution.
    #[cfg(any(test, feature = "test-support"))]
    #[allow(clippy::too_many_arguments)]
    pub fn for_test(
        origin: Vec3,
        direction: Vec3,
        speed: f32,
        radius: f32,
        range: f32,
        lifetime: f32,
        damage: f32,
        knockback_impulse: Vec3,
        credit_source: String,
        descriptor: ProjectileDescriptor,
        splash: Option<SplashDescriptor>,
    ) -> Self {
        Self {
            sounds: None,
            action: None,
            shot_id: None,
            model_scale: 1.0,
            origin,
            direction,
            speed,
            radius,
            range,
            lifetime,
            damage,
            knockback_impulse,
            credit_source,
            descriptor,
            splash,
        }
    }
}

const MUZZLE_DIRECTION_EPSILON_SQUARED: f32 = 1.0e-12;

/// Compose a model-local muzzle point through steady viewmodel placement and
/// the gameplay aim basis. Render-rate sway and bob intentionally do not enter
/// this authoritative origin.
pub fn muzzle_world_origin(
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
pub struct WeaponFireEvents {
    pub resolved_shot: Option<ResolvedWeaponShot>,
    pub(crate) activate: Option<WeaponActivation>,
    pub(crate) impacts: Vec<WeaponImpact>,
    pub(crate) projectile_launches: Vec<ProjectileLaunch>,
    /// Filled only by the mutable caller after it materializes a launch intent.
    pub(crate) spawned: Vec<ActivationOutcome>,
    pub(crate) dry_fire: bool,
    /// Set by the machine when this activation's shot latched an overheat.
    pub(crate) overheat: bool,
}

impl ClientFireResolution {
    /// Every predicted contact of the shot, entity and world, in pellet order
    /// by kind. The client's own impact sound resolves against these.
    pub fn impact_contacts(&self) -> Vec<ImpactContact> {
        self.hits
            .iter()
            .map(|hit| ImpactContact::new(hit.point, hit.normal, Some(hit.target)))
            .chain(
                self.world_contacts
                    .iter()
                    .map(|contact| ImpactContact::new(contact.point, contact.normal, None)),
            )
            .collect()
    }
}

impl WeaponFireEvents {
    #[cfg(test)]
    pub fn event_names(&self) -> Vec<&'static str> {
        self.emissions(&Emitter::Contacts(Vec::new()), None)
            .into_iter()
            .map(|emission| emission.address)
            .collect()
    }

    /// This activation's named events, in dispatch order. Fire, dry fire,
    /// spawn and overheat come from `shooter`; one `impact` carries every
    /// contact of the activation. `weapon` names the weapon descriptor the
    /// events came from.
    pub fn emissions(&self, shooter: &Emitter, weapon: Option<String>) -> Vec<WeaponEmission> {
        let from_shooter = |address| WeaponEmission {
            sounds: self.resolved_shot.as_ref().map(|shot| shot.sounds.clone()),
            action: if matches!(address, "activate" | "spawned") {
                self.resolved_shot
                    .as_ref()
                    .map(|shot| shot.action().clone())
            } else {
                None
            },
            shot_id: self
                .resolved_shot
                .as_ref()
                .map(|shot| shot.activation.shot_id),
            address,
            emitter: shooter.clone(),
            weapon: weapon.clone(),
        };
        let mut emissions = Vec::with_capacity(3);
        if self.dry_fire {
            emissions.push(from_shooter("dry_fire"));
        }
        if self.activate.is_some() {
            emissions.push(from_shooter("activate"));
        }
        if !self.impacts.is_empty() {
            emissions.push(WeaponEmission {
                sounds: self.resolved_shot.as_ref().map(|shot| shot.sounds.clone()),
                action: self
                    .resolved_shot
                    .as_ref()
                    .map(|shot| shot.action().clone()),
                shot_id: self
                    .resolved_shot
                    .as_ref()
                    .map(|shot| shot.activation.shot_id),
                address: "impact",
                emitter: Emitter::Contacts(
                    self.impacts
                        .iter()
                        .map(|impact| {
                            ImpactContact::new(impact.point, impact.normal, impact.target)
                        })
                        .collect(),
                ),
                weapon: weapon.clone(),
            });
        }
        if !self.spawned.is_empty() {
            emissions.push(from_shooter("spawned"));
        }
        if self.overheat {
            emissions.push(from_shooter("overheat"));
        }
        emissions
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WeaponFireAuthorization {
    Accepted,
    Rejected,
    Empty,
}

#[allow(clippy::too_many_arguments)] // weapon fire genuinely needs all of these inputs.
#[cfg(test)]
pub fn tick_resolved(
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
pub fn tick_resolved_component(
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
    let knockback = stats.knockback;
    let pellet_count = stats.pellet_count;
    let base_spread_degrees = stats.spread_degrees;
    let range = stats.range;
    let resolution = stats.resolution;
    let projectile = stats.projectile.cloned();
    let splash = stats.splash.cloned();
    let muzzle_offset = stats.muzzle_offset;
    let credit_source = stats.credit_source.to_string();
    match fire {
        WeaponFireAuthorization::Accepted => {
            // A shell position is consumed whether it is a single exact-axis ray
            // or a spread fan, preserving future spread changes' deterministic
            // sequence. Only a resolved shell advances this instance-local state.
            let shell_counter = weapon.shells_fired;
            weapon.shells_fired = weapon.shells_fired.wrapping_add(1);
            let (spread_radians, hitscan_direction) = if resolution == ResolutionMode::Hitscan {
                composed_hitscan_cone(registry, owner_pawn, weapon, command.aim_direction)
            } else {
                (base_spread_degrees.to_radians(), command.aim_direction)
            };
            let (origin, direction) = if resolution == ResolutionMode::Projectile {
                resolve_projectile_launch_pose(
                    owner_pawn,
                    command.aim_origin,
                    command.aim_direction,
                    placement,
                    muzzle_offset,
                    projectile
                        .as_ref()
                        .map_or(0.0, |projectile| projectile.radius),
                    collision_world,
                    registry,
                    hit_zone_store,
                    anim_time,
                    range,
                )
            } else {
                (command.aim_origin, hitscan_direction)
            };
            let events = fire_hitscan(
                owner_pawn,
                origin,
                direction,
                collision_world,
                registry,
                hit_zone_store,
                anim_time,
                damage,
                knockback.as_ref(),
                pellet_count,
                spread_radians,
                range,
                resolution,
                projectile.as_ref(),
                splash.as_ref(),
                &credit_source,
                shell_counter,
                pellet_salt_name,
                active_slot,
            );
            if resolution == ResolutionMode::Hitscan {
                weapon.apply_bloom_shot();
            }
            events
        }
        WeaponFireAuthorization::Empty => WeaponFireEvents {
            dry_fire: true,
            ..WeaponFireEvents::default()
        },
        WeaponFireAuthorization::Rejected => WeaponFireEvents::default(),
    }
}

/// Resolve a frozen fixed-tick shot against the caller's current aim and target pose.
#[allow(clippy::too_many_arguments)]
pub fn resolve_activation_shot(
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
    shot: ResolvedWeaponShot,
) -> WeaponFireEvents {
    resolve_activation_shot_owned(
        registry,
        owner_pawn,
        weapon.clone(),
        pellet_salt_name,
        active_slot,
        command,
        placement,
        collision_world,
        hit_zone_store,
        anim_time,
        shot,
    )
}

/// Consume the frozen producer scratch without cloning another live component.
#[allow(clippy::too_many_arguments)]
pub fn resolve_activation_shot_owned(
    registry: &EntityRegistry,
    owner_pawn: Option<EntityId>,
    mut snapshot: WeaponComponent,
    pellet_salt_name: &str,
    active_slot: usize,
    command: &WeaponFireCommand,
    placement: &WeaponPlacementDescriptor,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    shot: ResolvedWeaponShot,
) -> WeaponFireEvents {
    shot.apply_to(&mut snapshot);
    snapshot.shells_fired = shot.activation.shell_counter;
    snapshot.bloom_accumulator_degrees = shot.activation.bloom_degrees;
    let mut events = tick_resolved_component(
        registry,
        owner_pawn,
        &mut snapshot,
        pellet_salt_name,
        active_slot,
        command,
        placement,
        collision_world,
        hit_zone_store,
        anim_time,
        WeaponFireAuthorization::Accepted,
    );
    for launch in &mut events.projectile_launches {
        launch.sounds = Some(shot.sounds.clone());
        launch.action = Some(shot.activation.action.clone());
        launch.shot_id = Some(shot.activation.shot_id);
        launch.model_scale = shot.projectile_model_scale;
    }
    events.resolved_shot = Some(shot);
    events
}

/// Compose the dynamic hitscan cone identically for host simulation and client
/// prediction. The movement component is absent for non-pawn owners, where
/// movement accuracy contributes nothing.
fn composed_hitscan_cone(
    registry: &EntityRegistry,
    owner_pawn: Option<EntityId>,
    weapon: &WeaponComponent,
    aim_direction: Vec3,
) -> (f32, Vec3) {
    let (horizontal_speed, run_speed) = owner_pawn
        .and_then(|pawn| registry.get_component::<PlayerMovementComponent>(pawn).ok())
        .map_or((0.0, 0.0), |movement| {
            let velocity = movement.velocity;
            (
                (velocity.x * velocity.x + velocity.z * velocity.z).sqrt(),
                movement.ground_params.speed.run,
            )
        });
    let effective_degrees = weapon.effective_spread_degrees(horizontal_speed, run_speed);
    (
        effective_degrees.to_radians(),
        spread::tilt_cone_axis_upward(
            aim_direction,
            weapon.spread_vertical_bias,
            effective_degrees,
        ),
    )
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
    knockback: Option<&KnockbackDescriptor>,
    pellet_count: u32,
    spread_radians: f32,
    range: f32,
    resolution: ResolutionMode,
    projectile: Option<&ProjectileDescriptor>,
    splash: Option<&SplashDescriptor>,
    credit_source: &str,
    shell_counter: u32,
    pellet_salt_name: &str,
    active_slot: usize,
) -> WeaponFireEvents {
    let mut events = WeaponFireEvents {
        resolved_shot: None,
        activate: Some(WeaponActivation { origin, direction }),
        impacts: Vec::with_capacity(pellet_count as usize),
        projectile_launches: Vec::new(),
        spawned: Vec::new(),
        dry_fire: false,
        overheat: false,
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
                let mut impact = match resolve_nearest_hit(NearestHitQuery {
                    owner_pawn,
                    origin,
                    direction: pellet_direction,
                    collision_world,
                    registry,
                    hit_zone_store,
                    anim_time,
                    range,
                }) {
                    Some(NearestHit::Entity(entity)) => impact_from_entity(entity, damage),
                    Some(NearestHit::World(world)) => WeaponImpact {
                        point: world.point,
                        normal: world.normal,
                        target: None,
                        zone: None,
                        outcome: ActivationOutcome::Hit(DamagePayload {
                            amount: damage,
                            impulse: glam::Vec3::ZERO,
                        }),
                    },
                    None => continue,
                };
                if let ActivationOutcome::Hit(payload) = &mut impact.outcome {
                    payload.impulse = knockback.map_or(Vec3::ZERO, |config| {
                        postretro_foundation::knockback_impulse(
                            config.speed,
                            config.upward_bias,
                            pellet_direction,
                        )
                    });
                }
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
                sounds: None,
                action: None,
                shot_id: None,
                model_scale: 1.0,
                origin,
                direction,
                speed: projectile.speed,
                radius: projectile.radius,
                range,
                lifetime: projectile.lifetime_ms / 1000.0,
                damage,
                knockback_impulse: knockback.map_or(Vec3::ZERO, |config| {
                    postretro_foundation::knockback_impulse(
                        config.speed,
                        config.upward_bias,
                        direction,
                    )
                }),
                credit_source: credit_source.to_string(),
                descriptor: projectile.clone(),
                splash: splash.cloned(),
            });
        }
    }

    events
}

#[allow(clippy::too_many_arguments)]
pub fn resolve_projectile_launch_pose(
    owner_pawn: Option<EntityId>,
    aim_origin: Vec3,
    aim_direction: Vec3,
    placement: &WeaponPlacementDescriptor,
    muzzle_offset: Option<Vec3>,
    projectile_radius: f32,
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

    let launch_origin = resolve_muzzle_launch_origin(
        aim_origin,
        aim_direction,
        placement,
        muzzle_local,
        projectile_radius,
        collision_world,
        range,
    );
    if launch_origin.obstructed {
        // A composed muzzle can cross nearby world geometry. Launch from the
        // valid eye ray rather than letting spawn grace strand the projectile
        // beyond the first world contact.
        return (aim_origin, aim_direction);
    }

    let convergence = resolve_nearest_hit_with_world(
        NearestHitQuery {
            owner_pawn,
            origin: aim_origin,
            direction: aim_direction,
            collision_world,
            registry,
            hit_zone_store,
            anim_time,
            range,
        },
        launch_origin.eye_world_hit.filter(|hit| hit.toi <= range),
    )
    .map_or(aim_origin + aim_direction * range, |hit| match hit {
        NearestHit::World(hit) => hit.point,
        NearestHit::Entity(hit) => hit.point,
    });
    let muzzle_to_convergence = convergence - launch_origin.origin;
    let length_squared = muzzle_to_convergence.length_squared();
    if !muzzle_to_convergence.is_finite()
        || !length_squared.is_finite()
        || length_squared <= MUZZLE_DIRECTION_EPSILON_SQUARED
        || muzzle_to_convergence.dot(aim_direction) <= 0.0
    {
        return (launch_origin.origin, aim_direction);
    }

    (
        launch_origin.origin,
        muzzle_to_convergence / length_squared.sqrt(),
    )
}

struct ProjectileLaunchOrigin {
    origin: Vec3,
    eye_world_hit: Option<WorldHit>,
    obstructed: bool,
}

fn resolve_muzzle_launch_origin(
    aim_origin: Vec3,
    aim_direction: Vec3,
    placement: &WeaponPlacementDescriptor,
    muzzle_local: Vec3,
    projectile_radius: f32,
    collision_world: &CollisionWorld,
    range: f32,
) -> ProjectileLaunchOrigin {
    let muzzle = muzzle_world_origin(aim_origin, aim_direction, placement, muzzle_local);
    let muzzle_forward_distance = (muzzle - aim_origin).dot(aim_direction).max(0.0);
    let eye_world_hit = resolve_world_hit(
        aim_origin,
        aim_direction,
        collision_world,
        range.max(muzzle_forward_distance),
    );
    let muzzle_segment = muzzle - aim_origin;
    let muzzle_segment_length_squared = muzzle_segment.length_squared();
    let muzzle_segment_length = muzzle_segment_length_squared.sqrt();
    let valid_muzzle_segment = muzzle_segment.is_finite()
        && muzzle_segment_length_squared.is_finite()
        && muzzle_segment_length_squared > MUZZLE_DIRECTION_EPSILON_SQUARED;
    let muzzle_segment_obstructed = valid_muzzle_segment
        && if projectile_radius > 0.0 {
            cast_sphere_exact(
                collision_world,
                aim_origin,
                projectile_radius,
                Vec3::new(
                    muzzle_segment.x / muzzle_segment_length,
                    muzzle_segment.y / muzzle_segment_length,
                    muzzle_segment.z / muzzle_segment_length,
                ),
                muzzle_segment_length,
            )
            .is_some()
        } else {
            resolve_world_hit(
                aim_origin,
                muzzle_segment / muzzle_segment_length,
                collision_world,
                muzzle_segment_length,
            )
            .is_some()
        };
    let obstructed = muzzle_segment_obstructed
        || eye_world_hit.is_some_and(|hit| hit.toi <= muzzle_forward_distance);
    ProjectileLaunchOrigin {
        origin: if obstructed { aim_origin } else { muzzle },
        eye_world_hit,
        obstructed,
    }
}

/// The deterministic pellet salt chooses a canonical descriptor identity first,
/// then the live component's credit source, and finally the shared unknown
/// source. Never use allocation-ordered entity/network ids here: spawn-order
/// reversal replays must preserve the sampled fan.
pub fn pellet_salt_name(
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
struct NearestHitQuery<'a> {
    owner_pawn: Option<EntityId>,
    origin: Vec3,
    direction: Vec3,
    collision_world: &'a CollisionWorld,
    registry: &'a EntityRegistry,
    hit_zone_store: &'a HitZoneStore,
    anim_time: f64,
    range: f32,
}

fn resolve_nearest_hit(query: NearestHitQuery<'_>) -> Option<NearestHit> {
    let world_hit = resolve_world_hit(
        query.origin,
        query.direction,
        query.collision_world,
        query.range,
    );
    resolve_nearest_hit_with_world(query, world_hit)
}

fn resolve_world_hit(
    origin: Vec3,
    direction: Vec3,
    collision_world: &CollisionWorld,
    range: f32,
) -> Option<WorldHit> {
    cast_ray(collision_world, origin, direction, range).map(|hit| WorldHit {
        toi: hit.time_of_impact,
        point: origin + direction * hit.time_of_impact,
        normal: hit.normal,
    })
}

fn resolve_nearest_hit_with_world(
    query: NearestHitQuery<'_>,
    world_hit: Option<WorldHit>,
) -> Option<NearestHit> {
    let NearestHitQuery {
        owner_pawn,
        origin,
        direction,
        collision_world: _,
        registry,
        hit_zone_store,
        anim_time,
        range,
    } = query;

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
        outcome: ActivationOutcome::Hit(DamagePayload {
            amount: damage,
            impulse: glam::Vec3::ZERO,
        }),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use glam::Vec3;
    use postretro_entities::components::health::{HealthComponent, Hitbox};
    use postretro_entities::components::projectile::ProjectileComponent;
    use postretro_entities::registry::{ComponentKind, Transform};
    use postretro_foundation::{
        AmmoResource, ProjectileBodyVisual, ProjectileVisual, ReloadStyle, WeaponDescriptor,
        WeaponResource,
    };

    const EPSILON: f32 = 1.0e-5;

    /// Minimal fixed-tick aim fixture. Weapon tests exercise simulation rays,
    /// not the binary camera implementation.
    pub(crate) struct TestAim {
        origin: Vec3,
        direction: Vec3,
    }

    impl TestAim {
        pub(crate) fn forward(origin: Vec3) -> Self {
            Self {
                origin,
                direction: Vec3::NEG_Z,
            }
        }

        fn ray(&self) -> (Vec3, Vec3) {
            (self.origin, self.direction)
        }
    }

    /// Input binding is binary-owned; simulation tests supply the resolved
    /// fixed-tick button state directly.
    pub(crate) struct TestFireInput;

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
            sounds: None,
            knockback: None,
            damage: 25.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            bloom_per_shot_degrees: 0.0,
            bloom_max_degrees: 0.0,
            bloom_decay_degrees_per_second: 0.0,
            bloom_decay_delay_ms: 0.0,
            movement_spread_degrees: 0.0,
            spread_vertical_bias: 0.0,
            range: 10.0,
            primary: postretro_foundation::WeaponActivationDescriptor::single(
                match fire_mode {
                    postretro_foundation::FireMode::Semi => {
                        postretro_foundation::ActivationTrigger::Press
                    }
                    postretro_foundation::FireMode::Auto => {
                        postretro_foundation::ActivationTrigger::Hold
                    }
                },
                cooldown_ms,
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
            sounds: None,
            knockback: None,
            damage: 25.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            bloom_per_shot_degrees: 0.0,
            bloom_max_degrees: 0.0,
            bloom_decay_degrees_per_second: 0.0,
            bloom_decay_delay_ms: 0.0,
            movement_spread_degrees: 0.0,
            spread_vertical_bias: 0.0,
            range: 10.0,
            primary: postretro_foundation::WeaponActivationDescriptor::single(
                match fire_mode {
                    postretro_foundation::FireMode::Semi => {
                        postretro_foundation::ActivationTrigger::Press
                    }
                    postretro_foundation::FireMode::Auto => {
                        postretro_foundation::ActivationTrigger::Hold
                    }
                },
                cooldown_ms,
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
        button: &FireButtonState,
        aim: &TestAim,
        world: &CollisionWorld,
        _tick_dt: f32,
    ) -> WeaponFireEvents {
        let store = HitZoneStore::new();
        let (aim_origin, aim_direction) = aim.ray();
        tick_resolved(
            registry,
            active_wieldable,
            &WeaponFireCommand {
                button: *button,
                aim_origin,
                aim_direction,
                can_fire: true,
            },
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

    pub(crate) fn input_system() -> TestFireInput {
        TestFireInput
    }

    pub(crate) fn shoot_snapshot(_input: &mut TestFireInput, active: bool) -> FireButtonState {
        FireButtonState {
            pressed: active,
            active,
        }
    }

    pub(crate) fn wall_world() -> CollisionWorld {
        wall_world_at(-5.0)
    }

    pub(crate) fn wall_world_at(z: f32) -> CollisionWorld {
        let points = vec![
            Vec3::new(-1.0, -1.0, z),
            Vec3::new(1.0, -1.0, z),
            Vec3::new(1.0, 1.0, z),
            Vec3::new(-1.0, 1.0, z),
        ];
        let triangles = vec![[0u32, 1, 2], [0, 2, 3]];
        CollisionWorld::from_triangles_for_test(points, triangles)
    }

    fn lateral_wall_world_at(x: f32) -> CollisionWorld {
        let points = vec![
            Vec3::new(x, -1.0, -1.0),
            Vec3::new(x, -1.0, 1.0),
            Vec3::new(x, 1.0, 1.0),
            Vec3::new(x, 1.0, -1.0),
        ];
        let triangles = vec![[0u32, 1, 2], [0, 2, 3]];
        CollisionWorld::from_triangles_for_test(points, triangles)
    }

    fn ground_world() -> CollisionWorld {
        let points = vec![
            Vec3::new(-2.0, 0.0, -2.0),
            Vec3::new(2.0, 0.0, -2.0),
            Vec3::new(2.0, 0.0, 2.0),
            Vec3::new(-2.0, 0.0, 2.0),
        ];
        let triangles = vec![[0u32, 1, 2], [0, 2, 3]];
        CollisionWorld::from_triangles_for_test(points, triangles)
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
            0.0,
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
            0.0,
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
    fn projectile_launch_pose_uses_eye_when_short_range_ends_before_obstructed_muzzle() {
        // Regression: the obstruction guard stopped at projectile range and
        // missed a wall that was still between the eye and a farther muzzle.
        let placement = WeaponPlacementDescriptor::default();
        let registry = EntityRegistry::new();
        let zones = HitZoneStore::new();
        let range = 0.25;
        let muzzle = Vec3::new(0.0, 0.0, -0.8);
        assert!(
            range < -muzzle.z,
            "the muzzle must extend beyond flight range"
        );
        let (origin, direction) = resolve_projectile_launch_pose(
            None,
            Vec3::ZERO,
            Vec3::NEG_Z,
            &placement,
            Some(muzzle),
            0.0,
            &wall_world_at(-0.5),
            &registry,
            &zones,
            0.0,
            range,
        );

        assert_vec3_approx(origin, Vec3::ZERO);
        assert_vec3_approx(direction, Vec3::NEG_Z);
    }

    #[test]
    fn projectile_launch_pose_uses_eye_when_lateral_wall_intersects_muzzle_sweep() {
        // Regression: a laterally offset muzzle could put the projectile volume
        // through a side wall the crosshair ray never touched.
        let eye = Vec3::ZERO;
        let aim = Vec3::NEG_Z;
        let muzzle = Vec3::new(0.5, 0.0, -0.8);
        let projectile_radius = 0.2;
        let wall = lateral_wall_world_at(0.6);
        assert!(
            resolve_world_hit(eye, aim, &wall, 10.0).is_none(),
            "the crosshair ray must miss the lateral wall"
        );
        assert!(
            muzzle.x < 0.6 && muzzle.x + projectile_radius > 0.6,
            "the muzzle center stops short while its projectile volume crosses the wall"
        );
        let (origin, direction) = resolve_projectile_launch_pose(
            None,
            eye,
            aim,
            &WeaponPlacementDescriptor::default(),
            Some(muzzle),
            projectile_radius,
            &wall,
            &EntityRegistry::new(),
            &HitZoneStore::new(),
            0.0,
            10.0,
        );

        assert_vec3_approx(origin, eye);
        assert_vec3_approx(direction, aim);
    }

    #[test]
    fn projectile_launch_pose_uses_eye_when_downward_rocket_muzzle_is_below_ground() {
        // Regression: the reference rocket's composed muzzle could spawn below
        // the floor at steep downward pitch and miss its splash impact entirely.
        let eye = Vec3::new(0.0, 0.5, 0.0);
        let placement = WeaponPlacementDescriptor {
            offset: postretro_foundation::PlacementOffset {
                right: 0.4,
                up: -0.45,
                forward: 0.75,
            },
            rotation: postretro_foundation::PlacementRotation {
                yaw: -4.0,
                pitch: 1.0,
                roll: -2.0,
            },
        };
        let muzzle = Vec3::new(0.0, -0.05, -0.834);
        let composed_muzzle = muzzle_world_origin(eye, Vec3::NEG_Y, &placement, muzzle);
        assert!(
            composed_muzzle.y < 0.0,
            "the fixture must place the rocket muzzle below the ground"
        );

        let (origin, direction) = resolve_projectile_launch_pose(
            None,
            eye,
            Vec3::NEG_Y,
            &placement,
            Some(muzzle),
            0.0,
            &ground_world(),
            &EntityRegistry::new(),
            &HitZoneStore::new(),
            0.0,
            128.0,
        );

        assert_vec3_approx(origin, eye);
        assert_vec3_approx(direction, Vec3::NEG_Y);
    }

    #[test]
    fn projectile_without_muzzle_ignores_dynamic_accuracy_and_keeps_legacy_eye_launch_bits() {
        let mut weapon = projectile_weapon_component(None);
        weapon.spread_degrees = 20.0;
        weapon.bloom_per_shot_degrees = 3.0;
        weapon.bloom_max_degrees = 12.0;
        weapon.bloom_accumulator_degrees = 8.0;
        weapon.movement_spread_degrees = 6.0;
        weapon.spread_vertical_bias = 1.0;
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
        let resolution = resolve_test_client_shot(
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
            &[0.0, 0.0],
            &[],
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
        assert!(
            (weapon.bloom_accumulator_degrees - 8.0).abs() < f32::EPSILON,
            "trailing catch-up shots do not affect projectile accuracy"
        );
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
        weapon.spread_degrees = 20.0;
        weapon.bloom_per_shot_degrees = 3.0;
        weapon.bloom_max_degrees = 12.0;
        weapon.bloom_accumulator_degrees = 8.0;
        weapon.movement_spread_degrees = 6.0;
        weapon.spread_vertical_bias = 1.0;
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
        assert_vec3_approx(
            launch.direction,
            (command.aim_origin + command.aim_direction * launch.range - launch.origin).normalize(),
        );
        assert!((weapon.bloom_accumulator_degrees - 8.0).abs() < f32::EPSILON);
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

        let first = resolve_test_client_shot(
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
            &[0.0],
            &[],
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

        let blocked = resolve_test_client_shot(
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
            &[0.0],
            &[],
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
        let camera = TestAim::forward(Vec3::ZERO);
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
            ActivationOutcome::Hit(DamagePayload {
                amount: 25.0,
                impulse: glam::Vec3::ZERO
            })
        );
    }

    #[test]
    fn hitscan_knockback_uses_travel_direction_and_authored_upward_bias() {
        let mut registry = EntityRegistry::new();
        let mut component = weapon_component(FireMode::Semi, 100.0);
        component.damage = 0.0;
        component.knockback = Some(KnockbackDescriptor {
            speed: 12.0,
            upward_bias: 0.5,
        });
        let weapon_id = spawn_weapon(&mut registry, component);
        let camera = TestAim::forward(Vec3::ZERO);
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);
        let events = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &camera,
            &wall_world(),
            1.0 / 60.0,
        );
        let ActivationOutcome::Hit(payload) = only_impact(&events).outcome else {
            panic!("hit expected")
        };
        assert!(payload.amount.abs() < EPSILON);
        assert_vec3_approx(
            payload.impulse,
            Vec3::new(0.0, 1.0, -1.0).normalize() * 12.0,
        );
    }

    #[test]
    fn legacy_single_pellet_keeps_exact_axis_and_one_impact_event() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let camera = TestAim::forward(Vec3::ZERO);
        let (_, aim_direction) = camera.ray();
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

    // Pin P4: a multi-pellet shot is one impact event carrying every contact.
    #[test]
    fn multi_pellet_shot_is_one_impact_emission_carrying_every_contact() {
        let mut registry = EntityRegistry::new();
        let mut component = weapon_component(FireMode::Semi, 100.0);
        component.pellet_count = 8;
        component.spread_degrees = 0.0;
        let weapon_id = spawn_weapon(&mut registry, component);
        let shooter = Emitter::Entity {
            id: weapon_id,
            origin: Vec3::ZERO,
        };
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        let events = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &TestAim::forward(Vec3::ZERO),
            &wall_world(),
            1.0 / 60.0,
        );
        let emissions = events.emissions(&shooter, Some("shotgun".to_string()));

        let addresses: Vec<_> = emissions.iter().map(|emission| emission.address).collect();
        assert_eq!(
            addresses,
            ["activate", "impact"],
            "eight pellets, one impact"
        );
        assert_eq!(
            emissions[0].emitter, shooter,
            "fire sounds from the shooter"
        );
        let Emitter::Contacts(contacts) = &emissions[1].emitter else {
            panic!(
                "the impact carries its contacts, got {:?}",
                emissions[1].emitter
            );
        };
        assert_eq!(contacts.len(), 8);
        for contact in contacts {
            assert_eq!(contact.hit, postretro_entities::ContactHit::World);
            assert_vec3_approx(contact.point, Vec3::new(0.0, 0.0, -5.0));
            assert!(
                contact.normal.abs_diff_eq(Vec3::Z, 1.0e-4)
                    || contact.normal.abs_diff_eq(Vec3::NEG_Z, 1.0e-4),
                "each contact keeps the wall's normal, got {}",
                contact.normal,
            );
        }
        assert!(
            emissions
                .iter()
                .all(|emission| emission.weapon.as_deref() == Some("shotgun")),
            "every event names the weapon it came from",
        );
    }

    #[test]
    fn shot_with_no_contact_emits_no_impact() {
        let mut registry = EntityRegistry::new();
        let weapon_id = spawn_weapon(&mut registry, weapon_component(FireMode::Semi, 100.0));
        let mut input = input_system();
        let pressed = shoot_snapshot(&mut input, true);

        // The only wall stands behind the shooter.
        let events = fire_tick(
            &mut registry,
            Some(weapon_id),
            &pressed,
            &TestAim::forward(Vec3::ZERO),
            &wall_world_at(5.0),
            1.0 / 60.0,
        );
        let shooter = Emitter::Entity {
            id: weapon_id,
            origin: Vec3::ZERO,
        };
        let addresses: Vec<_> = events
            .emissions(&shooter, None)
            .iter()
            .map(|emission| emission.address)
            .collect();
        assert_eq!(addresses, ["activate"]);
    }

    // A connected client's predicted hitscan shot into a wall keeps the wall
    // contact and its normal, so it hears (and declares) its own impact.
    #[test]
    fn client_hitscan_into_a_wall_keeps_the_world_contact_and_its_normal() {
        let mut weapon = weapon_component(FireMode::Semi, 100.0);
        let resolution = resolve_test_client_shot(
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
            1,
            &[0.0],
            &[],
            &wall_world(),
            &EntityRegistry::new(),
            &HitZoneStore::new(),
            0.0,
            0.0,
        )
        .expect("hitscan fire resolves");
        assert!(resolution.hits.is_empty(), "a wall is no damage claim");
        let [contact] = resolution.world_contacts.as_slice() else {
            panic!("one wall contact, got {:?}", resolution.world_contacts);
        };
        assert_vec3_approx(contact.point, Vec3::new(0.0, 0.0, -5.0));
        assert!(
            contact.normal.abs_diff_eq(Vec3::Z, 1.0e-4)
                || contact.normal.abs_diff_eq(Vec3::NEG_Z, 1.0e-4)
        );
        let contacts = resolution.impact_contacts();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].hit, postretro_entities::ContactHit::World);
    }

    const PRESSED: FireButtonState = FireButtonState {
        pressed: true,
        active: true,
    };

    /// The owner projection of host slot `slot`: magazine, reload flag and
    /// reload progress, all from that slot.
    fn projection(
        slot: usize,
        magazine: f32,
        reload_active: bool,
        progress: f32,
    ) -> ReplicatedWeaponProjection {
        ReplicatedWeaponProjection {
            magazine: Some(SlotSample {
                slot,
                value: Some(magazine),
            }),
            reload_active: Some(SlotSample {
                slot,
                value: reload_active,
            }),
            reload_progress: Some(SlotSample {
                slot,
                value: progress,
            }),
            ..ReplicatedWeaponProjection::default()
        }
    }

    fn idle_magazine(magazine: f32) -> ReplicatedWeaponProjection {
        projection(0, magazine, false, 0.0)
    }

    fn reloading_magazine(magazine: f32, progress: f32) -> ReplicatedWeaponProjection {
        projection(0, magazine, true, progress)
    }

    fn per_shell(mut weapon: WeaponComponent) -> WeaponComponent {
        weapon
            .ammo
            .as_mut()
            .expect("an ammo-fed weapon")
            .reload_style = ReloadStyle::PerShell;
        weapon
    }

    // A connected client presents a dry fire from the replicated magazine.
    #[test]
    fn client_dry_fire_follows_the_replicated_magazine() {
        let weapon = ammo_weapon_component(FireMode::Semi, 100.0, 8, 1);
        assert_eq!(
            client_pull_presentation(&weapon, 0, &idle_magazine(0.0)),
            ClientPullPresentation::DryFire,
        );
        assert_eq!(
            client_pull_presentation(&weapon, 0, &idle_magazine(1.0)),
            ClientPullPresentation::Fire,
            "with ammo it fires",
        );
        assert_eq!(
            client_pull_presentation(
                &weapon,
                0,
                &ReplicatedWeaponProjection {
                    magazine: None,
                    ..idle_magazine(0.0)
                },
            ),
            ClientPullPresentation::Fire,
            "no count yet: present a fire",
        );
        assert_eq!(
            client_pull_presentation(
                &weapon,
                0,
                &ReplicatedWeaponProjection {
                    magazine: Some(SlotSample {
                        slot: 0,
                        value: None,
                    }),
                    ..idle_magazine(0.0)
                },
            ),
            ClientPullPresentation::Fire,
            "the host names this slot resourceless: no magazine to run dry",
        );
        let unlimited = weapon_component(FireMode::Semi, 100.0);
        assert_eq!(
            client_pull_presentation(&unlimited, 0, &idle_magazine(0.0)),
            ClientPullPresentation::Fire,
            "no ammo, never dry",
        );
    }

    #[test]
    fn client_pull_presentation_follows_the_host_reload_rules() {
        let magazine = ammo_weapon_component(FireMode::Semi, 100.0, 8, 1);
        for rounds in [0.0, 5.0] {
            assert_eq!(
                client_pull_presentation(&magazine, 0, &reloading_magazine(rounds, 0.4)),
                ClientPullPresentation::Silent,
                "the host refuses every pull during a magazine reload (magazine {rounds})",
            );
        }
        // Regression: the replayed Completed endpoint (flag up, full progress)
        // read as a reload in progress and silenced the next pull.
        assert_eq!(
            client_pull_presentation(&magazine, 0, &reloading_magazine(8.0, 1.0)),
            ClientPullPresentation::Fire,
            "a completed magazine reload is idle",
        );
        assert_eq!(
            client_pull_presentation(&magazine, 0, &reloading_magazine(0.0, 1.0)),
            ClientPullPresentation::DryFire,
            "a reload that completed with an empty reserve leaves an idle, empty weapon",
        );

        let shell = per_shell(ammo_weapon_component(FireMode::Semi, 100.0, 8, 2));
        assert_eq!(
            client_pull_presentation(&shell, 0, &reloading_magazine(2.0, 0.3)),
            ClientPullPresentation::Fire,
            "a covered pull cancels the per-shell reload and fires",
        );
        assert_eq!(
            client_pull_presentation(&shell, 0, &reloading_magazine(1.0, 0.3)),
            ClientPullPresentation::Silent,
            "an uncovered pull during a per-shell reload is refused, not dry",
        );
        assert_eq!(
            client_pull_presentation(&shell, 0, &idle_magazine(1.0)),
            ClientPullPresentation::DryFire,
            "an idle per-shell weapon still dry fires",
        );
    }

    #[test]
    fn a_projection_of_another_weapon_predicts_a_fire() {
        let weapon = ammo_weapon_component(FireMode::Semi, 100.0, 8, 1);
        assert_eq!(
            client_pull_presentation(&weapon, 1, &projection(0, 0.0, false, 0.0)),
            ClientPullPresentation::Fire,
            "the host still projects the weapon this client switched away from",
        );
        assert_eq!(
            client_pull_presentation(&weapon, 1, &ReplicatedWeaponProjection::default()),
            ClientPullPresentation::Fire,
            "nothing replicated yet",
        );
        assert_eq!(
            client_pull_presentation(
                &weapon,
                1,
                &ReplicatedWeaponProjection {
                    magazine: Some(SlotSample {
                        slot: 0,
                        value: Some(0.0),
                    }),
                    ..projection(1, 0.0, false, 0.0)
                },
            ),
            ClientPullPresentation::Fire,
            "the magazine still shows the previous weapon's count",
        );
        assert_eq!(
            client_pull_presentation(
                &weapon,
                1,
                &ReplicatedWeaponProjection {
                    reload_progress: Some(SlotSample {
                        slot: 0,
                        value: 0.4,
                    }),
                    ..projection(1, 0.0, true, 0.4)
                },
            ),
            ClientPullPresentation::Fire,
            "a reload flag is read only beside progress of the same slot",
        );
        assert_eq!(
            client_pull_presentation(&weapon, 1, &projection(1, 0.0, false, 0.0)),
            ClientPullPresentation::DryFire,
            "every value names this weapon",
        );
    }

    #[test]
    fn a_projection_merge_keeps_values_the_fresh_batch_did_not_carry() {
        let mut held = projection(0, 3.0, false, 0.0);
        held.merge(&ReplicatedWeaponProjection {
            magazine: Some(SlotSample {
                slot: 1,
                value: Some(7.0),
            }),
            ..ReplicatedWeaponProjection::default()
        });
        assert_eq!(
            held.magazine,
            Some(SlotSample {
                slot: 1,
                value: Some(7.0),
            })
        );
        assert_eq!(
            held.reload_active,
            Some(SlotSample {
                slot: 0,
                value: false,
            }),
        );
        // A fresh absence is a value: it replaces the held count.
        held.merge(&ReplicatedWeaponProjection {
            magazine: Some(SlotSample {
                slot: 1,
                value: None,
            }),
            ..ReplicatedWeaponProjection::default()
        });
        assert_eq!(
            held.magazine,
            Some(SlotSample {
                slot: 1,
                value: None,
            })
        );
    }

    // The fire gate alone decides whether a pull is predicted. A dry click on
    // an enemy still records its shot for reconcile and declares the hit it
    // resolved, so the host applies damage whenever it fired; only what the
    // pull presents changes.
    #[test]
    fn a_dry_pull_still_predicts_and_declares_its_shot() {
        let mut registry = EntityRegistry::new();
        let target = spawn_hitbox_entity(
            &mut registry,
            Vec3::new(0.0, 0.0, -3.0),
            Vec3::splat(0.5),
            Vec3::ZERO,
        );
        let mut weapon = ammo_weapon_component(FireMode::Semi, 100.0, 8, 1);
        let presentation = client_pull_presentation(&weapon, 0, &idle_magazine(0.0));
        assert_eq!(presentation, ClientPullPresentation::DryFire);
        let resolution = resolve_test_client_shot(
            None,
            &mut weapon,
            "weapon.unknown",
            0,
            PRESSED,
            Vec3::ZERO,
            Vec3::NEG_Z,
            &WeaponPlacementDescriptor::default(),
            None,
            7,
            &[0.0],
            &[],
            &wall_world(),
            &registry,
            &HitZoneStore::new(),
            0.0,
            0.0,
        )
        .expect("the fire gate passes, so the pull resolves a shot");
        assert!((weapon.cooldown_remaining_ms - 100.0).abs() < f32::EPSILON);

        // The production dispatch: a dry hitscan pull raises only `dry_fire`,
        // shows no projectile, and declares its resolved hits now.
        let effects = client_pull_effects(
            presentation,
            resolution.projectile_launch.is_some(),
            !resolution.impact_contacts().is_empty(),
        );
        assert_eq!(
            effects,
            ClientPullEffects {
                addresses: vec!["dry_fire"],
                spawn_projectile: false,
                declaration: ClientShotDeclaration::ResolvedNow,
            },
        );
        assert_eq!(
            resolution
                .hits
                .iter()
                .map(|hit| hit.target)
                .collect::<Vec<_>>(),
            [target],
            "the declaration names the struck enemy, as a fire's would",
        );
        assert_eq!(resolution.client_tick, 7);

        let mut shots = ClientPredictedShots::new();
        shots.predict(
            test_shot_id(0xD),
            EntityId::from_raw(1),
            &resolution,
            0.0,
            weapon.cooldown_remaining_ms,
            presentation,
        );
        let record = shots
            .get(test_shot_id(0xD))
            .expect("the shot is recorded for reconcile");
        assert_eq!(record.status, PredictedShotStatus::Pending);
        assert_eq!(record.client_tick, 7);
        assert!((record.cooldown_after_ms - 100.0).abs() < f32::EPSILON);
        assert!(!record.muzzle_fx_visible);
        assert!(
            !record.hitmarker_visible,
            "a dry click marks no hit before the verdict",
        );
        let reconciled = shots
            .apply_verdict(&mut registry, test_shot_id(0xD), true, true)
            .expect("the host's verdict reconciles the dry pull's shot");
        assert_eq!(reconciled.status, PredictedShotStatus::Accepted);
    }

    #[test]
    fn eight_zero_spread_pellets_resolve_eight_exact_axis_impacts() {
        let mut registry = EntityRegistry::new();
        let mut component = weapon_component(FireMode::Semi, 100.0);
        component.pellet_count = 8;
        component.spread_degrees = 0.0;
        let weapon_id = spawn_weapon(&mut registry, component);
        let camera = TestAim::forward(Vec3::ZERO);
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

        let resolution = resolve_test_client_shot(
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
            &[0.0],
            &[],
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

        let resolution = resolve_test_client_shot(
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
            &[0.0],
            &[],
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
        let hits = resolve_test_client_shot(
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
            &[0.0],
            &[],
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
        let launch = resolve_test_client_shot(
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
            &[0.0],
            &[],
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

        let resolution = resolve_test_client_shot(
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
            &[0.0],
            &[],
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
        let camera = TestAim::forward(Vec3::ZERO);
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
        let camera = TestAim::forward(Vec3::ZERO);
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
            ActivationOutcome::Hit(DamagePayload {
                amount: 25.0,
                impulse: glam::Vec3::ZERO
            })
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
        let camera = TestAim::forward(Vec3::ZERO);
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
        let camera = TestAim::forward(Vec3::ZERO);
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
        let camera = TestAim::forward(Vec3::ZERO);
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
        let camera = TestAim::forward(Vec3::ZERO);
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

        let camera = TestAim::forward(Vec3::ZERO);
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
        button: &FireButtonState,
        aim: &TestAim,
        world: &CollisionWorld,
        store: &HitZoneStore,
        anim_time: f64,
        _tick_dt: f32,
    ) -> WeaponFireEvents {
        let (aim_origin, aim_direction) = aim.ray();
        tick_resolved(
            registry,
            active_wieldable,
            &WeaponFireCommand {
                button: *button,
                aim_origin,
                aim_direction,
                can_fire: true,
            },
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

        let resolution = resolve_test_client_shot(
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
            &[0.0],
            &[],
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
        let camera = TestAim::forward(Vec3::ZERO);
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
        let camera = TestAim::forward(Vec3::ZERO);
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
        let camera = TestAim::forward(Vec3::ZERO);
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
            world_contacts: Vec::new(),
            client_tick: 9,
            hits: vec![LocalHitRecord {
                normal: Vec3::Y,
                target,
                point: Vec3::new(1.0, 2.0, 3.0),
                zone: Some("head".to_string()),
            }],
            projectile_launch: None,
        };
        let mut predicted = ClientPredictedShots::new();

        predicted.predict(
            test_shot_id(0xA),
            EntityId::from_raw(1),
            &resolution,
            0.0,
            100.0,
            ClientPullPresentation::Fire,
        );

        let record = predicted
            .get(test_shot_id(0xA))
            .expect("shot should be recorded");
        assert_eq!(record.client_tick, 9);
        assert!(record.muzzle_fx_visible);
        assert!(record.hitmarker_visible);
        assert_eq!(record.status, PredictedShotStatus::Pending);

        // A dry or silent pull that resolved the same hit presents no shot:
        // it is still recorded for reconcile, with neither marker shown.
        for presentation in [
            ClientPullPresentation::DryFire,
            ClientPullPresentation::Silent,
        ] {
            predicted.predict(
                test_shot_id(0xB),
                EntityId::from_raw(1),
                &resolution,
                0.0,
                100.0,
                presentation,
            );
            let record = predicted
                .get(test_shot_id(0xB))
                .expect("shot should be recorded");
            assert_eq!(
                record.status,
                PredictedShotStatus::Pending,
                "{presentation:?}"
            );
            assert!(!record.muzzle_fx_visible, "{presentation:?}");
            assert!(!record.hitmarker_visible, "{presentation:?}");
        }
    }

    #[test]
    fn predicted_projectile_impact_marks_hitmarker_after_later_resolution() {
        let resolution = ClientFireResolution {
            world_contacts: Vec::new(),
            client_tick: 9,
            hits: Vec::new(),
            projectile_launch: None,
        };
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(
            test_shot_id(0xA),
            EntityId::from_raw(1),
            &resolution,
            0.0,
            100.0,
            ClientPullPresentation::Fire,
        );

        assert!(
            !predicted
                .get(test_shot_id(0xA))
                .expect("projectile fire is pending")
                .hitmarker_visible
        );
        predicted.mark_hitmarker(test_shot_id(0xA));
        assert!(
            predicted
                .get(test_shot_id(0xA))
                .expect("projectile remains pending before verdict")
                .hitmarker_visible
        );
    }

    #[test]
    fn shot_verdict_accept_confirms_predicted_markers() {
        let resolution = ClientFireResolution {
            world_contacts: Vec::new(),
            client_tick: 9,
            hits: vec![LocalHitRecord {
                normal: Vec3::Y,
                target: EntityId::from_raw(2),
                point: Vec3::ZERO,
                zone: None,
            }],
            projectile_launch: None,
        };
        let (mut registry, weapon) = client_weapon_registry();
        set_client_cooldown(&mut registry, weapon, 100.0);
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(
            test_shot_id(0xA),
            weapon,
            &resolution,
            0.0,
            100.0,
            ClientPullPresentation::Fire,
        );

        let record = predicted
            .apply_verdict(&mut registry, test_shot_id(0xA), true, true)
            .expect("verdict should match a predicted shot");

        assert!(record.muzzle_fx_visible);
        assert!(record.hitmarker_visible);
        assert_eq!(record.status, PredictedShotStatus::Accepted);
        assert!(approx_eq(client_cooldown(&registry, weapon), 100.0));
        assert!(
            predicted.get(test_shot_id(0xA)).is_none(),
            "a terminal verdict prunes the record"
        );
    }

    #[test]
    fn shot_verdict_authorized_miss_keeps_fire_state_and_retracts_hitmarker() {
        let resolution = ClientFireResolution {
            world_contacts: Vec::new(),
            client_tick: 9,
            hits: vec![LocalHitRecord {
                normal: Vec3::Y,
                target: EntityId::from_raw(2),
                point: Vec3::ZERO,
                zone: None,
            }],
            projectile_launch: None,
        };
        let (mut registry, weapon) = client_weapon_registry();
        set_client_cooldown(&mut registry, weapon, 100.0);
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(
            test_shot_id(0xA),
            weapon,
            &resolution,
            25.0,
            100.0,
            ClientPullPresentation::Fire,
        );

        let record = predicted
            .apply_verdict(&mut registry, test_shot_id(0xA), true, false)
            .expect("verdict should match a predicted shot");

        assert!(record.muzzle_fx_visible);
        assert!(!record.hitmarker_visible);
        assert_eq!(record.status, PredictedShotStatus::Accepted);
        assert!(approx_eq(client_cooldown(&registry, weapon), 100.0));
        assert!(
            predicted.get(test_shot_id(0xA)).is_none(),
            "a terminal verdict prunes the record"
        );
    }

    #[test]
    fn shot_verdict_reject_retracts_presentation_and_keeps_correlated_recovery() {
        let resolution = ClientFireResolution {
            world_contacts: Vec::new(),
            client_tick: 9,
            hits: vec![LocalHitRecord {
                normal: Vec3::Y,
                target: EntityId::from_raw(2),
                point: Vec3::ZERO,
                zone: None,
            }],
            projectile_launch: None,
        };
        let (mut registry, weapon) = client_weapon_registry();
        set_client_cooldown(&mut registry, weapon, 100.0);
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(
            test_shot_id(0xA),
            weapon,
            &resolution,
            25.0,
            100.0,
            ClientPullPresentation::Fire,
        );

        let record = predicted
            .apply_verdict(&mut registry, test_shot_id(0xA), false, false)
            .expect("verdict should match a predicted shot");

        assert!(!record.muzzle_fx_visible);
        assert!(!record.hitmarker_visible);
        assert_eq!(record.status, PredictedShotStatus::Rejected);
        assert!(approx_eq(client_cooldown(&registry, weapon), 100.0));
        assert!(
            predicted.get(test_shot_id(0xA)).is_none(),
            "a terminal verdict prunes the record"
        );
    }

    // Regression: a host-rejected projectile kept flying locally until its later
    // impact/expiry declaration, despite FIRE already having no authority.
    #[test]
    fn shot_verdict_reject_removes_only_matching_predicted_projectile() {
        let resolution = ClientFireResolution {
            world_contacts: Vec::new(),
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
                        source_sounds: None,
                        predicted_visible: true,
                        source_action: None,
                        source_shot: None,
                        knockback_impulse: [0.0; 3],
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
                        splash: None,
                        source_weapon: None,
                        activation: None,
                    },
                )
                .expect("predicted projectile state attaches");
            projectile
        };
        let rejected = spawn_predicted(&mut registry, test_shot_id(0xA));
        let other = spawn_predicted(&mut registry, test_shot_id(0xB));
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(
            test_shot_id(0xA),
            weapon,
            &resolution,
            25.0,
            100.0,
            ClientPullPresentation::Fire,
        );

        let record = predicted
            .apply_verdict(&mut registry, test_shot_id(0xA), false, false)
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
            world_contacts: Vec::new(),
            client_tick: 9,
            hits: vec![LocalHitRecord {
                normal: Vec3::Y,
                target: EntityId::from_raw(2),
                point: Vec3::ZERO,
                zone: None,
            }],
            projectile_launch: None,
        };
        let (mut registry, weapon) = client_weapon_registry();
        set_client_cooldown(&mut registry, weapon, 100.0);
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(
            test_shot_id(0xA),
            weapon,
            &resolution,
            25.0,
            100.0,
            ClientPullPresentation::Fire,
        );

        let accepted = predicted
            .apply_verdict(&mut registry, test_shot_id(0xA), true, true)
            .expect("accept should match");
        assert_eq!(accepted.status, PredictedShotStatus::Accepted);
        assert!(accepted.muzzle_fx_visible);
        assert!(accepted.hitmarker_visible);

        // The terminal accept pruned the record, so a late reject finds nothing
        // and cannot undo the accepted shot's cooldown or presentation.
        assert!(
            predicted
                .apply_verdict(&mut registry, test_shot_id(0xA), false, false)
                .is_none()
        );
        assert!(predicted.get(test_shot_id(0xA)).is_none());
        assert!(approx_eq(client_cooldown(&registry, weapon), 100.0));
    }

    // Regression: retiring HIT feedback hid a later FIRE denial from the live flight.
    #[test]
    fn hit_refusal_retires_feedback_keeps_flight_and_later_fire_denial_retracts_it() {
        let resolution = ClientFireResolution {
            world_contacts: Vec::new(),
            client_tick: 9,
            hits: vec![LocalHitRecord {
                normal: Vec3::Y,
                target: EntityId::from_raw(2),
                point: Vec3::ZERO,
                zone: None,
            }],
            projectile_launch: None,
        };
        let (mut registry, weapon) = client_weapon_registry();
        set_client_cooldown(&mut registry, weapon, 100.0);
        let owner = registry.spawn(Transform::default());
        let spawn = |registry: &mut EntityRegistry, shot_id| {
            let entity = registry.spawn(Transform::default());
            registry
                .set_component(
                    entity,
                    ProjectileComponent {
                        source_sounds: None,
                        source_action: None,
                        source_shot: None,
                        predicted_visible: false,
                        knockback_impulse: [0.0; 3],
                        direction: Vec3::NEG_Z.to_array(),
                        speed: 10.0,
                        radius: 0.1,
                        remaining_range: 100.0,
                        remaining_lifetime: 10.0,
                        damage: 25.0,
                        credit_source: "weapon.refusal".to_string(),
                        owner_pawn: owner,
                        owner_weapon: weapon,
                        spawned: false,
                        predicted_shot_id: Some(shot_id),
                        elapsed_flight_age: 0.0,
                        flipbook_active: false,
                        impact_light: None,
                        splash: None,
                        source_weapon: None,
                        activation: None,
                    },
                )
                .unwrap();
            entity
        };
        let refused_id = test_shot_id(0xA);
        let other_id = test_shot_id(0xB);
        let refused_flight = spawn(&mut registry, refused_id);
        let other_flight = spawn(&mut registry, other_id);
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(
            refused_id,
            weapon,
            &resolution,
            25.0,
            100.0,
            ClientPullPresentation::Fire,
        );
        let refused = predicted.refuse_hit(refused_id).unwrap();
        assert!(
            refused.muzzle_fx_visible,
            "HIT refusal does not retract FIRE feedback"
        );
        assert!(!refused.hitmarker_visible);
        assert_eq!(
            refused.status,
            PredictedShotStatus::Pending,
            "FIRE remains undecided"
        );
        assert!(predicted.get(refused_id).is_none());
        assert!(registry.exists(refused_flight));
        assert!(
            !registry
                .get_component::<ProjectileComponent>(refused_flight)
                .unwrap()
                .predicted_visible
        );
        assert!(approx_eq(client_cooldown(&registry, weapon), 100.0));
        assert!(
            predicted.refuse_hit(refused_id).is_none(),
            "duplicate refusal is inert"
        );
        predicted.mark_hitmarker(refused_id);
        assert!(
            predicted.get(refused_id).is_none(),
            "a later contact cannot restore refused feedback"
        );
        assert!(
            predicted
                .apply_verdict(&mut registry, refused_id, false, false)
                .is_none()
        );
        assert!(
            !registry.exists(refused_flight),
            "real FIRE denial removes flight after feedback retired"
        );
        assert!(registry.exists(other_flight));
        assert!(approx_eq(client_cooldown(&registry, weapon), 100.0));
        predicted.clear();
        predicted.predict(
            other_id,
            weapon,
            &resolution,
            25.0,
            100.0,
            ClientPullPresentation::Fire,
        );
        assert!(
            predicted.refuse_hit(refused_id).is_none(),
            "a stale refusal after lifecycle clear cannot hit a new shot"
        );
        assert!(predicted.get(other_id).is_some());
        assert!(registry.exists(other_flight));
    }

    #[test]
    fn stale_reject_does_not_overwrite_fresh_authoritative_cooldown() {
        let resolution = ClientFireResolution {
            world_contacts: Vec::new(),
            client_tick: 9,
            hits: vec![LocalHitRecord {
                normal: Vec3::Y,
                target: EntityId::from_raw(2),
                point: Vec3::ZERO,
                zone: None,
            }],
            projectile_launch: None,
        };
        let (mut registry, weapon) = client_weapon_registry();
        set_client_cooldown(&mut registry, weapon, 100.0);
        let mut predicted = ClientPredictedShots::new();
        predicted.predict(
            test_shot_id(0xA),
            weapon,
            &resolution,
            25.0,
            100.0,
            ClientPullPresentation::Fire,
        );

        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        predicted.reconcile_cooldown(weapon, &mut component, 12.0);
        registry.set_component(weapon, component).unwrap();
        let record = predicted
            .apply_verdict(&mut registry, test_shot_id(0xA), false, false)
            .expect("reject should match");

        assert_eq!(record.status, PredictedShotStatus::Rejected);
        assert!(!record.muzzle_fx_visible);
        assert!(!record.hitmarker_visible);
        assert!(
            approx_eq(client_cooldown(&registry, weapon), 12.0),
            "fresh owner-private cooldown must win over stale rollback"
        );
        assert!(
            predicted.get(test_shot_id(0xA)).is_none(),
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

#[cfg(test)]
fn test_shot_id(tick: u32) -> postretro_foundation::ShotId {
    postretro_foundation::ShotId::from_parts(
        4,
        tick,
        postretro_foundation::ActivationLane::Primary,
        0,
    )
}

#[cfg(test)]
mod activation_authoring_tests {
    #[test]
    fn compiled_charge_scale_evaluation_and_final_validation_allocate_nothing() {
        use postretro_foundation::{
            ActivationStepDescriptor, ActivationTrigger, CompiledActivation, IrNode, IrValue,
            NumberOrIr, ShotResourceCost, ShotScaleDescriptor, ShotScaleInputs,
            WeaponActivationDescriptor,
        };
        let expression = NumberOrIr::Ir(IrNode::Add {
            a: Box::new(IrNode::Mul {
                a: Box::new(IrNode::Input {
                    name: "charge".into(),
                    owner: None,
                }),
                b: Box::new(IrNode::Const {
                    value: IrValue::Number(5.0),
                }),
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(1.0),
            }),
        });
        let mut descriptor = WeaponActivationDescriptor::single(ActivationTrigger::Press, 400.0);
        descriptor.steps = vec![ActivationStepDescriptor::Shot {
            scale: Box::new(ShotScaleDescriptor {
                damage: expression.clone(),
                resource_cost: expression,
                ..Default::default()
            }),
        }];
        let compiled = CompiledActivation::compile(&descriptor, "fixture").unwrap();
        let base = ShotScaleInputs {
            damage: 10.0,
            range: 96.0,
            projectile_speed: Some(40.0),
            projectile_radius: Some(0.5),
            projectile_size: Some(1.5),
            knockback_speed: None,
            splash_knockback_speed: None,
            resource_cost: ShotResourceCost::Cell(5.0),
        };
        let _ = compiled
            .resolve_scales(0, 1.0)
            .unwrap()
            .apply(base)
            .unwrap();
        let snapshot = crate::alloc_probe::AllocSnapshot::arm();
        let values = compiled
            .resolve_scales(0, 1.0)
            .unwrap()
            .apply(base)
            .unwrap();
        let allocations = snapshot.allocs_since();
        assert_eq!(values.damage, 60.0);
        assert_eq!(values.resource_cost, ShotResourceCost::Cell(30.0));
        assert_eq!(allocations, 0);
    }
}
