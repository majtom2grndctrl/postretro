// HDR bloom post-process resources and passes.
// See: context/lib/rendering_pipeline.md §7.8

use std::num::NonZeroU64;

use super::SCENE_COLOR_FORMAT;

/// Luminance at which HDR pixels begin contributing to bloom. Task 4's Neon
/// reference strength must keep its authored emissive value above this value.
pub const BLOOM_THRESHOLD: f32 = 1.0;

/// Strength of the blurred bloom contribution composited back into scene color.
pub const BLOOM_INTENSITY: f32 = 0.35;

/// Set `POSTRETRO_BLOOM=0` to disable the pass for the manual no-bloom
/// observation. The default preserves bloom for normal runs.
pub const BLOOM_ENABLED_BY_DEFAULT: bool = true;

const BLOOM_LEVEL_COUNT: usize = 5;
const BLOOM_PARAM_SLOT_COUNT: usize = BLOOM_LEVEL_COUNT * 4;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BloomParams {
    texel_size: [f32; 2],
    threshold: f32,
    intensity: f32,
    direction: [f32; 2],
    _padding: [f32; 2],
}

struct BloomLevel {
    #[allow(dead_code)]
    down_texture: wgpu::Texture,
    down_view: wgpu::TextureView,
    #[allow(dead_code)]
    blur_texture: wgpu::Texture,
    blur_view: wgpu::TextureView,
    down_bind_group: wgpu::BindGroup,
    blur_bind_group: wgpu::BindGroup,
    dimensions: (u32, u32),
}

/// Renderer-owned HDR bloom chain. Bright pixels are extracted into a
/// half-resolution target, filtered through successively smaller levels, then
/// accumulated back up the chain before the final additive scene composite.
pub struct BloomPass {
    enabled: bool,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
    params_buffer: wgpu::Buffer,
    params_stride: u64,
    #[allow(dead_code)]
    scene_source_view: wgpu::TextureView,
    scene_source_bind_group: wgpu::BindGroup,
    extract_pipeline: wgpu::RenderPipeline,
    downsample_pipeline: wgpu::RenderPipeline,
    blur_pipeline: wgpu::RenderPipeline,
    upsample_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    levels: Vec<BloomLevel>,
}

