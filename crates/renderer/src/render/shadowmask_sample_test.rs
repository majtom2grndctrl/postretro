// GPU readback of forward.wgsl's shadowmask sampling helper and both decode
// paths' slot selects, against a hand-built side-by-side BC5 atlas and the
// 2×1 placeholder, uploaded through the renderer's own upload functions.
// Every WGSL function it runs is extracted verbatim from forward.wgsl.
// See: context/lib/rendering_pipeline.md §4 (World specular shadowmask)
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests": the
// helper is WGSL, so verifying it means running it. `textureSample` needs a
// fragment stage, so this is a small offscreen render: each pixel of a one-row
// target evaluates one probe. The harness self-skips without a BC-capable
// adapter and says so; a skipped run proves nothing. Set
// `POSTRETRO_REQUIRE_GPU` to any non-empty value other than `0` to make a
// missing adapter fail the test instead of skipping it.

use postretro_level_format::shadowmask_atlas::{
    SHADOWMASK_CHANNEL_DROPPED, SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE, SHADOWMASK_GROUP_COUNT,
    ShadowmaskAtlasSection,
};
use postretro_lighting::spec_buffer::SPEC_LIGHT_SHADOWMASK_NONE;

use crate::lighting::lightmap::{
    filtering_sampler_descriptor, upload_placeholder_shadowmask, upload_shadowmask_texture,
};
use crate::render::shadowmask::metadata_channel_value;

const ATLAS_WIDTH: u32 = 8;
const ATLAS_HEIGHT: u32 = 8;
const ATLAS_LAYERS: u32 = 2;
const PROBE_BYTES: usize = 32;
const OUTPUT_TEXEL_BYTES: u32 = 16;
/// Render targets: the sampled mask, then the two paths' selects (even pixel)
/// or the union attenuation at entity visibility 1 and 0 (odd pixel). Two
/// `Rgba32Float` targets fit the default 32-byte per-sample colour limit, so
/// each probe renders two pixels to carry every value.
const TARGET_COUNT: usize = 2;
const PIXELS_PER_PROBE: u32 = 2;
const FORWARD_WGSL: &str = include_str!("../shaders/forward.wgsl");

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
    if !adapter
        .features()
        .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
    {
        return None;
    }
    eprintln!(
        "[shadowmask_sample_test] running on {:?}",
        adapter.get_info().name
    );
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("shadowmask_sample_test Device"),
        required_features: wgpu::Features::TEXTURE_COMPRESSION_BC,
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .ok()?;
    Some(GpuCtx { device, queue })
}

/// A BC-capable GPU, or `None` after saying the test skipped. Under
/// `POSTRETRO_REQUIRE_GPU` a missing adapter panics, so a run that must
/// prove something on an adapter cannot pass by skipping.
fn gpu_or_skip(test: &str) -> Option<GpuCtx> {
    if let Some(ctx) = try_init_gpu() {
        return Some(ctx);
    }
    let required =
        std::env::var("POSTRETRO_REQUIRE_GPU").is_ok_and(|value| !value.is_empty() && value != "0");
    assert!(
        !required,
        "[shadowmask_sample_test] {test}: POSTRETRO_REQUIRE_GPU is set but no BC-capable GPU \
         adapter is available"
    );
    eprintln!("[shadowmask_sample_test] skipping {test}: no BC-capable GPU adapter available");
    None
}

/// The text of `fn name(` through its matching closing brace in `source`.
fn wgsl_function<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("forward.wgsl must declare `{name}`"));
    let open = start + source[start..].find('{').expect("function body");
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
    panic!("`{name}` body never closes");
}

/// forward.wgsl's module-scope `const NAME…;` line.
fn wgsl_const_line(name: &str) -> &'static str {
    FORWARD_WGSL
        .lines()
        .find(|line| line.starts_with(&format!("const {name}:")))
        .unwrap_or_else(|| panic!("forward.wgsl must declare `{name}`"))
}

/// The union's skip sentinel, read from forward.wgsl so the harness asserts
/// against the shader's own value.
fn union_channel_none() -> u32 {
    let line = wgsl_const_line("SHADOWMASK_UNION_CHANNEL_NONE");
    line.split_once('=')
        .and_then(|(_, value)| value.trim().strip_suffix("u;"))
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("unparsed skip sentinel: {line}"))
}

fn shader_source() -> String {
    let consts = [
        wgsl_const_line("SHADOWMASK_CHANNEL_DROPPED"),
        wgsl_const_line("SHADOWMASK_UNION_CHANNEL_NONE"),
    ]
    .join("\n");
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
    layer: u32,
    union_channel: f32,
    spec_channel: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};
