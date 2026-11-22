// Mover-relative placement of delayed remote riders: blend in the mover's frame,
// place through its presented pose.
// See: context/lib/networking.md §Host-side remote-pawn presentation

use std::collections::{HashMap, VecDeque};

use glam::{Affine3A, EulerRot, Quat, Vec3};
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, KinematicMoverComponent, Transform,
};
use postretro_foundation::GroundRef;

use crate::interpolation::RiderBracket;
use crate::kinematic_mover::world_up_yaw;

/// Smallest scale component a mover frame may carry and still be inverted. A
/// frame below it falls back to the sampled world pose.
const MIN_FRAME_SCALE: f32 = 1.0e-6;

/// A mover as drawn this frame, and whether its riders' facing rides its yaw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PresentedMover {
    pub(crate) transform: Transform,
    pub(crate) carry_yaw: bool,
}

/// A rider placed on its mover, and the world-up yaw the placement turned it by
/// (zero without `carry_yaw`). Its legs turn by the same amount, so they keep
/// the heading relative to the body that the sample had.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RebasedRider {
    pub(crate) transform: Transform,
    pub(crate) yaw_turn: f32,
}

impl RebasedRider {
    /// `velocity` turned with the body about world-up.
    pub(crate) fn turn(&self, velocity: Vec3) -> Vec3 {
        if self.yaw_turn == 0.0 {
            return velocity;
        }
        Quat::from_rotation_y(self.yaw_turn) * velocity
    }
}

/// Place a delayed rider relative to the mover both of its bracketing samples
/// stand on. Each sample moves into that mover's frame at its own tick, through
/// `authoritative(mover_id, server_tick)`. The two blend there and are placed
/// through `presented(mover_id)`, the pose the mover is drawn at this frame.
///
/// Returns `None` whenever the interval does not attach. That covers samples
/// naming different movers or no mover, a missing ground or mover pose, and a
/// non-finite or non-invertible frame. The caller then draws the sampled world
/// pose, as before. Scale, pitch and roll keep the world blend. Yaw keeps it
/// too unless the mover carries yaw: then the rider's yaw relative to the
/// mover's world-up yaw blends, and the drawn mover's yaw is added back once.
pub(crate) fn rebase_rider(
    bracket: &RiderBracket,
    blended: &Transform,
    authoritative: impl Fn(u32, u32) -> Option<Transform>,
    presented: impl Fn(u32) -> Option<PresentedMover>,
) -> Option<RebasedRider> {
    let mover_id = shared_mover(bracket)?;
    let from_mover = authoritative(mover_id, bracket.from.server_tick)?;
    let to_mover = authoritative(mover_id, bracket.to.server_tick)?;
    let now = presented(mover_id)?;
    let from_frame = mover_frame(&from_mover)?;
    let to_frame = mover_frame(&to_mover)?;
    let now_frame = mover_frame(&now.transform)?;
    let from_local = from_frame
        .inverse()
        .transform_point3(bracket.from.transform.position);
    let to_local = to_frame
        .inverse()
        .transform_point3(bracket.to.transform.position);
    let local = from_local.lerp(to_local, bracket.alpha);
    let position = now_frame.transform_point3(local);
    if !position.is_finite() {
        return None;
    }
    let mut transform = Transform {
        position,
        ..*blended
    };
    let mut yaw_turn = 0.0;
    if now.carry_yaw {
        let relative = |rider: &Transform, mover: &Transform| {
            wrap_angle(rider_yaw(rider) - world_up_yaw(mover.rotation))
        };
        let from = relative(&bracket.from.transform, &from_mover);
        let to = relative(&bracket.to.transform, &to_mover);
        let carried = wrap_angle(
            from + wrap_angle(to - from) * bracket.alpha + world_up_yaw(now.transform.rotation),
        );
        let (blended_yaw, pitch, roll) = blended.rotation.to_euler(EulerRot::YXZ);
        let rotation = Quat::from_euler(EulerRot::YXZ, carried, pitch, roll);
        if !rotation.is_finite() {
            return None;
        }
        transform.rotation = rotation;
        yaw_turn = wrap_angle(carried - blended_yaw);
    }
    Some(RebasedRider {
        transform,
        yaw_turn,
    })
}

