// Host-side muzzle pose for an NPC shot: a carried weapon's launch point,
// posed on the game-tick clip clock rather than the render clock.
// See: context/lib/entity_model.md §2 (Weapon mounts) · context/lib/resource_management.md §7

use glam::{Mat4, Vec3};

use postretro_entities::components::mesh::MeshComponent;
use postretro_model::anim::{Loop, sample_clip_looped_world};
use postretro_model::gltf_loader::SocketBinding;

use super::hit_zones::{HitZoneStore, ModelHitZones};

/// The socket name a mount bake carries at its barrel tip.
pub const BAKE_MUZZLE_SOCKET: &str = "muzzle";

/// Where a carried weapon's shots leave its holder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MuzzleMount<'a> {
    /// A bake mounted on a holder socket; the shot leaves the bake's own
    /// [`BAKE_MUZZLE_SOCKET`], composed as the renderer places the bake.
    Mounted {
        holder_socket: &'a str,
        bake: &'a str,
    },
    /// A weapon built into the body: the shot leaves this body socket.
    BodySocket(&'a str),
}

/// The animation state the clip clock tracks and its clip time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipPose<'a> {
    pub state: &'a str,
    /// Unwrapped clip-local time, the clip clock's `time`. A looping state
    /// wraps it; a one-shot state clamps it.
    pub time: f64,
}

/// The muzzle's world point, posed at `pose` on the drawn clip alone.
///
/// No crossfade and no presentation-only pose modifiers: gameplay probes read
/// the unmodified pose (`entity_model.md` §7), and the clip clock times one
/// state. `model_to_world` is the holder's gameplay model matrix at the facing
/// the shot uses. `scratch` holds the posed joints and is reused across calls.
///
/// `None` when the muzzle cannot be read: the tracked state or its clip does
/// not resolve, a model was never loaded, a socket is missing, or a bake's
/// `muzzle` is not a rigid node socket. The caller treats that as a missing
/// muzzle.
pub fn posed_muzzle_world(
    store: &HitZoneStore,
    model_to_world: Mat4,
    mesh: &MeshComponent,
    pose: ClipPose<'_>,
    mount: MuzzleMount<'_>,
    scratch: &mut Vec<Mat4>,
) -> Option<Vec3> {
    let holder = store.get_by_name(&mesh.model)?;
    let (holder_socket, bake_muzzle) = match mount {
        MuzzleMount::Mounted {
            holder_socket,
            bake,
        } => {
            let bake = store.get_by_name(bake)?;
            let SocketBinding::RigidRest(muzzle) = bake.sockets.get(BAKE_MUZZLE_SOCKET)? else {
                return None;
            };
            (holder_socket, *muzzle)
        }
        MuzzleMount::BodySocket(socket) => (socket, Mat4::IDENTITY),
    };
    let socket = match *holder.sockets.get(holder_socket)? {
        SocketBinding::SkinnedJoint(joint) => {
            pose_at_clip_time(holder, mesh, pose, scratch)?;
            *scratch.get(joint)?
        }
        SocketBinding::RigidRest(rest) => rest,
    };
    let world = model_to_world * socket * bake_muzzle;
    let point = world.transform_point3(Vec3::ZERO);
    point.is_finite().then_some(point)
}

/// Pose `holder`'s skeleton on the clip `pose.state` draws, at `pose.time`,
/// into model-space joint matrices.
fn pose_at_clip_time(
    holder: &ModelHitZones,
    mesh: &MeshComponent,
    pose: ClipPose<'_>,
    out: &mut Vec<Mat4>,
) -> Option<()> {
    let animation = mesh.animation.as_ref()?;
    let state = animation.states.get(pose.state)?;
    let clip = holder.state_clip(state.clip_index, &state.clip)?;
    let loop_policy = if state.looping {
        Loop::Wrap
    } else {
        Loop::Clamp
    };
    out.clear();
    sample_clip_looped_world(
        clip,
        holder.skeleton.as_ref(),
        pose.time as f32,
        loop_policy,
        out,
    );
    Some(())
}

#[cfg(test)]
#[path = "muzzle_pose_tests.rs"]
mod tests;
