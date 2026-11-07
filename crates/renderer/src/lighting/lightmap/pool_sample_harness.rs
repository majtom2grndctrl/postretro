// GPU readback harness for the forward shader's cell-block lightmap path: probes
// resolved through the real block table and sampled through the real WGSL
// helpers (irradiance, direction, shadowmask, and the gate that lets face owners
// read their mask channels), spliced verbatim from lightmap_sample.wgsl and
// forward.wgsl.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// `pool_sample_test` and the streamed-pool tests (`stream/tests.rs`) use it.
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests": the
// helpers are WGSL, so verifying them means running them. Each probe renders
// two pixels of a one-row target. The harness self-skips without a BC-capable
// adapter and says so; a skipped run proves nothing. Set
// `POSTRETRO_REQUIRE_GPU` to any non-empty value other than `0` to make a
// missing adapter fail the test instead of skipping it.

use postretro_render_cpu::frame_uniforms::SPEC_SHADOWMASK_FLAG_MASK_PRESENT;

use super::filtering_sampler_descriptor;
use super::test_fixtures::lightmap_block_decode_wgsl;

use crate::render::gpu_test_harness::{GpuCtx, wgsl_const_line, wgsl_item};

const FORWARD_WGSL: &str = include_str!("../../shaders/forward.wgsl");
const LIGHTMAP_SAMPLE_WGSL: &str = include_str!("../../shaders/lightmap_sample.wgsl");
const PROBE_BYTES: usize = 16;
const OUTPUT_TEXEL_BYTES: u32 = 16;
const PIXELS_PER_PROBE: u32 = 2;
const TARGET_COUNT: usize = 2;
pub(super) const SCALE: u32 = 2;

/// What a probe's fragment writes, after the entry point has its block's
/// `texel`, `layer_flags`, `rect`, `plane_addresses`, its `pixel` and its
/// `probe`.
const SAMPLE_BLOCK_WGSL: &str = r#"
    let irradiance = sample_lightmap_irradiance(texel, layer_flags, rect);
    let direction = sample_lightmap_direction(texel, layer_flags, rect);
    let mask = sample_shadowmask_atlas(texel, layer_flags, rect, plane_addresses);
    let missing = lightmap_block_missing(layer_flags);
    // World static specular's own gate and channel select for the owner at
    // `owner_position`: the visibility it multiplies that owner's highlight
    // by, or 0 when owners may not read the mask at all.
    let readable = static_spec_owners_readable(layer_flags);
    let selects = vec4<f32>(
        select(0.0, shadowmask_channel_value(mask, probe.owner_position), readable),
        shadowmask_channel_value(mask, 0u),
        select(0.0, 1.0, missing),
        select(0.0, 1.0, readable),
    );
    let odd = (pixel & 1u) == 1u;
    var out: HarnessOut;
    out.sampled = select(vec4<f32>(irradiance, 1.0), direction, odd);
    out.mask = select(mask, selects, odd);
    return out;
"#;

/// The structs both harness shaders share.
pub(super) const PROBE_TYPES_WGSL: &str = r#"
struct HarnessUniforms {
    spec_shadowmask_flags: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};
struct Probe {
    uv: vec2<f32>,
    block: u32,
    owner_position: u32,
};
struct HarnessOut {
    // Even pixel: irradiance. Odd pixel: direction.
    @location(0) sampled: vec4<f32>,
    // Even pixel: the shadowmask. Odd pixel: gated selects.
    @location(1) mask: vec4<f32>,
};
"#;

/// `fn probe_out(...)`: the fragment body every harness runs, over a block's
/// varyings as the fragment stage receives them.
pub(super) fn probe_out_wgsl() -> String {
    format!(
        "fn probe_out(probe: Probe, pixel: u32, texel: vec2<f32>, layer_flags: u32, rect: \
         vec4<u32>, plane_addresses: vec2<u32>) -> HarnessOut {{{SAMPLE_BLOCK_WGSL}}}"
    )
}

