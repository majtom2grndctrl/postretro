// Skeleton + animation-clip CPU types for skinned models.
// See: context/lib/rendering_pipeline.md §9

use glam::{Quat, Vec3};
use thiserror::Error;

/// A joint's rest-pose local transform: the glTF node's default TRS, captured at
/// load time. The animation sampler holds this for any channel a clip omits —
/// the shipped clip carries rotation + translation only (no scale), and some
/// joints have no channel at all, so a missing channel MUST fall back to the
/// rest value, NOT to identity (identity would collapse joint scale/offset).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RestLocal {
    /// Rest-pose local translation (parent-relative).
    pub translation: Vec3,
    /// Rest-pose local rotation (parent-relative unit quaternion).
    pub rotation: Quat,
    /// Rest-pose local scale.
    pub scale: Vec3,
}

impl Default for RestLocal {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}

/// One joint in a skeleton. `parent` is `None` for the root; all other joints
/// reference their parent by index into [`Skeleton::joints`]. `inverse_bind`
/// transforms a vertex from model space into this joint's local bind space —
/// the standard glTF inverse-bind matrix.
///
/// Populated by `crate::gltf_loader`. The animation sampler in
/// `crate::anim` walks these parent links to compose world-space joint matrices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Joint {
    /// Parent joint index, or `None` for the root. An index keeps the hierarchy
    /// flat and cache-friendly; a topological (parent-before-child) ordering is
    /// expected so a single forward pass can compose world matrices.
    pub parent: Option<usize>,
    /// glTF inverse-bind matrix, column-major to match glam's `Mat4`.
    pub inverse_bind: [[f32; 4]; 4],
    /// The joint node's default local TRS, captured from the glTF node. The
    /// sampler uses each component as the rest-pose fallback for an absent
    /// channel (e.g. the shipped clip omits scale, so scale holds `rest_local`).
    pub rest_local: RestLocal,
}

/// A skeleton: the ordered joint hierarchy a skinned mesh binds against.
/// Loader-produced skeletons store joints parent-before-child so animation
/// sampling is a single forward sweep. Direct public field construction is still
/// possible for tests and callers; prefer [`Skeleton::new`] when accepting
/// external joint arrays.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Skeleton {
    pub joints: Vec<Joint>,
}

/// Why a [`Skeleton`] could not be built from a public joint array.
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum SkeletonBuildError {
    /// A joint's parent index does not refer to an earlier joint.
    #[error("joint {joint} references parent {parent}, which does not precede it")]
    ParentNotBeforeChild { joint: usize, parent: usize },
}

impl Skeleton {
    /// Build a skeleton after validating the parent-before-child contract the
    /// fast sampler path is designed around.
    pub fn new(joints: Vec<Joint>) -> Result<Self, SkeletonBuildError> {
        validate_parent_before_child(&joints)?;
        Ok(Self { joints })
    }
}

fn validate_parent_before_child(joints: &[Joint]) -> Result<(), SkeletonBuildError> {
    for (joint, data) in joints.iter().enumerate() {
        if let Some(parent) = data.parent {
            if parent >= joint {
                return Err(SkeletonBuildError::ParentNotBeforeChild { joint, parent });
            }
        }
    }
    Ok(())
}

/// Keyframe interpolation mode for a [`Track`], mapped from the glTF sampler's
/// interpolation algorithm.
///
/// glTF defines three modes; the engine evaluates two:
/// - `Linear` — lerp (translation/scale) / slerp (rotation) between bracketing keys.
/// - `Step` — hold the lower bracketing key's value (no fraction) until the next key.
///
/// glTF's third mode, CUBICSPLINE, is **not** a `Track` variant: the loader
/// extracts each keyframe's value element (discarding tangents) and stores the
/// track as `Linear`, degrading cubic to linear. True cubic evaluation is out of
/// scope (tangent storage and hermite blending are not implemented), so no runtime code ever sees a cubic mode here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Interp {
    /// Linearly interpolate between bracketing keyframes (lerp / slerp).
    #[default]
    Linear,
    /// Hold the lower bracketing keyframe's value until the next keyframe.
    Step,
}

/// One keyframe track: time samples (seconds, ascending) paired with values.
///
/// The two `Vec`s are parallel — `times[i]` is the sample time of `values[i]` —
/// and always the same length. Empty = the channel is not animated for this
/// joint (the sampler falls back to the joint's bind-pose component).
///
/// `mode` records how `crate::anim::sample_clip` blends between keys:
/// `Linear` interpolates (lerp for translation/scale, normalized slerp for
/// rotation); `Step` holds the lower bracketing key. The loader maps the glTF
/// sampler's interpolation here; CUBICSPLINE channels are degraded to `Linear`
/// with their value elements (tangents discarded) at load time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track<T> {
    pub(crate) times: Vec<f32>,
    pub(crate) values: Vec<T>,
    /// How to interpolate between keyframes. [`Interp::Linear`] is the meaningful
    /// default: crate-local field construction and `Track::default()` both
    /// interpolate rather than step, which is the common case for authored
    /// animation clips.
    pub(crate) mode: Interp,
}

