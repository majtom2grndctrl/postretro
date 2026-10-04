// GPU readback of the forward shader's cell-block lightmap path: blocks
// placed at non-zero offsets on two pool layers through the renderer's own
// pool upload, resolved through the real block table, and sampled through the
// real WGSL helpers — irradiance, direction, shadowmask, and the
// shadowmask-gated specular select — spliced verbatim from
// lightmap_sample.wgsl and forward.wgsl.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// The streamed-pool tests (`stream/tests.rs`) reuse this harness.
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests": the
// helpers are WGSL, so verifying them means running them. Each probe renders
// two pixels of a one-row target. The harness self-skips without a BC-capable
// adapter and says so; a skipped run proves nothing. Set
// `POSTRETRO_REQUIRE_GPU` to any non-empty value other than `0` to make a
// missing adapter fail the test instead of skipping it.

use postretro_level_format::shadowmask_atlas::SHADOWMASK_CHANNEL_DROPPED;
use postretro_render_cpu::lightmap_pool::{
    AllResidentPool, BlockPlacement, block_table_bytes, placeholder_block_table,
};

use super::pool::upload_static_pool;
use super::test_fixtures::{FixtureTexels, block_fixture};
use super::upload::{upload_placeholder_direction, upload_placeholder_irradiance};
use super::{StaticPoolPlan, filtering_sampler_descriptor, upload_placeholder_shadowmask};

const FORWARD_WGSL: &str = include_str!("../../shaders/forward.wgsl");
const LIGHTMAP_SAMPLE_WGSL: &str = include_str!("../../shaders/lightmap_sample.wgsl");
const PROBE_BYTES: usize = 16;
const OUTPUT_TEXEL_BYTES: u32 = 16;
const PIXELS_PER_PROBE: u32 = 2;
const TARGET_COUNT: usize = 2;

/// Resident blocks 0–3 and a fifth, unplaced block 4. Block 1 sits flush
/// right of block 0 and block 3 flush below block 2, so a clamp that let a
/// bilinear tap leave its block would read the neighbour's distinct values.
const EXTENTS: [(u32, u32); 5] = [(8, 8), (12, 8), (8, 12), (8, 8), (8, 8)];
const PLACEMENTS: [BlockPlacement; 4] = [
    BlockPlacement {
        layer: 0,
        x: 16,
        y: 8,
    },
    BlockPlacement {
        layer: 0,
        x: 24,
        y: 8,
    },
    BlockPlacement {
        layer: 1,
        x: 40,
        y: 20,
    },
    BlockPlacement {
        layer: 1,
        x: 40,
        y: 32,
    },
];
const MISSING_BLOCK: u32 = 4;
pub(super) const SCALE: u32 = 2;

pub(super) struct GpuCtx {
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
}

fn try_init_gpu() -> Option<GpuCtx> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok()?;
    if !adapter
        .features()
        .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
    {
        return None;
    }
    eprintln!(
        "[pool_sample_test] running on {:?}",
        adapter.get_info().name
    );
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("pool_sample_test Device"),
        required_features: wgpu::Features::TEXTURE_COMPRESSION_BC,
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .ok()?;
    Some(GpuCtx { device, queue })
}

pub(super) fn gpu_or_skip(test: &str) -> Option<GpuCtx> {
    if let Some(ctx) = try_init_gpu() {
        return Some(ctx);
    }
    let required =
        std::env::var("POSTRETRO_REQUIRE_GPU").is_ok_and(|value| !value.is_empty() && value != "0");
    assert!(
        !required,
        "[pool_sample_test] {test}: POSTRETRO_REQUIRE_GPU is set but no BC-capable GPU adapter \
         is available"
    );
    eprintln!("[pool_sample_test] skipping {test}: no BC-capable GPU adapter available");
    None
}

