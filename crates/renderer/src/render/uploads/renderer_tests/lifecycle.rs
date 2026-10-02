//! Lifecycle assertions use actual Renderer methods, including no-op commits.
use super::*;

fn empty_geometry<'a>(
    bvh: &'a postretro_render_data::geometry::BvhTree,
) -> crate::render::LevelGeometry<'a> {
    crate::render::LevelGeometry {
        vertices: &[],
        indices: &[],
        bvh,
        lights: &[],
        light_influences: &[],
        sh_volume: None,
        sh_storage: crate::render::LevelGeometryShStorage::Legacy,
        lightmap: None,
        lightmap_streaming: None,
        chunk_light_list: None,
        animated_light_chunks: None,
        animated_light_weight_maps: None,
        delta_sh_volumes: None,
        direct_sh_volume: None,
        direct_sh_delta_volumes: None,
        animated_direct_sh_delta_volumes: None,
        billboard_direct_scatter_volume: None,
        animated_billboard_direct_scatter_delta_volumes: None,
        entity_shadow_lights: &[],
        shadowmask_atlas: None,
        sdf_atlas: None,
        lightmap_mode: postretro_level_loader::LightmapMode::Shadowed,
        cell_draw_index: None,
        kinematic_geometry: None,
        cells: &[],
        texture_materials: &[],
    }
}

#[test]
#[cfg(debug_assertions)]
fn pending_frame_uploads_fail_at_actual_commit_install_unload_and_splash_entries() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    for boundary in 0..7 {
        renderer.update_per_frame_uniforms(Mat4::IDENTITY, Vec3::ZERO, 1.0);
        let before = renderer.queue.counts();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match boundary {
            0 => renderer.set_presentation_templates(vec![]),
            1 => renderer.set_bloom_render_profile(renderer.bloom_render_profile()),
            2 => renderer.clear_mesh_pass_for_level_load(),
            3 => renderer.release_level_resources(),
            4 => renderer.install_textures(&[], &Default::default(), std::path::Path::new(""), &[]),
            5 => {
                let bvh = postretro_render_data::geometry::BvhTree {
                    nodes: vec![],
                    leaves: vec![],
                    root_node_index: 0,
                };
                renderer.install_level_geometry(&empty_geometry(&bvh), Default::default());
            }
            6 => {
                renderer.inject_acquire_failure_for_test();
                renderer.render_splash_frame().unwrap();
            }
            _ => unreachable!(),
        }));
        assert!(
            result.is_err(),
            "boundary {boundary} must reject pending frame bytes"
        );
        assert_eq!(
            renderer.queue.counts().submits,
            before.submits,
            "assert before any boundary submit"
        );
        assert_eq!(
            renderer.queue.counts().writes,
            before.writes,
            "assert before mutation"
        );
        renderer.queue.flush_skipped_frame();
        renderer
            .queue
            .assert_empty("lifecycle assertion test cleanup");
    }
    eprintln!("[UploadProof] actual commit/install/unload/splash entries: 7 adapter cases ran");
}

#[test]
fn boot_splash_without_full_renderer_uses_raw_write_and_no_batch() {
    use crate::render::gpu_test_harness::{read_texture_rgba8, try_init_gpu};
    use crate::render::splash_pass::BootSplashPass;
    let Some(ctx) = try_init_gpu() else {
        return;
    };
    let queue = crate::render::uploads::UploadQueue::new(&ctx.device, ctx.queue.clone(), false);
    // Construct only the boot pass: no full renderer, world, UI or glyphon exists.
    let mut splash = BootSplashPass::new(&ctx.device, wgpu::TextureFormat::Rgba8UnormSrgb);
    splash.install_logo(
        &ctx.device,
        queue.raw(),
        &postretro_ui::UiTexture {
            data: vec![255, 0, 0, 255],
            width: 1,
            height: 1,
        },
    );
    let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("boot splash headless proof"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    splash.encode(
        queue.raw(),
        &mut encoder,
        &target.create_view(&Default::default()),
        [64, 64],
    );
    queue.submit_unbatched([encoder.finish()], "boot splash proof");
    assert_eq!(
        queue.counts().writes,
        0,
        "boot pass writes directly through its raw queue"
    );
    assert_eq!(queue.counts().batches, 0);
    assert_eq!(queue.counts().submits, 1);
    assert_eq!(queue.pool_counts(), (0, 0, 0));
    let encoder = ctx.device.create_command_encoder(&Default::default());
    let pixels = read_texture_rgba8(&ctx, &target, 64, 64, encoder);
    let center = (32 * 64 + 32) * 4;
    assert_eq!(&pixels.pixels[center..center + 4], &[255, 0, 0, 255]);
    queue.assert_empty("boot splash proof exit");
    eprintln!("[UploadProof] boot pass before full renderer: 1 adapter case ran");
}
