// CPU pose-sampling library: single-clip and two-source blended sampling, loop
// policies (wrap/clamp), snapshot capture, and animation clock helpers.
// See: context/lib/rendering_pipeline.md §9

use std::cell::RefCell;

use glam::{Mat4, Quat, Vec3};

use super::BonePaletteEntry;
use super::skeleton::{AnimationClip, Interp, Joint, JointTracks, RestLocal, Skeleton, Track};

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
    fn to_mat4(self) -> Mat4 {
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

impl BlendSource<'_> {
    /// Resolve this source's local TRS for joint `i` (parallel to the skeleton).
    /// A clip samples its tracks (holding `rest` for absent channels); a snapshot
    /// reads its buffer (holding `rest` past the buffer's end).
    fn local_at(&self, i: usize, rest: &RestLocal) -> LocalTrs {
        match self {
            BlendSource::Clip {
                clip,
                time,
                loop_policy,
            } => {
                let t = resolve_time(clip.duration, *time, *loop_policy);
                sample_local_trs(clip.joints.get(i), rest, t)
            }
            BlendSource::Snapshot(buf) => buf.get(i).copied().unwrap_or(LocalTrs {
                translation: rest.translation,
                rotation: rest.rotation,
                scale: rest.scale,
            }),
        }
    }
}

/// Map a raw clip time onto the clip's duration under `loop_policy`. A
/// non-positive duration (static or malformed clip) always samples the first
/// frame. `Wrap` repeats the clip; `Clamp` holds the final keyframe past the end.
fn resolve_time(duration: f32, time: f32, loop_policy: Loop) -> f32 {
    if duration > 0.0 {
        match loop_policy {
            Loop::Wrap => time.rem_euclid(duration),
            Loop::Clamp => time.clamp(0.0, duration),
        }
    } else {
        0.0
    }
}

/// Blend two local TRS values at `weight` (`0.0` → `a`, `1.0` → `b`).
/// Translation and scale lerp component-wise; rotation slerps along the shortest
/// path. The quats are put in the same hemisphere first (negate `b` if the dot is
/// negative) so a `1.0`/`0.0` weight reproduces the endpoint exactly and the
/// midpoint never takes the long way around.
fn blend_local(a: LocalTrs, b: LocalTrs, weight: f32) -> LocalTrs {
    let rot_a = a.rotation.normalize();
    let mut rot_b = b.rotation.normalize();
    if rot_a.dot(rot_b) < 0.0 {
        rot_b = -rot_b;
    }
    LocalTrs {
        translation: a.translation.lerp(b.translation, weight),
        rotation: rot_a.slerp(rot_b, weight).normalize(),
        scale: a.scale.lerp(b.scale, weight),
    }
}

/// Run the parent-before-child forward sweep, writing each joint's **world-space**
/// transform (PRE-inverse-bind, one [`Mat4`] per joint, in skeleton/topo order)
/// into `world`.
///
/// `local_of(i, joint)` returns joint `i`'s composed local transform (the
/// single-clip path samples + composes a clip; the blend path composes an
/// already-blended TRS). `world` is cleared then filled to
/// `skeleton.joints.len()`, so a steady-state call with a reused buffer performs
/// no heap allocation.
///
/// This is the shared hierarchy core: the world poses it produces are what the
/// skinning palette multiplies by each joint's inverse-bind matrix
/// ([`compose_palette`]) and what the world-joint samplers
/// ([`sample_clip_looped_world`], [`sample_blended_world`]) expose directly for
/// hit-zone / attachment queries. Factoring the sweep here keeps both paths on
/// exactly the same forward composition.
fn compose_world_pose(
    skeleton: &Skeleton,
    world: &mut Vec<Mat4>,
    mut local_of: impl FnMut(usize, &Joint) -> Mat4,
) {
    let joint_count = skeleton.joints.len();
    world.clear();
    world.reserve(joint_count);

    for (i, joint) in skeleton.joints.iter().enumerate() {
        let local = local_of(i, joint);

        // Forward sweep: parent-before-child topo order guarantees the
        // parent's world matrix is already in `world` when we reach a child.
        let world_pose = match joint.parent {
            Some(p) => world[p] * local,
            None => local,
        };
        world.push(world_pose);
    }
}

