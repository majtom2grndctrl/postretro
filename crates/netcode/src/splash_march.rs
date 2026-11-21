// Host march of a connected client's splash shot: one world-contact step per
// host tick against that tick's poses, the on-path credit rule, and release.
// See: context/lib/networking.md §Combat authority (march and credit)

use glam::Vec3;
use postretro_combat_model::AuthorizedShot;
use postretro_net::wire;

use crate::collision::moving::CombinedCollisionWorld;
use crate::collision::shot_queries::{cast_sphere_exact_combined, mover_at_sphere};

/// Travel this close to the reach counts as having reached it.
const REACH_EPSILON: f32 = 1.0e-4;

/// One open client splash shot's march along its frozen path.
#[derive(Debug, Clone)]
pub struct SplashMarch {
    pub(crate) shot: AuthorizedShot,
    pub(crate) owner_client_id: u64,
    /// Distance along the frozen path the march has cleared.
    pub(crate) travelled: f32,
    /// The host's own first world contact; the march stops there.
    pub(crate) contact: Option<MarchContact>,
    /// Travel reached range or lifetime without a contact.
    pub(crate) exhausted: bool,
    /// The bound declaration, from the tick it arrives.
    pub(crate) declaration: Option<BoundDeclaration>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MarchContact {
    pub(crate) distance: f32,
    pub(crate) point: Vec3,
    pub(crate) normal: Vec3,
}

/// What a splash declaration's first valid record says, judged once on arrival.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BoundDeclaration {
    /// No valid record: the client saw no impact.
    Empty,
    /// The first valid record lies off the host path: the host's own march
    /// contact decides.
    OffPath,
    /// The first valid record, on the host path `distance` along it.
    OnPath {
        distance: f32,
        record: wire::HitRecord,
    },
}

/// Where a march's blast lands, or that none does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MarchOutcome {
    /// The deciding point is not reached yet, or the declaration is not here.
    Wait,
    /// The host's own world contact decides, with its own surface.
    HostContact(MarchContact),
    /// The client's declared on-path point decides.
    Declared(wire::HitRecord),
    /// No blast: FIRE stands, the hit is refused.
    Refused,
}

impl SplashMarch {
    pub(crate) fn new(shot: AuthorizedShot, owner_client_id: u64) -> Option<Self> {
        let flight = Flight::of(&shot)?;
        // An unusable flight is born exhausted: it releases as refused.
        let exhausted = !flight.valid();
        Some(Self {
            shot,
            owner_client_id,
            travelled: 0.0,
            contact: None,
            exhausted,
            declaration: None,
        })
    }

    /// Whether the march has stopped casting: it found its contact or ran out.
    pub(crate) fn resolved(&self) -> bool {
        self.contact.is_some() || self.exhausted
    }

    /// Where the march's cursor stands on its frozen path.
    pub(crate) fn cursor(&self) -> Vec3 {
        match self.contact {
            Some(contact) => contact.point,
            None => {
                self.shot.fire_origin
                    + self.shot.projectile_direction.unwrap_or(Vec3::ZERO) * self.travelled
            }
        }
    }

    /// One host tick of travel against this tick's world, testing world
    /// contacts only. Never steps on the FIRE tick, as a host-spawned
    /// projectile does not.
    pub(crate) fn step(&mut self, world: &CombinedCollisionWorld<'_>, current_tick: u32) {
        if self.resolved() || current_tick == self.shot.fire_tick {
            return;
        }
        let Some(flight) = Flight::of(&self.shot) else {
            self.exhausted = true;
            return;
        };
        let position = self.shot.fire_origin + flight.direction * self.travelled;
        // A sweep from inside a hull meets its far face: an enclosed cursor is
        // struck where it is, before any expiry.
        if mover_at_sphere(world, position, flight.radius).is_some() {
            self.contact = Some(MarchContact {
                distance: self.travelled,
                point: position,
                normal: -flight.direction,
            });
            return;
        }
        let remaining = (flight.reach - self.travelled).max(0.0);
        let segment = flight.step.min(remaining);
        if let Some(hit) =
            cast_sphere_exact_combined(world, position, flight.radius, flight.direction, segment)
        {
            let toi = hit.time_of_impact.max(0.0);
            self.contact = Some(MarchContact {
                distance: self.travelled + toi,
                point: position + flight.direction * toi,
                normal: hit.normal,
            });
            return;
        }
        self.travelled += segment;
        if self.travelled >= flight.reach - REACH_EPSILON {
            self.exhausted = true;
        }
    }