impl BloomPass {
    pub fn new(
        device: &wgpu::Device,
        surface_width: u32,
        surface_height: u32,
        scene_color_texture: &wgpu::Texture,
    ) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Bloom Linear Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let params_size = std::mem::size_of::<BloomParams>() as u64;
        let params_alignment = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let params_stride = params_size.div_ceil(params_alignment) * params_alignment;
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Bloom BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(params_size),
                    },
                    count: None,
                },
            ],
        });
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Bloom Parameters"),
            size: params_stride * BLOOM_PARAM_SLOT_COUNT as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let scene_source_view =
            scene_color_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let scene_source_bind_group = create_bind_group(
            device,
            &bind_group_layout,
            &scene_source_view,
            &sampler,
            &params_buffer,
            "Bloom Scene Source Bind Group",
        );
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Bloom Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let extract_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Bloom Extract Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/bloom_extract.wgsl").into()),
        });
        let filter_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Bloom Filter Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/bloom_filter.wgsl").into()),
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Bloom Composite Shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../shaders/bloom_composite.wgsl").into(),
            ),
        });

        let extract_pipeline = create_pipeline(
            device,
            &layout,
            &extract_shader,
            "fs_main",
            None,
            "Bloom Bright Pass Pipeline",
        );
        let downsample_pipeline = create_pipeline(
            device,
            &layout,
            &filter_shader,
            "fs_downsample",
            None,
            "Bloom Downsample Pipeline",
        );
        let blur_pipeline = create_pipeline(
            device,
            &layout,
            &filter_shader,
            "fs_blur",
            None,
            "Bloom Gaussian Blur Pipeline",
        );
        let additive_blend = Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::REPLACE,
        });
        let upsample_pipeline = create_pipeline(
            device,
            &layout,
            &composite_shader,
            "fs_upsample",
            additive_blend,
            "Bloom Upsample Pipeline",
        );
        let composite_pipeline = create_pipeline(
            device,
            &layout,
            &composite_shader,
            "fs_composite",
            additive_blend,
            "Bloom Scene Composite Pipeline",
        );
        let levels = create_levels(
            device,
            surface_width,
            surface_height,
            &bind_group_layout,
            &sampler,
            &params_buffer,
        );

        Self {
            enabled: bloom_enabled_from_environment(),
            sampler,
            bind_group_layout,
            params_buffer,
            params_stride,
            scene_source_view,
            scene_source_bind_group,
            extract_pipeline,
            downsample_pipeline,
            blur_pipeline,
            upsample_pipeline,
            composite_pipeline,
            levels,
        }
    }

    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        surface_width: u32,
        surface_height: u32,
        scene_color_texture: &wgpu::Texture,
    ) {
        self.scene_source_view =
            scene_color_texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.scene_source_bind_group = create_bind_group(
            device,
            &self.bind_group_layout,
            &self.scene_source_view,
            &self.sampler,
            &self.params_buffer,
            "Bloom Scene Source Bind Group",
        );
        self.levels = create_levels(
            device,
            surface_width,
            surface_height,
            &self.bind_group_layout,
            &self.sampler,
            &self.params_buffer,
        );
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    #[cfg(feature = "dev-tools")]
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Encode bloom against the currently bound HDR scene target. The source
    /// scene bind group is rebuilt on resize, so this function has no material
    /// or light-path dependency and works for any HDR scene contribution.
    pub fn record(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene_color_view: &wgpu::TextureView,
    ) {
        let mut params_slot = 0;
        let first = &self.levels[0];
        let params_offset = self.write_params(
            queue,
            &mut params_slot,
            first.dimensions,
            [0.0, 0.0],
            BLOOM_THRESHOLD,
            1.0,
        );
        encode_fullscreen_pass(
            encoder,
            "Bloom Bright Pass",
            &first.down_view,
            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            &self.extract_pipeline,
            &self.scene_source_bind_group,
            params_offset,
        );

        for level_index in 0..self.levels.len() {
            if level_index > 0 {
                let source = &self.levels[level_index - 1];
                let target = &self.levels[level_index];
                let params_offset = self.write_params(
                    queue,
                    &mut params_slot,
                    source.dimensions,
                    [0.0, 0.0],
                    0.0,
                    1.0,
                );
                encode_fullscreen_pass(
                    encoder,
                    "Bloom Downsample Pass",
                    &target.down_view,
                    wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    &self.downsample_pipeline,
                    &source.down_bind_group,
                    params_offset,
                );
            }

            let level = &self.levels[level_index];
            let params_offset = self.write_params(
                queue,
                &mut params_slot,
                level.dimensions,
                [1.0, 0.0],
                0.0,
                1.0,
            );
            encode_fullscreen_pass(
                encoder,
                "Bloom Horizontal Gaussian Pass",
                &level.blur_view,
                wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                &self.blur_pipeline,
                &level.down_bind_group,
                params_offset,
            );
            let params_offset = self.write_params(
                queue,
                &mut params_slot,
                level.dimensions,
                [0.0, 1.0],
                0.0,
                1.0,
            );
            encode_fullscreen_pass(
                encoder,
                "Bloom Vertical Gaussian Pass",
                &level.down_view,
                wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                &self.blur_pipeline,
                &level.blur_bind_group,
                params_offset,
            );
        }

        for level_index in (1..self.levels.len()).rev() {
            let source = &self.levels[level_index];
            let target = &self.levels[level_index - 1];
            let params_offset = self.write_params(
                queue,
                &mut params_slot,
                source.dimensions,
                [0.0, 0.0],
                0.0,
                1.0,
            );
            encode_fullscreen_pass(
                encoder,
                "Bloom Upsample Pass",
                &target.down_view,
                wgpu::LoadOp::Load,
                &self.upsample_pipeline,
                &source.down_bind_group,
                params_offset,
            );
        }

        let params_offset = self.write_params(
            queue,
            &mut params_slot,
            first.dimensions,
            [0.0, 0.0],
            0.0,
            BLOOM_INTENSITY,
        );
        encode_fullscreen_pass(
            encoder,
            "Bloom Scene Composite Pass",
            scene_color_view,
            wgpu::LoadOp::Load,
            &self.composite_pipeline,
            &first.down_bind_group,
            params_offset,
        );
        debug_assert_eq!(params_slot, BLOOM_PARAM_SLOT_COUNT);
    }

    fn write_params(
        &self,
        queue: &wgpu::Queue,
        params_slot: &mut usize,
        dimensions: (u32, u32),
        direction: [f32; 2],
        threshold: f32,
        intensity: f32,
    ) -> u32 {
        debug_assert!(*params_slot < BLOOM_PARAM_SLOT_COUNT);
        let params = BloomParams {
            texel_size: [1.0 / dimensions.0 as f32, 1.0 / dimensions.1 as f32],
            threshold,
            intensity,
            direction,
            _padding: [0.0; 2],
        };
        let offset = self.params_stride * *params_slot as u64;
        *params_slot += 1;
        queue.write_buffer(&self.params_buffer, offset, bytemuck::bytes_of(&params));
        offset
            .try_into()
            .expect("bloom parameter dynamic offset must fit in u32")
    }
}

