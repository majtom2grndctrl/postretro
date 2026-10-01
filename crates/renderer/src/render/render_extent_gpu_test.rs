// Offscreen-renderer tests for the scene/surface extent split: every scene
// target follows the scene extent, the UI layer follows the surface, capture
// pins divisor 1. GPU tests self-skip without an adapter; source scans always run.
// See: context/lib/rendering_pipeline.md §7.8

use postretro_render_cpu::render_extent::{Extent, RenderResolutionPolicy, scene_extent};

use super::Renderer;
use super::bloom::bloom_level_dimensions_table;
use super::fog_pass::scatter_dims_for;
use super::gpu_test_harness::{GpuCtx, read_texture_rgba8};
use super::sdf_shadow::compute_half_res;

fn offscreen(width: u32, height: u32) -> Option<Renderer> {
    Renderer::new_offscreen(width, height).ok()
}

fn texture_extent(texture: &wgpu::Texture) -> Extent {
    Extent::new(texture.width(), texture.height())
}

/// Assert every scene-sized target matches `scene` and the UI layer matches
/// `surface`.
fn assert_targets(renderer: &Renderer, scene: Extent, surface: Extent, when: &str) {
    let full = renderer.full();
    assert_eq!(
        renderer.scene_extent(),
        scene,
        "{when}: committed scene extent"
    );
    assert_eq!(
        renderer.render_extents().surface,
        surface,
        "{when}: committed surface"
    );
    assert_eq!(
        (
            renderer.surface_config.width,
            renderer.surface_config.height
        ),
        (surface.width, surface.height),
        "{when}: surface configuration"
    );
    assert_eq!(
        texture_extent(full.depth_view.texture()),
        scene,
        "{when}: scene depth"
    );
    assert_eq!(
        texture_extent(full.screen_effects.scene_color_texture()),
        scene,
        "{when}: scene colour"
    );
    let bloom_expected =
        bloom_level_dimensions_table(scene.width, scene.height, renderer.bloom_render_profile());
    assert_eq!(
        full.bloom.level_sizes(),
        bloom_expected.to_vec(),
        "{when}: bloom chain"
    );
    assert_eq!(
        full.fog.scatter_dims(),
        scatter_dims_for(scene.width, scene.height, full.fog.pixel_scale),
        "{when}: fog scatter"
    );
    assert_eq!(
        full.sdf_shadow_pass.half_res(),
        compute_half_res(scene.width, scene.height),
        "{when}: SDF shadow factor"
    );
    assert_eq!(
        texture_extent(full.sdf_shadow_pass.shadow_view.texture()),
        Extent::new(
            compute_half_res(scene.width, scene.height).0,
            compute_half_res(scene.width, scene.height).1
        ),
        "{when}: SDF shadow texture"
    );
    assert_eq!(
        texture_extent(full.screen_effects.ui_layer_texture()),
        surface,
        "{when}: UI layer"
    );
}

// AC: a resize rebuilds every scene-sized target at the scene extent and the
// surface-sized targets at the surface extent, read back on a windowless
// renderer. Also bloom (P11) and fog after a resize.
#[test]
fn extent_changes_rebuild_every_scene_target_at_the_scene_extent() {
    let Some(mut renderer) = offscreen(320, 180) else {
        return;
    };
    assert_targets(
        &renderer,
        Extent::new(320, 180),
        Extent::new(320, 180),
        "built",
    );

    renderer.set_render_resolution(RenderResolutionPolicy::Fixed { divisor: 3 });
    renderer.record_surface_size(1001, 563);
    let change = renderer.commit_extents().expect("both extents changed");
    assert!(change.surface_changed && change.scene_changed);
    let surface = Extent::new(1001, 563);
    assert_targets(
        &renderer,
        scene_extent(surface, 3),
        surface,
        "resize + divisor",
    );
    assert_eq!(renderer.commit_extents(), None, "exactly one rebuild");

    // A render-resolution change with no resize (P17's renderer half).
    renderer.set_render_resolution(RenderResolutionPolicy::Fixed { divisor: 2 });
    let change = renderer.commit_extents().expect("scene changed");
    assert!(change.scene_changed && !change.surface_changed);
    assert_targets(
        &renderer,
        scene_extent(surface, 2),
        surface,
        "option change",
    );
}

