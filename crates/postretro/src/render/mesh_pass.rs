// Skinned-mesh render pass: forward draw of skinned models with a shared bone palette.
// See: context/lib/rendering_pipeline.md §9
//
// Mirrors the shape of `crate::render::smoke::SmokePass` (`new` builds the
// pipeline + layouts; `record_draw` sets bind groups + buffers and issues the
// draw). Owns ALL wgpu for skinned meshes — `crate::model` stays wgpu-free.
//
// Binding plan (forward, non-shadow):
//   * group 0 = camera (shared renderer-owned camera uniform / bind group)
//   * group 1 = material (the `build_material_bind_group` bind group — flat-lit
//               fragment samples only diffuse + aniso sampler, but the whole
//               group-1 layout is reused so the bind group is compatible)
//   * group 2 = LEFT OPEN — provisional lighting slot (the broadening lighting
//               task adds SH ambient + dynamic direct here; not allocated now)
//   * group 3 = skinned instance data: shared bone-palette storage buffer
//               (binding 0) + per-instance uniform carrying the model matrix and
//               the palette base index (binding 1)
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

use wgpu::util::DeviceExt;

use crate::model::BonePaletteEntry;
use crate::model::mesh::{MAX_JOINTS, SkinnedMesh};
use crate::prl::LevelWorld;
use crate::visibility::VisibleCells;

/// Byte size of one `BonePaletteEntry` (mat4x4<f32> = 64 B).
const BONE_PALETTE_ENTRY_SIZE: usize = std::mem::size_of::<BonePaletteEntry>();

/// Per-instance uniform: model matrix (64 B) + base index packed into a
/// `vec4<u32>` (16 B) = 80 B. Matches `InstanceUniforms` in skinned_mesh.wgsl.
const INSTANCE_UNIFORM_SIZE: usize = 80;

/// Pack one instance's uniform bytes (model matrix column-major + base index).
fn build_instance_uniform(model: glam::Mat4, base_index: u32) -> [u8; INSTANCE_UNIFORM_SIZE] {
    let mut bytes = [0u8; INSTANCE_UNIFORM_SIZE];
    let cols = model.to_cols_array();
    for (i, v) in cols.iter().enumerate() {
        let off = i * 4;
        bytes[off..off + 4].copy_from_slice(&v.to_ne_bytes());
    }
    // base index at offset 64 (x of the trailing vec4<u32>); 68..80 stay zero.
    bytes[64..68].copy_from_slice(&base_index.to_ne_bytes());
    bytes
}

const SKINNED_MESH_SHADER_SOURCE: &str = include_str!("../shaders/skinned_mesh.wgsl");

/// One uploaded skinned model: GPU vertex + index buffers, its index count, and
/// the material bind group resolved through the shared `.prm` → `LoadedTexture`
/// path. The slice carries at most one; the broadening tasks generalize to many.
struct UploadedModel {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    material_bind_group: wgpu::BindGroup,
}

/// GPU resources for the skinned-mesh forward pass.
pub struct MeshPass {
    pipeline: wgpu::RenderPipeline,

    /// Shared bone-palette storage buffer (sized for `MAX_JOINTS` entries). The
    /// slice draws one instance at base 0, but the buffer is sized for a full
    /// run so the broadening many-instance task scales it without a reshape.
    palette_buffer: wgpu::Buffer,

    /// Group 3 layout: palette storage (binding 0) + per-instance uniform
    /// (binding 1). Retained so per-draw instance bind groups can be built.
    instance_bind_group_layout: wgpu::BindGroupLayout,

