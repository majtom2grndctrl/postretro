// Skinned-mesh render pass: forward draw of many instances of many skinned
// models against a shared bone palette.
// See: context/lib/rendering_pipeline.md §9
//
// Mirrors the shape of `crate::render::smoke::SmokePass` (`new` builds the
// pipeline + layouts; `render_frame` writes the per-frame buffers + records the
// draws). The per-handle model table is a level resource (`LevelModels`, in the
// level set), as smoke's sprite sheets are in `SmokeLevelCollections`. Owns ALL
// wgpu for skinned meshes — `postretro_model` stays wgpu-free.
//
// Binding plan (forward, non-shadow):
//   * group 0 = camera (shared renderer-owned camera uniform / bind group)
//   * group 1 = material (the `build_material_bind_group` bind group — the SH-lit
//               fragment samples diffuse + character-model sampler from this group)
//   * group 2 = runtime direct lighting + shadow receipt (fully allocated b0–b10):
//               b0 three regions: dynamic prefix, raw animated-baked tail, and
//               selected-static suffix, b1
//               per-light influence volumes, b2
//               scripted-animation descriptors, b3 scripted-animation curve
//               samples, b4 the mesh-side params uniform; b5 spot shadow depth,
//               b6 comparison sampler, b7 light-space matrices uniform, b8 the
//               conditional cube-array depth (present iff `cube_array_supported`),
//               b9 promoted spot depth cache, b10 the conditional promoted cube
//               depth cache (present iff `cube_array_supported`).
//               SH indirect ships at group 4, so this is not the SH ambient slot.
//   * group 3 = skinned instance data: shared bone-palette storage buffer
//               (binding 0) + per-instance SSBO carrying each instance's model
//               matrix and palette base index, addressed by
//               `@builtin(instance_index)` (binding 1)
//   * group 4 = SH irradiance volume (`ShVolumeResources.mesh_bind_group` —
//               the SUPERSET bind group that extends the shared SH entries with
//               the direct-atlas texture at binding 15 and the
//               `DynamicDirectParams` uniform at binding 16; forward/billboard/
//               fog passes use the smaller base `bind_group` and its layout)
//
// Per-instance addressing: the palette base index lives in the per-instance SSBO
// entry, NOT in `first_instance`/`base_instance` — DX12 reads that as 0
// (gfx-rs/wgpu#2471) and it needs `INDIRECT_FIRST_INSTANCE` which we do not
// assume. The shader reads its instance via `@builtin(instance_index)`.
//
// Coordinate basis: the engine world is Y-up, right-handed, metric (camera
// builds via glam's `camera::rh` view / projection with up = +Y; the level compiler
// works in meters). glTF is ALSO Y-up, right-handed, meters, and positions are
// stored verbatim. So the glTF→engine basis conversion is the IDENTITY — no
// axis swap, no mirror, no scale. Winding matches too: glTF front faces are CCW
// and the engine forward pipeline is `front_face: Ccw` + `cull_mode: Back`, so
// we keep that here and front faces render. The per-instance model matrix is
// therefore the entity transform applied directly. (Models are authored facing
// `+Z`; the half turn that points that front along a map arrow is gameplay-facing,
// not a basis bug, and is applied once at spawn by
// `MapEntity::model_rotation_quat()` in `postretro-sim`.)

mod frame_upload;
mod lighting;
pub(in crate::render) use lighting::{LightGroupInputs, LightGroupLayout};
mod models;
mod pipeline;
mod pose;
mod record;
#[cfg(test)]
mod tests;

use postretro_model::BonePaletteEntry;
use postretro_model::anim::LocalTrs;

pub(super) use models::{LevelModels, ModelAnimationData, ModelEntry, UploadedModel};
use pose::{PaletteCache, SnapshotStore};

/// Byte size of one `BonePaletteEntry` (mat4x4<f32> = 64 B).
const BONE_PALETTE_ENTRY_SIZE: usize = std::mem::size_of::<BonePaletteEntry>();

/// Per-instance SSBO entry: model matrix (64 B) + base index and receiver-bias
/// scale packed into a trailing `vec4<u32>` (16 B) = 80 B. Matches the WGSL
/// `Instance` std430 struct (base at byte 64, scale at byte 68). The instance
/// SSBO is an array of these, read by
/// `@builtin(instance_index)`; the same shape drops into a future
/// `multi_draw_indexed_indirect` per-instance buffer without a contract change.
const INSTANCE_ENTRY_SIZE: usize = 80;

pub use postretro_model::ClipMetadata;

/// GPU resources for the skinned-mesh forward pass.
pub struct MeshPass {
    pipeline: wgpu::RenderPipeline,

    /// Depth-only skinned pipeline (shadow occluders). Skins vertices with the
    /// same `skin_matrix` kernel and projects by a per-render light-space matrix
    /// (group 0) supplied by the caller — one pipeline for both spot slots and
    /// cube faces. Shares group 3 (palette + instances) with `pipeline`,
    /// so it reads the SAME per-frame posed buffers with no extra upload.
    /// That "no extra upload" guarantee rests on an ordering invariant enforced
    /// OUTSIDE this struct: the pose/palette/instance buffers are written once per
    /// frame by the palette hoist (`plan_and_upload`, called from `render/mod.rs`'s
    /// frame loop after `update_dynamic_light_slots`) BEFORE the shadow depth loop
    /// reads them, and nothing rewrites them between the hoist and the forward draw.
    /// A future agent inserting a buffer-writing step between the hoist and the
    /// depth passes would silently break this — keep the hoist immediately ahead of
    /// every shadow pass that binds group 3.
    pub(super) depth_pipeline: wgpu::RenderPipeline,

