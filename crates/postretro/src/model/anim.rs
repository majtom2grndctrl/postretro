// Animation sampling: clip + time + skeleton → world bone palette (CPU math).
// See: context/lib/rendering_pipeline.md §5
//
// CPU-only (no wgpu): glam math here, palette UPLOAD lives in the render pass.
// Single clip, LINEAR interpolation, no blend, no state machine — the slice's
// scope. Reuse-friendly: `sample_clip` writes into a caller-owned `Vec` and
// keeps a thread-local scratch for the world-pose sweep, so steady-state frames
// allocate nothing (Task 6 measures per-frame pose-sampling cost).

use std::cell::RefCell;

use glam::{Mat4, Quat, Vec3};

use super::BonePaletteEntry;
use super::skeleton::{AnimationClip, JointTracks, RestLocal, Skeleton, Track};

thread_local! {
    /// Reusable world-pose scratch (one `Mat4` per joint) for the forward sweep.
    /// Cleared and refilled per call; grows to the largest skeleton seen and is
    /// reused thereafter so steady-state sampling does not allocate.
    static WORLD_POSE_SCRATCH: RefCell<Vec<Mat4>> = const { RefCell::new(Vec::new()) };
}

/// Sample `clip` at `time` (seconds) against `skeleton`, writing one
/// [`BonePaletteEntry`] per joint (in skeleton/topo order) into `out`.
///
/// Each output entry is the joint's **skinning matrix**: the composed world
/// joint transform multiplied by the joint's inverse-bind matrix, ready to
/// upload as one contiguous palette run. `out` is cleared then filled, so its
/// final length equals `skeleton.joints.len()`.
///
/// Per channel: LINEAR interpolation (component lerp for translation/scale,
/// shortest-path slerp for rotation). A channel with **no keyframes** holds the
/// joint's rest-pose component (NOT identity) — the shipped clip omits scale, so
/// scale falls back to `Joint::rest_local.scale`. `time` is wrapped into
/// `[0, duration)` so the clip loops; a non-positive duration samples at `t = 0`.
///
/// Reuse: pass the same `out` every frame. A thread-local scratch holds the
/// world-pose sweep, so a steady-state call performs no heap allocation.
pub fn sample_clip(
    clip: &AnimationClip,
    skeleton: &Skeleton,
    time: f32,
    out: &mut Vec<BonePaletteEntry>,
) {
    let joint_count = skeleton.joints.len();
    out.clear();
    out.reserve(joint_count);

    // Loop the clip: sample at `time mod duration`. Guard a zero/negative
    // duration (a static or malformed clip) by sampling the first frame.
    let t = if clip.duration > 0.0 {
        time.rem_euclid(clip.duration)
    } else {
        0.0
    };

    WORLD_POSE_SCRATCH.with(|cell| {
        let mut world = cell.borrow_mut();
        world.clear();
        world.reserve(joint_count);

        for (i, joint) in skeleton.joints.iter().enumerate() {
            // The clip's per-joint tracks are parallel to skeleton joints, but a
            // static-model / mismatched clip may be shorter — fall back to rest.
            let tracks = clip.joints.get(i);
            let local = sample_local_pose(tracks, &joint.rest_local, t);

            // Forward sweep: parent-before-child topo order guarantees the
            // parent's world matrix is already in `world` when we reach a child.
            let world_pose = match joint.parent {
                Some(p) => world[p] * local,
                None => local,
            };
            world.push(world_pose);

            let inverse_bind = Mat4::from_cols_array_2d(&joint.inverse_bind);
            let skinning = world_pose * inverse_bind;
            out.push(BonePaletteEntry {
                matrix: skinning.to_cols_array_2d(),
            });
        }
    });
}

/// Resolve one joint's local TRS at time `t`: each channel interpolates its
/// keyframes if present, else holds the rest-pose component. Composed to a
/// `Mat4` in TRS order (`translation * rotation * scale`), matching glTF's node
/// transform convention.
fn sample_local_pose(tracks: Option<&JointTracks>, rest: &RestLocal, t: f32) -> Mat4 {
    let (translation, rotation, scale) = match tracks {
        Some(tr) => (
            sample_vec3_track(&tr.translation, t).unwrap_or(rest.translation),
            sample_quat_track(&tr.rotation, t).unwrap_or(rest.rotation),
            sample_vec3_track(&tr.scale, t).unwrap_or(rest.scale),
        ),
        None => (rest.translation, rest.rotation, rest.scale),
    };
    Mat4::from_scale_rotation_translation(scale, rotation, translation)
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

/// Sample a `Vec3` track (translation/scale) with component-wise LINEAR lerp.
fn sample_vec3_track(track: &Track<Vec3>, t: f32) -> Option<Vec3> {
    let (i0, i1, frac) = locate_span(&track.times, t)?;
    let a = track.values[i0];
    let b = track.values[i1];
    Some(a.lerp(b, frac))
}

/// Sample a `Quat` rotation track with shortest-path slerp. Endpoints are
/// normalized (authored quats may drift) and slerp handles the dot-sign flip
/// internally, so the interpolation never takes the long way around.
fn sample_quat_track(track: &Track<Quat>, t: f32) -> Option<Quat> {
    let (i0, i1, frac) = locate_span(&track.times, t)?;
    let a = track.values[i0].normalize();
    if i0 == i1 {
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
        let mut tracks = JointTracks::default();
        tracks.translation = Track {
            times: vec![0.0, 2.0],
            values: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, -4.0, 2.0)],
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
        let mut tracks = JointTracks::default();
        tracks.rotation = Track {
            times: vec![0.0, 1.0],
            values: vec![q0, q1],
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
        let mut tracks = JointTracks::default();
        tracks.translation = Track {
            times: vec![0.0, 1.0],
            values: vec![Vec3::new(1.0, 2.0, 3.0), Vec3::new(1.0, 2.0, 3.0)],
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
        let mut tracks = JointTracks::default();
        tracks.translation = Track {
            times: vec![0.0, 2.0],
            values: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(8.0, 0.0, 0.0)],
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
}