    /// Judge the declaration on its first valid record, once.
    pub(crate) fn bind(&mut self, declaration: &wire::HitDeclaration) {
        if self.declaration.is_some() {
            return;
        }
        let first_valid = declaration
            .records
            .iter()
            .take(self.shot.pellet_count)
            .find(|record| crate::valid_projectile_contact(&self.shot, record));
        self.declaration = Some(match first_valid {
            None => BoundDeclaration::Empty,
            Some(record) => match self.on_path_distance(Vec3::from_array(record.point)) {
                Some(distance) => BoundDeclaration::OnPath {
                    distance,
                    record: record.clone(),
                },
                None => BoundDeclaration::OffPath,
            },
        });
    }

    /// Distance along the frozen path to `point`, when it lies within the
    /// frozen on-path tolerance of the path.
    fn on_path_distance(&self, point: Vec3) -> Option<f32> {
        let flight = Flight::of(&self.shot)?;
        let tolerance = self.shot.splash_on_path_tolerance?;
        let along = (point - self.shot.fire_origin).dot(flight.direction);
        let nearest =
            self.shot.fire_origin + flight.direction * along.clamp(0.0, flight.reach.max(0.0));
        // The distance is not clamped to the reach: a point past it is one the
        // march never covers.
        (point.distance(nearest) <= tolerance).then_some(along.max(0.0))
    }

    /// The deciding point, once the march and the declaration both allow it.
    pub(crate) fn outcome(&self) -> MarchOutcome {
        let Some(declaration) = &self.declaration else {
            return MarchOutcome::Wait;
        };
        match declaration {
            BoundDeclaration::Empty => MarchOutcome::Refused,
            BoundDeclaration::OffPath => match (self.contact, self.exhausted) {
                (Some(contact), _) => MarchOutcome::HostContact(contact),
                (None, true) => MarchOutcome::Refused,
                (None, false) => MarchOutcome::Wait,
            },
            BoundDeclaration::OnPath { distance, record } => {
                let world_record = record.target == crate::PRESENTATION_CONTACT_TARGET;
                let at_point = self.shot.splash_on_path_tolerance.unwrap_or(0.0);
                match self.contact {
                    // A host contact nearer than the declared point wins with
                    // its own surface; so does one at a declared world point.
                    Some(contact)
                        if contact.distance < *distance
                            || (world_record && contact.distance <= distance + at_point) =>
                    {
                        MarchOutcome::HostContact(contact)
                    }
                    // A contact beyond it means the march covered it.
                    Some(_) => MarchOutcome::Declared(record.clone()),
                    None if self.travelled >= *distance => MarchOutcome::Declared(record.clone()),
                    None if self.exhausted => MarchOutcome::Refused,
                    None => MarchOutcome::Wait,
                }
            }
        }
    }
}

/// The frozen flight facts a march steps with.
struct Flight {
    direction: Vec3,
    radius: f32,
    /// Distance per host tick.
    step: f32,
    /// Frozen range, capped by frozen lifetime.
    reach: f32,
}

impl Flight {
    fn of(shot: &AuthorizedShot) -> Option<Self> {
        let direction = shot.projectile_direction?;
        let speed = shot.projectile_speed?;
        let tick_seconds = shot.projectile_tick_seconds?;
        let lifetime = shot.projectile_lifetime_seconds?;
        let radius = shot.projectile_radius?;
        Some(Self {
            direction,
            radius,
            step: speed * tick_seconds,
            reach: shot.range.min(speed * lifetime),
        })
    }

