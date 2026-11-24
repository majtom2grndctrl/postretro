// The world forward pipeline's forms over forward's one layout, shader and
// vertex streams: the one-output form and the glossy overlay's
// static-diffuse forms, all built at full init.
// See: context/lib/rendering_pipeline.md §7.3, §7.3a (Glossy world faces)

use super::*;
use crate::render::gpu_create::GpuCreate;

/// One forward pipeline over forward's layout, shader and vertex streams:
/// every form shares the vertex stage, depth state and `@invariant` clip
/// position, and differs only in its fragment entry and colour targets.
pub(super) fn forward_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    label: &str,
    entry_point: &str,
    targets: &[Option<wgpu::ColorTargetState>],
) -> wgpu::RenderPipeline {
    device.new_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[
                Some(wgpu::VertexBufferLayout {
                    array_stride: postretro_render_data::geometry::WorldVertex::STRIDE
                        as wgpu::BufferAddress,
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
                        // lightmap_uv: u16x2 at offset 28 (block-local unorm UV)
                        wgpu::VertexAttribute {
                            offset: 28,
                            shader_location: 4,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                        // (lightmap_block, animated_block): u16x2 at offset 32 —
                        // the vertex's lightmap cell block id + 1 and the face's
                        // animated block id + 1 (0 = none for both).
                        wgpu::VertexAttribute {
                            offset: 32,
                            shader_location: 5,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                    ],
                }),
                // Slot 1: the forward-only owner stream, one entry per world
                // vertex (`spec_owner_buffer`). Its own buffer leaves the 36-byte
                // world vertex, and every other pipeline reading it, unchanged.
                Some(wgpu::VertexBufferLayout {
                    array_stride: SPEC_OWNER_STRIDE as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 6,
                            format: wgpu::VertexFormat::Uint16x4,
                        },
                        // The face's lightmap-to-mask texel offset, two
                        // packed i16s the shader sign-extends.
                        wgpu::VertexAttribute {
                            offset: SPEC_OWNER_MASK_OFFSET_AT as wgpu::BufferAddress,
                            shader_location: 7,
                            format: wgpu::VertexFormat::Uint32,
                        },
                    ],
                }),
                // Slot 2: the forward-only emissive volume id, one `Uint32`
                // per world vertex (volume index + 1, 0 = none). Depth,
                // mover and fluid pipelines never bind it.
                Some(wgpu::VertexBufferLayout {
                    array_stride: super::emissive_volumes::EMISSIVE_VOLUME_STRIDE
                        as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        offset: 0,
                        shader_location: super::emissive_volumes::EMISSIVE_VOLUME_SHADER_LOCATION,
                        format: wgpu::VertexFormat::Uint32,
                    }],
                }),
            ],
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
            module: shader,
            entry_point: Some(entry_point),
            targets,
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// One pair of forward sweeps over a level with glossy faces: glossy buckets
/// write D_static, every other bucket keeps it masked off.
pub(crate) struct ForwardGlossySweeps {
    /// Glossy buckets: colour, then D_static.
    pub written: wgpu::RenderPipeline,
    /// Every other bucket: the same colour, D_static masked off.
    pub masked: wgpu::RenderPipeline,
}

/// The forward forms a level with glossy faces binds at a reflection tier
/// above off, built at full init so an install or tier change builds none.
/// At a tier that runs the floor layer, on a level with an up-facing glossy
/// face, every bucket also writes the colour with its highlight left out, a
/// third output; otherwise the forms keep two.
pub(crate) struct ForwardStaticDiffusePipelines {
    /// `fs_main_static_diffuse`, and `fs_main` with D_static masked.
    pub static_diffuse: ForwardGlossySweeps,
    /// `fs_main_static_diffuse_highlight_free`, and `fs_main_highlight_free`
    /// with D_static masked: the floor layer's tiers.
    pub highlight_free: ForwardGlossySweeps,
    /// The D_static target's format: the scene colour's ([`static_diffuse_format`]).
    pub format: wgpu::TextureFormat,
    /// The highlight-free colour's format
    /// ([`super::glossy_floor::highlight_free_format`]).
    pub highlight_free_format: wgpu::TextureFormat,
    /// What [`Self::variant`] rebuilds the forms over.
    #[cfg(test)]
    layout: wgpu::PipelineLayout,
    #[cfg(test)]
    scene_colour_target: Option<wgpu::ColorTargetState>,
}