// P10: a level install sets the map's fog pixel scale after an extent change.
#[test]
fn fog_pixel_scale_install_divides_the_scene_extent() {
    let Some(mut renderer) = offscreen(640, 360) else {
        return;
    };
    renderer.set_render_resolution(RenderResolutionPolicy::Fixed { divisor: 2 });
    renderer.commit_extents();
    renderer.set_fog_pixel_scale(3);
    assert_eq!(renderer.full().fog.pixel_scale, 3);
    assert_eq!(
        renderer.full().fog.scatter_dims(),
        scatter_dims_for(320, 180, 3)
    );
}

// P11: a manifest reload commit sets a new bloom profile after an extent change.
#[test]
fn bloom_profile_change_keeps_the_chain_on_the_scene_extent_without_an_extent_rebuild() {
    let Some(mut renderer) = offscreen(640, 360) else {
        return;
    };
    renderer.set_render_resolution(RenderResolutionPolicy::Fixed { divisor: 2 });
    renderer.commit_extents();
    let profile = super::BloomRenderProfile {
        resolution: super::BloomResolution::Quarter,
        pixelated: true,
    };
    renderer.set_bloom_render_profile(profile);
    assert_eq!(
        renderer.full().bloom.level_sizes(),
        bloom_level_dimensions_table(320, 180, profile).to_vec()
    );
    assert_eq!(
        renderer.commit_extents(),
        None,
        "the commit touches no extent"
    );
}

// AC: capture produces an image at its requested resolution at divisor 1,
// whatever render resolution was recorded.
#[test]
fn capture_renders_at_its_requested_resolution_regardless_of_render_resolution() {
    let Some(mut renderer) = offscreen(96, 54) else {
        return;
    };
    renderer.set_render_resolution(RenderResolutionPolicy::Fixed { divisor: 3 });
    renderer.commit_extents();
    assert_eq!(renderer.scene_extent(), Extent::new(32, 18));

    renderer.commit_native_capture_extents();
    let requested = Extent::new(96, 54);
    assert_targets(&renderer, requested, requested, "capture");
    assert_eq!(renderer.render_extents().divisor, 1);

    let mut encoder = renderer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let capture = renderer.full().screen_effects.encode_capture_tonemap(
        &renderer.device,
        &renderer.queue,
        &mut encoder,
        96,
        54,
    );
    let pixels = renderer
        .read_texture_rgba8(&capture, 96, 54, encoder)
        .expect("capture readback");
    assert_eq!(pixels.len(), 96 * 54 * 4);
}

// P13–P15: the layer always matches the surface and is cleared transparent
// every frame, including a frame with no UI after one that had it.
#[test]
fn ui_layer_is_cleared_every_frame_even_with_no_ui() {
    let Some(mut renderer) = offscreen(64, 32) else {
        return;
    };
    renderer.set_render_resolution(RenderResolutionPolicy::Fixed { divisor: 2 });
    renderer.record_surface_size(70, 30);
    renderer.commit_extents();
    let surface = renderer.render_extents().surface;
    let layer = renderer.full().screen_effects.ui_layer_texture().clone();
    assert_eq!(texture_extent(&layer), surface);

    // Stand in for last frame's UI: opaque texels everywhere.
    renderer.queue.write_texture(
        layer.as_image_copy(),
        &vec![255u8; (surface.width * surface.height * 4) as usize],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(surface.width * 4),
            rows_per_image: Some(surface.height),
        },
        layer.size(),
    );

    let mut font_system = postretro_ui::text::build_font_system();
    let mut encoder = renderer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    renderer.record_ui_layer(&mut encoder, &mut font_system);
    let ctx = GpuCtx {
        device: renderer.device.clone(),
        queue: renderer.queue.clone(),
    };
    let readback = read_texture_rgba8(&ctx, &layer, surface.width, surface.height, encoder);
    assert!(
        readback.pixels.iter().all(|&b| b == 0),
        "a frame with no UI composites no earlier UI"
    );
}

