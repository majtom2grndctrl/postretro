// Captures the Metal Shading Language wgpu generates for the real mover and
// world forward pipelines, as input to the register screen in
// `tools/metal_register_screen/`.
// See: context/lib/rendering_pipeline.md §7.3 (the mover register cliff)

use postretro_render_cpu::frame_uniforms::SpecularStyle;
use postretro_test_log_capture::LogCapture;

use super::glossy_floor::{ColourOutputs, highlight_free_format};

/// The capture directory from `$POSTRETRO_REGISTER_SCREEN_OUT`.
fn capture_dir() -> String {
    std::env::var("POSTRETRO_REGISTER_SCREEN_OUT")
        .expect("set POSTRETRO_REGISTER_SCREEN_OUT to the capture directory")
}

/// A Metal device at `limits`, with `RG11B10UFLOAT_RENDERABLE` when the
/// adapter has it, as the engine requests it: the highlight-free colour's
/// format is then the one the engine draws.
fn capture_device(limits: wgpu::Limits) -> super::gpu_test_harness::GpuCtx {
    super::gpu_test_harness::try_init_gpu_with(
        wgpu::Features::RG11B10UFLOAT_RENDERABLE,
        limits.clone(),
    )
    .or_else(|| super::gpu_test_harness::try_init_gpu_with(wgpu::Features::empty(), limits))
    .expect("the register screen needs a GPU adapter")
}

/// Write each MSL source wgpu-hal logged to `<out>/<entry>.metal`; returns
/// the entries written.
fn write_captured_msl(capture: &LogCapture, out: &str) -> Vec<String> {
    let mut written = Vec::new();
    for record in capture.records() {
        let Some(rest) = record
            .message
            .strip_prefix("Naga generated shader for entry point '")
        else {
            continue;
        };
        let entry = rest.split('\'').next().expect("entry point name");
        let body = rest
            .split_once('\n')
            .map(|(_, body)| body)
            .expect("MSL follows the header line");
        std::fs::write(format!("{out}/{entry}.metal"), body).expect("write MSL");
        written.push(entry.to_owned());
    }
    written
}

/// The Metal pixel format name `screen.swift` reads for `format`.
fn metal_format(format: wgpu::TextureFormat) -> &'static str {
    match format {
        wgpu::TextureFormat::Rgba16Float => "rgba16Float",
        wgpu::TextureFormat::Rg11b10Ufloat => "rg11b10Float",
        other => panic!("no Metal name for {other:?}"),
    }
}

/// Write `<out>/color_formats`: one line per fragment entry, its colour
/// attachments' formats in attachment order, as its pipeline declares them.
fn write_color_formats(out: &str, entries: &[(&str, Vec<wgpu::TextureFormat>)]) {
    let lines: String = entries
        .iter()
        .map(|(entry, formats)| {
            let names: Vec<_> = formats.iter().copied().map(metal_format).collect();
            format!("{entry} {}\n", names.join(" "))
        })
        .collect();
    std::fs::write(format!("{out}/color_formats"), lines).expect("write color formats");
}

/// `$POSTRETRO_REGISTER_SCREEN_MAIN`, when set: a replacement for the
/// screened shader's own file (an older `kinematic_brush.wgsl` or
/// `forward.wgsl` from `git show <rev>:<path>`), composed with today's shared
/// snippets, so one run screens the forms before a change.
fn replacement_main() -> Option<String> {
    let path = std::env::var("POSTRETRO_REGISTER_SCREEN_MAIN").ok()?;
    Some(std::fs::read_to_string(&path).expect("read the replacement shader file"))
}

/// The fragment entries `source` declares.
fn fragment_entries(source: &str) -> Vec<String> {
    naga::front::wgsl::parse_str(source)
        .expect("the screened source parses")
        .entry_points
        .iter()
        .filter(|entry| entry.stage == naga::ShaderStage::Fragment)
        .map(|entry| entry.name.clone())
        .collect()
}

