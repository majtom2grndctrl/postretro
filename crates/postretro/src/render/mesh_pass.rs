// Skinned-mesh render pass: forward draw of many instances of many skinned
// models against a shared bone palette.
// See: context/lib/rendering_pipeline.md §9
//
// Mirrors the shape of `crate::render::smoke::SmokePass` (`new` builds the
// pipeline + layouts; a model cache keyed by handle mirrors `SmokePass::sheets`;
// `render_frame` writes the per-frame buffers + records the draws). Owns ALL
// wgpu for skinned meshes — `crate::model` stays wgpu-free.
//
// Binding plan (forward, non-shadow):
//   * group 0 = camera (shared renderer-owned camera uniform / bind group)
//   * group 1 = material (the `build_material_bind_group` bind group — the SH-lit
//               fragment samples diffuse + aniso sampler from this group)
//   * group 2 = dynamic direct lighting + shadow receipt (fully allocated b0–b8):
//               b0 dynamic-light records, b1 per-light influence volumes, b2
//               scripted-animation descriptors, b3 scripted-animation curve
//               samples, b4 the mesh-side params uniform; b5 spot shadow depth,
//               b6 comparison sampler, b7 light-space matrices uniform, b8 the
//               conditional cube-array depth (present iff `cube_array_supported`).
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
// builds via `look_at_rh` / `perspective_rh` with up = +Y; the level compiler
// works in meters). glTF is ALSO Y-up, right-handed, meters, and positions are
// stored verbatim. So the glTF→engine basis conversion is the IDENTITY — no
// axis swap, no mirror, no scale. Winding matches too: glTF front faces are CCW
// and the engine forward pipeline is `front_face: Ccw` + `cull_mode: Back`, so
// we keep that here and front faces render. The per-instance model matrix is
// therefore the entity transform applied directly. (A model authored facing a
// particular axis may need a yaw baked into the entity transform — that is
// gameplay-facing, not a basis bug; see
// `context/plans/done/M10--model-pipeline-slice/findings.md`
// (coordinate-system read).)

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use crate::model::anim::{BlendSource, LocalTrs};
use crate::model::mesh::SkinnedMesh;
use crate::model::sample_params::{ClipSample, FadeSource, MeshSampleParams, SnapshotTag};
use crate::model::skeleton::{AnimationClip, Skeleton};
use crate::model::{BonePaletteEntry, ModelHandle};
use crate::prl::LevelWorld;
use crate::render::mesh_instances::{
    JointCounts, MAX_INSTANCES, MAX_PALETTE_ENTRIES, MeshFramePlan, instance_casts_into_cone,
};
use crate::visibility::VisibleCells;

/// Byte size of one `BonePaletteEntry` (mat4x4<f32> = 64 B).
const BONE_PALETTE_ENTRY_SIZE: usize = std::mem::size_of::<BonePaletteEntry>();

/// Per-instance SSBO entry: model matrix (64 B) + base index packed into a
/// trailing `vec4<u32>` (16 B) = 80 B. Matches the WGSL `Instance` std430
/// struct (base at byte 64). The instance SSBO is an array of these, read by
/// `@builtin(instance_index)`; the same shape drops into a future
/// `multi_draw_indexed_indirect` per-instance buffer without a contract change.
const INSTANCE_ENTRY_SIZE: usize = 80;

/// Pack one instance's SSBO bytes (model matrix column-major + base index).
fn build_instance_entry(model: glam::Mat4, base_index: u32) -> [u8; INSTANCE_ENTRY_SIZE] {
    let mut bytes = [0u8; INSTANCE_ENTRY_SIZE];
    let cols = model.to_cols_array();
    for (i, v) in cols.iter().enumerate() {
        let off = i * 4;
        bytes[off..off + 4].copy_from_slice(&v.to_ne_bytes());
    }
    // base index at offset 64 (x of the trailing vec4<u32>); 68..80 stay zero.
    bytes[64..68].copy_from_slice(&base_index.to_ne_bytes());
    bytes
}

// `skinned_mesh.wgsl` declares the four SH bindings at group 4 (b1/b2/b10/b14);
// `sh_sample.wgsl` is the binding-agnostic depth-aware octahedral helper it
// calls (`sample_sh_indirect_corners_depth_aware`). WGSL resolves module-scope
// names regardless of textual order, so appending the helper after is safe —
// the same string-concat mechanism `render/mod.rs::SHADER_SOURCE` uses to
// assemble forward.wgsl.
//
// The mesh path NOW carries the dynamic-direct light scaffolding: `skinned_mesh.wgsl`
// declares the group-2 bindings (lights, influence volumes, scripted descriptors,
// `anim_samples`, params uniform) the runtime light loop reads,
// so the shared `light_eval.wgsl` per-light helpers and the `curve_eval.wgsl`
// Catmull-Rom samplers they call are appended here — mirroring the forward
// composition (`render/mod.rs::SHADER_SOURCE`). `curve_eval.wgsl` reads
// `anim_samples` (declared at group 2 binding 3 below) and `light_eval.wgsl`'s
// `light_eval_animated_direction` calls `sample_color_catmull_rom` from
// curve_eval, so both must be present together; WGSL resolves module-scope names
// regardless of textual order so the relative append order of these two is free.
// (The prior "mesh never evaluates animated layers" note is no longer true: the
// scripted-light direction/intensity curves are evaluated against group 2.)
//
// `shadow_sample.wgsl` (the shared runtime shadow-map samplers `sample_spot_shadow`
// / `sample_point_shadow` + their bias/resolution constants) is appended LAST so
// the runtime dynamic-light loop's per-light visibility term can call it against
// the mesh's own group-2 b5–b8 shadow bindings (declared in `skinned_mesh.wgsl`).
// It declares no bindings
// itself — it references `spot_shadow_depth` / `spot_shadow_compare` /
// `light_space_matrices` / `point_shadow_cube` by lexical name, the same way
// forward.wgsl composes it. On a no-`CUBE_ARRAY_TEXTURES` adapter the composed
// source runs through `render::strip_point_shadow_cube` (see
// `skinned_mesh_shader_source`), which drops the `// CUBE_SHADOW_BINDING`-tagged b8
// declaration and replaces `sample_point_shadow`'s body with `return 1.0;`.
const SKINNED_MESH_SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/skinned_mesh.wgsl"),
    "\n",
    include_str!("../shaders/sh_sample.wgsl"),
    "\n",
    include_str!("../shaders/curve_eval.wgsl"),
    "\n",
    include_str!("../shaders/light_eval.wgsl"),
    "\n",
    include_str!("../shaders/shadow_sample.wgsl"),
);

/// Compose the skinned-mesh shader source for the adapter's cube-array support.
/// On a `CUBE_ARRAY_TEXTURES` adapter the canonical `SKINNED_MESH_SHADER_SOURCE`
/// is used verbatim (b8 cube binding declared, `sample_point_shadow` samples the
/// cube). On an adapter WITHOUT it, `render::strip_point_shadow_cube` removes the
/// `// CUBE_SHADOW_BINDING`-tagged b8 declaration and neutralizes
/// `sample_point_shadow` (body → `return 1.0;`) so the shader matches a group-2
/// BGL that omits b8 — exactly the same marker mechanism the forward pass uses on
/// its group-5 cube binding. Returns an owned `Cow` so the supported path pays no
/// allocation.
fn skinned_mesh_shader_source(cube_array_supported: bool) -> std::borrow::Cow<'static, str> {
    if cube_array_supported {
        std::borrow::Cow::Borrowed(SKINNED_MESH_SHADER_SOURCE)
    } else {
        std::borrow::Cow::Owned(crate::render::strip_point_shadow_cube(
            SKINNED_MESH_SHADER_SOURCE,
        ))
    }
}

/// Mesh-side group-2 params uniform (binding 4): dynamic-light count, the frame's
/// render-clock time, and `lighting_isolation`. `time` is the SAME render-clock
/// value the renderer uploads to forward `Uniforms.time` that frame (the renderer
/// caches it and threads it in), so the scripted-light animated curves the mesh
/// loop evaluates stay phase-coherent with the forward pass and the CPU light
/// bridge. `lighting_isolation` is the SAME `LightingIsolation` value the renderer
/// writes to forward `Uniforms.lighting_isolation` that frame, so the mesh
/// dynamic-direct term participates in the lighting-isolation debug modes exactly
/// as the world dynamic term does (the shader derives `use_dynamic` from it,
/// mirroring forward.wgsl). `ambient_floor` is the SAME constant ambient fill the
/// renderer uploads to forward `Uniforms.ambient_floor` that frame; the mesh
/// fragment shader adds it once as an additive fill so shadowed mesh faces lift
/// with the diagnostics slider exactly as world surfaces do (see forward.wgsl's
/// ambient-floor term). std140-padded to 16 bytes (the WGSL `MeshLightParams`
/// struct mirrors this layout: `light_count: u32`, `time: f32`,
/// `lighting_isolation: u32`, `ambient_floor: f32`).
#[repr(C)]
#[derive(Clone, Copy)]
struct MeshLightParams {
    light_count: u32,
    time: f32,
    lighting_isolation: u32,
    ambient_floor: f32,
}

/// Byte size of the group-2 params uniform (`MeshLightParams`, 16 B).
const MESH_LIGHT_PARAMS_SIZE: u64 = std::mem::size_of::<MeshLightParams>() as u64;

/// Serialize `MeshLightParams` to its 16-byte std140 upload, field order matching
/// the struct (and the WGSL mirror): `light_count` (0..4), `time` (4..8),
/// `lighting_isolation` (8..12), `ambient_floor` (12..16). Split out from
/// `write_light_params` so the byte layout can be asserted GPU-free in tests.
fn build_light_params_bytes(params: MeshLightParams) -> Vec<u8> {
    [
        params.light_count.to_ne_bytes(),
        params.time.to_ne_bytes(),
        params.lighting_isolation.to_ne_bytes(),
        params.ambient_floor.to_ne_bytes(),
    ]
    .concat()
}

/// Depth-only skinned shader: position + joints + weights, skinned by the shared
/// `skin_matrix` kernel and projected by a per-render light-space matrix (group
/// 0). Renders animated entity occluders into a shadow map. Standalone (no
/// helper append) — it declares only the buffers it reads.
const SKINNED_DEPTH_SHADER_SOURCE: &str = include_str!("../shaders/skinned_depth.wgsl");

/// GPU-free builder for the mesh group-2 (dynamic direct lighting + shadow
/// receipt) BGL entries. Single source of truth: `MeshPass::new` builds the layout
/// from this, and the headless `mesh_group2_bgl_matches_shader_bindings` test
/// re-derives the binding map and per-stage storage budget from the SAME entries —
/// so a drift in either the shader's group-2 declarations or the budget fails CI
/// before a real GPU would reject the pipeline. Pinned binding map (mirrors
/// `skinned_mesh.wgsl` group 2 and rendering_pipeline.md §9, §10):
///   b0 dynamic-light records (the `is_dynamic`-filtered set), b1 per-light
///   influence volumes, b2 scripted-animation descriptors, b3 scripted-animation
///   curve samples, b4 the mesh-side params uniform (all FRAGMENT-only). The
///   dynamic-light loop runs in the fragment stage, so b0–b3 contribute FOUR
///   fragment-visible storage buffers — well under the per-stage ceiling of 8
///   (rendering_pipeline.md §10). b4 is a uniform (no storage-slot cost).
///   b5–b8 are the SHADOW-RECEIPT bindings on a MESH-SPECIFIC layout that omits
///   forward's SDF-factor + scene-depth entries the mesh must not sample. They
///   alias the SAME GPU resources the forward pass binds in its group 5 (NOT
///   forward's group-5 BGL):
///     b5 spot depth 2D-array (`spot_shadow_depth`, FRAGMENT),
///     b6 comparison sampler (`spot_shadow_compare`, FRAGMENT),
///     b7 light-space matrices UNIFORM (`light_space_matrices`, FRAGMENT) — a
///        uniform, NOT storage, so it adds NOTHING to the fragment storage-buffer
///        count (still 4); same `array<mat4x4<f32>, SHADOW_POOL_SIZE>` budget the
///        forward shader uses (well under the 16 KiB uniform cap),
///     b8 cube-array depth (`point_shadow_cube`, `texture_depth_cube_array`,
///        FRAGMENT) — present ONLY when `cube_array_supported`. A `CubeArray` BGL
///        entry requires `DownlevelFlags::CUBE_ARRAY_TEXTURES`, so on an adapter
///        without it the entry is omitted (and `render::strip_point_shadow_cube`
///        drops the matching shader declaration), exactly as forward's group-5 BGL
///        omits its binding 5. The cube view is passed `Some` to
///        `rebuild_light_bind_group` iff this entry is present (the
///        `Some`-iff-layout invariant — a single unconditional BGL crashes on a
///        no-cube adapter).
///
/// b5 + b8 are sampled depth textures (spot 2D-array always, cube array iff
/// supported): the mesh pipeline's group-2 sampled-texture count is ONE without
/// cube support and TWO with it.
fn mesh_light_bind_group_layout_entries(
    cube_array_supported: bool,
) -> Vec<wgpu::BindGroupLayoutEntry> {
    let storage_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let mut entries = vec![
        // b0: dynamic-light records (is_dynamic-filtered set).
        storage_entry(0),
        // b1: per-light influence volumes.
        storage_entry(1),
        // b2: scripted-animation descriptors (forward group-3 b13).
        storage_entry(2),
        // b3: scripted-animation curve samples (forward group-3 b12).
        storage_entry(3),
        // b4: mesh-side params uniform (light count, time, lighting_isolation).
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        // b5: spot shadow depth 2D-array (`spot_shadow_depth`). SAME texture the
        // forward pass binds at group-5 b0 (the spot pool's `array_view`).
        wgpu::BindGroupLayoutEntry {
            binding: 5,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        // b6: comparison sampler (`spot_shadow_compare`). SAME sampler the forward
        // pass binds at group-5 b1; reused by the cube path too.
        wgpu::BindGroupLayoutEntry {
            binding: 6,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
            count: None,
        },
        // b7: light-space matrices UNIFORM (`light_space_matrices`). SAME buffer
        // the forward pass binds at group-5 b2 — a uniform (NOT storage) to keep
        // the fragment storage-buffer count at 4 (rendering_pipeline.md §10).
        wgpu::BindGroupLayoutEntry {
            binding: 7,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: std::num::NonZeroU64::new(
                    crate::lighting::spot_shadow::LIGHT_SPACE_MATRICES_SIZE,
                ),
            },
            count: None,
        },
    ];
    // b8: dynamic POINT-light cube-array shadow depth (`point_shadow_cube`). SAME
    // `CubeArray` view the forward pass binds at group-5 b5 (the cube pool's
    // `sampling_view`). Present ONLY when `cube_array_supported`: a `CubeArray` BGL
    // entry requires `DownlevelFlags::CUBE_ARRAY_TEXTURES`, so omitting it lets the
    // mesh pipeline build on adapters without the feature (the no-cube shader
    // variant strips the matching declaration). The cube view is supplied `Some`
    // to `rebuild_light_bind_group` iff this entry is present.
    if cube_array_supported {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 8,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::CubeArray,
                multisampled: false,
            },
            count: None,
        });
    }
    entries
}

/// Count BGL entries that consume a `max_storage_buffers_per_shader_stage` slot
/// for the FRAGMENT stage: read-only storage `Buffer` entries whose visibility
/// includes FRAGMENT. wgpu charges this limit against the BGL *entry* set of a
/// pipeline layout per stage, not against what a shader reads. Mirrors
/// `render::mod::vertex_storage_buffers` for the fragment stage; the mesh
/// dynamic-light loop is the mesh fragment stage's first storage-buffer use.
#[cfg(test)]
fn fragment_storage_buffers(entries: &[wgpu::BindGroupLayoutEntry]) -> u32 {
    entries
        .iter()
        .filter(|e| {
            e.visibility.contains(wgpu::ShaderStages::FRAGMENT)
                && matches!(
                    e.ty,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { .. },
                        ..
                    }
                )
        })
        .count() as u32
}

/// Count BGL entries that consume a `max_sampled_textures_per_shader_stage` slot
/// for the FRAGMENT stage: `BindingType::Texture` entries whose visibility
/// includes FRAGMENT. wgpu charges this limit against the BGL *entry* set of a
/// pipeline layout per stage, not against how many textures a shader samples.
/// Mirrors `render::mod::fragment_sampled_textures` for the mesh group-2 budget
/// guard; the mesh group-2 shadow textures (spot depth array + the optional cube
/// array) are the mesh fragment stage's group-2 sampled-texture draw.
#[cfg(test)]
fn fragment_sampled_textures(entries: &[wgpu::BindGroupLayoutEntry]) -> u32 {
    entries
        .iter()
        .filter(|e| {
            e.visibility.contains(wgpu::ShaderStages::FRAGMENT)
                && matches!(e.ty, wgpu::BindingType::Texture { .. })
        })
        .count() as u32
}

