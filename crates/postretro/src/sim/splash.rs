// Caller-agnostic radial damage query and impact-composed splash emission.
// See: context/lib/entity_model.md §7 · context/plans/in-progress/E16--aoe-splash-damage

use glam::Vec3;
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::SplashDescriptor;
use postretro_render_data::cone_frustum::Aabb;

use crate::collision::CollisionWorld;
use crate::scripting_systems::health::is_damage_target_eligible;
use crate::scripting_systems::hit_zones::{
    HitZoneStore, damageable_volume, for_each_hittable_candidate,
};
use crate::sim::weapon_stage::apply_authorized_weapon_impact_damage;
use crate::weapon::{ActivationOutcome, DamagePayload, WeaponImpact};

/// A live damageable entity whose broad-phase volume intersects a sphere.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SphereEntity {
    pub(crate) entity: EntityId,
    /// Nearest point on the candidate's damageable volume. Task 2 uses this as
    /// the static-world occlusion segment endpoint.
    pub(crate) nearest_point: Vec3,
    pub(crate) distance: f32,
}

/// Return every live, non-excluded damageable entity whose broad-phase volume
/// intersects the sphere at `center`.
///
/// `occlude` deliberately belongs to the stable query contract now. Task 2
/// supplies its static-world line-of-sight filter; Task 1 leaves candidate
/// membership unaffected by the optional collider.
pub(crate) fn entities_in_sphere(
    registry: &EntityRegistry,
    hit_zone_store: &HitZoneStore,
    center: Vec3,
    radius: f32,
    exclude: impl Fn(EntityId) -> bool,
    _occlude: Option<&CollisionWorld>,
) -> Vec<SphereEntity> {
    if !center.is_finite() || !radius.is_finite() || radius <= 0.0 {
        return Vec::new();
    }

    let mut entities = Vec::new();
    for_each_hittable_candidate(registry, |entity| {
        if exclude(entity) || !is_damage_target_eligible(registry, entity) {
            return;
        }
        let Some(volume) = damageable_volume(registry, hit_zone_store, entity) else {
            return;
        };
        let nearest_point = closest_point_on_aabb(center, volume);
        let distance = center.distance(nearest_point);
        if distance <= radius {
            entities.push(SphereEntity {
                entity,
                nearest_point,
                distance,
            });
        }
    });
    entities
}

/// Closest point on a world-aligned AABB to `point`.
pub(crate) fn closest_point_on_aabb(point: Vec3, aabb: Aabb) -> Vec3 {
    point.clamp(aabb.min, aabb.max)
}

/// Linear radial-damage interpolation from full center damage to the authored
/// edge floor. Inputs are descriptor-validated at load; invalid inputs yield
/// no dispatch defensively for directly-constructed runtime components.
pub(crate) fn splash_damage_amount(
    damage: f32,
    radius: f32,
    min_fraction: f32,
    distance: f32,
) -> f32 {
    if !damage.is_finite()
        || !radius.is_finite()
        || radius <= 0.0
        || !min_fraction.is_finite()
        || !distance.is_finite()
    {
        return 0.0;
    }
    let progress = (distance / radius).clamp(0.0, 1.0);
    damage * (1.0 + (min_fraction - 1.0) * progress)
}

