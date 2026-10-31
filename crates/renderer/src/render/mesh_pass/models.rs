// Skinned-model tables keyed by `ModelHandle`: one level set's model entries,
// their upload, lookup and the clip-name / metadata query seam.
// See: context/lib/rendering_pipeline.md §9

use crate::render::gpu_create::GpuCreate;
use std::collections::HashMap;
use std::sync::Arc;

use postretro_model::ModelHandle;
use postretro_model::mesh::SkinnedMesh;
use postretro_model::skeleton::{AnimationClip, Skeleton};
use postretro_render_cpu::mesh_instances::JointCounts;

use super::{ClipMetadata, MeshPass};

/// One uploaded skinned model: GPU vertex + index buffers, its per-submesh
/// material bind groups, and the skeleton the per-frame palette is sampled
/// against. A single-material model has one submesh spanning the whole index
/// buffer; multi-material models carry one entry per primitive, in submesh order.
///
/// The model's animation clips and pose envelope do NOT live here — they sit
/// beside it on its [`ModelEntry`], so the clip-name / metadata query seam is
/// testable without a GPU device over any [`ModelClipSource`].
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

/// One uploaded skinned model with its CPU-side companions: the conservative
/// local-space pose envelope (for shadow and sampled-SH planning; GPU draws
/// never read it) and the FULL clip list in glTF (authored) index order.
/// Level sets hold entries behind `Arc`, so a shared model cache can hand the
/// same entry to two sets.
pub(in crate::render) struct ModelEntry {
    pub(in crate::render) uploaded: UploadedModel,
    pub(in crate::render) bounds: postretro_render_data::cone_frustum::Aabb,
    pub(in crate::render) clips: Vec<AnimationClip>,
}

/// One level set's skinned models keyed by handle (the raw
/// `MeshComponent.model` string). One entry per distinct model; mirrors
/// `SmokePass::sheets`. The level-load model sweep populates the active
/// set's through `Renderer::upload_parsed_skinned_model`; the mesh pass
/// resolves every draw's handle through the active set's.
#[derive(Default)]
pub(in crate::render) struct LevelModels {
    entries: HashMap<ModelHandle, Arc<ModelEntry>>,
}

impl LevelModels {
    /// The uploaded model at `handle`, `None` when not cached.
    pub(in crate::render) fn get(&self, handle: &ModelHandle) -> Option<&UploadedModel> {
        self.entries.get(handle).map(|entry| &entry.uploaded)
    }

    /// The whole entry at `handle`, `None` when not cached.
    pub(in crate::render) fn entry(&self, handle: &ModelHandle) -> Option<&Arc<ModelEntry>> {
        self.entries.get(handle)
    }

    /// Insert (or replace) the entry at `handle`. Re-loading the same handle
    /// replaces the cache entry (idempotent upload).
    /// Every cached handle, sorted (tests and the set summary).
    pub(in crate::render) fn handles(&self) -> Vec<String> {
        let mut handles: Vec<String> = self
            .entries
            .keys()
            .map(|handle| handle.0.to_string())
            .collect();
        handles.sort();
        handles
    }

    pub(in crate::render) fn insert(&mut self, handle: ModelHandle, entry: Arc<ModelEntry>) {
        self.entries.insert(handle, entry);
    }

    /// Whether any model has been uploaded. The renderer skips the pass entirely
    /// when the cache is empty.
    pub(in crate::render) fn has_model(&self) -> bool {
        !self.entries.is_empty()
    }

    /// Move every entry of `other` in, keeping an entry this set already holds.
    pub(in crate::render) fn absorb(&mut self, other: &mut LevelModels) {
        for (handle, entry) in std::mem::take(&mut other.entries) {
            self.entries.entry(handle).or_insert(entry);
        }
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
    /// Delegates to the GPU-free [`clip_by_name`], so the lookup is
    /// unit-testable without a device (mirrors the
    /// `postretro_render_cpu::mesh_pass::{mesh_visible, mesh_visible_in_cell}` split).
    ///
    /// The query seam awaits its runtime consumer (clip-name resolution at level
    /// load); the free [`clip_by_name`] it wraps is exercised by the GPU-free
    /// tests, so this thin wrapper carries an `allow(dead_code)` until that
    /// consumer lands, mirroring `ModelHandle::as_str`.
    #[allow(dead_code)]
    pub(in crate::render) fn clip_by_name(
        &self,
        handle: &ModelHandle,
        name: &str,
    ) -> Option<&AnimationClip> {
        clip_by_name(self, handle, name)
    }

    /// The clip metadata (name + duration) for the model at `handle`, in glTF
    /// (authored) index order. Returns an empty `Vec` when the handle is not
    /// cached or the model has no animation — no error, no panic. Delegates to
    /// the GPU-free [`clip_metadata`]. Consumed at level load (via
    /// `Renderer::skinned_model_clip_metadata`) to build the game-side clip
    /// tables.
    pub(in crate::render) fn clip_metadata(&self, handle: &ModelHandle) -> Vec<ClipMetadata> {
        clip_metadata(self, handle)
    }

    /// The cached conservative local-space pose envelope for a skinned model.
    /// Returns a zero box when absent, matching the planner's degradation path.
    pub(in crate::render) fn local_bounds(
        &self,
        handle: &ModelHandle,
    ) -> postretro_render_data::cone_frustum::Aabb {
        self.entries
            .get(handle)
            .map(|entry| entry.bounds)
            .unwrap_or_default()
    }
}

/// Joint-count and model-bound lookup over one set's models, so the GPU-free
/// frame planner can assign palette runs and stamp per-light caster bounds
/// without a wgpu reference. Missing models keep the existing zero-bound
/// degradation path.
impl JointCounts for LevelModels {
    fn joint_count(&self, model: &ModelHandle) -> Option<u32> {
        self.get(model)
            .map(|model| model.skeleton.joints.len() as u32)
    }

