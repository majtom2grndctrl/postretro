// Offscreen tests for the dev-tools egui overlay's texture-delta handling.
// GPU tests self-skip without an adapter.
// See: context/lib/rendering_pipeline.md §7.8

use super::Renderer;
use super::debug_ui::PendingTextures;

const SIDE: u32 = 64;

fn offscreen() -> Option<Renderer> {
    match Renderer::new_offscreen(SIDE, SIDE) {
        Ok(renderer) => Some(renderer),
        Err(error) if error.to_string().contains("requires a GPU adapter") => {
            eprintln!("debug UI GPU test skipped: no adapter ({error:#})");
            None
        }
        Err(error) => panic!("offscreen renderer initialization failed: {error:#}"),
    }
}

/// One real egui frame: the texture deltas it emitted and its paint jobs.
fn ui_frame(ctx: &egui::Context, text: &str) -> (egui::TexturesDelta, Vec<egui::ClippedPrimitive>) {
    let raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(SIDE as f32, SIDE as f32),
        )),
        ..Default::default()
    };
    let output = ctx.run_ui(raw_input, |ui| {
        ui.label(text);
    });
    let paint_jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
    (output.textures_delta, paint_jobs)
}

/// A stand-in for the swapchain view: same format, render-attachable.
fn target_view(renderer: &Renderer) -> wgpu::TextureView {
    renderer
        .device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("debug UI test target"),
            size: wgpu::Extent3d {
                width: SIDE,
                height: SIDE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: renderer.surface_config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

fn wait_idle(renderer: &Renderer) {
    renderer
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device poll after the overlay submit");
}

#[test]
fn first_overlay_frame_drains_the_font_atlas_delta() {
    let Some(mut renderer) = offscreen() else {
        return;
    };
    let view = target_view(&renderer);
    let ctx = egui::Context::default();
    let (mut delta, paint_jobs) = ui_frame(&ctx, "Diagnostics");
    assert!(
        delta.set.values().flatten().any(|image| image.is_whole()),
        "precondition: egui's first frame sends the font atlas as a whole set"
    );

    renderer
        .record_debug_ui(&view, &mut delta, paint_jobs, 1.0)
        .expect("overlay records");
    wait_idle(&renderer);

    // epaint debug-asserts on dropping a non-empty `TexturesDelta`.
    assert!(delta.is_empty(), "every applied delta is drained");
}

#[test]
fn deltas_from_an_unpresented_frame_apply_before_the_next_frames_partial_writes() {
    let Some(mut renderer) = offscreen() else {
        return;
    };
    let view = target_view(&renderer);
    let ctx = egui::Context::default();
    let mut pending = PendingTextures::default();

    // Frame 1 builds the UI but presents nothing: its paint jobs are dropped
    // and its whole-atlas set stays queued.
    let (first, _unpresented_jobs) = ui_frame(&ctx, "a");
    pending.push(first);

    // Frame 2 rasterizes new glyphs, which egui sends as partial atlas writes.
    let (second, paint_jobs) = ui_frame(&ctx, "Wxyz 0123456789 QJKV");
    assert!(
        second.set.values().flatten().any(|image| !image.is_whole()),
        "precondition: new glyphs arrive as partial atlas writes"
    );
    pending.push(second);

    // egui-wgpu panics on a partial write to a texture it never allocated, so
    // this only passes if frame 1's allocation applied first.
    renderer
        .record_debug_ui(&view, pending.delta_mut(), paint_jobs, 1.0)
        .expect("overlay records");
    wait_idle(&renderer);

    assert!(pending.is_empty(), "the presented frame drains the queue");
}