    /// The currently uploaded model (at most one this slice). `None` until
    /// `set_model` runs.
    model: Option<UploadedModel>,
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
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Skinned Mesh Shader"),
            source: wgpu::ShaderSource::Wgsl(SKINNED_MESH_SHADER_SOURCE.into()),
        });

        // Group 3: shared bone palette (storage) + per-instance uniform.
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
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        // Pipeline layout: group 0 (camera), 1 (material), 2 LEFT OPEN
        // (provisional lighting — `None`, like SmokePass leaves unused slots),
        // 3 (skinned instance data). The future lighting task adds group 2
        // rather than renumbering.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Skinned Mesh Pipeline Layout"),
            bind_group_layouts: &[
                Some(camera_bgl),
                Some(material_bgl),
                None,
                Some(&instance_bind_group_layout),
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

        // Shared bone-palette storage buffer, sized for one full run of
        // MAX_JOINTS entries. Default-filled to identity (bind pose) below.
        let palette_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Bone Palette Buffer"),
            size: (MAX_JOINTS * BONE_PALETTE_ENTRY_SIZE) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            palette_buffer,
            instance_bind_group_layout,
            model: None,
        }
    }

    /// Upload one skinned mesh's vertex + index buffers and retain its material
    /// bind group. Replaces any previously uploaded model (the slice carries
    /// one). The material bind group is built by the renderer via
    /// `build_material_bind_group` against the shared group-1 layout (the same
    /// `.prm` → `LoadedTexture` path the world uses).
    pub fn set_model(
        &mut self,
        device: &wgpu::Device,
        mesh: &SkinnedMesh,
        material_bind_group: wgpu::BindGroup,
    ) {
        let vertex_bytes = bytemuck::cast_slice(&mesh.vertices);
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Skinned Mesh Vertex Buffer"),
            contents: vertex_bytes,
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Skinned Mesh Index Buffer"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.model = Some(UploadedModel {
            vertex_buffer,
            index_buffer,
            index_count: mesh.indices.len() as u32,
            material_bind_group,
        });
    }

    /// Whether a model has been uploaded. The renderer skips the pass entirely
    /// when no skinned model is present.
    pub fn has_model(&self) -> bool {
        self.model.is_some()
    }

    /// Write a run of bone-palette entries starting at `base_index`. Called each
    /// frame with the sampled pose from `model::anim::sample_clip` to upload the
    /// current-frame palette before the draw is recorded.
    pub fn update_palette(
        &self,
        queue: &wgpu::Queue,
        base_index: u32,
        entries: &[BonePaletteEntry],
    ) {
        if entries.is_empty() {
            return;
        }
        let offset = base_index as u64 * BONE_PALETTE_ENTRY_SIZE as u64;
        queue.write_buffer(&self.palette_buffer, offset, bytemuck::cast_slice(entries));
    }

    /// Initialize the shared bone palette to identity (bind pose) before the first sampled frame.
    pub fn upload_identity_palette(&self, queue: &wgpu::Queue) {
        let identity = BonePaletteEntry {
            matrix: glam::Mat4::IDENTITY.to_cols_array_2d(),
        };
        let entries = vec![identity; MAX_JOINTS];
        self.update_palette(queue, 0, &entries);
    }

    /// Record the skinned draw for one instance into `pass`. Sets the pipeline,
    /// group 1 (material), and group 3 (palette + per-instance model matrix);
    /// binds the vertex/index buffers; and issues a direct `draw_indexed`.
    /// Group 0 (camera) must be set by the caller before recording — it owns
    /// the camera bind group. No-op if no model is uploaded.
    ///
    /// `base_index` is the instance's offset into the shared palette buffer
    /// (0 this slice). `model` is the final per-instance world matrix.
    ///
    /// Cull is the caller's job — see [`mesh_visible`]. The caller
    /// (`scripting::systems::mesh_render::MeshRenderCollector`, which holds the
    /// `LevelWorld`) tests visibility before calling this; an uploaded-but-culled
    /// instance simply isn't recorded.
    pub fn record_draw(
        &self,
        device: &wgpu::Device,
        pass: &mut wgpu::RenderPass<'_>,
        model: glam::Mat4,
        base_index: u32,
    ) {
        let Some(uploaded) = &self.model else {
            return;
        };
        if uploaded.index_count == 0 {
            return;
        }

        // Per-instance uniform (model matrix + base index). Built per-draw; at
        // N=1 this allocation is negligible. The broadening many-instance task
        // moves the per-instance data into the M3.5 indirect instance SSBO
        // (provisional contract — not pre-fit at N=1).
        let instance_bytes = build_instance_uniform(model, base_index);
        let instance_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Skinned Instance Uniform"),
            contents: &instance_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let instance_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Skinned Instance Bind Group"),
            layout: &self.instance_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.palette_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: instance_uniform.as_entire_binding(),
                },
            ],
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(1, &uploaded.material_bind_group, &[]);
        pass.set_bind_group(3, &instance_bind_group, &[]);
        pass.set_vertex_buffer(0, uploaded.vertex_buffer.slice(..));
        pass.set_index_buffer(uploaded.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..uploaded.index_count, 0, 0..1);
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
/// world reference. The `find_leaf` lookup and the membership decision are split
/// so the decision is unit-testable without constructing a full `LevelWorld`
/// (the GPU-free seam — see [`mesh_visible_in_leaf`]).
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
    fn instance_uniform_packs_model_and_base_index() {
        // Guard the WGSL layout contract: InstanceUniforms { model: mat4x4<f32>,
        // base_and_pad: vec4<u32> } — model at offset 0 (64 B), base_index at
        // offset 64 (first u32 of the trailing vec4), total 80 B. If either side
        // (Rust packer or WGSL struct) is edited silently, this assertion fires.
        assert_eq!(
            INSTANCE_UNIFORM_SIZE, 80,
            "INSTANCE_UNIFORM_SIZE must match WGSL InstanceUniforms total (80 B)",
        );

        let m = glam::Mat4::from_translation(Vec3::new(4.0, 5.0, 6.0));
        let bytes = build_instance_uniform(m, 7);
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
