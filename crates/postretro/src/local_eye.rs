// The local eye's presented pose: camera follow, eye height, mover yaw carry
// and its render residual, and authoritative mover corrections at the eye.
// See: context/lib/movement.md §6 (Moving bases)

use super::*;

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
        // A carry_yaw mover gliding onto a correction turns the drawn deck
        // only as the glide runs; the settled yaw took the whole correction at
        // apply, so the view adds the glide's yaw to stay on the drawn deck.
        let glide_yaw = registry
            .as_deref()
            .map_or(0.0, |registry| self.carried_glide_yaw(registry));
        PresentedAimPose {
            // Timing already carries the connected owner's presentation correction.
            position: eye,
            yaw: effective_render_yaw(
                self.camera.yaw,
                self.mover_yaw_carry_ground,
                &self.kinematic_mover_tick_states,
                alpha,
            ) + glide_yaw,
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

    /// The world-up yaw still to glide on the carry_yaw mover the view rides.
    fn carried_glide_yaw(&self, registry: &postretro_entities::EntityRegistry) -> f32 {
        let postretro_foundation::GroundRef::Mover(mover_id) = self.mover_yaw_carry_ground else {
            return 0.0;
        };
        let Some(state) = self.kinematic_mover_tick_states.get(mover_id) else {
            return 0.0;
        };
        if !state.carry_yaw {
            return 0.0;
        }
        registry
            .presentation_correction(state.entity)
            .map_or(0.0, |glide| world_up_yaw_delta(glide.rotation))
    }

    /// Draw the local pawn's mesh, shadow and mirror included, where this
    /// frame's eye places its body: on the drawn deck while riding, and on a
    /// connected client with the eye's world offset (option A of the rider
    /// brief). Anywhere else the mesh keeps its plain blend, bit for bit. Runs
    /// once per frame before any reader of the pawn's presented pose.
    pub(crate) fn place_local_pawn_body(&mut self, alpha: f32) {
        let connected_client = self.is_connected_client();
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let mut registry = session.scripting.script_ctx.registry.borrow_mut();
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