impl ForwardStaticDiffusePipelines {
    pub(super) fn new(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
        scene_colour_target: &Option<wgpu::ColorTargetState>,
    ) -> Self {
        let format = static_diffuse_format();
        let highlight_free_format = super::glossy_floor::highlight_free_format(device.features());
        let target = |format, write_mask| {
            Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask,
            })
        };
        let static_diffuse = target(format, wgpu::ColorWrites::ALL);
        let static_diffuse_masked = target(format, wgpu::ColorWrites::empty());
        let highlight_free = target(highlight_free_format, wgpu::ColorWrites::ALL);
        let form = |label, entry, targets: &[Option<wgpu::ColorTargetState>]| {
            forward_pipeline(device, layout, shader, label, entry, targets)
        };
        Self {
            static_diffuse: ForwardGlossySweeps {
                written: form(
                    "Textured Pipeline (static diffuse)",
                    "fs_main_static_diffuse",
                    &[scene_colour_target.clone(), static_diffuse.clone()],
                ),
                masked: form(
                    "Textured Pipeline (static diffuse masked)",
                    "fs_main",
                    &[scene_colour_target.clone(), static_diffuse_masked.clone()],
                ),
            },
            highlight_free: ForwardGlossySweeps {
                written: form(
                    "Textured Pipeline (static diffuse, highlight-free)",
                    "fs_main_static_diffuse_highlight_free",
                    &[
                        scene_colour_target.clone(),
                        static_diffuse,
                        highlight_free.clone(),
                    ],
                ),
                masked: form(
                    "Textured Pipeline (static diffuse masked, highlight-free)",
                    "fs_main_highlight_free",
                    &[
                        scene_colour_target.clone(),
                        static_diffuse_masked,
                        highlight_free,
                    ],
                ),
            },
            format,
            highlight_free_format,
            #[cfg(test)]
            layout: layout.clone(),
            #[cfg(test)]
            scene_colour_target: scene_colour_target.clone(),
        }
    }

    /// Every form rebuilt over the same layout and targets from `source`,
    /// already composed for the adapter's cube-array support: the test-only
    /// variants.
    #[cfg(test)]
    pub(crate) fn variant(&self, device: &wgpu::Device, source: &str) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Textured Shader (test variant)"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        Self::new(device, &self.layout, &shader, &self.scene_colour_target)
    }

    /// The sweeps a frame binds: the highlight-free forms when it runs the
    /// floor layer, else the two-output forms.
    pub(crate) fn sweeps(&self, floor_layer: bool) -> &ForwardGlossySweeps {
        if floor_layer {
            &self.highlight_free
        } else {
            &self.static_diffuse
        }
    }
}

/// D_static takes the scene colour's own format, so one rounding serves both
/// outputs: forward's colour is D_static plus non-negative terms (highlights,
/// dynamic light, glow), computed in f32 and stored by the same monotone
/// conversion, so the stored D_static never exceeds the stored colour, per
/// channel, and the overlay's forward + W·(R − D_static) at W ≤ 1, R ≥ 0
/// never falls below 0 (brief row 30). A shorter float (`Rg11b10Ufloat`) is
/// stored by a conversion whose rounding the backends leave to the
/// implementation: rounded to nearest it can exceed the colour by 2⁻⁷ (R, G)
/// or 2⁻⁶ (B) of itself and undershoot at W near 1; truncated (AMD on
/// Metal, measured) it leaves that share of the static light unreplaced.
pub(crate) fn static_diffuse_format() -> wgpu::TextureFormat {
    SCENE_COLOR_FORMAT
}
