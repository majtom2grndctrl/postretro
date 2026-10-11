// GPU readback of the forward shader's shadowmask sampling helper and both
// consumers' owner-channel selects, against hand-built BC5 cell blocks placed
// in the shadowmask pool and against the 2×1 placeholder, uploaded through the
// renderer's own pool upload and resolved through the real block table.
// The WGSL helpers it calls are extracted verbatim from forward.wgsl and
// lightmap_sample.wgsl; its own entry point repeats the union path's
// find-skip-select order, which the shader_tests grep gate pins in
// `shadowmask_union_subtraction` itself.
// See: context/lib/rendering_pipeline.md §4 (Static specular ownership)
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests": the
// helper is WGSL, so verifying it means running it. `textureSample` needs a
// fragment stage, so this is a small offscreen render: each pixel of a one-row
// target evaluates one probe. The harness self-skips without a BC-capable
// adapter and says so; a skipped run proves nothing. Set
// `POSTRETRO_REQUIRE_GPU` to any non-empty value other than `0` to make a
// missing adapter fail the test instead of skipping it.

use postretro_level_format::shadowmask_atlas::{SHADOWMASK_GROUP_COUNT, SHADOWMASK_OWNER_CHANNELS};
use postretro_render_cpu::frame_uniforms::SPEC_SHADOWMASK_FLAG_MASK_PRESENT;
use postretro_render_cpu::lightmap_pool::{
    AllResidentPool, BlockPlacement, block_table_bytes, placeholder_block_table,
};

use super::gpu_test_harness::{GpuCtx, bc_gpu_or_skip, wgsl_const_line, wgsl_function, wgsl_item};

use crate::lighting::lightmap::test_fixtures::{FixtureTexels, block_fixture};
use crate::lighting::lightmap::{
    StaticPoolPlan, filtering_sampler_descriptor, upload_placeholder_shadowmask, upload_static_pool,
};

/// Two 8×8 cell blocks, one per pool layer, at non-zero pool offsets.
const BLOCK_EDGE: u32 = 8;
const BLOCK_COUNT: u32 = 2;
const PLACEMENTS: [BlockPlacement; 2] = [
    BlockPlacement {
        layer: 0,
        x: 16,
        y: 8,
    },
    BlockPlacement {
        layer: 1,
        x: 40,
        y: 20,
    },
];
const PROBE_BYTES: usize = 16;
const OUTPUT_TEXEL_BYTES: u32 = 16;
/// Render targets: the sampled mask, then the two paths' selects (even pixel)
/// or the union attenuation at entity visibility 1 and 0 (odd pixel). Two
/// `Rgba32Float` targets fit the default 32-byte per-sample colour limit, so
/// each probe renders two pixels to carry every value.
const TARGET_COUNT: usize = 2;
const PIXELS_PER_PROBE: u32 = 2;
const FORWARD_WGSL: &str = include_str!("../shaders/forward.wgsl");
const LIGHTMAP_SAMPLE_WGSL: &str = include_str!("../shaders/lightmap_sample.wgsl");

/// Owner position the harness uses for a light that does not own the face:
/// the union's not-found value, `SPEC_OWNER_CHANNELS` in forward.wgsl.
const NOT_AN_OWNER: u32 = SHADOWMASK_OWNER_CHANNELS as u32;

