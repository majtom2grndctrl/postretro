// GPU parity of the compact animated lightmap atlas: one animated scene
// composed by the real compose pass and sampled through forward.wgsl's block
// lookup, once through the identity block table and once through a packed
// one. Zero gutters must make the two indistinguishable.
// See: context/lib/rendering_pipeline.md §7.1 (Animated lightmap compose)
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests": the
// compose pass and the forward lookup are WGSL, so verifying the layout means
// running them. The scene is a set of probes, one per fragment of a one-row-
// per-512-probes target, each sampling a face at a static lightmap UV the way
// the forward fragment stage does. The harness self-skips without an adapter
// and says so; a skipped run proves nothing. Set `POSTRETRO_REQUIRE_GPU` to any
// non-empty value other than `0` to make a missing adapter fail the test.

use postretro_level_format::animated_light_chunks::{
    AnimatedLightChunk, AnimatedLightChunksSection,
};
use postretro_level_format::animated_light_weight_maps::{
    AnimatedBlock, AnimatedLightWeightMapsSection, ChunkAtlasRect, TexelLight, TexelLightEntry,
};
use postretro_render_cpu::sh_volume::ANIMATION_DESCRIPTOR_SIZE;
use postretro_visibility::VisibleCells;

use super::animated_lightmap::{AnimatedLightmapResources, AnimatedLmDebugConfig};
use super::pipeline_layout::uniform_bind_group_layout_entries;
use super::sh_volume::AnimatedLightBuffers;
use crate::lighting::lightmap::{animated_block_table_bytes, filtering_sampler_descriptor};

const FORWARD_WGSL: &str = include_str!("../shaders/forward.wgsl");
const STATIC_SIZE: u32 = 64;
const STATIC_LAYERS: u32 = 2;
/// Probes per target row; three pixels each (irradiance, direction, remap).
const PROBES_PER_ROW: u32 = 512;
const PIXELS_PER_PROBE: u32 = 3;
const PROBE_BYTES: usize = 16;
/// One 8-bit step, the parity bound.
const EIGHT_BIT_STEP: f32 = 1.0 / 255.0;

struct GpuCtx {
    device: wgpu::Device,
    queue: wgpu::Queue,
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
    eprintln!(
        "[animated_atlas_parity_test] running on {:?}",
        adapter.get_info().name
    );
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("animated_atlas_parity_test Device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .ok()?;
    Some(GpuCtx { device, queue })
}

fn gpu_or_skip(test: &str) -> Option<GpuCtx> {
    if let Some(ctx) = try_init_gpu() {
        return Some(ctx);
    }
    let required =
        std::env::var("POSTRETRO_REQUIRE_GPU").is_ok_and(|value| !value.is_empty() && value != "0");
    assert!(
        !required,
        "[animated_atlas_parity_test] {test}: POSTRETRO_REQUIRE_GPU is set but no GPU adapter \
         is available"
    );
    eprintln!("[animated_atlas_parity_test] skipping {test}: no GPU adapter available");
    None
}

/// The text of `fn name(` or `struct name {` through its matching brace.
fn wgsl_item<'a>(source: &'a str, header: &str) -> &'a str {
    let start = source
        .find(header)
        .unwrap_or_else(|| panic!("forward.wgsl must declare `{header}`"));
    let open = start + source[start..].find('{').expect("item body");
    let mut depth = 0usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("`{header}` body never closes");
}