// --- Source scans: structural contracts no GPU is needed for ---

const RENDER_FRAME: &str = include_str!("renderer_render_frame.rs");
const UI_LAYER: &str = include_str!("renderer_ui_layer.rs");

fn renderer_sources() -> Vec<(std::path::PathBuf, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, String)>) {
        for entry in std::fs::read_dir(dir).expect("renderer source dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("rs" | "wgsl")
            ) {
                let text = std::fs::read_to_string(&path).expect("readable source");
                out.push((path, text));
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    walk(&root, &mut out);
    out
}

// AC: the renderer crate holds no 1440-row cap; it arrives from player
// options through the render-profile chokepoint.
#[test]
fn renderer_crate_holds_no_auto_row_cap() {
    let this_file = file!().rsplit('/').next().expect("file name");
    for (path, text) in renderer_sources() {
        if path.ends_with(this_file) {
            continue;
        }
        assert!(
            !text.contains("1440"),
            "{} names a 1440 cap; Auto's row cap belongs to player options",
            path.display()
        );
    }
}

// AC: no scene target reads the surface size. Surface-size reads are confined
// to swapchain, splash, debug UI and the extent chokepoint.
#[test]
fn only_surface_consumers_read_the_surface_configuration_size() {
    const ALLOWED: [&str; 3] = [
        "renderer_extent.rs",
        "renderer_splash.rs",
        "renderer_debug_ui.rs",
    ];
    for (path, text) in renderer_sources() {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if ALLOWED.contains(&name) || name.ends_with("_test.rs") {
            continue;
        }
        for needle in ["surface_config.width", "surface_config.height"] {
            assert!(
                !text.contains(needle),
                "{} reads `{needle}`; scene targets size from the scene extent",
                path.display()
            );
        }
    }
}

// AC: game UI records into the UI layer with its own depth target, never into
// scene colour; the resolve is the only gameplay pass writing the swapchain,
// after UI. (Debug UI records in the caller's later submission; frontend frames
// run the same windowed entry.)
#[test]
fn gameplay_ui_records_into_its_layer_before_the_sole_swapchain_resolve() {
    assert!(UI_LAYER.contains("screen_effects.ui_layer_view()"));
    assert!(UI_LAYER.contains("full.ui.encode("));
    assert!(UI_LAYER.contains("wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)"));
    assert!(
        !UI_LAYER.contains("scene_color"),
        "UI never targets scene colour"
    );
    assert!(
        !RENDER_FRAME.contains("ui.encode("),
        "UI encoding lives in its own module"
    );

    let ui = RENDER_FRAME
        .find("self.record_ui_layer(encoder, font_system)")
        .expect("windowed frame records the UI layer");
    let resolve = RENDER_FRAME
        .find("full.screen_effects.encode_resolve(")
        .expect("windowed frame resolves");
    assert!(ui < resolve, "the resolve composites this frame's UI layer");
    assert_eq!(
        RENDER_FRAME
            .matches("screen_effects.encode_resolve(")
            .count(),
        1,
        "one display resolve writes the swapchain"
    );
}

// AC: capture entries pin divisor 1 before recording any scene pass.
#[test]
fn capture_entries_commit_native_extents_before_recording() {
    let capture = include_str!("renderer_capture.rs");
    for entry in [
        "pub fn capture_measurement_frame_indirect(",
        "pub fn capture_frame_indirect(",
    ] {
        let start = capture.find(entry).expect("capture entry");
        let pin = start
            + capture[start..]
                .find("self.commit_native_capture_extents();")
                .expect("capture pins native extents");
        let record = start
            + capture[start..]
                .find("self.record_scene_passes(")
                .expect("capture records scene passes");
        assert!(pin < record, "{entry} pins before recording");
    }
}