/// One uploaded skinned model: GPU vertex + index buffers, its per-submesh
/// material bind groups, and the skeleton the per-frame palette is sampled
/// against. A single-material model has one submesh spanning the whole index
/// buffer; multi-material models carry one entry per primitive, in submesh order.
///
/// The model's animation clips do NOT live here — they sit in the cache-side
/// `MeshPass::model_clips` map (the `model_bounds` precedent) so the clip-name /
/// metadata query seam is testable without a GPU device. The render path reaches
/// them through that map by the same handle.
struct UploadedModel {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    /// Per-submesh material bind group (group 1) + its `start..end` range into
    /// the merged index buffer, in submesh order. Distinct keys are deduped
    /// upstream, so submeshes reusing a material share a (cloned) bind group.
    submeshes: Vec<(wgpu::BindGroup, std::ops::Range<u32>)>,
    /// Skeleton for pose sampling. Joint count == `skeleton.joints.len()` is the
    /// per-instance palette run length.
    skeleton: Skeleton,
}

/// Metadata for one animation clip — its authored name and duration in seconds.
/// Returned by [`MeshPass::model_clip_metadata`] in glTF (authored) index order
/// so a consumer can enumerate a model's clips without holding the clip data.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipMetadata {
    /// Clip name as authored in the glTF document.
    pub name: String,
    /// Clip length in seconds (latest keyframe time across all tracks).
    pub duration: f32,
}

/// One captured `"smooth"`-interrupt snapshot: the per-joint local-TRS pose
/// frozen at the interrupt instant, tagged with the entered state's entry stamp.
/// A subsequent snapshot fade blends against `pose` only when its
/// [`SnapshotTag`] matches `tag`; a mismatch (a replacement fade) drops the entry.
///
/// The pose buffer is owned (cloned out of the sampler's snapshot capture) — a
/// snapshot outlives the frame that captured it, so it cannot borrow the
/// per-frame scratch.
#[derive(Debug, Clone, PartialEq)]
struct StoredSnapshot {
    tag: SnapshotTag,
    pose: Vec<LocalTrs>,
}

/// Per-entity snapshot store for `"smooth"` interrupts: a plain CPU-side map
/// keyed by entity seed, each entry tagged. A GPU-free seam (mirrors the
/// `model_bounds` precedent) so the smooth-interrupt logic is unit-testable
/// without a `wgpu::Device`.
///
/// Lifecycle: a capture instruction installs (or refreshes) an entry; a planned
/// frame without an active snapshot fade for that entity (fade over, or tag
/// mismatch on replacement) drops it. Bounded by planned-instance count and
/// emptied wholesale at level load by [`MeshPass::clear_for_level_load`].
#[derive(Debug, Default)]
struct SnapshotStore {
    entries: HashMap<u32, StoredSnapshot>,
}

impl SnapshotStore {
    /// Apply a capture instruction: capture `blend(outgoing, incoming)` at the
    /// instruction's weight into the store, tagged. **Idempotent:** if the stored
    /// entry already carries this tag, nothing is evaluated (a re-emission under a
    /// frozen clock is a no-op). A snapshot-referencing outgoing source that
    /// MISSES the store captures `blend(fallback, incoming)` instead (the capture
    /// frame for the referenced snapshot was culled — degrade to the fallback).
    ///
    /// `resolve_clip` maps a clip index to its `&AnimationClip` (the model's clip
    /// list); a missing clip aborts the capture (no usable pose).
    fn apply_capture<'a>(
        &mut self,
        capture: &crate::model::sample_params::CaptureInstruction,
        skeleton: &Skeleton,
        resolve_clip: impl Fn(usize) -> Option<&'a AnimationClip>,
        scratch: &mut Vec<LocalTrs>,
    ) {
        // Idempotent: a matching tag means this capture already landed.
        if self
            .entries
            .get(&capture.seed)
            .is_some_and(|e| e.tag == capture.tag)
        {
            return;
        }

        // Resolve the incoming (entered) clip leg.
        let Some(incoming) = clip_blend_source(&capture.incoming, &resolve_clip) else {
            return;
        };

        // Resolve the outgoing source: a snapshot reference blends against the
        // stored pose if present, else its fallback clip (degrade-on-miss). A
        // clip source resolves directly.
        let outgoing_clip;
        let outgoing: BlendSource = match capture.outgoing {
            FadeSource::Snapshot { tag, fallback } => {
                match self.entries.get(&capture.seed) {
                    Some(stored) if stored.tag == tag => BlendSource::Snapshot(&stored.pose),
                    _ => {
                        // Store miss / stale tag: capture from the fallback clip.
                        let Some(src) = clip_blend_source(&fallback, &resolve_clip) else {
                            return;
                        };
                        outgoing_clip = src;
                        outgoing_clip.as_blend_source()
                    }
                }
            }
            FadeSource::Clip(leg) => {
                let Some(src) = clip_blend_source(&leg, &resolve_clip) else {
                    return;
                };
                outgoing_clip = src;
                outgoing_clip.as_blend_source()
            }
        };

        crate::model::anim::capture_blend(
            &outgoing,
            &incoming.as_blend_source(),
            capture.weight,
            skeleton,
            scratch,
        );
        self.entries.insert(
            capture.seed,
            StoredSnapshot {
                tag: capture.tag,
                pose: scratch.clone(),
            },
        );
    }

    /// Look up an entry whose tag matches `tag`, for a snapshot fade.
    fn matching(&self, seed: u32, tag: SnapshotTag) -> Option<&[LocalTrs]> {
        self.entries
            .get(&seed)
            .filter(|e| e.tag == tag)
            .map(|e| e.pose.as_slice())
    }

    /// Drop an entity's entry (fade over, or a replacement-fade tag mismatch).
    fn drop_entry(&mut self, seed: u32) {
        self.entries.remove(&seed);
    }

    /// Empty the store (level-load clear).
    fn clear(&mut self) {
        self.entries.clear();
    }
}

/// One cached palette run: the last sampled bone-palette matrices for an entity,
/// reused on a time-sliced SKIP frame so the pass re-uploads a valid pose without
/// re-sampling. The `Vec` is reused in place on a resample (cleared + extended),
/// so a steady-state cache hit allocates nothing.
#[derive(Debug, Default)]
struct CachedPalette {
    /// The entity's last sampled palette run (one `BonePaletteEntry` per joint).
    run: Vec<BonePaletteEntry>,
    /// Set true when the entry is touched (resample or skip) this frame; entries
    /// left `false` after a frame are evicted, so the cache never exceeds the
    /// frame's planned-instance count.
    seen_this_frame: bool,
}

/// Renderer-side per-entity palette cache for animation time-slicing,
/// keyed by entity seed — the `SnapshotStore`/`model_bounds` precedent (GPU-free
/// data logic, unit-testable without a `wgpu::Device`). On a RESAMPLE frame the
/// pass samples the pose and refreshes the cached run; on a SKIPPED frame it
/// re-uploads the cached run with no sampling. A cache MISS forces a resample
/// that frame regardless of the collector's flag (the collector cannot see
/// renderer-side cache state), so a culled instance re-entering view never shows
/// a stale pose.
///
/// Eviction: entries not touched in a frame are dropped at [`end_frame`], so the
/// cache is bounded by the frame's planned-instance count (≤ `MAX_INSTANCES`
/// entries, ≤ `MAX_PALETTE_ENTRIES` total slots). Emptied wholesale at level load
/// by [`PaletteCache::clear`] (entity seeds are not stable across levels).
///
/// [`end_frame`]: PaletteCache::end_frame
#[derive(Debug, Default)]
struct PaletteCache {
    entries: HashMap<u32, CachedPalette>,
}

impl PaletteCache {
    /// Resolve whether this instance must sample this frame. Returns `true` when
    /// the collector asked to resample OR the cache misses (no entry for this
    /// seed) — the miss upgrade is what keeps a re-entering instance from showing
    /// a stale pose. A `false` return means a valid cached run exists and the
    /// collector cleared the instance to skip.
    fn must_sample(&self, seed: u32, collector_resample: bool) -> bool {
        collector_resample || !self.entries.contains_key(&seed)
    }

    /// Store a freshly sampled run for `seed`, reusing the entry's `Vec` storage
    /// in place (cleared + extended — no realloc on a steady-state hit). Marks the
    /// entry seen this frame so eviction keeps it.
    fn store(&mut self, seed: u32, run: &[BonePaletteEntry]) {
        let entry = self.entries.entry(seed).or_default();
        entry.run.clear();
        entry.run.extend_from_slice(run);
        entry.seen_this_frame = true;
    }

    /// The cached run for `seed` on a SKIP frame, or `None` if absent. Also marks
    /// the entry seen so a skipped instance is not evicted. (A skip only reaches
    /// here when `must_sample` already returned `false`, i.e. the entry exists.)
    fn touch_cached(&mut self, seed: u32) -> Option<&[BonePaletteEntry]> {
        let entry = self.entries.get_mut(&seed)?;
        entry.seen_this_frame = true;
        Some(entry.run.as_slice())
    }

    /// Evict entries not touched this frame and reset the per-entry seen flags for
    /// the next frame. Called once at the end of the per-frame sample/upload pass,
    /// so the cache holds exactly this frame's planned instances.
    fn end_frame(&mut self) {
        self.entries.retain(|_, e| e.seen_this_frame);
        for e in self.entries.values_mut() {
            e.seen_this_frame = false;
        }
    }

    /// Empty the cache (level-load clear).
    fn clear(&mut self) {
        self.entries.clear();
    }
}

/// A resolved clip blend source's owned parts, so a `BlendSource::Clip` can be
/// reconstructed by reference. `BlendSource` borrows the clip, so the borrow
/// must outlive the `BlendSource` — this holds the `(clip, time, loop)` and
/// hands out a fresh `BlendSource` on demand.
struct ClipBlend<'a> {
    clip: &'a AnimationClip,
    time: f32,
    loop_policy: crate::model::anim::Loop,
}

impl<'a> ClipBlend<'a> {
    fn as_blend_source(&self) -> BlendSource<'a> {
        BlendSource::Clip {
            clip: self.clip,
            time: self.time,
            loop_policy: self.loop_policy,
        }
    }
}

/// Resolve a [`ClipSample`] leg into a borrowed [`ClipBlend`], or `None` if its
/// clip index is absent from the model.
fn clip_blend_source<'a>(
    leg: &ClipSample,
    resolve_clip: &impl Fn(usize) -> Option<&'a AnimationClip>,
) -> Option<ClipBlend<'a>> {
    let clip = resolve_clip(leg.clip_index)?;
    Some(ClipBlend {
        clip,
        time: leg.time,
        loop_policy: leg.loop_policy,
    })
}

/// Sample one instance's pose into `out` per its resolved [`MeshSampleParams`]:
/// a single clip (no fade), a clip→clip blend, or a snapshot→clip blend. Always
/// writes a full run into `out` and returns `true`, so the caller's palette write
/// covers the whole region.
///
/// When the primary clip does not resolve there is no pose to sample, so `out` is
/// filled with one identity (bind-pose) matrix per joint rather than left
/// untouched. Leaving it untouched would let the caller skip the write and expose
/// whatever matrices the densely-repacked palette region last held (another
/// instance's pose) — the identity fill makes the unsampled run a clean bind pose
/// instead of inheriting a stranger's matrices.
///
/// Fade resolution mirrors the collector's intent but degrades safely at the GPU
/// seam: a [`FadeSource::Snapshot`] whose store entry is missing (capture frame
/// culled) falls back to its `(clip, time)` pair — a `"snap"`-equivalent hard
/// blend the game layer never saw.
fn sample_instance<'a>(
    sample: &MeshSampleParams,
    skeleton: &Skeleton,
    store: &SnapshotStore,
    seed: u32,
    resolve_clip: &impl Fn(usize) -> Option<&'a AnimationClip>,
    out: &mut Vec<BonePaletteEntry>,
) -> bool {
    // Primary clip must resolve; without it there is no pose to sample. Write an
    // identity (bind-pose) run so the caller's palette write overwrites any stale
    // matrices the dense repack left in this instance's region.
    let Some(primary) = clip_blend_source(&sample.primary, resolve_clip) else {
        out.clear();
        out.resize(
            skeleton.joints.len(),
            BonePaletteEntry {
                matrix: glam::Mat4::IDENTITY.to_cols_array_2d(),
            },
        );
        return true;
    };

    let Some(fade) = sample.fade else {
        // Steady state: single clip sample (the common, allocation-free path).
        crate::model::anim::sample_clip_looped(
            primary.clip,
            skeleton,
            primary.time,
            primary.loop_policy,
            out,
        );
        return true;
    };

    // A fade is active: blend `from` → `primary` at the weight. The blended
    // sampler takes weight 0 → `a` (the outgoing `from`), 1 → `b` (the entered
    // `primary`), matching the collector's weight convention.
    let primary_src = primary.as_blend_source();
    match fade.from {
        FadeSource::Clip(leg) => {
            let Some(from) = clip_blend_source(&leg, resolve_clip) else {
                // Outgoing clip gone: fall back to the primary alone.
                crate::model::anim::sample_clip_looped(
                    primary.clip,
                    skeleton,
                    primary.time,
                    primary.loop_policy,
                    out,
                );
                return true;
            };
            crate::model::anim::sample_blended(
                &from.as_blend_source(),
                &primary_src,
                fade.weight,
                skeleton,
                out,
            );
        }
        FadeSource::Snapshot { tag, fallback } => {
            match store.matching(seed, tag) {
                Some(pose) => {
                    crate::model::anim::sample_blended(
                        &BlendSource::Snapshot(pose),
                        &primary_src,
                        fade.weight,
                        skeleton,
                        out,
                    );
                }
                None => {
                    // Store miss (capture frame culled): degrade to the fallback
                    // clip — a `"snap"`-equivalent blend the game layer never saw.
                    match clip_blend_source(&fallback, resolve_clip) {
                        Some(from) => crate::model::anim::sample_blended(
                            &from.as_blend_source(),
                            &primary_src,
                            fade.weight,
                            skeleton,
                            out,
                        ),
                        None => crate::model::anim::sample_clip_looped(
                            primary.clip,
                            skeleton,
                            primary.time,
                            primary.loop_policy,
                            out,
                        ),
                    }
                }
            }
        }
    }
    true
}

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
    depth_pipeline: wgpu::RenderPipeline,

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
    instance_bind_group: wgpu::BindGroup,

    /// Group 2 BGL (dynamic direct lighting). Pinned binding map (see
    /// [`MeshPass::new`]): b0 dynamic-light records, b1 per-light influence
    /// volumes, b2 scripted-animation descriptors, b3 scripted-animation curve
    /// samples, b4 the mesh-side params uniform. b0–b3 alias the SAME
    /// renderer-owned GPU buffers forward binds; b4 is owned here. Retained so
    /// the bind group can be rebuilt on buffer reallocation (level load).
    light_bind_group_layout: wgpu::BindGroupLayout,

    /// Group 2 bind group. `None` until the renderer first calls
    /// [`MeshPass::rebuild_light_bind_group`] with the runtime light buffers, and
    /// rebuilt whenever those buffers are reallocated (level load). The forward
    /// mesh draw sets it at group 2; b0–b3 alias renderer-owned buffers, b4 is
    /// [`MeshPass::light_params_buffer`].
    light_bind_group: Option<wgpu::BindGroup>,

    /// Group 2 binding 4 params uniform (`MeshLightParams`): light count, the
    /// frame's forward `time`, and the forward `lighting_isolation` mode.
    /// Fixed-size, owned here, written per frame by
    /// [`MeshPass::write_light_params`]; rebound by reference into every rebuilt
    /// group-2 bind group.
    light_params_buffer: wgpu::Buffer,

    /// Adapter cube-array support (`DownlevelFlags::CUBE_ARRAY_TEXTURES`), threaded
    /// from `Renderer::new`. Pins the `Some`-iff-layout invariant: the group-2 BGL
    /// carries the b8 cube entry iff this is `true`, so `rebuild_light_bind_group`
    /// supplies the cube view `Some` iff this is `true`. Fixed for the renderer's
    /// lifetime — the same flag drives the pipeline's no-cube shader strip.
    cube_array_supported: bool,

    /// Uploaded models keyed by handle (the raw `MeshComponent.model` string).
    /// One entry per distinct model; mirrors `SmokePass::sheets`. The level-load
    /// level-load model sweep populates this via [`MeshPass::insert_model`].
    models: HashMap<ModelHandle, UploadedModel>,

    /// Per-model LOCAL-space AABB, keyed by handle, populated at `insert_model`
    /// from the CPU `SkinnedMesh::bounds`. Kept on the cache (not in
    /// `UploadedModel`, which stays GPU-only) so the GPU-free frame planner can
    /// stamp each `PlannedInstance` with its model's bound for the CPU per-light
    /// caster cull — the renderer's GPU draw never reads it.
    model_bounds: HashMap<ModelHandle, crate::lighting::cone_frustum::Aabb>,

    /// Per-model animation clips, keyed by handle, in glTF (authored) index
    /// order — the FULL clip set parsed from the document, not just the first.
    /// Kept on the cache beside `model_bounds` (not in `UploadedModel`, which
    /// stays GPU-only) so the clip-name / metadata query seam is testable
    /// without a `wgpu::Device`. `plan_and_upload` samples each instance by the
    /// clip indices its per-instance `MeshSampleParams` carry (the collector
    /// resolves state → clip index game-side); the name/metadata accessors read
    /// the whole list. A model with no animation maps to an empty `Vec`.
    model_clips: HashMap<ModelHandle, Vec<AnimationClip>>,

    /// Per-entity `"smooth"`-interrupt snapshot store, keyed by entity seed. A
    /// GPU-free CPU map (the `model_bounds` precedent): a capture instruction
    /// installs an entry, a planned frame without an active snapshot fade drops
    /// it, and [`MeshPass::clear_for_level_load`] empties it at level load. The
    /// frozen pose is the blend source a `"smooth"` fade resumes from with no
    /// discontinuity.
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

    /// Optional per-frame pose-sampling measurement. `Some` only when
    /// `POSTRETRO_GPU_TIMING=1` (cached at construction so the hot path never
    /// touches the environment), so the unmeasured frame pays nothing beyond an
    /// `Option` check. Accumulates the CPU cost of the per-instance `sample_clip`
    /// loop and logs it rate-limited — a profiling gate to measure per-instance
    /// pose-sampling cost at representative wave counts and decide whether a baked
    /// pose buffer is worth the complexity over per-frame CPU sampling.
    pose_sample_stats: Option<PoseSampleStats>,
}

