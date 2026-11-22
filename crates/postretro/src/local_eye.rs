// The local eye's presented pose: camera follow, eye height, mover yaw carry
// and its render residual, and authoritative mover corrections at the eye.
// See: context/lib/movement.md §6 (Moving bases)

use super::*;

/// The view yaw on a carry_yaw deck drawn at `drawn`: the settled yaw, which
/// stands at the tick start of `state`, plus the deck's drawn world-up turn
/// since that tick start.
pub(crate) fn drawn_deck_view_yaw(
    settled_yaw: f32,
    state: &kinematic_mover::MoverTickState,
    drawn: &postretro_entities::Transform,
) -> Option<f32> {
    let tick_start = state.tick_rotation_delta.inverse() * state.transform.rotation;
    let turn = world_up_yaw_delta(drawn.rotation * tick_start.inverse());
    turn.is_finite().then_some(settled_yaw + turn)
}

/// A carry_yaw deck the view rides, its glide yaw, and whether a glide is in
/// flight on it.
pub(crate) type CarriedGlide = (Option<u32>, f32, bool);

/// One frame of the view-yaw hand-off: decay what is in flight by `frame_dt`,
/// then, when the carry_yaw deck the view rides changed since last frame or its
/// glide is gone (despawned, rebound, settled), take the glide yaw it drew
/// there less the one it draws now.
pub(crate) fn next_view_yaw_handoff(
    handoff: f32,
    drawn: (Option<u32>, f32),
    now: CarriedGlide,
    frame_dt: f32,
) -> f32 {
    let mut handoff = handoff;
    if frame_dt.is_finite() && frame_dt > 0.0 {
        handoff *= (-frame_dt / VIEW_YAW_HANDOFF_TAU_SECS).exp();
        if handoff.abs() < 1.0e-6 {
            handoff = 0.0;
        }
    }
    if (drawn.0 != now.0 || !now.2) && drawn.1.is_finite() && now.1.is_finite() {
        handoff += drawn.1 - now.1;
    }
    handoff
}

/// The view-yaw hand-off decays at the pawn presentation offset's rate:
/// 0.7 per 60 Hz tick.
const VIEW_YAW_HANDOFF_TAU_SECS: f32 = 0.0467;

pub(crate) fn world_up_yaw_delta(tick_rotation_delta: Quat) -> f32 {
    kinematic_mover::world_up_yaw(tick_rotation_delta)
}

pub(crate) fn yaw_after_mover_carry(
    camera_yaw: f32,
    carry_yaw: bool,
    tick_rotation_delta: Quat,
) -> f32 {
    if carry_yaw {
        camera_yaw + world_up_yaw_delta(tick_rotation_delta)
    } else {
        camera_yaw
    }
}

/// Carry only the owning player's upright view from its prior mover pose.
/// Input runs before this tick's mover update, so the pose and captured ground
/// reference both belong to the exact preceding tick.
pub(crate) fn apply_mover_yaw_carry(
    camera: &mut Camera,
    carry_ground: postretro_foundation::GroundRef,
    mover_states: &kinematic_mover::MoverTickStateTable,
) {
    let postretro_foundation::GroundRef::Mover(mover_id) = carry_ground else {
        return;
    };
    let Some(pose) = mover_states.get(mover_id) else {
        return;
    };

    camera.yaw = yaw_after_mover_carry(camera.yaw, pose.carry_yaw, pose.tick_rotation_delta);
}

/// Presentation-only fraction of the mover rotation that the next input seam will
/// commit in full. The simulation-facing camera yaw and `facing_yaw` remain settled
/// at fixed-tick boundaries.
pub(crate) fn mover_yaw_render_residual(
    carry_ground: postretro_foundation::GroundRef,
    mover_states: &kinematic_mover::MoverTickStateTable,
    alpha: f32,
) -> f32 {
    let postretro_foundation::GroundRef::Mover(mover_id) = carry_ground else {
        return 0.0;
    };
    let Some(pose) = mover_states.get(mover_id) else {
        return 0.0;
    };
    if !pose.carry_yaw {
        return 0.0;
    }

    world_up_yaw_delta(pose.tick_rotation_delta) * alpha.clamp(0.0, 1.0)
}

pub(crate) fn effective_render_yaw(
    settled_camera_yaw: f32,
    carry_ground: postretro_foundation::GroundRef,
    mover_states: &kinematic_mover::MoverTickStateTable,
    alpha: f32,
) -> f32 {
    settled_camera_yaw + mover_yaw_render_residual(carry_ground, mover_states, alpha)
}