/// Assert every entry in `expected` was captured.
fn assert_captured(written: &[String], expected: &[&str], what: &str) {
    for entry in expected {
        assert!(
            written.iter().any(|e| e == entry),
            "captured {written:?}; wgpu-hal must log the {what}'s {entry} (Metal only)"
        );
    }
}

/// Builds `KinematicBrushPass::new` on a Metal device and writes the MSL
/// wgpu-hal logs for each entry point, every beauty fragment entry included
/// (one per specular style, with and without the highlight-free colour), to
/// `$POSTRETRO_REGISTER_SCREEN_OUT/<entry>.metal`, and each entry's colour
/// attachments to `color_formats`. It drives the production constructor, so
/// the MSL is exactly what the engine hands the Metal compiler. A
/// measurement, not a check: run it through
/// `tools/metal_register_screen/screen_mover.sh`.
#[test]
#[ignore = "register-screen capture; needs a Metal adapter and POSTRETRO_REGISTER_SCREEN_OUT"]
fn kinematic_pipeline_msl_capture_for_the_register_screen() {
    let out = capture_dir();
    // The engine's requested limits for the mover pipeline's bindings.
    let ctx = capture_device(wgpu::Limits {
        max_bind_groups: 6,
        max_storage_buffers_per_shader_stage: 8,
        max_sampled_textures_per_shader_stage: 16,
        max_uniform_buffer_binding_size: 65536,
        ..wgpu::Limits::default()
    });
    let device = &ctx.device;
    // wgpu-hal's Metal backend logs each generated MSL source at debug level;
    // the capture raises the max level and keeps this thread's records. An
    // older source's run keeps only the records of its own pipelines.
    let main = replacement_main();
    let capture = main.is_none().then(LogCapture::start);
    let camera = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &super::pipeline_layout::uniform_bind_group_layout_entries(),
    });
    let material = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &super::pipeline_layout::material_bind_group_layout_entries(),
    });
    let sh = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &super::sh_volume::mesh_bind_group_layout_entries(),
    });
    // Cube-array shadows on, as on the Mac target.
    let pass = super::kinematic_brush::KinematicBrushPass::new(
        device,
        super::DEPTH_FORMAT,
        &camera,
        &material,
        &sh,
        true,
    );
    let colour = super::SCENE_COLOR_FORMAT;
    let highlight_free = highlight_free_format(device.features());
    let mut forms = Vec::new();
    for style in SpecularStyle::ALL {
        forms.push((
            super::kinematic_brush::beauty_fragment_entry(style),
            vec![colour],
            style,
            ColourOutputs::Colour,
        ));
        forms.push((
            super::kinematic_brush::highlight_free_fragment_entry(style),
            vec![colour, highlight_free],
            style,
            ColourOutputs::HighlightFree,
        ));
    }
    let capture = match main {
        None => capture.expect("started before the pass"),
        Some(main) => {
            // The older source's forms it declares, over today's layout.
            let source = pass.composed_with_main(&main);
            let declared = fragment_entries(&source);
            forms.retain(|(entry, ..)| declared.iter().any(|e| e == entry));
            let capture = LogCapture::start();
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            for (_, _, style, outputs) in &forms {
                let _ = pass.beauty_pipeline_from(device, &shader, *style, *outputs);
            }
            capture
        }
    };
    let written = write_captured_msl(&capture, &out);
    let formats: Vec<_> = forms
        .into_iter()
        .map(|(entry, formats, ..)| (entry, formats))
        .collect();
    let expected: Vec<&str> = formats.iter().map(|(entry, _)| *entry).collect();
    assert_captured(
        &written,
        &[["vs_main"].as_slice(), &expected].concat(),
        "mover",
    );
    write_color_formats(&out, &formats);
}