@group(0) @binding(0) var shadowmask_atlas: texture_2d_array<f32>;
@group(0) @binding(1) var lightmap_filtering_sampler: sampler;
@group(0) @binding(2) var<uniform> uniforms: HarnessUniforms;
@group(0) @binding(3) var<storage, read> probes: array<Probe>;
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
    let mask = sample_shadowmask_atlas(probe.uv, probe.layer);
    var spec: SpecLight;
    spec.cone_cos = vec4<f32>(0.0, 0.0, probe.spec_channel, 0.0);
    // `shadowmask_union_subtraction`'s own sequence: resolve the metadata
    // float through its guard, skip NONE, then select from the hoisted mask.
    let union_channel = shadowmask_union_channel(probe.union_channel);
    var selects = vec4<f32>(
        shadowmask_visibility_for_spec_light(spec, mask),
        f32(union_channel),
        0.0,
        0.0,
    );
    var attenuations = vec4<f32>(0.0);
    if union_channel != SHADOWMASK_UNION_CHANNEL_NONE {
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
        "shadowmask_channel_value",
        "sample_shadowmask_atlas",
        "shadowmask_visibility_for_spec_light",
        "shadowmask_attenuation",
        "shadowmask_union_channel",
    ]
    .into_iter()
    .map(|name| wgsl_function(FORWARD_WGSL, name))
    .collect();
    format!("{prelude}\n{consts}\n{}\n{entry}", helpers.join("\n\n"))
}

#[derive(Clone, Copy)]
struct Probe {
    uv: [f32; 2],
    layer: u32,
    /// Mask slot `0..3`, or `SHADOWMASK_CHANNEL_DROPPED`. Drives the
    /// world-specular channel.
    slot: u8,
    /// The promoted-union metadata channel float, as the CPU uploads it.
    union_channel: f32,
}

impl Probe {
    fn new(uv: [f32; 2], layer: u32, slot: u8) -> Self {
        Self {
            uv,
            layer,
            slot,
            union_channel: metadata_channel_value(slot),
        }
    }

    fn bytes(self) -> [u8; PROBE_BYTES] {
        let spec_channel = if self.slot == SHADOWMASK_CHANNEL_DROPPED {
            SPEC_LIGHT_SHADOWMASK_NONE
        } else {
            f32::from(self.slot)
        };
        let words = [
            self.uv[0].to_bits(),
            self.uv[1].to_bits(),
            self.layer,
            self.union_channel.to_bits(),
            spec_channel.to_bits(),
            0,
            0,
            0,
        ];
        let mut out = [0u8; PROBE_BYTES];
        for (chunk, word) in out.chunks_exact_mut(4).zip(words) {
            chunk.copy_from_slice(&word.to_ne_bytes());
        }
        out
    }
}

struct ProbeResult {
    mask: [f32; 4],
    spec_visibility: f32,
    /// `shadowmask_union_channel`'s result; the union skips the light when it
    /// is `union_channel_none()`.
    union_channel: u32,
    /// The union's selected mask value; meaningful only for a real channel.
    union_visibility: f32,
    union_attenuation_at_entity_visibility_one: f32,
    union_attenuation_at_entity_visibility_zero: f32,
}

