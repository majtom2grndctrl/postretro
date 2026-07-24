// HDR bloom post-process resources and passes.
// See: context/lib/rendering_pipeline.md §7.8

use std::num::NonZeroU64;

use super::SCENE_COLOR_FORMAT;
use wgpu::util::DeviceExt;

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
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
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
        let level_dimensions = bloom_level_dimensions_table(surface_width, surface_height);
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
        let params_buffer = create_params_buffer(
            device,
            params_stride,
            (surface_width, surface_height),
            &level_dimensions,
        );
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
        let additive_blend = Some(additive_bloom_blend());
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
            &level_dimensions,
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
        let level_dimensions = bloom_level_dimensions_table(surface_width, surface_height);
        self.params_buffer = create_params_buffer(
            device,
            self.params_stride,
            (surface_width, surface_height),
            &level_dimensions,
        );
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
            &level_dimensions,
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
    pub fn record(&self, encoder: &mut wgpu::CommandEncoder, scene_color_view: &wgpu::TextureView) {
        let mut params_slot = 0;
        let first = &self.levels[0];
        let params_offset = self.next_params_offset(&mut params_slot);
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
                let params_offset = self.next_params_offset(&mut params_slot);
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
            let params_offset = self.next_params_offset(&mut params_slot);
            encode_fullscreen_pass(
                encoder,
                "Bloom Horizontal Gaussian Pass",
                &level.blur_view,
                wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                &self.blur_pipeline,
                &level.down_bind_group,
                params_offset,
            );
            let params_offset = self.next_params_offset(&mut params_slot);
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
            let params_offset = self.next_params_offset(&mut params_slot);
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

        let params_offset = self.next_params_offset(&mut params_slot);
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

    fn next_params_offset(&self, params_slot: &mut usize) -> u32 {
        debug_assert!(*params_slot < BLOOM_PARAM_SLOT_COUNT);
        let offset = self.params_stride * *params_slot as u64;
        *params_slot += 1;
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
    level_dimensions: &[(u32, u32); BLOOM_LEVEL_COUNT],
    bind_group_layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    params_buffer: &wgpu::Buffer,
) -> Vec<BloomLevel> {
    level_dimensions
        .iter()
        .copied()
        .map(|dimensions| {
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
            }
        })
        .collect()
}

fn bloom_level_dimensions_table(width: u32, height: u32) -> [(u32, u32); BLOOM_LEVEL_COUNT] {
    std::array::from_fn(|level_index| bloom_level_dimensions(width, height, level_index))
}

fn bloom_level_dimensions(width: u32, height: u32, level_index: usize) -> (u32, u32) {
    let divisor = 1u32 << (level_index as u32 + 1);
    (
        width.max(1).div_ceil(divisor).max(1),
        height.max(1).div_ceil(divisor).max(1),
    )
}

fn bloom_params(
    dimensions: (u32, u32),
    direction: [f32; 2],
    threshold: f32,
    intensity: f32,
) -> BloomParams {
    BloomParams {
        texel_size: [1.0 / dimensions.0 as f32, 1.0 / dimensions.1 as f32],
        threshold,
        intensity,
        direction,
        _padding: [0.0; 2],
    }
}

fn bloom_parameter_slots(
    surface_dimensions: (u32, u32),
    level_dimensions: &[(u32, u32); BLOOM_LEVEL_COUNT],
) -> Vec<BloomParams> {
    let mut slots = Vec::with_capacity(BLOOM_PARAM_SLOT_COUNT);
    slots.push(bloom_params(
        surface_dimensions,
        [0.0, 0.0],
        BLOOM_THRESHOLD,
        1.0,
    ));

    for level_index in 0..BLOOM_LEVEL_COUNT {
        if level_index > 0 {
            slots.push(bloom_params(
                level_dimensions[level_index - 1],
                [0.0, 0.0],
                0.0,
                1.0,
            ));
        }
        slots.push(bloom_params(
            level_dimensions[level_index],
            [1.0, 0.0],
            0.0,
            1.0,
        ));
        slots.push(bloom_params(
            level_dimensions[level_index],
            [0.0, 1.0],
            0.0,
            1.0,
        ));
    }

    for source_dimensions in level_dimensions[1..].iter().rev() {
        slots.push(bloom_params(*source_dimensions, [0.0, 0.0], 0.0, 1.0));
    }

    slots.push(bloom_params(
        level_dimensions[0],
        [0.0, 0.0],
        0.0,
        BLOOM_INTENSITY,
    ));
    debug_assert_eq!(slots.len(), BLOOM_PARAM_SLOT_COUNT);
    slots
}