/// The lightmap helpers both harness shaders splice in.
pub(super) fn helpers_wgsl() -> String {
    let decode = lightmap_block_decode_wgsl();
    let mut items: Vec<&str> = vec![
        decode.as_str(),
        wgsl_const_line(FORWARD_WGSL, "SPEC_SHADOWMASK_FLAG_MASK_PRESENT"),
    ];
    for name in [
        "lightmap_block_missing",
        "sample_lightmap_irradiance",
        "sample_lightmap_direction",
    ] {
        items.push(wgsl_item(LIGHTMAP_SAMPLE_WGSL, &format!("fn {name}(")));
    }
    for name in [
        "shadowmask_channel_value",
        "static_spec_mask_present",
        "static_spec_owners_readable",
    ] {
        items.push(wgsl_item(FORWARD_WGSL, &format!("fn {name}(")));
    }
    items.join("\n\n")
}

fn shader_source() -> String {
    let prelude = r#"
@group(0) @binding(0) var lightmap_irradiance: texture_2d_array<f32>;
@group(0) @binding(1) var lightmap_direction: texture_2d_array<f32>;
@group(0) @binding(2) var shadowmask_atlas: texture_2d_array<f32>;
@group(0) @binding(3) var lightmap_filtering_sampler: sampler;
@group(0) @binding(4) var lightmap_sampler: sampler;
@group(0) @binding(5) var<storage, read> lightmap_block_table: array<vec4<u32>>;
@group(0) @binding(6) var<storage, read> probes: array<Probe>;
@group(0) @binding(7) var<uniform> uniforms: HarnessUniforms;
"#;
    let entry = r#"
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> HarnessOut {
    let pixel = u32(position.x);
    let probe = probes[pixel / 2u];
    let block = resolve_lightmap_block(probe.block, probe.uv);
    return probe_out(probe, pixel, block.texel, block.layer_flags, block.rect, block.plane_addresses);
}
"#;
    let (helpers, probe_out) = (helpers_wgsl(), probe_out_wgsl());
    format!("{PROBE_TYPES_WGSL}\n{prelude}\n{helpers}\n{probe_out}\n{entry}")
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Probe {
    /// Block-local unorm UV, as the vertex attribute carries it.
    pub(super) uv: [f32; 2],
    /// Cell block id + 1, as `WorldVertex::lightmap_block` names it.
    pub(super) block: u32,
    /// Owner position (mask channel 0..3) whose visibility the probe reads.
    pub(super) owner_position: u32,
}

impl Probe {
    pub(super) fn bytes(self) -> [u8; PROBE_BYTES] {
        let words = [
            self.uv[0].to_bits(),
            self.uv[1].to_bits(),
            self.block,
            self.owner_position,
        ];
        let mut out = [0u8; PROBE_BYTES];
        for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            chunk.copy_from_slice(&word.to_ne_bytes());
        }
        out
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct ProbeResult {
    pub(super) irradiance: [f32; 4],
    pub(super) direction: [f32; 4],
    pub(super) mask: [f32; 4],
    /// The visibility world static specular gives the probe's owner.
    pub(super) owner_visibility: f32,
    /// The union's channel-0 read, ungated: the mask value owner position 0
    /// selects.
    pub(super) union_channel0_visibility: f32,
    pub(super) missing: bool,
    /// Whether owners may read their mask at this fragment.
    pub(super) owners_readable: bool,
}

pub(super) struct BoundLightmap {
    pub(super) irradiance: wgpu::Texture,
    pub(super) direction: wgpu::Texture,
    pub(super) shadowmask: wgpu::Texture,
    pub(super) table: Vec<u8>,
    /// The frame uniforms' mask-present bit for this binding: set for a real
    /// id-42 pool, clear for the placeholder.
    pub(super) mask_present: bool,
}

pub(super) fn irradiance_texel(block: usize, x: u32, y: u32) -> [f32; 4] {
    [block as f32 + 1.0, x as f32, y as f32, 0.5]
}

pub(super) fn direction_texel(block: usize, x: u32, y: u32) -> [u8; 2] {
    [
        (10 + 40 * block as u32 + 3 * x) as u8,
        (7 + 5 * y + 50 * block as u32) as u8,
    ]
}

pub(super) fn mask_byte(block: usize, channel: u32, bx: u32, by: u32) -> u8 {
    (10 + channel * 50 + bx * 20 + by * 7 + block as u32 * 3) as u8
}

pub(super) fn run_probes(
    ctx: &GpuCtx,
    bound: &BoundLightmap,
    probes: &[Probe],
) -> Vec<ProbeResult> {
    use wgpu::util::DeviceExt;

    let device = &ctx.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("pool_sample_test shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source().into()),
    });
    let array_view = |texture: &wgpu::Texture| {
        texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
    };
    let views = [
        array_view(&bound.irradiance),
        array_view(&bound.direction),
        array_view(&bound.shadowmask),
    ];
    let filtering = device.create_sampler(&filtering_sampler_descriptor());
    let nearest = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("pool_sample_test nearest"),
        ..Default::default()
    });
    let table = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("block table"),
        contents: &bound.table,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let probe_bytes: Vec<u8> = probes.iter().flat_map(|probe| probe.bytes()).collect();
    let probe_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("probes"),
        contents: &probe_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let mut uniform_bytes = [0u8; 16];
    if bound.mask_present {
        uniform_bytes[..4].copy_from_slice(&SPEC_SHADOWMASK_FLAG_MASK_PRESENT.to_ne_bytes());
    }
    let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("harness uniforms"),
        contents: &uniform_bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    });

    let texture_entry = |binding: u32, filterable: bool| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    };
    let buffer_entry = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("pool_sample_test layout"),
        entries: &[
            texture_entry(0, true),
            texture_entry(1, false),
            texture_entry(2, true),
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                count: None,
            },
            buffer_entry(5, wgpu::BufferBindingType::Storage { read_only: true }),
            buffer_entry(6, wgpu::BufferBindingType::Storage { read_only: true }),
            buffer_entry(7, wgpu::BufferBindingType::Uniform),
        ],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("pool_sample_test bind group"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&views[0]),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&views[1]),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&views[2]),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&filtering),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&nearest),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: table.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: probe_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: uniforms.as_entire_binding(),
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("pool_sample_test pipeline layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let (target_state, targets) = (
        ProbeTargets::color_target(),
        ProbeTargets::new(ctx, probes.len()),
    );
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("pool_sample_test pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[target_state.clone(), target_state],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = targets.begin_pass(&mut encoder);
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
    targets.read(ctx, encoder)
}

