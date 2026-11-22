// Mover-relative placement of delayed remote riders: blend in the mover's frame,
// place through its presented pose.
// See: context/lib/networking.md §Host-side remote-pawn presentation

use glam::Affine3A;
use postretro_entities::Transform;
use postretro_foundation::GroundRef;

use crate::interpolation::RiderBracket;

/// Smallest scale component a mover frame may carry and still be inverted. A
/// frame below it falls back to the sampled world pose.
const MIN_FRAME_SCALE: f32 = 1.0e-6;

/// Place a delayed rider relative to the mover both of its bracketing samples
/// stand on. Each sample moves into that mover's frame at its own tick, through
/// `authoritative(mover_id, server_tick)`. The two blend there and are placed
/// through `presented(mover_id)`, the pose the mover is drawn at this frame.
///
/// Returns `None` whenever the interval does not attach. That covers samples
/// naming different movers or no mover, a missing ground or mover pose, and a
/// non-finite or non-invertible frame. The caller then draws the sampled world
/// pose, as before. Orientation and scale keep the world blend; only position
/// rides the mover.
pub(crate) fn rebase_rider(
    bracket: &RiderBracket,
    blended: &Transform,
    authoritative: impl Fn(u32, u32) -> Option<Transform>,
    presented: impl Fn(u32) -> Option<Transform>,
) -> Option<Transform> {
    let mover_id = shared_mover(bracket)?;
    let from_frame = mover_frame(&authoritative(mover_id, bracket.from.server_tick)?)?;
    let to_frame = mover_frame(&authoritative(mover_id, bracket.to.server_tick)?)?;
    let now_frame = mover_frame(&presented(mover_id)?)?;
    let from_local = from_frame
        .inverse()
        .transform_point3(bracket.from.transform.position);
    let to_local = to_frame
        .inverse()
        .transform_point3(bracket.to.transform.position);
    let local = from_local.lerp(to_local, bracket.alpha);
    let position = now_frame.transform_point3(local);
    position.is_finite().then_some(Transform {
        position,
        ..*blended
    })
}

/// The mover both ends stand on, if they name the same one.
fn shared_mover(bracket: &RiderBracket) -> Option<u32> {
    match (bracket.from.ground?, bracket.to.ground?) {
        (GroundRef::Mover(from), GroundRef::Mover(to)) if from == to => Some(from),
        _ => None,
    }
}

/// A mover pose as an invertible affine frame, or `None` when it is non-finite
/// or collapses an axis.
fn mover_frame(transform: &Transform) -> Option<Affine3A> {
    let finite = transform.position.is_finite()
        && transform.rotation.is_finite()
        && transform.scale.is_finite();
    if !finite || transform.scale.abs().min_element() < MIN_FRAME_SCALE {
        return None;
    }
    if transform.rotation.length_squared() < f32::EPSILON {
        return None;
    }
    Some(Affine3A::from_scale_rotation_translation(
        transform.scale,
        transform.rotation.normalize(),
        transform.position,
    ))
}

#[cfg(test)]
#[path = "rider_rebase_tests.rs"]
mod tests;
