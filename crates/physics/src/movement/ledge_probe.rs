// Ledge probe for vault entry, built on substrate cast helpers; runs from the tick-start pose.
// See: context/lib/movement.md §4

use glam::Vec3;

use crate::collision::SKIN_DISTANCE;
use crate::collision::moving::{CollisionSource, CombinedCollisionWorld};
use crate::movement::substrate::{cast_capsule_query, cast_ray_query, clearance_probe};
use parry3d::shape::Capsule;
use postretro_foundation::PlayerMovementComponent;

/// How far past the capsule surface the first sweep looks for a wall, metres,
/// beyond the distance the pawn covers this tick.
const WALL_REACH: f32 = 0.1;

/// Extra inset past the wall plane, beyond radius plus skin, where the down ray
/// samples the top, so a bevelled lip does not read as the top's edge.
const LIP_INSET: f32 = 0.02;

/// Height above the reach ceiling the down ray starts at, so a top exactly at
/// the ceiling is still hit rather than started on.
const RAY_START_MARGIN: f32 = 0.05;

#[cfg(test)]
thread_local! {
    static PROBE_CASTS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Casts the probe made on this thread since the last call, for the probe-cost
/// rows. Production builds carry no counter.
#[cfg(test)]
pub(super) fn take_probe_casts() -> u32 {
    PROBE_CASTS.with(|casts| casts.replace(0))
}

fn count_cast() {
    #[cfg(test)]
    PROBE_CASTS.with(|casts| casts.set(casts.get() + 1));
}

/// The ledge the probe found: where the pawn's feet rest on its top, and the
/// mover carrying it, if any.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct LedgeCandidate {
    /// World-space feet position at rest on the top: one skin width above it.
    pub(super) target_feet: Vec3,
    pub(super) mover: Option<u32>,
}

/// Ledge tops the probe accepts, as world heights: strictly above `min_top_y`,
/// at most `max_top_y`.
#[derive(Debug, Clone, Copy)]
pub(super) struct ReachBand {
    pub(super) min_top_y: f32,
    pub(super) max_top_y: f32,
}

/// Find a vaultable ledge in front of the capsule centred at `position`,
/// pushing along the unit horizontal `wish` while covering `travel` metres
/// along it this tick. At most four casts, all through the combined world so
/// movers count:
///
/// 1. A forward capsule sweep finds a wall within `travel` plus a margin, so a
///    fast pawn vaults before its own sweep hits the wall and loses its speed.
///    Nothing ahead, or a walkable surface, ends the probe here.
/// 2. A down ray past the wall finds the top. A non-walkable hit or a top
///    outside `band` refuses.
/// 3. An upward capsule sweep from the tick-start pose to just above the top
///    proves the rise is clear.
/// 4. A forward capsule sweep at that height to the landing point proves the
///    path over the lip and the standing capsule's room on the top.
///
/// Sweeps 3 and 4 run one skin width above the resting height, so the top
/// itself never reads as blocking them. The combined (mover-aware) collision
/// has no overlap query; the two sweeps from a known-clear pose stand in for
/// it, so the probe still respects movers.
pub(super) fn probe_ledge(
    component: &PlayerMovementComponent,
    collision: &CombinedCollisionWorld<'_>,
    position: Vec3,
    wish: Vec3,
    travel: f32,
    band: ReachBand,
) -> Option<LedgeCandidate> {
    let half_height = component.capsule.half_height;
    let radius = component.capsule.radius;
    let capsule = Capsule::new_y(half_height, radius);

    count_cast();
    let wall = cast_capsule_query(
        collision,
        position,
        &capsule,
        wish,
        travel.max(0.0) + WALL_REACH,
    )?;
    if wall.normal.y.abs() >= component.cos_walkable {
        return None;
    }

    let forward = wall.time_of_impact.max(0.0) + 2.0 * radius + SKIN_DISTANCE + LIP_INSET;
    let landing = position + wish * forward;
    let ray_top = band.max_top_y + RAY_START_MARGIN;
    let ray_length = ray_top - band.min_top_y;
    if ray_length <= 0.0 {
        return None;
    }
    count_cast();
    let top = cast_ray_query(
        collision,
        Vec3::new(landing.x, ray_top, landing.z),
        Vec3::NEG_Y,
        ray_length,
    )?;
    if top.normal.y < component.cos_walkable {
        return None;
    }
    let top_y = ray_top - top.time_of_impact;
    if top_y <= band.min_top_y || top_y > band.max_top_y {
        return None;
    }

    let feet_offset = half_height + radius;
    let rest_center_y = top_y + SKIN_DISTANCE + feet_offset;
    let probe_center_y = rest_center_y + SKIN_DISTANCE;
    let rise = probe_center_y - position.y;
    if rise <= 0.0 {
        return None;
    }
    count_cast();
    if !clearance_probe(component, collision, position, half_height, Vec3::Y, rise) {
        return None;
    }
    let raised = Vec3::new(position.x, probe_center_y, position.z);
    count_cast();
    if !clearance_probe(component, collision, raised, half_height, wish, forward) {
        return None;
    }

    let mover = match top.source {
        CollisionSource::Static => None,
        CollisionSource::Mover(id) => Some(id),
    };
    Some(LedgeCandidate {
        target_feet: Vec3::new(landing.x, rest_center_y - feet_offset, landing.z),
        mover,
    })
}
