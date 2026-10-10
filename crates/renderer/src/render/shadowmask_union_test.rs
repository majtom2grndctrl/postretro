// GPU readback of forward.wgsl's promoted-light union subtraction against a
// test-held verbatim copy of the pre-ownership function, on CPU-packed metadata.
// See: context/lib/rendering_pipeline.md §4 (Static specular ownership)
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests": the
// function is WGSL, so verifying it means running it. The pool samplers and
// the mask sample are stubbed with fixed values, the same for both functions,
// so the only difference under test is how each finds a light's mask channel.
// The harness self-skips without an adapter and says so; a skipped run proves
// nothing. Set `POSTRETRO_REQUIRE_GPU` to any non-empty value other than `0`
// to make a missing adapter fail instead.

use postretro_render_cpu::frame_uniforms::SPEC_SHADOWMASK_FLAG_MASK_PRESENT;

use super::gpu_test_harness::{GpuCtx, try_init_gpu};
use super::renderer_types::{
    PromotedBakedLightRecord, PromotedBakedLightSource, PromotedShadowPoolKind,
};
use super::shadowmask::pack_forward_shadowmask_metadata;

const FORWARD_WGSL: &str = include_str!("../shaders/forward.wgsl");
const LIGHT_FALLOFF_WGSL: &str = include_str!("../shaders/light_falloff.wgsl");

const WIDTH: u32 = 16;
const TEXEL_BYTES: u32 = 16;
const E: u16 = 0xFFFF;
/// Spot pool slots in the shader's `light_space_matrices`; the cube stub's
/// visibilities follow them in one table.
const SPOT_SLOTS: usize = 96;
/// The pre-change metadata `.z` for a dropped or unslotted light.
const PRE_CHANGE_DROPPED: f32 = 4.0;

/// `shadowmask_union_subtraction` and its channel guard as they stood before
/// per-face ownership, verbatim but for their names: a promoted light read
/// the map-wide mask slot the CPU packed into its metadata `.z`.
const PRE_CHANGE_UNION: &str = r#"
const SHADOWMASK_CHANNEL_DROPPED: f32 = 4.0;
// `shadowmask_union_channel`'s skip sentinel; never a mask slot.
const SHADOWMASK_UNION_CHANNEL_NONE: u32 = 4u;

// A promoted light's mask slot from its metadata channel float, or
// SHADOWMASK_UNION_CHANNEL_NONE for a dropped, negative, fractional or
// out-of-range value. The union skips a light whose slot is NONE, so the u32
// cast here follows every guard.
fn pre_change_shadowmask_union_channel(channel_value: f32) -> u32 {
    if channel_value < 0.0 ||
       channel_value >= SHADOWMASK_CHANNEL_DROPPED ||
       floor(channel_value) != channel_value {
        return SHADOWMASK_UNION_CHANNEL_NONE;
    }
    return u32(channel_value);
}