/// The two-target, one-row render every probe harness reads back.
pub(super) struct ProbeTargets {
    targets: Vec<wgpu::Texture>,
    views: Vec<wgpu::TextureView>,
    readbacks: Vec<wgpu::Buffer>,
    width: u32,
    row_bytes: u32,
}

impl ProbeTargets {
    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

    pub(super) fn color_target() -> Option<wgpu::ColorTargetState> {
        Some(wgpu::ColorTargetState {
            format: Self::FORMAT,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })
    }

    pub(super) fn new(ctx: &GpuCtx, probes: usize) -> Self {
        let device = &ctx.device;
        let width = probes as u32 * PIXELS_PER_PROBE;
        let targets: Vec<wgpu::Texture> = (0..TARGET_COUNT)
            .map(|_| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("pool_sample_test target"),
                    size: wgpu::Extent3d {
                        width,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: Self::FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
            })
            .collect();
        let views = targets
            .iter()
            .map(|target| target.create_view(&Default::default()))
            .collect();
        let row_bytes =
            (width * OUTPUT_TEXEL_BYTES).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readbacks = (0..TARGET_COUNT)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("pool_sample_test readback"),
                    size: u64::from(row_bytes),
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        Self {
            targets,
            views,
            readbacks,
            width,
            row_bytes,
        }
    }