/// One frame's displayed gameplay aim, before cosmetic view-feel offsets.
/// Both connected shots and render-eye assembly consume this selected pose.
#[derive(Clone, Copy)]
pub(crate) struct PresentedAimPose {
    pub(crate) position: Vec3,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
}

impl PresentedAimPose {
    pub(crate) fn aim_ray(self) -> (Vec3, Vec3) {
        Camera::new(self.position, self.yaw, self.pitch).aim_ray()
    }

    pub(crate) fn frame_eye_inputs(
        self,
        aspect: f32,
        base_hfov: f32,
        driver: Option<frame_eye::ViewFeelDriver>,
        movement_edges: &[view_feel::TimedMovementEdge],
        frame_dt: f32,
        view_feel_scale: f32,
    ) -> frame_eye::FrameEyeInputs<'_> {
        frame_eye::FrameEyeInputs {
            presented_eye: self.position,
            aspect,
            base_hfov,
            render_yaw: self.yaw,
            pitch: self.pitch,
            driver,
            movement_edges,
            frame_dt,
            view_feel_scale,
        }
    }
}

impl App {
    pub(crate) fn presented_aim_pose(&self, alpha: f32) -> PresentedAimPose {
        let registry = self
            .session
            .as_ref()
            .and_then(|session| session.scripting.script_ctx.registry.try_borrow().ok());
        let eye = registry.as_deref().map_or_else(
            || self.frame_timing.interpolated_state().position,
            |registry| self.presented_local_eye(registry, alpha).eye,
        );
        // On a carry_yaw deck the view turns with the deck as drawn: the
        // settled yaw stands at the tick start, and the deck's drawn world-up
        // turn since that tick start is added. With no glide this is the tick's
        // alpha-scaled turn; across a correction the settled yaw took the whole
        // start correction at apply while the drawn deck glides, so the view
        // stays on the drawn deck even when the correction changed the spin rate.
        let yaw = registry
            .as_deref()
            .and_then(|registry| self.drawn_deck_yaw(registry, alpha))
            .unwrap_or_else(|| {
                effective_render_yaw(
                    self.camera.yaw,
                    self.mover_yaw_carry_ground,
                    &self.kinematic_mover_tick_states,
                    alpha,
                )
            })
            + self.view_yaw_handoff;
        PresentedAimPose {
            // Timing already carries the connected owner's presentation correction.
            position: eye,
            yaw,
            // Look is render-rate, including frames with no fixed tick.
            pitch: self.camera.pitch,
        }
    }

    /// The local eye and pawn body this frame presents at `alpha`: placed on
    /// the drawn deck while both tick endpoints stand on one mover, otherwise
    /// the world-space blend (`movement.md` §6, Moving bases).
    pub(crate) fn presented_local_eye(
        &self,
        registry: &postretro_entities::EntityRegistry,
        alpha: f32,
    ) -> frame_timing::PresentedEye {
        frame_timing::present_eye(
            &self.frame_timing.previous_state,
            &self.frame_timing.current_state,
            alpha,
            |mover| registry.interpolated_transform(mover, alpha).ok(),
        )
    }

    /// The view yaw on the carry_yaw deck the view rides, from the deck as
    /// drawn at `alpha`; `None` off such a deck or when it is not drawn.
    fn drawn_deck_yaw(
        &self,
        registry: &postretro_entities::EntityRegistry,
        alpha: f32,
    ) -> Option<f32> {
        let postretro_foundation::GroundRef::Mover(mover_id) = self.mover_yaw_carry_ground else {
            return None;
        };
        let state = self.kinematic_mover_tick_states.get(mover_id)?;
        if !state.carry_yaw {
            return None;
        }
        let drawn = registry.interpolated_transform(state.entity, alpha).ok()?;
        drawn_deck_view_yaw(self.camera.yaw, state, &drawn)
    }

    /// The carry_yaw mover the view rides, the world-up yaw still to glide on
    /// it, and whether a glide is in flight there.
    fn carried_glide_yaw(&self, registry: &postretro_entities::EntityRegistry) -> CarriedGlide {
        let postretro_foundation::GroundRef::Mover(mover_id) = self.mover_yaw_carry_ground else {
            return (None, 0.0, false);
        };
        let Some(state) = self.kinematic_mover_tick_states.get(mover_id) else {
            return (None, 0.0, false);
        };
        if !state.carry_yaw {
            return (None, 0.0, false);
        }
        match registry.presentation_correction(state.entity) {
            Some(glide) => (Some(mover_id), world_up_yaw_delta(glide.rotation), true),
            None => (Some(mover_id), 0.0, false),
        }
    }