fn pre_change_shadowmask_union_subtraction(
    world_pos: vec3<f32>,
    lightmap_texel: vec2<f32>,
    lightmap_layer_flags: u32,
    lightmap_rect: vec4<u32>,
    mesh_n: vec3<f32>,
    bump_n: vec3<f32>,
) -> ShadowmaskUnion {
    var out: ShadowmaskUnion;
    out.subtraction = vec3<f32>(0.0);
    // White means no eligible promoted light covers this receiver.
    out.raw_pool_visibility = 1.0;
    let promoted_start = min(
        uniforms.light_count + animated_baked_light_tail_count(),
        uniforms.total_light_count,
    );
    if uniforms.total_light_count <= promoted_start {
        return out;
    }
    // Hoisted because every promoted light shares this fragment's lightmap
    // block and texel. A missed block reads zero, so no light subtracts.
    let mask = sample_shadowmask_atlas(lightmap_texel, lightmap_layer_flags, lightmap_rect);
    let promoted_count = uniforms.total_light_count - promoted_start;
    let influence_len = arrayLength(&light_influence);
    let spec_len = arrayLength(&spec_lights);
    for (var p: u32 = 0u; p < promoted_count; p = p + 1u) {
        let influence_index = promoted_start + p;
        if influence_index >= influence_len {
            break;
        }
        let influence = light_influence[influence_index];
        let inf_radius = influence.w;
        if inf_radius <= 1.0e30 {
            let d = world_pos - influence.xyz;
            if dot(d, d) > inf_radius * inf_radius {
                continue;
            }
        }

        let meta_index = uniforms.total_light_count
            + animated_baked_light_tail_count() * SHADOWMASK_META_VEC4S_PER_RECORD
            + p * SHADOWMASK_META_VEC4S_PER_RECORD;
        if meta_index + 1u >= influence_len {
            break;
        }
        let meta0 = light_influence[meta_index];
        let meta1 = light_influence[meta_index + 1u];
        let spec_idx_value = meta0.z;
        let weight = clamp(meta0.w, 0.0, 1.0);
        let pool_kind_value = meta1.x;
        let slot_value = meta1.y;
        let channel = pre_change_shadowmask_union_channel(meta1.z);

        if weight <= 0.0 ||
           spec_idx_value <= SHADOWMASK_INVALID_INDEX_VALUE ||
           spec_idx_value >= f32(spec_len) ||
           channel == SHADOWMASK_UNION_CHANNEL_NONE {
            continue;
        }
        if pool_kind_value != SHADOWMASK_POOL_SPOT_VALUE && pool_kind_value != SHADOWMASK_POOL_CUBE_VALUE {
            continue;
        }
        if floor(spec_idx_value) != spec_idx_value ||
           floor(slot_value) != slot_value {
            continue;
        }

        let spec_idx = u32(spec_idx_value);
        let pool_kind = u32(pool_kind_value);
        let slot = u32(slot_value);
        let sl = spec_lights[spec_idx];
        let direct = shadowmask_direct(sl, world_pos, mesh_n, bump_n);
        if direct.valid == 0u {
            continue;
        }

        let baked_vis = shadowmask_channel_value(mask, channel);
        // A baked-dark channel cannot contribute to production attenuation.
        // Mode 6 still samples it to diagnose entity-only pool visibility.
        if uniforms.sdf_shadow_mode != SHADOWMASK_RAW_POOL_VISIBILITY_MODE &&
           baked_vis <= 0.0 {
            continue;
        }
        let shadow_map_vis = shadowmask_shadow_visibility(pool_kind, slot, sl, world_pos, mesh_n);
        // The raw-pool diagnostic follows the union's promoted-light coverage;
        // when several lights cover a receiver, the darkest map visibility wins.
        out.raw_pool_visibility = min(out.raw_pool_visibility, shadow_map_vis);
        out.subtraction = out.subtraction + direct.value * shadowmask_attenuation(baked_vis, shadow_map_vis) * weight;
    }
    return out;
}
"#;

fn gpu_or_skip(test: &str) -> Option<GpuCtx> {
    if let Some(ctx) = try_init_gpu() {
        return Some(ctx);
    }
    let required =
        std::env::var("POSTRETRO_REQUIRE_GPU").is_ok_and(|value| !value.is_empty() && value != "0");
    assert!(
        !required,
        "[shadowmask_union_test] {test}: POSTRETRO_REQUIRE_GPU is set but no GPU adapter is available"
    );
    eprintln!("[shadowmask_union_test] skipping {test}: no GPU adapter available");
    None
}

/// The text of `fn name(` through its matching closing brace in `source`.
fn wgsl_function<'a>(source: &'a str, name: &str) -> &'a str {
    let header = format!("fn {name}(");
    let start = source
        .find(&header)
        .unwrap_or_else(|| panic!("the WGSL source must declare `{header}`"));
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
    panic!("`{header}` never closes");
}

