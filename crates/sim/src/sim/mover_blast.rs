// Projectiles and blasts that start inside a mover's posed hull.
// See: context/lib/entity_model.md §Components (Splash) · context/lib/networking.md §Combat authority

use glam::Vec3;

use crate::collision::moving::CombinedCollisionWorld;
use crate::collision::shot_queries::{mover_enclosing_point, mover_surface_distance};
use crate::weapon::{ActivationOutcome, DamagePayload, WeaponImpact};

use super::projectile_stage::{
    WORLD_IMPACT_SPLASH_OCCLUSION_EPSILON, projectile_splash_occlusion_origin,
};

/// Far enough to leave any mover hull from inside it.
const HULL_EXIT_REACH: f32 = 1.0e4;

/// Faces a sightline may cross leaving nested hulls: a multi-brush mover's
/// internal faces, and overlapping movers such as bi-parting doors.
const MAX_HULL_EXITS: usize = 8;

/// A projectile whose centre starts its step inside a mover, because a door
/// closed or snapped onto it, is struck where it is. A sweep from inside the
/// hull would meet its far face instead. A projectile only overlapping a
/// surface is left to the sweep, which flies it on as a static wall would
/// unless it moves into the surface. The contact faces back along the flight,
/// the side the projectile came from.
pub(crate) fn enclosed_step_impact(
    world: &CombinedCollisionWorld<'_>,
    position: Vec3,
    direction: Vec3,
    damage: f32,
) -> Option<WeaponImpact> {
    mover_enclosing_point(world, position)?;
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
/// sees out only along its reverse flight, so the side the projectile came
/// from takes the blast and the mover shields the far side. Every other
/// centre, and one that cannot get out, keeps the projectile-surface rule.
/// Damage and falloff stay centred on the impact point either way.
pub fn blast_sightline_origin(
    world: &CombinedCollisionWorld<'_>,
    impact: &WeaponImpact,
    projectile_radius: f32,
    flight_direction: Vec3,
) -> Vec3 {
    let back = -flight_direction.normalize_or_zero();
    if back != Vec3::ZERO
        && let Some(origin) = exit_enclosing_movers(world, impact.point, back)
    {
        return origin;
    }
    projectile_splash_occlusion_origin(projectile_radius, impact)
}

/// Walk back along `back` from an enclosed `point`, one face at a time, until
/// the walk stands outside every mover. `None` when `point` is not enclosed,
/// or the walk finds no exit within [`MAX_HULL_EXITS`] faces. Static geometry
/// behind the mover is out of scope: it has no cheap exact enclosure test.
fn exit_enclosing_movers(
    world: &CombinedCollisionWorld<'_>,
    point: Vec3,
    back: Vec3,
) -> Option<Vec3> {
    let mut mover = mover_enclosing_point(world, point)?;
    let mut origin = point;
    for _ in 0..MAX_HULL_EXITS {
        let exit = mover_surface_distance(world, mover, origin, back, HULL_EXIT_REACH)?;
        origin += back * (exit + WORLD_IMPACT_SPLASH_OCCLUSION_EPSILON);
        if !origin.is_finite() {
            return None;
        }
        match mover_enclosing_point(world, origin) {
            None => return Some(origin),
            Some(next) => mover = next,
        }
    }
    None
}
