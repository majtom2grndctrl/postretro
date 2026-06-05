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
//   * group 1 = material (the `build_material_bind_group` bind group — flat-lit
//               fragment samples only diffuse + aniso sampler, but the whole
//               group-1 layout is reused so the bind group is compatible)
//   * group 2 = LEFT OPEN — provisional lighting slot (the broadening lighting
//               task adds SH ambient + dynamic direct here; not allocated now)
//   * group 3 = skinned instance data: shared bone-palette storage buffer
//               (binding 0) + per-instance SSBO carrying each instance's model
//               matrix and palette base index, addressed by
//               `@builtin(instance_index)` (binding 1)
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

use crate::model::mesh::SkinnedMesh;
use crate::model::skeleton::{AnimationClip, Skeleton};
use crate::model::{BonePaletteEntry, ModelHandle};
use crate::prl::LevelWorld;
use crate::render::mesh_instances::{
    JointCounts, MAX_PALETTE_ENTRIES, MeshFramePlan, instance_phase,
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

/// Upper bound on per-frame instances. One instance consumes at least one
/// palette slot, so the densely-packed palette budget caps the instance count at
/// `MAX_PALETTE_ENTRIES`; sizing the instance SSBO to that bound means it never
/// reallocates per frame.
const MAX_INSTANCES: usize = MAX_PALETTE_ENTRIES;

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
// assemble forward.wgsl. The mesh path never evaluates animated layers, so
// `curve_eval.wgsl` is NOT appended (unlike the forward composition).
const SKINNED_MESH_SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/skinned_mesh.wgsl"),
    "\n",
    include_str!("../shaders/sh_sample.wgsl"),
);

/// One uploaded skinned model: GPU vertex + index buffers, its per-submesh
/// material bind groups, and the CPU-side animation data (skeleton + first clip)
/// the per-frame palette is sampled from. A single-material model has one
/// submesh spanning the whole index buffer; multi-material models carry one
/// entry per primitive, in submesh order.
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
    /// First animation clip, if the model carries one. `None` → the model holds
    /// its bind pose (identity palette run) every frame.
    clip: Option<AnimationClip>,
}

/// GPU resources for the skinned-mesh forward pass.
pub struct MeshPass {
    pipeline: wgpu::RenderPipeline,

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

    /// Uploaded models keyed by handle (the raw `MeshComponent.model` string).
    /// One entry per distinct model; mirrors `SmokePass::sheets`. The level-load
    /// step (Task D) populates this via [`MeshPass::insert_model`].
    models: HashMap<ModelHandle, UploadedModel>,
}

impl MeshPass {
    /// Build the skinned-mesh pipeline. `camera_bgl` and `material_bgl` are the
    /// renderer-owned layouts shared with the forward pass (group 0 = camera
    /// uniform, group 1 = material). Mirrors `SmokePass::new`'s shape.
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        camera_bgl: &wgpu::BindGroupLayout,
        material_bgl: &wgpu::BindGroupLayout,
        sh_volume_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Skinned Mesh Shader"),
            source: wgpu::ShaderSource::Wgsl(SKINNED_MESH_SHADER_SOURCE.into()),
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

        // Pipeline layout: group 0 (camera), 1 (material), 2 LEFT OPEN
        // (provisional dynamic-DIRECT lighting slot — `None`, like SmokePass
        // leaves unused slots; the dynamic-direct task adds group 2 rather than
        // renumbering), 3 (skinned instance data), 4 (SH irradiance volume —
        // the SAME `ShVolumeResources.bind_group_layout` the forward/billboard/
        // fog passes use, reused verbatim so the shared bind group binds here).
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Skinned Mesh Pipeline Layout"),
            bind_group_layouts: &[
                Some(camera_bgl),
                Some(material_bgl),
                None,
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
                // but unused by the flat-lit fragment this slice; committing it
                // now lets depth-only, lighting, and normal-map passes reuse
                // this vertex layout without a format change.
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

        Self {
            pipeline,
            palette_buffer,
            instance_buffer,
            instance_bind_group,
            models: HashMap::new(),
        }
    }

