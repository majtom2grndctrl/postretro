// GPU readback harness for forward.wgsl's promoted-light union subtraction, on
// CPU-packed metadata: against a test-held verbatim copy of the pre-ownership
// function, or through the real mask sampler beside the real world specular.
// See: context/lib/rendering_pipeline.md §4 (Static specular ownership)
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests": the
// function is WGSL, so verifying it means running it. The pool samplers are
// stubbed with fixed values. The pre-change comparison stubs the mask sample
// too, the same for both functions, so the only difference under test is how
// each finds a light's mask channel.
// The harness self-skips without an adapter and says so; a skipped run proves
// nothing. Set `POSTRETRO_REQUIRE_GPU` to any non-empty value other than `0`
// to make a missing adapter fail instead.

use super::gpu_test_harness::{GpuCtx, wgsl_const_line, wgsl_function, wgsl_item};
use super::shadowmask_union_fixtures::{Case, pool_visibility, spec_lights};

use crate::lighting::lightmap::filtering_sampler_descriptor;
use crate::lighting::lightmap::test_fixtures::lightmap_block_decode_wgsl;

const FORWARD_WGSL: &str = include_str!("../shaders/forward.wgsl");
const LIGHT_FALLOFF_WGSL: &str = include_str!("../shaders/light_falloff.wgsl");
const MATERIAL_SHADING_WGSL: &str = include_str!("../shaders/material_shading.wgsl");
const SDF_LIGHT_SELECT_WGSL: &str = include_str!("../shaders/sdf_light_select.wgsl");

const WIDTH: u32 = 16;
const TEXEL_BYTES: u32 = 16;

/// `shadowmask_union_subtraction` and its channel guard as they stood before
/// per-face ownership, verbatim but for their names: a promoted light read
/// the map-wide mask slot the CPU packed into its metadata `.z`. Every name
/// from that retired format carries a `pre_change` prefix, so none can stand
/// in for a production name the shader grep gates retire.
const PRE_CHANGE_UNION: &str = r#"
const PRE_CHANGE_SHADOWMASK_CHANNEL_DROPPED: f32 = 4.0;
// `pre_change_shadowmask_union_channel`'s skip sentinel; never a mask slot.
const PRE_CHANGE_SHADOWMASK_UNION_CHANNEL_NONE: u32 = 4u;