fn shader_source() -> String {
    let consts = [
        wgsl_const_line(FORWARD_WGSL, "SPEC_OWNER_EMPTY"),
        wgsl_const_line(FORWARD_WGSL, "SPEC_OWNER_CHANNELS"),
        wgsl_const_line(FORWARD_WGSL, "SPEC_SHADOWMASK_FLAG_MASK_PRESENT"),
        wgsl_const_line(LIGHTMAP_SAMPLE_WGSL, "LIGHTMAP_POOL_LAYER_EDGE"),
        wgsl_const_line(LIGHTMAP_SAMPLE_WGSL, "LIGHTMAP_BLOCK_RESIDENT"),
        wgsl_const_line(LIGHTMAP_SAMPLE_WGSL, "LIGHTMAP_BLOCK_NONE"),
        wgsl_item(LIGHTMAP_SAMPLE_WGSL, "struct LightmapBlockVaryings {"),
    ]
    .join("\n");
    let prelude = r#"
struct HarnessUniforms {
    spec_shadowmask_flags: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};
struct Probe {
    uv: vec2<f32>,
    block: u32,
    position: u32,
};
@group(0) @binding(0) var shadowmask_atlas: texture_2d_array<f32>;
@group(0) @binding(1) var lightmap_filtering_sampler: sampler;
@group(0) @binding(2) var<uniform> uniforms: HarnessUniforms;
@group(0) @binding(3) var<storage, read> probes: array<Probe>;
@group(0) @binding(4) var<storage, read> lightmap_block_table: array<vec4<u32>>;
"#;
    let entry = r#"
struct HarnessOut {
    @location(0) mask: vec4<f32>,
    // Even pixel: selects. Odd pixel: attenuations.
    @location(1) values: vec4<f32>,
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
    let mask = sample_shadowmask_atlas(block.texel, block.layer_flags, block.rect);
    // A face owned by spec lights 10..13 at positions 0..3. The probe's light
    // is the owner at `position`, or spec light 99, which owns nothing.
    let owners = vec4<u32>(10u, 11u, 12u, 13u);
    let spec_idx = select(99u, 10u + probe.position, probe.position < SPEC_OWNER_CHANNELS);
    // World static specular: owner position s reads channel s, gated as
    // fs_main gates it.
    let readable = static_spec_owners_readable(block.layer_flags);
    var spec_visibility = 0.0;
    if readable && probe.position < SPEC_OWNER_CHANNELS {
        spec_visibility = shadowmask_channel_value(mask, probe.position);
    }
    // `shadowmask_union_subtraction`'s own sequence: find the light among the
    // face's owners, skip a non-owner, then select from the hoisted mask.
    let union_channel = shadowmask_owner_channel(owners, spec_idx);
    var selects = vec4<f32>(spec_visibility, f32(union_channel), 0.0, 0.0);
    var attenuations = vec4<f32>(0.0);
    if union_channel != SPEC_OWNER_CHANNELS {
        let baked_vis = shadowmask_channel_value(mask, union_channel);
        selects.z = baked_vis;
        attenuations = vec4<f32>(
            shadowmask_attenuation(baked_vis, 1.0),
            shadowmask_attenuation(baked_vis, 0.0),
            0.0,
            0.0,
        );
    }
    var out: HarnessOut;
    out.mask = mask;
    out.values = select(selects, attenuations, (pixel & 1u) == 1u);
    return out;
}
"#;
    let helpers: Vec<&str> = [
        ("shadowmask_channel_value", FORWARD_WGSL),
        ("resolve_lightmap_block", LIGHTMAP_SAMPLE_WGSL),
        ("lightmap_block_layer", LIGHTMAP_SAMPLE_WGSL),
        ("lightmap_block_resident", LIGHTMAP_SAMPLE_WGSL),
        ("lightmap_pool_uv", LIGHTMAP_SAMPLE_WGSL),
        ("sample_shadowmask_atlas", LIGHTMAP_SAMPLE_WGSL),
        ("static_spec_mask_present", FORWARD_WGSL),
        ("static_spec_owners_readable", FORWARD_WGSL),
        ("shadowmask_owner_channel", FORWARD_WGSL),
        ("shadowmask_attenuation", FORWARD_WGSL),
    ]
    .into_iter()
    .map(|(name, source)| wgsl_function(source, name))
    .collect();
    format!("{prelude}\n{consts}\n{}\n{entry}", helpers.join("\n\n"))
}

#[derive(Clone, Copy)]
struct Probe {
    /// Block-local unorm UV, as the vertex attribute carries it.
    uv: [f32; 2],
    /// Cell block id + 1, as `WorldVertex::lightmap_block` names it.
    block: u32,
    /// The light's owner position `0..3` on the face, or [`NOT_AN_OWNER`].
    position: u32,
}

impl Probe {
    fn new(uv: [f32; 2], block: u32, position: u32) -> Self {
        Self {
            uv,
            block,
            position,
        }
    }