fn create_params_buffer(
    device: &wgpu::Device,
    params_stride: u64,
    surface_dimensions: (u32, u32),
    level_dimensions: &[(u32, u32); BLOOM_LEVEL_COUNT],
) -> wgpu::Buffer {
    let slots = bloom_parameter_slots(surface_dimensions, level_dimensions);
    let buffer_size = params_stride * BLOOM_PARAM_SLOT_COUNT as u64;
    let buffer_size =
        usize::try_from(buffer_size).expect("bloom parameter buffer size must fit in usize");
    let params_size = std::mem::size_of::<BloomParams>();
    let mut bytes = vec![0; buffer_size];
    for (slot, params) in slots.iter().enumerate() {
        let offset = usize::try_from(params_stride)
            .expect("bloom parameter stride must fit in usize")
            * slot;
        bytes[offset..offset + params_size].copy_from_slice(bytemuck::bytes_of(params));
    }
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Bloom Parameters"),
        contents: &bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

fn additive_bloom_blend() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        // Bloom changes scene radiance, not coverage. Preserve destination alpha
        // through both the intermediate upsample and final scene composite.
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    }
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
    use postretro_render_data::material::Material;

    #[test]
    fn neon_emissive_strength_clears_the_bloom_threshold() {
        let strength = Material::Neon.emissive_strength();
        assert!(strength > 1.0, "neon must produce HDR emissive output");
        assert!(
            strength > BLOOM_THRESHOLD,
            "neon authored at full emissive value must pass the bloom bright-pass"
        );
    }

    #[test]
    fn bloom_levels_keep_nonzero_dimensions() {
        assert_eq!(bloom_level_dimensions(1, 1, 0), (1, 1));
        assert_eq!(bloom_level_dimensions(1920, 1080, 0), (960, 540));
        assert_eq!(bloom_level_dimensions(1920, 1080, 4), (60, 34));
    }

    #[test]
    fn bloom_parameter_slots_follow_record_order_and_resize_dimensions() {
        let dimensions = bloom_level_dimensions_table(1920, 1080);
        let slots = bloom_parameter_slots((1920, 1080), &dimensions);
        assert_eq!(slots.len(), BLOOM_PARAM_SLOT_COUNT);
        assert_eq!(slots[0].texel_size, [1.0 / 1920.0, 1.0 / 1080.0]);
        assert_eq!(slots[0].threshold, BLOOM_THRESHOLD);
        assert_eq!(slots[3].texel_size, [1.0 / 960.0, 1.0 / 540.0]);
        assert_eq!(slots[15].texel_size, [1.0 / 60.0, 1.0 / 34.0]);
        assert_eq!(slots[19].intensity, BLOOM_INTENSITY);

        let resized_dimensions = bloom_level_dimensions_table(1279, 719);
        let resized_slots = bloom_parameter_slots((1279, 719), &resized_dimensions);
        assert_eq!(resized_slots.len(), BLOOM_PARAM_SLOT_COUNT);
        assert_eq!(resized_slots[0].texel_size, [1.0 / 1279.0, 1.0 / 719.0]);
        assert_eq!(resized_slots[1].texel_size, [1.0 / 640.0, 1.0 / 360.0]);
    }

    #[test]
    fn additive_bloom_blend_preserves_destination_alpha() {
        let blend = additive_bloom_blend();
        assert_eq!(blend.color.src_factor, wgpu::BlendFactor::One);
        assert_eq!(blend.color.dst_factor, wgpu::BlendFactor::One);
        assert_eq!(blend.color.operation, wgpu::BlendOperation::Add);
        assert_eq!(blend.alpha.src_factor, wgpu::BlendFactor::Zero);
        assert_eq!(blend.alpha.dst_factor, wgpu::BlendFactor::One);
        assert_eq!(blend.alpha.operation, wgpu::BlendOperation::Add);
    }

    #[test]
    fn bloom_extract_thresholds_source_texels_before_reduction() {
        // Regression: filtering first could average a thin HDR texel down to the
        // threshold and erase its bloom contribution.
        let source = include_str!("../shaders/bloom_extract.wgsl");
        let source_load = source
            .find("textureLoad(")
            .expect("extract shader must load unfiltered source texels");
        let threshold = source
            .find("let excess =")
            .expect("extract shader must threshold each source texel");
        let reduction = source
            .find("extracted * 0.25")
            .expect("extract shader must reduce thresholded source texels");
        assert!(source_load < threshold);
        assert!(threshold < reduction);
        assert!(!source.contains("textureSample(bloom_source"));
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