/// A rider's facing yaw, the yaw term of its YXZ rotation.
fn rider_yaw(rider: &Transform) -> f32 {
    rider.rotation.to_euler(EulerRot::YXZ).0
}

fn wrap_angle(angle: f32) -> f32 {
    (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

/// The loaded mover `mover_id` as drawn at `alpha`, with its `carry_yaw`.
/// A client finds it by its stable mover id.
pub(crate) fn presented_mover(
    registry: &EntityRegistry,
    mover_id: u32,
    alpha: f32,
) -> Option<PresentedMover> {
    let entity = registry
        .iter_with_kind(ComponentKind::KinematicMover)
        .find_map(|(id, value)| match value {
            ComponentValue::KinematicMover(mover) if mover.mover_id == mover_id => Some(id),
            _ => None,
        })?;
    presented_entity_mover(registry, entity, alpha)
}

/// Mover `entity` as drawn at `alpha`, with its `carry_yaw`. `None` once the
/// entity is gone or is no mover.
pub(crate) fn presented_entity_mover(
    registry: &EntityRegistry,
    entity: EntityId,
    alpha: f32,
) -> Option<PresentedMover> {
    let carry_yaw = registry
        .get_component::<KinematicMoverComponent>(entity)
        .ok()?
        .carry_yaw;
    Some(PresentedMover {
        transform: registry.interpolated_transform(entity, alpha).ok()?,
        carry_yaw,
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

/// Ticks of mover pose the host keeps per ridden mover: the delayed target
/// plus the in-flight tick, with margin, never more than the rider buffer holds.
pub(crate) const HOST_MOVER_POSE_TICKS: usize = 16;

/// The host seat's end-of-tick pose of each mover a client pawn stood on, keyed
/// by the tick label its rider samples carry (pin P11). Recorded beside each
/// rider sample, so a delayed sample finds its mover's pose for the same tick.
/// A connected client reads mover history instead.
#[derive(Debug, Default)]
pub(crate) struct RiderMoverPoses {
    movers: HashMap<u32, RiddenMover>,
}

#[derive(Debug)]
struct RiddenMover {
    entity: EntityId,
    poses: VecDeque<(u32, Transform)>,
}

impl RiderMoverPoses {
    /// Record `mover_id`'s pose for `tick`. A repeat for the same tick replaces
    /// it; the oldest tick past the bound is dropped. Allocates only the first
    /// time a mover is ridden.
    pub(crate) fn record(
        &mut self,
        mover_id: u32,
        entity: EntityId,
        tick: u32,
        transform: Transform,
    ) {
        let ridden = self.movers.entry(mover_id).or_insert_with(|| RiddenMover {
            entity,
            poses: VecDeque::with_capacity(HOST_MOVER_POSE_TICKS + 1),
        });
        ridden.entity = entity;
        if ridden
            .poses
            .back()
            .is_some_and(|(newest, _)| *newest == tick)
        {
            ridden.poses.pop_back();
        }
        ridden.poses.push_back((tick, transform));
        while ridden.poses.len() > HOST_MOVER_POSE_TICKS {
            ridden.poses.pop_front();
        }
    }

    /// The pose recorded for exactly `tick`, if it is still held.
    pub(crate) fn pose_at(&self, mover_id: u32, tick: u32) -> Option<Transform> {
        self.movers
            .get(&mover_id)?
            .poses
            .iter()
            .rev()
            .find(|(recorded, _)| *recorded == tick)
            .map(|(_, transform)| *transform)
    }

    /// The entity last recorded for `mover_id`, whose presented pose places
    /// the rider. A despawned entity fails that read and the rider falls back.
    pub(crate) fn entity(&self, mover_id: u32) -> Option<EntityId> {
        self.movers.get(&mover_id).map(|ridden| ridden.entity)
    }
}

#[cfg(test)]
#[path = "rider_rebase_tests.rs"]
mod tests;
