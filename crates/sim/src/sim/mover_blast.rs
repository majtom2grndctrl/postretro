// Projectiles and blasts that start inside a mover's posed hull.
// See: context/lib/entity_model.md §Components (Splash) · context/lib/networking.md §Combat authority

use glam::Vec3;

use crate::collision::moving::CombinedCollisionWorld;
use crate::collision::shot_queries::{mover_at_sphere, mover_surface_distance};
use crate::weapon::{ActivationOutcome, DamagePayload, WeaponImpact};

use super::projectile_stage::{
    WORLD_IMPACT_SPLASH_OCCLUSION_EPSILON, projectile_splash_occlusion_origin,
};

/// Far enough to leave any mover hull from inside it.
const HULL_EXIT_REACH: f32 = 1.0e4;

/// A projectile that starts its step touching or inside a mover, because a
/// door closed or snapped onto it, is struck where it is. A sweep from inside
/// the hull would meet its far face instead. The contact faces back along the
/// flight, the side the projectile came from.
pub(crate) fn enclosed_step_impact(
    world: &CombinedCollisionWorld<'_>,
    position: Vec3,
    direction: Vec3,
    radius: f32,
    damage: f32,
) -> Option<WeaponImpact> {
    mover_at_sphere(world, position, radius)?;
    Some(WeaponImpact {
        point: position,
        normal: -direction,
        target: None,
        zone: None,
        outcome: ActivationOutcome::Hit(DamagePayload {
            amount: damage,
            impulse: Vec3::ZERO,
            damage_type: None,
        }),
    })
}

/// Where a blast's splash sightline starts. A centre inside a mover's hull
/// sees out only through the face its reverse flight leaves by, so the side
/// the projectile came from takes the blast and the mover shields the far
/// side. Every other centre keeps the projectile-surface rule. Damage and
/// falloff stay centred on the impact point either way.
pub fn blast_sightline_origin(
    world: &CombinedCollisionWorld<'_>,
    impact: &WeaponImpact,
    projectile_radius: f32,
    flight_direction: Vec3,
) -> Vec3 {
    let back = -flight_direction.normalize_or_zero();
    if back != Vec3::ZERO
        && let Some(mover) = mover_at_sphere(world, impact.point, 0.0)
        && let Some(exit) =
            mover_surface_distance(world, mover, impact.point, back, HULL_EXIT_REACH)
    {
        let origin = impact.point + back * (exit + WORLD_IMPACT_SPLASH_OCCLUSION_EPSILON);
        if origin.is_finite() {
            return origin;
        }
    }
    projectile_splash_occlusion_origin(projectile_radius, impact)
}