    /// Pixels probe `index` covers, for a scissor.
    pub(super) fn probe_pixels(index: u32) -> (u32, u32) {
        (index * PIXELS_PER_PROBE, PIXELS_PER_PROBE)
    }

    pub(super) fn begin_pass<'e>(
        &self,
        encoder: &'e mut wgpu::CommandEncoder,
    ) -> wgpu::RenderPass<'e> {
        let attachments: Vec<_> = self
            .views
            .iter()
            .map(|view| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })
            })
            .collect();
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pool_sample_test pass"),
            color_attachments: &attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    /// Submit `encoder` with the target copies and read every probe back.
    pub(super) fn read(&self, ctx: &GpuCtx, mut encoder: wgpu::CommandEncoder) -> Vec<ProbeResult> {
        let (device, width) = (&ctx.device, self.width);
        for (target, readback) in self.targets.iter().zip(&self.readbacks) {
            encoder.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(self.row_bytes),
                        rows_per_image: Some(1),
                    },
                },
                wgpu::Extent3d {
                    width,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        ctx.queue.submit([encoder.finish()]);

        let read = |buffer: &wgpu::Buffer| -> Vec<[f32; 4]> {
            let slice = buffer.slice(..);
            slice.map_async(wgpu::MapMode::Read, |result| {
                result.expect("map pool_sample_test readback");
            });
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("poll pool_sample_test device");
            let bytes = slice
                .get_mapped_range()
                .expect("buffer mapped for readback");
            let texels = bytes[..(width * OUTPUT_TEXEL_BYTES) as usize]
                .as_chunks::<{ OUTPUT_TEXEL_BYTES as usize }>()
                .0
                .iter()
                .map(|texel| {
                    std::array::from_fn(|c| {
                        f32::from_ne_bytes(texel[c * 4..c * 4 + 4].try_into().unwrap())
                    })
                })
                .collect();
            drop(bytes);
            buffer.unmap();
            texels
        };
        let sampled = read(&self.readbacks[0]);
        let masks = read(&self.readbacks[1]);
        sampled
            .as_chunks::<{ PIXELS_PER_PROBE as usize }>()
            .0
            .iter()
            .zip(masks.as_chunks::<{ PIXELS_PER_PROBE as usize }>().0.iter())
            .map(|(sampled, masks)| ProbeResult {
                irradiance: sampled[0],
                direction: sampled[1],
                mask: masks[0],
                owner_visibility: masks[1][0],
                union_channel0_visibility: masks[1][1],
                missing: masks[1][2] == 1.0,
                owners_readable: masks[1][3] == 1.0,
            })
            .collect()
    }
}

pub(super) fn assert_near(actual: &[f32], expected: &[f32], tolerance: f32, what: &str) {
    for (component, (&a, &e)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (a - e).abs() <= tolerance,
            "{what}: component {component} is {a}, expected {e} (all: {actual:?} vs {expected:?})"
        );
    }
}

/// The texels block `block`'s own local texel `(x, y)` holds.
pub(super) fn expected_texel(block: usize, x: u32, y: u32) -> ([f32; 3], [f32; 2], [f32; 4]) {
    let [r, g, b, _] = irradiance_texel(block, x, y);
    let direction = direction_texel(block, x / SCALE, y / SCALE).map(|c| f32::from(c) / 255.0);
    let mask = std::array::from_fn(|channel| {
        f32::from(mask_byte(block, channel as u32, x / 4, y / 4)) / 255.0
    });
    ([r, g, b], direction, mask)
}

pub(super) fn resident_probe(uv: [f32; 2], block: u32) -> Probe {
    Probe {
        uv,
        block: block + 1,
        owner_position: 0,
    }
}
