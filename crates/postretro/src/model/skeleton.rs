// Skeleton + animation-clip CPU types for skinned models.
// See: context/lib/rendering_pipeline.md §5

use glam::{Quat, Vec3};

/// One joint in a skeleton. `parent` is `None` for the root; all other joints
/// reference their parent by index into [`Skeleton::joints`]. `inverse_bind`
/// transforms a vertex from model space into this joint's local bind space —
/// the standard glTF inverse-bind matrix.
///
/// Populated by the glTF loader (Task 2). The animation sampler (Task 4) walks
/// these parent links to compose world-space joint matrices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Joint {
    /// Parent joint index, or `None` for the root. An index keeps the hierarchy
    /// flat and cache-friendly; a topological (parent-before-child) ordering is
    /// expected so a single forward pass can compose world matrices.
    pub parent: Option<usize>,
    /// glTF inverse-bind matrix, column-major to match glam's `Mat4`.
    pub inverse_bind: [[f32; 4]; 4],
}

/// A skeleton: the ordered joint hierarchy a skinned mesh binds against.
/// Joints are stored parent-before-child so animation sampling is a single
/// forward sweep. Filled by Task 2 (load).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Skeleton {
    pub joints: Vec<Joint>,
}

/// One keyframe track: time samples (seconds, ascending) paired with values.
///
/// The two `Vec`s are parallel — `times[i]` is the sample time of `values[i]` —
/// and always the same length. Empty = the channel is not animated for this
/// joint (the sampler falls back to the joint's bind-pose component).
///
/// glTF mixamo clips author LINEAR interpolation for translation and rotation
/// and omit scale; the loader stores the raw samples and Task 4 interpolates
/// (lerp for translation/scale, nlerp/slerp for rotation). Interpolation mode is
/// not stored — the slice samples LINEAR; a STEP/CUBICSPLINE-carrying clip is the
/// broadening task's concern and would add a mode tag here.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track<T> {
    pub times: Vec<f32>,
    pub values: Vec<T>,
}

impl<T> Track<T> {
    /// True when no samples were authored for this channel.
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
/// entry per skeleton joint and in the same order as [`Skeleton::joints`]. Task 4
/// samples a joint's tracks at a time `t` to recover its local TRS, composes the
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