fn bloom_enabled_from_environment() -> bool {
    match std::env::var("POSTRETRO_BLOOM") {
        Ok(value) => value != "0",
        Err(_) => BLOOM_ENABLED_BY_DEFAULT,
    }
}

fn create_levels(
    device: &wgpu::Device,
    surface_width: u32,
    surface_height: u32,
    bind_group_layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    params_buffer: &wgpu::Buffer,
) -> Vec<BloomLevel> {
    (0..BLOOM_LEVEL_COUNT)
        .map(|level_index| {
            let dimensions = bloom_level_dimensions(surface_width, surface_height, level_index);
            let (down_texture, down_view) = create_bloom_target(device, dimensions, "Bloom Down");
            let (blur_texture, blur_view) = create_bloom_target(device, dimensions, "Bloom Blur");
            let down_bind_group = create_bind_group(
                device,
                bind_group_layout,
                &down_view,
                sampler,
                params_buffer,
                "Bloom Down Bind Group",
            );
            let blur_bind_group = create_bind_group(
                device,
                bind_group_layout,
                &blur_view,
                sampler,
                params_buffer,
                "Bloom Blur Bind Group",
            );
            BloomLevel {
                down_texture,
                down_view,
                blur_texture,
                blur_view,
                down_bind_group,
                blur_bind_group,
                dimensions,
            }
        })
        .collect()
}

fn bloom_level_dimensions(width: u32, height: u32, level_index: usize) -> (u32, u32) {
    let divisor = 1u32 << (level_index as u32 + 1);
    (
        width.max(1).div_ceil(divisor).max(1),
        height.max(1).div_ceil(divisor).max(1),
    )
}

fn create_bloom_target(
    device: &wgpu::Device,
    dimensions: (u32, u32),
    label_prefix: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(&format!("{label_prefix} Texture")),
        size: wgpu::Extent3d {
            width: dimensions.0,
            height: dimensions.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SCENE_COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    params_buffer: &wgpu::Buffer,
    label: &'static str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: params_buffer,
                    offset: 0,
                    size: NonZeroU64::new(std::mem::size_of::<BloomParams>() as u64),
                }),
            },
        ],
    })
}

fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    fragment_entry: &'static str,
    blend: Option<wgpu::BlendState>,
    label: &'static str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format: SCENE_COLOR_FORMAT,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn encode_fullscreen_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &'static str,
    target: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    params_offset: u32,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[params_offset]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bloom_levels_keep_nonzero_dimensions() {
        assert_eq!(bloom_level_dimensions(1, 1, 0), (1, 1));
        assert_eq!(bloom_level_dimensions(1920, 1080, 0), (960, 540));
        assert_eq!(bloom_level_dimensions(1920, 1080, 4), (60, 34));
    }

    #[test]
    fn bloom_wgsl_sources_parse_and_validate() {
        for source in [
            include_str!("../shaders/bloom_extract.wgsl"),
            include_str!("../shaders/bloom_filter.wgsl"),
            include_str!("../shaders/bloom_composite.wgsl"),
        ] {
            let module =
                naga::front::wgsl::parse_str(source).expect("bloom shader should parse as WGSL");
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .expect("bloom shader should pass naga validation");
        }
    }
}