/// `struct name {` through its `};` in forward.wgsl.
fn wgsl_struct(name: &str) -> &'static str {
    let header = format!("struct {name} {{");
    let start = FORWARD_WGSL
        .find(&header)
        .unwrap_or_else(|| panic!("forward.wgsl must declare `{name}`"));
    let end = start + FORWARD_WGSL[start..].find("};").expect("struct closes") + 2;
    &FORWARD_WGSL[start..end]
}

fn wgsl_const_line(name: &str) -> &'static str {
    FORWARD_WGSL
        .lines()
        .find(|line| line.starts_with(&format!("const {name}:")))
        .unwrap_or_else(|| panic!("forward.wgsl must declare `{name}`"))
}

fn shader_source() -> String {
    let prelude = r#"
struct SpecLight {
    position_and_range: vec4<f32>,
    color_and_pad:      vec4<f32>,
    cone_dir_and_type:  vec4<f32>,
    cone_cos:           vec4<f32>,
};
// The forward `Uniforms` members the union reads, by their own names.
struct HarnessUniforms {
    light_count: u32,
    total_light_count: u32,
    has_direct: u32,
    sdf_shadow_mode: u32,
    spec_shadowmask_flags: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    mask: vec4<f32>,
    owners: vec4<u32>,
};
struct LightSpaceMatrices {
    m: array<mat4x4<f32>, 96>,
};
@group(0) @binding(0) var<uniform> uniforms: HarnessUniforms;
@group(0) @binding(1) var<storage, read> light_influence: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> spec_lights: array<SpecLight>;
// Entity-only pool visibility per slot: spot slots, then cube slots.
@group(0) @binding(3) var<storage, read> pool_visibility: array<f32>;
var<private> light_space_matrices: LightSpaceMatrices;

// Stubs for the samplers both unions share.
fn sample_shadowmask_atlas(texel: vec2<f32>, layer_flags: u32, rect: vec4<u32>) -> vec4<f32> {
    return uniforms.mask;
}
fn sample_spot_shadow(
    slot_index: u32,
    light_pos: vec3<f32>,
    world_pos: vec3<f32>,
    receiver_normal: vec3<f32>,
    bias_scale: f32,
    light_proj: mat4x4<f32>,
) -> f32 {
    return pool_visibility[slot_index];
}
fn sample_point_shadow(
    slot_index: u32,
    light_pos: vec3<f32>,
    world_pos: vec3<f32>,
    receiver_normal: vec3<f32>,
    bias_scale: f32,
    far_range: f32,
) -> f32 {
    return pool_visibility[96u + slot_index];
}
"#;
    let entry = r#"
struct HarnessOut {
    @location(0) current: vec4<f32>,
    @location(1) pre_change: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> HarnessOut {
    // Column c is a floor point at x = (c + 0.5 - 8) * 0.5, normal-mapped
    // slightly off the geometric +Y.
    let world = vec3<f32>((position.x - 8.0) * 0.5, 0.0, 0.25);
    let mesh_n = vec3<f32>(0.0, 1.0, 0.0);
    let bump_n = normalize(vec3<f32>(0.15, 1.0, -0.1));
    let current = shadowmask_union_subtraction(
        world, vec2<f32>(0.0), 0u, vec4<u32>(0u), mesh_n, bump_n, uniforms.owners,
    );
    let pre_change = pre_change_shadowmask_union_subtraction(
        world, vec2<f32>(0.0), 0u, vec4<u32>(0u), mesh_n, bump_n,
    );
    var out: HarnessOut;
    out.current = vec4<f32>(current.subtraction, current.raw_pool_visibility);
    out.pre_change = vec4<f32>(pre_change.subtraction, pre_change.raw_pool_visibility);
    return out;
}
"#;
    let consts = [
        "SHADOWMASK_RAW_POOL_VISIBILITY_MODE",
        "SHADOWMASK_INVALID_INDEX_VALUE",
        "SHADOWMASK_POOL_SPOT",
        "SHADOWMASK_POOL_CUBE",
        "SHADOWMASK_POOL_SPOT_VALUE",
        "SHADOWMASK_POOL_CUBE_VALUE",
        "SHADOWMASK_SPOT_SLOT_COUNT",
        "SHADOWMASK_CUBE_SLOT_COUNT",
        "SHADOWMASK_EPS",
        "SHADOWMASK_NDOTL_EPS",
        "SHADOWMASK_META_VEC4S_PER_RECORD",
        "SHADOWMASK_UNION_RECEIVER_BIAS_SCALE",
        "SPEC_OWNER_EMPTY",
        "SPEC_OWNER_CHANNELS",
        "SPEC_SHADOWMASK_FLAG_MASK_PRESENT",
    ]
    .map(wgsl_const_line)
    .join("\n");
    let structs = [
        wgsl_struct("ShadowmaskDirect"),
        wgsl_struct("ShadowmaskUnion"),
    ]
    .join("\n");
    let helpers = [
        wgsl_function(FORWARD_WGSL, "animated_baked_light_tail_count"),
        wgsl_function(FORWARD_WGSL, "cone_attenuation_cos"),
        wgsl_function(FORWARD_WGSL, "shadowmask_channel_value"),
        wgsl_function(FORWARD_WGSL, "static_spec_mask_present"),
        wgsl_function(FORWARD_WGSL, "shadowmask_owner_channel"),
        wgsl_function(FORWARD_WGSL, "shadowmask_direct"),
        wgsl_function(FORWARD_WGSL, "shadowmask_shadow_visibility"),
        wgsl_function(FORWARD_WGSL, "shadowmask_attenuation"),
        wgsl_function(FORWARD_WGSL, "shadowmask_union_subtraction"),
        wgsl_function(LIGHT_FALLOFF_WGSL, "light_eval_falloff"),
    ]
    .join("\n\n");
    format!("{prelude}\n{consts}\n{structs}\n{helpers}\n{PRE_CHANGE_UNION}\n{entry}")
}

/// A linear-falloff point light, or a spot aimed straight down, as
/// `pack_spec_lights` lays it out.
fn spec_light(position: [f32; 3], color: [f32; 3], spot: bool) -> Vec<u8> {
    let (aim, kind, cos_inner, cos_outer) = if spot {
        (
            [0.0, -1.0, 0.0],
            1.0,
            0.6f32.cos(),
            std::f32::consts::FRAC_PI_3.cos(),
        )
    } else {
        ([0.0; 3], 0.0, 1.0, -1.0)
    };
    let floats = [
        position[0],
        position[1],
        position[2],
        9.0,
        color[0],
        color[1],
        color[2],
        0.0,
        aim[0],
        aim[1],
        aim[2],
        kind,
        cos_inner,
        cos_outer,
        0.0,
        0.0,
    ];
    floats.iter().flat_map(|f| f.to_ne_bytes()).collect()
}

/// Four spec lights over the floor, all in range of every column.
fn spec_lights() -> Vec<u8> {
    [
        spec_light([-2.0, 2.0, 0.0], [1.0, 0.2, 0.1], false),
        spec_light([1.5, 2.5, 0.5], [0.1, 1.0, 0.3], true),
        spec_light([0.0, 3.0, -1.0], [0.6, 0.6, 0.6], false),
        spec_light([2.5, 1.5, 0.0], [0.2, 0.3, 1.0], false),
    ]
    .concat()
}

/// A promoted selected-static light: its compact spec index, pool slot and
/// promotion weight.
struct Promoted {
    spec_index: u32,
    pool: PromotedShadowPoolKind,
    slot: u32,
    weight: f32,
}

/// Spec lights 3, 1 and 0 promoted, in that record order.
fn promoted() -> [Promoted; 3] {
    [
        Promoted {
            spec_index: 3,
            pool: PromotedShadowPoolKind::Spot,
            slot: 2,
            weight: 1.0,
        },
        Promoted {
            spec_index: 1,
            pool: PromotedShadowPoolKind::Cube,
            slot: 1,
            weight: 0.6,
        },
        Promoted {
            spec_index: 0,
            pool: PromotedShadowPoolKind::Spot,
            slot: 7,
            weight: 0.35,
        },
    ]
}

/// The influence buffer the renderer uploads with no dynamic or animated
/// lights: one uncullable influence per promoted light, then the metadata
/// tail from the production packer. `pre_change_slots[p]` overwrites record
/// `p`'s reserved `.z` lane with the map-wide mask slot the pre-change packer
/// wrote there.
fn influence_bytes(promoted: &[Promoted], pre_change_slots: &[f32]) -> Vec<u8> {
    let mut bytes: Vec<u8> = Vec::new();
    for _ in promoted {
        for value in [0.0f32, 0.0, 0.0, 2.0e30] {
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
    }
    let records: Vec<PromotedBakedLightRecord> = promoted
        .iter()
        .enumerate()
        .map(|(index, light)| PromotedBakedLightRecord {
            global_light_index: 20 + index as u32,
            pool_kind: light.pool,
            slot: light.slot,
            weight: light.weight,
            source: PromotedBakedLightSource::SelectedStatic {
                selection_index: index as u32,
            },
        })
        .collect();
    let spec_indices: Vec<u32> = promoted.iter().map(|light| light.spec_index).collect();
    let mut metadata = Vec::new();
    pack_forward_shadowmask_metadata(
        0,
        &records,
        &vec![-1; records.len()],
        &spec_indices,
        &mut metadata,
    );
    for (record, slot) in pre_change_slots.iter().enumerate() {
        let lane = record * 32 + 16 + 8;
        metadata[lane..lane + 4].copy_from_slice(&slot.to_ne_bytes());
    }
    bytes.extend_from_slice(&metadata);
    bytes
}

/// Entity-only visibility per pool slot: each promoted light's occluder
/// leaves a different fraction of its pool light.
fn pool_visibility() -> Vec<u8> {
    let mut visibility = vec![1.0f32; SPOT_SLOTS + 6];
    visibility[2] = 0.0;
    visibility[7] = 0.8;
    visibility[SPOT_SLOTS + 1] = 0.4;
    visibility.iter().flat_map(|v| v.to_ne_bytes()).collect()
}

#[derive(Clone, Copy)]
struct Case {
    promoted_count: u32,
    sdf_shadow_mode: u32,
    mask_present: bool,
    mask: [f32; 4],
    owners: [u16; 4],
}

impl Case {
    fn bytes(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        let flags = if self.mask_present {
            SPEC_SHADOWMASK_FLAG_MASK_PRESENT
        } else {
            0
        };
        for word in [
            0u32,
            self.promoted_count,
            0,
            self.sdf_shadow_mode,
            flags,
            0,
            0,
            0,
        ] {
            out.extend_from_slice(&word.to_ne_bytes());
        }
        for value in self.mask {
            out.extend_from_slice(&value.to_ne_bytes());
        }
        for owner in self.owners {
            out.extend_from_slice(&u32::from(owner).to_ne_bytes());
        }
        out
    }
}

/// Per column: (current union, pre-change union), each as RGB subtraction
/// plus raw pool visibility in alpha.
fn render(ctx: &GpuCtx, case: Case, influence: &[u8]) -> Vec<([f32; 4], [f32; 4])> {
    use wgpu::util::DeviceExt;

    let device = &ctx.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shadowmask_union_test shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source().into()),
    });
    let buffer = |label: &str, contents: &[u8], usage: wgpu::BufferUsages| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents,
            usage,
        })
    };
    let uniforms = buffer("union case", &case.bytes(), wgpu::BufferUsages::UNIFORM);
    let influence = buffer("union influence", influence, wgpu::BufferUsages::STORAGE);
    let lights = buffer(
        "union spec lights",
        &spec_lights(),
        wgpu::BufferUsages::STORAGE,
    );
    let pool = buffer(
        "union pool visibility",
        &pool_visibility(),
        wgpu::BufferUsages::STORAGE,
    );
    let entry = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let storage = wgpu::BufferBindingType::Storage { read_only: true };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("shadowmask_union_test layout"),
        entries: &[
            entry(0, wgpu::BufferBindingType::Uniform),
            entry(1, storage),
            entry(2, storage),
            entry(3, storage),
        ],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("shadowmask_union_test bind group"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: influence.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: lights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: pool.as_entire_binding(),
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("shadowmask_union_test pipeline layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let format = wgpu::TextureFormat::Rgba32Float;
    let target_state = Some(wgpu::ColorTargetState {
        format,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("shadowmask_union_test pipeline"),
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

    let targets: Vec<wgpu::Texture> = (0..2)
        .map(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("shadowmask_union_test target"),
                size: wgpu::Extent3d {
                    width: WIDTH,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        })
        .collect();
    let views: Vec<_> = targets
        .iter()
        .map(|target| target.create_view(&Default::default()))
        .collect();
    let row_bytes = (WIDTH * TEXEL_BYTES).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readbacks: Vec<wgpu::Buffer> = (0..2)
        .map(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("shadowmask_union_test readback"),
                size: u64::from(row_bytes),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })
        .collect();

    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let attachments: Vec<_> = views
            .iter()
            .map(|view| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: -1.0,
                            g: -1.0,
                            b: -1.0,
                            a: -1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })
            })
            .collect();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shadowmask_union_test pass"),
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
                width: WIDTH,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
    }
    ctx.queue.submit([encoder.finish()]);

    let read = |buffer: &wgpu::Buffer| -> Vec<[f32; 4]> {
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |result| {
            result.expect("map shadowmask_union_test readback");
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll shadowmask_union_test device");
        let bytes = slice
            .get_mapped_range()
            .expect("buffer mapped for readback");
        let texels = bytes[..(WIDTH * TEXEL_BYTES) as usize]
            .as_chunks::<{ TEXEL_BYTES as usize }>()
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
    let current = read(&readbacks[0]);
    let pre_change = read(&readbacks[1]);
    current.into_iter().zip(pre_change).collect()
}

fn assert_close(actual: [f32; 4], expected: [f32; 4], what: &str) {
    for c in 0..4 {
        assert!(
            (actual[c] - expected[c]).abs() <= 1.0e-6 + expected[c].abs() * 1.0e-5,
            "{what}: lane {c} is {actual:?}, expected {expected:?}"
        );
    }
}

/// A mask with one baked-dark channel, so the production skip and mode 6's
/// diagnostic both run.
const MASK: [f32; 4] = [0.9, 0.55, 0.0, 0.7];

// AC 18: on a face every promoted light owns, the ownership union reproduces
// the pre-change union exactly, with each light's owner position equal to the
// map-wide slot it held before. Owners ascend (0, 1, 3) while records arrive
// as 3, 1, 0, and spec light 3's position 2 holds a baked-dark channel.
#[test]
fn union_matches_the_pre_change_union_where_every_promoted_light_owns_the_face() {
    let Some(ctx) =
        gpu_or_skip("union_matches_the_pre_change_union_where_every_promoted_light_owns_the_face")
    else {
        return;
    };
    let promoted = promoted();
    // Owners [0, 1, 3]: spec light 3 at position 2, 1 at 1, 0 at 0.
    let influence = influence_bytes(&promoted, &[2.0, 1.0, 0.0]);
    for (sdf_shadow_mode, mask) in [(0, MASK), (6, MASK), (0, [0.9, 0.55, 0.35, 0.7])] {
        let case = Case {
            promoted_count: promoted.len() as u32,
            sdf_shadow_mode,
            mask_present: true,
            mask,
            owners: [0, 1, 3, E],
        };
        let out = render(&ctx, case, &influence);
        let mut subtracted = false;
        for (column, (current, pre_change)) in out.iter().enumerate() {
            assert!(pre_change[3] >= 0.0, "column {column} rendered");
            assert_close(
                *current,
                *pre_change,
                &format!("mode {sdf_shadow_mode} mask {mask:?} column {column}"),
            );
            subtracted |= current[..3].iter().any(|&c| c > 1.0e-4);
        }
        assert!(
            subtracted,
            "mode {sdf_shadow_mode}: the fixture must subtract"
        );
    }
}

// AC 19: a promoted light that does not own the face subtracts nothing there.
// The current union equals the pre-change union with that light unslotted.
#[test]
fn union_subtracts_nothing_for_a_promoted_light_that_does_not_own_the_face() {
    let Some(ctx) =
        gpu_or_skip("union_subtracts_nothing_for_a_promoted_light_that_does_not_own_the_face")
    else {
        return;
    };
    let promoted = promoted();
    let base = Case {
        promoted_count: promoted.len() as u32,
        sdf_shadow_mode: 0,
        mask_present: true,
        mask: [0.9, 0.55, 0.35, 0.7],
        owners: [E; 4],
    };

    // Only spec light 1 owns this face, at position 0.
    let only_light_1 = render(
        &ctx,
        Case {
            owners: [1, E, E, E],
            ..base
        },
        &influence_bytes(&promoted, &[PRE_CHANGE_DROPPED, 0.0, PRE_CHANGE_DROPPED]),
    );
    // The same lights, all slotted as before ownership: lights 3 and 0 would
    // have subtracted here.
    let all_slotted = render(
        &ctx,
        Case {
            owners: [1, E, E, E],
            ..base
        },
        &influence_bytes(&promoted, &[2.0, 0.0, 1.0]),
    );
    for (column, (current, pre_change)) in only_light_1.iter().enumerate() {
        assert_close(
            *current,
            *pre_change,
            &format!("column {column}: only the owner subtracts"),
        );
        assert!(
            all_slotted[column].1[..3]
                .iter()
                .zip(&current[..3])
                .any(|(slotted, owned)| slotted > &(owned + 1.0e-4)),
            "column {column}: the non-owners must have had something to subtract"
        );
    }

    // A face no promoted light owns subtracts nothing at all.
    let unowned = render(&ctx, base, &influence_bytes(&promoted, &[2.0, 0.0, 1.0]));
    for (column, (current, pre_change)) in unowned.iter().enumerate() {
        assert_eq!(current[..3], [0.0; 3], "column {column}");
        assert_eq!(current[3], 1.0, "column {column}: no light covers it");
        assert!(
            pre_change[..3].iter().any(|&c| c > 1.0e-4),
            "column {column}"
        );
    }
}

// AC 13 (union side): with the mask-present bit clear — no id 42, a refused
// pool, or placeholder mode — no promoted light subtracts, even where it owns
// the face.
#[test]
fn union_subtracts_nothing_without_a_bound_mask() {
    let Some(ctx) = gpu_or_skip("union_subtracts_nothing_without_a_bound_mask") else {
        return;
    };
    let promoted = promoted();
    let out = render(
        &ctx,
        Case {
            promoted_count: promoted.len() as u32,
            sdf_shadow_mode: 0,
            mask_present: false,
            mask: [1.0; 4],
            owners: [0, 1, 3, E],
        },
        &influence_bytes(&promoted, &[2.0, 1.0, 0.0]),
    );
    for (column, (current, _)) in out.iter().enumerate() {
        assert_eq!(current[..3], [0.0; 3], "column {column}");
        assert_eq!(current[3], 1.0, "column {column}");
    }
}
