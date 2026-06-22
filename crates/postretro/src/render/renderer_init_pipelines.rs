// World/wireframe/depth/shadow render-pipeline construction, factored out of
// `Renderer::new` to keep the constructor within the module size budget.
// See: context/lib/rendering_pipeline.md §4

use super::*;

pub(crate) struct RendererPipelines {
    pub pipeline: wgpu::RenderPipeline,
    pub wireframe_cull_status_layout: wgpu::BindGroupLayout,
    pub wireframe_cull_status_pipeline: wgpu::RenderPipeline,
    pub wireframe_visible_pipeline: wgpu::RenderPipeline,
    pub depth_prepass_pipeline: wgpu::RenderPipeline,
    pub shadow_vs_bgl: wgpu::BindGroupLayout,
    pub shadow_depth_pipeline: wgpu::RenderPipeline,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_renderer_pipelines(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    uniform_bind_group_layout: &wgpu::BindGroupLayout,
    texture_bind_group_layout: &wgpu::BindGroupLayout,
    lighting_bind_group_layout: &wgpu::BindGroupLayout,
    sh_volume_bind_group_layout: &wgpu::BindGroupLayout,
    lightmap_bind_group_layout: &wgpu::BindGroupLayout,
    spot_shadow_bgl: &wgpu::BindGroupLayout,
    cube_array_supported: bool,
) -> RendererPipelines {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Textured Pipeline Layout"),
        bind_group_layouts: &[
            Some(uniform_bind_group_layout),
            Some(texture_bind_group_layout),
            Some(lighting_bind_group_layout),
            Some(sh_volume_bind_group_layout),
            Some(lightmap_bind_group_layout),
            Some(spot_shadow_bgl),
        ],
        immediate_size: 0,
    });

    // On an adapter without CUBE_ARRAY_TEXTURES the shared group-5 BGL omits
    // binding 5, so the forward shader must not declare or sample the
    // `point_shadow_cube` binding. Derive that variant from the one source via
    // `strip_point_shadow_cube` (strips the binding decl, neutralizes
    // `sample_point_shadow` to a no-shadow constant) rather than maintaining a
    // second copy. When supported, the source is used verbatim.
    let forward_source: std::borrow::Cow<str> = if cube_array_supported {
        SHADER_SOURCE.into()
    } else {
        strip_point_shadow_cube(SHADER_SOURCE).into()
    };
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Textured Shader"),
        source: wgpu::ShaderSource::Wgsl(forward_source),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Textured Pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: crate::geometry::WorldVertex::STRIDE as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    // position: vec3<f32> at offset 0
                    wgpu::VertexAttribute {
                        offset: 0,
                        shader_location: 0,
                        format: wgpu::VertexFormat::Float32x3,
                    },
                    // base_uv: vec2<f32> at offset 12
                    wgpu::VertexAttribute {
                        offset: 12,
                        shader_location: 1,
                        format: wgpu::VertexFormat::Float32x2,
                    },
                    // normal_oct: u16x2 at offset 20
                    wgpu::VertexAttribute {
                        offset: 20,
                        shader_location: 2,
                        format: wgpu::VertexFormat::Uint16x2,
                    },
                    // tangent_packed: u16x2 at offset 24
                    wgpu::VertexAttribute {
                        offset: 24,
                        shader_location: 3,
                        format: wgpu::VertexFormat::Uint16x2,
                    },
                    // lightmap_uv: u16x2 at offset 28 (quantized 0..1 UV)
                    wgpu::VertexAttribute {
                        offset: 28,
                        shader_location: 4,
                        format: wgpu::VertexFormat::Uint16x2,
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
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            // Pre-pass filled the buffer; Equal test → one shade per pixel.
            // Write disabled to skip redundant rewrite of pre-pass values.
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Equal),
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

    // Wireframe: group 0 = uniforms, group 1 = per-leaf cull status from compute shader.
    let wireframe_cull_status_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Wireframe Cull Status BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
    let wireframe_pipeline_layout =
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Wireframe Pipeline Layout"),
            bind_group_layouts: &[
                Some(uniform_bind_group_layout),
                Some(&wireframe_cull_status_layout),
            ],
            immediate_size: 0,
        });

    let wireframe_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Wireframe Shader"),
        source: wgpu::ShaderSource::Wgsl(WIREFRAME_SHADER_SOURCE.into()),
    });

    let wireframe_cull_status_pipeline =
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Wireframe Cull Status Pipeline"),
            layout: Some(&wireframe_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &wireframe_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: crate::geometry::WorldVertex::STRIDE as wgpu::BufferAddress,
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
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 20,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 24,
                            shader_location: 3,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                    ],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            // Always so the culling diagnostic draws every leaf on top of the
            // scene; write disabled since the forward pass owns depth contents.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &wireframe_shader,
                entry_point: Some("fs_cull_status"),
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

    let wireframe_visible_pipeline =
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Wireframe Visible Pipeline"),
            layout: Some(&wireframe_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &wireframe_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: crate::geometry::WorldVertex::STRIDE as wgpu::BufferAddress,
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
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 20,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 24,
                            shader_location: 3,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                    ],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            // Depth-tested visible wireframe uses the shared scene depth without
            // writing to it. LessEqual keeps coplanar submitted triangles visible
            // while geometry hidden behind nearer surfaces is rejected.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &wireframe_shader,
                entry_point: Some("fs_visible"),
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

    let depth_prepass_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Depth Pre-Pass Pipeline Layout"),
        bind_group_layouts: &[Some(uniform_bind_group_layout)],
        immediate_size: 0,
    });

    let depth_prepass_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Depth Pre-Pass Shader"),
        source: wgpu::ShaderSource::Wgsl(DEPTH_PREPASS_SHADER_SOURCE.into()),
    });

    let depth_prepass_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Depth Pre-Pass Pipeline"),
        layout: Some(&depth_prepass_layout),
        vertex: wgpu::VertexState {
            module: &depth_prepass_shader,
            entry_point: Some("vs_main"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: crate::geometry::WorldVertex::STRIDE as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    // Shares the forward vertex buffer — only position used; rest declared to match.
                    wgpu::VertexAttribute {
                        offset: 0,
                        shader_location: 0,
                        format: wgpu::VertexFormat::Float32x3,
                    },
                    wgpu::VertexAttribute {
                        offset: 12,
                        shader_location: 1,
                        format: wgpu::VertexFormat::Float32x2,
                    },
                    wgpu::VertexAttribute {
                        offset: 20,
                        shader_location: 2,
                        format: wgpu::VertexFormat::Uint16x2,
                    },
                    wgpu::VertexAttribute {
                        offset: 24,
                        shader_location: 3,
                        format: wgpu::VertexFormat::Uint16x2,
                    },
                    // Lightmap UV — consumed by the fragment stage and
                    // written to the Rg16Float gbuffer slot below.
                    wgpu::VertexAttribute {
                        offset: 28,
                        shader_location: 4,
                        format: wgpu::VertexFormat::Uint16x2,
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
        // Unchanged from the vertex-only pre-pass: writes depth with a
        // `Less` test. The forward pass still re-tests with `Equal`.
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        // Vertex-only depth pre-pass: no color attachment. The
        // lightmap-UV gbuffer MRT was removed with the animated
        // dominant-direction trace (the per-light SDF trace keys on
        // light position, not lightmap UV).
        fragment: None,
        multiview_mask: None,
        cache: None,
    });

    // Spot shadow pipeline: shared across all SHADOW_POOL_SIZE slots via dynamic-offset uniform.
    // Depth bias (constant=2, slope=1.5) suppresses acne without Peter-Panning.
    let shadow_vs_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Shadow VS BGL"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: std::num::NonZeroU64::new(64),
            },
            count: None,
        }],
    });
    let shadow_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Spot Shadow Pipeline Layout"),
        bind_group_layouts: &[Some(&shadow_vs_bgl)],
        immediate_size: 0,
    });
    let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Spot Shadow Shader"),
        source: wgpu::ShaderSource::Wgsl(SPOT_SHADOW_SHADER_SOURCE.into()),
    });
    let shadow_depth_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Spot Shadow Depth Pipeline"),
        layout: Some(&shadow_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shadow_shader,
            entry_point: Some("vs_main"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: crate::geometry::WorldVertex::STRIDE as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                }],
            }],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: crate::lighting::spot_shadow::SHADOW_DEPTH_FORMAT,
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

    RendererPipelines {
        pipeline,
        wireframe_cull_status_layout,
        wireframe_cull_status_pipeline,
        wireframe_visible_pipeline,
        depth_prepass_pipeline,
        shadow_vs_bgl,
        shadow_depth_pipeline,
    }
}

