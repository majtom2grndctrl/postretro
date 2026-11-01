//! The shared model cache's key, derived on the load pool: blake3 over the
//! content of every file a model's parse and upload read, plus every
//! install-time parameter that shapes the upload, in a versioned key.
//! See: context/plans/in-progress/level-load-pipeline/index.md (Model cache) · context/lib/resource_management.md §7

use postretro_model::gltf_loader::ModelSources;
use postretro_render_cpu::loaded_texture::model_materials_consume_specular;
use postretro_render_cpu::surface_depth::SurfaceDepthQuality;
use postretro_render_cpu::texture_prep::ModelSidecars;

use crate::render::ModelContentKey;

/// The key format's version, hashed into every key and stamped on it. Bump
/// it whenever what the key covers, or how the parse and upload turn those
/// inputs into a model (the glTF loader, the material plan, the uniform
/// packing, `Material::Default`), changes: every older entry then misses.
pub(crate) const MODEL_CONTENT_KEY_VERSION: u32 = 1;

/// Every install-time parameter that shapes a model upload beyond its files.
///
/// Today none does, and this says so explicitly rather than by omission:
/// models render at their authored scale with no import scale or pivot
/// (`resource_management.md` §7); model materials bind `Material::Default`,
/// a constant the key version covers; and their uniforms pack flat at every
/// Surface Depth tier, because the model slot policy never consumes the
/// specular slot the two-channel surface map rides, so the bound specular is
/// the single-channel placeholder. The Surface Depth tier enters the key only
/// while that policy consumes specular ([`Self::at_tier`]).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ModelInstallParams {
    /// The Surface Depth tier, when it shapes model materials; `None` while
    /// it cannot.
    surface_depth: Option<SurfaceDepthQuality>,
    /// The Surface Depth tier a model built under these parameters binds its
    /// materials at: the tier they were derived from. Not hashed: it shapes
    /// the upload only through `surface_depth`, which carries it whenever
    /// it does.
    build_tier: SurfaceDepthQuality,
}

/// Equal when the key is: `build_tier` is not hashed, so two parameter sets
/// that differ only there name the same cache entry.
impl PartialEq for ModelInstallParams {
    fn eq(&self, other: &Self) -> bool {
        self.surface_depth == other.surface_depth
    }
}

impl Eq for ModelInstallParams {}

impl ModelInstallParams {
    /// The parameters a load dispatched at the `surface_depth` tier builds
    /// its models at.
    pub(crate) fn at_tier(surface_depth: SurfaceDepthQuality) -> Self {
        Self {
            surface_depth: model_materials_consume_specular().then_some(surface_depth),
            build_tier: surface_depth,
        }
    }

    /// The tier a model built under these parameters binds its materials
    /// at, so a build and the key it is entered under agree.
    pub(crate) fn build_tier(self) -> SurfaceDepthQuality {
        self.build_tier
    }

    /// Parameters with an explicit Surface Depth input, whatever the slot
    /// policy says: the key's tests show a parameter change misses.
    #[cfg(test)]
    pub(crate) fn with_surface_depth(surface_depth: Option<SurfaceDepthQuality>) -> Self {
        Self {
            surface_depth,
            build_tier: surface_depth.unwrap_or_default(),
        }
    }

    fn hash_into(self, hasher: &mut blake3::Hasher) {
        let surface_depth: u8 = match self.surface_depth {
            None => 0,
            Some(SurfaceDepthQuality::Off) => 1,
            Some(SurfaceDepthQuality::On) => 2,
        };
        hasher.update(b"params");
        hasher.update(&[surface_depth]);
    }
}

/// One model's cache key: [`MODEL_CONTENT_KEY_VERSION`] and a blake3 digest
/// over
///
/// - the glTF document's bytes;
/// - every buffer it references, external `.bin` files and `data:` URIs
///   alike, in buffer-index order, each length-prefixed;
/// - each primitive's material key of the mesh the loader selects, in
///   primitive order: blake3 of that material's base-color PNG, which names
///   the sidecar the material reads (the PNG shapes nothing else);
/// - each distinct material sidecar the upload reads (`<key>.prm`): its key,
///   whether it could be read, and its bytes;
/// - `params`.
///
/// Exactly what the parse and the material prep read, and nothing else: no
/// path enters it. The loader resolves every reference relative to the
/// document and the key covers what each resolves to, so identical content
/// under another handle or directory is the same model. Reads no file: the
/// caller read `sources` and `sidecars` once, and a miss parses and prepares
/// from those same bytes.
pub(crate) fn model_content_key(
    sources: &ModelSources,
    sidecars: &ModelSidecars,
    params: ModelInstallParams,
) -> ModelContentKey {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"postretro-model-content-key");
    hasher.update(&MODEL_CONTENT_KEY_VERSION.to_le_bytes());
    hasher.update(b"document");
    hasher.update(&sources.document_digest());
    let buffers: Vec<&[u8]> = sources.buffers().collect();
    hasher.update(b"buffers");
    hasher.update(&(buffers.len() as u64).to_le_bytes());
    for buffer in buffers {
        hasher.update(&(buffer.len() as u64).to_le_bytes());
        hasher.update(buffer);
    }
    let material_keys = sources.material_keys();
    hasher.update(b"materials");
    hasher.update(&(material_keys.len() as u64).to_le_bytes());
    for key in material_keys {
        hasher.update(&(key.len() as u64).to_le_bytes());
        hasher.update(key.as_bytes());
    }
    hasher.update(b"sidecars");
    for (key, bytes) in sidecars.contents() {
        hasher.update(&key);
        match bytes {
            Some(bytes) => {
                hasher.update(&[1]);
                hasher.update(&(bytes.len() as u64).to_le_bytes());
                hasher.update(bytes);
            }
            None => {
                hasher.update(&[0]);
            }
        }
    }
    params.hash_into(&mut hasher);
    ModelContentKey::new(MODEL_CONTENT_KEY_VERSION, *hasher.finalize().as_bytes())
}