/// The text of `fn name(` or `struct name {` through its matching brace.
fn wgsl_item<'a>(source: &'a str, header: &str) -> &'a str {
    let start = source
        .find(header)
        .unwrap_or_else(|| panic!("the WGSL source must declare `{header}`"));
    let open = start + source[start..].find('{').expect("item body");
    let mut depth = 0usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    // Structs end `};`; keep the semicolon with them.
                    let end = open + offset + 1;
                    let end = if source[end..].starts_with(';') {
                        end + 1
                    } else {
                        end
                    };
                    return &source[start..end];
                }
            }
            _ => {}
        }
    }
    panic!("`{header}` body never closes");
}

fn wgsl_const_line<'a>(source: &'a str, name: &str) -> &'a str {
    source
        .lines()
        .find(|line| line.starts_with(&format!("const {name}:")))
        .unwrap_or_else(|| panic!("the WGSL source must declare `{name}`"))
}

fn shader_source() -> String {
    let prelude = r#"
struct SpecLight {
    position_and_range: vec4<f32>,
    color_and_pad:      vec4<f32>,
    cone_dir_and_type:  vec4<f32>,
    cone_cos:           vec4<f32>,
};
struct HarnessUniforms {
    spec_shadowmask_force_one: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};
struct Probe {
    uv: vec2<f32>,
    block: u32,
    spec_channel: f32,
};
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
struct HarnessOut {
    // Even pixel: irradiance. Odd pixel: direction.
    @location(0) sampled: vec4<f32>,
    // Even pixel: the shadowmask. Odd pixel: gated selects.
    @location(1) mask: vec4<f32>,
};

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
    let irradiance = sample_lightmap_irradiance(block.texel, block.layer_flags, block.rect);
    let direction = sample_lightmap_direction(block.texel, block.layer_flags, block.rect);
    let mask = sample_shadowmask_atlas(block.texel, block.layer_flags, block.rect);
    let missing = lightmap_block_missing(block.layer_flags);
    var spec: SpecLight;
    spec.cone_cos = vec4<f32>(0.0, 0.0, probe.spec_channel, 0.0);
    let selects = vec4<f32>(
        shadowmask_visibility_for_spec_light(spec, mask, missing),
        shadowmask_channel_value(mask, 0u),
        select(0.0, 1.0, missing),
        0.0,
    );
    let odd = (pixel & 1u) == 1u;
    var out: HarnessOut;
    out.sampled = select(vec4<f32>(irradiance, 1.0), direction, odd);
    out.mask = select(mask, selects, odd);
    return out;
}
"#;
    let mut items: Vec<&str> = [
        "LIGHTMAP_POOL_LAYER_EDGE",
        "LIGHTMAP_BLOCK_RESIDENT",
        "LIGHTMAP_BLOCK_NONE",
    ]
    .into_iter()
    .map(|name| wgsl_const_line(LIGHTMAP_SAMPLE_WGSL, name))
    .collect();
    items.push(wgsl_const_line(FORWARD_WGSL, "SHADOWMASK_CHANNEL_DROPPED"));
    items.push(wgsl_item(
        LIGHTMAP_SAMPLE_WGSL,
        "struct LightmapBlockVaryings {",
    ));
    for name in [
        "resolve_lightmap_block",
        "lightmap_block_layer",
        "lightmap_block_resident",
        "lightmap_block_missing",
        "lightmap_pool_uv",
        "sample_lightmap_irradiance",
        "sample_lightmap_direction",
        "sample_shadowmask_atlas",
    ] {
        items.push(wgsl_item(LIGHTMAP_SAMPLE_WGSL, &format!("fn {name}(")));
    }
    for name in [
        "shadowmask_channel_value",
        "shadowmask_visibility_for_spec_light",
    ] {
        items.push(wgsl_item(FORWARD_WGSL, &format!("fn {name}(")));
    }
    format!("{prelude}\n{}\n{entry}", items.join("\n\n"))
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Probe {
    /// Block-local unorm UV, as the vertex attribute carries it.
    pub(super) uv: [f32; 2],
    /// Cell block id + 1, as `WorldVertex::lightmap_block` names it.
    pub(super) block: u32,
    /// Mask slot of the specular light the probe selects for, or the dropped
    /// sentinel.
    pub(super) spec_channel: f32,
}