/// Why a [`Track`] could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackBuildError {
    /// `times` and `values` were not parallel arrays.
    LengthMismatch,
    /// A key time was NaN or infinite.
    NonFiniteTime,
    /// Key times were not strictly ascending.
    NonAscendingTime,
}

impl<T> Track<T> {
    /// Build a keyframe track after validating the public invariants the sampler
    /// relies on: parallel arrays, finite times, and strictly ascending keys.
    pub fn new(times: Vec<f32>, values: Vec<T>, mode: Interp) -> Result<Self, TrackBuildError> {
        validate_track_times(&times, values.len())?;
        Ok(Self {
            times,
            values,
            mode,
        })
    }

    /// Key times in seconds. Parallel to [`Track::values`].
    pub fn times(&self) -> &[f32] {
        &self.times
    }

    /// Keyframe values. Parallel to [`Track::times`].
    pub fn values(&self) -> &[T] {
        &self.values
    }

    /// The interpolation mode used between keyframes.
    pub fn mode(&self) -> Interp {
        self.mode
    }

    /// True when the track carries no keyframes.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }
}

fn validate_track_times(times: &[f32], value_count: usize) -> Result<(), TrackBuildError> {
    if times.len() != value_count {
        return Err(TrackBuildError::LengthMismatch);
    }
    for &time in times {
        if !time.is_finite() {
            return Err(TrackBuildError::NonFiniteTime);
        }
    }
    for pair in times.windows(2) {
        if pair[0] >= pair[1] {
            return Err(TrackBuildError::NonAscendingTime);
        }
    }
    Ok(())
}

/// Per-joint animation tracks, indexed in lockstep with [`Skeleton::joints`]
/// (entry `i` animates joint `i`). A joint with no channel in the clip has all
/// three tracks empty and holds its bind pose.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JointTracks {
    /// Local-space translation keyframes (parent-relative), if animated.
    pub translation: Track<Vec3>,
    /// Local-space rotation keyframes (parent-relative unit quaternion), if
    /// animated.
    pub rotation: Track<Quat>,
    /// Local-space scale keyframes, if animated.
    pub scale: Track<Vec3>,
}

/// A single named animation clip.
///
/// Loader-produced clips store one [`AnimationClip::joints`] entry per skeleton
/// joint in the same order as [`Skeleton::joints`]. The sampler also accepts
/// shorter or mismatched public clips: a missing joint-track entry holds that
/// joint's rest pose.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimationClip {
    /// Clip name as authored (e.g. the glTF animation name "mixamo.com").
    pub name: String,
    /// Total clip length in seconds — the latest valid input time across all
    /// channels, including channels that are subsequently skipped for malformed
    /// output values or unsupported interpolation details.
    pub duration: f32,
    /// Per-joint TRS tracks. Loader output is parallel to [`Skeleton::joints`].
    /// Public clips may be shorter; missing entries hold rest pose.
    pub joints: Vec<JointTracks>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_constructor_rejects_non_parallel_arrays() {
        assert_eq!(
            Track::new(vec![0.0, 1.0], vec![Vec3::ZERO], Interp::Linear).unwrap_err(),
            TrackBuildError::LengthMismatch
        );
    }

    #[test]
    fn track_constructor_rejects_non_finite_times() {
        assert_eq!(
            Track::new(
                vec![0.0, f32::NAN],
                vec![Vec3::ZERO, Vec3::ONE],
                Interp::Linear,
            )
            .unwrap_err(),
            TrackBuildError::NonFiniteTime
        );
    }

    #[test]
    fn track_constructor_rejects_duplicate_or_descending_times() {
        assert_eq!(
            Track::new(vec![0.0, 0.0], vec![Vec3::ZERO, Vec3::ONE], Interp::Linear,).unwrap_err(),
            TrackBuildError::NonAscendingTime
        );
        assert_eq!(
            Track::new(vec![1.0, 0.0], vec![Vec3::ZERO, Vec3::ONE], Interp::Linear,).unwrap_err(),
            TrackBuildError::NonAscendingTime
        );
    }

    #[test]
    fn skeleton_constructor_rejects_non_topological_parent_links() {
        let child_first = vec![
            Joint {
                parent: Some(1),
                inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
                rest_local: RestLocal::default(),
            },
            Joint {
                parent: None,
                inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
                rest_local: RestLocal::default(),
            },
        ];
        assert_eq!(
            Skeleton::new(child_first).unwrap_err(),
            SkeletonBuildError::ParentNotBeforeChild {
                joint: 0,
                parent: 1,
            },
        );
    }
}