/// forward.wgsl's block table, block lookup and animated sample, verbatim,
/// around a probe entry point that samples the way the fragment stage does.
fn shader_source() -> String {
    let prelude = r#"
struct Probe {
    uv: vec2<f32>,
    block: u32,
    _pad: u32,
};
@group(0) @binding(0) var animated_lm_atlas: texture_2d_array<f32>;
@group(0) @binding(1) var lightmap_filtering_sampler: sampler;
@group(0) @binding(2) var animated_lm_direction: texture_2d_array<f32>;
@group(0) @binding(3) var lightmap_sampler: sampler;
@group(0) @binding(4) var<uniform> animated_block_table: AnimatedBlockTable;
@group(0) @binding(5) var<storage, read> probes: array<Probe>;
"#;
    let entry = r#"
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let column = u32(position.x);
    let probe_index = u32(position.y) * 512u + column / 3u;
    let probe = probes[probe_index];
    let animated = animated_block_uv(probe.uv, probe.block);
    var irradiance = vec4<f32>(0.0);
    var direction = vec4<f32>(0.5, 1.0, 0.5, 0.0);
    if animated.found {
        irradiance = vec4<f32>(sample_lightmap_animated(animated.uv, animated.page), 1.0);
        direction = textureSample(
            animated_lm_direction,
            lightmap_sampler,
            animated.uv,
            i32(animated.page),
        );
    }
    let remap = vec4<f32>(animated.uv, f32(animated.page), select(0.0, 1.0, animated.found));
    var out = remap;
    if column % 3u == 0u {
        out = irradiance;
    } else if column % 3u == 1u {
        out = direction;
    }
    return out;
}
"#;
    let items = [
        wgsl_item(FORWARD_WGSL, "struct AnimatedBlockTable {"),
        wgsl_item(FORWARD_WGSL, "struct AnimatedBlockUv {"),
        wgsl_item(FORWARD_WGSL, "fn sample_lightmap_animated("),
        wgsl_item(FORWARD_WGSL, "fn animated_block_uv("),
    ];
    // The block table struct must precede the binding that names it.
    format!(
        "{}\n{prelude}\n{}\n{entry}",
        items[0],
        items[1..].join("\n\n")
    )
}

/// The test scene in static space: three animated faces on two static layers
/// of a 64² lightmap, as `(static layer, placement rect)`. Face 0 has one lit
/// chunk and a culled neighbour (absent); face 1 has two chunks; face 2 one.
const FACES: [(u32, [u32; 4]); 3] = [
    (0, [4, 4, 24, 16]),
    (1, [30, 8, 24, 24]),
    (1, [0, 40, 16, 16]),
];
/// Surviving chunks as `(face, static rect)`.
const CHUNKS: [(u32, [u32; 4]); 4] = [
    (0, [6, 6, 10, 12]),
    (1, [32, 10, 10, 20]),
    (1, [42, 10, 10, 20]),
    (2, [2, 42, 12, 12]),
];
/// Face 0's culled chunk: inside its block, owned by no surviving chunk.
const CULLED: [u32; 4] = [16, 6, 10, 12];

/// Deterministic per-texel weight in (0.1, 0.9).
fn weight(x: u32, y: u32, light: u32) -> f32 {
    let h =
        x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663) ^ light.wrapping_mul(83_492_791);
    0.1 + (h % 1000) as f32 / 1250.0
}

/// Section 25 with every block at `placements[face] = (page, x, y)`.
fn section(placements: &[(u32, u32, u32); 3], page_size: u32) -> AnimatedLightWeightMapsSection {
    let blocks: Vec<AnimatedBlock> = FACES
        .iter()
        .zip(placements)
        .map(|(&(layer, [x, y, w, h]), &(page, cx, cy))| AnimatedBlock {
            static_layer: layer,
            static_x: x,
            static_y: y,
            compact_x: cx,
            compact_y: cy,
            compact_layer: page,
            width: w,
            height: h,
        })
        .collect();
    let mut chunk_rects = Vec::new();
    let mut offset_counts = Vec::new();
    let mut texel_lights = Vec::new();
    for &(face, [x, y, w, h]) in &CHUNKS {
        let block = blocks[face as usize];
        chunk_rects.push(ChunkAtlasRect {
            compact_x: x - block.static_x + block.compact_x,
            compact_y: y - block.static_y + block.compact_y,
            width: w,
            height: h,
            texel_offset: offset_counts.len() as u32,
            block: face,
        });
        // Row-major, as the compose shader indexes them.
        for ty in y..y + h {
            for tx in x..x + w {
                let offset = texel_lights.len() as u32;
                let lights = if (tx + ty) % 3 == 0 { 2 } else { 1 };
                for light in 0..lights {
                    texel_lights.push(TexelLight {
                        light_index: light,
                        weight: weight(tx, ty + 64 * block.static_layer, light),
                        direction_oct: [
                            (20_000 + 97 * tx) as u16,
                            (30_000 + 131 * ty + 1_000 * light) as u16,
                        ],
                    });
                }
                offset_counts.push(TexelLightEntry {
                    offset,
                    count: lights,
                });
            }
        }
    }
    AnimatedLightWeightMapsSection {
        page_size,
        compact_layers: placements
            .iter()
            .map(|&(page, _, _)| page + 1)
            .max()
            .unwrap(),
        blocks,
        chunk_rects,
        offset_counts,
        texel_lights,
    }
}