impl Probe {
    fn bytes(self) -> [u8; PROBE_BYTES] {
        let words = [
            self.uv[0].to_bits(),
            self.uv[1].to_bits(),
            self.block,
            self.spec_channel.to_bits(),
        ];
        let mut out = [0u8; PROBE_BYTES];
        for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            chunk.copy_from_slice(&word.to_ne_bytes());
        }
        out
    }
}

#[derive(Debug)]
pub(super) struct ProbeResult {
    pub(super) irradiance: [f32; 4],
    pub(super) direction: [f32; 4],
    pub(super) mask: [f32; 4],
    pub(super) spec_visibility: f32,
    pub(super) union_slot0_visibility: f32,
    pub(super) missing: bool,
}

pub(super) struct BoundLightmap {
    pub(super) irradiance: wgpu::Texture,
    pub(super) direction: wgpu::Texture,
    pub(super) shadowmask: wgpu::Texture,
    pub(super) table: Vec<u8>,
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

pub(super) fn mask_byte(block: usize, slot: u32, bx: u32, by: u32) -> u8 {
    (10 + slot * 50 + bx * 20 + by * 7 + block as u32 * 3) as u8
}

/// The two-layer pool holding blocks 0–3, and a table naming them resident
/// and block 4 missing.
fn fixture_pool(ctx: &GpuCtx) -> BoundLightmap {
    let resident = &EXTENTS[..PLACEMENTS.len()];
    let fixture = block_fixture(
        resident,
        SCALE,
        &FixtureTexels {
            irradiance: &irradiance_texel,
            direction: &direction_texel,
            shadowmask: Some(&mask_byte),
        },
    );
    let plan = StaticPoolPlan {
        header: fixture.index.header,
        extents: resident.to_vec(),
        pool: AllResidentPool {
            placements: PLACEMENTS.to_vec(),
            layer_count: 2,
        },
        with_shadowmask: true,
    };
    let textures = upload_static_pool(&ctx.device, &ctx.queue, &plan, &fixture.payloads);
    let mut placements: Vec<_> = PLACEMENTS.iter().copied().map(Some).collect();
    placements.push(None);
    BoundLightmap {
        irradiance: textures.irradiance,
        direction: textures.direction,
        shadowmask: textures.shadowmask.expect("the plan keeps the shadowmask"),
        table: block_table_bytes(&EXTENTS, &placements),
    }
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
    let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("harness uniforms"),
        contents: &[0u8; 16],
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
    let target_format = wgpu::TextureFormat::Rgba32Float;
    let target_state = Some(wgpu::ColorTargetState {
        format: target_format,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    });
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

    let width = probes.len() as u32 * PIXELS_PER_PROBE;
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
                format: target_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        })
        .collect();
    let target_views: Vec<_> = targets
        .iter()
        .map(|target| target.create_view(&Default::default()))
        .collect();
    let row_bytes =
        (width * OUTPUT_TEXEL_BYTES).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readbacks: Vec<wgpu::Buffer> = (0..TARGET_COUNT)
        .map(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("pool_sample_test readback"),
                size: u64::from(row_bytes),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })
        .collect();

    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let attachments: Vec<_> = target_views
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
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pool_sample_test pass"),
            color_attachments: &attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
    for (target, readback) in targets.iter().zip(&readbacks) {
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
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
        let bytes = slice.get_mapped_range();
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
    let sampled = read(&readbacks[0]);
    let masks = read(&readbacks[1]);
    sampled
        .as_chunks::<{ PIXELS_PER_PROBE as usize }>()
        .0
        .iter()
        .zip(masks.as_chunks::<{ PIXELS_PER_PROBE as usize }>().0.iter())
        .map(|(sampled, masks)| ProbeResult {
            irradiance: sampled[0],
            direction: sampled[1],
            mask: masks[0],
            spec_visibility: masks[1][0],
            union_slot0_visibility: masks[1][1],
            missing: masks[1][2] == 1.0,
        })
        .collect()
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
    let mask =
        std::array::from_fn(|slot| f32::from(mask_byte(block, slot as u32, x / 4, y / 4)) / 255.0);
    ([r, g, b], direction, mask)
}

