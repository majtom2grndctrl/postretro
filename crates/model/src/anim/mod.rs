// CPU pose-sampling library: single-clip and two-source blended sampling, loop
// policies (wrap/clamp), snapshot capture, and animation clock helpers.
// See: context/lib/rendering_pipeline.md §9

mod blend;
mod compose;
mod track;
mod types;

use std::cell::RefCell;

use glam::Mat4;
use postretro_foundation::PoseInputs;

use crate::BonePaletteEntry;
use crate::pose_modifier::{PoseModifierStack, apply_pose_modifier_stack};
use crate::skeleton::{AnimationClip, Skeleton};

use blend::resolve_blend_into;
use compose::{compose_palette, compose_world_pose};
use track::{sample_local_pose, sample_local_trs};
use types::resolve_time;

pub use blend::capture_blend;
pub use types::{BlendSource, LocalTrs, Loop};

#[cfg(test)]
use crate::skeleton::{Interp, JointTracks, RestLocal, Track};
#[cfg(test)]
use glam::{Quat, Vec3};

thread_local! {
    /// Reusable world-pose scratch (one `Mat4` per joint) for the forward sweep.
    /// Cleared and refilled per call; grows to the largest skeleton seen and is
    /// reused thereafter so steady-state sampling does not allocate.
    static WORLD_POSE_SCRATCH: RefCell<Vec<Mat4>> = const { RefCell::new(Vec::new()) };

    /// Reusable per-joint local-TRS scratch for the blended sampler. The blend
    /// pass resolves each joint's blended local TRS into this buffer, then the
    /// existing forward sweep composes it once — so the blend path runs the
    /// hierarchy compose + inverse-bind sweep exactly once, like the single-clip
    /// path. Grows to the largest skeleton seen and is reused thereafter, so
    /// steady-state blended sampling allocates nothing.
    static BLEND_LOCAL_SCRATCH: RefCell<Vec<LocalTrs>> = const { RefCell::new(Vec::new()) };
}

/// Sample `clip` at `time` (seconds) against `skeleton`, writing one
/// [`BonePaletteEntry`] per joint (in skeleton/topo order) into `out`.
///
/// `Loop::Wrap` shorthand over [`sample_clip_looped`]: time is always wrapped
/// into `[0, duration)` so the clip loops. Production render paths carry an
/// explicit per-state loop policy and call [`sample_clip_looped`] directly;
/// this shorthand is retained for callers that always want the wrapping default.
///
/// Each output entry is the joint's **skinning matrix**: the composed world
/// joint transform multiplied by the joint's inverse-bind matrix, ready to
/// upload as one contiguous palette run. `out` is cleared then filled, so its
/// final length equals `skeleton.joints.len()`.
///
/// Per channel: interpolation follows the track's [`Interp`] mode — `Linear`
/// (component lerp for translation/scale, shortest-path slerp for rotation) or
/// `Step` (hold the lower bracketing key's value). A channel with **no keyframes** holds the
/// joint's rest-pose component (NOT identity) — the shipped clip omits scale, so
/// scale falls back to `Joint::rest_local.scale`. A non-positive duration samples
/// at `t = 0`.
///
/// Reuse: pass the same `out` every frame. A thread-local scratch holds the
/// world-pose sweep, so a steady-state call performs no heap allocation.
pub fn sample_clip(
    clip: &AnimationClip,
    skeleton: &Skeleton,
    time: f32,
    out: &mut Vec<BonePaletteEntry>,
) {
    sample_clip_looped(clip, skeleton, time, Loop::Wrap, out);
}

/// Sample `clip` at `time` (seconds) under `loop_policy` against `skeleton`,
/// writing the skinning palette into `out` (see [`sample_clip`] for the per-entry
/// contract).
///
/// The loop-aware single-clip path: `Loop::Wrap` repeats the clip (today's
/// behavior), `Loop::Clamp` holds the final keyframe forever after the clip ends
/// (one-shot states — attack, death). A non-positive duration samples at `t = 0`.
pub fn sample_clip_looped(
    clip: &AnimationClip,
    skeleton: &Skeleton,
    time: f32,
    loop_policy: Loop,
    out: &mut Vec<BonePaletteEntry>,
) {
    let t = resolve_time(clip.duration, time, loop_policy);
    compose_palette(skeleton, out, |i, joint| {
        // The clip's per-joint tracks are parallel to skeleton joints, but a
        // static-model / mismatched clip may be shorter — fall back to rest.
        sample_local_pose(clip.joints.get(i), &joint.rest_local, t)
    });
}

/// Blend two sources at `weight` (`0.0` → `a`, `1.0` → `b`) into one skinning
/// palette, writing one [`BonePaletteEntry`] per joint into `out`.
///
/// Each source is a [`BlendSource`] — a clip to sample (with its own time and
/// loop policy) or a borrowed per-joint local-TRS snapshot. Per joint the two
/// sources' **local** TRS are blended (component lerp for translation/scale,
/// shortest-path slerp for rotation; see [`blend::blend_local`]); the hierarchy
/// compose-and-inverse-bind sweep then runs **once** over the blended locals — so
/// this costs at most two clip samples per joint, never two full palette composes.
///
/// At `weight == 0.0` the palette equals `a`'s pose; at `1.0`, `b`'s; in between,
/// the per-joint blend. Reuse `out` across frames: a thread-local TRS scratch and
/// the world-pose scratch are both reused, so steady-state blended sampling
/// allocates nothing.
pub fn sample_blended(
    a: &BlendSource,
    b: &BlendSource,
    weight: f32,
    skeleton: &Skeleton,
    out: &mut Vec<BonePaletteEntry>,
) {
    BLEND_LOCAL_SCRATCH.with(|cell| {
        let mut locals = cell.borrow_mut();
        resolve_blend_into(a, b, weight, skeleton, &mut locals);
        compose_palette(skeleton, out, |i, _joint| locals[i].to_mat4());
    });
}