/// CPU pose-sampling cost accumulator for the mesh pass (finding-grade, not a
/// gate). Counts the instances sampled and the wall time spent in `sample_clip`,
/// flushing a rate-limited `[Renderer]` line so the measurement does not spam the
/// hot path. Only constructed under `POSTRETRO_GPU_TIMING=1`.
///
/// Measured shape (GTX 1660 Super, debug build): one `sample_clip` over a
/// few-dozen-joint clip is ~single-digit microseconds; a 64-instance wave costs
/// ~tens of microseconds per frame — well under a frame budget, so per-instance
/// CPU sampling is not a bottleneck at the representative wave counts this task
/// targets. The shared palette buffer at `MAX_PALETTE_ENTRIES = 4096` slots is
/// 256 KiB of VRAM.
struct PoseSampleStats {
    /// Instances sampled since the last flushed log line.
    instances: u64,
    /// Accumulated `sample_clip` wall time since the last flush.
    elapsed: std::time::Duration,
    /// When the last line was logged, so the flush is interval-gated.
    last_log: std::time::Instant,
}

impl PoseSampleStats {
    /// Minimum wall-clock gap between flushed measurement lines.
    const LOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

    fn new() -> Self {
        Self {
            instances: 0,
            elapsed: std::time::Duration::ZERO,
            last_log: std::time::Instant::now(),
        }
    }

    /// Fold one frame's sampled-instance count + elapsed time in, then flush a
    /// rate-limited line and reset the running totals when the interval elapses.
    fn record_frame(&mut self, instances: u64, elapsed: std::time::Duration) {
        self.instances += instances;
        self.elapsed += elapsed;
        if self.last_log.elapsed() < Self::LOG_INTERVAL {
            return;
        }
        if self.instances > 0 {
            let per_inst_us = self.elapsed.as_secs_f64() * 1.0e6 / self.instances as f64;
            log::info!(
                "[Renderer] mesh pose sampling: {} instance-samples in {:.3} ms total \
                 ({:.2} us/instance) over the last interval",
                self.instances,
                self.elapsed.as_secs_f64() * 1.0e3,
                per_inst_us,
            );
        }
        self.instances = 0;
        self.elapsed = std::time::Duration::ZERO;
        self.last_log = std::time::Instant::now();
    }
}