// A promoted light's mask slot from its metadata channel float, or
// PRE_CHANGE_SHADOWMASK_UNION_CHANNEL_NONE for a dropped, negative, fractional
// or out-of-range value. The union skips a light whose slot is NONE, so the u32
// cast here follows every guard.
fn pre_change_shadowmask_union_channel(channel_value: f32) -> u32 {
    if channel_value < 0.0 ||
       channel_value >= PRE_CHANGE_SHADOWMASK_CHANNEL_DROPPED ||
       floor(channel_value) != channel_value {
        return PRE_CHANGE_SHADOWMASK_UNION_CHANNEL_NONE;
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
    let mask = sample_shadowmask_atlas(lightmap_texel, lightmap_layer_flags, lightmap_rect, vec2<u32>(0u));
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
           channel == PRE_CHANGE_SHADOWMASK_UNION_CHANNEL_NONE {
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

/// What declares `sample_shadowmask_atlas`: a stub reading `uniforms.mask`, or
/// the real helper over a mask pool.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MaskSource {
    Stub,
    Sampled,
}

fn shader_source(mask_source: MaskSource) -> String {
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
    block: u32,
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

// Stubs for the shadow pool samplers.
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
    let stub_mask = r#"
fn sample_shadowmask_atlas(
    texel: vec2<f32>,
    layer_flags: u32,
    rect: vec4<u32>,
    plane_addresses: vec2<u32>,
) -> vec4<f32> {
    return uniforms.mask;
}
"#;
    let shared_entry = r#"
struct HarnessOut {
    @location(0) current: vec4<f32>,
    @location(1) second: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}
"#;
    // Column c is a floor point at x = (c + 0.5 - 8) * 0.5, normal-mapped
    // slightly off the geometric +Y.
    let surface = r#"
    let world = vec3<f32>((position.x - 8.0) * 0.5, 0.0, 0.25);
    let mesh_n = vec3<f32>(0.0, 1.0, 0.0);
    let bump_n = normalize(vec3<f32>(0.15, 1.0, -0.1));
"#;
    let stub_entry = format!(
        r#"
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> HarnessOut {{
{surface}
    let current = shadowmask_union_subtraction(
        world, vec2<f32>(0.0), 0u, vec4<u32>(0u), vec2<u32>(0u), mesh_n, bump_n, uniforms.owners,
    );
    let pre_change = pre_change_shadowmask_union_subtraction(
        world, vec2<f32>(0.0), 0u, vec4<u32>(0u), mesh_n, bump_n,
    );
    var out: HarnessOut;
    out.current = vec4<f32>(current.subtraction, current.raw_pool_visibility);
    out.second = vec4<f32>(pre_change.subtraction, pre_change.raw_pool_visibility);
    return out;
}}
"#
    );
    // fs_main's own sequence: the union, then the specular gate and its
    // hoisted mask read, each off the block the vertex stage resolved.
    let sampled_entry = format!(
        r#"
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> HarnessOut {{
{surface}
    let block = resolve_lightmap_block(uniforms.block, vec2<f32>(0.4, 0.6));
    let current = shadowmask_union_subtraction(
        world, block.texel, block.layer_flags, block.rect, block.plane_addresses,
        mesh_n, bump_n, uniforms.owners,
    );
    let owners_readable = static_spec_owners_readable(block.layer_flags);
    var specular_shadowmask = vec4<f32>(0.0);
    if owners_readable && uniforms.owners.x != SPEC_OWNER_EMPTY {{
        specular_shadowmask = sample_shadowmask_atlas(
            block.texel, block.layer_flags, block.rect, block.plane_addresses,
        );
    }}
    var no_sdf: SdfLightSelection;
    no_sdf.count = 0u;
    let specular = world_static_specular(
        world, bump_n, normalize(vec3<f32>(0.2, 1.0, 0.1)), 16.0, 1.0, uniforms.owners,
        specular_shadowmask, owners_readable, no_sdf, vec4<f32>(1.0), false,
    );
    var out: HarnessOut;
    out.current = vec4<f32>(current.subtraction, current.raw_pool_visibility);
    out.second = vec4<f32>(specular, 1.0);
    return out;
}}
"#
    );
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
        "SPEC_LIGHT_TYPE_DIRECTIONAL",
        "SPEC_SHADOWMASK_FLAG_FORCE_ONE",
        "SPEC_SHADOWMASK_FLAG_MASK_PRESENT",
    ]
    .map(|name| wgsl_const_line(FORWARD_WGSL, name))
    .join("\n");
    let structs = [
        wgsl_item(FORWARD_WGSL, "struct ShadowmaskDirect {"),
        wgsl_item(FORWARD_WGSL, "struct ShadowmaskUnion {"),
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
    match mask_source {
        MaskSource::Stub => format!(
            "{prelude}\n{stub_mask}\n{consts}\n{structs}\n{helpers}\n{PRE_CHANGE_UNION}\n\
             {shared_entry}\n{stub_entry}"
        ),
        MaskSource::Sampled => {
            let bindings = r#"
@group(0) @binding(4) var shadowmask_atlas: texture_2d_array<f32>;
@group(0) @binding(5) var lightmap_filtering_sampler: sampler;
@group(0) @binding(6) var<storage, read> lightmap_block_table: array<vec4<u32>>;
"#;
            let decode = lightmap_block_decode_wgsl();
            let specular = [
                wgsl_item(SDF_LIGHT_SELECT_WGSL, "struct SdfLightSelection {"),
                wgsl_function(MATERIAL_SHADING_WGSL, "blinn_phong"),
                wgsl_function(FORWARD_WGSL, "slice_for_visibility"),
                wgsl_function(FORWARD_WGSL, "positional_cone_aim"),
                wgsl_function(FORWARD_WGSL, "spec_shadowmask_forced_one"),
                wgsl_function(FORWARD_WGSL, "static_spec_owners_readable"),
                wgsl_function(FORWARD_WGSL, "world_static_light_specular"),
                wgsl_function(FORWARD_WGSL, "world_static_specular"),
            ]
            .join("\n\n");
            format!(
                "{prelude}\n{bindings}\n{decode}\n{consts}\n{structs}\n{helpers}\n{specular}\n\
                 {shared_entry}\n{sampled_entry}"
            )
        }
    }
}

/// A mask pool a sampled render reads, and its block table.
pub(super) struct MaskPool<'a> {
    pub(super) texture: &'a wgpu::Texture,
    pub(super) table: &'a [u8],
}

/// Per column: (current union, pre-change union), each as RGB subtraction
/// plus raw pool visibility in alpha.
pub(super) fn render(ctx: &GpuCtx, case: Case, influence: &[u8]) -> Vec<([f32; 4], [f32; 4])> {
    render_source(ctx, MaskSource::Stub, case, influence, None)
}

/// Per column: (current union as `render` gives it, world static specular in
/// RGB), through the real mask sampler over `pool`.
pub(super) fn render_sampled(
    ctx: &GpuCtx,
    case: Case,
    influence: &[u8],
    pool: &MaskPool<'_>,
) -> Vec<([f32; 4], [f32; 4])> {
    render_source(ctx, MaskSource::Sampled, case, influence, Some(pool))
}

fn render_source(
    ctx: &GpuCtx,
    mask_source: MaskSource,
    case: Case,
    influence: &[u8],
    pool: Option<&MaskPool<'_>>,
) -> Vec<([f32; 4], [f32; 4])> {
    use wgpu::util::DeviceExt;

    let device = &ctx.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shadowmask_union_test shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source(mask_source).into()),
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
    let pool_visibility = buffer(
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
    let mut layout_entries = vec![
        entry(0, wgpu::BufferBindingType::Uniform),
        entry(1, storage),
        entry(2, storage),
        entry(3, storage),
    ];
    let mut group_entries = vec![
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
            resource: pool_visibility.as_entire_binding(),
        },
    ];
    let sampled = pool.map(|pool| {
        let view = pool.texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&filtering_sampler_descriptor());
        let table = buffer("union block table", pool.table, wgpu::BufferUsages::STORAGE);
        (view, sampler, table)
    });
    if let Some((view, sampler, table)) = &sampled {
        layout_entries.extend([
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            entry(6, storage),
        ]);
        group_entries.extend([
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: table.as_entire_binding(),
            },
        ]);
    }
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("shadowmask_union_test layout"),
        entries: &layout_entries,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("shadowmask_union_test bind group"),
        layout: &layout,
        entries: &group_entries,
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
