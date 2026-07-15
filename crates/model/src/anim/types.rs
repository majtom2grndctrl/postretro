// Animation sampling data types and clip-time policy.
// See: context/lib/rendering_pipeline.md §9

use glam::{Mat4, Quat, Vec3};

use crate::skeleton::AnimationClip;

/// How a clip's time is mapped onto its duration at the sampling boundary.
///
/// This is the per-sampled-clip loop policy: a looping clip wraps so it repeats,
/// a one-shot clip clamps and holds its final keyframe forever after the clip
/// ends. Which policy applies is the *caller's* decision (a state's loop flag);
/// this type only names the two behaviors so the sampler can apply them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loop {
    /// Wrap time into `[0, duration)` (`rem_euclid`) — the clip repeats.
    Wrap,
    /// Clamp time into `[0, duration]` — the clip holds its final keyframe after
    /// it ends (one-shot clips: attack, death).
    Clamp,
}

/// One joint's local-space transform in TRS form: the intermediate representation
/// the blended sampler blends in, and the element type of a captured "smooth"
/// snapshot buffer.
///
/// TRS, never a baked matrix: rotation must stay a quaternion so it can slerp.
/// A matrix snapshot could not be re-blended without decomposing it (and the
/// decompose is lossy / ambiguous for non-uniform scale), so the snapshot buffer
/// the "smooth" interrupt captures stores TRS directly. Small and `Copy` so a
/// per-joint buffer is cheap to fill and read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalTrs {
    /// Local translation (parent-relative).
    pub translation: Vec3,
    /// Local rotation (parent-relative unit quaternion).
    pub rotation: Quat,
    /// Local scale.
    pub scale: Vec3,
}

impl LocalTrs {
    /// Compose this local TRS to a `Mat4` in glTF node order
    /// (`translation * rotation * scale`), matching the single-clip path.
    pub(super) fn to_mat4(self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

/// One side of a blend: either a clip to sample (with its time and loop policy)
/// or a borrowed per-joint local-TRS snapshot.
///
/// The `Snapshot` arm is the "smooth" interrupt's captured pose — a static
/// per-joint local TRS that re-feeds as a blend source so an interrupted fade
/// resumes from the live blended pose with no discontinuity. The borrowed slice
/// must be parallel to the skeleton's joints (entry `i` is joint `i`); a joint
/// past the slice's end falls back to rest, mirroring a short clip.
pub enum BlendSource<'a> {
    /// Sample `clip` at `time` (seconds) under `loop_policy`.
    Clip {
        clip: &'a AnimationClip,
        time: f32,
        loop_policy: Loop,
    },
    /// Use this caller-provided per-joint local-TRS buffer directly.
    Snapshot(&'a [LocalTrs]),
}

/// Map a raw clip time onto the clip's duration under `loop_policy`. A
/// non-positive duration (static or malformed clip) always samples the first
/// frame. `Wrap` repeats the clip; `Clamp` holds the final keyframe past the end.
pub(super) fn resolve_time(duration: f32, time: f32, loop_policy: Loop) -> f32 {
    if duration > 0.0 {
        match loop_policy {
            Loop::Wrap => time.rem_euclid(duration),
            Loop::Clamp => time.clamp(0.0, duration),
        }
    } else {
        0.0
    }
}
