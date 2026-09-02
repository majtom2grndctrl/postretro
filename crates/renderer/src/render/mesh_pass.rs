// Skinned-mesh render pass: forward draw of many instances of many skinned
// models against a shared bone palette.
// See: context/lib/rendering_pipeline.md §9
//
// Mirrors the shape of `crate::render::smoke::SmokePass` (`new` builds the
// pipeline + layouts; a model cache keyed by handle mirrors `SmokePass::sheets`;
// `render_frame` writes the per-frame buffers + records the draws). Owns ALL
// wgpu for skinned meshes — `postretro_model` stays wgpu-free.
//
// Binding plan (forward, non-shadow):
//   * group 0 = camera (shared renderer-owned camera uniform / bind group)
//   * group 1 = material (the `build_material_bind_group` bind group — the SH-lit
//               fragment samples diffuse + character-model sampler from this group)
//   * group 2 = runtime direct lighting + shadow receipt (fully allocated b0–b8):
//               b0 dynamic-tier records plus promoted static records, b1
//               per-light influence volumes, b2
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

use postretro_model::anim::{BlendSource, LocalTrs};
use postretro_model::mesh::SkinnedMesh;
use postretro_model::sample_params::{ClipSample, FadeSource, MeshSampleParams, SnapshotTag};
use postretro_model::skeleton::{AnimationClip, Skeleton};
use postretro_model::{BonePaletteEntry, ModelHandle};
use postretro_render_cpu::mesh_instances::{
    MAX_INSTANCES, MAX_PALETTE_ENTRIES, MeshFramePlan, MeshPaletteCacheKey,
};

use super::UNIFORM_SIZE;

/// Byte size of one `BonePaletteEntry` (mat4x4<f32> = 64 B).
const BONE_PALETTE_ENTRY_SIZE: usize = std::mem::size_of::<BonePaletteEntry>();

/// Per-instance SSBO entry: model matrix (64 B) + base index and receiver-bias
/// scale packed into a trailing `vec4<u32>` (16 B) = 80 B. Matches the WGSL
/// `Instance` std430 struct (base at byte 64, scale at byte 68). The instance
/// SSBO is an array of these, read by
/// `@builtin(instance_index)`; the same shape drops into a future
/// `multi_draw_indexed_indirect` per-instance buffer without a contract change.
const INSTANCE_ENTRY_SIZE: usize = 80;