    /// Hand the view yaw across a change of the carry_yaw deck it rides (pin
    /// P20): leaving, landing on or losing a gliding deck moves the glide yaw it
    /// drew into a render-only offset that decays at the pawn offset's rate, so
    /// the drawn view does not turn and the settled yaw commands read is
    /// untouched. Runs once per frame.
    fn hand_off_view_yaw(&mut self, registry: &postretro_entities::EntityRegistry, frame_dt: f32) {
        // A held eye (teleport, cut, level install, menu) carries no yaw
        // across: drop any hand-off in flight and the deck it came from.
        if self.frame_timing.previous_state.body.is_none()
            && self.frame_timing.current_state.body.is_none()
        {
            self.view_yaw_handoff = 0.0;
            self.drawn_glide_yaw = (None, 0.0);
            return;
        }
        let now = self.carried_glide_yaw(registry);
        self.view_yaw_handoff =
            next_view_yaw_handoff(self.view_yaw_handoff, self.drawn_glide_yaw, now, frame_dt);
        self.drawn_glide_yaw = (now.0, now.1);
    }

    /// Draw the local pawn's mesh, shadow and mirror included, where this
    /// frame's eye places its body: on the drawn deck while riding, and on a
    /// connected client with the eye's world offset (option A of the rider
    /// brief). Anywhere else the mesh keeps its plain blend, bit for bit. Runs
    /// once per frame before any reader of the pawn's presented pose.
    pub(crate) fn place_local_pawn_body(&mut self, alpha: f32, frame_dt: f32) {
        let connected_client = self.is_connected_client();
        let Some(script_ctx) = self
            .session
            .as_ref()
            .map(|session| session.scripting.script_ctx.clone())
        else {
            return;
        };
        self.hand_off_view_yaw(&script_ctx.registry.borrow(), frame_dt);
        let mut registry = script_ctx.registry.borrow_mut();
        registry.clear_pinned_presentation_offsets();
        let Some(pawn) = followed_player_pawn(&registry) else {
            return;
        };
        let presented = self.presented_local_eye(&registry, alpha);
        let pin = match presented.body {
            Some(body) if presented.riding.is_some() => registry
                .interpolated_transform(pawn, alpha)
                .ok()
                .map(|blend| body - blend.position),
            Some(_) if connected_client && presented.offset != Vec3::ZERO => Some(presented.offset),
            _ => None,
        };
        registry.set_pinned_presentation_offset(pawn, pin);
    }

    pub(crate) fn render_aim_pose(&self, gameplay_aim: PresentedAimPose) -> PresentedAimPose {
        // A playing camera route replaces the presented aim; the pawn's own
        // aim and position stay where the sim leaves them.
        #[cfg(feature = "dev-tools")]
        if let Some(view) = self.playback.route_view() {
            return PresentedAimPose {
                position: view.eye,
                yaw: view.yaw,
                pitch: view.pitch,
            };
        }
        if self.frontend_menu_is_present() {
            // The authored menu camera is held independently of a prior rider's
            // mover carry state, including while a submenu is on top.
            PresentedAimPose {
                position: self.camera.position,
                yaw: self.camera.yaw,
                pitch: self.camera.pitch,
            }
        } else {
            gameplay_aim
        }
    }
}

/// Reconcile the mover pose table at the same seam that owns camera yaw. The
/// start-of-tick correction preserves the rider's camera-to-platform offset;
/// the refreshed tick delta is then committed once by the ordinary input seam.
pub(crate) fn apply_authoritative_mover_corrections(
    camera: &mut camera::Camera,
    carry_ground: postretro_foundation::GroundRef,
    mover_states: &mut kinematic_mover::MoverTickStateTable,
    corrections: &[netcode::MoverCorrection],
) {
    for correction in corrections {
        let authoritative = correction.authoritative_state;
        let previous = mover_states.get(correction.mover_id).copied();
        if carry_ground == postretro_foundation::GroundRef::Mover(correction.mover_id)
            && authoritative.carry_yaw
            && let Some(previous) = previous
        {
            let predicted_tick_start =
                previous.tick_rotation_delta.inverse() * previous.transform.rotation;
            let authoritative_tick_start =
                authoritative.tick_rotation_delta.inverse() * authoritative.transform.rotation;
            let start_correction = authoritative_tick_start * predicted_tick_start.inverse();
            camera.yaw += world_up_yaw_delta(start_correction);
        }
        mover_states.publish(correction.mover_id, authoritative);
    }
}

