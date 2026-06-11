// Skeleton + animation-clip CPU types for skinned models.
// See: context/lib/rendering_pipeline.md §9

use glam::{Quat, Vec3};

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
/// Populated by `crate::model::gltf_loader`. The animation sampler in
/// `crate::model::anim` walks these parent links to compose world-space joint matrices.
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
/// Joints are stored parent-before-child so animation sampling is a single
/// forward sweep. Populated by `gltf_loader::load_model`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Skeleton {
    pub joints: Vec<Joint>,
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
/// scope (see the M10 plan), so no runtime code ever sees a cubic mode here.
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
/// `mode` records how `crate::model::anim::sample_clip` blends between keys:
/// `Linear` interpolates (lerp for translation/scale, normalized slerp for
/// rotation); `Step` holds the lower bracketing key. The loader maps the glTF
/// sampler's interpolation here; CUBICSPLINE channels are degraded to `Linear`
/// with their value elements (tangents discarded) at load time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track<T> {
    pub times: Vec<f32>,
    pub values: Vec<T>,
    /// How to interpolate between keyframes. Defaults to [`Interp::Linear`] so a
    /// `Track::default()` (and the field-elided synthetic-test construction)
    /// behaves as it did before the mode was tracked.
    pub mode: Interp,
}

impl<T> Track<T> {
    /// True when no samples were authored for this channel.
    // Kept for the sampler-broadening task that handles unanimated-channel fallback.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }
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
/// Keyframe storage is per-joint TRS tracks in [`AnimationClip::joints`], one
/// entry per skeleton joint and in the same order as [`Skeleton::joints`].
/// `model::anim::sample_clip` samples a joint's tracks at a time `t` to recover its local TRS, composes the
/// hierarchy (parent-before-child), and applies each joint's inverse-bind matrix.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimationClip {
    /// Clip name as authored (e.g. the glTF animation name "mixamo.com").
    pub name: String,
    /// Total clip length in seconds — the latest keyframe time across all tracks.
    pub duration: f32,
    /// Per-joint TRS tracks, parallel to [`Skeleton::joints`]. Joints with no
    /// channel in the clip carry empty tracks (held at bind pose by the sampler).
    pub joints: Vec<JointTracks>,
}