/// Identity: every block at its static position, one page per static layer.
fn identity_section() -> AnimatedLightWeightMapsSection {
    let placements = [0, 1, 2].map(|face: usize| {
        let (layer, [x, y, _, _]) = FACES[face];
        (layer, x, y)
    });
    section(&placements, STATIC_SIZE)
}

/// Packed: faces 1 and 0 share page 0 at new positions; face 2 spills to
/// page 1 at an offset, so both pages and a non-origin translation are live.
fn packed_section() -> AnimatedLightWeightMapsSection {
    section(&[(0, 24, 0), (0, 0, 0), (1, 40, 40)], STATIC_SIZE)
}

fn chunk_section() -> AnimatedLightChunksSection {
    AnimatedLightChunksSection {
        chunks: CHUNKS
            .iter()
            .map(|&(face, _)| AnimatedLightChunk {
                aabb_min: [0.0; 3],
                face_index: face,
                aabb_max: [1.0; 3],
                index_offset: 0,
                uv_min: [0.0; 2],
                uv_max: [1.0; 2],
                index_count: 0,
                _padding: 0,
            })
            .collect(),
        light_indices: Vec::new(),
    }
}

/// Two animated descriptors with constant brightness (no curve samples),
/// forced active or inactive.
fn descriptors(active: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    for color in [[1.0_f32, 0.6, 0.2], [0.1, 0.4, 1.0]] {
        let mut record = [0u8; ANIMATION_DESCRIPTOR_SIZE];
        let mut put =
            |at: usize, word: u32| record[at..at + 4].copy_from_slice(&word.to_ne_bytes());
        put(0, 1.0_f32.to_bits()); // period
        put(4, 0.0_f32.to_bits()); // phase
        put(8, 0); // brightness_offset
        put(12, 0); // brightness_count: constant 1.0
        put(16, color[0].to_bits());
        put(20, color[1].to_bits());
        put(24, color[2].to_bits());
        put(28, 0); // color_offset
        put(32, 0); // color_count
        put(36, u32::from(active));
        bytes.extend_from_slice(&record);
    }
    bytes
}

#[derive(Clone, Copy)]
struct Probe {
    uv: [f32; 2],
    block: u32,
}

/// Probes across every face's chart interior (the placement minus its
/// 2-texel gutter), reaching one texel past it on each side — the forward
/// footprint the compiler guard allows — at an irregular step so bilinear
/// weights vary. Also returns the probes inside face 0's culled chunk.
fn probes() -> (Vec<Probe>, Vec<usize>) {
    let mut probes = Vec::new();
    let mut culled = Vec::new();
    for (face, &(_, [x, y, w, h])) in FACES.iter().enumerate() {
        let (lo_x, hi_x) = (x as f32 + 1.0, (x + w) as f32 - 1.0);
        let (lo_y, hi_y) = (y as f32 + 1.0, (y + h) as f32 - 1.0);
        let mut ty = lo_y;
        while ty <= hi_y {
            let mut tx = lo_x;
            while tx <= hi_x {
                // Away from any surviving chunk's footprint.
                let in_culled = face == 0
                    && tx >= (CULLED[0] + 1) as f32
                    && tx <= (CULLED[0] + CULLED[2]) as f32
                    && ty >= CULLED[1] as f32
                    && ty <= (CULLED[1] + CULLED[3]) as f32;
                if in_culled {
                    culled.push(probes.len());
                }
                probes.push(Probe {
                    uv: [tx / STATIC_SIZE as f32, ty / STATIC_SIZE as f32],
                    block: face as u32 + 1,
                });
                tx += 0.37;
            }
            ty += 0.41;
        }
    }
    (probes, culled)
}