pub(super) fn resident_probe(uv: [f32; 2], block: u32) -> Probe {
    Probe {
        uv,
        block: block + 1,
        spec_channel: 0.0,
    }
}

// AC 5, renderer half: every resident block reads its own texels at pool
// offset + block-local texel on both layers, and a block-local coordinate past
// any edge clamps to the block's own edge texel instead of reaching the
// neighbour placed flush against it.
#[test]
fn resident_blocks_read_their_own_texels_on_both_layers_and_clamp_at_their_edges() {
    let Some(ctx) = gpu_or_skip(
        "resident_blocks_read_their_own_texels_on_both_layers_and_clamp_at_their_edges",
    ) else {
        return;
    };
    let bound = fixture_pool(&ctx);
    let mut probes = Vec::new();
    let mut expected = Vec::new();
    for (block, &(width, height)) in EXTENTS[..PLACEMENTS.len()].iter().enumerate() {
        let center = |t: u32, size: u32| (t as f32 + 0.5) / size as f32;
        for y in 0..height {
            for x in 0..width {
                probes.push(resident_probe(
                    [center(x, width), center(y, height)],
                    block as u32,
                ));
                expected.push((block, x, y, "texel center"));
            }
        }
        // Past every edge, and just inside the far edges where an unclamped
        // bilinear tap would blend the neighbour in.
        let (w, h) = (width as f32, height as f32);
        for (uv, x, y) in [
            ([-0.4, center(2, height)], 0, 2),
            ([1.3, center(2, height)], width - 1, 2),
            ([1.0, center(3, height)], width - 1, 3),
            ([(w - 0.1) / w, center(1, height)], width - 1, 1),
            ([center(2, width), -0.25], 2, 0),
            ([center(2, width), 1.6], 2, height - 1),
            ([center(1, width), (h - 0.05) / h], 1, height - 1),
            ([-1.0, 2.0], 0, height - 1),
        ] {
            probes.push(resident_probe(uv, block as u32));
            expected.push((block, x, y, "clamped edge"));
        }
    }
    let results = run_probes(&ctx, &bound, &probes);
    for ((probe, result), &(block, x, y, what)) in probes.iter().zip(&results).zip(&expected) {
        let what = format!("block {block} local ({x}, {y}) [{what}] uv {:?}", probe.uv);
        let (irradiance, direction, mask) = expected_texel(block, x, y);
        assert_near(
            &result.irradiance[..3],
            &irradiance,
            1.0e-3,
            &format!("{what}: irradiance"),
        );
        assert_near(
            &result.direction[..2],
            &direction,
            1.0e-5,
            &format!("{what}: direction"),
        );
        assert_near(&result.mask, &mask, 1.0e-5, &format!("{what}: shadowmask"));
        assert!(!result.missing, "{what}: a resident block is not a miss");
        assert_near(
            &[result.spec_visibility, result.union_slot0_visibility],
            &[mask[0], mask[0]],
            1.0e-5,
            &format!("{what}: slot-0 selects"),
        );
    }
}

