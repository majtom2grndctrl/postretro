// glTF → engine skinned-model loader. Signature + return shape; Task 2 fills the body.
// See: context/lib/rendering_pipeline.md §5

use std::path::Path;

use thiserror::Error;

use super::mesh::SkinnedMesh;
use super::skeleton::{AnimationClip, Skeleton};

/// A model loaded from glTF: one skinned mesh, its skeleton, its animation
/// clips, and the material cache keys its primitives reference.
///
/// The material is referenced by a **cache key** (the same string-keyed scheme
/// the texture/material cache uses elsewhere) rather than an owned `Material`,
/// so the renderer resolves and de-duplicates textures through the shared cache
/// at upload time. One key per mesh primitive, in primitive order.
///
/// Task 2 (load) populates every field. The shape is final — Task 2 fills in
/// place without restructuring.
#[derive(Debug, Clone, Default)]
pub struct LoadedModel {
    /// The skinned geometry. A model with multiple primitives merges them into
    /// one interleaved stream; `material_keys` carries the per-primitive split.
    pub mesh: SkinnedMesh,
    /// The joint hierarchy the mesh binds against. Empty for a static model
    /// loaded through this path (the rigid single-bone degenerate case).
    pub skeleton: Skeleton,
    /// Animation clips parsed from the glTF. The hardcoded slice ships one clip.
    pub clips: Vec<AnimationClip>,
    /// Material cache keys, one per mesh primitive in primitive order. The
    /// renderer resolves each key against the shared material/texture cache.
    pub material_keys: Vec<String>,
}

/// Errors surfaced while loading a glTF model.
///
/// Task 2 adds the concrete I/O and parse variants (file read, glTF parse,
/// unsupported feature). [`ModelLoadError::NotImplemented`] is the placeholder
/// the stub returns until then.
#[derive(Debug, Error)]
pub enum ModelLoadError {
    #[error("model loading is not yet implemented (Task 2): {0}")]
    NotImplemented(String),
}

/// Load a skinned model from a glTF file at `path`.
///
/// **Stub.** Task 2 fills the body (it adds the `gltf` crate dependency and
/// parses mesh / skeleton / animation / material-key data into [`LoadedModel`]).
/// The signature and return shape are final.
pub fn load_model(path: &Path) -> Result<LoadedModel, ModelLoadError> {
    Err(ModelLoadError::NotImplemented(format!(
        "load_model({})",
        path.display()
    )))
}
