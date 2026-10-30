// Skinned-model tables keyed by `ModelHandle`: insert, lookup and level release.
// See: context/lib/rendering_pipeline.md §9

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use postretro_model::ModelHandle;
use postretro_model::mesh::SkinnedMesh;
use postretro_model::skeleton::{AnimationClip, Skeleton};

use super::{ClipMetadata, MeshPass};

/// One uploaded skinned model: GPU vertex + index buffers, its per-submesh
/// material bind groups, and the skeleton the per-frame palette is sampled
/// against. A single-material model has one submesh spanning the whole index
/// buffer; multi-material models carry one entry per primitive, in submesh order.
///
/// The model's animation clips do NOT live here — they sit in the cache-side
/// `MeshPass::model_clips` map (the `model_bounds` precedent) so the clip-name /
/// metadata query seam is testable without a GPU device. The render path reaches
/// them through that map by the same handle.
pub(in crate::render) struct UploadedModel {
    pub(in crate::render) vertex_buffer: wgpu::Buffer,
    pub(in crate::render) index_buffer: wgpu::Buffer,
    /// Per-submesh material bind group (group 1) + its `start..end` range into
    /// the merged index buffer, in submesh order. Distinct keys are deduped
    /// upstream, so submeshes reusing a material share a (cloned) bind group.
    pub(in crate::render) submeshes: Vec<(wgpu::BindGroup, std::ops::Range<u32>)>,
    /// Skeleton for pose sampling. Joint count == `skeleton.joints.len()` is the
    /// per-instance palette run length.
    pub(in crate::render) skeleton: Skeleton,
    /// CPU-only presentation modifiers resolved beside the skeleton at palette
    /// sampling time. Never copied into the per-instance payload.
    pub(super) pose_stack: postretro_model::pose_modifier::PoseModifierStack,
}

/// CPU animation assets moved into the mesh cache together at model install.
/// Clips remain in the cache-side lookup map; skeleton and modifier stack stay
/// paired on the uploaded-model entry used by palette sampling.
pub(in crate::render) struct ModelAnimationData {
    pub(in crate::render) skeleton: Skeleton,
    pub(in crate::render) clips: Vec<AnimationClip>,
    pub(in crate::render) pose_stack: postretro_model::pose_modifier::PoseModifierStack,
}

impl MeshPass {
    /// Insert (or replace) an uploaded skinned model keyed by `handle`. Uploads
    /// the mesh's vertex + index buffers and retains its per-submesh material
    /// bind groups plus the CPU-side animation data (skeleton + the full clip
    /// list) the per-frame palette is sampled from.
    ///
    /// `submeshes` pairs each material bind group with the index range it draws,
    /// in submesh order — built by the renderer via `build_material_bind_group`
    /// against the shared group-1 layout (the same `.prm` → `LoadedTexture` path
    /// the world uses). This is the cache-insertion seam the level-load model
    /// sweep calls once per distinct model at install.
    ///
    /// `clips` is the model's FULL animation set in glTF (authored) index order.
    /// Stored cache-side in `model_clips` for the name/metadata query seam and for
    /// per-instance sampling: `plan_and_upload` indexes this list by each
    /// instance's resolved `MeshSampleParams`. An empty list → the model holds its
    /// bind pose (identity palette run) every frame.
    pub(in crate::render) fn insert_model(
        &mut self,
        device: &wgpu::Device,
        handle: ModelHandle,
        mesh: &SkinnedMesh,
        submeshes: Vec<(wgpu::BindGroup, std::ops::Range<u32>)>,
        animation: ModelAnimationData,
    ) {
        let ModelAnimationData {
            skeleton,
            clips,
            pose_stack,
        } = animation;
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Skinned Mesh Vertex Buffer"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Skinned Mesh Index Buffer"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        // Stash the CPU-side conservative pose envelope for shadow and
        // sampled-SH planning. It lives outside `UploadedModel`; GPU draws never
        // read it.
        self.model_bounds.insert(handle.clone(), mesh.bounds());
        // Stash the full clip list cache-side (same rationale as `model_bounds`):
        // it keeps the clip-name / metadata query seam testable without a GPU.
        self.model_clips.insert(handle.clone(), clips);
        self.models.insert(
            handle,
            UploadedModel {
                vertex_buffer,
                index_buffer,
                submeshes,
                skeleton,
                pose_stack,
            },
        );
    }