fn run_probes(ctx: &GpuCtx, texture: &wgpu::Texture, probes: &[Probe]) -> Vec<ProbeResult> {
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
    let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("harness uniforms"),
        contents: &[0u8; 16],
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let probe_bytes: Vec<u8> = probes.iter().flat_map(|probe| probe.bytes()).collect();
    let probe_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("probes"),
        contents: &probe_bytes,
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
        let bytes = slice.get_mapped_range();
        let texels = bytes[..(width * OUTPUT_TEXEL_BYTES) as usize]
            .chunks_exact(OUTPUT_TEXEL_BYTES as usize)
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
        .chunks_exact(PIXELS_PER_PROBE as usize)
        .zip(values.chunks_exact(PIXELS_PER_PROBE as usize))
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

/// The raw byte slot `slot` holds in lightmap block (`bx`, `by`) of `layer`.
/// Distinct per slot, block and layer, and constant within a block, so BC4
/// reproduces it exactly and any misrouted read shows as a wrong value.
fn mask_byte(slot: u32, bx: u32, by: u32, layer: u32) -> u8 {
    (10 + slot * 50 + bx * 20 + by * 7 + layer * 3) as u8
}

/// A side-by-side BC5 section whose group 0 holds slots 0/1 and group 1
/// slots 2/3, built block by block as the wire format lays them out.
fn fixture_section() -> ShadowmaskAtlasSection {
    let bc4 = |value: u8| [value, value, 0, 0, 0, 0, 0, 0];
    let mut data = Vec::new();
    for layer in 0..ATLAS_LAYERS {
        for by in 0..ATLAS_HEIGHT / 4 {
            for group in 0..2 {
                for bx in 0..ATLAS_WIDTH / 4 {
                    data.extend_from_slice(&bc4(mask_byte(group * 2, bx, by, layer)));
                    data.extend_from_slice(&bc4(mask_byte(group * 2 + 1, bx, by, layer)));
                }
            }
        }
    }
    ShadowmaskAtlasSection {
        format: SHADOWMASK_FORMAT_BC5_RG_SIDE_BY_SIDE,
        width: ATLAS_WIDTH,
        height: ATLAS_HEIGHT,
        layer_count: ATLAS_LAYERS,
        channels: vec![0, 1, 2, 3],
        data,
    }
}

fn expected_mask(texel_x: u32, texel_y: u32, layer: u32) -> [f32; 4] {
    std::array::from_fn(|slot| {
        f32::from(mask_byte(slot as u32, texel_x / 4, texel_y / 4, layer)) / 255.0
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

/// Both decode paths select slot `slot`'s own mask, or read fully lit / skip
/// for the dropped sentinel. For a real slot, the union's attenuation is wired
/// to that slot's mask: all of it at entity visibility 0, none at 1.
fn assert_selects(result: &ProbeResult, slot: u8, what: &str) {
    if slot == SHADOWMASK_CHANNEL_DROPPED {
        assert_near(
            result.spec_visibility,
            1.0,
            &format!("{what}: world specular reads a dropped slot fully lit"),
        );
        assert_eq!(
            result.union_channel,
            union_channel_none(),
            "{what}: the promoted union must skip a dropped slot"
        );
        return;
    }
    let baked_vis = result.mask[slot as usize];
    assert_near(
        result.spec_visibility,
        baked_vis,
        &format!("{what}: world specular"),
    );
    assert_eq!(
        result.union_channel,
        u32::from(slot),
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
        &format!("{what}: a fully occluded entity attenuates by the slot's own mask"),
    );
}

// Pins: second-group-light, seam-bleed, seam-outer-halftexel. M1 (union
// half), M3, M4 (union half). Seam-bleed rides on the driven u = 0 / u = 1
// probes against groups that differ at the seam.
#[test]
fn every_slot_reads_its_own_group_at_centers_block_edges_and_outer_uv() {
    let Some(ctx) =
        gpu_or_skip("every_slot_reads_its_own_group_at_centers_block_edges_and_outer_uv")
    else {
        return;
    };
    let (header, data) = fixture_section().into_parts();
    let texture = upload_shadowmask_texture(&ctx.device, &ctx.queue, &header, &data);

    let mut probes = Vec::new();
    let mut expectations = Vec::new();
    for layer in 0..ATLAS_LAYERS {
        for &texel_y in &[0, 5] {
            let v = texel_center(texel_y, ATLAS_HEIGHT);
            // Texel centers either side of the block edge inside each group.
            for &texel_x in &[0, 3, 4, ATLAS_WIDTH - 1] {
                let u = texel_center(texel_x, ATLAS_WIDTH);
                expectations.push((
                    expected_mask(texel_x, texel_y, layer),
                    format!("texel ({texel_x}, {texel_y}) layer {layer}"),
                ));
                probes.push([u, v, layer as f32]);
            }
            // Driven outer UVs a baked chart gutter never reaches: each group
            // must clamp to its own edge column, never blend across the seam.
            for (u, texel_x) in [
                (0.0, 0),
                (0.2 / ATLAS_WIDTH as f32, 0),
                (1.0, ATLAS_WIDTH - 1),
                (1.0 - 0.2 / ATLAS_WIDTH as f32, ATLAS_WIDTH - 1),
            ] {
                expectations.push((
                    expected_mask(texel_x, texel_y, layer),
                    format!("u = {u}, row {texel_y}, layer {layer}"),
                ));
                probes.push([u, v, layer as f32]);
            }
        }
    }
    // A baked layer index past the atlas clamps to its last layer.
    probes.push([
        texel_center(1, ATLAS_WIDTH),
        texel_center(1, ATLAS_HEIGHT),
        7.0,
    ]);
    expectations.push((
        expected_mask(1, 1, ATLAS_LAYERS - 1),
        "layer clamp".to_string(),
    ));

    // The seam really differs: group 0's last column against group 1's first.
    let seam_left = expected_mask(ATLAS_WIDTH - 1, 0, 0);
    let seam_right = expected_mask(0, 0, 0);
    assert_ne!(seam_left[0], seam_right[2]);
    assert_ne!(seam_left[1], seam_right[3]);

    let slots = [0u8, 1, 2, 3, SHADOWMASK_CHANNEL_DROPPED];
    let expanded: Vec<Probe> = probes
        .iter()
        .flat_map(|&[u, v, layer]| {
            slots
                .iter()
                .map(move |&slot| Probe::new([u, v], layer as u32, slot))
        })
        .collect();
    let results = run_probes(&ctx, &texture, &expanded);

    for (index, result) in results.iter().enumerate() {
        let (expected, what) = &expectations[index / slots.len()];
        let slot = slots[index % slots.len()];
        for (component, (&actual, &want)) in result.mask.iter().zip(expected).enumerate() {
            assert_near(actual, want, &format!("{what}: slot {component} mask"));
        }
        assert_selects(result, slot, &format!("{what}, slot {slot}"));
    }
}

// Pin: placeholder-second-group. A second-group slot against the 2×1 white
// placeholder reads a real, fully visible texel at any UV or baked layer.
#[test]
fn placeholder_reads_fully_lit_for_every_slot_and_layer() {
    let Some(ctx) = gpu_or_skip("placeholder_reads_fully_lit_for_every_slot_and_layer") else {
        return;
    };
    let texture = upload_placeholder_shadowmask(&ctx.device, &ctx.queue);
    // One real texel per group: a 1×1 placeholder would pass the reads below
    // with both groups sharing one texel.
    assert_eq!(texture.width(), SHADOWMASK_GROUP_COUNT);
    assert_eq!(texture.height(), 1);
    assert_eq!(texture.depth_or_array_layers(), 1);
    let probes: Vec<Probe> = [[0.0, 0.0], [0.5, 0.5], [1.0, 1.0], [0.97, 0.03]]
        .into_iter()
        .flat_map(|uv| {
            [0u32, 3].into_iter().flat_map(move |layer| {
                [0u8, 1, 2, 3, SHADOWMASK_CHANNEL_DROPPED]
                    .into_iter()
                    .map(move |slot| Probe::new(uv, layer, slot))
            })
        })
        .collect();
    let results = run_probes(&ctx, &texture, &probes);
    for (probe, result) in probes.iter().zip(&results) {
        let what = format!(
            "placeholder uv {:?} layer {} slot {}",
            probe.uv, probe.layer, probe.slot
        );
        assert!(
            result.mask.iter().all(|&m| m == 1.0),
            "{what}: mask {:?}",
            result.mask
        );
        assert_selects(result, probe.slot, &what);
        assert_eq!(result.spec_visibility, 1.0, "{what}");
    }
}

// M1 (union half): the union's real channel guard, run on an adapter. Every
// metadata float the CPU never emits for a live slot — negative, fractional,
// at or past the dropped sentinel — resolves to the skip sentinel, and the
// real path then skips the light instead of selecting a mask. NaN is left out:
// WGSL lets implementations assume it never occurs.
#[test]
fn union_channel_skips_dropped_negative_fractional_and_out_of_range_metadata() {
    let Some(ctx) =
        gpu_or_skip("union_channel_skips_dropped_negative_fractional_and_out_of_range_metadata")
    else {
        return;
    };
    let (header, data) = fixture_section().into_parts();
    let texture = upload_shadowmask_texture(&ctx.device, &ctx.queue, &header, &data);
    let uv = [texel_center(1, ATLAS_WIDTH), texel_center(1, ATLAS_HEIGHT)];
    let invalid = [
        metadata_channel_value(SHADOWMASK_CHANNEL_DROPPED),
        -1.0,
        -0.5,
        0.5,
        2.25,
        3.999,
        4.0,
        5.0,
        255.0,
    ];
    let probes: Vec<Probe> = invalid
        .iter()
        .map(|&union_channel| Probe {
            union_channel,
            ..Probe::new(uv, 0, SHADOWMASK_CHANNEL_DROPPED)
        })
        .collect();
    let results = run_probes(&ctx, &texture, &probes);
    for (probe, result) in probes.iter().zip(&results) {
        assert_eq!(
            result.union_channel,
            union_channel_none(),
            "union channel metadata {} must skip the light",
            probe.union_channel
        );
    }
}