/// Push a connected client's eye endpoint, keeping the eye still where the
/// push changes what places it (pins P4, P5, P15, P19). The returned delta
/// moves into the world offset, so later endpoints carry it; zero when nothing
/// was handed off.
///
/// - Attaching (the new interval rides a deck the old one did not): the gap
///   between the old interval's end and the new one's start moves into both
///   new endpoints, so the glide still in flight is cancelled at the eye and
///   decays with the offset.
/// - Detaching (leaving, or changing to another deck): the old endpoint takes
///   the eye the deck drew at the boundary, and the new endpoint and the
///   offset take the glide's displacement at the eye's point, so the eye keeps
///   the drawn deck's place and lands on its world position as that decays.
pub(crate) fn push_eye_endpoint(
    timing: &mut frame_timing::FrameTiming,
    endpoint: frame_timing::InterpolableState,
    registry: &postretro_entities::EntityRegistry,
) -> Vec3 {
    // At the tick boundary the mover's previous slot holds the pose the old
    // interval ends on and the new one starts from.
    let drawn = |mover| registry.interpolated_transform(mover, 0.0).ok();
    let old_end =
        frame_timing::present_eye(&timing.previous_state, &timing.current_state, 1.0, drawn);
    // The same boundary without any glide: the deck as simulated.
    let simulated = |mover| {
        let drawn = registry.interpolated_transform(mover, 0.0).ok()?;
        Some(
            registry
                .presentation_correction(mover)
                .map_or(drawn, |glide| glide.inverse().apply(&drawn)),
        )
    };
    let old_end_simulated = frame_timing::present_eye(
        &timing.previous_state,
        &timing.current_state,
        1.0,
        simulated,
    );
    timing.push_state(endpoint);
    let new_start =
        frame_timing::present_eye(&timing.previous_state, &timing.current_state, 0.0, drawn);
    let placement =
        |eye: &frame_timing::PresentedEye| eye.riding.map(|mover| (mover.mover_id, mover.entity));
    if placement(&old_end) == placement(&new_start) {
        return Vec3::ZERO;
    }
    let shift = |state: &mut frame_timing::InterpolableState, delta: Vec3| {
        state.position += delta;
        if let Some(body) = state.body.as_mut() {
            body.offset += delta;
        }
    };
    if old_end.riding.is_some() && new_start.riding.is_none() {
        let glide = old_end.eye - old_end_simulated.eye;
        let to_drawn = old_end.eye - timing.previous_state.position;
        if !glide.is_finite() || !to_drawn.is_finite() {
            return Vec3::ZERO;
        }
        shift(&mut timing.previous_state, to_drawn);
        shift(&mut timing.current_state, glide);
        return glide;
    }
    let delta = old_end.eye - new_start.eye;
    if delta == Vec3::ZERO || !delta.is_finite() {
        return Vec3::ZERO;
    }
    timing.shift_interval(delta);
    delta
}

/// Keep the eye still across a snapshot apply that changes what places it.
/// A deck that snapped under the eye cuts it to the snapped deck (pin P7).
/// A glide dropped under it, or a mover lost or rebound beneath it, hands the
/// residual at the eye's point into the world offset and both endpoints, so it
/// reaches the eye on this frame at any alpha, even with no tick to push a new
/// endpoint (pins P12, P16).
pub(crate) fn hand_off_eye_across_apply(
    timing: &mut frame_timing::FrameTiming,
    prediction: &mut netcode::ClientPrediction,
    before: frame_timing::PresentedEye,
    after: frame_timing::PresentedEye,
    snapped: &[u32],
) {
    let Some(riding) = before.riding else {
        return;
    };
    // The apply already cut the eye (a teleport, a teleport-class correction,
    // a composed-offset snap, a first-record hold): it held both endpoints
    // with no pawn behind them. A cut carries nothing across (pin P6).
    if timing.previous_state.body.is_none() && timing.current_state.body.is_none() {
        return;
    }
    if snapped.contains(&riding.mover_id) {
        timing.hold_state(frame_timing::InterpolableState::new(after.eye));
        return;
    }
    let delta = before.eye - after.eye;
    if delta == Vec3::ZERO || !delta.is_finite() {
        return;
    }
    timing.shift_interval(delta);
    netcode::shift_local_presentation_offset(prediction, delta);
}