    fn model_bounds(&self, model: &ModelHandle) -> postretro_render_data::cone_frustum::Aabb {
        self.local_bounds(model)
    }
}

/// Where the clip-name / metadata query seam reads a model's clips: a level
/// set's entries, or a bare handle → clips map in GPU-free tests.
pub(super) trait ModelClipSource {
    fn clips(&self, handle: &ModelHandle) -> Option<&[AnimationClip]>;
}

impl ModelClipSource for LevelModels {
    fn clips(&self, handle: &ModelHandle) -> Option<&[AnimationClip]> {
        self.entries.get(handle).map(|entry| entry.clips.as_slice())
    }
}

#[cfg(test)]
impl ModelClipSource for HashMap<ModelHandle, Vec<AnimationClip>> {
    fn clips(&self, handle: &ModelHandle) -> Option<&[AnimationClip]> {
        self.get(handle).map(Vec::as_slice)
    }
}

impl MeshPass {
    /// Upload one skinned model into a fresh entry: the mesh's vertex + index
    /// buffers, its per-submesh material bind groups, the CPU-side animation
    /// data (skeleton + the full clip list) the per-frame palette is sampled
    /// from, and the mesh's conservative pose envelope.
    ///
    /// `submeshes` pairs each material bind group with the index range it draws,
    /// in submesh order — built by the renderer via `build_material_bind_group`
    /// against the shared group-1 layout (the same `.prm` → `LoadedTexture` path
    /// the world uses). The caller inserts the entry into one level set's
    /// [`LevelModels`]; this is the upload seam the level-load model sweep calls
    /// once per distinct model.
    ///
    /// `clips` is the model's FULL animation set in glTF (authored) index order,
    /// for the name/metadata query seam and for per-instance sampling:
    /// `plan_and_upload` indexes this list by each instance's resolved
    /// `MeshSampleParams`. An empty list → the model holds its bind pose
    /// (identity palette run) every frame.
    pub(in crate::render) fn upload_model(
        device: &wgpu::Device,
        mesh: &SkinnedMesh,
        submeshes: Vec<(wgpu::BindGroup, std::ops::Range<u32>)>,
        animation: ModelAnimationData,
    ) -> ModelEntry {
        let ModelAnimationData {
            skeleton,
            clips,
            pose_stack,
        } = animation;
        let vertex_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Skinned Mesh Vertex Buffer"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Skinned Mesh Index Buffer"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        ModelEntry {
            uploaded: UploadedModel {
                vertex_buffer,
                index_buffer,
                submeshes,
                skeleton,
                pose_stack,
            },
            bounds: mesh.bounds(),
            clips,
        }
    }

    /// Level-load clear hook: reset per-level transient mesh-pass state. Called
    /// when a level set is published and at the model-cache install site in
    /// the level-load sweep (mirrors `FogPass::clear_for_level_load`). The
    /// single per-level reset seam, so any future per-level state lands here
    /// rather than scattered.
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

    /// Unload hook: drop per-level transient state. The models themselves
    /// drop with their level set; renderer-lifetime buffers and pipelines
    /// stay resident so the next load can rebuild.
    pub fn release_level_resources(&mut self) {
        self.capture_scratch.clear();
        self.clear_for_level_load();
    }
}

/// Look up an animation clip by authored `name` in a model-clip source. Pure
/// data logic — no GPU, no `MeshPass`. Backs [`LevelModels::clip_by_name`] and
/// is split out (the `postretro_render_cpu::mesh_pass::mesh_visible_in_cell`
/// precedent) so the clip-name query seam is testable without a
/// `wgpu::Device`.
///
/// Returns `None` when `handle` is not in the map or its clip list holds no clip
/// of that name — absence is normal, never an error or panic. **First match
/// wins:** glTF does not forbid duplicate animation names, and clips are stored
/// in authored (glTF index) order, so the earliest authored clip with the name
/// is returned.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn clip_by_name<'a, S: ModelClipSource + ?Sized>(
    model_clips: &'a S,
    handle: &ModelHandle,
    name: &str,
) -> Option<&'a AnimationClip> {
    model_clips
        .clips(handle)?
        .iter()
        .find(|clip| clip.name == name)
}

/// The clip metadata (name + duration) for `handle` in a model-clip source, in
/// glTF (authored) index order. Pure data logic — no GPU. Backs
/// [`LevelModels::clip_metadata`]; split out for the same headless-testability
/// reason as [`clip_by_name`]. Returns an empty `Vec` when `handle` is absent or
/// its model has no animation — no error, no panic.
pub(super) fn clip_metadata<S: ModelClipSource + ?Sized>(
    model_clips: &S,
    handle: &ModelHandle,
) -> Vec<ClipMetadata> {
    model_clips
        .clips(handle)
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
