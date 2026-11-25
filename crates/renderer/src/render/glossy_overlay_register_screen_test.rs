// MSL capture of the glossy overlay pipelines for the AMD register screen
// (tools/metal_register_screen/screen_glossy.sh).
// See: context/lib/rendering_pipeline.md §7.3 (Mover register cliff), §7.3a (Glossy world faces)

use super::*;
use crate::render::pipeline_layout::{
    material_bind_group_layout_entries, uniform_bind_group_layout_entries,
};

/// Captures the MSL wgpu-hal generates for the overlay's entries into
/// `$POSTRETRO_REGISTER_SCREEN_OUT`: `vs_main`, `fs_pixel` and `fs_smooth`,
/// plus `vertex_layout`, the overlay's two vertex streams for the screen's
/// pipeline descriptor. Builds over the production layout entries, shader
/// source and pipeline constructor. `$POSTRETRO_REGISTER_SCREEN_SOURCE`, when
/// set, names a WGSL file that replaces the production source (an older
/// overlay, for a before/after screen); every fragment entry it declares is
/// captured. A measurement, not a check: run it through
/// `tools/metal_register_screen/screen_glossy.sh`.
#[test]
#[ignore = "register-screen capture; needs a Metal adapter and POSTRETRO_REGISTER_SCREEN_OUT"]
fn glossy_overlay_msl_capture_for_the_register_screen() {
    let out = std::env::var("POSTRETRO_REGISTER_SCREEN_OUT")
        .expect("set POSTRETRO_REGISTER_SCREEN_OUT to the capture directory");
    let source = match std::env::var("POSTRETRO_REGISTER_SCREEN_SOURCE") {
        Ok(path) => std::fs::read_to_string(&path).expect("read the replacement source"),
        Err(_) => GLOSSY_OVERLAY_SHADER_SOURCE.to_owned(),
    };
    let fragments: Vec<String> = naga::front::wgsl::parse_str(&source)
        .expect("the screened source parses")
        .entry_points
        .iter()
        .filter(|entry| entry.stage == naga::ShaderStage::Fragment)
        .map(|entry| entry.name.clone())
        .collect();
    let ctx = crate::render::gpu_test_harness::try_init_gpu_with_limits(wgpu::Limits::default())
        .expect("the register screen needs a GPU adapter");
    let device = &ctx.device;
    let capture = postretro_test_log_capture::LogCapture::start();
    let layout = |entries: &[wgpu::BindGroupLayoutEntry]| {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries,
        })
    };
    let layouts = [
        layout(&uniform_bind_group_layout_entries()),
        layout(&material_bind_group_layout_entries()),
        layout(&glossy_overlay_probe_layout_entries()),
        layout(&glossy_overlay_inputs_layout_entries()),
    ];
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &layouts.iter().map(Some).collect::<Vec<_>>(),
        immediate_size: 0,
    });
    for entry in &fragments {
        let _ = overlay_pipeline(device, &pipeline_layout, &source, entry);
    }
    let mut written = Vec::new();
    for record in capture.records() {
        let Some(rest) = record
            .message
            .strip_prefix("Naga generated shader for entry point '")
        else {
            continue;
        };
        let (Some(entry), Some((_, body))) = (rest.split('\'').next(), rest.split_once('\n'))
        else {
            continue;
        };
        std::fs::write(format!("{out}/{entry}.metal"), body).expect("write MSL");
        written.push(entry.to_owned());
    }
    for entry in fragments.iter().map(String::as_str).chain(["vs_main"]) {
        assert!(
            written.iter().any(|e| e == entry),
            "captured {written:?}; wgpu-hal must log the overlay's {entry} (Metal only)"
        );
    }

    // The overlay's vertex streams, as the screen's MTLVertexDescriptor needs
    // them: slot 0 the world vertex at Metal buffer 30, slot 1 the probe
    // stream at 29.
    let stride = postretro_render_data::geometry::WorldVertex::STRIDE;
    let vertex_layout = format!(
        "layout 30 {stride}\n\
         attribute 0 float3 0 30\n\
         attribute 1 float2 12 30\n\
         attribute 2 ushort2 20 30\n\
         attribute 4 ushort2 24 30\n\
         layout 29 4\n\
         attribute 3 uint 0 29\n"
    );
    std::fs::write(format!("{out}/vertex_layout"), vertex_layout).expect("write layout");
    eprintln!("[RegisterScreen] glossy overlay MSL captured into {out}: {written:?}");
}
