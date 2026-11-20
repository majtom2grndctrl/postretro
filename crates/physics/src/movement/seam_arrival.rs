// A level seam's landmark-relative arrival: captured before unload, applied to the next level's spawn.
// See: context/lib/entity_model.md §7e · context/lib/movement.md §6 (Teleport)

use super::MovementStateEdge;
use super::teleport::{rotate_arrival_motion, settle_arrival};
use crate::collision::moving::CombinedCollisionWorld;
use glam::{Quat, Vec3};
use postretro_foundation::{
    CapsuleParams, MovementState, NO_TELEPORTER, PlayerMovementComponent, SeamlessMapping,
    wrap_teleport_yaw,
};

/// A named point a seam measures arrivals from: origin and world-up yaw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Landmark {
    pub origin: Vec3,
    pub yaw: f32,
}

impl Landmark {
    /// The landmark's frame as a seamless mapping from the world: its origin
    /// goes to zero and its yaw to zero.
    fn into_frame(self) -> SeamlessMapping {
        SeamlessMapping::new(self.origin, self.yaw, Vec3::ZERO, 0.0)
    }
    /// The world from the landmark's frame.
    fn out_of_frame(self) -> SeamlessMapping {
        SeamlessMapping::new(Vec3::ZERO, 0.0, self.origin, self.yaw)
    }
}

/// A pawn's pose and motion in a landmark's frame. The unload harvest writes
/// one per seat inside the crossing seam; the seat's next spawn maps it through
/// the destination seam's landmark. A seam checkpoint keeps only the pose
/// (`at_rest`).
#[derive(Debug, Clone, PartialEq)]
pub struct LandmarkRelative {
    /// Body point (capsule centre) minus the landmark origin, unrotated by its yaw.
    pub offset: Vec3,
    /// View yaw minus the landmark yaw, wrapped.
    pub view_yaw: f32,
    /// View pitch: a landmark turns only yaw.
    pub view_pitch: f32,
    pub velocity: Vec3,
    pub knockback_velocity: Vec3,
    pub push_velocity: Vec3,
    /// The state payload, its boost in the landmark's frame and its clock kept,
    /// so a dash or slide resumes rather than restarts.
    pub movement_state: MovementState,
    /// The live capsule while crouched or sliding, whose shrunk size the state
    /// assumes; `None` arrives at the spawned pawn's own size.
    pub crouched_capsule: Option<CapsuleParams>,
    /// `offset` for a standing capsule with the same feet: what a seam
    /// checkpoint saves, since Continue arrives standing at rest.
    pub rest_offset: Vec3,
}

impl LandmarkRelative {
    /// Read `component` at `body` with view `view_yaw` / `view_pitch` into
    /// `landmark`'s frame.
    pub fn capture(
        component: &PlayerMovementComponent,
        body: Vec3,
        view_yaw: f32,
        view_pitch: f32,
        landmark: Landmark,
    ) -> Self {
        let frame = landmark.into_frame();
        let mut movement_state = component.movement_state;
        if let MovementState::Dash { boost, .. } | MovementState::Sliding { boost, .. } =
            &mut movement_state
        {
            *boost = frame.vector(*boost);
        }
        let crouched_capsule = matches!(
            movement_state,
            MovementState::Crouching { .. } | MovementState::Sliding { .. }
        )
        .then(|| component.capsule.clone());
        let offset = frame.point(body);
        // A shrunk capsule keeps its feet planted: standing up raises the
        // centre by the half-height it lost.
        let rest_offset = offset
            + Vec3::Y
                * crouched_capsule.as_ref().map_or(0.0, |capsule| {
                    (component.standing_half_height - capsule.half_height).max(0.0)
                });
        Self {
            offset,
            rest_offset,
            view_yaw: wrap_teleport_yaw(view_yaw + frame.yaw_delta()),
            view_pitch,
            velocity: frame.vector(component.velocity),
            knockback_velocity: frame.vector(component.knockback_velocity),
            push_velocity: frame.vector(component.push_velocity),
            movement_state,
            crouched_capsule,
        }
    }

    /// A pose with no motion: a seam checkpoint's Continue arrives at rest.
    pub fn at_rest(offset: Vec3, view_yaw: f32, view_pitch: f32) -> Self {
        Self {
            offset,
            rest_offset: offset,
            view_yaw,
            view_pitch,
            velocity: Vec3::ZERO,
            knockback_velocity: Vec3::ZERO,
            push_velocity: Vec3::ZERO,
            movement_state: MovementState::Normal,
            crouched_capsule: None,
        }
    }

    /// The world body point this arrival takes through `landmark`.
    pub fn body_point(&self, landmark: Landmark) -> Vec3 {
        landmark.out_of_frame().point(self.offset)
    }

    /// The world view yaw this arrival takes through `landmark`.
    pub fn world_view_yaw(&self, landmark: Landmark) -> f32 {
        wrap_teleport_yaw(self.view_yaw + landmark.yaw)
    }
}

/// Where an applied arrival put the pawn and its view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeamArrivalPose {
    pub body: Vec3,
    pub view_yaw: f32,
    pub view_pitch: f32,
}

/// Why an arrival did not apply. The caller falls back to a player start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeamArrivalRefusal {
    /// The mapped pose is not finite.
    NonFinite,
    /// The capsule does not fit the static world and movers there.
    Blocked,
}

/// Place a freshly spawned pawn at `arrival` through `landmark`, under the
/// teleport arrival rule: every velocity channel and a dash or slide boost turn
/// by the landmark yaw, a vault aborts to `Normal`, the ground settles, and the
/// pawn latches against every armed teleporter containing its body point.
/// A blocked or non-finite arrival changes nothing. Live pawns are the
/// caller's test: this sees only the static world and movers.
pub fn apply_seam_arrival(
    component: &mut PlayerMovementComponent,
    collision: &CombinedCollisionWorld<'_>,
    arrival: &LandmarkRelative,
    landmark: Landmark,
    armed_teleporter: impl FnMut(usize) -> bool,
) -> Result<(SeamArrivalPose, Option<MovementStateEdge>), SeamArrivalRefusal> {
    let body = arrival.body_point(landmark);
    let view_yaw = arrival.world_view_yaw(landmark);
    if !body.is_finite() || !view_yaw.is_finite() || !arrival.view_pitch.is_finite() {
        return Err(SeamArrivalRefusal::NonFinite);
    }
    let capsule = arrival
        .crouched_capsule
        .clone()
        .unwrap_or_else(|| component.capsule.clone());
    if !crate::collision::capsule_fits_combined(collision, body, &capsule) {
        return Err(SeamArrivalRefusal::Blocked);
    }
    component.capsule = capsule;
    component.velocity = arrival.velocity;
    component.knockback_velocity = arrival.knockback_velocity;
    component.push_velocity = arrival.push_velocity;
    component.movement_state = match &arrival.movement_state {
        // A fluid index names the source level's fluid table; the next tick's
        // immersion re-enters swim where the arrival is wet.
        MovementState::Swim { .. } => MovementState::Normal,
        state => *state,
    };
    let edge = rotate_arrival_motion(component, Quat::from_rotation_y(landmark.yaw));
    // A spawn starts a fresh teleport frame: the view is set absolutely, so
    // no yaw accumulates.
    settle_arrival(
        component,
        collision,
        body,
        0.0,
        NO_TELEPORTER,
        armed_teleporter,
    );
    Ok((
        SeamArrivalPose {
            body,
            view_yaw,
            view_pitch: arrival.view_pitch,
        },
        edge,
    ))
}

#[cfg(test)]
#[path = "seam_arrival_tests.rs"]
mod tests;
