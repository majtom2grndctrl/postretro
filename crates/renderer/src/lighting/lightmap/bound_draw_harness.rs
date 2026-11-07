// Draws probes through a level's real bound resources: the group-4 bind group
// and the VERTEX-only group-6 block table `LightmapResources` holds, resolving
// each block in the vertex stage and handing the fragment stage the forward
// shader's flat varyings. A growth that failed to rebind reads stale here.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests": the
// sampling is WGSL through real bind groups, so verifying it means running it.

use postretro_render_cpu::frame_uniforms::SPEC_SHADOWMASK_FLAG_MASK_PRESENT;

use super::pool_sample_harness::{
    PROBE_TYPES_WGSL, Probe, ProbeResult, ProbeTargets, helpers_wgsl, probe_out_wgsl,
};
use super::{LightmapResources, bind_group_layout, block_table_bind_group_layout};
use crate::render::gpu_test_harness::GpuCtx;

/// Group 0 is group 4 and group 1 is group 6, by binding number. The varyings
/// sit at forward.wgsl's locations and interpolation.
const BINDINGS_AND_ENTRIES: &str = r#"
@group(0) @binding(0) var lightmap_irradiance: texture_2d_array<f32>;
@group(0) @binding(1) var lightmap_direction: texture_2d_array<f32>;
@group(0) @binding(2) var lightmap_sampler: sampler;
@group(0) @binding(4) var lightmap_filtering_sampler: sampler;
@group(0) @binding(6) var shadowmask_atlas: texture_2d_array<f32>;
@group(1) @binding(0) var<storage, read> lightmap_block_table: array<vec4<u32>>;
@group(2) @binding(0) var<storage, read> probes: array<Probe>;
@group(2) @binding(1) var<uniform> uniforms: HarnessUniforms;

struct Varyings {
    @builtin(position) clip_position: vec4<f32>,
    @location(5) lightmap_texel: vec2<f32>,
    @location(6) @interpolate(flat) lightmap_layer_flags: u32,
    @location(7) @interpolate(flat) probe_index: u32,
    @location(8) @interpolate(flat) lightmap_rect: vec4<u32>,
    @location(10) @interpolate(flat) lightmap_plane_addresses: vec2<u32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) index: u32,
    @builtin(instance_index) instance: u32,
) -> Varyings {
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    let probe = probes[instance];
    let block = resolve_lightmap_block(probe.block, probe.uv);
    var out: Varyings;
    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    out.lightmap_texel = block.texel;
    out.lightmap_layer_flags = block.layer_flags;
    out.probe_index = instance;
    out.lightmap_rect = block.rect;
    out.lightmap_plane_addresses = block.plane_addresses;
    return out;
}

@fragment
fn fs_main(in: Varyings) -> HarnessOut {
    return probe_out(
        probes[in.probe_index],
        u32(in.clip_position.x),
        in.lightmap_texel,
        in.lightmap_layer_flags,
        in.lightmap_rect,
        in.lightmap_plane_addresses,
    );
}
"#;

/// One probe per instance, each scissored to its own pixels, through
/// `resources`' bind groups as they stand.
pub(super) fn draw_probes(
    ctx: &GpuCtx,
    resources: &LightmapResources,
    probes: &[Probe],
) -> Vec<ProbeResult> {
    use wgpu::util::DeviceExt;

    let device = &ctx.device;
    let source = format!(
        "{PROBE_TYPES_WGSL}\n{BINDINGS_AND_ENTRIES}\n{}\n{}",
        helpers_wgsl(),
        probe_out_wgsl()
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("bound_draw shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let probe_bytes: Vec<u8> = probes.iter().flat_map(|probe| probe.bytes()).collect();
    let probe_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("bound_draw probes"),
        contents: &probe_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let mut uniform_bytes = [0u8; 16];
    if resources.shadowmask_present {
        uniform_bytes[..4].copy_from_slice(&SPEC_SHADOWMASK_FLAG_MASK_PRESENT.to_ne_bytes());
    }
    let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("bound_draw uniforms"),
        contents: &uniform_bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let entry = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let probe_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bound_draw probe layout"),
        entries: &[
            entry(0, wgpu::BufferBindingType::Storage { read_only: true }),
            entry(1, wgpu::BufferBindingType::Uniform),
        ],
    });
    let probe_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bound_draw probe group"),
        layout: &probe_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: probe_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uniforms.as_entire_binding(),
            },
        ],
    });
    let group4 = bind_group_layout(device);
    let group6 = block_table_bind_group_layout(device);
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("bound_draw pipeline layout"),
        bind_group_layouts: &[Some(&group4), Some(&group6), Some(&probe_layout)],
        immediate_size: 0,
    });
    let target = ProbeTargets::color_target();
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("bound_draw pipeline"),
        layout: Some(&layout),
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
            targets: &[target.clone(), target],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    let targets = ProbeTargets::new(ctx, probes.len());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = targets.begin_pass(&mut encoder);
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &resources.bind_group, &[]);
        pass.set_bind_group(1, &resources.block_table_bind_group, &[]);
        pass.set_bind_group(2, &probe_group, &[]);
        for index in 0..probes.len() as u32 {
            let (x, width) = ProbeTargets::probe_pixels(index);
            pass.set_scissor_rect(x, 0, width, 1);
            pass.draw(0..3, index..index + 1);
        }
    }
    targets.read(ctx, encoder)
}