    fn valid(&self) -> bool {
        self.direction.is_finite()
            && (self.direction.length_squared() - 1.0).abs() <= 1.0e-3
            && self.radius.is_finite()
            && self.radius >= 0.0
            && self.step.is_finite()
            && self.step > 0.0
            && self.reach.is_finite()
            && self.reach >= 0.0
    }
}

/// One march's release: who fired it, and what the host settled.
pub(crate) struct Released {
    pub(crate) client_id: u64,
    pub(crate) shot_id: postretro_combat_model::ShotId,
    pub(crate) result: crate::HitDeclarationResult,
}

/// Step every open march one host tick against this tick's world, then
/// release each that reached its deciding point with its declaration in
/// hand: the authoritative blast, then its verdict and presentation. Runs in
/// the Projectiles stage, before host projectiles advance and before AI.
#[allow(clippy::too_many_arguments)]
pub fn host_step_splash_marches(
    server: &mut crate::NetServer,
    registry: &mut postretro_entities::EntityRegistry,
    world: &CombinedCollisionWorld<'_>,
    hit_zone_store: &crate::scripting_systems::hit_zones::HitZoneStore,
    allocator: &crate::NetworkIdAllocator,
    open_shots: &mut crate::OpenAuthorizedShots,
    current_tick: u32,
    mut on_impact: impl FnMut(&mut postretro_entities::EntityRegistry),
    mut on_projectile_contact: impl FnMut(postretro_combat_model::ShotId, Vec3),
    mut on_remote_impact: impl FnMut(crate::emission::WeaponEmission),
) -> bool {
    let mut accepted_any_hit = false;
    for released in step_and_release(
        registry,
        world,
        hit_zone_store,
        allocator,
        open_shots,
        current_tick,
        &mut on_impact,
    ) {
        accepted_any_hit |= crate::finish_hit_declaration(
            server,
            registry,
            allocator,
            released.client_id,
            released.shot_id,
            true,
            released.result,
            &mut on_projectile_contact,
            &mut on_remote_impact,
        );
    }
    accepted_any_hit
}

/// The march half of [`host_step_splash_marches`]: each released blast's
/// result, in release order, without sending anything.
pub(crate) fn step_and_release(
    registry: &mut postretro_entities::EntityRegistry,
    world: &CombinedCollisionWorld<'_>,
    hit_zone_store: &crate::scripting_systems::hit_zones::HitZoneStore,
    allocator: &crate::NetworkIdAllocator,
    open_shots: &mut crate::OpenAuthorizedShots,
    current_tick: u32,
    on_impact: &mut impl FnMut(&mut postretro_entities::EntityRegistry),
) -> Vec<Released> {
    if open_shots.marches.is_empty() {
        return Vec::new();
    }
    // One fixed order, so blasts on one tick credit kills the same way on
    // every run.
    let mut ids: Vec<_> = open_shots.marches.keys().copied().collect();
    ids.sort_unstable_by_key(|id| {
        let wire = crate::wire_convert::shot_id_to_wire(*id);
        (wire.pawn, wire.start_tick, wire.lane, wire.ordinal)
    });
    let mut released = Vec::new();
    for id in ids {
        let Some(march) = open_shots.marches.get_mut(&id) else {
            continue;
        };
        // Nothing steps or lands on the FIRE tick, as a host rocket's spawn
        // grace holds it at its launch.
        if current_tick == march.shot.fire_tick {
            continue;
        }
        march.step(world, current_tick);
        let outcome = march.outcome();
        if outcome == MarchOutcome::Wait {
            continue;
        }
        let Some(march) = open_shots.marches.remove(&id) else {
            continue;
        };
        let result = release(
            registry,
            world,
            hit_zone_store,
            allocator,
            &march.shot,
            outcome,
            on_impact,
        );
        released.push(Released {
            client_id: march.owner_client_id,
            shot_id: id,
            result,
        });
    }
    released
}

