// Animation sampling: clip + time + skeleton → world bone palette. Task 4 fills the body.
// See: context/lib/rendering_pipeline.md §5

use super::BonePaletteEntry;
use super::skeleton::{AnimationClip, Skeleton};

/// Sample `clip` at `time` (seconds) against `skeleton`, producing one
/// [`BonePaletteEntry`] per joint in skeleton order.
///
/// Each output entry is the joint's **skinning matrix**: world joint transform
/// composed with the joint's inverse-bind matrix, ready to upload as one
/// contiguous palette run for the instance. The output length equals
/// `skeleton.joints.len()`.
///
/// **Stub.** Task 4 fills the body: locate the surrounding keyframes for `time`,
/// interpolate per-channel TRS, compose down the parent chain (parent-before-
/// child order lets this be a single forward sweep), and multiply by each
/// joint's inverse-bind matrix.
pub fn sample_clip(
    _clip: &AnimationClip,
    _time: f32,
    _skeleton: &Skeleton,
) -> Vec<BonePaletteEntry> {
    todo!("Task 4: sample clip at time and compose world bone palette")
}
