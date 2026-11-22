// GPU readback of `forward.wgsl`'s `area_glow_factor` against render-cpu's
// mirror: static entries, pulses, colour shifts, settled one-shots, inactive
// lights and the ceiling clamp, at several frame times.
// See: context/lib/rendering_pipeline.md §7.3 (Area glow) · testing_guide.md §3
//
// Intentional exception to testing_guide.md §3 "No GPU context in tests": the
// function is WGSL, so verifying it means running it. The production function
// and `curve_eval.wgsl` are spliced in by name; only the bindings and the
// entry point are harness code. The test self-skips without an adapter and
// says so; set `POSTRETRO_REQUIRE_GPU` to make a missing adapter fail.

use postretro_render_cpu::emissive_volume::{AREA_GLOW_STATIC, area_glow_factor};
use postretro_render_cpu::sh_compose::AnimatedLightScaleDescriptor;
use wgpu::util::DeviceExt;

use super::gpu_test_harness::{gpu_or_skip, wgsl_const_line, wgsl_function, wgsl_item};

const FORWARD_WGSL: &str = include_str!("../shaders/forward.wgsl");
const CURVE_EVAL_WGSL: &str = include_str!("../shaders/curve_eval.wgsl");

/// One harness light: its descriptor fields and curve samples.
struct Light {
    period: f32,
    phase: f32,
    base_color: [f32; 3],
    brightness: Vec<f32>,
    color: Vec<[f32; 3]>,
    is_active: bool,
}

fn source() -> String {
    format!(
        "{descriptor}\n\
         struct HarnessUniforms {{ time: f32, _pad0: f32, _pad1: f32, _pad2: f32, }};\n\
         @group(0) @binding(0) var<storage, read> anim_descriptors: array<AnimationDescriptor>;\n\
         @group(0) @binding(1) var<storage, read> anim_samples: array<f32>;\n\
         @group(0) @binding(2) var<storage, read> entries: array<vec4<f32>>;\n\
         @group(0) @binding(3) var<storage, read_write> results: array<vec4<f32>>;\n\
         @group(0) @binding(4) var<uniform> uniforms: HarnessUniforms;\n\
         {ceiling}\n\
         {curve_eval}\n\
         {factor}\n\
         @compute @workgroup_size(1)\n\
         fn main(@builtin(global_invocation_id) id: vec3<u32>) {{\n\
             results[id.x] = vec4<f32>(area_glow_factor(entries[id.x]), 0.0);\n\
         }}\n",
        descriptor = wgsl_item(FORWARD_WGSL, "struct AnimationDescriptor {"),
        ceiling = wgsl_const_line(FORWARD_WGSL, "AREA_GLOW_CEILING"),
        curve_eval = CURVE_EVAL_WGSL,
        factor = wgsl_function(FORWARD_WGSL, "area_glow_factor"),
    )
}

/// Descriptors packed as `ANIMATION_DESCRIPTOR_SIZE` records over one sample
/// buffer, as the bridge uploads them.
fn pack(lights: &[Light]) -> (Vec<u8>, Vec<f32>) {
    let mut descriptors = Vec::new();
    let mut samples = vec![0.0f32];
    for light in lights {
        let brightness_offset = samples.len() as u32;
        samples.extend(&light.brightness);
        let color_offset = samples.len() as u32;
        samples.extend(light.color.iter().flatten());
        let mut record = Vec::new();
        record.extend(light.period.to_ne_bytes());
        record.extend(light.phase.to_ne_bytes());
        record.extend(brightness_offset.to_ne_bytes());
        record.extend((light.brightness.len() as u32).to_ne_bytes());
        for channel in light.base_color {
            record.extend(channel.to_ne_bytes());
        }
        record.extend(color_offset.to_ne_bytes());
        record.extend((light.color.len() as u32).to_ne_bytes());
        record.extend(u32::from(light.is_active).to_ne_bytes());
        record.extend([0u8; 8]);
        assert_eq!(
            record.len(),
            postretro_render_cpu::sh_volume::ANIMATION_DESCRIPTOR_SIZE
        );
        descriptors.extend(record);
    }
    (descriptors, samples)
}

