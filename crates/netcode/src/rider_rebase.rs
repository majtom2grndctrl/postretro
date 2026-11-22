// Mover-relative placement of delayed remote riders: blend in the mover's frame,
// place through its presented pose.
// See: context/lib/networking.md §Host-side remote-pawn presentation

use std::collections::{HashMap, VecDeque};

use glam::{Affine3A, EulerRot, Quat, Vec3};
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, KinematicMoverComponent, Transform,
};
use postretro_foundation::GroundRef;

use postretro_entities::presentation_correction::PoseCorrection;
use postretro_net::wire::NetworkId;

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
    /// The mover the rider was placed on.
    pub(crate) mover_id: u32,
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
        mover_id,
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

/// How fast a remote's hand-off ease converges, in seconds: about the
/// interpolation delay, so a rider leaving a deck catches up to its delayed
/// world pose in roughly the time that delay spans.
pub(crate) const RIDER_HANDOFF_TAU_SECS: f64 = 0.1;

/// An ease within this many metres and radians is done.
const RIDER_HANDOFF_SETTLED: f32 = 1.0e-5;

/// One remote's hand-off between placements: what placed it last frame, where
/// it was drawn, and the ease still carrying it from there.
#[derive(Debug, Clone, Copy)]
struct RiderHandoff {
    placed_on: Option<u32>,
    drawn: Transform,
    ease: PoseCorrection,
    at_tick: f64,
}

/// Per-remote hand-offs, so a rider that leaves, lands on or changes decks
/// never steps (`networking.md` §Host-side remote-pawn presentation). On a
/// deck a delayed rider is drawn at the deck's present pose; off it, at its
/// delayed world pose. The two differ by the deck's motion over the delay, so
/// a change of placement starts an ease from where the rider was last drawn,
/// converging on the new pose over presented time. Allocates only the first
/// time a remote is presented.
#[derive(Debug, Default)]
pub(crate) struct RiderHandoffs {
    remotes: HashMap<NetworkId, RiderHandoff>,
    /// Test A/B switch: draw every pose as placed, with no ease.
    bypass: bool,
}

impl RiderHandoffs {
    /// The pose to draw `network_id` at this frame: `pose` as sampled and
    /// placed (`placed_on` names the mover the rebase placed it on), eased off
    /// the pose it was drawn at last frame when its placement changed. A
    /// teleport `cut` draws `pose` exactly and drops any ease. `render_tick` is
    /// the presentation's target tick, which times the ease.
    pub(crate) fn present(
        &mut self,
        network_id: NetworkId,
        pose: Transform,
        placed_on: Option<u32>,
        render_tick: f64,
        cut: bool,
    ) -> Transform {
        if self.bypass {
            return pose;
        }
        let fresh = RiderHandoff {
            placed_on,
            drawn: pose,
            ease: PoseCorrection::IDENTITY,
            at_tick: render_tick,
        };
        let entry = self.remotes.entry(network_id).or_insert(fresh);
        if cut {
            *entry = fresh;
            return pose;
        }
        let elapsed_ticks = (render_tick - entry.at_tick).max(0.0);
        let elapsed_secs = elapsed_ticks * crate::SERVER_TICK_MICROS as f64 / 1_000_000.0;
        let remaining = (-elapsed_secs / RIDER_HANDOFF_TAU_SECS).exp() as f32;
        let mut ease = entry.ease.scaled(remaining);
        if entry.placed_on != placed_on {
            // Hold the rider where it was drawn; the ease takes it on from there.
            ease = PoseCorrection::between(&pose, &entry.drawn);
        }
        if !ease.is_finite() || ease.is_within(RIDER_HANDOFF_SETTLED) {
            ease = PoseCorrection::IDENTITY;
        }
        let drawn = ease.apply(&pose);
        *entry = RiderHandoff {
            placed_on,
            drawn,
            ease,
            at_tick: render_tick.max(entry.at_tick),
        };
        drawn
    }

    /// `pose` carried by the ease `network_id` is drawn with now, without
    /// advancing it: a look-ahead read of the pose the frame will draw.
    pub(crate) fn peek(&self, network_id: NetworkId, pose: Transform) -> Transform {
        self.remotes
            .get(&network_id)
            .map_or(pose, |handoff| handoff.ease.apply(&pose))
    }

    pub(crate) fn forget(&mut self, network_id: NetworkId) {
        self.remotes.remove(&network_id);
    }

    /// Draw every pose as placed; the before half of a test's A/B.
    #[cfg(test)]
    pub(crate) fn set_bypass(&mut self, bypass: bool) {
        self.bypass = bypass;
    }
}

#[cfg(test)]
#[path = "rider_rebase_tests.rs"]
mod tests;