    /// Unlit ghost pipelines, a colour and a depth-only one per
    /// `render.ghosts` mode, over the same palette + instance bind group.
    /// Built in `mesh_ghost.rs`; the set's shape lives in `ghost_pipeline.rs`.
    pub(super) ghost_pipelines: super::ghost_pipeline::GhostPipelines,

    /// Shared bone-palette storage buffer, sized for `MAX_PALETTE_ENTRIES`
    /// entries. Each instance's contiguous run of joints is written at its
    /// planned base index before the draw is recorded.
    palette_buffer: wgpu::Buffer,

    /// Per-instance SSBO (group 3 binding 1), sized for `MAX_INSTANCES` entries.
    /// Filled densely each frame from the frame plan and read by
    /// `@builtin(instance_index)`.
    instance_buffer: wgpu::Buffer,

    /// Group 3 bind group: shared palette (binding 0) + the per-instance SSBO
    /// (binding 1). Both buffers are fixed-size and reused every frame, so the
    /// bind group is built once at init.
    pub(super) instance_bind_group: wgpu::BindGroup,

    /// Camera-compatible group-0 bind group whose buffer carries the alternate
    /// first-person view-projection. It reuses the unchanged world camera
    /// layout and the skinned mesh pipeline; the renderer selects it only for
    /// the dedicated viewmodel pass.
    viewmodel_uniform_buffer: wgpu::Buffer,
    viewmodel_uniform_bind_group: wgpu::BindGroup,

    /// Group 2 BGL (runtime direct lighting). Pinned binding map (see
    /// [`MeshPass::new`]): b0 dynamic prefix, raw animated-baked tail, then
    /// selected-static suffix; b1 per-light influence volumes, b2
    /// scripted-animation descriptors, b3 scripted-animation curve samples, b4
    /// the mesh-side params uniform; b5–b10 shadow receipt. b0–b3 alias the
    /// SAME level-set GPU buffers forward binds; b4 is owned here. Retained so
    /// each level set can build its own group-2 bind groups
    /// ([`MeshPass::light_group_layout`]).
    light_bind_group_layout: wgpu::BindGroupLayout,

    // The group-2 bind groups themselves (main view and planar mirror) are
    // level resources: [`LightGroupLayout::light_bind_groups`] builds them over one
    // load's light buffers into that load's `LevelResourceSet`.
    /// Whether this frame's mirror draws read the mirror's light records:
    /// set by slot assignment each frame, true only while a mirror-only light
    /// holds a shadow slot.
    mirror_light_records: bool,

    /// Group 2 binding 4 params uniform (`MeshLightParams`): light count, the
    /// frame's forward `time`, and the captured forward `light_term_mask`.
    /// Fixed-size, owned here, written per frame by
    /// [`MeshPass::write_light_params`]; rebound by reference into every rebuilt
    /// group-2 bind group.
    light_params_buffer: wgpu::Buffer,

    /// Adapter cube-array support (`DownlevelFlags::CUBE_ARRAY_TEXTURES`), threaded
    /// from `Renderer::new`. Pins the `Some`-iff-layout invariant: the group-2 BGL
    /// carries the b8 and b10 cube entries iff this is `true`, so
    /// `LightGroupLayout::light_bind_groups` takes the cube views `Some` iff
    /// this is `true`. Fixed for the renderer's
    /// lifetime — the same flag drives the pipeline's no-cube shader strip.
    cube_array_supported: bool,

    // The uploaded models, their pose envelopes and clips are level
    // resources: each [`LevelModels`] lives in its level set, and every draw
    // resolves its handle through the active set's.
    /// Per-entity `"smooth"`-interrupt snapshot store, keyed by entity seed. A
    /// GPU-free CPU map (the `model_bounds` precedent): a capture instruction
    /// installs an entry, and end-of-frame retention keeps it only while the
    /// current plan has an active matching snapshot fade for that entity. Level
    /// load clears the store. The frozen pose is the blend source a `"smooth"`
    /// fade resumes from with no discontinuity.
    snapshot_store: SnapshotStore,

    /// Per-entity palette cache for animation time-slicing, keyed by
    /// entity seed — the `model_bounds`/`SnapshotStore` GPU-free precedent. On a
    /// resample frame the freshly sampled run refreshes the cache; on a skipped
    /// frame the cached run is re-uploaded with no sampling; a cache miss forces a
    /// resample. Per-frame eviction bounds it by the planned-instance count, and
    /// [`MeshPass::clear_for_level_load`] empties it at level load.
    palette_cache: PaletteCache,

    /// Reusable per-joint local-TRS scratch for snapshot CAPTURE (kept off the
    /// hot path; capture is a one-time event, not steady-state). Separate from
    /// the renderer's palette scratch so a capture does not clobber an in-flight
    /// pose sample.
    capture_scratch: Vec<LocalTrs>,
}
