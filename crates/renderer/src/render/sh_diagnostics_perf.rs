// Manual CPU measurements of the probe tool against an existing baked map.
// See: context/lib/testing_guide.md §3 (Resource bounds)

use super::debug_lines::DebugLineRenderer;
use super::gpu_test_harness::gpu_or_skip;
use super::sh_diagnostics::{MarkerMode, ShDiagnosticsState};
use super::sh_streaming::ShResidencyState;
use glam::Vec3;
use std::hint::black_box;
use std::time::Instant;

#[test]
#[ignore = "manual profiling of existing PRL; POSTRETRO_PROBE_PERF_MAP selects fixture, never bakes"]
fn measure_baked_map_probe_overlay_cpu() {
    let path = std::env::var("POSTRETRO_PROBE_PERF_MAP").expect("existing baked PRL path");
    let world = postretro_level_loader::load_prl(&path).expect("load existing bake");
    let manifest = world.sh_stream_manifest().expect("streamed SH map");
    let residency =
        ShResidencyState::from_manifest(manifest, &world.cells).expect("residency metadata");
    let Some(ctx) = gpu_or_skip("probe overlay CPU measurement") else {
        return;
    };
    let layout = ctx
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Probe measurement camera"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
    let mut lines =
        DebugLineRenderer::new(&ctx.device, wgpu::TextureFormat::Depth32Float, 1, &layout);
    let base = manifest.base();
    eprintln!(
        "[ProbeOverlayCpuProof] fixture={path} dense_positions={} grid={:?}",
        base.probes.len(),
        base.grid_dimensions
    );
    let center = Vec3::from(base.grid_origin)
        + Vec3::from(base.cell_size)
            * Vec3::from(base.grid_dimensions.map(|n| (n - 1) as f32))
            * 0.5;
    for (pose, camera) in [("arena", Vec3::new(21.13, 2.44, 30.48)), ("center", center)] {
        for radius in [8.0, 16.0, 30.0, 64.0] {
            for mode in [MarkerMode::Irradiance, MarkerMode::Residency] {
                let state = ShDiagnosticsState {
                    show_markers: true,
                    marker_mode: mode,
                    cell_radius: radius,
                    ..Default::default()
                };
                for _ in 0..3 {
                    lines.clear();
                    super::sh_diagnostics_residency::emit(
                        &state,
                        &residency,
                        camera,
                        &world,
                        &[],
                        &mut lines,
                    );
                }
                let repeats = 20;
                let start = Instant::now();
                for _ in 0..repeats {
                    lines.clear();
                    super::sh_diagnostics_residency::emit(
                        &state,
                        &residency,
                        camera,
                        &world,
                        &[],
                        &mut lines,
                    );
                    black_box(lines.segment_count());
                }
                let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(repeats);
                eprintln!(
                    "[ProbeOverlayCpuProof] pose={pose} mode={mode:?} radius={radius} segments={} emit_ms={ms:.3}",
                    lines.segment_count()
                );
            }
        }
    }
}
