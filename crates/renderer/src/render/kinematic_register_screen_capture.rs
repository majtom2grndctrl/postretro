// Captures the Metal Shading Language wgpu generates for the real mover
// pipeline, as input to the register screen in `tools/metal_register_screen/`.
// See: context/lib/rendering_pipeline.md §7.3 (the mover register cliff)

/// Builds `KinematicBrushPass::new` on a Metal device and writes the MSL
/// wgpu-hal logs for each entry point, both beauty fragment entries included,
/// to `$POSTRETRO_REGISTER_SCREEN_OUT/<entry>.metal`.
/// It drives the production constructor, so the MSL is exactly what the engine
/// hands the Metal compiler. A measurement, not a check: run it through
/// `tools/metal_register_screen/screen_mover.sh`.
#[test]
#[ignore = "register-screen capture; needs a Metal adapter and POSTRETRO_REGISTER_SCREEN_OUT"]
fn kinematic_pipeline_msl_capture_for_the_register_screen() {
    let out = std::env::var("POSTRETRO_REGISTER_SCREEN_OUT")
        .expect("set POSTRETRO_REGISTER_SCREEN_OUT to the capture directory");
    // The engine's requested limits for the mover pipeline's bindings.
    let ctx = super::gpu_test_harness::try_init_gpu_with_limits(wgpu::Limits {
        max_bind_groups: 6,
        max_storage_buffers_per_shader_stage: 8,
        max_sampled_textures_per_shader_stage: 16,
        max_uniform_buffer_binding_size: 65536,
        ..wgpu::Limits::default()
    })
    .expect("the register screen needs a GPU adapter");
    let device = &ctx.device;
    // wgpu-hal's Metal backend logs each generated MSL source at debug level;
    // the capture raises the max level and keeps this thread's records.
    let capture = postretro_test_log_capture::LogCapture::start();
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
    let _pass = super::kinematic_brush::KinematicBrushPass::new(
        device,
        super::DEPTH_FORMAT,
        &camera,
        &material,
        &sh,
        true,
    );
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
    // Both beauty pipelines' fragment entries, one per specular style.
    let expected = postretro_render_cpu::frame_uniforms::SpecularStyle::ALL
        .map(super::kinematic_brush::beauty_fragment_entry);
    for entry in std::iter::once("vs_main").chain(expected) {
        assert!(
            written.iter().any(|e| e == entry),
            "captured {written:?}; wgpu-hal must log the mover's {entry} (Metal only)"
        );
    }
}