/// Pack one instance's SSBO bytes (model matrix column-major + base index +
/// receiver-bias scale). The scale occupies `Instance.base_and_pad.y` as f32
/// bits, preserving the WGSL record's 80-byte stride.
fn build_instance_entry(
    model: glam::Mat4,
    base_index: u32,
    shadow_bias_scale: f32,
) -> [u8; INSTANCE_ENTRY_SIZE] {
    let mut bytes = [0u8; INSTANCE_ENTRY_SIZE];
    let cols = model.to_cols_array();
    for (i, v) in cols.iter().enumerate() {
        let off = i * 4;
        bytes[off..off + 4].copy_from_slice(&v.to_ne_bytes());
    }
    // Base index at offset 64 (x) and bias scale bits at 68 (y). z/w stay zero.
    bytes[64..68].copy_from_slice(&base_index.to_ne_bytes());
    bytes[68..72].copy_from_slice(&shadow_bias_scale.to_bits().to_ne_bytes());
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
    include_str!("../shaders/light_falloff.wgsl"),
    "\n",
    include_str!("../shaders/light_eval.wgsl"),
    "\n",
    include_str!("../shaders/shadow_sample.wgsl"),
    "\n",
    include_str!("../shaders/shadow_sample_static_cache.wgsl"),
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

/// Mesh-side group-2 params uniform (binding 4): runtime-light count, the frame's
/// render-clock time, and `light_term_mask`. `time` is the SAME render-clock
/// value the renderer uploads to forward `Uniforms.time` that frame (the renderer
/// caches it and threads it in), so the scripted-light animated curves the mesh
/// loop evaluates stay phase-coherent with the forward pass and the CPU light
/// bridge. `light_term_mask` is the per-frame snapshot that the renderer writes
/// to forward `Uniforms.light_term_mask`, so mesh ambient and runtime-direct
/// gates agree with the world path. `ambient_floor` is the SAME constant ambient fill the
/// renderer uploads to forward `Uniforms.ambient_floor` that frame; the mesh
/// fragment shader adds it once as an additive fill so shadowed mesh faces lift
/// with the diagnostics slider exactly as world surfaces do (see forward.wgsl's
/// ambient-floor term). Its second std140 row carries the dynamic-prefix count
/// used to identify promoted static records in the shader.
#[repr(C)]
#[derive(Clone, Copy)]
struct MeshLightParams {
    light_count: u32,
    time: f32,
    light_term_mask: u32,
    ambient_floor: f32,
    dynamic_light_count: u32,
    _pad: [u32; 3],
}

/// Byte size of the group-2 params uniform (`MeshLightParams`, 32 B).
const MESH_LIGHT_PARAMS_SIZE: u64 = std::mem::size_of::<MeshLightParams>() as u64;

/// Serialize `MeshLightParams` to its 32-byte std140 upload. `dynamic_light_count`
/// sits at bytes 16..20; the remaining second row stays explicit zero padding.
/// Split out from
/// `write_light_params` so the byte layout can be asserted GPU-free in tests.
fn build_light_params_bytes(params: MeshLightParams) -> Vec<u8> {
    [
        params.light_count.to_ne_bytes(),
        params.time.to_ne_bytes(),
        params.light_term_mask.to_ne_bytes(),
        params.ambient_floor.to_ne_bytes(),
        params.dynamic_light_count.to_ne_bytes(),
        params._pad[0].to_ne_bytes(),
        params._pad[1].to_ne_bytes(),
        params._pad[2].to_ne_bytes(),
    ]
    .concat()
}

/// GPU-free builder for the mesh group-2 (runtime direct lighting + shadow
/// receipt) BGL entries. Single source of truth: `MeshPass::new` builds the layout
/// from this, and the headless `mesh_group2_bgl_matches_shader_bindings` test
/// re-derives the binding map and per-stage storage budget from the SAME entries —
/// so a drift in either the shader's group-2 declarations or the budget fails CI
/// before a real GPU would reject the pipeline. Pinned binding map (mirrors
/// `skinned_mesh.wgsl` group 2 and rendering_pipeline.md §9, §10):
///   b0 dynamic-tier records plus promoted static records, b1 per-light
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
        // b0: dynamic-tier records first, promoted static records appended.
        storage_entry(0),
        // b1: per-light influence volumes.
        storage_entry(1),
        // b2: scripted-animation descriptors (forward group-3 b13).
        storage_entry(2),
        // b3: scripted-animation curve samples (forward group-3 b12).
        storage_entry(3),
        // b4: mesh-side params uniform (light count, time, light-term mask).
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
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 9,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    });
    if cube_array_supported {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 10,
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
pub(super) struct UploadedModel {
    pub(super) vertex_buffer: wgpu::Buffer,
    pub(super) index_buffer: wgpu::Buffer,
    /// Per-submesh material bind group (group 1) + its `start..end` range into
    /// the merged index buffer, in submesh order. Distinct keys are deduped
    /// upstream, so submeshes reusing a material share a (cloned) bind group.
    pub(super) submeshes: Vec<(wgpu::BindGroup, std::ops::Range<u32>)>,
    /// Skeleton for pose sampling. Joint count == `skeleton.joints.len()` is the
    /// per-instance palette run length.
    pub(super) skeleton: Skeleton,
    /// CPU-only presentation modifiers resolved beside the skeleton at palette
    /// sampling time. Never copied into the per-instance payload.
    pose_stack: postretro_model::pose_modifier::PoseModifierStack,
}

pub use postretro_render_cpu::mesh_pass::ClipMetadata;

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
/// Lifecycle: a capture instruction installs (or refreshes) an entry; end-of-frame
/// retention keeps only entries whose entity has an active matching snapshot fade
/// in the current plan. Culled, budget-dropped, despawned, and no-mesh frames
/// evict stale entries. Emptied wholesale at level load by
/// [`MeshPass::clear_for_level_load`].
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
        capture: &postretro_model::sample_params::CaptureInstruction,
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

        postretro_model::anim::capture_blend(
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

    /// End-of-frame retention. A snapshot survives only when the same entity had
    /// an active snapshot fade whose tag matched the stored capture this frame.
    fn retain_active_snapshot_fades(&mut self, active_fades: &HashMap<u32, SnapshotTag>) {
        if active_fades.is_empty() {
            self.entries.clear();
            return;
        }

        self.entries
            .retain(|seed, stored| active_fades.get(seed).is_some_and(|tag| *tag == stored.tag));
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
/// keyed by the collector's stable per-instance identity (GPU-free data logic,
/// unit-testable without a `wgpu::Device`). On a RESAMPLE frame the
/// pass samples the pose and refreshes the cached run; on a SKIPPED frame it
/// re-uploads the cached run with no sampling. A cache MISS forces a resample
/// that frame regardless of the collector's flag (the collector cannot see
/// renderer-side cache state), so a culled instance re-entering view never shows
/// a stale pose.
///
/// Eviction: entries not touched in a frame are dropped at [`end_frame`], so the
/// cache is bounded by the frame's planned-instance count (≤ `MAX_INSTANCES`
/// entries, ≤ `MAX_PALETTE_ENTRIES` total slots). Emptied wholesale at level load
/// by [`PaletteCache::clear`] (instance identities are not stable across levels).
///
/// [`end_frame`]: PaletteCache::end_frame
#[derive(Debug, Default)]
struct PaletteCache {
    entries: HashMap<MeshPaletteCacheKey, CachedPalette>,
}

impl PaletteCache {
    /// Resolve whether this instance must sample this frame. Returns `true` when
    /// the collector asked to resample OR the cache misses (no entry for this
    /// key) — the miss upgrade is what keeps a re-entering instance from showing
    /// a stale pose. A `false` return means a valid cached run exists and the
    /// collector cleared the instance to skip.
    fn must_sample(&self, key: MeshPaletteCacheKey, collector_resample: bool) -> bool {
        collector_resample || !self.entries.contains_key(&key)
    }

    /// Store a freshly sampled run for `key`, reusing the entry's `Vec` storage
    /// in place (cleared + extended — no realloc on a steady-state hit). Marks the
    /// entry seen this frame so eviction keeps it.
    fn store(&mut self, key: MeshPaletteCacheKey, run: &[BonePaletteEntry]) {
        let entry = self.entries.entry(key).or_default();
        entry.run.clear();
        entry.run.extend_from_slice(run);
        entry.seen_this_frame = true;
    }

    /// The cached run for `key` on a SKIP frame, or `None` if absent. Also marks
    /// the entry seen so a skipped instance is not evicted. (A skip only reaches
    /// here when `must_sample` already returned `false`, i.e. the entry exists.)
    fn touch_cached(&mut self, key: MeshPaletteCacheKey) -> Option<&[BonePaletteEntry]> {
        let entry = self.entries.get_mut(&key)?;
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
    loop_policy: postretro_model::anim::Loop,
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

/// Per-instance values that select and modify one palette sample. Grouping
/// these keeps the sampling seam aligned with the render-plan payload while the
/// model-owned skeleton and modifier stack remain separate cache data.
struct InstancePoseSample<'a> {
    params: &'a MeshSampleParams,
    pose_inputs: Option<&'a postretro_entities::PoseInputs>,
    seed: u32,
}

/// Sample an active modified pose whose primary animation clip is unresolved.
/// The skeleton rest pose becomes the entered endpoint, so a model whose
/// primary clip failed to resolve can still consume presentation inputs
/// (in production `pose_inputs` is only produced for meshes with an
/// animation block, so this is that block's clip going unresolved — not a
/// model authored with no animation at all), and an in-flight fade retains
/// its normal outgoing→incoming weight convention.
fn sample_modified_rest_instance<'a>(
    instance: &InstancePoseSample<'_>,
    pose_inputs: &postretro_entities::PoseInputs,
    skeleton: &Skeleton,
    pose_stack: &postretro_model::pose_modifier::PoseModifierStack,
    store: &SnapshotStore,
    resolve_clip: &impl Fn(usize) -> Option<&'a AnimationClip>,
    out: &mut Vec<BonePaletteEntry>,
) {
    let sample = instance.params;
    let rest = BlendSource::Rest;
    let Some(fade) = sample.fade else {
        postretro_model::anim::sample_rest_pose_modified(skeleton, pose_stack, pose_inputs, out);
        return;
    };

    let outgoing = match fade.from {
        FadeSource::Clip(leg) => {
            clip_blend_source(&leg, resolve_clip).map(|source| source.as_blend_source())
        }
        FadeSource::Snapshot { tag, fallback } => store
            .matching(instance.seed, tag)
            .map(BlendSource::Snapshot)
            .or_else(|| {
                clip_blend_source(&fallback, resolve_clip).map(|source| source.as_blend_source())
            }),
    };

    match outgoing {
        Some(outgoing) => postretro_model::anim::sample_blended_modified(
            &outgoing,
            &rest,
            fade.weight,
            skeleton,
            pose_stack,
            Some(pose_inputs),
            out,
        ),
        None => {
            postretro_model::anim::sample_rest_pose_modified(skeleton, pose_stack, pose_inputs, out)
        }
    }
}

/// Sample one instance's pose into `out` per its resolved [`MeshSampleParams`]:
/// explicit rest pose, a single clip, a clip→clip blend, or a snapshot→clip
/// blend. Always writes a full run into `out` and returns `true`, so the caller's
/// palette write covers the whole region.
///
/// When the primary clip does not resolve, an active modifier stack samples the
/// skeleton rest pose so a model whose primary clip is unresolved can still
/// consume pose inputs (in production, only meshes with an animation block
/// carry `pose_inputs`, so this covers that block's clip going unresolved, not
/// a model authored with no animation at all). Otherwise `out` is filled with
/// one identity (bind-pose) matrix per joint
/// rather than left untouched. The exact identity fallback prevents an inactive
/// unsampled run from inheriting another instance's densely repacked matrices.
///
/// Fade resolution mirrors the collector's intent but degrades safely at the GPU
/// seam: a [`FadeSource::Snapshot`] whose store entry is missing (capture frame
/// culled) falls back to its `(clip, time)` pair — a `"snap"`-equivalent hard
/// blend the game layer never saw.
fn sample_instance<'a>(
    instance: InstancePoseSample<'_>,
    skeleton: &Skeleton,
    pose_stack: &postretro_model::pose_modifier::PoseModifierStack,
    store: &SnapshotStore,
    resolve_clip: &impl Fn(usize) -> Option<&'a AnimationClip>,
    out: &mut Vec<BonePaletteEntry>,
) -> bool {
    let sample = instance.params;
    let pose_inputs = instance.pose_inputs;
    let seed = instance.seed;
    if sample.is_rest_pose() {
        if let Some(inputs) = pose_inputs.filter(|_| !pose_stack.is_empty()) {
            postretro_model::anim::sample_rest_pose_modified(skeleton, pose_stack, inputs, out);
        } else {
            postretro_model::anim::sample_rest_pose(skeleton, out);
        }
        return true;
    }

    // A model whose primary clip is unresolved can still consume external
    // presentation inputs: modify its skeleton rest pose, retaining any
    // outgoing fade source. Pose inputs are presentation data and may also be
    // supplied by stateless holders, so this fallback is not limited to an
    // animation block whose clip failed to resolve. Without both an active
    // stack and inputs, preserve the historical exact identity fallback rather
    // than composing the model's rest/inverse-bind data.
    let Some(primary) = clip_blend_source(&sample.primary, resolve_clip) else {
        if let Some(inputs) = pose_inputs.filter(|_| !pose_stack.is_empty()) {
            sample_modified_rest_instance(
                &instance,
                inputs,
                skeleton,
                pose_stack,
                store,
                resolve_clip,
                out,
            );
            return true;
        }
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
        postretro_model::anim::sample_clip_looped_modified(
            primary.clip,
            skeleton,
            primary.time,
            primary.loop_policy,
            pose_stack,
            pose_inputs,
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
                postretro_model::anim::sample_clip_looped_modified(
                    primary.clip,
                    skeleton,
                    primary.time,
                    primary.loop_policy,
                    pose_stack,
                    pose_inputs,
                    out,
                );
                return true;
            };
            postretro_model::anim::sample_blended_modified(
                &from.as_blend_source(),
                &primary_src,
                fade.weight,
                skeleton,
                pose_stack,
                pose_inputs,
                out,
            );
        }
        FadeSource::Snapshot { tag, fallback } => {
            match store.matching(seed, tag) {
                Some(pose) => {
                    postretro_model::anim::sample_blended_modified(
                        &BlendSource::Snapshot(pose),
                        &primary_src,
                        fade.weight,
                        skeleton,
                        pose_stack,
                        pose_inputs,
                        out,
                    );
                }
                None => {
                    // Store miss (capture frame culled): degrade to the fallback
                    // clip — a `"snap"`-equivalent blend the game layer never saw.
                    match clip_blend_source(&fallback, resolve_clip) {
                        Some(from) => postretro_model::anim::sample_blended_modified(
                            &from.as_blend_source(),
                            &primary_src,
                            fade.weight,
                            skeleton,
                            pose_stack,
                            pose_inputs,
                            out,
                        ),
                        None => postretro_model::anim::sample_clip_looped_modified(
                            primary.clip,
                            skeleton,
                            primary.time,
                            primary.loop_policy,
                            pose_stack,
                            pose_inputs,
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
    pub(super) depth_pipeline: wgpu::RenderPipeline,

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
    /// [`MeshPass::new`]): b0 dynamic-tier records plus promoted static records,
    /// b1 per-light influence volumes, b2 scripted-animation descriptors, b3
    /// scripted-animation curve samples, b4 the mesh-side params uniform. b0–b3 alias the SAME
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
    /// frame's forward `time`, and the captured forward `light_term_mask`.
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
    pub(super) models: HashMap<ModelHandle, UploadedModel>,

    /// Per-model LOCAL-space AABB, keyed by handle, populated at `insert_model`
    /// from the CPU `SkinnedMesh::bounds`. Kept on the cache (not in
    /// `UploadedModel`, which stays GPU-only) so the GPU-free frame planner can
    /// stamp each `PlannedInstance` with its model's bound for the CPU per-light
    /// caster cull — the renderer's GPU draw never reads it.
    pub(super) model_bounds: HashMap<ModelHandle, postretro_render_data::cone_frustum::Aabb>,

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

    /// Optional per-frame pose-sampling measurement. `Some` only when
    /// `POSTRETRO_GPU_TIMING=1` (cached at construction so the hot path never
    /// touches the environment), so the unmeasured frame pays nothing beyond an
    /// `Option` check. Accumulates the CPU cost of the per-instance `sample_clip`
    /// loop and logs it rate-limited — a profiling gate to measure per-instance
    /// pose-sampling cost at representative wave counts and decide whether a baked
    /// pose buffer is worth the complexity over per-frame CPU sampling.
    pose_sample_stats: Option<PoseSampleStats>,
}

/// CPU animation assets moved into the mesh cache together at model install.
/// Clips remain in the cache-side lookup map; skeleton and modifier stack stay
/// paired on the uploaded-model entry used by palette sampling.
pub(super) struct ModelAnimationData {
    pub(super) skeleton: Skeleton,
    pub(super) clips: Vec<AnimationClip>,
    pub(super) pose_stack: postretro_model::pose_modifier::PoseModifierStack,
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

        // Group 2: runtime direct lighting. Binding map PINNED across both M10
        // mesh specs — b0 dynamic-tier records plus promoted static records
        // appended by the renderer, b1 per-light influence volumes, b2 scripted-animation
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
                // (postretro-model stays wgpu-free). Offsets:
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
                    array_stride: std::mem::size_of::<postretro_model::mesh::SkinnedVertex>()
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
                    format: super::SCENE_COLOR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        let depth_pipeline = super::mesh_depth::create_skinned_depth_pipeline(
            device,
            light_space_bgl,
            &instance_bind_group_layout,
            shadow_depth_format,
        );

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

        // The shared group-0 layout does not require a full `FrameUniforms`
        // payload for the mesh shader (it reads only `view_proj`), but retain the
        // existing 128-byte allocation so the bind group stays exactly compatible
        // with the renderer-wide camera contract.
        let viewmodel_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Viewmodel View-Projection Uniform"),
            size: UNIFORM_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let viewmodel_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Viewmodel View-Projection Bind Group"),
            layout: camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewmodel_uniform_buffer.as_entire_binding(),
            }],
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
            viewmodel_uniform_buffer,
            viewmodel_uniform_bind_group,
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

    /// (Re)build the group-2 runtime-direct light bind group over the renderer's
    /// runtime light buffers. Called once after geometry installs and again on any
    /// reallocation of these buffers (level load), mirroring how the renderer
    /// rebuilds its forward `lighting_bind_group`. The buffers are owned by the
    /// renderer and bound here by reference; b4 is this pass's own
    /// `light_params_buffer`.
    ///
    /// The runtime-light buffer's dynamic prefix is the renderer's
    /// `filter_dynamic_lights` output; promoted static records may be appended each
    /// frame and are loop-bound by the params uniform. Do not bind the raw
    /// shadow-candidate set here. `influence` is the matching per-light
    /// influence-volume buffer. `scripted_descriptors` is forward's group-3 b13
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
        promoted_spot_cache: &wgpu::TextureView,
        promoted_cube_cache: Option<&wgpu::TextureView>,
    ) {
        assert_eq!(
            point_shadow_cube.is_some(),
            self.cube_array_supported,
            "mesh group-2 cube view must be Some iff the BGL carries the b8 cube \
             entry (cube_array_supported) — the Some-iff-layout invariant",
        );
        assert_eq!(
            promoted_cube_cache.is_some(),
            self.cube_array_supported,
            "mesh group-2 promoted cube cache must be Some iff the BGL carries the b10 cube entry",
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
        entries.push(wgpu::BindGroupEntry {
            binding: 9,
            resource: wgpu::BindingResource::TextureView(promoted_spot_cache),
        });
        if let Some(cube_view) = promoted_cube_cache {
            entries.push(wgpu::BindGroupEntry {
                binding: 10,
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
    /// `light_count`, the frame's render-clock `time`, and `light_term_mask`.
    /// `time` MUST be the SAME value the renderer wrote to forward `Uniforms.time`
    /// this frame (the renderer caches it in `update_per_frame_uniforms` and
    /// threads it here), so the scripted-light curves the mesh loop evaluates stay
    /// phase-coherent with the forward pass. `light_term_mask` MUST be the SAME
    /// captured snapshot the renderer writes to forward `Uniforms.light_term_mask`,
    /// so the mesh ambient and dynamic-direct gates land with the world path.
    /// `ambient_floor` MUST be the SAME value the renderer writes to forward
    /// `Uniforms.ambient_floor` this frame, so shadowed mesh faces lift with the
    /// diagnostics ambient-floor slider exactly as world surfaces do.
    pub fn write_light_params(
        &self,
        queue: &wgpu::Queue,
        light_count: u32,
        dynamic_light_count: u32,
        time: f32,
        light_term_mask: u32,
        ambient_floor: f32,
    ) {
        let bytes = build_light_params_bytes(MeshLightParams {
            light_count,
            dynamic_light_count,
            time,
            light_term_mask,
            ambient_floor,
            _pad: [0; 3],
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
    pub(super) fn insert_model(
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
        // Stash the CPU-side local bound for the planner (drives the per-light
        // caster cull). Lives on the cache, NOT in `UploadedModel` — the GPU draw
        // never reads it.
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

    /// The cached local-space bound for a skinned model. Returns a zero box when
    /// the model is absent, matching the frame planner's degradation path.
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
    /// ([`postretro_model::anim::sample_clip_looped`]), a clip→clip blend, or a
    /// snapshot→clip blend. All sample times arrive in the params (the collector
    /// computed them from the animation clock), so the pass holds no render-clock
    /// of its own. The optional pose-sampling measurement uses an `Instant`, not
    /// the render clock.
    ///
    /// Snapshot-store lifecycle: a capture installs/refreshes an entry (idempotent
    /// by tag), then frame-end retention keeps only entries for entities with an
    /// active matching snapshot fade in the current plan. A snapshot fade whose
    /// store entry is missing (its capture frame was culled / budget-dropped)
    /// degrades to the fallback clip — a discontinuity no one saw because the
    /// entity was not drawn at the interrupt instant.
    ///
    /// Cull is the caller's job — see [`postretro_render_cpu::mesh_pass::mesh_visible`];
    /// the plan already holds only surviving, in-budget instances.
    pub fn plan_and_upload(
        &mut self,
        queue: &wgpu::Queue,
        plans: &[&MeshFramePlan],
        scratch: &mut Vec<BonePaletteEntry>,
    ) {
        if plans.iter().all(|plan| plan.groups.is_empty()) {
            self.snapshot_store
                .retain_active_snapshot_fades(&HashMap::new());
            self.palette_cache.end_frame();
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
        let mut active_snapshot_fades: HashMap<u32, SnapshotTag> = HashMap::new();

        for plan in plans {
            for group in &plan.groups {
                let Some(model) = models.get(&group.model) else {
                    // Planner only emits groups for cached models, but guard anyway.
                    continue;
                };
                let clips = model_clips.get(&group.model);
                let resolve_clip = |idx: usize| clips.and_then(|c| c.get(idx));

                for (i, inst) in group.instances.iter().enumerate() {
                    let instance_index = group.instance_offset as usize + i;
                    let entry = build_instance_entry(
                        inst.transform,
                        inst.palette_base,
                        inst.shadow_bias_scale,
                    );
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

                    // Retention mark: the capture above just installed the matching
                    // entry on a capture frame, so this frame's snapshot fade can
                    // sample it below and keep it at frame end. Missing/stale tags are
                    // left unmarked and will fall back during sampling, then evict.
                    if let Some(FadeSource::Snapshot { tag, .. }) = inst.sample.fade.map(|f| f.from)
                    {
                        if snapshot_store.matching(inst.phase_seed, tag).is_some() {
                            active_snapshot_fades.insert(inst.phase_seed, tag);
                        }
                    }

                    // Time-slicing decision. Sample when the collector asked
                    // for a resample OR the cache misses (a re-entering instance with
                    // no cached run must sample, never show a stale pose). Otherwise
                    // re-upload the cached run with no sampling.
                    if palette_cache.must_sample(inst.palette_cache_key, inst.resample) {
                        // RESAMPLE: sample this instance's pose, upload it, and refresh
                        // the cache with the freshly sampled run.
                        let started = measure.then(std::time::Instant::now);
                        let sampled = sample_instance(
                            InstancePoseSample {
                                params: &inst.sample,
                                pose_inputs: inst.pose_inputs.as_ref(),
                                seed: inst.phase_seed,
                            },
                            &model.skeleton,
                            &model.pose_stack,
                            snapshot_store,
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
                            palette_cache.store(inst.palette_cache_key, scratch);
                        }
                    } else if let Some(cached) = palette_cache.touch_cached(inst.palette_cache_key)
                    {
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
        }

        // Evict cache entries not touched this frame, so the cache holds exactly
        // this frame's planned instances (bounded by MAX_INSTANCES / the palette
        // budget) and a culled-out entity's stale run does not linger.
        palette_cache.end_frame();

        // Evict snapshots after sampling, not before: a capture frame's snapshot
        // fade must resolve against the capture that landed earlier in this pass.
        snapshot_store.retain_active_snapshot_fades(&active_snapshot_fades);

        // Fold this frame's pose-sampling tallies in and flush the rate-limited
        // line when the interval elapses. Only `Some` under POSTRETRO_GPU_TIMING.
        if let Some(stats) = pose_sample_stats.as_mut() {
            stats.record_frame(sampled_instances, sample_elapsed);
        }
    }

    /// Upload the tight view-projection used exclusively by the viewmodel pass.
    /// `skinned_mesh.wgsl` reads only the leading `mat4x4`, while the buffer keeps
    /// the existing group-0 allocation size/layout for bind-group compatibility.
    pub(super) fn write_viewmodel_view_projection(
        &self,
        queue: &wgpu::Queue,
        view_projection: glam::Mat4,
    ) {
        let mut data = [0u8; UNIFORM_SIZE];
        for (index, value) in view_projection.to_cols_array().iter().enumerate() {
            let offset = index * std::mem::size_of::<f32>();
            data[offset..offset + std::mem::size_of::<f32>()].copy_from_slice(&value.to_ne_bytes());
        }
        queue.write_buffer(&self.viewmodel_uniform_buffer, 0, &data);
    }

    pub(super) fn viewmodel_uniform_bind_group(&self) -> &wgpu::BindGroup {
        &self.viewmodel_uniform_bind_group
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
    /// The mesh collector can include shadow-only instances for selected static
    /// lights. Filter `forward_visible` so those instances feed shadow depth but
    /// skip the color pass, batching contiguous visible runs so the common
    /// all-visible frame issues one draw per group/submesh.
    pub fn record_draws(&self, pass: &mut wgpu::RenderPass<'_>, plan: &MeshFramePlan) {
        if plan.groups.is_empty() {
            return;
        }

        pass.set_pipeline(&self.pipeline);
        // Group 2 (runtime direct lighting): the runtime light buffers + the
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
            // One instanced draw per maximal contiguous run of `forward_visible`
            // instances. Non-forward synthetic entries break a run; an all-visible
            // group collapses to one run. The base instance is
            // the run's absolute SSBO offset — the palette base still rides each
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
}

/// One instanced draw per submesh over a contiguous `range` of `forward_visible`
/// instances. Sets group 1 (material) per submesh; group 3 (palette + instance
/// SSBO) is bound once per frame by the caller.
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
    use crate::render::{
        pipeline_layout::{material_bind_group_layout_entries, uniform_bind_group_layout_entries},
        sh_volume,
    };
    use glam::Vec3;

    fn extract_wgsl_fn<'a>(source: &'a str, name: &str) -> &'a str {
        let needle = format!("fn {name}");
        let start = source
            .find(&needle)
            .unwrap_or_else(|| panic!("shader should declare fn {name}"));
        let body_start = source[start..]
            .find('{')
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("fn {name} should have a body"));
        let mut depth = 0i32;
        for (offset, ch) in source[body_start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &source[start..body_start + offset + 1];
                    }
                }
                _ => {}
            }
        }
        panic!("fn {name} should close its body");
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
        let depth_body =
            extract_skin_matrix(crate::render::mesh_depth::SKINNED_DEPTH_SHADER_SOURCE);

        assert_eq!(
            mesh_body, depth_body,
            "skin_matrix body in skinned_depth.wgsl must be byte-identical to the copy \
             in skinned_mesh.wgsl — update both when changing the skinning kernel",
        );
    }

    #[test]
    fn instance_entry_packs_model_base_index_and_shadow_bias_scale() {
        // Guard the WGSL layout contract: Instance { model: mat4x4<f32>,
        // base_and_pad: vec4<u32> } — model at offset 0 (64 B), base_index at
        // offset 64, bias scale bits at offset 68, total 80 B. If either side
        // (Rust packer or WGSL struct) is edited silently, this assertion fires.
        assert_eq!(
            INSTANCE_ENTRY_SIZE, 80,
            "INSTANCE_ENTRY_SIZE must match WGSL Instance total (80 B)",
        );

        let m = glam::Mat4::from_translation(Vec3::new(4.0, 5.0, 6.0));
        let bytes = build_instance_entry(m, 7, 2.5);
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

        // base_and_pad.y carries the authored receiver-bias scale as f32 bits.
        let shadow_bias_scale = f32::from_ne_bytes(bytes[68..72].try_into().unwrap());
        assert!(
            (shadow_bias_scale - 2.5).abs() < f32::EPSILON,
            "bias scale must be packed at byte offset 68"
        );

        // The two remaining padding lanes stay zero.
        assert_eq!(
            &bytes[72..80],
            &[0u8; 8],
            "padding bytes 72..80 must be zero"
        );
    }

    // Guard the group-2 params uniform layout contract: `MeshLightParams` is eight
    // u32/f32 lanes (32 B std140), mirrored by the WGSL struct at group 2 binding 4.
    // Its first row holds the total count, time, term mask, and ambient floor;
    // `dynamic_light_count` begins the explicit padded second row at byte 16. That
    // count separates dynamic-prefix records from appended promoted static records,
    // so a silent layout edit on either side must fail here.
    #[test]
    fn mesh_light_params_is_thirty_two_bytes() {
        assert_eq!(
            MESH_LIGHT_PARAMS_SIZE, 32,
            "MeshLightParams must be 32 B to match the std140 WGSL uniform",
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
            light_term_mask: 0x7F,
            ambient_floor,
            dynamic_light_count: 2,
            _pad: [0; 3],
        });
        assert_eq!(bytes.len(), 32, "serialized MeshLightParams must be 32 B");
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
            &0x7Fu32.to_le_bytes(),
            "light_term_mask at 8..12",
        );
        assert_eq!(
            &bytes[16..20],
            &2u32.to_le_bytes(),
            "dynamic_light_count at 16..20",
        );
        assert_eq!(&bytes[20..], &[0; 12], "second-row padding stays zero");
    }

    #[test]
    fn mesh_light_params_wgsl_layout_matches_rust_upload() {
        let module = naga::front::wgsl::parse_str(SKINNED_MESH_SHADER_SOURCE)
            .expect("composed skinned mesh shader should parse as WGSL");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("composed skinned mesh shader should pass Naga validation");

        let (span, members) = module
            .types
            .iter()
            .find_map(|(_handle, ty)| match (&ty.name, &ty.inner) {
                (Some(name), naga::TypeInner::Struct { span, members, .. })
                    if name == "MeshLightParams" =>
                {
                    Some((*span, members))
                }
                _ => None,
            })
            .expect("skinned mesh shader should declare MeshLightParams");

        assert_eq!(span as usize, MESH_LIGHT_PARAMS_SIZE as usize);
        assert_eq!(
            members
                .iter()
                .map(|member| member.name.as_deref())
                .collect::<Vec<_>>(),
            vec![
                Some("light_count"),
                Some("time"),
                Some("light_term_mask"),
                Some("ambient_floor"),
                Some("dynamic_light_count"),
                Some("_pad0"),
                Some("_pad1"),
                Some("_pad2"),
            ],
        );
        assert_eq!(
            members
                .iter()
                .map(|member| member.offset)
                .collect::<Vec<_>>(),
            vec![0, 4, 8, 12, 16, 20, 24, 28],
        );
    }

    // Headless guard for the mesh group-2 BGL: the entries the pipeline composes
    // from must match the shader's declared group-2 binding map and stay
    // within the per-stage fragment storage-buffer budget. Modeled on
    // `billboard_pipeline_vertex_storage_request_matches_bgl_definitions` — both
    // re-derive the count from the SAME GPU-free BGL builder the layout is built
    // from, so a drift fails CI before a real GPU rejects the pipeline.
    #[test]
    fn mesh_group2_bgl_matches_shader_bindings() {
        // Cube-supported variant carries the full b0..=b8 plus cache b9/b10 map; the dynamic-direct
        // half (b0–b4) is identical in both variants, so assert it here against the
        // cube variant and cover the cube-vs-no-cube b5–b8 split in the dedicated
        // `mesh_group2_shadow_bindings_match_both_cube_variants` test.
        let entries = mesh_light_bind_group_layout_entries(true);

        // Binding map: b0–b3 read-only storage buffers, b4 a uniform. Mirrors the
        // `@group(2) @binding(N)` declarations in skinned_mesh.wgsl exactly.
        let bindings: Vec<u32> = entries.iter().map(|e| e.binding).collect();
        assert_eq!(
            bindings,
            vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
            "cube-supported mesh group-2 BGL must declare bindings b0..=b10 in order",
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
    // group-2 b0 carries dynamic-tier records first, then promoted static records
    // appended by the renderer. This lives in the actual bind-group wiring
    // (`rebuild_light_bind_group`), which takes a `lights` slice and cannot be
    // exercised without a GPU. This test does NOT verify that wiring; it is a
    // string pin that keeps the DOCUMENTED contract present and self-consistent.
    // If a future edit deletes or contradicts that documented contract, this
    // fails — flagging the docs for review. It would NOT catch a wiring bug that
    // rebound b0 to the wrong buffer while leaving the doc strings intact; that is
    // the GPU layer, verified by running the engine (testing_guide §3).
    #[test]
    fn skinned_mesh_b0_count_split_contract_is_documented() {
        // The shader's b0 declaration documents the count-split invariant.
        let shader_src = include_str!("../shaders/skinned_mesh.wgsl");
        assert!(
            shader_src.contains("@group(2) @binding(0) var<storage, read> lights"),
            "skinned_mesh.wgsl must declare runtime light records at group-2 b0",
        );
        assert!(
            shader_src.contains("promoted static lights appended"),
            "the b0 declaration must document the dynamic-first/promoted-static-appended split",
        );
        // The wiring contract (`rebuild_light_bind_group`) names the count split
        // as the REQUIRED b0 source.
        let rust_src = include_str!("mesh_pass.rs");
        assert!(
            rust_src.contains("dynamic-tier records plus promoted static records"),
            "rebuild_light_bind_group must pin the count-split runtime light records as the b0 source",
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

    // The mesh dynamic-direct loop must use the same LightTermMask bit as the
    // world path. The mesh's group-2 params carry the raw mask, while forward
    // declares the shared bit as a named WGSL constant.
    #[test]
    fn mesh_dynamic_gate_uses_light_term_mask_bit_five_like_forward() {
        let mesh_src = include_str!("../shaders/skinned_mesh.wgsl");
        let forward_src = include_str!("../shaders/forward.wgsl");
        assert!(
            mesh_src.contains("let use_dynamic = (light_terms & 0x20u) != 0u;"),
            "skinned_mesh.wgsl must gate its dynamic loop with LightTermMask bit 5",
        );
        assert!(
            forward_src.contains("const LIGHT_TERM_DYNAMIC_DIRECT: u32 = 0x20u;")
                && forward_src
                    .contains("let use_dynamic = (light_terms & LIGHT_TERM_DYNAMIC_DIRECT) != 0u;"),
            "forward.wgsl must keep LightTermMask bit 5 as its dynamic-direct gate",
        );
    }

    #[test]
    fn skinned_mesh_animated_descriptors_are_limited_to_dynamic_prefix() {
        let dynamic_loop = extract_wgsl_fn(
            include_str!("../shaders/skinned_mesh.wgsl"),
            "accumulate_dynamic_direct",
        );
        assert!(
            dynamic_loop.contains("if i < mesh_light_params.dynamic_light_count {")
                && dynamic_loop.contains("let scripted_desc = scripted_light_descriptors[i];"),
            "promoted static records append after the descriptor-upload prefix and must not read stale descriptor tail bytes",
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
            "@group(2) @binding(9) var promoted_spot_depth_cache: texture_depth_2d_array",
            "@group(2) @binding(10) var promoted_cube_depth_cache: texture_depth_cube_array",
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
        const CUBE_DECLS: [&str; 2] = [
            "@group(2) @binding(8) var point_shadow_cube",
            "@group(2) @binding(10) var promoted_cube_depth_cache",
        ];

        let no_cube = skinned_mesh_shader_source(false);
        for decl in CUBE_DECLS {
            assert!(
                !no_cube.contains(decl),
                "no-cube skinned-mesh source must NOT declare cube binding: {decl}",
            );
        }

        let cube = skinned_mesh_shader_source(true);
        for decl in CUBE_DECLS {
            assert!(
                cube.contains(decl),
                "cube-supported skinned-mesh source must declare cube binding: {decl}",
            );
        }

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

    // Headless guard for the mesh group-2 shadow-receipt bindings across BOTH
    // cube-support variants. b5–b7 and the cache spot b9 are unconditional;
    // b8 and cache cube b10 are
    // present IFF `cube_array_supported` — the `Some`-iff-layout invariant the
    // BGL builder and `rebuild_light_bind_group` both honor. All FRAGMENT-only.
    #[test]
    fn mesh_group2_shadow_bindings_match_both_cube_variants() {
        // No cube support: b5–b7 and b9 present; cube bindings b8/b10 absent.
        let no_cube = mesh_light_bind_group_layout_entries(false);
        let no_cube_bindings: Vec<u32> = no_cube.iter().map(|e| e.binding).collect();
        assert_eq!(
            no_cube_bindings,
            vec![0, 1, 2, 3, 4, 5, 6, 7, 9],
            "no-cube mesh group-2 BGL must omit only cube bindings b8 and b10",
        );

        // Cube support: b8 and b10 carry the pool/cache cube arrays.
        let cube = mesh_light_bind_group_layout_entries(true);
        let cube_bindings: Vec<u32> = cube.iter().map(|e| e.binding).collect();
        assert_eq!(
            cube_bindings,
            vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
            "cube-supported mesh group-2 BGL must declare b0..=b10",
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
        assert!(
            matches!(
                cube[9].ty,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                }
            ),
            "mesh group-2 b9 must be a Depth 2D-array cache texture",
        );
        assert!(
            matches!(
                cube[10].ty,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::CubeArray,
                    multisampled: false,
                }
            ),
            "mesh group-2 b10 must be a Depth cube-array cache texture",
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
    // No cube support: TWO sampled textures — pool/cache spot depth arrays.
    // Cube support: FOUR — both spot arrays plus both cube arrays.
    // Modeled on the billboard storage-count guard
    // (`billboard_pipeline_vertex_storage_request_matches_bgl_definitions`) and the
    // forward sampled-texture guard: re-derive from the SAME GPU-free BGL builder.
    #[test]
    fn mesh_group2_sampled_texture_count_recorded_for_both_cube_variants() {
        // No-cube: b5 pool spot depth + b9 cache spot depth.
        let no_cube = mesh_light_bind_group_layout_entries(false);
        assert_eq!(
            fragment_sampled_textures(&no_cube),
            2,
            "no-cube mesh group-2 must carry two sampled spot depth textures",
        );

        // Cube: b5/b9 spot arrays plus b8/b10 cube arrays = four.
        let cube = mesh_light_bind_group_layout_entries(true);
        assert_eq!(
            fragment_sampled_textures(&cube),
            4,
            "cube-supported mesh group-2 must carry four pool/cache depth textures",
        );

        // The cube variant adds exactly ONE sampled texture over the no-cube
        // variant — the point-shadow cube array (b8) and nothing else.
        assert_eq!(
            fragment_sampled_textures(&cube) - fragment_sampled_textures(&no_cube),
            2,
            "enabling cube support must add the pool/cache cube arrays",
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

    #[test]
    fn skinned_mesh_pipeline_fragment_texture_budget_includes_shared_emissive_binding() {
        let total = |cube_array_supported| {
            let per_group = [
                fragment_sampled_textures(&uniform_bind_group_layout_entries()),
                fragment_sampled_textures(&material_bind_group_layout_entries()),
                fragment_sampled_textures(&mesh_light_bind_group_layout_entries(
                    cube_array_supported,
                )),
                0, // group 3 palette + instance storage buffers
                fragment_sampled_textures(&sh_volume::mesh_bind_group_layout_entries()),
            ];
            (per_group, per_group.iter().sum::<u32>())
        };

        let (cube_groups, cube_total) = total(true);
        assert_eq!(cube_groups, [0, 4, 4, 0, 3]);
        assert_eq!(cube_total, 11);
        assert!(cube_total <= 16);

        let (no_cube_groups, no_cube_total) = total(false);
        assert_eq!(no_cube_groups, [0, 4, 2, 0, 3]);
        assert_eq!(no_cube_total, 9);
        assert!(no_cube_total <= 16);
    }

    // --- Cache-side clip query seam (GPU-free) ----------------------------------
    //
    // The clip-name / metadata lookups back clip-name resolution at level load
    // (main.rs's level-load sweep). They read the cache-side `model_clips` map,
    // split out of `MeshPass` (which needs a `wgpu::Device`) into the GPU-free
    // `clip_by_name` / `clip_metadata` free functions so the seam is testable here
    // without a GPU.

    use postretro_model::skeleton::AnimationClip;

    /// Build a named clip with `duration` and no per-joint tracks. The query seam
    /// keys on name + duration only; track contents are irrelevant to it.
    fn named_clip(name: &str, duration: f32) -> AnimationClip {
        AnimationClip {
            name: name.to_string(),
            duration,
            joints: Vec::new(),
            travel_speed: None,
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
            .join("../model/tests/fixtures/multi_clip/multi_clip.gltf");
        let model =
            postretro_model::gltf_loader::load_model(&fixture).expect("multi-clip fixture loads");

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
    //
    // Paired producer coverage lives game-side in
    // `scripting::systems::mesh_anim`:
    // - `smooth_interrupt_capture_freezes_interrupted_blend_at_entry_stamp`
    // - `smooth_interrupt_capture_can_chain_from_prior_snapshot_tag`
    //
    // Together, those tests prove the game layer emits the CPU-only
    // `CaptureInstruction`/`MeshSampleParams` contract and these tests prove the
    // renderer consumes it without exposing renderer internals across crates.

    use glam::{EulerRot, Mat4, Quat};
    use postretro_model::anim::Loop as AnimLoop;
    use postretro_model::sample_params::{
        CaptureInstruction, ClipSample, FadeSource, MeshFade, MeshSampleParams,
    };
    use postretro_model::skeleton::{Interp, Joint, JointTracks, RestLocal, Skeleton, Track};

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
                translation: Track::new(vec![0.0], vec![Vec3::new(tx, 0.0, 0.0)], Interp::Linear)
                    .expect("valid constant translation track"),
                rotation: Track::new(vec![0.0], vec![Quat::IDENTITY], Interp::Linear)
                    .expect("valid constant rotation track"),
                scale: Track::new(vec![0.0], vec![Vec3::ONE], Interp::Linear)
                    .expect("valid constant scale track"),
            }],
            travel_speed: None,
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

    fn sample_unmodified_instance<'a>(
        params: &MeshSampleParams,
        skeleton: &Skeleton,
        store: &SnapshotStore,
        seed: u32,
        resolve_clip: &impl Fn(usize) -> Option<&'a AnimationClip>,
        out: &mut Vec<BonePaletteEntry>,
    ) -> bool {
        sample_instance(
            InstancePoseSample {
                params,
                pose_inputs: None,
                seed,
            },
            skeleton,
            &postretro_model::pose_modifier::PoseModifierStack::default(),
            store,
            resolve_clip,
            out,
        )
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
        let sampled =
            sample_unmodified_instance(&params, &skel, &store, 1, &|i| clips.get(i), &mut out);
        assert!(sampled);
        assert!(
            (palette_x(&out) - 5.0).abs() < 1.0e-4,
            "single clip → primary pose"
        );
    }

    #[test]
    fn sample_instance_explicit_rest_pose_ignores_clip_zero() {
        let mut skel = one_joint_skeleton();
        skel.joints[0].rest_local.translation = Vec3::new(2.0, 0.0, 0.0);
        let clips = [const_x_clip("clip-zero", 5.0)];
        let store = SnapshotStore::default();
        let mut out = Vec::new();

        let sampled = sample_unmodified_instance(
            &MeshSampleParams::rest(),
            &skel,
            &store,
            1,
            &|i| clips.get(i),
            &mut out,
        );

        assert!(sampled);
        assert!(
            (palette_x(&out) - 2.0).abs() < 1.0e-4,
            "explicit rest selection must not sample clip zero",
        );
    }

    #[test]
    fn sample_instance_applies_pose_stack_for_single_clip_and_all_fade_sources() {
        use postretro_model::pose_modifier::{
            JointMask, ModifierEntry, PoseModifier, PoseModifierStack,
        };

        let skel = one_joint_skeleton();
        let clips = [const_x_clip("from", 0.0), const_x_clip("to", 10.0)];
        let mut mask = JointMask::new();
        assert!(mask.insert(0));
        let stack = PoseModifierStack::new(vec![ModifierEntry {
            mask,
            modifier: PoseModifier::AimPitchBend {
                bend_weights: vec![1.0],
            },
        }]);
        let inputs = postretro_entities::PoseInputs {
            aim_pitch: 0.4,
            aim_yaw: 0.0,
            heading_yaw: 0.0,
            ..Default::default()
        };
        let mut store = SnapshotStore::default();
        let tag = 42;
        store.entries.insert(
            7,
            StoredSnapshot {
                tag,
                pose: vec![LocalTrs {
                    translation: Vec3::ZERO,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                }],
            },
        );

        let cases = [
            MeshSampleParams {
                primary: clip_leg(1, 0.0),
                fade: None,
            },
            MeshSampleParams {
                primary: clip_leg(1, 0.0),
                fade: Some(MeshFade {
                    from: FadeSource::Clip(clip_leg(0, 0.0)),
                    weight: 0.5,
                }),
            },
            MeshSampleParams {
                primary: clip_leg(1, 0.0),
                fade: Some(MeshFade {
                    from: FadeSource::Snapshot {
                        tag,
                        fallback: clip_leg(0, 0.0),
                    },
                    weight: 0.5,
                }),
            },
        ];

        for params in cases {
            let mut out = Vec::new();
            sample_instance(
                InstancePoseSample {
                    params: &params,
                    pose_inputs: Some(&inputs),
                    seed: 7,
                },
                &skel,
                &stack,
                &store,
                &|i| clips.get(i),
                &mut out,
            );
            let (_, rotation, _) =
                Mat4::from_cols_array_2d(&out[0].matrix).to_scale_rotation_translation();
            let (pitch, _, _) = rotation.to_euler(EulerRot::XYZ);
            assert!((pitch + inputs.aim_pitch).abs() <= 1.0e-5);
        }
    }

    #[test]
    fn sample_instance_unresolved_primary_writes_identity_bind_pose_run() {
        // Regression: an unresolved primary clip used to return false and skip the
        // palette write, leaving the densely-repacked run holding another
        // instance's stale matrices. It must now write a clean identity bind-pose
        // run (one per joint) and return true so the caller overwrites the region.
        let mut skel = one_joint_skeleton();
        // Make the authored rest local visibly non-identity: the inactive path
        // must preserve the historical exact identity fill, not compose rest.
        skel.joints[0].rest_local.translation = Vec3::new(3.0, 0.0, 0.0);
        let clips: Vec<AnimationClip> = vec![]; // index 0 absent
        let store = SnapshotStore::default();
        let params = MeshSampleParams {
            primary: clip_leg(0, 0.0),
            fade: None,
        };
        // Pre-fill `out` with a stranger's stale pose to prove it is overwritten.
        let mut out = palette_run(99.0, 1);
        let sampled =
            sample_unmodified_instance(&params, &skel, &store, 1, &|i| clips.get(i), &mut out);
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
    fn sample_instance_unresolved_primary_modifies_rest_pose_when_active() {
        use postretro_model::pose_modifier::{
            JointMask, ModifierEntry, PoseModifier, PoseModifierStack,
        };

        let skel = one_joint_skeleton();
        let clips: Vec<AnimationClip> = vec![];
        let store = SnapshotStore::default();
        let params = MeshSampleParams {
            primary: clip_leg(0, 0.0),
            fade: None,
        };
        let mut mask = JointMask::new();
        assert!(mask.insert(0));
        let stack = PoseModifierStack::new(vec![
            ModifierEntry {
                mask,
                modifier: PoseModifier::UpperLowerSplit {
                    lower_body_mask: JointMask::new(),
                },
            },
            ModifierEntry {
                mask,
                modifier: PoseModifier::AimPitchBend {
                    bend_weights: vec![1.0],
                },
            },
        ]);
        let inputs = postretro_entities::PoseInputs {
            aim_pitch: 0.25,
            aim_yaw: 0.4,
            heading_yaw: 0.0,
            ..Default::default()
        };
        let mut out = Vec::new();

        sample_instance(
            InstancePoseSample {
                params: &params,
                pose_inputs: Some(&inputs),
                seed: 1,
            },
            &skel,
            &stack,
            &store,
            &|i| clips.get(i),
            &mut out,
        );

        let (_, rotation, _) =
            Mat4::from_cols_array_2d(&out[0].matrix).to_scale_rotation_translation();
        let expected =
            Quat::from_rotation_y(inputs.aim_yaw) * Quat::from_rotation_x(-inputs.aim_pitch);
        assert!(
            rotation.normalize().dot(expected.normalize()).abs() > 1.0 - 1.0e-5,
            "unresolved primary samples and modifies the skeleton rest pose"
        );
    }

    #[test]
    fn unresolved_primary_fade_blends_outgoing_clip_toward_modified_rest() {
        use postretro_model::pose_modifier::{
            JointMask, ModifierEntry, PoseModifier, PoseModifierStack,
        };

        let skel = one_joint_skeleton();
        let clips = [const_x_clip("outgoing", 4.0)];
        let mut store = SnapshotStore::default();
        store.entries.insert(
            1,
            StoredSnapshot {
                tag: 7,
                pose: vec![LocalTrs {
                    translation: Vec3::new(6.0, 0.0, 0.0),
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                }],
            },
        );
        let mut mask = JointMask::new();
        assert!(mask.insert(0));
        let stack = PoseModifierStack::new(vec![ModifierEntry {
            mask,
            modifier: PoseModifier::AimPitchBend {
                bend_weights: vec![1.0],
            },
        }]);
        let inputs = postretro_entities::PoseInputs {
            aim_pitch: 0.3,
            ..Default::default()
        };
        let cases = [
            ("clip", FadeSource::Clip(clip_leg(0, 0.0)), 2.0),
            (
                "snapshot hit",
                FadeSource::Snapshot {
                    tag: 7,
                    fallback: clip_leg(0, 0.0),
                },
                3.0,
            ),
            (
                "snapshot miss falls back to clip",
                FadeSource::Snapshot {
                    tag: 8,
                    fallback: clip_leg(0, 0.0),
                },
                2.0,
            ),
        ];

        for (case, from, expected_x) in cases {
            let params = MeshSampleParams {
                primary: clip_leg(9, 0.0),
                fade: Some(MeshFade { from, weight: 0.5 }),
            };
            let mut out = Vec::new();
            sample_instance(
                InstancePoseSample {
                    params: &params,
                    pose_inputs: Some(&inputs),
                    seed: 1,
                },
                &skel,
                &stack,
                &store,
                &|i| clips.get(i),
                &mut out,
            );

            let (scale, rotation, translation) =
                Mat4::from_cols_array_2d(&out[0].matrix).to_scale_rotation_translation();
            assert!(
                (translation.x - expected_x).abs() < 1.0e-5,
                "{case} keeps its outgoing endpoint"
            );
            assert!((scale - Vec3::ONE).length() < 1.0e-5);
            let expected = Quat::from_rotation_x(-inputs.aim_pitch);
            assert!(
                rotation.normalize().dot(expected.normalize()).abs() > 1.0 - 1.0e-5,
                "{case}: modifier applies after outgoing-to-rest blending"
            );
        }
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
        sample_unmodified_instance(&make(0.0), &skel, &store, 1, &|i| clips.get(i), &mut out);
        assert!(
            (palette_x(&out) - 0.0).abs() < 1.0e-4,
            "weight 0 = outgoing"
        );
        sample_unmodified_instance(&make(1.0), &skel, &store, 1, &|i| clips.get(i), &mut out);
        assert!(
            (palette_x(&out) - 10.0).abs() < 1.0e-4,
            "weight 1 = primary"
        );
        sample_unmodified_instance(&make(0.5), &skel, &store, 1, &|i| clips.get(i), &mut out);
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
        sample_unmodified_instance(&params, &skel, &store, 7, &|i| clips.get(i), &mut out);
        assert!(
            (palette_x(&out) - 4.0).abs() < 1.0e-4,
            "snapshot fade weight 0 reproduces the captured in-flight blend, got {}",
            palette_x(&out),
        );
    }

    /// Mirrors the public CPU payload produced by the game-side
    /// `smooth_interrupt_capture_freezes_interrupted_blend_at_entry_stamp` test:
    /// A→B is halfway through when C smoothly interrupts, so the renderer freezes
    /// S = blend(A, B, 0.5), then samples C's fade from S with no pop.
    #[test]
    fn smooth_interrupt_cpu_contract_reproduces_snapshot_then_eases_to_primary() {
        let skel = one_joint_skeleton();
        let clips = [
            const_x_clip("idle", 0.0),
            const_x_clip("walk", 10.0),
            const_x_clip("run", 100.0),
        ];
        let mut store = SnapshotStore::default();
        let tag = 1.1_f64.to_bits();

        let capture = CaptureInstruction {
            seed: 7,
            tag,
            outgoing: FadeSource::Clip(clip_leg(0, 1.1)),
            incoming: clip_leg(1, 0.1),
            weight: 0.5,
        };
        let mut scratch = Vec::new();
        store.apply_capture(&capture, &skel, |i| clips.get(i), &mut scratch);
        let captured = store.matching(7, tag).expect("snapshot stored");
        assert!(
            (captured[0].translation.x - 5.0).abs() < 1.0e-4,
            "S = blend(A=0, B=10, 0.5)"
        );

        let sample = |weight| MeshSampleParams {
            primary: clip_leg(2, 0.0),
            fade: Some(MeshFade {
                from: FadeSource::Snapshot {
                    tag,
                    fallback: clip_leg(1, 0.1),
                },
                weight,
            }),
        };
        let mut out = Vec::new();
        sample_unmodified_instance(&sample(0.0), &skel, &store, 7, &|i| clips.get(i), &mut out);
        assert!(
            (palette_x(&out) - 5.0).abs() < 1.0e-4,
            "interrupt-instant sample starts from captured S"
        );

        sample_unmodified_instance(&sample(0.5), &skel, &store, 7, &|i| clips.get(i), &mut out);
        assert!(
            (palette_x(&out) - 52.5).abs() < 1.0e-4,
            "mid-fade sample eases from S=5 toward C=100"
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
        sample_unmodified_instance(&params, &skel, &store, 5, &|i| clips.get(i), &mut out);
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

    #[test]
    fn snapshot_store_empty_frame_evicts_captured_entry() {
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

        // Regression: an empty/no-entity plan used to skip snapshot-store
        // lifecycle work, leaving captured smooth-interrupt poses alive until
        // level load.
        store.retain_active_snapshot_fades(&HashMap::new());
        assert!(
            store.matching(1, 5).is_none(),
            "a frame with no active planned snapshot fades evicts the capture"
        );
    }

    #[test]
    fn snapshot_store_retains_only_active_matching_fades_and_clear_empties_it() {
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

        store.apply_capture(
            &CaptureInstruction {
                seed: 2,
                tag: 7,
                outgoing: FadeSource::Clip(clip_leg(0, 0.0)),
                incoming: clip_leg(1, 0.0),
                weight: 0.5,
            },
            &skel,
            |i| clips.get(i),
            &mut scratch,
        );

        let mut active = HashMap::new();
        active.insert(1, 5);
        store.retain_active_snapshot_fades(&active);
        assert!(
            store.matching(1, 5).is_some(),
            "matching active fade survives"
        );
        assert!(
            store.matching(2, 7).is_none(),
            "absent planned entity is evicted"
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
        assert!(store.matching(1, 5).is_none(), "clear empties kept entries");
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
        let key = MeshPaletteCacheKey::Entity(7);

        // Miss: even with collector_resample = false, must_sample is true.
        assert!(
            cache.must_sample(key, false),
            "a cache miss forces a resample even when the collector cleared a skip",
        );

        // Store a sampled run (the resample frame's outcome).
        let run = palette_run(1.0, 4);
        cache.store(key, &run);

        // Now a collector skip (resample = false) is honored — the entry exists.
        assert!(
            !cache.must_sample(key, false),
            "with a cached run, a collector skip is honored (no forced resample)",
        );
        // And the cached run is served for the skip re-upload.
        let cached = cache.touch_cached(key).expect("cached run present on skip");
        assert_eq!(cached.len(), 4);
        assert_eq!(cached[0].matrix[0][0], 1.0);

        // A collector resample still samples even with a cache hit.
        assert!(
            cache.must_sample(key, true),
            "an explicit collector resample always samples, cache hit or not",
        );
    }

    #[test]
    fn palette_cache_store_reuses_storage_in_place() {
        // A resample refreshes the run in place — repeated stores must not change
        // the served contents' shape unexpectedly, and the latest store wins.
        let mut cache = PaletteCache::default();
        let key = MeshPaletteCacheKey::Entity(3);
        cache.store(key, &palette_run(1.0, 6));
        cache.store(key, &palette_run(2.0, 6));
        let cached = cache.touch_cached(key).expect("present");
        assert_eq!(cached.len(), 6);
        assert_eq!(cached[0].matrix[0][0], 2.0, "the latest stored run wins");
    }

    #[test]
    fn palette_cache_evicts_entries_absent_from_the_frame() {
        // Entries not touched in a frame are evicted at end_frame, so the cache is
        // bounded by the frame's planned-instance count — a culled-out entity's
        // stale run does not linger.
        let mut cache = PaletteCache::default();
        let one = MeshPaletteCacheKey::Entity(1);
        let two = MeshPaletteCacheKey::Entity(2);
        cache.store(one, &palette_run(1.0, 2));
        cache.store(two, &palette_run(1.0, 2));
        cache.end_frame(); // both stored this "frame" → both survive
        assert!(!cache.must_sample(one, false), "entry 1 survives its frame");
        assert!(!cache.must_sample(two, false), "entry 2 survives its frame");

        // Next frame: touch only entity 1 (it skips), entity 2 is absent (culled).
        assert!(cache.touch_cached(one).is_some());
        cache.end_frame();
        assert!(
            !cache.must_sample(one, false),
            "the touched entry survives eviction",
        );
        assert!(
            cache.must_sample(two, false),
            "the untouched entry is evicted → its next appearance forces a resample",
        );
    }

    #[test]
    fn palette_cache_empty_frame_evicts_all_entries() {
        // Regression: an empty mesh plan still ends the palette-cache frame, so
        // all previously cached culled-out poses are evicted before they re-enter.
        let mut cache = PaletteCache::default();
        let one = MeshPaletteCacheKey::Entity(1);
        let two = MeshPaletteCacheKey::Entity(2);
        cache.store(one, &palette_run(1.0, 2));
        cache.store(two, &palette_run(2.0, 2));
        cache.end_frame();
        assert!(!cache.must_sample(one, false), "entry 1 survived setup");
        assert!(!cache.must_sample(two, false), "entry 2 survived setup");

        cache.end_frame();

        assert!(
            cache.must_sample(one, false),
            "empty frame evicts entry 1 so re-entry forces resample",
        );
        assert!(
            cache.must_sample(two, false),
            "empty frame evicts entry 2 so re-entry forces resample",
        );
    }

    #[test]
    fn palette_cache_clear_empties_for_level_load() {
        // The level-load clear empties the cache wholesale — entity seeds are not
        // stable across levels, so a stale run must not survive.
        let mut cache = PaletteCache::default();
        let key = MeshPaletteCacheKey::Entity(9);
        cache.store(key, &palette_run(1.0, 3));
        cache.end_frame();
        assert!(!cache.must_sample(key, false), "entry present before clear");
        cache.clear();
        assert!(
            cache.must_sample(key, false),
            "clear empties the cache → a miss forces a resample",
        );
    }

    #[test]
    fn palette_cache_separates_attachment_from_entity_seed_collision() {
        // The first holder's first attachment used to derive seed 1, which can
        // be a second entity's raw id. Cache identity must retain both runs.
        let mut cache = PaletteCache::default();
        let attachment = MeshPaletteCacheKey::Attachment {
            holder: 0,
            attachment_index: 0,
        };
        let entity = MeshPaletteCacheKey::Entity(1);
        cache.store(attachment, &palette_run(1.0, 1));
        cache.store(entity, &palette_run(2.0, 2));

        assert_eq!(
            cache.touch_cached(attachment).unwrap()[0].matrix[0][0],
            1.0,
            "rigid attachment keeps its own identity palette",
        );
        assert_eq!(
            cache.touch_cached(entity).unwrap()[0].matrix[0][0],
            2.0,
            "entity palette is not overwritten by the attachment",
        );
    }
}