impl MeshPass {
    /// Build the skinned-mesh pipelines (forward + depth-only). `camera_bgl` and
    /// `material_bgl` are the renderer-owned layouts shared with the forward pass
    /// (group 0 = camera uniform, group 1 = material). `light_space_bgl` is the
    /// renderer-owned light-space-matrix BGL (a 64-byte mat4x4 dynamic-offset
    /// uniform — the same `shadow_vs_bgl` the world spot-shadow depth pipeline
    /// uses); the depth-only pipeline binds it at group 0 so spot slots (and later
    /// cube faces) supply the per-render light-space matrix. `shadow_depth_format`
    /// is the shadow-map depth format the depth pipeline writes. Mirrors
    /// `SmokePass::new`'s shape.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        shadow_depth_format: wgpu::TextureFormat,
        camera_bgl: &wgpu::BindGroupLayout,
        material_bgl: &wgpu::BindGroupLayout,
        light_space_bgl: &wgpu::BindGroupLayout,
        sh_volume_bgl: &wgpu::BindGroupLayout,
        cube_array_supported: bool,
    ) -> Self {
        // Compose the group-2 shader source for the adapter's cube-array support:
        // the canonical source (b8 cube binding declared, `sample_point_shadow`
        // samples the cube) on a cube-capable adapter, else the `// CUBE_SHADOW_BINDING`
        // strip applied to drop the b8 declaration and neutralize
        // `sample_point_shadow`. Mirrors forward's `strip_point_shadow_cube` use.
        let mesh_source = skinned_mesh_shader_source(cube_array_supported);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Skinned Mesh Shader"),
            source: wgpu::ShaderSource::Wgsl(mesh_source.as_ref().into()),
        });

        // Group 3: shared bone palette (storage) + per-instance SSBO (storage).
        let instance_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Skinned Instance BGL"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        // Group 2: dynamic direct lighting. Binding map PINNED across both M10
        // mesh specs — b0 dynamic-light records (the renderer's `is_dynamic`-
        // filtered set, NOT the shadow-candidate set, so the lighting-tier split
        // holds by construction — plan D10: the mesh dynamic loop evaluates the
        // dynamic tier only, static-tier direct for movers is the group-4 baked
        // atlas), b1 per-light influence volumes, b2 scripted-animation
        // descriptors (forward's group-3 b13 `scripted_light_descriptors`, the
        // SAME buffer rebound here), b3 scripted-animation curve samples
        // (forward's group-3 b12 `anim_samples`, same buffer), b4 the
        // mesh-side params uniform (light count, frame time, debug gate). b5–b8
        // are the shadow-receipt bindings (spot depth, comparison sampler,
        // light-space matrices uniform, conditional cube-array depth), allocated by
        // the `mesh_light_bind_group_layout_entries` builder call below.
        //
        // Every entry is FRAGMENT-only: the mesh dynamic-light loop AND its shadow
        // sampling run in the fragment stage. This is the mesh fragment stage's
        // FIRST storage-buffer use (group 3's palette + instance SSBO are
        // VERTEX-stage), so the fragment
        // stage sits at FOUR storage buffers here — well under the per-stage ceiling
        // of 8 (rendering_pipeline.md §10). Entries come from the GPU-free
        // `mesh_light_bind_group_layout_entries` builder so the layout and the
        // `mesh_group2_bgl_matches_shader_bindings` headless test never drift.
        let light_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Skinned Mesh Light BGL (group 2)"),
                entries: &mesh_light_bind_group_layout_entries(cube_array_supported),
            });

        // Pipeline layout: group 0 (camera), 1 (material), 2 (dynamic direct
        // lighting + shadow receipt — the group-2 BGL above),
        // 3 (skinned instance data), 4 (SH irradiance volume —
        // `ShVolumeResources.mesh_bind_group_layout`, the SUPERSET layout that
        // extends the shared SH entries with the direct-atlas texture at binding
        // 15 and the `DynamicDirectParams` uniform at binding 16; forward/
        // billboard/fog passes use the smaller `bind_group_layout` without those
        // two extra bindings, so mesh binds `mesh_bind_group`, not the shared
        // `ShVolumeResources` bind group).
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Skinned Mesh Pipeline Layout"),
            bind_group_layouts: &[
                Some(camera_bgl),
                Some(material_bgl),
                Some(&light_bind_group_layout),
                Some(&instance_bind_group_layout),
                Some(sh_volume_bgl),
            ],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Skinned Mesh Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                // Vertex layout BUILT HERE from `SkinnedVertex`'s fields
                // (model/ stays wgpu-free). Offsets:
                //   position       Float32x3  @ 0
                //   base_uv        Unorm16x2  @ 12  → vec2<f32> (0..1, decoded)
                //   normal_oct     Uint16x2   @ 16
                //   tangent_packed Uint16x2   @ 20
                //   joints (u8x4)  Uint8x4    @ 24  → vec4<u32>
                //   weights (u8x4) Unorm8x4   @ 28  → vec4<f32> (0..1)
                // Stride 32. The tangent attribute is carried (committed layout)
                // but unused by the SH-lit fragment because there is no
                // normal-map pass yet; committing it now lets depth-only,
                // lighting, and normal-map passes reuse this vertex layout
                // without a format change.
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<crate::model::mesh::SkinnedVertex>()
                        as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        wgpu::VertexAttribute {
                            offset: 12,
                            shader_location: 1,
                            // base_uv is u16-quantized (gltf_loader::quantize_uv:
                            // 0..1 → 0..65535). Unorm16x2 hardware-decodes it back
                            // to vec2<f32> (0..1), matching the shader's
                            // `@location(1) base_uv: vec2<f32>` and forward.wgsl's
                            // UV convention. (Uint16x2 here surfaced as vec2<u32>
                            // and failed pipeline validation against the float UV.)
                            format: wgpu::VertexFormat::Unorm16x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 16,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 20,
                            shader_location: 3,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 24,
                            shader_location: 4,
                            format: wgpu::VertexFormat::Uint8x4,
                        },
                        wgpu::VertexAttribute {
                            offset: 28,
                            shader_location: 5,
                            format: wgpu::VertexFormat::Unorm8x4,
                        },
                    ],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // glTF front faces are CCW; engine forward pipeline matches.
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            // The mesh is NOT in the world depth pre-pass, so it depth-tests
            // (`Less`) against the world depth AND writes its own depth so it
            // self-occludes correctly. Recorded in a dedicated render pass that
            // loads the existing depth attachment writably (see render/mod.rs).
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        // --- Depth-only skinned pipeline (shadow occluders) ------------------
        // Its OWN layout: group 0 = the per-render light-space matrix BGL
        // (dynamic-offset 64-byte mat4x4, shared with the world spot-shadow
        // depth pipeline), group 3 = the SAME instance BGL as the forward pass
        // (palette + per-instance SSBO). Groups 1, 2, 4 are omitted — depth-only
        // reads no material, lighting, or SH. Forcing group 3 to index 3 keeps
        // the forward pass's group-3 bind group reusable here without re-upload.
        let depth_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Skinned Depth Shader"),
            source: wgpu::ShaderSource::Wgsl(SKINNED_DEPTH_SHADER_SOURCE.into()),
        });
        let depth_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Skinned Depth Pipeline Layout"),
                bind_group_layouts: &[
                    Some(light_space_bgl),
                    None,
                    None,
                    Some(&instance_bind_group_layout),
                ],
                immediate_size: 0,
            });
        let depth_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Skinned Depth Pipeline"),
            layout: Some(&depth_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &depth_shader,
                entry_point: Some("vs_main"),
                // Position (loc 0) + joints (loc 4) + weights (loc 5) only — the
                // color attributes are dropped. Offsets match the forward layout
                // so the SAME vertex buffer binds: joints at byte 24, weights at
                // 28; stride is the full `SkinnedVertex` (the skipped attributes
                // still occupy the stride, they are simply not declared).
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<crate::model::mesh::SkinnedVertex>()
                        as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        wgpu::VertexAttribute {
                            offset: 24,
                            shader_location: 4,
                            format: wgpu::VertexFormat::Uint8x4,
                        },
                        wgpu::VertexAttribute {
                            offset: 28,
                            shader_location: 5,
                            format: wgpu::VertexFormat::Unorm8x4,
                        },
                    ],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            // Depth-only into the shadow map: write depth, no color target, with
            // the same acne-suppressing bias the world spot-shadow pass uses.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: shadow_depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 1.5,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });

        // Shared bone-palette storage buffer, sized for the full per-frame
        // budget. Default-filled to identity (bind pose) below so an
        // un-sampled run still renders.
        let palette_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Bone Palette Buffer"),
            size: (MAX_PALETTE_ENTRIES * BONE_PALETTE_ENTRY_SIZE) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Per-instance SSBO, sized for the worst-case instance count.
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Skinned Instance Buffer"),
            size: (MAX_INSTANCES * INSTANCE_ENTRY_SIZE) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Group 3 bind group: both buffers are fixed-size and reused every
        // frame, so this is built once (mirrors `SmokePass::instance_bind_group`).
        let instance_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Skinned Instance Bind Group"),
            layout: &instance_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: palette_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: instance_buffer.as_entire_binding(),
                },
            ],
        });

        // Group 2 binding 4 params uniform (`MeshLightParams`). Fixed-size, owned
        // here, written per frame; rebound by reference into every rebuilt group-2
        // bind group. The group-2 bind group itself is left `None` until the
        // renderer calls `rebuild_light_bind_group` with the runtime light buffers
        // (after geometry installs) — the draw path skips the mesh pass when no
        // model is uploaded, so no frame draws meshes before that wiring lands.
        let light_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Skinned Mesh Light Params Uniform"),
            size: MESH_LIGHT_PARAMS_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Cache the gate once at construction so the per-frame sampling loop
        // never re-reads the environment. Same flag the GPU-timing path uses.
        let pose_sample_stats = (std::env::var("POSTRETRO_GPU_TIMING").ok().as_deref()
            == Some("1"))
        .then(PoseSampleStats::new);

        Self {
            pipeline,
            depth_pipeline,
            palette_buffer,
            instance_buffer,
            instance_bind_group,
            light_bind_group_layout,
            light_bind_group: None,
            light_params_buffer,
            cube_array_supported,
            models: HashMap::new(),
            model_bounds: HashMap::new(),
            model_clips: HashMap::new(),
            snapshot_store: SnapshotStore::default(),
            palette_cache: PaletteCache::default(),
            capture_scratch: Vec::new(),
            pose_sample_stats,
        }
    }

    /// (Re)build the group-2 dynamic-direct light bind group over the renderer's
    /// runtime light buffers. Called once after geometry installs and again on any
    /// reallocation of these buffers (level load), mirroring how the renderer
    /// rebuilds its forward `lighting_bind_group`. The buffers are owned by the
    /// renderer and bound here by reference; b4 is this pass's own
    /// `light_params_buffer`.
    ///
    /// `lights` MUST be the `is_dynamic`-FILTERED dynamic-light set (the renderer's
    /// `filter_dynamic_lights` output / `lights_buffer`), NOT the shadow-candidate
    /// set — binding the filtered set is what makes the lighting-tier split hold by
    /// construction (plan D10). `influence` is the per-light influence-volume
    /// buffer. `scripted_descriptors` is forward's group-3 b13
    /// `scripted_light_descriptors`; `anim_samples` is forward's group-3 b12
    /// `anim_samples` — the SAME GPU buffers, rebound at mesh group 2 b2/b3.
    ///
    /// b5–b8 are the SHADOW-RECEIPT bindings, on a mesh-specific layout that
    /// OMITS forward's SDF-factor + scene-depth entries the mesh must not sample.
    /// They alias the SAME pool-owned GPU resources the forward pass binds in its
    /// group 5 (NOT forward's group-5 BGL):
    /// `spot_shadow_depth` is the spot pool's D2-array `array_view` (b5),
    /// `spot_shadow_compare` is the pool's comparison sampler (b6),
    /// `light_space_matrices` is the pool's `matrices_buffer` UNIFORM (b7), and
    /// `point_shadow_cube` is the cube pool's `CubeArray` `sampling_view` (b8).
    ///
    /// `point_shadow_cube` MUST be `Some` IFF the layout carries the b8 entry — i.e.
    /// iff `self.cube_array_supported` (the `Some`-iff-layout invariant). Passing
    /// `Some` on a no-cube layout (or `None` on a cube layout) is a bind-group /
    /// layout mismatch wgpu rejects; the assert below pins the invariant before the
    /// GPU sees it. The pool resources are stable for the renderer's lifetime (the
    /// pools are built once in `Renderer::new` and never recreated — not on resize,
    /// not on level load), so these b5–b8 references only ever rebind here alongside
    /// the b0–b4 reallocation rebind on level load.
    #[allow(clippy::too_many_arguments)]
    pub fn rebuild_light_bind_group(
        &mut self,
        device: &wgpu::Device,
        lights: &wgpu::Buffer,
        influence: &wgpu::Buffer,
        scripted_descriptors: &wgpu::Buffer,
        anim_samples: &wgpu::Buffer,
        spot_shadow_depth: &wgpu::TextureView,
        spot_shadow_compare: &wgpu::Sampler,
        light_space_matrices: &wgpu::Buffer,
        point_shadow_cube: Option<&wgpu::TextureView>,
    ) {
        assert_eq!(
            point_shadow_cube.is_some(),
            self.cube_array_supported,
            "mesh group-2 cube view must be Some iff the BGL carries the b8 cube \
             entry (cube_array_supported) — the Some-iff-layout invariant",
        );
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: lights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: influence.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: scripted_descriptors.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: anim_samples.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: self.light_params_buffer.as_entire_binding(),
            },
            // b5: spot shadow depth 2D-array (pool `array_view`).
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(spot_shadow_depth),
            },
            // b6: comparison sampler (pool `compare_sampler`).
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(spot_shadow_compare),
            },
            // b7: light-space matrices uniform (pool `matrices_buffer`).
            wgpu::BindGroupEntry {
                binding: 7,
                resource: light_space_matrices.as_entire_binding(),
            },
        ];
        // b8: cube-array depth — present IFF the BGL carries it (cube support).
        if let Some(cube_view) = point_shadow_cube {
            entries.push(wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(cube_view),
            });
        }
        self.light_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Skinned Mesh Light Bind Group (group 2)"),
            layout: &self.light_bind_group_layout,
            entries: &entries,
        }));
    }

    /// Write this frame's group-2 params uniform (binding 4): the dynamic-light
    /// `light_count`, the frame's render-clock `time`, and `lighting_isolation`.
    /// `time` MUST be the SAME value the renderer wrote to forward `Uniforms.time`
    /// this frame (the renderer caches it in `update_per_frame_uniforms` and
    /// threads it here), so the scripted-light curves the mesh loop evaluates stay
    /// phase-coherent with the forward pass. `lighting_isolation` MUST be the SAME
    /// `LightingIsolation as u32` the renderer writes to forward
    /// `Uniforms.lighting_isolation`, so the mesh dynamic-direct term is gated by
    /// the lighting-isolation debug modes exactly as the world dynamic term is.
    /// `ambient_floor` MUST be the SAME value the renderer writes to forward
    /// `Uniforms.ambient_floor` this frame, so shadowed mesh faces lift with the
    /// diagnostics ambient-floor slider exactly as world surfaces do.
    pub fn write_light_params(
        &self,
        queue: &wgpu::Queue,
        light_count: u32,
        time: f32,
        lighting_isolation: u32,
        ambient_floor: f32,
    ) {
        let bytes = build_light_params_bytes(MeshLightParams {
            light_count,
            time,
            lighting_isolation,
            ambient_floor,
        });
        queue.write_buffer(&self.light_params_buffer, 0, &bytes);
    }

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
    pub fn insert_model(
        &mut self,
        device: &wgpu::Device,
        handle: ModelHandle,
        mesh: &SkinnedMesh,
        submeshes: Vec<(wgpu::BindGroup, std::ops::Range<u32>)>,
        skeleton: Skeleton,
        clips: Vec<AnimationClip>,
    ) {
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
        // Stash the CPU-side local bound for the planner (drives the per-light
        // caster cull). Lives on the cache, NOT in `UploadedModel` — the GPU draw
        // never reads it.
        self.model_bounds.insert(handle.clone(), mesh.bounds);
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
    /// `mesh_visible` / `mesh_visible_in_leaf` split).
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

    /// Initialize the shared bone palette to identity (bind pose) before the
    /// first sampled frame, so any un-sampled run renders in bind pose rather
    /// than reading uninitialized buffer memory.
    pub fn upload_identity_palette(&self, queue: &wgpu::Queue) {
        let identity = BonePaletteEntry {
            matrix: glam::Mat4::IDENTITY.to_cols_array_2d(),
        };
        let entries = vec![identity; MAX_PALETTE_ENTRIES];
        queue.write_buffer(&self.palette_buffer, 0, bytemuck::cast_slice(&entries));
    }

    /// Plan-sample-upload step: write this frame's per-instance SSBO entries and
    /// sample every instance's clip into its bone-palette run. NO draws recorded.
    ///
    /// This is the pose/upload HOIST: the renderer runs it AFTER
    /// `update_dynamic_light_slots` and BEFORE the spot-shadow depth loop, so the
    /// skinned-depth pass (shadow occluders) and the forward mesh draw both read
    /// the SAME already-posed `palette_buffer`/`instance_buffer`. Nothing rewrites
    /// these buffers between the shadow loop and the forward draw, so there is no
    /// one-frame pose lag between an entity and its shadow.
    ///
    /// For each planned instance: pack its SSBO entry (model matrix + palette
    /// base), evaluate any one-time snapshot-capture instruction into the
    /// per-entity snapshot store, then sample its pose into the palette at that
    /// base per the instance's resolved [`MeshSampleParams`] — a single clip
    /// ([`crate::model::anim::sample_clip_looped`]), a clip→clip blend, or a
    /// snapshot→clip blend. All sample times arrive in the params (the collector
    /// computed them from the animation clock), so the pass holds no render-clock
    /// of its own. The optional pose-sampling measurement uses an `Instant`, not
    /// the render clock.
    ///
    /// Snapshot-store lifecycle: a capture installs/refreshes an entry (idempotent
    /// by tag); an instance whose fade is NOT an active matching snapshot fade
    /// drops its entry (fade over, or a replacement-fade tag mismatch). A
    /// snapshot fade whose store entry is missing (its capture frame was culled /
    /// budget-dropped) degrades to the fallback clip — a discontinuity no one saw
    /// because the entity was not drawn at the interrupt instant.
    ///
    /// Cull is the caller's job — see [`mesh_visible`]; the plan already holds
    /// only surviving, in-budget instances.
    pub fn plan_and_upload(
        &mut self,
        queue: &wgpu::Queue,
        plan: &MeshFramePlan,
        scratch: &mut Vec<BonePaletteEntry>,
    ) {
        if plan.groups.is_empty() {
            return;
        }

        // Disjoint field borrows: the capture step mutates `snapshot_store` +
        // `capture_scratch` while reading `model_clips`/`models`; the sample step
        // reads `snapshot_store`. Destructuring lets the borrow checker see they
        // are distinct fields (a `self.method` call would borrow all of `self`).
        let Self {
            models,
            model_clips,
            snapshot_store,
            palette_cache,
            capture_scratch,
            instance_buffer,
            palette_buffer,
            pose_sample_stats,
            ..
        } = self;

        let measure = pose_sample_stats.is_some();
        let mut sampled_instances: u64 = 0;
        let mut sample_elapsed = std::time::Duration::ZERO;

        for group in &plan.groups {
            let Some(model) = models.get(&group.model) else {
                // Planner only emits groups for cached models, but guard anyway.
                continue;
            };
            let clips = model_clips.get(&group.model);
            let resolve_clip = |idx: usize| clips.and_then(|c| c.get(idx));

            for (i, inst) in group.instances.iter().enumerate() {
                let instance_index = group.instance_offset as usize + i;
                let entry = build_instance_entry(inst.transform, inst.palette_base);
                queue.write_buffer(
                    instance_buffer,
                    (instance_index * INSTANCE_ENTRY_SIZE) as u64,
                    &entry,
                );

                // Evaluate the one-time `"smooth"` capture (if any) into the store
                // BEFORE sampling, so this frame's snapshot fade resolves against
                // it. Idempotent by tag — a re-emission evaluates nothing.
                if let Some(capture) = &inst.capture {
                    snapshot_store.apply_capture(
                        capture,
                        &model.skeleton,
                        resolve_clip,
                        capture_scratch,
                    );
                }

                // Lifecycle: drop this entity's store entry unless its fade is an
                // active snapshot fade whose tag matches a stored entry. The
                // capture above just installed the matching entry on a capture
                // frame, so an in-progress smooth fade survives; a finished or
                // clip/snap fade clears it.
                let keep_snapshot = matches!(
                    inst.sample.fade.map(|f| f.from),
                    Some(FadeSource::Snapshot { tag, .. })
                        if snapshot_store.matching(inst.phase_seed, tag).is_some()
                );
                if !keep_snapshot {
                    snapshot_store.drop_entry(inst.phase_seed);
                }

                // Time-slicing decision. Sample when the collector asked
                // for a resample OR the cache misses (a re-entering instance with
                // no cached run must sample, never show a stale pose). Otherwise
                // re-upload the cached run with no sampling.
                if palette_cache.must_sample(inst.phase_seed, inst.resample) {
                    // RESAMPLE: sample this instance's pose, upload it, and refresh
                    // the cache with the freshly sampled run.
                    let started = measure.then(std::time::Instant::now);
                    let sampled = sample_instance(
                        &inst.sample,
                        &model.skeleton,
                        snapshot_store,
                        inst.phase_seed,
                        &resolve_clip,
                        scratch,
                    );
                    if let Some(started) = started {
                        sampled_instances += 1;
                        sample_elapsed += started.elapsed();
                    }
                    if sampled && !scratch.is_empty() {
                        queue.write_buffer(
                            palette_buffer,
                            inst.palette_base as u64 * BONE_PALETTE_ENTRY_SIZE as u64,
                            bytemuck::cast_slice(scratch),
                        );
                        // Refresh the cache so a future skipped frame re-uploads
                        // THIS pose. Reuses the entry's `Vec` storage in place.
                        palette_cache.store(inst.phase_seed, scratch);
                    }
                } else if let Some(cached) = palette_cache.touch_cached(inst.phase_seed) {
                    // SKIP: re-upload the cached run at this frame's palette base
                    // (the base can move frame to frame as the dense plan repacks).
                    // No sampling, no allocation.
                    if !cached.is_empty() {
                        queue.write_buffer(
                            palette_buffer,
                            inst.palette_base as u64 * BONE_PALETTE_ENTRY_SIZE as u64,
                            bytemuck::cast_slice(cached),
                        );
                    }
                }
            }
        }

        // Evict cache entries not touched this frame, so the cache holds exactly
        // this frame's planned instances (bounded by MAX_INSTANCES / the palette
        // budget) and a culled-out entity's stale run does not linger.
        palette_cache.end_frame();

        // Fold this frame's pose-sampling tallies in and flush the rate-limited
        // line when the interval elapses. Only `Some` under POSTRETRO_GPU_TIMING.
        if let Some(stats) = pose_sample_stats.as_mut() {
            stats.record_frame(sampled_instances, sample_elapsed);
        }
    }

    /// Record the forward skinned-mesh draws from the already-uploaded buffers.
    ///
    /// Must run AFTER [`MeshPass::plan_and_upload`] has populated the palette +
    /// instance buffers for this `plan` — this method records draws only, it does
    /// NOT touch the buffers, so the data it draws is the identical posed data the
    /// shadow loop read. One instanced `draw_indexed` per model per submesh.
    ///
    /// Group 0 (camera) and group 4 (SH irradiance volume) must be set by the
    /// caller before recording — the renderer owns those bind groups (camera is
    /// shared across passes; SH uses the mesh-superset `mesh_bind_group`).
    ///
    /// The plan can carry OFF-PVS shadow casters (`forward_visible == false`)
    /// alongside the camera-visible set — they share the posed buffers so the
    /// shadow depth passes draw them, but the FORWARD pass must NOT (they are
    /// outside the camera's portal PVS; drawing them would violate the
    /// portal-visibility contract). This pass draws only the `forward_visible`
    /// instances, batching contiguous visible runs into one instanced draw so the
    /// common all-visible frame still issues one draw per group/submesh.
    pub fn record_draws(&self, pass: &mut wgpu::RenderPass<'_>, plan: &MeshFramePlan) {
        if plan.groups.is_empty() {
            return;
        }

        pass.set_pipeline(&self.pipeline);
        // Group 2 (dynamic direct lighting): the runtime light buffers + the
        // per-frame params uniform. Set once for the frame. The pipeline layout
        // declares group 2, so the bind group MUST be present before any mesh
        // draw — the renderer wires it (`rebuild_light_bind_group`) once geometry
        // installs, and the draw path is skipped until a model is uploaded, so
        // this is `Some` on every frame a mesh actually draws. The expect guards
        // against a future caller reordering that wiring after the draw.
        let light_bind_group = self
            .light_bind_group
            .as_ref()
            .expect("mesh group-2 light bind group must be built before recording mesh draws");
        pass.set_bind_group(2, light_bind_group, &[]);
        // Group 3 (palette + instance SSBO) is shared across every group/submesh
        // this frame — set once. The shader selects each instance's run via
        // `@builtin(instance_index)` against the densely-packed SSBO.
        pass.set_bind_group(3, &self.instance_bind_group, &[]);

        for group in &plan.groups {
            let Some(model) = self.models.get(&group.model) else {
                continue;
            };
            if model.submeshes.is_empty() {
                continue;
            }

            pass.set_vertex_buffer(0, model.vertex_buffer.slice(..));
            pass.set_index_buffer(model.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            // Draw each maximal contiguous run of `forward_visible` instances as
            // one instanced draw. Off-PVS shadow casters (`forward_visible ==
            // false`) interleave the dense SSBO, so a run breaks at each one; the
            // common all-visible group collapses to a single run (one draw per
            // submesh, identical to before this two-tier split). The base instance
            // is the run's absolute SSBO offset — the palette base still rides each
            // SSBO entry, never `first_instance` (DX12 reads it as 0,
            // gfx-rs/wgpu#2471), addressed by `@builtin(instance_index)`.
            let mut run_start: Option<u32> = None;
            for (i, inst) in group.instances.iter().enumerate() {
                let abs = group.instance_offset + i as u32;
                if inst.forward_visible {
                    run_start.get_or_insert(abs);
                } else if let Some(start) = run_start.take() {
                    draw_forward_run(pass, model, start..abs);
                }
            }
            if let Some(start) = run_start.take() {
                let end = group.instance_offset + group.instances.len() as u32;
                draw_forward_run(pass, model, start..end);
            }
        }
    }

    /// Record skinned ENTITY occluders into a shadow map through the
    /// parameterized depth-only path, culled per-slot by the slot's cone frustum.
    /// `light_space_bind_group` + `dynamic_offset` select the per-render
    /// light-space matrix at group 0 (the spot path passes the renderer's
    /// `shadow_vs_bind_group` and the per-slot offset; a cube path would pass a
    /// per-face uniform) — nothing here assumes one slot per light or a 2D target,
    /// proving the cube-ready contract.
    ///
    /// `cone_planes` are the slot's 6 cone-frustum planes (from the slot's
    /// light-space matrix). Each planned instance's local bound is transformed by
    /// its world matrix and tested against the cone; only intersecting instances
    /// are drawn into the slot. Entities are not in the world BVH, so this cull is
    /// per-instance CPU (distinct from the GPU world cull). Returns the count of
    /// instances actually submitted into this slot, so the caller can tally the
    /// per-frame submitted-occluder counter that verifies the out-of-cone
    /// acceptance criterion — no GPU readback.
    ///
    /// The caller owns the target view (it begins the render pass against the
    /// slot's depth attachment) and supplies the light-space matrix via the bind
    /// group; this method binds the depth pipeline + the SHARED group-3 instance
    /// data and records the draws from the SAME palette/instance buffers
    /// [`MeshPass::plan_and_upload`] populated. No per-frame buffer writes here —
    /// it reads the already-posed data.
    ///
    /// Surviving instances are drawn as per-instance `draw_indexed` calls
    /// (`instance_index..+1`) because the cone cull selects an arbitrary subset of
    /// each group's contiguous SSBO range; wave counts are small (a few dozen), so
    /// per-instance draws stay cheap. The base instance is the absolute index into
    /// the dense SSBO, so `@builtin(instance_index)` selects this occluder's entry —
    /// the SAME `first_instance`-borne addressing the forward path uses, with the
    /// SAME documented DX12 exposure (gfx-rs/wgpu#2471). See the per-draw comment at
    /// the `draw_indexed` site below; the per-instance palette base still travels in
    /// the SSBO entry, never in `first_instance`.
    pub fn record_skinned_depth(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        plan: &MeshFramePlan,
        light_space_bind_group: &wgpu::BindGroup,
        dynamic_offset: u32,
        cone_planes: &[glam::Vec4; 6],
    ) -> u32 {
        if plan.groups.is_empty() {
            return 0;
        }

        pass.set_pipeline(&self.depth_pipeline);
        pass.set_bind_group(0, light_space_bind_group, &[dynamic_offset]);
        // Same shared group-3 instance data as the forward pass — the depth
        // layout forces it to index 3 so the bind group is reusable verbatim.
        pass.set_bind_group(3, &self.instance_bind_group, &[]);

        let mut submitted: u32 = 0;
        for group in &plan.groups {
            let Some(model) = self.models.get(&group.model) else {
                continue;
            };
            if model.submeshes.is_empty() {
                continue;
            }
            pass.set_vertex_buffer(0, model.vertex_buffer.slice(..));
            pass.set_index_buffer(model.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for (i, inst) in group.instances.iter().enumerate() {
                // Per-light caster cull: skip instances whose transformed bound
                // does not intersect this slot's cone. An enemy outside the cone
                // is not drawn into the slot.
                if !instance_casts_into_cone(inst, cone_planes) {
                    continue;
                }
                let instance_index = group.instance_offset + i as u32;
                let instance_range = instance_index..instance_index + 1;
                // The draw's `first_instance` is the absolute SSBO index, so the
                // shader reads `instances[instance_index]` / `bone_palette[base]`
                // for THIS occluder via `@builtin(instance_index)`. This shares the
                // forward path's `@builtin(instance_index)` assumption (record_draws
                // above, file header §"Per-instance addressing"): the SSBO ENTRY is
                // selected through `first_instance`, and a backend that zeroes it
                // (the documented DX12 quirk, gfx-rs/wgpu#2471 — we do not assume
                // `INDIRECT_FIRST_INSTANCE`) would read entry 0 for every occluder,
                // projecting all of them with the first instance's pose. Known DX12
                // exposure, correct on Vulkan/Metal; it is NOT unique to the depth
                // path — both paths route the entry index through `first_instance`
                // identically, so a future DX12-robust fix (per-instance index via a
                // vertex-stepped buffer or per-draw dynamic offset) must change both
                // in lock-step, not just here. Only the per-instance palette BASE
                // (`base_and_pad.x`) is kept out of `first_instance` today.
                // Depth-only: one draw per submesh range, no material bind (the
                // depth layout omits group 1).
                for (_material_bind_group, indices) in &model.submeshes {
                    if indices.is_empty() {
                        continue;
                    }
                    pass.draw_indexed(indices.clone(), 0, instance_range.clone());
                }
                submitted += 1;
            }
        }
        submitted
    }
}

/// Record one instanced forward draw per submesh over a contiguous `range` of
/// `forward_visible` instances. Group 1 (material) is set per submesh; group 3
/// (palette + instance SSBO) is already bound once by the caller for the frame.
/// Shared by the contiguous-run loop in [`MeshPass::record_draws`].
fn draw_forward_run(
    pass: &mut wgpu::RenderPass<'_>,
    model: &UploadedModel,
    range: std::ops::Range<u32>,
) {
    if range.is_empty() {
        return;
    }
    for (material_bind_group, indices) in &model.submeshes {
        if indices.is_empty() {
            continue;
        }
        pass.set_bind_group(1, material_bind_group, &[]);
        pass.draw_indexed(indices.clone(), 0, range.clone());
    }
}

/// Joint-count lookup over the model cache, so the GPU-free frame planner
/// (`mesh_instances::plan_mesh_frame`) can assign palette runs without a wgpu
/// reference. Returns `None` for an un-uploaded handle (its instances are
/// skipped, not budget-dropped).
impl JointCounts for MeshPass {
    fn joint_count(&self, model: &ModelHandle) -> Option<u32> {
        self.models
            .get(model)
            .map(|m| m.skeleton.joints.len() as u32)
    }

    fn model_bounds(&self, model: &ModelHandle) -> crate::lighting::cone_frustum::Aabb {
        self.model_bounds.get(model).copied().unwrap_or_default()
    }
}

/// Pure cull decision for one skinned-mesh instance — GPU-free, unit-testable.
///
/// An instance draws iff the visible set is `DrawAll`, or the BSP leaf its
/// position lands in (cell id == leaf index in the current compiler) is a member
/// of the visible cell set. Mirrors the world path's membership test
/// (`cells.contains(&(find_leaf(pos) as u32))`).
///
/// The render-frame mesh collector (`scripting/systems/mesh_render.rs`) calls
/// this (it holds the `LevelWorld` + the frame's `VisibleCells`) before pushing
/// an instance into the draw list, so the renderer's GPU pass never needs a
/// world reference. The cull tests the entity's CURRENT-TICK transform (stable
/// per-tick visibility), not the sub-tick interpolated position. The `find_leaf`
/// lookup and the membership decision are split so the decision is unit-testable
/// without constructing a full `LevelWorld` (see [`mesh_visible_in_leaf`]).
pub fn mesh_visible(world: &LevelWorld, visible: &VisibleCells, pos: glam::Vec3) -> bool {
    // `DrawAll` short-circuits before the leaf lookup: every instance draws, so
    // the (non-trivial) `find_leaf` BSP descent is pure waste on that path.
    let VisibleCells::Culled(_) = visible else {
        return true;
    };
    let leaf = world.find_leaf(pos) as u32;
    mesh_visible_in_leaf(visible, leaf)
}

/// Membership half of the cull decision: does `leaf_id` draw given `visible`?
/// `DrawAll` always draws; otherwise the leaf must be in the visible cell set
/// (cell id == leaf index). Pure data logic — no world, no GPU. Consumed by
/// `mesh_visible` (the collector path) and the cull unit tests.
pub fn mesh_visible_in_leaf(visible: &VisibleCells, leaf_id: u32) -> bool {
    match visible {
        VisibleCells::DrawAll => true,
        VisibleCells::Culled(cells) => cells.contains(&leaf_id),
    }
}

/// Look up an animation clip by authored `name` in a model-clip map. Pure data
/// logic — no GPU, no `MeshPass`. Backs [`MeshPass::model_clip_by_name`] and is
/// split out (the `model_bounds` / `mesh_visible_in_leaf` precedent) so the
/// clip-name query seam is testable without `MeshPass::new`, which needs a
/// `wgpu::Device`.
///
/// Returns `None` when `handle` is not in the map or its clip list holds no clip
/// of that name — absence is normal, never an error or panic. **First match
/// wins:** glTF does not forbid duplicate animation names, and clips are stored
/// in authored (glTF index) order, so the earliest authored clip with the name
/// is returned.
#[cfg_attr(not(test), allow(dead_code))]
fn clip_by_name<'a>(
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
fn clip_metadata(
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

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    // The cull AC is verified against a SYNTHETIC visible-set (the plan permits
    // this in lieu of a full world / closed-portal arrangement). `mesh_visible`
    // = `find_leaf` (covered by `prl.rs`'s own `find_leaf` tests) composed with
    // the membership decision below, so testing the decision pins the cull
    // behavior without constructing a heavyweight `LevelWorld`.

    #[test]
    fn mesh_cull_excludes_instance_in_nonvisible_cell() {
        // Instance lands in leaf 1; the visible set holds only leaf 0.
        let visible = VisibleCells::Culled(vec![0]);
        assert!(
            !mesh_visible_in_leaf(&visible, 1),
            "instance in leaf 1 must be culled when only leaf 0 is visible",
        );
    }

    #[test]
    fn mesh_cull_includes_instance_in_visible_cell() {
        let visible = VisibleCells::Culled(vec![0, 1]);
        assert!(
            mesh_visible_in_leaf(&visible, 1),
            "instance in leaf 1 must draw when leaf 1 is visible",
        );
    }

    #[test]
    fn mesh_cull_includes_instance_on_draw_all() {
        // DrawAll always draws regardless of the instance's leaf.
        assert!(mesh_visible_in_leaf(&VisibleCells::DrawAll, 1));
        assert!(mesh_visible_in_leaf(&VisibleCells::DrawAll, 999));
    }

    #[test]
    fn skinned_mesh_wgsl_parses() {
        let module = naga::front::wgsl::parse_str(SKINNED_MESH_SHADER_SOURCE)
            .expect("skinned_mesh.wgsl should parse as WGSL");
        let has_vs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "vs_main" && ep.stage == naga::ShaderStage::Vertex);
        let has_fs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "fs_main" && ep.stage == naga::ShaderStage::Fragment);
        assert!(has_vs, "skinned_mesh.wgsl must export @vertex vs_main");
        assert!(has_fs, "skinned_mesh.wgsl must export @fragment fs_main");
    }

    #[test]
    fn skinned_mesh_wgsl_passes_naga_validation() {
        let module = naga::front::wgsl::parse_str(SKINNED_MESH_SHADER_SOURCE)
            .expect("skinned_mesh.wgsl must parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("skinned_mesh.wgsl must pass naga validation");
    }

    #[test]
    fn skinned_depth_wgsl_parses_and_is_vertex_only() {
        // The depth-only skinned shader must parse, export `@vertex vs_main`, and
        // carry NO fragment stage (depth-only) — mirroring depth_prepass.wgsl's
        // relationship to forward.wgsl.
        let module = naga::front::wgsl::parse_str(SKINNED_DEPTH_SHADER_SOURCE)
            .expect("skinned_depth.wgsl should parse as WGSL");
        let has_vs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "vs_main" && ep.stage == naga::ShaderStage::Vertex);
        assert!(has_vs, "skinned_depth.wgsl must export @vertex vs_main");
        let has_fs = module
            .entry_points
            .iter()
            .any(|ep| ep.stage == naga::ShaderStage::Fragment);
        assert!(
            !has_fs,
            "skinned_depth.wgsl is depth-only — it must declare no fragment stage"
        );
    }

    #[test]
    fn skinned_depth_wgsl_passes_naga_validation() {
        let module = naga::front::wgsl::parse_str(SKINNED_DEPTH_SHADER_SOURCE)
            .expect("skinned_depth.wgsl must parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("skinned_depth.wgsl must pass naga validation");
    }

    /// The `skin_matrix` function is duplicated verbatim from `skinned_mesh.wgsl`
    /// into `skinned_depth.wgsl` because WGSL cannot share a function that reads
    /// module-scope buffers across two separate shader sources. This test extracts
    /// the function body from both shaders and asserts byte-identical equality,
    /// so any divergence between the forward-pass and depth-pass copies fails CI
    /// rather than only mis-skinning shadows at runtime.
    #[test]
    fn skin_matrix_body_matches_across_skinned_shaders() {
        // Extract `fn skin_matrix(` … matching `}` by brace counting. Returns the
        // slice from the `fn` keyword through the closing brace (inclusive).
        fn extract_skin_matrix(src: &str) -> &str {
            let marker = "fn skin_matrix(";
            let fn_start = src
                .find(marker)
                .expect("shader must declare fn skin_matrix(");
            // Find the opening `{` of the function body.
            let body_open = fn_start
                + src[fn_start..]
                    .find('{')
                    .expect("skin_matrix must have an opening brace");
            // Walk forward, counting braces to find the matching close.
            let mut depth = 0usize;
            let mut close = body_open;
            for (i, ch) in src[body_open..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            close = body_open + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            &src[fn_start..=close]
        }

        // `SKINNED_MESH_SHADER_SOURCE` is a concat of skinned_mesh.wgsl + sh_sample.wgsl.
        // `skin_matrix` is declared in the skinned_mesh.wgsl portion.
        // `SKINNED_DEPTH_SHADER_SOURCE` is skinned_depth.wgsl directly.
        let mesh_body = extract_skin_matrix(SKINNED_MESH_SHADER_SOURCE);
        let depth_body = extract_skin_matrix(SKINNED_DEPTH_SHADER_SOURCE);

        assert_eq!(
            mesh_body, depth_body,
            "skin_matrix body in skinned_depth.wgsl must be byte-identical to the copy \
             in skinned_mesh.wgsl — update both when changing the skinning kernel",
        );
    }

    #[test]
    fn instance_entry_packs_model_and_base_index() {
        // Guard the WGSL layout contract: Instance { model: mat4x4<f32>,
        // base_and_pad: vec4<u32> } — model at offset 0 (64 B), base_index at
        // offset 64 (first u32 of the trailing vec4), total 80 B. If either side
        // (Rust packer or WGSL struct) is edited silently, this assertion fires.
        assert_eq!(
            INSTANCE_ENTRY_SIZE, 80,
            "INSTANCE_ENTRY_SIZE must match WGSL Instance total (80 B)",
        );

        let m = glam::Mat4::from_translation(Vec3::new(4.0, 5.0, 6.0));
        let bytes = build_instance_entry(m, 7);
        assert_eq!(bytes.len(), 80);

        // Model matrix occupies bytes 0..64 (column-major f32x16).
        // Verify a known column: col 0 = (1,0,0,0) for a pure-translation matrix.
        let col0_x = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        assert_eq!(col0_x, 1.0, "model matrix col 0 x must be 1.0 at offset 0");

        // Translation lands in the 4th column (offsets 48,52,56 for x,y,z).
        let tx = f32::from_ne_bytes(bytes[48..52].try_into().unwrap());
        let ty = f32::from_ne_bytes(bytes[52..56].try_into().unwrap());
        let tz = f32::from_ne_bytes(bytes[56..60].try_into().unwrap());
        assert_eq!([tx, ty, tz], [4.0, 5.0, 6.0]);

        // base_index at byte 64 (first u32 of base_and_pad vec4).
        let base = u32::from_ne_bytes(bytes[64..68].try_into().unwrap());
        assert_eq!(base, 7, "base_index must be packed at byte offset 64");

        // Padding bytes 68..80 must be zero.
        assert_eq!(
            &bytes[68..80],
            &[0u8; 12],
            "padding bytes 68..80 must be zero"
        );
    }

    // Guard the group-2 params uniform layout contract: `MeshLightParams`
    // { light_count: u32, time: f32, lighting_isolation: u32, ambient_floor: f32 }
    // — 16 B std140, mirrored by the WGSL `MeshLightParams` struct at group 2
    // binding 4. The mesh dynamic-light loop reads `time` for scripted-curve phase,
    // `lighting_isolation` for the forward-matching debug gate, and `ambient_floor`
    // for the constant additive fill, so a silent layout edit on either side must
    // fail here.
    #[test]
    fn mesh_light_params_is_sixteen_bytes() {
        assert_eq!(
            MESH_LIGHT_PARAMS_SIZE, 16,
            "MeshLightParams must be 16 B to match the std140 WGSL uniform",
        );
    }

    // Byte-layout guard for the group-2 params serialization: `ambient_floor` is
    // the 4th word and MUST land at bytes 12..16 (matching the WGSL struct offset),
    // so the diagnostics ambient-floor slider reaches the mesh shader. Mirrors the
    // forward `ambient_floor` byte-offset precedent in render/mod.rs. Exact
    // `f32::to_le_bytes` comparison — a dropped/reordered field fails here.
    #[test]
    fn write_light_params_places_ambient_floor_at_bytes_twelve_to_sixteen() {
        let ambient_floor = 0.375_f32;
        let bytes = build_light_params_bytes(MeshLightParams {
            light_count: 3,
            time: 1.5,
            lighting_isolation: 8,
            ambient_floor,
        });
        assert_eq!(bytes.len(), 16, "serialized MeshLightParams must be 16 B");
        assert_eq!(
            &bytes[12..16],
            &ambient_floor.to_le_bytes(),
            "ambient_floor must serialize at bytes 12..16 (4th word)",
        );
        // The leading three words must be undisturbed by the new field.
        assert_eq!(&bytes[0..4], &3u32.to_le_bytes(), "light_count at 0..4");
        assert_eq!(&bytes[4..8], &1.5f32.to_le_bytes(), "time at 4..8");
        assert_eq!(
            &bytes[8..12],
            &8u32.to_le_bytes(),
            "lighting_isolation at 8..12",
        );
    }

    // Headless guard for the mesh group-2 BGL: the entries the pipeline composes
    // from must match the shader's declared group-2 binding map (b0–b8) and stay
    // within the per-stage fragment storage-buffer budget. Modeled on
    // `billboard_pipeline_vertex_storage_request_matches_bgl_definitions` — both
    // re-derive the count from the SAME GPU-free BGL builder the layout is built
    // from, so a drift fails CI before a real GPU rejects the pipeline.
    #[test]
    fn mesh_group2_bgl_matches_shader_bindings() {
        // Cube-supported variant carries the full b0..=b8 map; the dynamic-direct
        // half (b0–b4) is identical in both variants, so assert it here against the
        // cube variant and cover the cube-vs-no-cube b5–b8 split in the dedicated
        // `mesh_group2_shadow_bindings_match_both_cube_variants` test.
        let entries = mesh_light_bind_group_layout_entries(true);

        // Binding map: b0–b3 read-only storage buffers, b4 a uniform. Mirrors the
        // `@group(2) @binding(N)` declarations in skinned_mesh.wgsl exactly.
        let bindings: Vec<u32> = entries.iter().map(|e| e.binding).collect();
        assert_eq!(
            bindings,
            vec![0, 1, 2, 3, 4, 5, 6, 7, 8],
            "cube-supported mesh group-2 BGL must declare bindings b0..=b8 in order",
        );
        for b in 0..4u32 {
            assert!(
                matches!(
                    entries[b as usize].ty,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        ..
                    }
                ),
                "mesh group-2 b{b} must be a read-only storage buffer",
            );
        }
        assert!(
            matches!(
                entries[4].ty,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    ..
                }
            ),
            "mesh group-2 b4 must be the params uniform",
        );

        // Every entry is FRAGMENT-only — the mesh dynamic loop AND the shadow
        // sampling are fragment-stage, and no entry should carry VERTEX/COMPUTE it
        // does not read (the over-broad-visibility trap that spends a per-stage slot
        // for free).
        for e in &entries {
            assert_eq!(
                e.visibility,
                wgpu::ShaderStages::FRAGMENT,
                "mesh group-2 b{} must be FRAGMENT-only",
                e.binding,
            );
        }

        // Per-stage storage budget: four fragment-visible storage buffers (b0–b3);
        // the uniforms (b4 params, b7 light-space matrices) and the shadow textures/
        // sampler (b5/b6/b8) do not count. 8 is the downlevel/WebGPU-default ceiling
        // for `max_storage_buffers_per_shader_stage` (rendering_pipeline.md §10).
        let frag_storage = fragment_storage_buffers(&entries);
        assert_eq!(
            frag_storage, 4,
            "mesh group-2 must contribute exactly four fragment-visible storage buffers",
        );
        assert!(
            frag_storage <= 8,
            "mesh group-2 fragment-visible storage-buffer count ({frag_storage}) exceeds the \
             downlevel-default max_storage_buffers_per_shader_stage of 8",
        );
    }

    // CONTRACT-DOC PIN (not a behavioral test): the lighting-tier split — mesh
    // group-2 b0 carries the renderer's `filter_dynamic_lights` output (the
    // `is_dynamic`-filtered set), so static lights are excluded BY CONSTRUCTION —
    // lives in the actual bind-group wiring (`rebuild_light_bind_group`), which
    // takes a `lights` slice and cannot be exercised without a GPU. This test does
    // NOT verify that wiring; it is a string pin that keeps the DOCUMENTED contract
    // present and self-consistent: the shader's b0 declaration and the
    // `rebuild_light_bind_group` doc both must keep naming the
    // `filter_dynamic_lights` / `is_dynamic`-filtered set as the b0 source. If a
    // future edit deletes or contradicts that documented contract, this fails —
    // flagging the docs for review. It would NOT catch a wiring bug that rebound b0
    // to the wrong buffer while leaving the doc strings intact; that is the GPU
    // layer, verified by running the engine (testing_guide §3).
    #[test]
    fn skinned_mesh_b0_filtered_dynamic_lights_contract_is_documented() {
        // The shader's b0 declaration documents the filtered-set invariant.
        let shader_src = include_str!("../shaders/skinned_mesh.wgsl");
        assert!(
            shader_src.contains("@group(2) @binding(0) var<storage, read> lights"),
            "skinned_mesh.wgsl must declare the dynamic-light records at group-2 b0",
        );
        assert!(
            shader_src.contains("`is_dynamic`-filtered set"),
            "the b0 declaration must document that it carries the is_dynamic-filtered set \
             (static lights excluded by construction)",
        );
        // The wiring contract (`rebuild_light_bind_group`) names the
        // `filter_dynamic_lights` output as the REQUIRED b0 source.
        let rust_src = include_str!("mesh_pass.rs");
        assert!(
            rust_src.contains("filter_dynamic_lights"),
            "rebuild_light_bind_group must pin the filter_dynamic_lights output as the b0 source",
        );
    }

    // The mesh dynamic-direct loop contributes nothing when `light_count == 0`.
    // Structural assertion (no headless render harness): the accumulator starts at
    // zero and the loop bound is `light_count` (clamped to 0 when the
    // lighting-isolation gate excludes the dynamic term via the SAME
    // `select(0u, light_count, use_dynamic)` forward applies), so a zero-trip loop
    // adds nothing. This scans the shader for those two structural facts.
    #[test]
    fn mesh_dynamic_loop_contributes_nothing_when_light_count_zero() {
        let src = include_str!("../shaders/skinned_mesh.wgsl");
        // Accumulator starts at zero.
        assert!(
            src.contains("var total = vec3<f32>(0.0);"),
            "accumulate_dynamic_direct must seed its accumulator to zero",
        );
        // Loop bound is the (gated) light_count.
        assert!(
            src.contains(
                "let light_count = select(0u, mesh_light_params.light_count, use_dynamic);"
            ),
            "the loop bound must be the gated mesh_light_params.light_count",
        );
        assert!(
            src.contains("i < light_count"),
            "the loop must iterate i in [0, light_count) — zero trips at light_count == 0",
        );
    }

    // The mesh dynamic-direct term participates in the lighting-isolation debug
    // modes via the SAME mode set forward.wgsl uses to gate its world dynamic term.
    // Pin the exact `use_dynamic` derivation in both shaders so a forward-side edit
    // that desyncs the mesh gate fails here. (Forward and mesh both compute
    // `use_dynamic = iso 0|1|2|8`.)
    #[test]
    fn mesh_use_dynamic_gate_matches_forward() {
        const GATE: &str = "(iso == 0u) || (iso == 1u) || (iso == 2u) || (iso == 8u)";
        let mesh_src = include_str!("../shaders/skinned_mesh.wgsl");
        let forward_src = include_str!("../shaders/forward.wgsl");
        assert!(
            mesh_src.contains(&format!("let use_dynamic = {GATE};")),
            "skinned_mesh.wgsl must derive use_dynamic from the forward isolation mode set",
        );
        assert!(
            forward_src.contains(&format!("let use_dynamic = {GATE};")),
            "forward.wgsl's use_dynamic gate changed — update the mesh gate in lock-step",
        );
    }

    // The skinned-mesh shader must DECLARE the pinned group-2 binding map so the
    // appended `curve_eval.wgsl` (`anim_samples` at b3) and `light_eval.wgsl`
    // (`AnimationDescriptor` for b2) symbols resolve and the BGL agrees with the
    // shader. b5–b8 are the shadow-receipt bindings the appended
    // `shadow_sample.wgsl` references by lexical name.
    #[test]
    fn skinned_mesh_wgsl_declares_group2_light_bindings() {
        let src = include_str!("../shaders/skinned_mesh.wgsl");
        for decl in [
            "@group(2) @binding(0) var<storage, read> lights",
            "@group(2) @binding(1) var<storage, read> light_influence",
            "@group(2) @binding(2) var<storage, read> scripted_light_descriptors",
            "@group(2) @binding(3) var<storage, read> anim_samples",
            "@group(2) @binding(4) var<uniform> mesh_light_params",
            "@group(2) @binding(5) var spot_shadow_depth: texture_depth_2d_array",
            "@group(2) @binding(6) var spot_shadow_compare: sampler_comparison",
            "@group(2) @binding(7) var<uniform> light_space_matrices",
            "@group(2) @binding(8) var point_shadow_cube: texture_depth_cube_array",
        ] {
            assert!(
                src.contains(decl),
                "skinned_mesh.wgsl must declare group-2 binding: {decl}",
            );
        }
        // The b8 cube binding must carry the `// CUBE_SHADOW_BINDING` tag so the
        // no-cube `strip_point_shadow_cube` transform can find and drop it.
        assert!(
            src.contains("// CUBE_SHADOW_BINDING"),
            "skinned_mesh.wgsl b8 cube binding must carry the // CUBE_SHADOW_BINDING tag",
        );
        // The b7 light-space matrices array length must match SHADOW_POOL_SIZE so
        // the mesh declaration agrees with the pool's `matrices_buffer`.
        assert!(
            src.contains(&format!(
                "array<mat4x4<f32>, {}>",
                crate::lighting::spot_shadow::SHADOW_POOL_SIZE
            )),
            "skinned_mesh.wgsl b7 must size light_space_matrices to SHADOW_POOL_SIZE",
        );
    }

    // The composed skinned-mesh source must pass naga validation in BOTH cube
    // variants: the canonical source (b8 cube binding present) and the stripped
    // no-cube source (`strip_point_shadow_cube` drops the b8 declaration and
    // neutralizes `sample_point_shadow`). The pipeline picks the matching variant
    // for the adapter, so a validation break in either would only surface at GPU
    // bring-up on the un-tested adapter class — this pins both at build time.
    //
    // Regression: an unused cube binding is legal WGSL, so naga-validating both
    // variants alone does NOT prove the strip removed the b8 declaration. If the
    // `// CUBE_SHADOW_BINDING` tag drifts off the declaration line (onto a comment),
    // the strip leaves the b8 `var point_shadow_cube` declared while the no-cube BGL
    // omits b8 → `create_render_pipeline` rejects the mismatch on a no-cube adapter.
    // The contains-assertions below catch that drift in CI: the no-cube variant must
    // NOT declare b8; the cube variant must.
    #[test]
    fn skinned_mesh_shader_source_validates_both_cube_variants() {
        const CUBE_DECL: &str = "@group(2) @binding(8) var point_shadow_cube";

        let no_cube = skinned_mesh_shader_source(false);
        assert!(
            !no_cube.contains(CUBE_DECL),
            "no-cube skinned-mesh source must NOT declare the b8 cube binding — the strip \
             must remove it so the shader matches the no-cube group-2 BGL (which omits b8)",
        );

        let cube = skinned_mesh_shader_source(true);
        assert!(
            cube.contains(CUBE_DECL),
            "cube-supported skinned-mesh source must declare the b8 cube binding",
        );

        for cube_supported in [true, false] {
            let src = skinned_mesh_shader_source(cube_supported);
            let module = naga::front::wgsl::parse_str(&src).unwrap_or_else(|e| {
                panic!("skinned_mesh source (cube={cube_supported}) must parse: {e:?}")
            });
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| {
                panic!("skinned_mesh source (cube={cube_supported}) must validate: {e:?}")
            });
        }
    }

    // Headless guard for the mesh group-2 shadow-receipt bindings (b5–b8) across
    // BOTH cube-support variants. b5–b7 are unconditional (spot depth 2D-array,
    // comparison sampler, light-space-matrices uniform); b8 (cube-array depth) is
    // present IFF `cube_array_supported` — the `Some`-iff-layout invariant the
    // BGL builder and `rebuild_light_bind_group` both honor. All FRAGMENT-only.
    #[test]
    fn mesh_group2_shadow_bindings_match_both_cube_variants() {
        // No cube support: b5–b7 present, b8 absent (9 entries total: b0–b7).
        let no_cube = mesh_light_bind_group_layout_entries(false);
        let no_cube_bindings: Vec<u32> = no_cube.iter().map(|e| e.binding).collect();
        assert_eq!(
            no_cube_bindings,
            vec![0, 1, 2, 3, 4, 5, 6, 7],
            "no-cube mesh group-2 BGL must declare b0..=b7 (b8 omitted)",
        );

        // Cube support: b8 appended (cube-array depth).
        let cube = mesh_light_bind_group_layout_entries(true);
        let cube_bindings: Vec<u32> = cube.iter().map(|e| e.binding).collect();
        assert_eq!(
            cube_bindings,
            vec![0, 1, 2, 3, 4, 5, 6, 7, 8],
            "cube-supported mesh group-2 BGL must declare b0..=b8",
        );

        // b5: spot shadow depth, Depth 2D-array.
        assert!(
            matches!(
                cube[5].ty,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                }
            ),
            "mesh group-2 b5 must be a Depth 2D-array texture",
        );
        // b6: comparison sampler.
        assert!(
            matches!(
                cube[6].ty,
                wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison)
            ),
            "mesh group-2 b6 must be a comparison sampler",
        );
        // b7: light-space matrices UNIFORM (not storage — fragment storage budget).
        assert!(
            matches!(
                cube[7].ty,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    ..
                }
            ),
            "mesh group-2 b7 must be a uniform buffer (not storage)",
        );
        // b8: cube-array depth (only on the cube variant).
        assert!(
            matches!(
                cube[8].ty,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::CubeArray,
                    multisampled: false,
                }
            ),
            "mesh group-2 b8 must be a Depth cube-array texture",
        );

        // All shadow entries (both variants) are FRAGMENT-only — the mesh shadow
        // sampling runs in the fragment stage; an over-broad visibility would spend
        // a per-stage slot for free.
        for e in &cube {
            assert_eq!(
                e.visibility,
                wgpu::ShaderStages::FRAGMENT,
                "mesh group-2 b{} must be FRAGMENT-only",
                e.binding,
            );
        }

        // Adding the shadow bindings must NOT raise the fragment storage-buffer
        // count: b5/b8 are sampled textures, b6 a sampler, b7 a uniform — still 4.
        assert_eq!(
            fragment_storage_buffers(&cube),
            4,
            "shadow-receipt bindings must keep the fragment storage-buffer count at 4",
        );
        assert_eq!(
            fragment_storage_buffers(&no_cube),
            4,
            "shadow-receipt bindings must keep the fragment storage-buffer count at 4",
        );
    }

    // Recording guard for the mesh pipeline's group-2 sampled-texture count across
    // BOTH cube-support variants. wgpu charges
    // `max_sampled_textures_per_shader_stage` against the BGL *entry* set per
    // stage, and per-stage sampled-texture slots are a hard, low ceiling
    // (rendering_pipeline.md §10; the forward pipeline pins its own count in
    // `forward_pipeline_sampled_texture_request_matches_bgl_definitions`). Pin the
    // mesh group-2 numbers so a future binding addition that pushes a sampled
    // texture into group 2 is caught headlessly before a real GPU rejects it.
    //
    // No cube support: ONE sampled texture — b5 spot depth 2D-array. (b6 is a
    // sampler, b7 a uniform; b8 cube array is omitted on the no-cube layout.)
    // Cube support: TWO — b5 spot depth array + b8 point-shadow cube array.
    // Modeled on the billboard storage-count guard
    // (`billboard_pipeline_vertex_storage_request_matches_bgl_definitions`) and the
    // forward sampled-texture guard: re-derive from the SAME GPU-free BGL builder.
    #[test]
    fn mesh_group2_sampled_texture_count_recorded_for_both_cube_variants() {
        // No-cube: only b5 (spot depth array) is a fragment-sampled texture.
        let no_cube = mesh_light_bind_group_layout_entries(false);
        assert_eq!(
            fragment_sampled_textures(&no_cube),
            1,
            "no-cube mesh group-2 must carry exactly ONE sampled texture (b5 spot depth array)",
        );

        // Cube: b5 (spot depth array) + b8 (point-shadow cube array) = two.
        let cube = mesh_light_bind_group_layout_entries(true);
        assert_eq!(
            fragment_sampled_textures(&cube),
            2,
            "cube-supported mesh group-2 must carry exactly TWO sampled textures \
             (b5 spot depth array + b8 cube array)",
        );

        // The cube variant adds exactly ONE sampled texture over the no-cube
        // variant — the point-shadow cube array (b8) and nothing else.
        assert_eq!(
            fragment_sampled_textures(&cube) - fragment_sampled_textures(&no_cube),
            1,
            "enabling cube support must add exactly one sampled texture (the b8 cube array)",
        );

        // Both counts sit well under the Metal/WebGPU sampled-texture spec floor of
        // 16. Group 2 is only one of the mesh pipeline's bind groups, but pinning
        // its contribution here keeps the group-2 share honest; raising it toward
        // the ceiling should be a deliberate budget decision (rendering_pipeline.md
        // §10), not an accidental binding addition.
        assert!(
            fragment_sampled_textures(&cube) <= 16,
            "mesh group-2 sampled-texture count must stay under the spec floor of 16",
        );
    }

    // --- Cache-side clip query seam (GPU-free) ----------------------------------
    //
    // The clip-name / metadata lookups back clip-name resolution at level load
    // (main.rs's level-load sweep). They read the cache-side `model_clips` map,
    // split out of `MeshPass` (which needs a `wgpu::Device`) into the GPU-free
    // `clip_by_name` / `clip_metadata` free functions so the seam is testable here
    // without a GPU.

    use crate::model::skeleton::AnimationClip;

    /// Build a named clip with `duration` and no per-joint tracks. The query seam
    /// keys on name + duration only; track contents are irrelevant to it.
    fn named_clip(name: &str, duration: f32) -> AnimationClip {
        AnimationClip {
            name: name.to_string(),
            duration,
            joints: Vec::new(),
        }
    }

    fn clip_map(
        entries: Vec<(ModelHandle, Vec<AnimationClip>)>,
    ) -> HashMap<ModelHandle, Vec<AnimationClip>> {
        entries.into_iter().collect()
    }

    /// A multi-clip model retains every clip in glTF (authored) order, each
    /// retrievable by its authored name reporting its own duration — the
    /// cache-level half of the multi-clip query contract.
    #[test]
    fn clip_query_retains_all_clips_in_order_each_by_name_with_own_duration() {
        let handle = ModelHandle::from("multi");
        let map = clip_map(vec![(
            handle.clone(),
            vec![
                named_clip("idle", 1.0),
                named_clip("walk", 2.5),
                named_clip("attack", 0.75),
            ],
        )]);

        // Metadata preserves authored order and per-clip duration.
        let meta = clip_metadata(&map, &handle);
        assert_eq!(
            meta,
            vec![
                ClipMetadata {
                    name: "idle".to_string(),
                    duration: 1.0
                },
                ClipMetadata {
                    name: "walk".to_string(),
                    duration: 2.5
                },
                ClipMetadata {
                    name: "attack".to_string(),
                    duration: 0.75
                },
            ],
            "clip metadata must list every clip in authored glTF order",
        );

        // Each clip is retrievable by its authored name, reporting its own
        // duration — not just the first.
        for (name, duration) in [("idle", 1.0_f32), ("walk", 2.5), ("attack", 0.75)] {
            let clip = clip_by_name(&map, &handle, name)
                .unwrap_or_else(|| panic!("clip '{name}' must be retrievable by name"));
            assert_eq!(clip.name, name);
            assert!(
                (clip.duration - duration).abs() < 1.0e-6,
                "clip '{name}' must report its own duration {duration}, got {}",
                clip.duration,
            );
        }
    }

    /// Looking up a clip name absent from a model returns nothing — no error, no
    /// panic.
    #[test]
    fn clip_by_name_absent_name_returns_none() {
        let handle = ModelHandle::from("m");
        let map = clip_map(vec![(handle.clone(), vec![named_clip("idle", 1.0)])]);
        assert!(
            clip_by_name(&map, &handle, "nonexistent").is_none(),
            "an absent clip name must return None, not panic",
        );
    }

    /// An un-cached handle returns nothing from both queries — empty metadata, no
    /// clip — covering a model that never loaded or has no animation.
    #[test]
    fn clip_query_absent_handle_returns_empty() {
        let map = clip_map(vec![(
            ModelHandle::from("present"),
            vec![named_clip("idle", 1.0)],
        )]);
        let missing = ModelHandle::from("missing");
        assert!(
            clip_by_name(&map, &missing, "idle").is_none(),
            "clip_by_name on an un-cached handle must return None",
        );
        assert!(
            clip_metadata(&map, &missing).is_empty(),
            "clip_metadata on an un-cached handle must return an empty Vec",
        );
    }

    /// Duplicate authored names: first match wins (the earliest glTF-index clip).
    /// glTF does not forbid duplicate animation names, and the documented rule is
    /// that the earlier authored clip is returned.
    #[test]
    fn clip_by_name_returns_first_match_on_duplicate_names() {
        let handle = ModelHandle::from("dupes");
        let map = clip_map(vec![(
            handle.clone(),
            vec![named_clip("loop", 1.0), named_clip("loop", 9.0)],
        )]);
        let clip = clip_by_name(&map, &handle, "loop").expect("a 'loop' clip must be found");
        assert!(
            (clip.duration - 1.0).abs() < 1.0e-6,
            "duplicate names must resolve to the FIRST authored clip (duration 1.0), got {}",
            clip.duration,
        );
    }

    /// End-to-end cache seam: clips parsed from a real multi-clip glTF, inserted
    /// under a `ModelHandle`, are queryable by authored name and
    /// enumerable as metadata in glTF order through the GPU-free free functions —
    /// no `wgpu::Device`. Drives the same `clip_metadata` / `clip_by_name` free
    /// functions that `model_clip_metadata` / `model_clip_by_name` delegate to at
    /// runtime, but headless.
    #[test]
    fn loaded_multi_clip_model_is_queryable_by_name_and_metadata_through_cache() {
        use std::path::PathBuf;

        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/multi_clip/multi_clip.gltf");
        let model =
            crate::model::gltf_loader::load_model(&fixture).expect("multi-clip fixture loads");

        let handle = ModelHandle::from("multi_clip");
        let map = clip_map(vec![(handle.clone(), model.clips.clone())]);

        // Metadata lists both clips in authored glTF order, each with its own
        // duration (idle 1.0, walk 2.0) — exactly what was parsed from the file.
        let meta = clip_metadata(&map, &handle);
        assert_eq!(meta.len(), 2, "both loaded clips appear in metadata");
        assert_eq!(meta[0].name, "idle");
        assert_eq!(meta[1].name, "walk");
        assert!((meta[0].duration - 1.0).abs() < 1.0e-4, "idle duration");
        assert!((meta[1].duration - 2.0).abs() < 1.0e-4, "walk duration");

        // Each clip is retrievable by its authored name, reporting its own
        // duration.
        let idle = clip_by_name(&map, &handle, "idle").expect("'idle' clip found by name");
        assert!((idle.duration - 1.0).abs() < 1.0e-4);
        let walk = clip_by_name(&map, &handle, "walk").expect("'walk' clip found by name");
        assert!((walk.duration - 2.0).abs() < 1.0e-4);

        // A name the model does not carry returns nothing — no error, no panic.
        assert!(
            clip_by_name(&map, &handle, "run").is_none(),
            "an absent clip name returns None",
        );
    }

    // --- Snapshot store + per-instance sampling (GPU-free) ----------------------
    //
    // `SnapshotStore`, `apply_capture`, and `sample_instance` take no wgpu types
    // (the `model_bounds` precedent), so the `"smooth"`-interrupt seam and the
    // per-instance blend selection are unit-testable without a device. These pin:
    // single-clip steady state, clip→clip + snapshot→clip blends, the missed-
    // capture degrade-to-fallback, idempotent capture, and the store lifecycle.

    use crate::model::anim::Loop as AnimLoop;
    use crate::model::sample_params::{
        CaptureInstruction, ClipSample, FadeSource, MeshFade, MeshSampleParams,
    };
    use crate::model::skeleton::{Joint, JointTracks, RestLocal, Skeleton, Track};
    use glam::{Mat4, Quat};

    /// Single-root skeleton with identity inverse-bind, so a palette entry's
    /// skinning matrix decomposes straight to the joint's local TRS.
    fn one_joint_skeleton() -> Skeleton {
        Skeleton {
            joints: vec![Joint {
                parent: None,
                inverse_bind: Mat4::IDENTITY.to_cols_array_2d(),
                rest_local: RestLocal::default(),
            }],
        }
    }

    /// One-joint clip holding a constant translation (single key), so it samples
    /// to exactly `tx` on X at any time.
    fn const_x_clip(name: &str, tx: f32) -> AnimationClip {
        AnimationClip {
            name: name.to_string(),
            duration: 1.0,
            joints: vec![JointTracks {
                translation: Track {
                    times: vec![0.0],
                    values: vec![Vec3::new(tx, 0.0, 0.0)],
                    ..Default::default()
                },
                rotation: Track {
                    times: vec![0.0],
                    values: vec![Quat::IDENTITY],
                    ..Default::default()
                },
                scale: Track {
                    times: vec![0.0],
                    values: vec![Vec3::ONE],
                    ..Default::default()
                },
            }],
        }
    }

    fn palette_x(out: &[BonePaletteEntry]) -> f32 {
        Mat4::from_cols_array_2d(&out[0].matrix).w_axis.x
    }

    fn clip_leg(idx: usize, time: f32) -> ClipSample {
        ClipSample {
            clip_index: idx,
            time,
            loop_policy: AnimLoop::Wrap,
        }
    }

    #[test]
    fn sample_instance_single_clip_no_fade_samples_primary() {
        let skel = one_joint_skeleton();
        let clips = [const_x_clip("idle", 5.0)];
        let store = SnapshotStore::default();
        let params = MeshSampleParams {
            primary: clip_leg(0, 0.0),
            fade: None,
        };
        let mut out = Vec::new();
        let sampled = sample_instance(&params, &skel, &store, 1, &|i| clips.get(i), &mut out);
        assert!(sampled);
        assert!(
            (palette_x(&out) - 5.0).abs() < 1.0e-4,
            "single clip → primary pose"
        );
    }

    #[test]
    fn sample_instance_unresolved_primary_writes_identity_bind_pose_run() {
        // Regression: an unresolved primary clip used to return false and skip the
        // palette write, leaving the densely-repacked run holding another
        // instance's stale matrices. It must now write a clean identity bind-pose
        // run (one per joint) and return true so the caller overwrites the region.
        let skel = one_joint_skeleton();
        let clips: Vec<AnimationClip> = vec![]; // index 0 absent
        let store = SnapshotStore::default();
        let params = MeshSampleParams {
            primary: clip_leg(0, 0.0),
            fade: None,
        };
        // Pre-fill `out` with a stranger's stale pose to prove it is overwritten.
        let mut out = palette_run(99.0, 1);
        let sampled = sample_instance(&params, &skel, &store, 1, &|i| clips.get(i), &mut out);
        assert!(
            sampled,
            "an unresolved primary still writes a (bind-pose) run"
        );
        assert_eq!(out.len(), skel.joints.len(), "one entry per joint");
        let identity = glam::Mat4::IDENTITY.to_cols_array_2d();
        assert_eq!(
            out[0].matrix, identity,
            "the unsampled run is the identity bind pose, not stale matrices",
        );
    }

    #[test]
    fn sample_instance_clip_fade_blends_endpoints_and_midpoint() {
        let skel = one_joint_skeleton();
        let clips = [const_x_clip("from", 0.0), const_x_clip("to", 10.0)];
        let store = SnapshotStore::default();
        let mut out = Vec::new();
        let make = |weight: f32| MeshSampleParams {
            primary: clip_leg(1, 0.0),
            fade: Some(MeshFade {
                from: FadeSource::Clip(clip_leg(0, 0.0)),
                weight,
            }),
        };
        // Weight 0 → all `from` (x=0); weight 1 → all primary (x=10); 0.5 → 5.
        sample_instance(&make(0.0), &skel, &store, 1, &|i| clips.get(i), &mut out);
        assert!(
            (palette_x(&out) - 0.0).abs() < 1.0e-4,
            "weight 0 = outgoing"
        );
        sample_instance(&make(1.0), &skel, &store, 1, &|i| clips.get(i), &mut out);
        assert!(
            (palette_x(&out) - 10.0).abs() < 1.0e-4,
            "weight 1 = primary"
        );
        sample_instance(&make(0.5), &skel, &store, 1, &|i| clips.get(i), &mut out);
        assert!(
            (palette_x(&out) - 5.0).abs() < 1.0e-4,
            "weight 0.5 = midpoint"
        );
    }

    /// `apply_capture` freezes `blend(outgoing_clip, incoming_clip)` into the
    /// store; a subsequent snapshot fade at weight 0 reproduces that captured
    /// pose — the smooth interrupt has no discontinuity.
    #[test]
    fn capture_then_snapshot_fade_reproduces_in_flight_blend() {
        let skel = one_joint_skeleton();
        // outgoing idle (x=0), incoming walk (x=10). Capture at weight 0.4 →
        // blended x = 4.0.
        let clips = [const_x_clip("idle", 0.0), const_x_clip("walk", 10.0)];
        let mut store = SnapshotStore::default();
        let tag: SnapshotTag = 42;
        let capture = CaptureInstruction {
            seed: 7,
            tag,
            outgoing: FadeSource::Clip(clip_leg(0, 0.0)),
            incoming: clip_leg(1, 0.0),
            weight: 0.4,
        };
        let mut scratch = Vec::new();
        store.apply_capture(&capture, &skel, |i| clips.get(i), &mut scratch);
        assert!(store.matching(7, tag).is_some(), "store holds the capture");

        // Snapshot fade at weight 0 reproduces the captured pose (x = 4.0).
        let params = MeshSampleParams {
            primary: clip_leg(1, 0.0),
            fade: Some(MeshFade {
                from: FadeSource::Snapshot {
                    tag,
                    fallback: clip_leg(0, 0.0),
                },
                weight: 0.0,
            }),
        };
        let mut out = Vec::new();
        sample_instance(&params, &skel, &store, 7, &|i| clips.get(i), &mut out);
        assert!(
            (palette_x(&out) - 4.0).abs() < 1.0e-4,
            "snapshot fade weight 0 reproduces the captured in-flight blend, got {}",
            palette_x(&out),
        );
    }

    /// Capture is IDEMPOTENT by tag: a re-emission under the same tag evaluates
    /// nothing (a frozen-clock re-render does not re-capture a moved pose).
    #[test]
    fn capture_is_idempotent_by_tag() {
        let skel = one_joint_skeleton();
        let clips = [const_x_clip("idle", 0.0), const_x_clip("walk", 10.0)];
        let mut store = SnapshotStore::default();
        let tag: SnapshotTag = 1;
        let first = CaptureInstruction {
            seed: 3,
            tag,
            outgoing: FadeSource::Clip(clip_leg(0, 0.0)),
            incoming: clip_leg(1, 0.0),
            weight: 0.4,
        };
        let mut scratch = Vec::new();
        store.apply_capture(&first, &skel, |i| clips.get(i), &mut scratch);
        let captured = store.matching(3, tag).unwrap().to_vec();

        // Re-emit with the SAME tag but a different weight — must NOT re-capture.
        let again = CaptureInstruction {
            weight: 0.9,
            ..first
        };
        store.apply_capture(&again, &skel, |i| clips.get(i), &mut scratch);
        assert_eq!(
            store.matching(3, tag).unwrap(),
            captured.as_slice(),
            "a same-tag re-emission must evaluate nothing (idempotent)",
        );
    }

    /// A snapshot fade whose store entry is MISSING (capture frame culled /
    /// budget-dropped) degrades to the fallback clip — a `"snap"`-equivalent
    /// blend, no panic, no stale snapshot.
    #[test]
    fn missing_snapshot_degrades_to_fallback_clip() {
        let skel = one_joint_skeleton();
        let clips = [const_x_clip("fallback", 2.0), const_x_clip("primary", 10.0)];
        let store = SnapshotStore::default(); // empty — capture frame never planned
        let params = MeshSampleParams {
            primary: clip_leg(1, 0.0),
            fade: Some(MeshFade {
                from: FadeSource::Snapshot {
                    tag: 99,
                    fallback: clip_leg(0, 0.0),
                },
                weight: 0.5,
            }),
        };
        let mut out = Vec::new();
        sample_instance(&params, &skel, &store, 5, &|i| clips.get(i), &mut out);
        // Blend fallback (x=2) → primary (x=10) at 0.5 = 6.0 (NOT the snapshot).
        assert!(
            (palette_x(&out) - 6.0).abs() < 1.0e-4,
            "missed snapshot degrades to fallback×primary blend, got {}",
            palette_x(&out),
        );
    }

    /// A snapshot-referencing capture that MISSES the store captures
    /// `blend(fallback, incoming)` instead — the degrade applies to the capture
    /// path too, so a chained smooth interrupt over a culled snapshot is sound.
    #[test]
    fn snapshot_referencing_capture_misses_store_uses_fallback() {
        let skel = one_joint_skeleton();
        // fallback x=2, incoming x=10. Capture at weight 0.5 → x = 6.0.
        let clips = [
            const_x_clip("fallback", 2.0),
            const_x_clip("incoming", 10.0),
        ];
        let mut store = SnapshotStore::default();
        let new_tag: SnapshotTag = 100;
        let capture = CaptureInstruction {
            seed: 8,
            tag: new_tag,
            // Outgoing references a PRIOR snapshot (tag 77) that is NOT in the
            // store, carrying the same fallback the sampling frames use.
            outgoing: FadeSource::Snapshot {
                tag: 77,
                fallback: clip_leg(0, 0.0),
            },
            incoming: clip_leg(1, 0.0),
            weight: 0.5,
        };
        let mut scratch = Vec::new();
        store.apply_capture(&capture, &skel, |i| clips.get(i), &mut scratch);
        let pose = store
            .matching(8, new_tag)
            .expect("capture landed via fallback");
        let x = pose[0].translation.x;
        assert!(
            (x - 6.0).abs() < 1.0e-4,
            "missed snapshot reference captures blend(fallback, incoming), got {x}",
        );
    }

    /// Chained smooth interrupt: a capture whose outgoing references a PRIOR
    /// stored snapshot blends against that snapshot (store HIT), freezing
    /// `blend(prior_snapshot, incoming)` and superseding the prior entry — the
    /// "interrupt whose source is itself a snapshot" acceptance criterion.
    #[test]
    fn chained_capture_blends_against_prior_snapshot() {
        let skel = one_joint_skeleton();
        // Seed a prior snapshot (tag 1) holding x = 8.0 directly.
        let mut store = SnapshotStore::default();
        store.entries.insert(
            7,
            StoredSnapshot {
                tag: 1,
                pose: vec![LocalTrs {
                    translation: Vec3::new(8.0, 0.0, 0.0),
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                }],
            },
        );
        // Incoming clip x = 0. New capture (tag 2) outgoing references the prior
        // snapshot (tag 1) → HIT. At weight 0.25, x = 8*(0.75)+0*(0.25) = 6.0.
        let clips = [const_x_clip("incoming", 0.0)];
        let capture = CaptureInstruction {
            seed: 7,
            tag: 2,
            outgoing: FadeSource::Snapshot {
                tag: 1,
                fallback: clip_leg(0, 0.0),
            },
            incoming: clip_leg(0, 0.0),
            weight: 0.25,
        };
        let mut scratch = Vec::new();
        store.apply_capture(&capture, &skel, |i| clips.get(i), &mut scratch);
        // Old entry (tag 1) superseded by the new one (tag 2).
        assert!(store.matching(7, 1).is_none(), "prior entry superseded");
        let pose = store.matching(7, 2).expect("new chained capture stored");
        assert!(
            (pose[0].translation.x - 6.0).abs() < 1.0e-4,
            "chained capture blends against the prior snapshot, got {}",
            pose[0].translation.x,
        );
    }

    // --- End-to-end smooth-interrupt palette continuity (game side → renderer) --
    //
    // Drives the WHOLE smooth-interrupt path with no GPU: build a `MeshAnimation`,
    // start an A→B fade, switch to C (smooth) mid-flight, run the game-side resolve
    // pass + `animate_entity`, then feed the emitted capture + sample params into
    // `apply_capture`/`sample_instance` and read the palette. Asserts the pose at
    // the interrupt instant equals the pre-switch in-flight blend (no pop) and
    // then eases toward C. Regression: a "smooth" interrupt that captured the
    // outgoing leg (or dropped the in-flight blend's OUT leg) snapped instead.

    use crate::scripting::components::mesh::{
        AnimationState as GameState, FadeSourceKind, InterruptPolicy, MeshAnimation,
    };
    use std::collections::HashMap as GameMap;

    /// Build a game-side `AnimationState` with an explicit resolved clip index.
    fn game_state(clip: &str, crossfade_ms: f32, clip_index: usize) -> GameState {
        GameState {
            clip: clip.into(),
            looping: true,
            crossfade_ms,
            interrupt: InterruptPolicy::Smooth,
            clip_index: Some(clip_index),
        }
    }

    /// A three-state animation: A=idle(clip0), B=walk(clip1), C=run(clip2), all
    /// looping, B/C fading in over `b_ms`/`c_ms`. Used to drive an A→B fade then a
    /// smooth interrupt to C.
    fn abc_animation(b_ms: f32, c_ms: f32) -> MeshAnimation {
        let mut states = GameMap::new();
        states.insert("A".into(), game_state("idle", 0.0, 0));
        states.insert("B".into(), game_state("walk", b_ms, 1));
        states.insert("C".into(), game_state("run", c_ms, 2));
        MeshAnimation::new(states, "A".into())
    }

    #[test]
    fn smooth_interrupt_end_to_end_no_pop_then_eases_to_c() {
        // A→B fade in flight (B over 0.2s); at t=1.1 (w_AB = 0.5) interrupt to C
        // (smooth, C over 0.1s). The interrupt-instant pose must equal
        // blend(A, B, 0.5) with NO discontinuity, then ease toward C.
        let skel = one_joint_skeleton();
        // clip0 A x=0, clip1 B x=10, clip2 C x=100 (const, so blends are exact).
        let clips = [
            const_x_clip("idle", 0.0),
            const_x_clip("walk", 10.0),
            const_x_clip("run", 100.0),
        ];
        let resolve = |i: usize| clips.get(i);

        let mut anim = abc_animation(200.0, 100.0);
        // A entered at 0.0 (resolved), B fading in from A at 1.0.
        anim.entered_at = Some(0.0);
        anim.current_state = "B".into();
        anim.previous_state = Some("A".into());
        anim.previous_entered_at = Some(0.0);
        // B's own entry stamp:
        anim.entered_at = Some(1.0);
        anim.fade_source = FadeSourceKind::Clip;

        // Mid-B-fade: interrupt to C (smooth). Mirror what
        // `switch_animation_state` records for a smooth interrupt, then resolve C's
        // stamp to the interrupt instant t2 = 1.1.
        anim.interrupted_outgoing = Some(
            crate::scripting::components::mesh::InterruptedOutgoing::Clip {
                state: "A".into(),
                entered_at: 0.0,
            },
        );
        anim.previous_state = Some("B".into());
        anim.previous_entered_at = Some(1.0);
        anim.current_state = "C".into();
        anim.fade_source = FadeSourceKind::Snapshot;
        let t2 = 1.1_f64; // C entered_at — the interrupt instant
        anim.entered_at = Some(t2);

        // Expected in-flight pose the entity showed JUST before the switch:
        // blend(A, B, w_AB) with w_AB = (t2 - B_stamp)/B_crossfade = 0.1/0.2 = 0.5.
        let expected_s_x = 0.0 * 0.5 + 10.0 * 0.5; // = 5.0

        // Game side: emit the capture + sample params at the interrupt instant.
        let result =
            crate::scripting_systems::mesh_anim::animate_entity(&anim, t2, 0.0).expect("animates");
        let capture = result.capture.expect("smooth interrupt emits a capture");

        // Renderer side: apply the capture, then sample the pose this frame.
        let mut store = SnapshotStore::default();
        let mut scratch = Vec::new();
        store.apply_capture(&capture, &skel, resolve, &mut scratch);

        let mut out = Vec::new();
        sample_instance(&result.sample, &skel, &store, 0, &resolve, &mut out);
        assert!(
            (palette_x(&out) - expected_s_x).abs() < 1.0e-3,
            "interrupt-instant pose must equal the in-flight blend {expected_s_x}, got {}",
            palette_x(&out),
        );

        // Now advance into C's fade window: the pose must ease toward C (x=100),
        // moving away from S. At t = t2 + 0.05 (halfway through C's 0.1s window)
        // the snapshot→C blend weight is 0.5, so x = blend(5.0, 100.0, 0.5) = 52.5.
        let mid = crate::scripting_systems::mesh_anim::animate_entity(&anim, t2 + 0.05, 0.0)
            .expect("animates mid-C-fade");
        // The capture is idempotent (same tag) — re-applying changes nothing.
        store.apply_capture(
            &mid.capture.expect("re-emitted capture under frozen stamp"),
            &skel,
            resolve,
            &mut scratch,
        );
        let mut out_mid = Vec::new();
        sample_instance(&mid.sample, &skel, &store, 0, &resolve, &mut out_mid);
        let x_mid = palette_x(&out_mid);
        assert!(
            (x_mid - 52.5).abs() < 1.0e-3,
            "mid-C-fade pose eases from S(5.0) toward C(100.0): expected 52.5, got {x_mid}",
        );
        assert!(
            x_mid > expected_s_x,
            "the pose moves toward C, not back toward the outgoing leg",
        );
    }

    #[test]
    fn smooth_interrupt_end_to_end_chained_snapshot_no_pop() {
        // Snapshot-of-snapshot: a smooth interrupt over an ALREADY-smooth fade.
        // A→B interrupted to C (smooth) leaves a snapshot S1 = blend(A,B,0.5).
        // Then C is interrupted to D (smooth) mid-C-fade: the new capture must
        // blend against S1 (store HIT) so D's fade resumes from the live pose
        // (blend(S1, C, w_C)) with no discontinuity.
        let skel = one_joint_skeleton();
        let clips = [
            const_x_clip("idle", 0.0),  // A clip0
            const_x_clip("walk", 10.0), // B clip1
            const_x_clip("run", 100.0), // C clip2
            const_x_clip("dash", 50.0), // D clip3
        ];
        let resolve = |i: usize| clips.get(i);

        // First interrupt (A→B → C) at t2a = 1.1, capturing S1 = blend(A,B,0.5)=5.0,
        // tagged bits(1.1). Build the post-first-interrupt anim directly.
        let mut anim = {
            let mut states = GameMap::new();
            states.insert("A".into(), game_state("idle", 0.0, 0));
            states.insert("B".into(), game_state("walk", 200.0, 1));
            states.insert("C".into(), game_state("run", 100.0, 2));
            states.insert("D".into(), game_state("dash", 100.0, 3));
            MeshAnimation::new(states, "A".into())
        };
        let t2a = 1.1_f64;
        anim.current_state = "C".into();
        anim.previous_state = Some("B".into());
        anim.previous_entered_at = Some(1.0);
        anim.entered_at = Some(t2a);
        anim.fade_source = FadeSourceKind::Snapshot;
        anim.interrupted_outgoing = Some(
            crate::scripting::components::mesh::InterruptedOutgoing::Clip {
                state: "A".into(),
                entered_at: 0.0,
            },
        );

        // Apply the first capture into the store (S1 tagged bits(1.1)).
        let first = crate::scripting_systems::mesh_anim::animate_entity(&anim, t2a, 0.0)
            .unwrap()
            .capture
            .expect("first smooth interrupt capture");
        let mut store = SnapshotStore::default();
        let mut scratch = Vec::new();
        store.apply_capture(&first, &skel, resolve, &mut scratch);
        let s1_x = palette_x_trs(store.matching(0, t2a.to_bits()).expect("S1 stored"));
        assert!((s1_x - 5.0).abs() < 1.0e-3, "S1 = blend(A,B,0.5) = 5.0");

        // Second interrupt (C → D, smooth) at t2b = t2a + 0.05 (w_C = 0.5 over C's
        // 0.1s window). The interrupted fade was the SNAPSHOT fade S1→C, so D's
        // capture references the prior snapshot S1 (tag bits(t2a)) and blends
        // blend(S1, C, 0.5) = blend(5.0, 100.0, 0.5) = 52.5 — the live pose.
        let t2b = t2a + 0.05;
        anim.current_state = "D".into();
        anim.previous_state = Some("C".into());
        anim.previous_entered_at = Some(t2a);
        anim.entered_at = Some(t2b);
        anim.fade_source = FadeSourceKind::Snapshot;
        // The interrupted fade (S1→C) had a SNAPSHOT outgoing: stash references S1.
        anim.interrupted_outgoing = Some(
            crate::scripting::components::mesh::InterruptedOutgoing::Snapshot {
                tag: t2a.to_bits(),
            },
        );

        let second = crate::scripting_systems::mesh_anim::animate_entity(&anim, t2b, 0.0)
            .expect("animates")
            .capture
            .expect("second (chained) smooth interrupt capture");
        store.apply_capture(&second, &skel, resolve, &mut scratch);
        let s2_x = palette_x_trs(store.matching(0, t2b.to_bits()).expect("S2 stored"));
        assert!(
            (s2_x - 52.5).abs() < 1.0e-3,
            "chained capture S2 = blend(S1, C, 0.5) = 52.5 (no pop), got {s2_x}",
        );
        // And the prior snapshot S1 is superseded.
        assert!(
            store.matching(0, t2a.to_bits()).is_none(),
            "S1 is superseded by the chained capture S2",
        );

        // Sample D's fade at its instant (weight 0) → reproduces S2 exactly.
        let mut out = Vec::new();
        let sample = crate::scripting_systems::mesh_anim::animate_entity(&anim, t2b, 0.0)
            .unwrap()
            .sample;
        sample_instance(&sample, &skel, &store, 0, &resolve, &mut out);
        assert!(
            (palette_x(&out) - 52.5).abs() < 1.0e-3,
            "D's fade at the interrupt instant reproduces S2 (no discontinuity)",
        );
    }

    #[test]
    fn culled_interrupt_frame_reconstructs_same_snapshot_idempotently() {
        // The capture frame is CULLED (never applied). On the first PLANNED frame —
        // later in C's fade window — the re-evaluated capture must reconstruct the
        // SAME interrupt-instant snapshot S, because legs are sampled at the frozen
        // entered_at, not the moving clock. Regression: sampling at the live clock
        // installed a drifted mid-fade pose on a late capture.
        let skel = one_joint_skeleton();
        let clips = [
            const_x_clip("idle", 0.0),
            const_x_clip("walk", 10.0),
            const_x_clip("run", 100.0),
        ];
        let resolve = |i: usize| clips.get(i);

        let mut anim = abc_animation(200.0, 100.0);
        let t2 = 1.1_f64;
        anim.current_state = "C".into();
        anim.previous_state = Some("B".into());
        anim.previous_entered_at = Some(1.0);
        anim.entered_at = Some(t2);
        anim.fade_source = FadeSourceKind::Snapshot;
        anim.interrupted_outgoing = Some(
            crate::scripting::components::mesh::InterruptedOutgoing::Clip {
                state: "A".into(),
                entered_at: 0.0,
            },
        );

        // The capture as it would be emitted AT the interrupt instant (the frame
        // that got culled — never applied to the store).
        let at_instant = crate::scripting_systems::mesh_anim::animate_entity(&anim, t2, 0.0)
            .unwrap()
            .capture
            .expect("interrupt-instant capture");

        // The capture re-emitted on a LATER planned frame (clock advanced to
        // t2 + 0.06, deep into C's fade). It must be byte-identical: same tag, same
        // outgoing/incoming legs (frozen at t2), same weight.
        let late = crate::scripting_systems::mesh_anim::animate_entity(&anim, t2 + 0.06, 0.0)
            .unwrap()
            .capture
            .expect("late re-emitted capture");
        assert_eq!(
            at_instant, late,
            "a culled capture, re-emitted late, reconstructs the SAME instruction",
        );

        // And applying the late capture to a cold store yields S = blend(A,B,0.5).
        let mut store = SnapshotStore::default();
        let mut scratch = Vec::new();
        store.apply_capture(&late, &skel, resolve, &mut scratch);
        let s_x = palette_x_trs(store.matching(0, t2.to_bits()).expect("S stored late"));
        assert!(
            (s_x - 5.0).abs() < 1.0e-3,
            "the late capture reconstructs the interrupt-instant S = 5.0, got {s_x}",
        );
    }

    /// Read joint 0's X translation directly out of a stored snapshot's TRS buffer.
    fn palette_x_trs(pose: &[LocalTrs]) -> f32 {
        pose[0].translation.x
    }

    #[test]
    fn snapshot_store_drop_and_clear() {
        let skel = one_joint_skeleton();
        let clips = [const_x_clip("a", 0.0), const_x_clip("b", 1.0)];
        let mut store = SnapshotStore::default();
        let mut scratch = Vec::new();
        store.apply_capture(
            &CaptureInstruction {
                seed: 1,
                tag: 5,
                outgoing: FadeSource::Clip(clip_leg(0, 0.0)),
                incoming: clip_leg(1, 0.0),
                weight: 0.5,
            },
            &skel,
            |i| clips.get(i),
            &mut scratch,
        );
        assert!(store.matching(1, 5).is_some());
        store.drop_entry(1);
        assert!(
            store.matching(1, 5).is_none(),
            "drop_entry removes the entry"
        );

        // A tag mismatch never matches even when an entry exists.
        store.apply_capture(
            &CaptureInstruction {
                seed: 2,
                tag: 5,
                outgoing: FadeSource::Clip(clip_leg(0, 0.0)),
                incoming: clip_leg(1, 0.0),
                weight: 0.5,
            },
            &skel,
            |i| clips.get(i),
            &mut scratch,
        );
        assert!(store.matching(2, 6).is_none(), "tag mismatch never matches");
        store.clear();
        assert!(store.matching(2, 5).is_none(), "clear empties the store");
    }

    // --- Time-slicing palette cache --------------------------------------------

    fn palette_run(fill: f32, joints: usize) -> Vec<BonePaletteEntry> {
        vec![
            BonePaletteEntry {
                matrix: [[fill; 4]; 4],
            };
            joints
        ]
    }

    #[test]
    fn palette_cache_miss_forces_resample_then_skip_serves_cache() {
        // A cold cache MISSES, so it must force a resample regardless of the
        // collector's flag — a re-entering instance never re-uploads a stale (or
        // absent) pose. After the run is stored, a collector skip serves the cache.
        let mut cache = PaletteCache::default();
        let seed = 7u32;

        // Miss: even with collector_resample = false, must_sample is true.
        assert!(
            cache.must_sample(seed, false),
            "a cache miss forces a resample even when the collector cleared a skip",
        );

        // Store a sampled run (the resample frame's outcome).
        let run = palette_run(1.0, 4);
        cache.store(seed, &run);

        // Now a collector skip (resample = false) is honored — the entry exists.
        assert!(
            !cache.must_sample(seed, false),
            "with a cached run, a collector skip is honored (no forced resample)",
        );
        // And the cached run is served for the skip re-upload.
        let cached = cache
            .touch_cached(seed)
            .expect("cached run present on skip");
        assert_eq!(cached.len(), 4);
        assert_eq!(cached[0].matrix[0][0], 1.0);

        // A collector resample still samples even with a cache hit.
        assert!(
            cache.must_sample(seed, true),
            "an explicit collector resample always samples, cache hit or not",
        );
    }

    #[test]
    fn palette_cache_store_reuses_storage_in_place() {
        // A resample refreshes the run in place — repeated stores must not change
        // the served contents' shape unexpectedly, and the latest store wins.
        let mut cache = PaletteCache::default();
        let seed = 3u32;
        cache.store(seed, &palette_run(1.0, 6));
        cache.store(seed, &palette_run(2.0, 6));
        let cached = cache.touch_cached(seed).expect("present");
        assert_eq!(cached.len(), 6);
        assert_eq!(cached[0].matrix[0][0], 2.0, "the latest stored run wins");
    }

    #[test]
    fn palette_cache_evicts_entries_absent_from_the_frame() {
        // Entries not touched in a frame are evicted at end_frame, so the cache is
        // bounded by the frame's planned-instance count — a culled-out entity's
        // stale run does not linger.
        let mut cache = PaletteCache::default();
        cache.store(1, &palette_run(1.0, 2));
        cache.store(2, &palette_run(1.0, 2));
        cache.end_frame(); // both stored this "frame" → both survive
        assert!(!cache.must_sample(1, false), "entry 1 survives its frame");
        assert!(!cache.must_sample(2, false), "entry 2 survives its frame");

        // Next frame: touch only entity 1 (it skips), entity 2 is absent (culled).
        assert!(cache.touch_cached(1).is_some());
        cache.end_frame();
        assert!(
            !cache.must_sample(1, false),
            "the touched entry survives eviction",
        );
        assert!(
            cache.must_sample(2, false),
            "the untouched entry is evicted → its next appearance forces a resample",
        );
    }

    #[test]
    fn palette_cache_clear_empties_for_level_load() {
        // The level-load clear empties the cache wholesale — entity seeds are not
        // stable across levels, so a stale run must not survive.
        let mut cache = PaletteCache::default();
        cache.store(9, &palette_run(1.0, 3));
        cache.end_frame();
        assert!(!cache.must_sample(9, false), "entry present before clear");
        cache.clear();
        assert!(
            cache.must_sample(9, false),
            "clear empties the cache → a miss forces a resample",
        );
    }
}
