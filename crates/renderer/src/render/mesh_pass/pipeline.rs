// Skinned-mesh shader composition and pipeline, layout and buffer construction.
// See: context/lib/rendering_pipeline.md §9

use std::collections::HashMap;

use postretro_render_cpu::mesh_instances::{MAX_INSTANCES, MAX_PALETTE_ENTRIES};

use super::lighting::{MESH_LIGHT_PARAMS_SIZE, mesh_light_bind_group_layout_entries};
use super::pose::{PaletteCache, SnapshotStore};
use super::{BONE_PALETTE_ENTRY_SIZE, INSTANCE_ENTRY_SIZE, MeshPass};
use crate::render::UNIFORM_SIZE;

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
pub(super) const SKINNED_MESH_SHADER_SOURCE: &str = concat!(
    include_str!("../../shaders/skinned_mesh.wgsl"),
    "\n",
    include_str!("../../shaders/sh_indirection.wgsl"),
    "\n",
    include_str!("../../shaders/sh_sample.wgsl"),
    "\n",
    include_str!("../../shaders/curve_eval.wgsl"),
    "\n",
    include_str!("../../shaders/light_falloff.wgsl"),
    "\n",
    include_str!("../../shaders/light_eval.wgsl"),
    "\n",
    include_str!("../../shaders/shadow_sample.wgsl"),
    "\n",
    include_str!("../../shaders/shadow_sample_static_cache.wgsl"),
    "\n",
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
pub(super) fn skinned_mesh_shader_source(
    cube_array_supported: bool,
) -> std::borrow::Cow<'static, str> {
    if cube_array_supported {
        std::borrow::Cow::Borrowed(SKINNED_MESH_SHADER_SOURCE)
    } else {
        std::borrow::Cow::Owned(crate::render::strip_point_shadow_cube(
            SKINNED_MESH_SHADER_SOURCE,
        ))
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
        // mesh specs — b0 dynamic prefix, raw animated-baked tail, then
        // selected-static suffix, b1 per-light influence volumes, b2 scripted-animation
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
                buffers: &[Some(wgpu::VertexBufferLayout {
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
                })],
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
                    format: crate::render::SCENE_COLOR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        let depth_pipeline = crate::render::mesh_depth::create_skinned_depth_pipeline(
            device,
            light_space_bgl,
            &instance_bind_group_layout,
            shadow_depth_format,
        );
        let ghost_pipelines = crate::render::mesh_ghost::skinned_ghost_pipelines(
            device,
            depth_format,
            camera_bgl,
            material_bgl,
            &instance_bind_group_layout,
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

        Self {
            pipeline,
            depth_pipeline,
            ghost_pipelines,
            palette_buffer,
            instance_buffer,
            instance_bind_group,
            viewmodel_uniform_buffer,
            viewmodel_uniform_bind_group,
            light_bind_group_layout,
            light_bind_group: None,
            mirror_light_bind_group: None,
            mirror_light_records: false,
            light_params_buffer,
            cube_array_supported,
            models: HashMap::new(),
            model_bounds: HashMap::new(),
            model_clips: HashMap::new(),
            snapshot_store: SnapshotStore::default(),
            palette_cache: PaletteCache::default(),
            capture_scratch: Vec::new(),
        }
    }
}