    /// Look up an animation clip by authored `name` for the model at `handle`.
    /// Returns `None` when the handle is not cached or the model carries no clip
    /// of that name — absence is normal, never an error or panic.
    ///
    /// First match wins: glTF does not forbid duplicate animation names, so on a
    /// model with two clips sharing a name the earlier (lower glTF index) clip is
    /// returned. Clips are stored in authored order, so this is the first
    /// authored clip with the name.
    ///
    /// Delegates to the GPU-free [`clip_by_name`] over the `model_clips` map, so
    /// the lookup is unit-testable without `MeshPass::new` (mirrors the
    /// `postretro_render_cpu::mesh_pass::{mesh_visible, mesh_visible_in_cell}` split).
    ///
    /// The query seam awaits its runtime consumer (clip-name resolution at level
    /// load); the free [`clip_by_name`] it wraps is exercised by the GPU-free
    /// tests, so this thin device-bound wrapper carries an `allow(dead_code)` until
    /// that consumer lands, mirroring `ModelHandle::as_str`.
    #[allow(dead_code)]
    pub fn model_clip_by_name(&self, handle: &ModelHandle, name: &str) -> Option<&AnimationClip> {
        clip_by_name(&self.model_clips, handle, name)
    }

    /// The clip metadata (name + duration) for the model at `handle`, in glTF
    /// (authored) index order. Returns an empty `Vec` when the handle is not
    /// cached or the model has no animation — no error, no panic.
    ///
    /// Delegates to the GPU-free [`clip_metadata`] over the `model_clips` map for
    /// headless testability (same rationale as [`MeshPass::model_clip_by_name`]).
    /// Consumed at level load (via `Renderer::skinned_model_clip_metadata`) to
    /// build the game-side clip tables.
    pub fn model_clip_metadata(&self, handle: &ModelHandle) -> Vec<ClipMetadata> {
        clip_metadata(&self.model_clips, handle)
    }

    /// The cached conservative local-space pose envelope for a skinned model.
    /// Returns a zero box when absent, matching the planner's degradation path.
    pub fn model_local_bounds(
        &self,
        handle: &ModelHandle,
    ) -> postretro_render_data::cone_frustum::Aabb {
        self.model_bounds.get(handle).copied().unwrap_or_default()
    }

    /// Whether any model has been uploaded. The renderer skips the pass entirely
    /// when the cache is empty.
    pub fn has_model(&self) -> bool {
        !self.models.is_empty()
    }

    /// Level-load clear hook: reset per-level transient mesh-pass state. Called
    /// at the model-cache install site in the level-load sweep (mirrors
    /// `FogPass::clear_for_level_load`). The single per-level reset seam, so any
    /// future per-level state lands here rather than scattered.
    ///
    /// Empties both per-entity caches keyed by entity seed — the `"smooth"`-
    /// interrupt snapshot store and the time-slicing palette cache. Entity seeds
    /// are not stable across levels, so a stale snapshot or cached palette run
    /// from a prior level must not survive: a new level's instance reusing a prior
    /// seed would otherwise blend against (or re-upload) a pose from a different
    /// model.
    pub fn clear_for_level_load(&mut self) {
        self.snapshot_store.clear();
        self.palette_cache.clear();
    }

    /// Unload hook: drop per-level model GPU buffers and CPU mirrors. Renderer
    /// lifetime buffers/pipelines stay resident so the next load can rebuild.
    pub fn release_level_resources(&mut self) {
        self.models.clear();
        self.model_bounds.clear();
        self.model_clips.clear();
        self.capture_scratch.clear();
        self.clear_for_level_load();
    }
}

/// Look up an animation clip by authored `name` in a model-clip map. Pure data
/// logic — no GPU, no `MeshPass`. Backs [`MeshPass::model_clip_by_name`] and is
/// split out (the `model_bounds` / `postretro_render_cpu::mesh_pass::mesh_visible_in_cell` precedent) so the
/// clip-name query seam is testable without `MeshPass::new`, which needs a
/// `wgpu::Device`.
///
/// Returns `None` when `handle` is not in the map or its clip list holds no clip
/// of that name — absence is normal, never an error or panic. **First match
/// wins:** glTF does not forbid duplicate animation names, and clips are stored
/// in authored (glTF index) order, so the earliest authored clip with the name
/// is returned.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn clip_by_name<'a>(
    model_clips: &'a HashMap<ModelHandle, Vec<AnimationClip>>,
    handle: &ModelHandle,
    name: &str,
) -> Option<&'a AnimationClip> {
    model_clips
        .get(handle)?
        .iter()
        .find(|clip| clip.name == name)
}

/// The clip metadata (name + duration) for `handle` in a model-clip map, in glTF
/// (authored) index order. Pure data logic — no GPU. Backs
/// [`MeshPass::model_clip_metadata`]; split out for the same headless-testability
/// reason as [`clip_by_name`]. Returns an empty `Vec` when `handle` is absent or
/// its model has no animation — no error, no panic.
pub(super) fn clip_metadata(
    model_clips: &HashMap<ModelHandle, Vec<AnimationClip>>,
    handle: &ModelHandle,
) -> Vec<ClipMetadata> {
    model_clips
        .get(handle)
        .map(|clips| {
            clips
                .iter()
                .map(|clip| ClipMetadata {
                    name: clip.name.clone(),
                    duration: clip.duration,
                })
                .collect()
        })
        .unwrap_or_default()
}