    fn bytes(self) -> [u8; PROBE_BYTES] {
        let words = [
            self.uv[0].to_bits(),
            self.uv[1].to_bits(),
            self.block,
            self.position,
        ];
        let mut out = [0u8; PROBE_BYTES];
        for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            chunk.copy_from_slice(&word.to_ne_bytes());
        }
        out
    }
}

struct ProbeResult {
    mask: [f32; 4],
    /// The visibility world static specular gives the probe's owner.
    spec_visibility: f32,
    /// `shadowmask_owner_channel`'s result; the union skips the light when it
    /// is [`NOT_AN_OWNER`].
    union_channel: u32,
    /// The union's selected mask value; meaningful only for a real channel.
    union_visibility: f32,
    union_attenuation_at_entity_visibility_one: f32,
    union_attenuation_at_entity_visibility_zero: f32,
}

/// `mask_present` is the frame uniforms' bit: set for a real id-42 pool,
/// clear for the placeholder.
fn run_probes(
    ctx: &GpuCtx,
    texture: &wgpu::Texture,
    block_table: &[u8],
    probes: &[Probe],
    mask_present: bool,
) -> Vec<ProbeResult> {
    use wgpu::util::DeviceExt;

    let device = &ctx.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shadowmask_sample_test shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source().into()),
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let sampler = device.create_sampler(&filtering_sampler_descriptor());
    let mut uniform_bytes = [0u8; 16];
    if mask_present {
        uniform_bytes[..4].copy_from_slice(&SPEC_SHADOWMASK_FLAG_MASK_PRESENT.to_ne_bytes());
    }
    let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("harness uniforms"),
        contents: &uniform_bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let probe_bytes: Vec<u8> = probes.iter().flat_map(|probe| probe.bytes()).collect();
    let probe_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("probes"),
        contents: &probe_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let table_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("block table"),
        contents: block_table,
        usage: wgpu::BufferUsages::STORAGE,
    });

    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("shadowmask_sample_test layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
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
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("shadowmask_sample_test bind group"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: probe_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: table_buffer.as_entire_binding(),
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("shadowmask_sample_test pipeline layout"),
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
        label: Some("shadowmask_sample_test pipeline"),
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
                label: Some("shadowmask_sample_test target"),
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
                label: Some("shadowmask_sample_test readback"),
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
            label: Some("shadowmask_sample_test pass"),
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
            result.expect("map shadowmask_sample_test readback");
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll shadowmask_sample_test device");
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
    let masks = read(&readbacks[0]);
    let values = read(&readbacks[1]);
    masks
        .as_chunks::<{ PIXELS_PER_PROBE as usize }>()
        .0
        .iter()
        .zip(values.as_chunks::<{ PIXELS_PER_PROBE as usize }>().0.iter())
        .map(|(mask, values)| {
            let (selects, attenuations) = (values[0], values[1]);
            ProbeResult {
                mask: mask[0],
                spec_visibility: selects[0],
                // Channels and the sentinel are small integers, exact in f32.
                union_channel: selects[1] as u32,
                union_visibility: selects[2],
                union_attenuation_at_entity_visibility_one: attenuations[0],
                union_attenuation_at_entity_visibility_zero: attenuations[1],
            }
        })
        .collect()
}

/// The raw byte channel `channel` holds in BC block (`bx`, `by`) of cell
/// `block`. Distinct per channel, BC block and cell block, and constant within
/// a BC block, so BC4 reproduces it exactly and any misrouted read shows as a
/// wrong value.
fn mask_byte(channel: u32, bx: u32, by: u32, block: u32) -> u8 {
    (10 + channel * 50 + bx * 20 + by * 7 + block * 3) as u8
}

/// The two cell blocks in the shadowmask pool, group 0 holding channels 0/1
/// and group 1 channels 2/3, and the block table naming their placements.
fn fixture_pool(ctx: &GpuCtx) -> (wgpu::Texture, Vec<u8>) {
    let extents = [(BLOCK_EDGE, BLOCK_EDGE); BLOCK_COUNT as usize];
    let mask =
        |block: usize, channel: u32, bx: u32, by: u32| mask_byte(channel, bx, by, block as u32);
    let fixture = block_fixture(
        &extents,
        2,
        &FixtureTexels {
            irradiance: &|_, _, _| [0.0; 4],
            direction: &|_, _, _| [128, 255],
            shadowmask: Some(&mask),
        },
    );
    let plan = StaticPoolPlan {
        header: fixture.index.header,
        extents: extents.to_vec(),
        pool: AllResidentPool {
            placements: PLACEMENTS.to_vec(),
            layer_count: 2,
        },
        with_shadowmask: true,
    };
    let textures = upload_static_pool(&ctx.device, &ctx.queue, &plan, &fixture.payloads);
    let table = block_table_bytes(&extents, &PLACEMENTS.map(Some));
    (
        textures.shadowmask.expect("the plan keeps the shadowmask"),
        table,
    )
}

fn expected_mask(texel_x: u32, texel_y: u32, block: u32) -> [f32; 4] {
    std::array::from_fn(|channel| {
        f32::from(mask_byte(channel as u32, texel_x / 4, texel_y / 4, block)) / 255.0
    })
}

fn texel_center(texel: u32, size: u32) -> f32 {
    (texel as f32 + 0.5) / size as f32
}

fn assert_near(actual: f32, expected: f32, what: &str) {
    assert!(
        (actual - expected).abs() <= 1.0e-5,
        "{what}: got {actual}, expected {expected}"
    );
}

/// Both consumers select owner position `position`'s own mask channel; a
/// light that owns nothing reads no channel and the union skips it. For an
/// owner, the union's attenuation is wired to that channel: all of it at
/// entity visibility 0, none at 1. Where owners are unreadable (the
/// no-lightmap entry), world specular gives every owner zero visibility.
fn assert_selects(result: &ProbeResult, position: u32, owners_readable: bool, what: &str) {
    if position == NOT_AN_OWNER {
        assert_eq!(
            result.spec_visibility, 0.0,
            "{what}: a non-owner adds no specular"
        );
        assert_eq!(
            result.union_channel, NOT_AN_OWNER,
            "{what}: the promoted union must skip a light that does not own the face"
        );
        return;
    }
    let baked_vis = result.mask[position as usize];
    let spec_visibility = if owners_readable { baked_vis } else { 0.0 };
    assert_near(
        result.spec_visibility,
        spec_visibility,
        &format!("{what}: world specular"),
    );
    assert_eq!(
        result.union_channel, position,
        "{what}: promoted union channel"
    );
    assert_near(
        result.union_visibility,
        baked_vis,
        &format!("{what}: promoted union"),
    );
    assert!(
        baked_vis > 0.0,
        "{what}: the fixture mask must be nonzero so attenuation can show it"
    );
    assert_eq!(
        result.union_attenuation_at_entity_visibility_one, 0.0,
        "{what}: static→static union attenuation must be exactly zero"
    );
    assert_near(
        result.union_attenuation_at_entity_visibility_zero,
        baked_vis,
        &format!("{what}: a fully occluded entity attenuates by the channel's own mask"),
    );
}

// Pins: second-group-light, seam-bleed, seam-outer-halftexel. M1 (union
// half), M3, M4 (union half). Seam-bleed rides on the driven u = 0 / u = 1
// probes against groups that differ at the seam; the block clamp also keeps
// them off the empty pool texels beside each block.
#[test]
fn every_owner_position_reads_its_own_channel_at_centers_block_edges_and_outer_uv() {
    let Some(ctx) = bc_gpu_or_skip(
        "every_owner_position_reads_its_own_channel_at_centers_block_edges_and_outer_uv",
    ) else {
        return;
    };
    let (texture, table) = fixture_pool(&ctx);

    let mut probes = Vec::new();
    let mut expectations = Vec::new();
    for block in 0..BLOCK_COUNT {
        for &texel_y in &[0, 5] {
            let v = texel_center(texel_y, BLOCK_EDGE);
            // Texel centers either side of the BC block edge inside each group.
            for &texel_x in &[0, 3, 4, BLOCK_EDGE - 1] {
                let u = texel_center(texel_x, BLOCK_EDGE);
                expectations.push((
                    expected_mask(texel_x, texel_y, block),
                    true,
                    format!("texel ({texel_x}, {texel_y}) block {block}"),
                ));
                probes.push(([u, v], block + 1));
            }
            // Driven outer UVs a baked chart gutter never reaches: each group
            // must clamp to its own block's edge column, never blend across
            // the seam or into the pool texels beside the block.
            for (u, texel_x) in [
                (0.0, 0),
                (0.2 / BLOCK_EDGE as f32, 0),
                (1.0, BLOCK_EDGE - 1),
                (1.0 - 0.2 / BLOCK_EDGE as f32, BLOCK_EDGE - 1),
                (-0.5, 0),
                (1.5, BLOCK_EDGE - 1),
            ] {
                expectations.push((
                    expected_mask(texel_x, texel_y, block),
                    true,
                    format!("u = {u}, row {texel_y}, block {block}"),
                ));
                probes.push(([u, v], block + 1));
            }
        }
    }
    // A block id past the table resolves to the no-lightmap entry: it reads
    // all visible, and that read is no owner's baked visibility, so world
    // specular drops every owner there.
    probes.push((
        [texel_center(1, BLOCK_EDGE), texel_center(1, BLOCK_EDGE)],
        BLOCK_COUNT + 7,
    ));
    expectations.push(([1.0; 4], false, "id past the table".to_string()));

    // The seam really differs: group 0's last column against group 1's first.
    let seam_left = expected_mask(BLOCK_EDGE - 1, 0, 0);
    let seam_right = expected_mask(0, 0, 0);
    assert_ne!(seam_left[0], seam_right[2]);
    assert_ne!(seam_left[1], seam_right[3]);

    let positions = [0u32, 1, 2, 3, NOT_AN_OWNER];
    let expanded: Vec<Probe> = probes
        .iter()
        .flat_map(|&(uv, block)| {
            positions
                .iter()
                .map(move |&position| Probe::new(uv, block, position))
        })
        .collect();
    let results = run_probes(&ctx, &texture, &table, &expanded, true);

    for (index, result) in results.iter().enumerate() {
        let (expected, owners_readable, what) = &expectations[index / positions.len()];
        let position = positions[index % positions.len()];
        for (component, (&actual, &want)) in result.mask.iter().zip(expected).enumerate() {
            assert_near(actual, want, &format!("{what}: channel {component} mask"));
        }
        assert_selects(
            result,
            position,
            *owners_readable,
            &format!("{what}, position {position}"),
        );
    }
}

// Pin: placeholder-second-group. A second-group channel against the 2×1 white
// placeholder reads a real, fully visible texel at any UV and for any block
// id, through placeholder mode's one-entry table. That texel is no mask: the
// mask-present bit is clear, so world specular gives no owner a visibility,
// and only the union's bit gate (pinned by shader_tests) keeps it from
// subtracting.
#[test]
fn placeholder_reads_fully_lit_for_every_channel_and_block() {
    let Some(ctx) = bc_gpu_or_skip("placeholder_reads_fully_lit_for_every_channel_and_block")
    else {
        return;
    };
    let texture = upload_placeholder_shadowmask(&ctx.device, &ctx.queue);
    // One real texel per group: a 1×1 placeholder would pass the reads below
    // with both groups sharing one texel.
    assert_eq!(texture.width(), SHADOWMASK_GROUP_COUNT);
    assert_eq!(texture.height(), 1);
    assert_eq!(texture.depth_or_array_layers(), 1);
    let table = placeholder_block_table();
    let probes: Vec<Probe> = [[0.0, 0.0], [0.5, 0.5], [1.0, 1.0], [0.97, 0.03]]
        .into_iter()
        .flat_map(|uv| {
            [0u32, 3].into_iter().flat_map(move |block| {
                [0u32, 1, 2, 3]
                    .into_iter()
                    .map(move |position| Probe::new(uv, block, position))
            })
        })
        .collect();
    let results = run_probes(&ctx, &texture, &table, &probes, false);
    for (probe, result) in probes.iter().zip(&results) {
        let what = format!(
            "placeholder uv {:?} block {} position {}",
            probe.uv, probe.block, probe.position
        );
        assert!(
            result.mask.iter().all(|&m| m == 1.0),
            "{what}: mask {:?}",
            result.mask
        );
        assert_eq!(
            result.spec_visibility, 0.0,
            "{what}: no mask present, so no owner reads the placeholder"
        );
        assert_eq!(result.union_channel, probe.position, "{what}");
    }
}