/// Per probe: animated irradiance, direction sample, and `(uv, page, found)`.
struct ProbeResult {
    irradiance: [f32; 4],
    direction: [f32; 4],
    remap: [f32; 4],
}

fn run(
    ctx: &GpuCtx,
    section: &AnimatedLightWeightMapsSection,
    active: bool,
    probes: &[Probe],
) -> Vec<ProbeResult> {
    use wgpu::util::DeviceExt;
    let device = &ctx.device;

    let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("parity uniforms layout"),
        entries: &uniform_bind_group_layout_entries(),
    });
    let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("parity uniforms"),
        contents: &[0u8; postretro_render_cpu::frame_uniforms::UNIFORM_SIZE],
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("parity uniforms"),
        layout: &uniform_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buffer.as_entire_binding(),
        }],
    });
    let animation = AnimatedLightBuffers::for_test(device, descriptors(active));
    let chunks = chunk_section();
    let mut resources = AnimatedLightmapResources::new(
        device,
        Some(section),
        Some(&chunks),
        &[],
        &animation,
        &uniform_layout,
        Some((STATIC_SIZE, STATIC_LAYERS)),
        AnimatedLmDebugConfig::disabled(),
    )
    .expect("parity scene builds a real compact atlas");
    assert!(resources.is_active(), "the scene must compose");

    let table = animated_block_table_bytes(Some(section), STATIC_SIZE);
    let table_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("parity block table"),
        contents: &table,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let probe_bytes: Vec<u8> = probes
        .iter()
        .flat_map(|probe| {
            let mut bytes = [0u8; PROBE_BYTES];
            bytes[0..4].copy_from_slice(&probe.uv[0].to_ne_bytes());
            bytes[4..8].copy_from_slice(&probe.uv[1].to_ne_bytes());
            bytes[8..12].copy_from_slice(&probe.block.to_ne_bytes());
            bytes
        })
        .collect();
    let probe_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("parity probes"),
        contents: &probe_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let filtering = device.create_sampler(&filtering_sampler_descriptor());
    let nearest = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("parity nearest"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
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
    let sampler_entry = |binding: u32, ty: wgpu::SamplerBindingType| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(ty),
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
        label: Some("parity probe layout"),
        entries: &[
            texture_entry(0, true),
            sampler_entry(1, wgpu::SamplerBindingType::Filtering),
            texture_entry(2, false),
            sampler_entry(3, wgpu::SamplerBindingType::NonFiltering),
            buffer_entry(4, wgpu::BufferBindingType::Uniform),
            buffer_entry(5, wgpu::BufferBindingType::Storage { read_only: true }),
        ],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("parity probe bind group"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&resources.forward_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&filtering),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&resources.direction_forward_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&nearest),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: table_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: probe_buffer.as_entire_binding(),
            },
        ],
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("parity probe shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source().into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("parity probe pipeline layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let format = wgpu::TextureFormat::Rgba32Float;
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("parity probe pipeline"),
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
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    let width = PROBES_PER_ROW * PIXELS_PER_PROBE;
    let height = (probes.len() as u32).div_ceil(PROBES_PER_ROW);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("parity target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    let row_bytes = width * 16;
    assert_eq!(row_bytes % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT, 0);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("parity readback"),
        size: u64::from(row_bytes * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&Default::default());
    resources.dispatch(
        &ctx.queue,
        &mut encoder,
        &uniform_bind_group,
        &VisibleCells::DrawAll,
        None,
    );
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("parity probes"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device poll");
    receiver
        .recv()
        .expect("map callback")
        .expect("readback maps");
    let data = slice.get_mapped_range();
    let pixel = |x: u32, y: u32| -> [f32; 4] {
        let at = (y * row_bytes + x * 16) as usize;
        std::array::from_fn(|c| {
            f32::from_ne_bytes(data[at + c * 4..at + c * 4 + 4].try_into().unwrap())
        })
    };
    (0..probes.len() as u32)
        .map(|index| {
            let (row, column) = (index / PROBES_PER_ROW, (index % PROBES_PER_ROW) * 3);
            ProbeResult {
                irradiance: pixel(column, row),
                direction: pixel(column + 1, row),
                remap: pixel(column + 2, row),
            }
        })
        .collect()
}

fn max_difference(a: [f32; 4], b: [f32; 4]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

#[test]
fn identity_and_packed_block_tables_render_the_same_forced_animated_frame() {
    let Some(ctx) =
        gpu_or_skip("identity_and_packed_block_tables_render_the_same_forced_animated_frame")
    else {
        return;
    };
    let identity = identity_section();
    let packed = packed_section();
    assert_eq!(identity.consistency_error(), None);
    assert_eq!(packed.consistency_error(), None);
    assert_eq!(identity.compact_layers, 2);
    assert_eq!(
        packed.compact_layers, 2,
        "the packed layout spills onto page 1"
    );
    assert_eq!(identity.offset_counts, packed.offset_counts);
    assert_eq!(identity.texel_lights, packed.texel_lights);

    let (probes, culled) = probes();
    assert!(probes.len() > 2_000 && !culled.is_empty());
    let through_identity = run(&ctx, &identity, true, &probes);
    let through_packed = run(&ctx, &packed, true, &probes);
    let lights_off = run(&ctx, &identity, false, &probes);

    // The forced lights must change the frame: a render where no animated
    // light contributes proves nothing about parity.
    let lit = through_identity
        .iter()
        .filter(|result| result.irradiance[..3].iter().any(|&c| c > 0.05))
        .count();
    assert!(
        lit > probes.len() / 3,
        "only {lit} of {} probes are lit",
        probes.len()
    );
    assert!(
        lights_off
            .iter()
            .all(|result| result.irradiance[..3].iter().all(|&c| c == 0.0)),
        "with the animated lights off the frame carries no animated light",
    );

    for (index, (a, b)) in through_identity.iter().zip(&through_packed).enumerate() {
        let probe = probes[index];
        let irradiance = max_difference(a.irradiance, b.irradiance);
        let direction = max_difference(a.direction, b.direction);
        assert!(
            irradiance <= EIGHT_BIT_STEP && direction <= EIGHT_BIT_STEP,
            "probe {index} (block {}, uv {:?}): identity {:?}/{:?} vs packed {:?}/{:?}",
            probe.block,
            probe.uv,
            a.irradiance,
            a.direction,
            b.irradiance,
            b.direction,
        );
    }

    // Culled chunk texels are never composed; they read zero in both layouts.
    for &index in &culled {
        for result in [&through_identity[index], &through_packed[index]] {
            assert_eq!(result.irradiance, [0.0, 0.0, 0.0, 1.0], "probe {index}");
        }
    }
}

#[test]
fn identity_block_table_samples_each_face_at_its_static_uv_and_layer_page() {
    let Some(ctx) =
        gpu_or_skip("identity_block_table_samples_each_face_at_its_static_uv_and_layer_page")
    else {
        return;
    };
    let identity = identity_section();
    let (probes, _) = probes();
    let results = run(&ctx, &identity, true, &probes);
    for (probe, result) in probes.iter().zip(&results) {
        let (layer, _) = FACES[probe.block as usize - 1];
        assert_eq!(
            result.remap,
            [probe.uv[0], probe.uv[1], layer as f32, 1.0],
            "identity remap must be the static UV on the static layer's page",
        );
    }
    // Every chunk composes at its static layer's page and static position.
    for index in 0..identity.chunk_rects.len() {
        let (layer, x, y) = identity.chunk_static_origin(index).unwrap();
        let rect = identity.chunk_rects[index];
        let block = identity.blocks[rect.block as usize];
        assert_eq!((rect.compact_x, rect.compact_y), (x, y));
        assert_eq!(
            block.compact_layer, layer,
            "layer order is the page order here"
        );
    }
}
