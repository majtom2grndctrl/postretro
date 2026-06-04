// Skeleton + animation-clip CPU types for skinned models.
// See: context/lib/rendering_pipeline.md §5

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

/// A single named animation clip.
///
/// **Stub.** Task 2 (load) fills the real keyframe storage — per-channel
/// translation / rotation / scale tracks keyed by joint index — and Task 4
/// (sample) consumes it. The `name` and `duration` fields are stable now so the
/// loader and sampler can be wired against them without restructuring; the
/// keyframe storage will be added as additional fields.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimationClip {
    /// Clip name as authored (e.g. the glTF animation name "mixamo.com").
    pub name: String,
    /// Total clip length in seconds. `0.0` until Task 2 populates keyframes.
    pub duration: f32,
}