// Restated AC 7, shader half: a block whose entry is not resident reads zero
// irradiance and zero shadowmask, so its shadowmask-gated specular (dropped
// channel included) and union visibility drop; the no-lightmap entry reads
// zero irradiance, the neutral direction and an all-visible shadowmask.
#[test]
fn missing_and_no_lightmap_entries_drop_or_neutralize_their_terms() {
    let Some(ctx) = gpu_or_skip("missing_and_no_lightmap_entries_drop_or_neutralize_their_terms")
    else {
        return;
    };
    let bound = fixture_pool(&ctx);
    let dropped = f32::from(SHADOWMASK_CHANNEL_DROPPED);
    let probe = |block: u32, spec_channel: f32| Probe {
        uv: [0.4, 0.6],
        block,
        spec_channel,
    };
    let probes = [
        probe(MISSING_BLOCK + 1, 0.0),
        probe(MISSING_BLOCK + 1, dropped),
        probe(0, 0.0),
        probe(0, dropped),
        // Past the table: the vertex stage reads entry 0.
        probe(EXTENTS.len() as u32 + 30, 0.0),
    ];
    let results = run_probes(&ctx, &bound, &probes);
    for (what, result) in [("missing", &results[0]), ("missing, dropped", &results[1])] {
        assert_eq!(result.irradiance[..3], [0.0; 3], "{what}: irradiance");
        assert_eq!(result.mask, [0.0; 4], "{what}: shadowmask");
        assert!(result.missing, "{what}");
        assert_eq!(result.union_slot0_visibility, 0.0, "{what}: union drops");
    }
    // A miss drops exactly the block's terms (restated AC 7): specular gated
    // by a mask channel drops, a dropped-channel light never read the block.
    assert_eq!(
        results[0].spec_visibility, 0.0,
        "missing: gated specular drops"
    );
    assert_eq!(
        results[1].spec_visibility, 1.0,
        "missing, dropped: unmasked specular stays lit"
    );
    for (what, result) in [
        ("no lightmap", &results[2]),
        ("no lightmap, dropped", &results[3]),
        ("past the table", &results[4]),
    ] {
        assert_eq!(result.irradiance[..3], [0.0; 3], "{what}: irradiance");
        assert_eq!(
            result.direction,
            [0.5, 1.0, 0.5, 0.0],
            "{what}: neutral direction"
        );
        assert_eq!(result.mask, [1.0; 4], "{what}: all-visible shadowmask");
        assert!(!result.missing, "{what}");
        assert_eq!(result.spec_visibility, 1.0, "{what}: specular stays lit");
    }
}

// Placeholder mode matches the pre-block look: every vertex reads the white
// irradiance, +Y direction and all-visible shadowmask placeholders.
#[test]
fn placeholder_mode_reads_the_neutral_placeholders_for_every_block_id() {
    let Some(ctx) =
        gpu_or_skip("placeholder_mode_reads_the_neutral_placeholders_for_every_block_id")
    else {
        return;
    };
    let bound = BoundLightmap {
        irradiance: upload_placeholder_irradiance(&ctx.device, &ctx.queue),
        direction: upload_placeholder_direction(&ctx.device, &ctx.queue),
        shadowmask: upload_placeholder_shadowmask(&ctx.device, &ctx.queue),
        table: placeholder_block_table(),
    };
    let probes: Vec<Probe> = [0u32, 1, 9]
        .into_iter()
        .flat_map(|block| {
            [[0.0, 0.0], [0.7, 0.2], [1.0, 1.0]].map(|uv| Probe {
                uv,
                block,
                spec_channel: 2.0,
            })
        })
        .collect();
    let results = run_probes(&ctx, &bound, &probes);
    for (probe, result) in probes.iter().zip(&results) {
        let what = format!("placeholder block {} uv {:?}", probe.block, probe.uv);
        assert_eq!(result.irradiance[..3], [1.0; 3], "{what}: white irradiance");
        assert_near(
            &result.direction[..2],
            &[128.0 / 255.0, 1.0],
            1.0e-5,
            &format!("{what}: +Y direction"),
        );
        assert_eq!(result.mask, [1.0; 4], "{what}: all-visible shadowmask");
        assert!(!result.missing, "{what}");
        assert_eq!(result.spec_visibility, 1.0, "{what}");
    }
}
