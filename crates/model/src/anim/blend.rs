// Two-source local-pose blending and snapshot capture.
// See: context/lib/rendering_pipeline.md §9

use crate::skeleton::{RestLocal, Skeleton};

use super::track::sample_local_trs;
use super::types::{BlendSource, LocalTrs, resolve_time};

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
pub(super) fn resolve_blend_into(
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
