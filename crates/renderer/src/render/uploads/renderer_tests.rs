//! Actual renderer entry-point proofs for the deferred upload contract.
//! See: context/plans/in-progress/per-frame-upload-batching/index.md

use super::UploadCounts;
use crate::render::{ClearColor, Renderer, ShSampleRegionSets};
use glam::{Mat4, Vec3};
use postretro_level_loader::ShDrainBatch;
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

const CLEAR: ClearColor = ClearColor {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};
const ALL: VisibleCells = VisibleCells::DrawAll;
fn camera() -> CameraCullVisibility<'static> {
    CameraCullVisibility {
        cells: &ALL,
        path: VisibilityPath::EmptyWorldFallback,
    }
}
fn regions() -> ShSampleRegionSets<'static> {
    ShSampleRegionSets {
        visible_cells: &ALL,
        fog_cells: &[],
        movers: &[],
    }
}
fn renderer() -> Option<Renderer> {
    match Renderer::new_offscreen(64, 64) {
        Ok(renderer) => Some(renderer),
        Err(error) if error.to_string().contains("requires a GPU adapter") => {
            eprintln!("[UploadProof] skipped: no adapter ({error:#})");
            None
        }
        Err(error) => panic!("adapter present but renderer initialization failed: {error:#}"),
    }
}
fn assert_submit(renderer: &Renderer, before: UploadCounts, submits: u64) {
    let after = renderer.queue.counts();
    assert!(
        after.writes > before.writes,
        "actual renderer writers must stage"
    );
    assert_eq!(after.direct_writes, before.direct_writes);
    assert_eq!(after.submits - before.submits, submits);
    assert_eq!(after.batches - before.batches, 1);
    assert_eq!(after.batches_first - before.batches_first, 1);
    renderer
        .queue
        .assert_empty("renderer integration frame exit");
}
fn site_writes(renderer: &Renderer, file: &str, source: &str, needle: &str) -> u64 {
    let line = source
        .lines()
        .position(|line| line.contains(needle))
        .expect("writer anchor") as u32
        + 1;
    renderer
        .queue
        .writer_counts()
        .iter()
        .filter(|writer| writer.site.file().ends_with(file) && writer.site.line() == line)
        .map(|writer| writer.writes)
        .sum()
}