/// The one authoritative blast at the deciding point. Occlusion and the
/// entity set read this release tick's world.
fn release(
    registry: &mut postretro_entities::EntityRegistry,
    world: &CombinedCollisionWorld<'_>,
    hit_zone_store: &crate::scripting_systems::hit_zones::HitZoneStore,
    allocator: &crate::NetworkIdAllocator,
    shot: &AuthorizedShot,
    outcome: MarchOutcome,
    on_impact: &mut impl FnMut(&mut postretro_entities::EntityRegistry),
) -> crate::HitDeclarationResult {
    use crate::weapon::{ActivationOutcome, DamagePayload, WeaponImpact};

    let refused = crate::HitDeclarationResult {
        sounds: shot.sounds.clone(),
        action: shot.action.clone(),
        fire_accepted: true,
        hit_accepted: false,
        projectile_contact: None,
        contacts: Vec::new(),
        weapon: shot
            .source_weapon
            .clone()
            .or_else(|| crate::emission::descriptor_name(registry, shot.weapon)),
        deferred: false,
    };
    let Some(splash) = shot.splash.as_ref() else {
        return refused;
    };
    let hit = |point: Vec3, normal: Vec3, target, zone| WeaponImpact {
        point,
        normal,
        target,
        zone,
        outcome: ActivationOutcome::Hit(DamagePayload {
            amount: shot.damage,
            impulse: Vec3::ZERO,
            damage_type: None,
        }),
    };
    let impact = match outcome {
        MarchOutcome::Wait | MarchOutcome::Refused => return refused,
        MarchOutcome::HostContact(contact) => hit(contact.point, contact.normal, None, None),
        MarchOutcome::Declared(record) => {
            // A declared enemy still live at release is the impact target; one
            // killed or removed since leaves the declared normal alone.
            let target = (record.target != crate::PRESENTATION_CONTACT_TARGET)
                .then(|| {
                    allocator.entity_for_network_id(postretro_net::wire::NetworkId(record.target))
                })
                .flatten()
                .filter(|target| declared_target_live(registry, *target));
            let zone = target.and(record.zone.clone());
            hit(
                Vec3::from_array(record.point),
                crate::declared_normal(&record),
                target,
                zone,
            )
        }
    };
    let direction = shot.projectile_direction.unwrap_or(Vec3::ZERO);
    // Splash replaces direct health damage, but a declared enemy still takes
    // the weapon's direct push, as from a host rocket.
    if impact.target.is_some()
        && let Some(knockback) = shot.knockback
    {
        let mut direct_push = impact.clone();
        direct_push.outcome = ActivationOutcome::Hit(DamagePayload {
            amount: 0.0,
            impulse: postretro_foundation::knockback_impulse(
                knockback.speed,
                knockback.upward_bias,
                direction,
            ),
            damage_type: None,
        });
        crate::sim::apply_authorized_shot_impact_damage(
            registry,
            shot.weapon,
            Some(shot.pawn),
            Some(shot.pawn_life),
            &direct_push,
            shot.credit_source.clone(),
            0.0,
        );
    }
    let hit_accepted = crate::sim::splash::emit_shot_splash_damage(
        registry,
        hit_zone_store,
        world,
        impact.point,
        crate::sim::blast_sightline_origin(
            world,
            &impact,
            shot.projectile_radius.unwrap_or(0.0),
            direction,
        ),
        splash,
        shot.damage,
        shot.weapon,
        shot.pawn,
        Some(shot.pawn_life),
        true,
        shot.credit_source.clone(),
        shot.damage_type,
        on_impact,
    );
    crate::HitDeclarationResult {
        hit_accepted,
        projectile_contact: Some(impact.point),
        contacts: vec![crate::emission::ImpactContact::new(
            impact.point,
            impact.normal,
            impact.target,
        )],
        ..refused
    }
}

fn declared_target_live(
    registry: &postretro_entities::EntityRegistry,
    target: postretro_entities::EntityId,
) -> bool {
    registry.exists(target)
        && !registry.is_inventory_owned(target)
        && !crate::scripting_systems::health::is_terminally_committed_to_removal(registry, target)
        && crate::scripting_systems::health::is_damage_target_eligible(registry, target)
}