fn run(
    ctx: &super::gpu_test_harness::GpuCtx,
    lights: &[Light],
    entries: &[[f32; 4]],
    time: f32,
) -> Vec<[f32; 3]> {
    let device = &ctx.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("area glow harness"),
        source: wgpu::ShaderSource::Wgsl(source().into()),
    });
    let (descriptor_bytes, samples) = pack(lights);
    let storage = |label: &str, bytes: &[u8]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytes,
            usage: wgpu::BufferUsages::STORAGE,
        })
    };
    let descriptors = storage("harness descriptors", &descriptor_bytes);
    let sample_bytes: Vec<u8> = samples.iter().flat_map(|v| v.to_ne_bytes()).collect();
    let samples = storage("harness samples", &sample_bytes);
    let entry_bytes: Vec<u8> = entries
        .iter()
        .flatten()
        .flat_map(|v| v.to_ne_bytes())
        .collect();
    let entry_buffer = storage("harness entries", &entry_bytes);
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("harness time"),
        contents: &[time, 0.0, 0.0, 0.0]
            .iter()
            .flat_map(|v| v.to_ne_bytes())
            .collect::<Vec<u8>>(),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let size = (entries.len() * 16) as u64;
    let results = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("harness results"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("harness readback"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("area glow harness"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: descriptors.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: samples.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: entry_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: results.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: uniform.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(entries.len() as u32, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&results, 0, &readback, 0, size);
    ctx.queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| result.unwrap());
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("harness readback");
    let bytes = slice.get_mapped_range().expect("mapped");
    let values: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| f32::from_ne_bytes(*word))
        .collect();
    values
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&[r, g, b, _]| [r, g, b])
        .collect()
}

// Acceptance rows 49, 53, 54 and 59 (GPU half): the shader's area glow is
// the CPU mirror's, frame for frame: a static entry holds `_glow × _color`
// beside a pulsing light (O16); a colour shift and a pulse follow their
// descriptor with `light` divided out (O2, O13); settled one-shots read
// their folded radiance (O1, O17); an inactive light is dark (O3); the live
// factor stops at the ceiling.
#[test]
fn shader_area_glow_matches_the_cpu_mirror() {
    let Some(ctx) = gpu_or_skip("shader_area_glow_matches_the_cpu_mirror") else {
        return;
    };
    let lights = [
        // 0: a pulse at intensity 2.
        Light {
            period: 1.0,
            phase: 0.0,
            base_color: [2.0, 1.6, 1.2],
            brightness: vec![0.2, 1.0, 0.6],
            color: Vec::new(),
            is_active: true,
        },
        // 1: a colour shift at intensity 1.5.
        Light {
            period: 0.75,
            phase: 0.25,
            base_color: [1.5; 3],
            brightness: Vec::new(),
            color: vec![[1.0, 1.0, 1.0], [1.0, 0.6, 0.1], [0.2, 0.4, 1.0]],
            is_active: true,
        },
        // 2: a settled fade, 0.25 folded into its intensity 2.
        Light {
            period: 1.0,
            phase: 0.0,
            base_color: [0.5, 0.4, 0.3],
            brightness: Vec::new(),
            color: Vec::new(),
            is_active: true,
        },
        // 3: starts inactive.
        Light {
            period: 1.0,
            phase: 0.0,
            base_color: [1.0; 3],
            brightness: Vec::new(),
            color: Vec::new(),
            is_active: false,
        },
        // 4: a curve past 1 on a hot `_glow`.
        Light {
            period: 1.0,
            phase: 0.0,
            base_color: [1.0, 0.5, 0.0],
            brightness: vec![3.0],
            color: Vec::new(),
            is_active: true,
        },
    ];
    let entries = [
        [3.0, 0.0, 0.0, AREA_GLOW_STATIC],
        [4.0, 0.5, 0.0, 0.0],
        [6.0, 1.0 / 1.5, 0.0, 1.0],
        [4.0, 0.5, 0.0, 2.0],
        [4.0, 1.0, 0.0, 3.0],
        [64.0, 1.0, 0.0, 4.0],
        // A slot past the descriptors reads nothing.
        [4.0, 1.0, 0.0, 9.0],
    ];
    for time in [0.0, 0.13, 0.5, 0.77, 3.3] {
        let gpu = run(&ctx, &lights, &entries, time);
        for (index, entry) in entries.iter().enumerate() {
            let slot = entry[3];
            let light = (slot >= 0.0).then(|| lights.get(slot as usize)).flatten();
            let cpu = area_glow_factor(
                *entry,
                light.map(|light| AnimatedLightScaleDescriptor {
                    period: light.period,
                    phase: light.phase,
                    base_color: light.base_color,
                    brightness: &light.brightness,
                    color: &light.color,
                    is_active: light.is_active,
                }),
                time,
            );
            for channel in 0..3 {
                let (g, c) = (gpu[index][channel], cpu[channel]);
                assert!(
                    (g - c).abs() <= 1e-5 * c.abs().max(1.0),
                    "entry {index} at t {time}: gpu {:?} vs cpu {cpu:?}",
                    gpu[index]
                );
            }
        }
        assert_eq!(gpu[0], [3.0, 0.0, 0.0], "the static light holds");
        assert_eq!(gpu[4], [0.0; 3], "the inactive light is dark");
        assert_eq!(gpu[5], [64.0, 32.0, 0.0], "the ceiling");
        assert_eq!(gpu[6], [0.0; 3], "no descriptor, no glow");
        for channel in 0..3 {
            let settled = gpu[3][channel] / 4.0 / [1.0, 0.8, 0.6][channel];
            assert!(
                (settled - 0.25).abs() < 1e-6,
                "the settled fade: {:?}",
                gpu[3]
            );
        }
    }
}
