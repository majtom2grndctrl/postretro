// Per-joint animation-track interpolation and local-pose sampling.
// See: context/lib/rendering_pipeline.md §9

use glam::{Mat4, Quat, Vec3};

use crate::skeleton::{Interp, JointTracks, RestLocal, Track};

use super::types::LocalTrs;

/// Resolve one joint's local TRS at time `t`: each channel interpolates its
/// keyframes if present, else holds the rest-pose component. Returns the raw TRS
/// so the blend path can blend two of them as quaternions before composing; the
/// single-clip path composes one to a `Mat4` via [`sample_local_pose`].
pub(super) fn sample_local_trs(tracks: Option<&JointTracks>, rest: &RestLocal, t: f32) -> LocalTrs {
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
pub(super) fn sample_local_pose(tracks: Option<&JointTracks>, rest: &RestLocal, t: f32) -> Mat4 {
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
    let values = track.values();
    let (i0, i1, frac) = locate_span(track.times(), t)?;
    let a = values[i0];
    match track.mode() {
        Interp::Step => Some(a),
        Interp::Linear => {
            let b = values[i1];
            Some(a.lerp(b, frac))
        }
    }
}

/// Sample a `Quat` rotation track. `Linear` slerps (shortest-path) between the
/// bracketing keys — endpoints are normalized (authored quats may drift) and
/// glam's `slerp` handles the dot-sign flip internally, so the interpolation
/// never takes the long way around. `Step` holds the lower key (`i0`).
fn sample_quat_track(track: &Track<Quat>, t: f32) -> Option<Quat> {
    let values = track.values();
    let (i0, i1, frac) = locate_span(track.times(), t)?;
    let a = values[i0].normalize();
    if i0 == i1 || track.mode() == Interp::Step {
        return Some(a);
    }
    let b = values[i1].normalize();
    // glam's `slerp` already picks the shortest arc (it negates `b` when the dot
    // is negative), so we get the correct hemisphere without a manual flip.
    Some(a.slerp(b, frac).normalize())
}