/// Compute reads the real UNIFORM buffer, avoiding production COPY_SRC flags.
struct Probe {
    readback: wgpu::Buffer,
}
impl Probe {
    fn record(
        renderer: &Renderer,
        encoder: &mut wgpu::CommandEncoder,
        buffer: &wgpu::Buffer,
        source: &'static str,
        size: u64,
    ) -> Self {
        let device = &renderer.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("renderer uniform readback proof"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("renderer uniform readback proof"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniform probe output"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniform probe readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer,
                        offset: 0,
                        size: std::num::NonZeroU64::new(size),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, size);
        Self { readback }
    }
    fn read(self, renderer: &Renderer) -> Vec<u8> {
        let (send, recv) = std::sync::mpsc::channel();
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                send.send(result).unwrap();
            });
        renderer
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        recv.recv().unwrap().unwrap();
        let bytes = self.readback.slice(..).get_mapped_range().to_vec();
        self.readback.unmap();
        bytes
    }
}
fn read_frame_uniforms(renderer: &Renderer) -> Vec<u8> {
    renderer
        .queue
        .assert_empty("capture uniform proof readback");
    let before = renderer.queue.counts();
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    let probe = Probe::record(
        renderer,
        &mut encoder,
        &renderer.full().uniform_buffer,
        include_str!("frame_readback.wgsl"),
        128,
    );
    renderer
        .queue
        .submit_unbatched([encoder.finish()], "capture uniform proof readback");
    assert_eq!(renderer.queue.counts().batches, before.batches);
    probe.read(renderer)
}
fn record_window(
    renderer: &mut Renderer,
    font: &mut postretro_ui::text::FontSystem,
) -> wgpu::CommandEncoder {
    let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("headless window branch"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: renderer.surface_config.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    renderer
        .record_scene_passes(
            &mut encoder,
            Some(font),
            Some(&view),
            camera(),
            &[],
            &[],
            &[],
            regions(),
            None,
            Mat4::IDENTITY,
            &[],
            &[],
            1.0,
            CLEAR,
            true,
        )
        .unwrap();
    encoder
}

#[test]
fn mesh_light_params_stage_once_for_world_viewmodel_both_and_neither() {
    use postretro_render_cpu::mesh_instances::{MeshInstanceInput, MeshPaletteCacheKey};
    let Some(mut renderer) = renderer() else {
        return;
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
    let model = "models/pose-modifier-fixture/joint_zones.gltf";
    assert!(
        renderer
            .load_skinned_model(model, &root, &root.join("cache"))
            .is_some()
    );
    let mut font = postretro_ui::text::build_font_system();
    for (world, viewmodel) in [(true, false), (false, true), (true, true), (false, false)] {
        let draws: Vec<_> = [(false, world), (true, viewmodel)]
            .into_iter()
            .filter(|(_, enabled)| *enabled)
            .map(|(is_viewmodel, _)| MeshInstanceInput {
                model: postretro_model::ModelHandle::from(model),
                transform: Mat4::IDENTITY,
                shadow_bias_scale: 1.0,
                phase_seed: u32::from(is_viewmodel),
                palette_cache_key: MeshPaletteCacheKey::Entity(u32::from(is_viewmodel)),
                sample: postretro_model::sample_params::MeshSampleParams::rest(),
                pose_inputs: None,
                capture: None,
                resample: true,
                forward_visible: true,
                dynamic_shadow_visible: false,
                is_viewmodel,
            })
            .collect();
        renderer.set_mesh_draws(&draws);
        let source = include_str!("../mesh_pass.rs");
        let count = site_writes(
            &renderer,
            "mesh_pass.rs",
            source,
            "queue.write_buffer(&self.light_params_buffer",
        );
        let before = renderer.queue.counts();
        renderer.update_per_frame_uniforms(Mat4::IDENTITY, Vec3::ZERO, 1.0);
        renderer.update_viewmodel_view_projection(1.0, Mat4::IDENTITY);
        let encoder = record_window(&mut renderer, &mut font);
        renderer.submit_windowed_frame(encoder);
        assert_submit(&renderer, before, 1);
        assert_eq!(
            site_writes(
                &renderer,
                "mesh_pass.rs",
                source,
                "queue.write_buffer(&self.light_params_buffer"
            ) - count,
            u64::from(world || viewmodel),
            "world={world}, viewmodel={viewmodel}"
        );
    }
    eprintln!("[UploadProof] mesh light params: 4 adapter cases ran");
}

#[test]
fn options_fog_setter_reaches_the_next_frame_params_before_passes() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut fog: postretro_render_cpu::fog_volume::FogVolume = bytemuck::Zeroable::zeroed();
    fog.min = [-1.0; 3];
    fog.max_v = [1.0; 3];
    fog.density = 0.1;
    fog.half_diag = 1.0;
    fog.inv_half_ext = [1.0; 3];
    fog.tint = [1.0; 3];
    fog.saturation = 1.0;
    renderer.upload_fog_volumes(bytemuck::bytes_of(&fog), &[], 1);
    renderer.set_fog_step_size(0.375);
    let before = renderer.queue.counts();
    renderer.update_per_frame_uniforms(Mat4::IDENTITY, Vec3::ZERO, 1.0);
    let mut font = postretro_ui::text::build_font_system();
    let mut encoder = record_window(&mut renderer, &mut font);
    let probe = Probe::record(
        &renderer,
        &mut encoder,
        &renderer.full().fog.params_buffer,
        include_str!("fog_readback.wgsl"),
        112,
    );
    renderer.submit_windowed_frame(encoder);
    assert_submit(&renderer, before, 1);
    let bytes = probe.read(&renderer);
    assert_eq!(&bytes[76..80], &0.375f32.to_ne_bytes());
    assert!(
        site_writes(
            &renderer,
            "fog_pass.rs",
            include_str!("../fog_pass.rs"),
            "queue.write_buffer(&self.params_buffer"
        ) > 0
    );
    eprintln!("[UploadProof] options fog setter: 1 adapter case ran");
}

#[cfg(feature = "dev-tools")]
#[test]
fn devtools_probe_setter_between_uniforms_and_submit_stages_and_reaches_gpu() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let before = renderer.queue.counts();
    renderer.update_per_frame_uniforms(Mat4::IDENTITY, Vec3::ZERO, 1.0);
    let after_uniforms = renderer.queue.counts();
    let enabled = !renderer.probe_occlusion_enabled();
    renderer.set_probe_occlusion_enabled(enabled);
    assert_eq!(renderer.queue.counts().writes, after_uniforms.writes + 1);
    let mut font = postretro_ui::text::build_font_system();
    let mut encoder = record_window(&mut renderer, &mut font);
    let probe = Probe::record(
        &renderer,
        &mut encoder,
        renderer
            .full()
            .sh_volume_resources
            .grid_info_buffer_for_test(),
        include_str!("grid_readback.wgsl"),
        96,
    );
    renderer.submit_windowed_frame(encoder);
    assert_submit(&renderer, before, 1);
    assert_eq!(
        &probe.read(&renderer)[80..84],
        &u32::from(enabled).to_ne_bytes()
    );
    eprintln!("[UploadProof] devtools immediate probe setter: 1 adapter case ran");
}