/// Builds world forward's one-output pipeline and its static-diffuse forms
/// (D_static, and the floor layer's highlight-free colour) through the
/// production constructors over the production layout entries, with
/// cube-array shadows on, and writes each entry's MSL, `color_formats` and
/// `vertex_layout` (forward's three vertex streams). A measurement, not a
/// check: run it through `tools/metal_register_screen/screen_forward.sh`.
#[test]
#[ignore = "register-screen capture; needs a Metal adapter and POSTRETRO_REGISTER_SCREEN_OUT"]
fn forward_pipeline_msl_capture_for_the_register_screen() {
    let out = capture_dir();
    let ctx = capture_device(wgpu::Limits {
        max_bind_groups: 8,
        max_storage_buffers_per_shader_stage: 8,
        max_sampled_textures_per_shader_stage:
            super::pipeline_layout::forward_pipeline_sampled_texture_count(true),
        ..wgpu::Limits::default()
    });
    let device = &ctx.device;
    let capture = LogCapture::start();
    let layouts = super::pipeline_layout::forward_bind_group_layout_entries(true).map(|entries| {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &entries,
        })
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &layouts.iter().map(Some).collect::<Vec<_>>(),
        immediate_size: 0,
    });
    let colour = super::SCENE_COLOR_FORMAT;
    let static_diffuse = super::static_diffuse_format();
    let three = vec![
        colour,
        static_diffuse,
        highlight_free_format(device.features()),
    ];
    let mut formats = vec![
        ("fs_main", vec![colour]),
        ("fs_main_static_diffuse", vec![colour, static_diffuse]),
        ("fs_main_highlight_free", three.clone()),
        ("fs_main_static_diffuse_highlight_free", three),
    ];
    let scene_colour_target = Some(wgpu::ColorTargetState {
        format: colour,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    });
    match replacement_main() {
        None => {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl(super::SHADER_SOURCE.into()),
            });
            let _one_output = super::forward_pipeline(
                device,
                &pipeline_layout,
                &shader,
                "Textured Pipeline",
                "fs_main",
                std::slice::from_ref(&scene_colour_target),
            );
            let _forms = super::ForwardStaticDiffusePipelines::new(
                device,
                &pipeline_layout,
                &shader,
                &scene_colour_target,
            );
        }
        Some(main) => {
            // The older source's forms it declares, each with its targets.
            let own = include_str!("../shaders/forward.wgsl");
            assert_eq!(super::SHADER_SOURCE.matches(own).count(), 1);
            let source = super::SHADER_SOURCE.replace(own, &main);
            let declared = fragment_entries(&source);
            formats.retain(|(entry, _)| declared.iter().any(|e| e == entry));
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            for (entry, targets) in &formats {
                let targets: Vec<_> = targets
                    .iter()
                    .map(|&format| {
                        Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })
                    })
                    .collect();
                let _ = super::forward_pipeline(
                    device,
                    &pipeline_layout,
                    &shader,
                    entry,
                    entry,
                    &targets,
                );
            }
        }
    }
    let written = write_captured_msl(&capture, &out);
    let expected: Vec<&str> = formats.iter().map(|(entry, _)| *entry).collect();
    assert_captured(
        &written,
        &[["vs_main"].as_slice(), &expected].concat(),
        "forward",
    );
    write_color_formats(&out, &formats);

    // Forward's vertex streams at the Metal buffers wgpu gives slots 0, 1 and
    // 2 (30, 29, 28): the world vertex, the owner stream, the emissive id.
    let stride = postretro_render_data::geometry::WorldVertex::STRIDE;
    let owner_stride = super::renderer_geometry::SPEC_OWNER_STRIDE;
    let mask_at = super::renderer_geometry::SPEC_OWNER_MASK_OFFSET_AT;
    let emissive = super::emissive_volumes::EMISSIVE_VOLUME_SHADER_LOCATION;
    let vertex_layout = format!(
        "layout 30 {stride}\n\
         attribute 0 float3 0 30\n\
         attribute 1 float2 12 30\n\
         attribute 2 ushort2 20 30\n\
         attribute 3 ushort2 24 30\n\
         attribute 4 ushort2 28 30\n\
         attribute 5 ushort2 32 30\n\
         layout 29 {owner_stride}\n\
         attribute 6 ushort4 0 29\n\
         attribute 7 uint {mask_at} 29\n\
         layout 28 4\n\
         attribute {emissive} uint 0 28\n"
    );
    std::fs::write(format!("{out}/vertex_layout"), vertex_layout).expect("write layout");
}
