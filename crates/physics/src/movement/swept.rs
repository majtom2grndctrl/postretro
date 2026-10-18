// Swept own-movement primitives shared by the player substrate and the agent controller.
// See: context/lib/movement.md §1, §7

use glam::Vec3;

use crate::collision::moving::{
    CombinedCollisionWorld, cast_capsule_combined_parry, cast_capsule_combined_to,
    cast_ray_combined_parry,
};
use crate::collision::{CollisionCapsule, OVERLAP_TOLERANCE, SKIN_DISTANCE};
use crate::movement::mover_carry::ground_ref_from_hit;
use postretro_foundation::GroundRef;

/// A controller's step tuning for [`step_up`] and [`snap_to_floor`]. The
/// primitives are geometry; when to call them and with which values stays each
/// controller's policy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepParams {
    /// Tallest riser climbed in one tick. Zero or less disables step-up and
    /// snap-to-floor.
    pub step_height: f32,
    /// Lift past `step_height` so the capsule clears the step's top edge. Must
    /// exceed `SKIN_DISTANCE`.
    pub lift_margin: f32,
    /// Minimum contact normal `y` that counts as floor.
    pub cos_walkable: f32,
}

/// Move the capsule by `delta`, stopping [`OVERLAP_TOLERANCE`] short of the
/// first surface on the way: the one way own movement changes position outside
/// a slide sweep. A body that starts clear therefore ends clear. Movers count,
/// like every movement cast through `collision`.
pub fn swept_move(
    collision: &CombinedCollisionWorld<'_>,
    position: Vec3,
    capsule: CollisionCapsule,
    delta: Vec3,
) -> Vec3 {
    let distance = delta.length();
    // Also rejects a non-finite delta, which has no direction to sweep along.
    if !(distance > 0.0 && distance.is_finite()) {
        return position;
    }
    let direction = delta / distance;
    let hit = cast_capsule_combined_to(
        collision.static_world,
        collision.movers,
        collision.poses,
        position,
        &capsule.parry(),
        direction,
        distance,
        OVERLAP_TOLERANCE,
    );
    match hit {
        Some(hit) => position + direction * hit.time_of_impact.clamp(0.0, distance),
        None => position + delta,
    }
}

/// Returns the lifted position when a step-up is warranted: horizontal motion is
/// blocked by a wall-like surface AND a walkable surface sits within the step
/// below the lifted capsule. Returns `None` for pure walls, so the slide loop
/// handles them by plane projection without a step-height intra-tick excursion.
pub fn step_up(
    collision: &CombinedCollisionWorld<'_>,
    capsule: CollisionCapsule,
    position: Vec3,
    horizontal_velocity: Vec3,
    remaining_dt: f32,
    params: StepParams,
) -> Option<Vec3> {
    let horiz_speed = horizontal_velocity.length();
    let step_height = params.step_height;
    if horiz_speed <= 1e-4 || step_height <= 0.0 {
        return None;
    }
    let shape = capsule.parry();
    let radius = capsule.radius;
    let dir = horizontal_velocity / horiz_speed;
    let probe_dist = (horiz_speed * remaining_dt).max(step_height + radius);
    let cast = |from: Vec3, dir: Vec3, max_toi: f32| {
        cast_capsule_combined_parry(
            collision.static_world,
            collision.movers,
            collision.poses,
            from,
            &shape,
            dir,
            max_toi,
        )
    };

    // 1. Is a wall-like obstacle in front?
    let probe = cast(position, dir, probe_dist)?;
    if !(probe.time_of_impact < probe_dist && probe.normal.y.abs() < params.cos_walkable) {
        return None;
    }

    // 2. Is the lifted capsule clear of the obstacle? The rise is a skin cast,
    // like a slide sweep, so a ceiling lower than the full lift caps it a skin
    // short: a step whose top has room for the capsule still steps up, one
    // without room is refused below, and the lift never commits into the
    // ceiling. Stopping a skin short (not `swept_move`'s tolerance) leaves the
    // lifted capsule at the band's edge, where a slide along the ceiling is
    // tangent; deeper in the band its coplanar seams tilt the contact normal
    // and refuse the forward probe.
    let lift = step_height + params.lift_margin;
    let rise = cast(position, Vec3::Y, lift).map_or(lift, |h| h.time_of_impact.clamp(0.0, lift));
    let lifted = position + Vec3::Y * rise;
    let lifted_clear = match cast(lifted, dir, probe_dist) {
        None => true,
        Some(h) => h.time_of_impact >= probe_dist - SKIN_DISTANCE,
    };
    if !lifted_clear {
        return None;
    }

    // 3. Does a walkable surface sit beneath a point advanced past the riser?
    // At decision time the capsule centre is still over the lower floor, so
    // probing straight down from `lifted` would miss a top that only exists
    // past the riser. Cap the forward offset at `radius + step_height` so a
    // narrow top is not overshot when `probe.time_of_impact` is large (a probe
    // distance inflated by high speed).
    let forward_offset = (probe.time_of_impact + radius + SKIN_DISTANCE).min(radius + step_height);
    let sample = lifted + dir * forward_offset;
    match cast(sample, Vec3::NEG_Y, step_height + 0.1) {
        Some(h) if h.normal.y >= params.cos_walkable => Some(lifted),
        _ => None,
    }
}

