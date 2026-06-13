// Headless regression for the multi-batch instance-buffer clobber.
//
// Regression: `UiPass::encode` wrote every batch's instances to the instance
// buffer at offset 0 via `queue.write_buffer`, then recorded each draw — all in
// one submitted command buffer. `write_buffer` resolves on the queue timeline,
// so the last batch's data won last-wins at offset 0 and EVERY batch's draw read
// it; the live splash's panel batch rendered the logo batch's instance instead.
//
// This encodes two non-empty batches into disjoint screen regions with distinct
// colors (red left half, blue right half), reads the offscreen target back, and
// asserts each half keeps its OWN batch's color. Under the old clobber both
// halves would read the LAST batch's color (blue), so the test fails pre-fix and
// passes after each batch gets its own instance-buffer region.
//
// Built on a `pollster` headless device (the shared `gpu_test_harness`
// precedent); self-skips when no GPU adapter is present so it can never be the
// thing that fails CI.

use super::gpu_test_harness::{GpuCtx, Readback, read_texture_rgba8, try_init_gpu};
use super::{UiBatch, UiComposition, UiDrawList, UiInstance, UiPass};

/// Offscreen target size. Even width so the left/right halves split cleanly at
/// `width / 2`. 64*4 = 256 bytes/row already meets `COPY_BYTES_PER_ROW_ALIGNMENT`,
/// but the readback de-pads generically regardless.
const TARGET_W: u32 = 64;
const TARGET_H: u32 = 32;

/// Encode two solid panels (left red, right blue) as two separate batches and
/// read the offscreen target back. Both batches sample the pass's 1x1 white
/// texel, so a second texture is not needed — the colors come through as tint.
fn render_two_batches_offscreen(ctx: &GpuCtx) -> Readback {
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut pass = UiPass::new(&ctx.device, &ctx.queue, format);

    let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("multi_batch offscreen target"),
        size: wgpu::Extent3d {
            width: TARGET_W,
            height: TARGET_H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let viewport = [TARGET_W, TARGET_H];
    let half_w = (TARGET_W / 2) as f32;
    let h = TARGET_H as f32;

    // Pure-channel linear colors so the sRGB encode is exact: linear 1.0 -> 255,
    // linear 0.0 -> 0. Red left, blue right.
    let mut left = UiDrawList::new();
    left.push(UiInstance::panel(
        [0.0, 0.0, half_w, h],
        [1.0, 0.0, 0.0, 1.0],
        [0.0; 4],
    ));
    let mut right = UiDrawList::new();
    right.push(UiInstance::panel(
        [half_w, 0.0, half_w, h],
        [0.0, 0.0, 1.0, 1.0],
        [0.0; 4],
    ));

    let white = pass.white_bind_group().clone();
    let composition = UiComposition::from_batches(
        vec![
            UiBatch {
                list: &left,
                bind_group: &white,
            },
            UiBatch {
                list: &right,
                bind_group: &white,
            },
        ],
        Vec::new(),
    );

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("multi_batch encoder"),
        });
    pass.encode(
        &ctx.device,
        &ctx.queue,
        &mut encoder,
        &view,
        viewport,
        wgpu::LoadOp::Clear(wgpu::Color::BLACK),
        &composition,
    );

    read_texture_rgba8(ctx, &target, TARGET_W, TARGET_H, encoder)
}

/// Two batches drawn into disjoint halves keep their OWN colors: the left half
/// reads red (batch A) and the right half reads blue (batch B). Pre-fix, both
/// batches wrote offset 0 and every draw read the last batch's data, so both
/// halves read blue — this asserts the clobber is gone.
#[test]
fn two_batches_render_their_own_instance_data() {
    let Some(ctx) = try_init_gpu() else {
        eprintln!("[multi_batch_test] skipping: no GPU adapter available");
        return;
    };

    let rb = render_two_batches_offscreen(&ctx);

    // Sample well inside each half, away from the seam at width/2.
    let y = TARGET_H / 2;
    let left = rb.at(TARGET_W / 4, y);
    let right = rb.at(TARGET_W * 3 / 4, y);

    assert_eq!(
        left,
        [255, 0, 0, 255],
        "left half should be batch A's red; got {left:?} (clobber would make it blue)",
    );
    assert_eq!(
        right,
        [0, 0, 255, 255],
        "right half should be batch B's blue; got {right:?}",
    );
}
