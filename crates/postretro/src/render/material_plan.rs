// Material/sampler helpers, the GPU-free submesh material plan (dedup + draw
// bookkeeping), blake3 key parsing, and the model open-path/cache-key resolver.
// See: context/lib/resource_management.md

use super::*;

/// Highest valid LOD index for a chain of `mip_count` mips. The anisotropic
/// sampler pool clamps `lod_max` to this so no sampler reads past the uploaded chain.
pub(crate) fn mip_lod_max_clamp(mip_count: u32) -> f32 {
    mip_count.saturating_sub(1) as f32
}

/// Create the Post Retro filtering pool's sampler: fully Linear min/mag/mip
/// with `anisotropy_clamp = POST_RETRO_ANISO_CLAMP`, with a per-mip-count LOD
/// clamp. wgpu 29 validates that aniso > 1 requires all three filters to be
/// Linear. One sampler per distinct mip count is kept in
/// `Renderer::mip_count_aniso_samplers` so each material binds the clamp that
/// matches its uploaded mip chain. Bound in every material bind group
/// (binding 5).
pub(crate) fn create_mip_aniso_sampler(device: &wgpu::Device, mip_count: u32) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("Mip Texture Aniso Sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        lod_min_clamp: 0.0,
        lod_max_clamp: mip_lod_max_clamp(mip_count),
        anisotropy_clamp: POST_RETRO_ANISO_CLAMP,
        ..Default::default()
    })
}

pub(crate) fn build_material_bind_group(
    device: &wgpu::Device,
    texture_bind_group_layout: &wgpu::BindGroupLayout,
    loaded: &LoadedTexture,
    aniso_sampler: &wgpu::Sampler,
    material: Material,
    label_prefix: &str,
) -> wgpu::BindGroup {
    let uniform_bytes = build_material_uniform(material.shininess());
    let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&format!("{label_prefix} Uniform")),
        contents: &uniform_bytes,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(&format!("{label_prefix} Bind Group")),
        layout: texture_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&loaded.diffuse_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&loaded.specular_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: uniform_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&loaded.normal_view),
            },
            // Post Retro filtering: the anisotropic sampler paired with
            // in-shader texel-grid reconstruction in forward.wgsl.
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Sampler(aniso_sampler),
            },
        ],
    })
}

// std140: trailing _pad forces size to 32 bytes to match WGSL `MaterialUniform`.
//   0..4  shininess   4..32  pad
pub(crate) const MATERIAL_UNIFORM_SIZE: usize = 32;

pub(crate) fn build_material_uniform(shininess: f32) -> [u8; MATERIAL_UNIFORM_SIZE] {
    let mut bytes = [0u8; MATERIAL_UNIFORM_SIZE];
    bytes[0..4].copy_from_slice(&shininess.to_le_bytes());
    bytes
}

/// Per-submesh draw assignment: the index of the *distinct* material this
/// submesh draws with (into [`SubmeshMaterialPlan::distinct_keys`]) and the
/// `start..end` index range it occupies in the merged buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubmeshDraw {
    /// Index into `distinct_keys` — which deduped material bind group to bind.
    pub(crate) distinct: usize,
    /// `start..end` into the merged index buffer (what `draw_indexed` consumes).
    pub(crate) indices: std::ops::Range<u32>,
}

/// The GPU-free plan for drawing a multi-submesh model: the distinct material
/// keys to build a bind group for (first-seen order, deduped) and the per-submesh
/// assignment of (distinct material, index range), in submesh order.
///
/// First-seen dedup order keeps submesh 0's material at `distinct[0]`, so a
/// single-material model is the trivial special case of the multi-material path
/// (one-submesh ≡ one-distinct ≡ the whole model).
///
/// Factored out of the GPU resolve so the dedup + range bookkeeping is unit
/// testable without a `wgpu::Device`: a model reusing one material across N
/// primitives yields one distinct key and N draws; N distinct materials yield N
/// of each. The GPU layer ([`Renderer::resolve_skinned_model_material`]) builds
/// one bind group per distinct key, then pairs each submesh's range with its
/// (possibly shared) bind group in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubmeshMaterialPlan {
    /// The distinct material keys, in first-seen submesh order. One GPU material
    /// bind group is built per entry.
    pub(crate) distinct_keys: Vec<String>,
    /// One entry per submesh, in submesh order: which distinct key it uses and
    /// the index range it draws.
    pub(crate) draws: Vec<SubmeshDraw>,
}

/// Build the [`SubmeshMaterialPlan`] for a model's submeshes: dedup the material
/// keys (first-seen order) and assign each submesh to its distinct key + range.
/// Pure data logic — no GPU — so the dedup/range bookkeeping is unit-testable.
pub(crate) fn plan_submesh_materials(
    submeshes: &[postretro_model::gltf_loader::Submesh],
) -> SubmeshMaterialPlan {
    let mut distinct_keys: Vec<String> = Vec::new();
    let mut draws: Vec<SubmeshDraw> = Vec::with_capacity(submeshes.len());
    for sub in submeshes {
        let distinct = match distinct_keys.iter().position(|k| k == &sub.material_key) {
            Some(idx) => idx,
            None => {
                distinct_keys.push(sub.material_key.clone());
                distinct_keys.len() - 1
            }
        };
        draws.push(SubmeshDraw {
            distinct,
            indices: sub.indices.clone(),
        });
    }
    SubmeshMaterialPlan {
        distinct_keys,
        draws,
    }
}

/// Parse a 64-char hex blake3 cache key into 32 bytes. Returns the shared
/// all-zero placeholder sentinel on malformed input, so an absent/garbled model
/// material key degrades to a placeholder rather than panicking.
pub(crate) fn parse_blake3_key(hex: &str) -> [u8; 32] {
    let mut key = [0u8; 32];
    if hex.len() != 64 {
        return [0u8; 32];
    }

    for (byte, pair) in key.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
        let [high, low] = pair else {
            return [0u8; 32];
        };
        let (Some(high), Some(low)) = (ascii_hex_nibble(*high), ascii_hex_nibble(*low)) else {
            return [0u8; 32];
        };
        *byte = (high << 4) | low;
    }
    key
}

pub(crate) fn ascii_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Derive the glTF open path and the renderer cache handle for one skinned model
/// from its content-relative handle. These are DELIBERATELY decoupled: the file
/// opens from `content_root.join(model_rel)` (every other asset joins the content
/// root), but the cache key is the VERBATIM `model_rel` string — the
/// `MeshComponent.model` handle the spawn attaches and the per-frame planner
/// groups by, so a joined key would miss `models.get(&group.model)` and silently
/// drop every draw. Split out as a pure helper so the key/path contract is
/// unit-testable without a GPU device (`load_skinned_model` needs one).
pub(crate) fn resolve_model_open_path_and_handle(
    model_rel: &str,
    content_root: &Path,
) -> (std::path::PathBuf, postretro_model::ModelHandle) {
    (
        content_root.join(model_rel),
        postretro_model::ModelHandle::from(model_rel.to_string()),
    )
}