#[test]
fn acquire_failures_flush_each_frame_and_preserve_a_real_diff_gated_setter() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut font = postretro_ui::text::build_font_system();
    for enabled in [false, true] {
        let before = renderer.queue.counts();
        renderer.update_per_frame_uniforms(Mat4::IDENTITY, Vec3::ZERO, 1.0);
        renderer.set_probe_occlusion_enabled(enabled);
        renderer.inject_acquire_failure_for_test();
        let result = renderer
            .render_frame_indirect(
                &mut font,
                camera(),
                &[],
                &[],
                &[],
                regions(),
                None,
                Mat4::IDENTITY,
                &[],
                1.0,
                CLEAR,
                true,
                ShDrainBatch::default(),
            )
            .unwrap();
        assert!(result.frame.unwrap().is_none());
        assert_submit(&renderer, before, 1);
    }
    let before = renderer.queue.counts();
    // The CPU diff gate must suppress this write; the skip already delivered it.
    renderer.set_probe_occlusion_enabled(true);
    assert_eq!(renderer.queue.counts().writes, before.writes);
    renderer.update_per_frame_uniforms(Mat4::IDENTITY, Vec3::ZERO, 2.0);
    let mut encoder = record_window(&mut renderer, &mut font);
    let probe = Probe::record(
        &renderer,
        &mut encoder,
        renderer
            .full()
            .sh_volume_resources
            .grid_info_buffer_for_test(),
        include_str!("grid_readback.wgsl"),
        96,
    );
    renderer.submit_windowed_frame(encoder);
    assert_submit(&renderer, before, 1);
    assert_eq!(&probe.read(&renderer)[80..84], &1u32.to_ne_bytes());
    eprintln!("[UploadProof] acquire failure: 2 skips and 1 drawn adapter case ran");
}

#[test]
fn capture_warmup_sample_and_png_submit_their_own_staged_writes() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    for time in [1.25, 2.5] {
        let before = renderer.queue.counts();
        let result = renderer
            .capture_measurement_frame_indirect(
                camera(),
                &[],
                &[],
                &[],
                regions(),
                None,
                Mat4::IDENTITY,
                Vec3::ZERO,
                &[],
                &[],
                time,
                CLEAR,
                false,
                ShDrainBatch::default(),
            )
            .unwrap();
        result.frame.unwrap();
        assert_submit(&renderer, before, 1);
        assert_eq!(&read_frame_uniforms(&renderer)[84..88], &time.to_ne_bytes());
    }
    let before = renderer.queue.counts();
    let result = renderer
        .capture_frame_indirect(
            camera(),
            &[],
            &[],
            &[],
            regions(),
            None,
            Mat4::IDENTITY,
            Vec3::ZERO,
            &[],
            &[],
            3.75,
            CLEAR,
            false,
            ShDrainBatch::default(),
        )
        .unwrap();
    assert_eq!(result.frame.unwrap().len(), 64 * 64 * 4);
    assert_submit(&renderer, before, 2); // Scene carries one batch; PNG readback carries none.
    assert_eq!(
        &read_frame_uniforms(&renderer)[84..88],
        &3.75f32.to_ne_bytes()
    );
    eprintln!("[UploadProof] capture warmup/sample/PNG: 3 adapter cases ran");
}

#[path = "renderer_tests/inventory.rs"]
mod inventory;
#[path = "renderer_ordering_tests.rs"]
mod ordering;

#[path = "renderer_tests/lifecycle.rs"]
mod lifecycle;

#[path = "renderer_tests/streamed_inventory.rs"]
#[cfg(feature = "dev-tools")]
mod streamed_inventory;