    /// Insert (or replace) an uploaded skinned model keyed by `handle`. Uploads
    /// the mesh's vertex + index buffers and retains its per-submesh material
    /// bind groups plus the CPU-side animation data (skeleton + first clip) the
    /// per-frame palette is sampled from.
    ///
    /// `submeshes` pairs each material bind group with the index range it draws,
    /// in submesh order — built by the renderer via `build_material_bind_group`
    /// against the shared group-1 layout (the same `.prm` → `LoadedTexture` path
    /// the world uses). This is the cache-insertion seam the level-load step
    /// (Task D) calls once per distinct model at install.
    pub fn insert_model(
        &mut self,
        device: &wgpu::Device,
        handle: ModelHandle,
        mesh: &SkinnedMesh,
        submeshes: Vec<(wgpu::BindGroup, std::ops::Range<u32>)>,
        skeleton: Skeleton,
        clip: Option<AnimationClip>,
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
        self.models.insert(
            handle,
            UploadedModel {
                vertex_buffer,
                index_buffer,
                submeshes,
                skeleton,
                clip,
            },
        );
    }

    /// Whether any model has been uploaded. The renderer skips the pass entirely
    /// when the cache is empty.
    pub fn has_model(&self) -> bool {
        !self.models.is_empty()
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

    /// Write this frame's per-instance SSBO + bone-palette runs from `plan`, then
    /// record one instanced `draw_indexed` per model per submesh.
    ///
    /// For each planned instance: pack its SSBO entry (model matrix + palette
    /// base) and sample its model's clip into the palette at that base, at a
    /// per-instance phase derived from the instance's seed (so a wave is not
    /// lock-step). `now_seconds` is the render clock; `scratch` is the renderer's
    /// reusable pose buffer (kept off the GPU pass so a steady-state frame
    /// allocates nothing). Group 0 (camera) and group 4 (SH irradiance volume)
    /// must be set by the caller before recording — the renderer owns the camera
    /// and `ShVolumeResources` bind groups (both shared across passes).
    ///
    /// Cull is the caller's job — see [`mesh_visible`]; the plan already holds
    /// only surviving, in-budget instances.
    pub fn render_frame(
        &self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        plan: &MeshFramePlan,
        now_seconds: f32,
        scratch: &mut Vec<BonePaletteEntry>,
    ) {
        if plan.groups.is_empty() {
            return;
        }

        pass.set_pipeline(&self.pipeline);
        // Group 3 (palette + instance SSBO) is shared across every group/submesh
        // this frame — set once. The shader selects each instance's run via
        // `@builtin(instance_index)` against the densely-packed SSBO.
        pass.set_bind_group(3, &self.instance_bind_group, &[]);

        for group in &plan.groups {
            let Some(model) = self.models.get(&group.model) else {
                // Planner only emits groups for cached models, but guard anyway.
                continue;
            };
            if model.submeshes.is_empty() {
                continue;
            }

            // Write each instance's SSBO entry + sample its palette run.
            for (i, inst) in group.instances.iter().enumerate() {
                let instance_index = group.instance_offset as usize + i;
                let entry = build_instance_entry(inst.transform, inst.palette_base);
                queue.write_buffer(
                    &self.instance_buffer,
                    (instance_index * INSTANCE_ENTRY_SIZE) as u64,
                    &entry,
                );

                // Sample this instance's clip into its palette run at a
                // per-instance phase. No clip → leave the run as the identity
                // bind pose seeded at init.
                if let Some(clip) = &model.clip {
                    let phase = instance_phase(inst.phase_seed, clip.duration);
                    crate::model::anim::sample_clip(
                        clip,
                        &model.skeleton,
                        now_seconds + phase,
                        scratch,
                    );
                    if !scratch.is_empty() {
                        queue.write_buffer(
                            &self.palette_buffer,
                            inst.palette_base as u64 * BONE_PALETTE_ENTRY_SIZE as u64,
                            bytemuck::cast_slice(scratch),
                        );
                    }
                }
            }

            // One instanced draw per submesh over this group's contiguous
            // instance range. The base instance stays 0 — the palette base
            // never travels through `first_instance` (DX12 reads it as 0,
            // gfx-rs/wgpu#2471); it lives in each SSBO entry, addressed by
            // `@builtin(instance_index)`.
            let instance_range =
                group.instance_offset..group.instance_offset + group.instances.len() as u32;
            pass.set_vertex_buffer(0, model.vertex_buffer.slice(..));
            pass.set_index_buffer(model.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for (material_bind_group, indices) in &model.submeshes {
                if indices.is_empty() {
                    continue;
                }
                pass.set_bind_group(1, material_bind_group, &[]);
                pass.draw_indexed(indices.clone(), 0, instance_range.clone());
            }
        }
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
}