pub(crate) fn local_player_ground(
    registry: &postretro_entities::EntityRegistry,
) -> postretro_foundation::GroundRef {
    followed_player_pawn(registry)
        .and_then(|pawn| {
            registry
                .get_component::<postretro_foundation::PlayerMovementComponent>(pawn)
                .ok()
        })
        .map_or(postretro_foundation::GroundRef::Airborne, |movement| {
            movement.ground
        })
}

/// Follow the camera to the local pawn's eye. `presentation_offset` is the local-pawn
/// correction offset (the decaying composition of smoothed reconcile corrections); it
/// is added to the gameplay-authoritative registry transform so the first-person eye
/// glides across a reconcile correction without rubber-banding. Single-player and the
/// host pass `Vec3::ZERO`. The connected client's predicted tick passes the live
/// offset at tick rate, before its frame-timing push, so the render stage
/// interpolates presented eyes and never re-adds it.
pub(crate) fn follow_camera_to_local_pawn(
    camera: &mut Camera,
    registry: &postretro_entities::EntityRegistry,
    presentation_offset: Vec3,
) {
    if let Some(eye) = local_pawn_eye_position(registry, presentation_offset) {
        camera.position = eye;
    }
}

/// The eye endpoint a fixed tick pushes: the camera's position, and, when the
/// camera follows the local pawn's eye, that pawn's capsule centre, the world
/// offset baked into the eye, and the mover it stands on with the mover's pose
/// this tick, so the frame can place the eye on the drawn deck
/// (`movement.md` §6, Moving bases). A camera off the pawn's eye (fly camera,
/// no pawn) pushes its position alone.
pub(crate) fn eye_endpoint(
    camera_position: Vec3,
    registry: &postretro_entities::EntityRegistry,
    mover_states: &kinematic_mover::MoverTickStateTable,
    presentation_offset: Vec3,
) -> frame_timing::InterpolableState {
    use frame_timing::{EyeBody, EyeMover, InterpolableState};
    let plain = InterpolableState::new(camera_position);
    let Some(pawn) = followed_player_pawn(registry) else {
        return plain;
    };
    if pawn_eye_position(registry, pawn, presentation_offset) != Some(camera_position) {
        return plain;
    }
    let (Ok(movement), Ok(transform)) = (
        registry.get_component::<postretro_foundation::PlayerMovementComponent>(pawn),
        registry.get_component::<postretro_entities::Transform>(pawn),
    ) else {
        return plain;
    };
    let mover = match movement.ground {
        postretro_foundation::GroundRef::Mover(mover_id) => {
            mover_states.get(mover_id).map(|state| EyeMover {
                mover_id,
                entity: state.entity,
                pose: state.transform,
            })
        }
        _ => None,
    };
    InterpolableState::following(
        camera_position,
        EyeBody {
            centre: transform.position,
            offset: presentation_offset,
            mover,
        },
    )
}

/// The followed local pawn's eye; see [`pawn_eye_position`].
pub(crate) fn local_pawn_eye_position(
    registry: &postretro_entities::EntityRegistry,
    presentation_offset: Vec3,
) -> Option<Vec3> {
    pawn_eye_position(
        registry,
        followed_player_pawn(registry)?,
        presentation_offset,
    )
}

/// A movement pawn's presented eye: its registry position, plus the
/// presentation offset, plus the capsule's eye height. The one place the eye
/// offset is applied, so level install holds, and preloads lightmaps for, the
/// same point the first tick moves the camera to, and an adoption's camera cut
/// holds the eye the next tick presents.
pub(crate) fn pawn_eye_position(
    registry: &postretro_entities::EntityRegistry,
    pawn: postretro_entities::EntityId,
    presentation_offset: Vec3,
) -> Option<Vec3> {
    use postretro_entities::Transform;

    let component = registry
        .get_component::<postretro_foundation::PlayerMovementComponent>(pawn)
        .ok()?;
    let transform = registry.get_component::<Transform>(pawn).ok()?;
    Some(
        transform.position
            + presentation_offset
            + Vec3::new(0.0, component.capsule.eye_height, 0.0),
    )
}

#[cfg(test)]
#[path = "local_eye_tests.rs"]
mod tests;