pub(crate) struct WorldVertexBuffers {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    pub wireframe_index_buffer: wgpu::Buffer,
    pub wireframe_index_count: u32,
}

pub(crate) fn build_world_vertex_buffers(
    device: &wgpu::Device,
    geometry: Option<&LevelGeometry>,
) -> WorldVertexBuffers {
    let (vertex_data, index_data, index_count) =
        if let Some(geom) = geometry.filter(|g| !g.vertices.is_empty() && !g.indices.is_empty()) {
            let count = geom.indices.len() as u32;
            (
                cast_world_vertices_to_bytes(geom.vertices),
                bytemuck_cast_slice_u32(geom.indices),
                count,
            )
        } else {
            (
                vec![0u8; crate::geometry::WorldVertex::STRIDE], // one dummy vertex
                vec![0u8; 4],                                    // one dummy index
                0u32,
            )
        };

    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("World Vertex Buffer"),
        contents: &vertex_data,
        usage: wgpu::BufferUsages::VERTEX,
    });

    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("World Index Buffer"),
        contents: &index_data,
        usage: wgpu::BufferUsages::INDEX,
    });

    // Build a line-list index buffer from the triangle index buffer for the
    // wireframe overlay. Each triangle contributes its three edges as line
    // pairs. Shared edges are duplicated (cheap, and avoids a hash set).
    let (wireframe_index_data, wireframe_index_count) =
        if let Some(geom) = geometry.filter(|g| !g.vertices.is_empty() && !g.indices.is_empty()) {
            let line_indices = build_line_indices_from_triangles(geom.indices);
            let count = line_indices.len() as u32;
            (bytemuck_cast_slice_u32(&line_indices), count)
        } else {
            (vec![0u8; 4], 0u32)
        };

    let wireframe_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Wireframe Line Index Buffer"),
        contents: &wireframe_index_data,
        usage: wgpu::BufferUsages::INDEX,
    });

    WorldVertexBuffers {
        vertex_buffer,
        index_buffer,
        index_count,
        wireframe_index_buffer,
        wireframe_index_count,
    }
}