/// Sample one clip, apply an ordered local-TRS modifier stack, then compose the
/// skinning palette.
///
/// An empty `stack` or absent `inputs` immediately delegates to
/// [`sample_clip_looped`], preserving the fused, allocation-free unmodified
/// path. Only an active modified pose materializes one [`LocalTrs`] per joint.
pub fn sample_clip_looped_modified(
    clip: &AnimationClip,
    skeleton: &Skeleton,
    time: f32,
    loop_policy: Loop,
    stack: &PoseModifierStack,
    inputs: Option<&PoseInputs>,
    out: &mut Vec<BonePaletteEntry>,
) {
    let Some(inputs) = inputs.filter(|_| !stack.is_empty()) else {
        sample_clip_looped(clip, skeleton, time, loop_policy, out);
        return;
    };

    let t = resolve_time(clip.duration, time, loop_policy);
    BLEND_LOCAL_SCRATCH.with(|cell| {
        let mut locals = cell.borrow_mut();
        locals.clear();
        locals.reserve(skeleton.joints.len());
        for (i, joint) in skeleton.joints.iter().enumerate() {
            locals.push(sample_local_trs(clip.joints.get(i), &joint.rest_local, t));
        }
        apply_pose_modifier_stack(stack, inputs, &mut locals);
        compose_palette(skeleton, out, |i, _joint| locals[i].to_mat4());
    });
}

/// Blend two sources into local TRS, apply an ordered modifier stack, then
/// compose the skinning palette.
///
/// An empty `stack` or absent `inputs` immediately delegates to
/// [`sample_blended`], preserving its existing scratch/allocation behavior.
pub fn sample_blended_modified(
    a: &BlendSource,
    b: &BlendSource,
    weight: f32,
    skeleton: &Skeleton,
    stack: &PoseModifierStack,
    inputs: Option<&PoseInputs>,
    out: &mut Vec<BonePaletteEntry>,
) {
    let Some(inputs) = inputs.filter(|_| !stack.is_empty()) else {
        sample_blended(a, b, weight, skeleton, out);
        return;
    };

    BLEND_LOCAL_SCRATCH.with(|cell| {
        let mut locals = cell.borrow_mut();
        resolve_blend_into(a, b, weight, skeleton, &mut locals);
        apply_pose_modifier_stack(stack, inputs, &mut locals);
        compose_palette(skeleton, out, |i, _joint| locals[i].to_mat4());
    });
}

/// Sample `clip` at `time` (seconds) under `loop_policy` against `skeleton`,
/// writing each joint's **world-space** transform (PRE-inverse-bind, one
/// [`Mat4`] per joint, in skeleton/topo order) into `out`.
///
/// The world-pose counterpart of [`sample_clip_looped`]: same inputs, same
/// forward hierarchy compose ([`compose_world_pose`]) — but it stops at the
/// composed world joint transform instead of multiplying by the inverse-bind
/// matrix. That is the joint's placement in model space, which hit-zone /
/// attachment queries need (the skinning palette's inverse-bind product is not a
/// joint position; it maps bind-space vertices, so it is the wrong space for
/// locating a joint). Multiplying each output by that joint's inverse-bind matrix
/// recovers the skinning palette exactly.
///
/// Reuse: pass the same `out` every frame. `out` is cleared then filled to
/// `skeleton.joints.len()`, so a steady-state call performs no heap allocation —
/// the same contract as the palette samplers.
pub fn sample_clip_looped_world(
    clip: &AnimationClip,
    skeleton: &Skeleton,
    time: f32,
    loop_policy: Loop,
    out: &mut Vec<Mat4>,
) {
    let t = resolve_time(clip.duration, time, loop_policy);
    compose_world_pose(skeleton, out, |i, joint| {
        // The clip's per-joint tracks are parallel to skeleton joints, but a
        // static-model / mismatched clip may be shorter — fall back to rest.
        sample_local_pose(clip.joints.get(i), &joint.rest_local, t)
    });
}

/// Blend two sources at `weight` (`0.0` → `a`, `1.0` → `b`) into per-joint
/// **world-space** transforms (PRE-inverse-bind, one [`Mat4`] per joint, in
/// skeleton/topo order), writing into `out`.
///
/// The world-pose counterpart of [`sample_blended`]: same per-joint local blend
/// (see [`blend::blend_local`]) resolved through the same scratch, same single forward
/// compose ([`compose_world_pose`]) — but it stops at the composed world joint
/// transform instead of multiplying by the inverse-bind matrix (see
/// [`sample_clip_looped_world`] for why hit-zone / attachment queries want the
/// world pose, not the skinning matrix). Multiplying each output by that joint's
/// inverse-bind matrix recovers the blended skinning palette exactly.
///
/// Reuse `out` across frames: a thread-local TRS scratch is reused and `out` is
/// cleared then refilled, so steady-state world-pose blending allocates nothing —
/// the same contract as [`sample_blended`].
pub fn sample_blended_world(
    a: &BlendSource,
    b: &BlendSource,
    weight: f32,
    skeleton: &Skeleton,
    out: &mut Vec<Mat4>,
) {
    BLEND_LOCAL_SCRATCH.with(|cell| {
        let mut locals = cell.borrow_mut();
        resolve_blend_into(a, b, weight, skeleton, &mut locals);
        compose_world_pose(skeleton, out, |i, _joint| locals[i].to_mat4());
    });
}

#[cfg(test)]
mod tests;