/// Apply one complete blast through the ordinary weapon-impact damage
/// chokepoint. Candidate collection happens before any damage so one target's
/// health change cannot alter the membership or falloff of another target in
/// the same blast.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_splash_damage(
    registry: &mut EntityRegistry,
    hit_zone_store: &HitZoneStore,
    collision_world: &CollisionWorld,
    center: Vec3,
    splash: &SplashDescriptor,
    damage: f32,
    owner_weapon: EntityId,
    owner_pawn: EntityId,
    credit_source: String,
    on_impact: &mut impl FnMut(&mut EntityRegistry),
) {
    // Task 2 adds the self-damage exclusion policy. The owner remains a normal
    // candidate in Task 1, matching the descriptor's `selfDamage: true`
    // default while keeping this caller signature stable.
    let targets = entities_in_sphere(
        registry,
        hit_zone_store,
        center,
        splash.radius,
        |_| false,
        Some(collision_world),
    );
    let attacker = registry.exists(owner_pawn).then_some(owner_pawn);
    let mut dispatched = false;

    for target in targets {
        let amount =
            splash_damage_amount(damage, splash.radius, splash.min_fraction, target.distance);
        // A zero edge floor deliberately produces no hit or impact dispatch.
        if !amount.is_finite() || amount <= 0.0 {
            continue;
        }
        let impact = WeaponImpact {
            point: center,
            normal: Vec3::Y,
            target: Some(target.entity),
            zone: None,
            outcome: ActivationOutcome::Hit(DamagePayload { amount }),
        };
        apply_authorized_weapon_impact_damage(
            registry,
            owner_weapon,
            attacker,
            &impact,
            credit_source.clone(),
            amount,
        );
        dispatched = true;
    }

    // Impact-policy evaluation is intentionally a single post-blast drain: a
    // policy observes every health result from this blast before it runs.
    if dispatched {
        on_impact(registry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::Transform;
    use postretro_entities::components::health::{HealthComponent, Hitbox};

    fn spawn_target(registry: &mut EntityRegistry, position: Vec3, half_extents: Vec3) -> EntityId {
        let target = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry
            .set_component(
                target,
                HealthComponent {
                    max: 100.0,
                    current: 100.0,
                    hitbox: Some(Hitbox {
                        half_extents,
                        offset: Vec3::ZERO,
                    }),
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .expect("target health attaches");
        target
    }

    #[test]
    fn entities_in_sphere_enumerates_intersecting_live_damageable_volumes() {
        let mut registry = EntityRegistry::new();
        let enclosing = spawn_target(&mut registry, Vec3::ZERO, Vec3::splat(0.25));
        let edge = spawn_target(&mut registry, Vec3::new(5.25, 0.0, 0.0), Vec3::splat(0.25));
        let outside = spawn_target(&mut registry, Vec3::new(5.26, 0.0, 0.0), Vec3::splat(0.25));
        let zones = HitZoneStore::new();

        let hits = entities_in_sphere(&registry, &zones, Vec3::ZERO, 5.0, |_| false, None);
        assert_eq!(hits.len(), 2);
        assert!(
            hits.iter()
                .any(|hit| hit.entity == enclosing && hit.distance <= 1.0e-6)
        );
        assert!(
            hits.iter()
                .any(|hit| hit.entity == edge && (hit.distance - 5.0).abs() <= 1.0e-6)
        );
        assert!(!hits.iter().any(|hit| hit.entity == outside));

        let empty = entities_in_sphere(
            &registry,
            &zones,
            Vec3::new(50.0, 0.0, 0.0),
            1.0,
            |_| false,
            None,
        );
        assert!(empty.is_empty());
    }

    #[test]
    fn splash_damage_amount_interpolates_center_midpoint_and_edge() {
        assert!((splash_damage_amount(100.0, 10.0, 0.2, 0.0) - 100.0).abs() <= 1.0e-5);
        assert!((splash_damage_amount(100.0, 10.0, 0.2, 5.0) - 60.0).abs() <= 1.0e-5);
        assert!((splash_damage_amount(100.0, 10.0, 0.2, 10.0) - 20.0).abs() <= 1.0e-5);
    }

    #[test]
    fn splash_emitter_routes_each_nonzero_target_through_credit_and_one_post_blast_drain() {
        let mut registry = EntityRegistry::new();
        let owner = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let center = spawn_target(&mut registry, Vec3::ZERO, Vec3::splat(0.25));
        let midpoint = spawn_target(&mut registry, Vec3::new(2.75, 0.0, 0.0), Vec3::splat(0.25));
        let edge = spawn_target(&mut registry, Vec3::new(5.25, 0.0, 0.0), Vec3::splat(0.25));
        let splash = SplashDescriptor {
            radius: 5.0,
            min_fraction: 0.0,
            self_damage: true,
        };
        let zones = HitZoneStore::new();
        let world = CollisionWorld::default();
        let mut drains = 0;

        emit_splash_damage(
            &mut registry,
            &zones,
            &world,
            Vec3::ZERO,
            &splash,
            100.0,
            weapon,
            owner,
            "weapon.splash-test".to_string(),
            &mut |_| drains += 1,
        );

        assert_eq!(drains, 1);
        for (target, expected_damage) in [(center, 100.0), (midpoint, 50.0)] {
            let health = registry
                .get_component::<HealthComponent>(target)
                .expect("target remains live");
            assert!((health.current - (100.0 - expected_damage)).abs() <= 1.0e-6);
            let entry = health
                .contributor_ledger
                .entries()
                .first()
                .expect("damage uses the contributor ledger chokepoint");
            assert_eq!(entry.source_id, "weapon.splash-test");
            assert_eq!(entry.last_weapon, Some(weapon));
            assert_eq!(entry.last_attacker, Some(owner));
            assert_eq!(entry.hit_count, 1);
        }
        let edge_health = registry
            .get_component::<HealthComponent>(edge)
            .expect("edge target remains live");
        assert_eq!(edge_health.current, 100.0);
        assert!(edge_health.contributor_ledger.entries().is_empty());
    }
}