/// Compose a per-joint local-matrix function into the skinning palette: run the
/// parent-before-child forward sweep ([`compose_world_pose`]) and apply each
/// joint's inverse-bind matrix, writing one [`BonePaletteEntry`] per joint into
/// `out`.
///
/// `local_of(i, joint)` returns joint `i`'s composed local transform (the
/// single-clip path samples + composes a clip; the blend path composes an
/// already-blended TRS). Both `out` and the world-pose scratch are cleared then
/// filled, so a steady-state call with a reused `out` performs no heap
/// allocation. The hierarchy compose happens **once** here (in the shared core),
/// then the inverse-bind multiply runs per joint — the blend path resolves its
/// per-joint blend before this, never inside it.
fn compose_palette(
    skeleton: &Skeleton,
    out: &mut Vec<BonePaletteEntry>,
    local_of: impl FnMut(usize, &Joint) -> Mat4,
) {
    let joint_count = skeleton.joints.len();
    out.clear();
    out.reserve(joint_count);

    WORLD_POSE_SCRATCH.with(|cell| {
        let mut world = cell.borrow_mut();
        compose_world_pose(skeleton, &mut world, local_of);

        for (joint, world_pose) in skeleton.joints.iter().zip(world.iter()) {
            let inverse_bind = Mat4::from_cols_array_2d(&joint.inverse_bind);
            let skinning = *world_pose * inverse_bind;
            out.push(BonePaletteEntry {
                matrix: skinning.to_cols_array_2d(),
            });
        }
    });
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
#[cfg_attr(not(test), allow(dead_code))]
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
/// shortest-path slerp for rotation; see [`blend_local`]); the hierarchy
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
#[cfg_attr(not(test), allow(dead_code))]
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
/// (see [`blend_local`]) resolved through the same scratch, same single forward
/// compose ([`compose_world_pose`]) — but it stops at the composed world joint
/// transform instead of multiplying by the inverse-bind matrix (see
/// [`sample_clip_looped_world`] for why hit-zone / attachment queries want the
/// world pose, not the skinning matrix). Multiplying each output by that joint's
/// inverse-bind matrix recovers the blended skinning palette exactly.
///
/// Reuse `out` across frames: a thread-local TRS scratch is reused and `out` is
/// cleared then refilled, so steady-state world-pose blending allocates nothing —
/// the same contract as [`sample_blended`].
#[cfg_attr(not(test), allow(dead_code))]
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

/// Blend two sources at `weight` (`0.0` → `a`, `1.0` → `b`) into a per-joint
/// local-TRS buffer (one [`LocalTrs`] per skeleton joint), writing into `out`.
///
/// This is the "smooth" interrupt's one-time snapshot capture: it evaluates the
/// same per-joint blend [`sample_blended`] composes, but stops at the local TRS
/// instead of composing to matrices — so the captured pose can be fed back as a
/// [`BlendSource::Snapshot`] and re-blended (a matrix snapshot could not slerp).
/// Either source may itself be a snapshot, so a snapshot-fade interrupted again
/// captures `blend(snapshot, clip)` through this same path.
///
/// `out` is cleared then filled to `skeleton.joints.len()`. Capture is a
/// one-time event (not a steady-state per-frame call), so a growing `out` here is
/// not on the hot path — but reuse is still safe and free of churn.
pub fn capture_blend(
    a: &BlendSource,
    b: &BlendSource,
    weight: f32,
    skeleton: &Skeleton,
    out: &mut Vec<LocalTrs>,
) {
    resolve_blend_into(a, b, weight, skeleton, out);
}

/// Resolve the per-joint blend of two sources at `weight` into `out` (one
/// [`LocalTrs`] per skeleton joint). The shared core of [`sample_blended`] (which
/// then composes the result once) and [`capture_blend`] (which returns it as the
/// snapshot buffer), so both paths run identical per-joint blend math.
fn resolve_blend_into(
    a: &BlendSource,
    b: &BlendSource,
    weight: f32,
    skeleton: &Skeleton,
    out: &mut Vec<LocalTrs>,
) {
    out.clear();
    out.reserve(skeleton.joints.len());
    for (i, joint) in skeleton.joints.iter().enumerate() {
        let la = a.local_at(i, &joint.rest_local);
        let lb = b.local_at(i, &joint.rest_local);
        out.push(blend_local(la, lb, weight));
    }
}

/// Resolve one joint's local TRS at time `t`: each channel interpolates its
/// keyframes if present, else holds the rest-pose component. Returns the raw TRS
/// so the blend path can blend two of them as quaternions before composing; the
/// single-clip path composes one to a `Mat4` via [`sample_local_pose`].
fn sample_local_trs(tracks: Option<&JointTracks>, rest: &RestLocal, t: f32) -> LocalTrs {
    let (translation, rotation, scale) = match tracks {
        Some(tr) => (
            sample_vec3_track(&tr.translation, t).unwrap_or(rest.translation),
            sample_quat_track(&tr.rotation, t).unwrap_or(rest.rotation),
            sample_vec3_track(&tr.scale, t).unwrap_or(rest.scale),
        ),
        None => (rest.translation, rest.rotation, rest.scale),
    };
    LocalTrs {
        translation,
        rotation,
        scale,
    }
}

/// Resolve one joint's local TRS at time `t` and compose it to a `Mat4` in TRS
/// order (`translation * rotation * scale`), matching glTF's node transform
/// convention. The composing wrapper over [`sample_local_trs`] for the
/// single-clip path.
fn sample_local_pose(tracks: Option<&JointTracks>, rest: &RestLocal, t: f32) -> Mat4 {
    sample_local_trs(tracks, rest, t).to_mat4()
}

/// Find the keyframe span bracketing `t` and the fraction within it.
///
/// Returns `None` for an empty track (channel not animated → caller holds rest).
/// Otherwise `(i0, i1, frac)` where the value is `lerp(values[i0], values[i1],
/// frac)`. Before the first key the result clamps to it (`i0 == i1 == 0`); after
/// the last key it clamps to the last (`i0 == i1 == last`).
fn locate_span(times: &[f32], t: f32) -> Option<(usize, usize, f32)> {
    if times.is_empty() {
        return None;
    }
    if t <= times[0] {
        return Some((0, 0, 0.0));
    }
    let last = times.len() - 1;
    if t >= times[last] {
        return Some((last, last, 0.0));
    }
    // `times` is ascending; binary-search for the first key strictly after `t`.
    // `partition_point` returns the count of keys `<= t`, so `i1` is the upper
    // key and `i0 = i1 - 1` the lower. Both in-range given the clamps above.
    let i1 = times.partition_point(|&k| k <= t);
    let i0 = i1 - 1;
    let span = times[i1] - times[i0];
    let frac = if span > 0.0 {
        ((t - times[i0]) / span).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Some((i0, i1, frac))
}

/// Sample a `Vec3` track (translation/scale). `Linear` lerps component-wise
/// between the bracketing keys; `Step` holds the lower key (`i0`) with no blend.
fn sample_vec3_track(track: &Track<Vec3>, t: f32) -> Option<Vec3> {
    let (i0, i1, frac) = locate_span(&track.times, t)?;
    let a = track.values[i0];
    match track.mode {
        Interp::Step => Some(a),
        Interp::Linear => {
            let b = track.values[i1];
            Some(a.lerp(b, frac))
        }
    }
}

/// Sample a `Quat` rotation track. `Linear` slerps (shortest-path) between the
/// bracketing keys — endpoints are normalized (authored quats may drift) and
/// glam's `slerp` handles the dot-sign flip internally, so the interpolation
/// never takes the long way around. `Step` holds the lower key (`i0`).
fn sample_quat_track(track: &Track<Quat>, t: f32) -> Option<Quat> {
    let (i0, i1, frac) = locate_span(&track.times, t)?;
    let a = track.values[i0].normalize();
    if i0 == i1 || track.mode == Interp::Step {
        return Some(a);
    }
    let b = track.values[i1].normalize();
    // glam's `slerp` already picks the shortest arc (it negates `b` when the dot
    // is negative), so we get the correct hemisphere without a manual flip.
    Some(a.slerp(b, frac).normalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::skeleton::Joint;

    const EPS: f32 = 1.0e-5;

    fn assert_vec3_eq(a: Vec3, b: Vec3, ctx: &str) {
        assert!(
            (a - b).length() < EPS,
            "{ctx}: expected {b:?}, got {a:?} (|d|={})",
            (a - b).length()
        );
    }

    fn assert_quat_eq(a: Quat, b: Quat, ctx: &str) {
        // Quats q and -q are the same rotation; compare via the angle between.
        let dot = a.normalize().dot(b.normalize()).abs().min(1.0);
        let angle = 2.0 * dot.acos();
        assert!(
            angle < 1.0e-3,
            "{ctx}: expected {b:?}, got {a:?} (angle={angle})"
        );
    }

    fn assert_mat4_eq(a: Mat4, b: Mat4, ctx: &str) {
        let (ca, cb) = (a.to_cols_array(), b.to_cols_array());
        for i in 0..16 {
            assert!(
                (ca[i] - cb[i]).abs() < 1.0e-4,
                "{ctx}: element {i} expected {}, got {}",
                cb[i],
                ca[i]
            );
        }
    }

    fn joint(parent: Option<usize>, inverse_bind: Mat4, rest: RestLocal) -> Joint {
        Joint {
            parent,
            inverse_bind: inverse_bind.to_cols_array_2d(),
            rest_local: rest,
        }
    }

    fn translation_clip(name: &str, duration: f32, joints: Vec<JointTracks>) -> AnimationClip {
        AnimationClip {
            name: name.to_string(),
            duration,
            joints,
        }
    }

    /// Child world/skinning matrix equals parentWorld * childLocal * inverseBind.
    #[test]
    fn hierarchy_composes_child_world_through_parent() {
        // Root at +X(2); child rest-translated +Y(3) relative to root, animated
        // to hold that rest (empty tracks). Inverse-binds chosen non-identity so
        // the multiply order is actually exercised.
        let root_ib = Mat4::from_translation(Vec3::new(-2.0, 0.0, 0.0));
        let child_ib = Mat4::from_translation(Vec3::new(0.0, -5.0, 0.0));
        let skeleton = Skeleton {
            joints: vec![
                joint(
                    None,
                    root_ib,
                    RestLocal {
                        translation: Vec3::new(2.0, 0.0, 0.0),
                        ..Default::default()
                    },
                ),
                joint(
                    Some(0),
                    child_ib,
                    RestLocal {
                        translation: Vec3::new(0.0, 3.0, 0.0),
                        ..Default::default()
                    },
                ),
            ],
        };
        // Empty tracks → rest pose held.
        let clip = translation_clip("rest", 1.0, vec![JointTracks::default(); 2]);

        let mut out = Vec::new();
        sample_clip(&clip, &skeleton, 0.25, &mut out);
        assert_eq!(out.len(), 2);

        // Expected: rebuild by hand.
        let root_local = Mat4::from_translation(Vec3::new(2.0, 0.0, 0.0));
        let child_local = Mat4::from_translation(Vec3::new(0.0, 3.0, 0.0));
        let root_world = root_local;
        let child_world = root_world * child_local;
        let expected_child = child_world * child_ib;

        assert_mat4_eq(
            Mat4::from_cols_array_2d(&out[1].matrix),
            expected_child,
            "child skinning = parentWorld * childLocal * childInverseBind",
        );
        assert_mat4_eq(
            Mat4::from_cols_array_2d(&out[0].matrix),
            root_world * root_ib,
            "root skinning = rootWorld * rootInverseBind",
        );
    }

    /// Two translation keys → midpoint sample is the lerped position.
    #[test]
    fn translation_track_lerps_at_midpoint() {
        let skeleton = Skeleton {
            joints: vec![joint(None, Mat4::IDENTITY, RestLocal::default())],
        };
        let tracks = JointTracks {
            translation: Track {
                times: vec![0.0, 2.0],
                values: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, -4.0, 2.0)],
                ..Default::default()
            },
            ..Default::default()
        };
        let clip = translation_clip("t", 2.0, vec![tracks]);

        let mut out = Vec::new();
        // t = 1.0 is the midpoint of [0, 2].
        sample_clip(&clip, &skeleton, 1.0, &mut out);
        let translation = Mat4::from_cols_array_2d(&out[0].matrix).w_axis.truncate();
        assert_vec3_eq(translation, Vec3::new(5.0, -2.0, 1.0), "midpoint lerp");
    }

    /// Two rotation keys → midpoint slerp is the half-angle rotation.
    #[test]
    fn rotation_track_slerps_at_midpoint() {
        let skeleton = Skeleton {
            joints: vec![joint(None, Mat4::IDENTITY, RestLocal::default())],
        };
        let q0 = Quat::IDENTITY;
        let q1 = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2); // 90°
        let tracks = JointTracks {
            rotation: Track {
                times: vec![0.0, 1.0],
                values: vec![q0, q1],
                ..Default::default()
            },
            ..Default::default()
        };
        let clip = translation_clip("r", 1.0, vec![tracks]);

        let mut out = Vec::new();
        sample_clip(&clip, &skeleton, 0.5, &mut out);
        let sampled = Quat::from_mat4(&Mat4::from_cols_array_2d(&out[0].matrix));
        let expected = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4); // 45°
        assert_quat_eq(sampled, expected, "midpoint slerp = half angle");
    }

    /// An empty SCALE track holds the joint's rest scale (not identity 1,1,1).
    /// An empty translation/rotation track holds rest translation/rotation too.
    #[test]
    fn empty_channel_holds_rest_pose() {
        let rest = RestLocal {
            translation: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_3),
            scale: Vec3::new(0.5, 0.5, 0.5),
        };
        let skeleton = Skeleton {
            joints: vec![joint(None, Mat4::IDENTITY, rest)],
        };
        // Clip animates ONLY translation; scale + rotation tracks are empty and
        // must fall back to rest (rest scale 0.5, NOT 1.0).
        let tracks = JointTracks {
            translation: Track {
                times: vec![0.0, 1.0],
                values: vec![Vec3::new(1.0, 2.0, 3.0), Vec3::new(1.0, 2.0, 3.0)],
                ..Default::default()
            },
            ..Default::default()
        };
        let clip = translation_clip("partial", 1.0, vec![tracks]);

        let mut out = Vec::new();
        sample_clip(&clip, &skeleton, 0.5, &mut out);

        let m = Mat4::from_cols_array_2d(&out[0].matrix);
        let (scale, rotation, translation) = m.to_scale_rotation_translation();
        assert_vec3_eq(scale, rest.scale, "empty scale track holds rest scale");
        assert_quat_eq(rotation, rest.rotation, "empty rotation track holds rest");
        assert_vec3_eq(
            translation,
            rest.translation,
            "translation animated to rest value",
        );
    }

    /// Sampling at t = duration + ε equals sampling at ε (the clip loops).
    #[test]
    fn time_wraps_at_duration() {
        let skeleton = Skeleton {
            joints: vec![joint(None, Mat4::IDENTITY, RestLocal::default())],
        };
        let tracks = JointTracks {
            translation: Track {
                times: vec![0.0, 2.0],
                values: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(8.0, 0.0, 0.0)],
                ..Default::default()
            },
            ..Default::default()
        };
        let clip = translation_clip("wrap", 2.0, vec![tracks]);

        let eps = 0.1f32;
        let mut early = Vec::new();
        sample_clip(&clip, &skeleton, eps, &mut early);
        let mut wrapped = Vec::new();
        sample_clip(&clip, &skeleton, clip.duration + eps, &mut wrapped);

        let p_early = Mat4::from_cols_array_2d(&early[0].matrix).w_axis.truncate();
        let p_wrapped = Mat4::from_cols_array_2d(&wrapped[0].matrix)
            .w_axis
            .truncate();
        assert_vec3_eq(p_wrapped, p_early, "t = duration + eps wraps to t = eps");
    }

    /// Tripwire 2 (CPU-only, no GPU): measure per-frame `sample_clip` cost on the
    /// real shipped skeleton + clip and print a min/mean/max summary. This is the
    /// CPU pose-sampling figure `findings.md` projects to wave scale; it needs no
    /// renderer, so it runs here. Gated on the asset existing (mirrors the loader's
    /// real-model test) and `#[ignore]`d so it only runs on demand:
    ///   cargo test -p postretro --release sample_clip_cpu_cost -- --ignored --nocapture
    /// (Run `--release` for a representative steady-state figure; debug is far
    /// slower and not the number to report.)
    #[test]
    #[ignore = "measurement; run explicitly with --ignored --nocapture (prefer --release)"]
    fn sample_clip_cpu_cost_on_real_model() {
        use std::path::PathBuf;
        use std::time::Instant;

        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/dev/models/decraniated_low_poly_retro_pixel/scene.gltf");
        if !path.exists() {
            eprintln!("skipping: model asset not present at {}", path.display());
            return;
        }
        let model = crate::model::gltf_loader::load_model(&path).expect("model loads");
        let clip = model.clips.first().expect("model has one clip");
        let skeleton = &model.skeleton;

        // Warm the thread-local scratch + caches so the first sample doesn't skew
        // the window.
        let mut out = Vec::new();
        for i in 0..64 {
            sample_clip(clip, skeleton, i as f32 * 0.016, &mut out);
        }

        const SAMPLES: u32 = 100_000;
        let mut min = u64::MAX;
        let mut max = 0u64;
        let mut total: u128 = 0;
        let mut t = 0.0f32;
        for _ in 0..SAMPLES {
            t += 0.016; // advance ~1 frame at 60fps so we sweep the whole clip
            let start = Instant::now();
            sample_clip(clip, skeleton, t, &mut out);
            let ns = start.elapsed().as_nanos() as u64;
            min = min.min(ns);
            max = max.max(ns);
            total += ns as u128;
        }
        let mean_us = (total as f64 / SAMPLES as f64) / 1000.0;
        eprintln!(
            "[sample_clip CPU cost] joints={} samples={} min={:.3}us mean={:.3}us max={:.3}us | \
             projected wave N=200: {:.1}us/frame ({:.3}ms)",
            skeleton.joints.len(),
            SAMPLES,
            min as f64 / 1000.0,
            mean_us,
            max as f64 / 1000.0,
            mean_us * 200.0,
            mean_us * 200.0 / 1000.0,
        );

        // Sanity only — the measurement is the print above, not a threshold.
        assert!(out.len() == skeleton.joints.len());
    }

    /// A STEP translation track holds the lower keyframe's value between keys and
    /// snaps to a keyframe's value at/after that keyframe's time — no lerp.
    #[test]
    fn step_translation_track_holds_lower_keyframe() {
        let skeleton = Skeleton {
            joints: vec![joint(None, Mat4::IDENTITY, RestLocal::default())],
        };
        // Three keys so the snap-at-key assertion lands on an interior key (t=2),
        // away from t = duration where `sample_clip` wraps the time to 0.
        let k0 = Vec3::new(0.0, 0.0, 0.0);
        let k1 = Vec3::new(10.0, 0.0, 0.0);
        let k2 = Vec3::new(20.0, 0.0, 0.0);
        let tracks = JointTracks {
            translation: Track {
                times: vec![0.0, 2.0, 4.0],
                values: vec![k0, k1, k2],
                mode: Interp::Step,
            },
            ..Default::default()
        };
        let clip = translation_clip("step", 4.0, vec![tracks]);

        let sample_at = |t: f32| {
            let mut out = Vec::new();
            sample_clip(&clip, &skeleton, t, &mut out);
            Mat4::from_cols_array_2d(&out[0].matrix).w_axis.truncate()
        };
        // Between two keys: holds the LOWER key, not the midpoint lerp a LINEAR
        // track would yield ((5,0,0) on [k0,k1]).
        assert_vec3_eq(sample_at(1.0), k0, "STEP holds lower key mid-span");
        assert_vec3_eq(sample_at(1.99), k0, "STEP holds lower key just before next");
        // At and after the (interior) keyframe time: snaps to that key's value.
        assert_vec3_eq(sample_at(2.0), k1, "STEP snaps at the keyframe time");
        assert_vec3_eq(sample_at(3.0), k1, "STEP holds k1 until the next key");
    }

    /// LINEAR remains the default and still interpolates (regression guard that
    /// adding the mode field did not change default behavior).
    #[test]
    fn linear_default_track_still_lerps() {
        let skeleton = Skeleton {
            joints: vec![joint(None, Mat4::IDENTITY, RestLocal::default())],
        };
        // Field-elided construction: `mode` defaults to Interp::Linear.
        let tracks = JointTracks {
            translation: Track {
                times: vec![0.0, 2.0],
                values: vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)],
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            tracks.translation.mode,
            Interp::Linear,
            "mode defaults LINEAR"
        );
        let clip = translation_clip("lin", 2.0, vec![tracks]);
        let mut out = Vec::new();
        sample_clip(&clip, &skeleton, 1.0, &mut out);
        let p = Mat4::from_cols_array_2d(&out[0].matrix).w_axis.truncate();
        assert_vec3_eq(p, Vec3::new(5.0, 0.0, 0.0), "LINEAR midpoint lerps");
    }

    /// A STEP rotation track holds the lower keyframe (no slerp between keys).
    #[test]
    fn step_rotation_track_holds_lower_keyframe() {
        let skeleton = Skeleton {
            joints: vec![joint(None, Mat4::IDENTITY, RestLocal::default())],
        };
        let q0 = Quat::IDENTITY;
        let q1 = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let tracks = JointTracks {
            rotation: Track {
                times: vec![0.0, 1.0],
                values: vec![q0, q1],
                mode: Interp::Step,
            },
            ..Default::default()
        };
        let clip = translation_clip("stepr", 1.0, vec![tracks]);
        let mut out = Vec::new();
        // Midpoint: a LINEAR track would slerp to 45°; STEP holds q0 (0°).
        sample_clip(&clip, &skeleton, 0.5, &mut out);
        let sampled = Quat::from_mat4(&Mat4::from_cols_array_2d(&out[0].matrix));
        assert_quat_eq(sampled, q0, "STEP rotation holds lower key (no slerp)");
    }

    /// `out` is cleared and refilled, and reuse across calls does not change the
    /// result — the steady-state allocation-free reuse path the renderer uses.
    #[test]
    fn reused_out_buffer_is_cleared_and_refilled() {
        let skeleton = Skeleton {
            joints: vec![joint(None, Mat4::IDENTITY, RestLocal::default())],
        };
        let clip = translation_clip("rest", 1.0, vec![JointTracks::default()]);

        let mut out = vec![
            BonePaletteEntry {
                matrix: [[9.0; 4]; 4]
            };
            5
        ];
        sample_clip(&clip, &skeleton, 0.0, &mut out);
        assert_eq!(
            out.len(),
            1,
            "out resized to joint count, stale entries gone"
        );
        // Second call reuses the same buffer and yields the same result.
        let first = out[0];
        sample_clip(&clip, &skeleton, 0.0, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], first, "reuse is deterministic");
    }

    // --- Blended and loop-policy sampling ---

    /// Decompose a palette entry's skinning matrix back to (translation,
    /// rotation, scale). The blend tests use identity inverse-binds and a single
    /// root joint, so the skinning matrix *is* the joint's local transform.
    fn decompose(entry: BonePaletteEntry) -> (Vec3, Quat, Vec3) {
        let (s, r, t) = Mat4::from_cols_array_2d(&entry.matrix).to_scale_rotation_translation();
        (t, r, s)
    }

    /// A single-root skeleton with identity inverse-bind, so a sampled palette
    /// entry decomposes straight back to the joint's local TRS.
    fn single_root_skeleton() -> Skeleton {
        Skeleton {
            joints: vec![joint(None, Mat4::IDENTITY, RestLocal::default())],
        }
    }

    /// A one-joint clip that holds a constant local TRS (single key on each
    /// channel), so it samples to exactly `(t, r, s)` at any time.
    fn constant_pose_clip(name: &str, t: Vec3, r: Quat, s: Vec3) -> AnimationClip {
        let tracks = JointTracks {
            translation: Track {
                times: vec![0.0],
                values: vec![t],
                ..Default::default()
            },
            rotation: Track {
                times: vec![0.0],
                values: vec![r],
                ..Default::default()
            },
            scale: Track {
                times: vec![0.0],
                values: vec![s],
                ..Default::default()
            },
        };
        AnimationClip {
            name: name.to_string(),
            duration: 1.0,
            joints: vec![tracks],
        }
    }

    /// Blend weight 0 reproduces source A's pose; weight 1 reproduces source B's;
    /// the midpoint differs from both. The endpoints must be exact (slerp
    /// hemisphere handling) — a blend is not allowed to perturb an endpoint.
    #[test]
    fn blend_endpoints_reproduce_each_source_midpoint_differs() {
        let skeleton = single_root_skeleton();
        let pose_a = (Vec3::new(1.0, 0.0, 0.0), Quat::IDENTITY, Vec3::splat(1.0));
        let pose_b = (
            Vec3::new(0.0, 4.0, 0.0),
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            Vec3::splat(2.0),
        );
        let clip_a = constant_pose_clip("a", pose_a.0, pose_a.1, pose_a.2);
        let clip_b = constant_pose_clip("b", pose_b.0, pose_b.1, pose_b.2);
        let src_a = BlendSource::Clip {
            clip: &clip_a,
            time: 0.0,
            loop_policy: Loop::Wrap,
        };
        let src_b = BlendSource::Clip {
            clip: &clip_b,
            time: 0.0,
            loop_policy: Loop::Wrap,
        };

        let mut out = Vec::new();

        sample_blended(&src_a, &src_b, 0.0, &skeleton, &mut out);
        let (t0, r0, s0) = decompose(out[0]);
        assert_vec3_eq(t0, pose_a.0, "weight 0 → A translation");
        assert_quat_eq(r0, pose_a.1, "weight 0 → A rotation");
        assert_vec3_eq(s0, pose_a.2, "weight 0 → A scale");

        sample_blended(&src_a, &src_b, 1.0, &skeleton, &mut out);
        let (t1, r1, s1) = decompose(out[0]);
        assert_vec3_eq(t1, pose_b.0, "weight 1 → B translation");
        assert_quat_eq(r1, pose_b.1, "weight 1 → B rotation");
        assert_vec3_eq(s1, pose_b.2, "weight 1 → B scale");

        sample_blended(&src_a, &src_b, 0.5, &skeleton, &mut out);
        let (tm, rm, sm) = decompose(out[0]);
        // Translation/scale lerp; rotation slerps to the half angle.
        assert_vec3_eq(
            tm,
            pose_a.0.lerp(pose_b.0, 0.5),
            "midpoint translation lerp",
        );
        assert_vec3_eq(sm, pose_a.2.lerp(pose_b.2, 0.5), "midpoint scale lerp");
        assert_quat_eq(
            rm,
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_4),
            "midpoint rotation = half angle",
        );
        // And the midpoint is genuinely between, not at either endpoint.
        assert!((tm - pose_a.0).length() > EPS, "midpoint differs from A");
        assert!((tm - pose_b.0).length() > EPS, "midpoint differs from B");
    }

    /// Shortest-path slerp: blending 170° and −170° about Z goes the short way
    /// (through 180°), not the long way (through 0°). The midpoint is 180°.
    /// The endpoints being in opposite hemispheres is the case the manual flip guards.
    #[test]
    fn blend_rotation_takes_shortest_path() {
        let skeleton = single_root_skeleton();
        // 170° each side of zero about Z: shortest arc between them passes through
        // 180°, the long arc through 0°. Midpoint must land near 180°, not 0°.
        let r_a = Quat::from_rotation_z(170f32.to_radians());
        let r_b = Quat::from_rotation_z(-170f32.to_radians());
        let clip_a = constant_pose_clip("a", Vec3::ZERO, r_a, Vec3::ONE);
        let clip_b = constant_pose_clip("b", Vec3::ZERO, r_b, Vec3::ONE);
        let src_a = BlendSource::Clip {
            clip: &clip_a,
            time: 0.0,
            loop_policy: Loop::Wrap,
        };
        let src_b = BlendSource::Clip {
            clip: &clip_b,
            time: 0.0,
            loop_policy: Loop::Wrap,
        };

        let mut out = Vec::new();
        sample_blended(&src_a, &src_b, 0.5, &skeleton, &mut out);
        let (_, rm, _) = decompose(out[0]);
        // Shortest arc midpoint is ±180° about Z (through the back), NOT identity:
        // 170° and -170° are 20° apart the short way, so their midpoint is 180°.
        assert_quat_eq(
            rm,
            Quat::from_rotation_z(180f32.to_radians()),
            "shortest-path midpoint of 170°/-170° is 180°, not 0°",
        );
    }

    /// A looping clip wraps past its duration; a non-looping clip clamps and holds
    /// its final keyframe forever after the clip ends.
    #[test]
    fn loop_policy_wraps_or_clamps_past_duration() {
        let skeleton = single_root_skeleton();
        // Translation 0 → 8 over [0, 2]. After the end: Wrap repeats (t=2.1 ≡ 0.1),
        // Clamp holds the final key (8).
        let tracks = JointTracks {
            translation: Track {
                times: vec![0.0, 2.0],
                values: vec![Vec3::ZERO, Vec3::new(8.0, 0.0, 0.0)],
                ..Default::default()
            },
            ..Default::default()
        };
        let clip = translation_clip("loopclamp", 2.0, vec![tracks]);

        let pos = |time: f32, policy: Loop| {
            let mut out = Vec::new();
            sample_clip_looped(&clip, &skeleton, time, policy, &mut out);
            Mat4::from_cols_array_2d(&out[0].matrix).w_axis.truncate()
        };

        // Just past the end.
        let wrapped = pos(2.1, Loop::Wrap);
        let clamped = pos(2.1, Loop::Clamp);
        // Wrap ≡ sampling at 0.1 (linearly 0.4 along x).
        assert_vec3_eq(wrapped, pos(0.1, Loop::Wrap), "Wrap repeats the clip");
        // Clamp holds the final keyframe value (8,0,0).
        assert_vec3_eq(
            clamped,
            Vec3::new(8.0, 0.0, 0.0),
            "Clamp holds final keyframe",
        );
        // Far past the end the clamp still holds — the death pose persists.
        assert_vec3_eq(
            pos(100.0, Loop::Clamp),
            Vec3::new(8.0, 0.0, 0.0),
            "Clamp holds indefinitely",
        );

        // `sample_clip` (the Wrap shorthand) matches `sample_clip_looped(Wrap)`.
        let mut shorthand = Vec::new();
        sample_clip(&clip, &skeleton, 2.1, &mut shorthand);
        assert_vec3_eq(
            Mat4::from_cols_array_2d(&shorthand[0].matrix)
                .w_axis
                .truncate(),
            wrapped,
            "sample_clip defaults to Wrap",
        );
    }

    /// `capture_blend` produces a per-joint local-TRS buffer that, fed back as a
    /// `Snapshot` blend source, reproduces the captured pose exactly — so a
    /// "smooth" interrupt resumes from the live blended pose with no discontinuity.
    #[test]
    fn captured_snapshot_reproduces_blended_pose() {
        let skeleton = single_root_skeleton();
        let clip_a = constant_pose_clip(
            "a",
            Vec3::new(1.0, 2.0, 3.0),
            Quat::from_rotation_y(0.3),
            Vec3::splat(1.5),
        );
        let clip_b = constant_pose_clip(
            "b",
            Vec3::new(-4.0, 0.0, 5.0),
            Quat::from_rotation_x(1.1),
            Vec3::splat(0.5),
        );
        let src_a = BlendSource::Clip {
            clip: &clip_a,
            time: 0.0,
            loop_policy: Loop::Wrap,
        };
        let src_b = BlendSource::Clip {
            clip: &clip_b,
            time: 0.0,
            loop_policy: Loop::Wrap,
        };

        // The live blended palette at an arbitrary mid-fade weight.
        let mut live = Vec::new();
        sample_blended(&src_a, &src_b, 0.4, &skeleton, &mut live);

        // Capture that same blend into a snapshot buffer.
        let mut snapshot = Vec::new();
        capture_blend(&src_a, &src_b, 0.4, &skeleton, &mut snapshot);
        assert_eq!(snapshot.len(), skeleton.joints.len());

        // Feeding the snapshot back at weight 0 (snapshot vs anything) reproduces
        // the captured pose — the interrupt has no discontinuity.
        let snap_src = BlendSource::Snapshot(&snapshot);
        let mut resumed = Vec::new();
        sample_blended(&snap_src, &src_a, 0.0, &skeleton, &mut resumed);
        for (l, r) in live.iter().zip(resumed.iter()) {
            assert_mat4_eq(
                Mat4::from_cols_array_2d(&r.matrix),
                Mat4::from_cols_array_2d(&l.matrix),
                "snapshot reproduces live blended pose",
            );
        }
    }

    /// A snapshot-fade interrupted again captures `blend(snapshot, clip)` through
    /// the same path — the snapshot arm works as either blend operand.
    #[test]
    fn capture_blends_snapshot_against_clip() {
        let skeleton = single_root_skeleton();
        let snapshot = vec![LocalTrs {
            translation: Vec3::new(2.0, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }];
        let clip = constant_pose_clip("c", Vec3::new(0.0, 6.0, 0.0), Quat::IDENTITY, Vec3::ONE);
        let snap_src = BlendSource::Snapshot(&snapshot);
        let clip_src = BlendSource::Clip {
            clip: &clip,
            time: 0.0,
            loop_policy: Loop::Wrap,
        };

        let mut captured = Vec::new();
        capture_blend(&snap_src, &clip_src, 0.5, &skeleton, &mut captured);
        // Component lerp of the two translations at 0.5.
        assert_vec3_eq(
            captured[0].translation,
            Vec3::new(1.0, 3.0, 0.0),
            "snapshot×clip translation lerp",
        );
    }

    /// A snapshot source shorter than the skeleton falls back to rest for the
    /// joints past its end (mirroring a short clip) — no panic, no garbage.
    #[test]
    fn snapshot_shorter_than_skeleton_holds_rest() {
        // Two joints; rest scale 0.5 on the second so a rest fallback is visible.
        let rest_child = RestLocal {
            translation: Vec3::new(0.0, 7.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(0.5),
        };
        let skeleton = Skeleton {
            joints: vec![
                joint(None, Mat4::IDENTITY, RestLocal::default()),
                joint(Some(0), Mat4::IDENTITY, rest_child),
            ],
        };
        // Snapshot covers only joint 0.
        let snapshot = vec![LocalTrs {
            translation: Vec3::new(3.0, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }];
        let snap_src = BlendSource::Snapshot(&snapshot);

        let mut captured = Vec::new();
        // Blend snapshot against itself at weight 0 so the output equals the
        // snapshot's resolved locals (rest fallback included for joint 1).
        capture_blend(&snap_src, &snap_src, 0.0, &skeleton, &mut captured);
        assert_eq!(captured.len(), 2);
        assert_vec3_eq(
            captured[0].translation,
            Vec3::new(3.0, 0.0, 0.0),
            "joint 0 from snapshot",
        );
        assert_vec3_eq(
            captured[1].translation,
            rest_child.translation,
            "joint 1 holds rest translation",
        );
        assert_vec3_eq(
            captured[1].scale,
            rest_child.scale,
            "joint 1 holds rest scale",
        );
    }

    /// A two-joint skeleton with NON-IDENTITY inverse-bind matrices on both
    /// joints, so `worldPose * inverseBind` is a meaningful transform — an
    /// identity inverse-bind would let the world/palette comparison pass even if
    /// the factored core were wrong.
    fn two_joint_skeleton_nonidentity_ib() -> (Skeleton, Mat4, Mat4) {
        let root_ib = Mat4::from_translation(Vec3::new(-2.0, 1.0, 0.0));
        let child_ib = Mat4::from_scale_rotation_translation(
            Vec3::new(2.0, 2.0, 2.0),
            Quat::from_rotation_x(0.7),
            Vec3::new(0.0, -5.0, 3.0),
        );
        let skeleton = Skeleton {
            joints: vec![
                joint(
                    None,
                    root_ib,
                    RestLocal {
                        translation: Vec3::new(2.0, 0.0, 0.0),
                        ..Default::default()
                    },
                ),
                joint(
                    Some(0),
                    child_ib,
                    RestLocal {
                        translation: Vec3::new(0.0, 3.0, 0.0),
                        ..Default::default()
                    },
                ),
            ],
        };
        (skeleton, root_ib, child_ib)
    }

    /// The world-joint sampler's output, multiplied per joint by that joint's
    /// inverse-bind matrix, equals the skinning palette for the SAME single-clip
    /// inputs (with a loop policy). Non-identity inverse-binds make the per-joint
    /// multiply load-bearing, so this proves the shared forward-sweep core
    /// produces the world pose the palette path applies inverse-bind to.
    #[test]
    fn world_clip_sampler_times_inverse_bind_equals_palette() {
        let (skeleton, root_ib, child_ib) = two_joint_skeleton_nonidentity_ib();
        // Animate the child translation so the pose is non-trivial at the sampled
        // time, and use Clamp past the end so the loop policy is exercised too.
        let child_tracks = JointTracks {
            translation: Track {
                times: vec![0.0, 2.0],
                values: vec![Vec3::new(0.0, 3.0, 0.0), Vec3::new(4.0, 3.0, -1.0)],
                ..Default::default()
            },
            ..Default::default()
        };
        let clip = translation_clip("walk", 2.0, vec![JointTracks::default(), child_tracks]);

        let ibs = [root_ib, child_ib];
        for (time, policy) in [(0.5, Loop::Wrap), (3.0, Loop::Clamp), (2.1, Loop::Wrap)] {
            let mut palette = Vec::new();
            sample_clip_looped(&clip, &skeleton, time, policy, &mut palette);
            let mut world = Vec::new();
            sample_clip_looped_world(&clip, &skeleton, time, policy, &mut world);

            assert_eq!(world.len(), skeleton.joints.len());
            for (j, ib) in ibs.iter().enumerate() {
                let recovered = world[j] * *ib;
                assert_mat4_eq(
                    recovered,
                    Mat4::from_cols_array_2d(&palette[j].matrix),
                    &format!(
                        "joint {j} worldPose*inverseBind == palette (time={time}, {policy:?})"
                    ),
                );
            }
        }
    }

    /// Same equivalence for a two-source blend at a weight: the world-joint
    /// blend sampler's output, multiplied per joint by the inverse-bind matrix,
    /// equals the blended skinning palette. Non-identity inverse-binds again make
    /// the comparison meaningful.
    #[test]
    fn world_blend_sampler_times_inverse_bind_equals_palette() {
        let (skeleton, root_ib, child_ib) = two_joint_skeleton_nonidentity_ib();
        let clip_a = constant_pose_clip(
            "a",
            Vec3::new(1.0, 2.0, 0.0),
            Quat::from_rotation_z(0.4),
            Vec3::splat(1.0),
        );
        let clip_b = constant_pose_clip(
            "b",
            Vec3::new(-3.0, 0.0, 2.0),
            Quat::from_rotation_y(1.2),
            Vec3::splat(1.5),
        );
        let src_a = BlendSource::Clip {
            clip: &clip_a,
            time: 0.0,
            loop_policy: Loop::Wrap,
        };
        let src_b = BlendSource::Clip {
            clip: &clip_b,
            time: 0.0,
            loop_policy: Loop::Wrap,
        };

        let ibs = [root_ib, child_ib];
        for weight in [0.0, 0.35, 1.0] {
            let mut palette = Vec::new();
            sample_blended(&src_a, &src_b, weight, &skeleton, &mut palette);
            let mut world = Vec::new();
            sample_blended_world(&src_a, &src_b, weight, &skeleton, &mut world);

            assert_eq!(world.len(), skeleton.joints.len());
            for (j, ib) in ibs.iter().enumerate() {
                let recovered = world[j] * *ib;
                assert_mat4_eq(
                    recovered,
                    Mat4::from_cols_array_2d(&palette[j].matrix),
                    &format!(
                        "joint {j} blended worldPose*inverseBind == palette (weight={weight})"
                    ),
                );
            }
        }
    }

    /// The world-joint samplers honor the caller-reused-buffer allocation
    /// contract: `out` is cleared/resized to the joint count, and a warmed
    /// steady-state call neither reallocates `out` nor changes the result.
    #[test]
    fn world_samplers_reuse_out_buffer_steady_state() {
        let (skeleton, _, _) = two_joint_skeleton_nonidentity_ib();
        let clip = translation_clip(
            "rest",
            1.0,
            vec![JointTracks::default(), JointTracks::default()],
        );

        // Stale, oversized buffer must be cleared and resized to the joint count.
        let mut out = vec![Mat4::from_scale(Vec3::splat(9.0)); 5];
        sample_clip_looped_world(&clip, &skeleton, 0.0, Loop::Wrap, &mut out);
        assert_eq!(
            out.len(),
            skeleton.joints.len(),
            "out resized to joint count"
        );

        // Warm so capacity is sized, then assert steady-state reuse allocates
        // nothing and stays deterministic.
        let cap = out.capacity();
        let first = out.clone();
        for _ in 0..16 {
            sample_clip_looped_world(&clip, &skeleton, 0.0, Loop::Wrap, &mut out);
        }
        assert_eq!(
            out.capacity(),
            cap,
            "world clip sampler does not reallocate out"
        );
        assert_eq!(out, first, "world clip sampler reuse is deterministic");
    }

    /// Steady-state blended sampling reuses both thread-locals and the caller's
    /// `out`, so a warmed call allocates nothing. Probed by capacity stability:
    /// after a warm-up the buffers are sized, and a subsequent call neither grows
    /// `out`'s capacity nor changes the result.
    #[test]
    fn blended_sampling_reuses_scratch_steady_state() {
        let skeleton = single_root_skeleton();
        let clip_a = constant_pose_clip("a", Vec3::new(1.0, 0.0, 0.0), Quat::IDENTITY, Vec3::ONE);
        let clip_b = constant_pose_clip("b", Vec3::new(0.0, 1.0, 0.0), Quat::IDENTITY, Vec3::ONE);
        let src_a = BlendSource::Clip {
            clip: &clip_a,
            time: 0.0,
            loop_policy: Loop::Wrap,
        };
        let src_b = BlendSource::Clip {
            clip: &clip_b,
            time: 0.0,
            loop_policy: Loop::Wrap,
        };

        let mut out = Vec::new();
        // Warm-up: grows `out` and the thread-locals to skeleton size once.
        sample_blended(&src_a, &src_b, 0.5, &skeleton, &mut out);
        let cap_after_warm = out.capacity();
        let first = out[0];

        // Steady state: the reused `out` must not reallocate.
        for _ in 0..16 {
            sample_blended(&src_a, &src_b, 0.5, &skeleton, &mut out);
        }
        assert_eq!(
            out.capacity(),
            cap_after_warm,
            "out not reallocated in steady state"
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], first, "blended reuse is deterministic");
    }
}