/// How far below a pose [`snap_to_floor`] looks: the step-up lift plus a skin
/// width and headroom, so a body that stepped down a riser or rode a floor push
/// still finds its floor.
pub fn ground_stick_reach(params: StepParams) -> f32 {
    params.step_height + params.lift_margin + SKIN_DISTANCE + 0.03
}

/// A walkable floor found below a pose.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloorBelow {
    /// How far the capsule drops to rest a skin width above the floor.
    pub drop: f32,
    pub normal: Vec3,
    pub ground: GroundRef,
}

/// The floor probe: a swept capsule down-cast within `max_down`, then a
/// centre-ray fallback. Reads collision only, so it classifies a pose without
/// moving it (a teleport arrival); [`snap_to_floor`] is the moving caller.
pub fn floor_below(
    collision: &CombinedCollisionWorld<'_>,
    capsule: CollisionCapsule,
    position: Vec3,
    max_down: f32,
    cos_walkable: f32,
) -> Option<FloorBelow> {
    probe_floor(collision, capsule, position, max_down, cos_walkable).map(|(floor, _)| floor)
}

/// Where a [`FloorBelow`] came from: the swept down-cast already proved its
/// drop clear; the centre ray proved nothing beside the capsule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloorSource {
    Sweep,
    CentreRay,
}

fn probe_floor(
    collision: &CombinedCollisionWorld<'_>,
    capsule: CollisionCapsule,
    position: Vec3,
    max_down: f32,
    cos_walkable: f32,
) -> Option<(FloorBelow, FloorSource)> {
    if let Some(h) = cast_capsule_combined_parry(
        collision.static_world,
        collision.movers,
        collision.poses,
        position,
        &capsule.parry(),
        Vec3::NEG_Y,
        max_down,
    ) && h.normal.y >= cos_walkable
    {
        let floor = FloorBelow {
            drop: h.time_of_impact,
            normal: h.normal,
            ground: ground_ref_from_hit(h),
        };
        return Some((floor, FloorSource::Sweep));
    }
    // Wall-normal preference fallback: when the capsule is pressed against a
    // wall the swept down-cast may report the wall's normal first (n.y ≈ 0).
    // Without a fallback that silently latches the body above the floor. A thin
    // ray from the capsule centre straight down ignores wall geometry on the
    // side and finds the floor below.
    let reach = capsule.half_height + capsule.radius;
    let h = cast_ray_combined_parry(
        collision.static_world,
        collision.movers,
        collision.poses,
        position,
        Vec3::NEG_Y,
        max_down + reach,
    )?;
    if h.normal.y < cos_walkable {
        return None;
    }
    // Ray TOI is the distance from the capsule centre to the surface; the
    // capsule rests with its lower hemisphere `reach` below centre, separated by
    // SKIN_DISTANCE.
    let drop = h.time_of_impact - (reach + SKIN_DISTANCE);
    // Only downward, and only within the envelope the swept down-cast covered.
    (drop > 0.0 && drop <= max_down).then(|| {
        let floor = FloorBelow {
            drop,
            normal: h.normal,
            ground: ground_ref_from_hit(h),
        };
        (floor, FloorSource::CentreRay)
    })
}

/// Snap-to-floor (ground-stick): drop the capsule onto a walkable floor within
/// [`ground_stick_reach`], returning the snapped position and the floor. `None`
/// when no floor is in reach, or when `params.step_height` is zero or less.
pub fn snap_to_floor(
    collision: &CombinedCollisionWorld<'_>,
    capsule: CollisionCapsule,
    position: Vec3,
    params: StepParams,
) -> Option<(Vec3, FloorBelow)> {
    if params.step_height <= 0.0 {
        return None;
    }
    let (floor, source) = probe_floor(
        collision,
        capsule,
        position,
        ground_stick_reach(params),
        params.cos_walkable,
    )?;
    let drop = Vec3::new(0.0, -floor.drop, 0.0);
    let snapped = match source {
        FloorSource::Sweep => position + drop,
        // The ray found the floor past an edge the sweep met first, so the
        // drop is swept: beside a riser it rests on the edge instead of sinking
        // the capsule's side into it. The floor still classifies the body as
        // grounded, as before.
        FloorSource::CentreRay => swept_move(collision, position, capsule, drop),
    };
    Some((snapped, floor))
}
