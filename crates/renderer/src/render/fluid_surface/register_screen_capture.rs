// MSL capture of the fluid surface pipelines for the AMD register screen
// (tools/metal_register_screen/screen_fluid.sh).
// See: context/lib/rendering_pipeline.md §7.3 (Mover register cliff), §7.3a

use super::per_fragment_oracle::per_fragment_ratio_oracle_source;
use super::*;

/// Captures the MSL wgpu-hal generates for the fluid pipelines' entries into
/// `$POSTRETRO_REGISTER_SCREEN_OUT`: `vs_main`, `fs_main`,
/// `fs_main_reflecting`, and `fs_main_reflecting_per_fragment` (the
/// reflecting entry built from the per-fragment ratio, renamed so both screen
/// side by side), plus `vertex_layout`, the fluid vertex streams for the
/// screen's pipeline descriptor. Builds over the production layout entries,
/// shader source and pipeline constructor. A measurement, not a check: run it
/// through `tools/metal_register_screen/screen_fluid.sh`.
#[test]
#[ignore = "register-screen capture; needs a Metal adapter and POSTRETRO_REGISTER_SCREEN_OUT"]
fn fluid_pipeline_msl_capture_for_the_register_screen() {
    let out = std::env::var("POSTRETRO_REGISTER_SCREEN_OUT")
        .expect("set POSTRETRO_REGISTER_SCREEN_OUT to the capture directory");
    let ctx = crate::render::gpu_test_harness::try_init_gpu_with_limits(wgpu::Limits {
        max_bind_groups: 6,
        max_storage_buffers_per_shader_stage: 8,
        max_sampled_textures_per_shader_stage: 16,
        max_uniform_buffer_binding_size: 65536,
        ..wgpu::Limits::default()
    })
    .expect("the register screen needs a GPU adapter");
    let device = &ctx.device;
    let capture = postretro_test_log_capture::LogCapture::start();
    let layouts: Vec<wgpu::BindGroupLayout> = fluid_pipeline_bind_group_layout_entries()
        .iter()
        .map(|entries| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries,
            })
        })
        .collect();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &layouts.iter().map(Some).collect::<Vec<_>>(),
        immediate_size: 0,
    });
    let module = |source: &str| {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(source.into()),
        })
    };
    let entries = |from: usize| -> Vec<(String, String)> {
        capture
            .records()
            .iter()
            .skip(from)
            .filter_map(|record| {
                let rest = record
                    .message
                    .strip_prefix("Naga generated shader for entry point '")?;
                let entry = rest.split('\'').next()?.to_owned();
                let body = rest.split_once('\n')?.1.to_owned();
                Some((entry, body))
            })
            .collect()
    };

    let production = module(SHADER_SOURCE);
    for variant in FluidShaderVariant::ALL {
        let _ = create_fluid_pipeline(device, &layout, &production, DEPTH_FORMAT, variant);
    }
    let mut written = Vec::new();
    for (entry, body) in entries(0) {
        std::fs::write(format!("{out}/{entry}.metal"), body).expect("write MSL");
        written.push(entry);
    }
    let seen = capture.records().len();
    let before = module(&per_fragment_ratio_oracle_source());
    let _ = create_fluid_pipeline(
        device,
        &layout,
        &before,
        DEPTH_FORMAT,
        FluidShaderVariant::Reflecting,
    );
    let renamed = "fs_main_reflecting_per_fragment";
    for (entry, body) in entries(seen) {
        if entry == "fs_main_reflecting" {
            std::fs::write(
                format!("{out}/{renamed}.metal"),
                body.replace("fs_main_reflecting", renamed),
            )
            .expect("write MSL");
            written.push(renamed.to_owned());
        }
    }
    for entry in ["vs_main", "fs_main", "fs_main_reflecting", renamed] {
        assert!(
            written.iter().any(|e| e == entry),
            "captured {written:?}; wgpu-hal must log the fluid {entry} (Metal only)"
        );
    }

    // The fluid vertex streams, as the screen's MTLVertexDescriptor needs
    // them: buffer i at Metal buffer 30 - i.
    let mut vertex_layout = String::new();
    for (buffer, layout) in fluid_vertex_buffer_layouts().iter().enumerate() {
        let layout = layout.as_ref().expect("every fluid stream is bound");
        vertex_layout.push_str(&format!("layout {} {}\n", 30 - buffer, layout.array_stride));
        for attribute in layout.attributes {
            let format = match attribute.format {
                wgpu::VertexFormat::Float32x3 => "float3",
                wgpu::VertexFormat::Float32x2 => "float2",
                wgpu::VertexFormat::Uint32 => "uint",
                other => panic!("screen.swift maps no {other:?}"),
            };
            vertex_layout.push_str(&format!(
                "attribute {} {format} {} {}\n",
                attribute.shader_location,
                attribute.offset,
                30 - buffer
            ));
        }
    }
    std::fs::write(format!("{out}/vertex_layout"), vertex_layout).expect("write layout");
    eprintln!("[RegisterScreen] fluid MSL captured into {out}: {written:?}");
}
