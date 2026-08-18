// Headless regression for ring painter depth and opaque-quad occlusion. Rings
// use a second pipeline but must still ride the one whole-composition encode.

use super::gpu_test_harness::{GpuCtx, Readback, read_texture_rgba8, try_init_gpu};
use super::{UiComposition, UiImageRegistry, UiInstance, UiPass, UiRingInstance, tree};

const TARGET: u32 = 64;
const RING_SAMPLE: (u32, u32) = (32, 13);

fn ring_draw(color: [f32; 4]) -> tree::UiDrawData {
    let mut draw = tree::UiDrawData::default();
    draw.push_ring(UiRingInstance {
        rect: [0.0, 0.0, TARGET as f32, TARGET as f32],
        color,
        radius: 20.0,
        thickness: 8.0,
        start_angle: 0.0,
        sweep: std::f32::consts::TAU,
    });
    draw
}

fn disc_draw(color: [f32; 4]) -> tree::UiDrawData {
    let mut draw = tree::UiDrawData::default();
    draw.push_ring(UiRingInstance {
        rect: [0.0, 0.0, TARGET as f32, TARGET as f32],
        color,
        radius: 20.0,
        thickness: 20.0,
        start_angle: 0.0,
        sweep: std::f32::consts::TAU,
    });
    draw
}

fn open_arc_draw(color: [f32; 4]) -> tree::UiDrawData {
    let mut draw = tree::UiDrawData::default();
    draw.push_ring(UiRingInstance {
        rect: [0.0, 0.0, TARGET as f32, TARGET as f32],
        color,
        radius: 20.0,
        thickness: 8.0,
        start_angle: 0.0,
        sweep: std::f32::consts::FRAC_PI_2,
    });
    draw
}

fn opaque_cover_draw(color: [f32; 4]) -> tree::UiDrawData {
    let mut draw = tree::UiDrawData::default();
    draw.push_quad(UiInstance::panel(
        [0.0, 0.0, TARGET as f32, TARGET as f32],
        color,
        [0.0; 4],
    ));
    draw
}

fn render_layers(ctx: &GpuCtx, layers: &[tree::UiDrawData]) -> Readback {
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut pass = UiPass::new(&ctx.device, &ctx.queue, format);
    let mut font_system = postretro_ui::text::build_font_system();
    let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ring composition offscreen target"),
        size: wgpu::Extent3d {
            width: TARGET,
            height: TARGET,
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
    let images = UiImageRegistry::default();
    let white = pass.white_bind_group().clone();
    let composition = UiComposition::from_layer_draws(layers, &white, &images);
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ring composition encoder"),
        });
    pass.encode(
        &mut font_system,
        &ctx.device,
        &ctx.queue,
        &mut encoder,
        &view,
        [TARGET, TARGET],
        wgpu::LoadOp::Clear(wgpu::Color::BLACK),
        &composition,
    );
    read_texture_rgba8(ctx, &target, TARGET, TARGET, encoder)
}

#[test]
fn rings_follow_painter_order_and_opaque_upper_quad_occludes_them() {
    let Some(ctx) = try_init_gpu() else {
        eprintln!("[ring_composition_test] skipping: no GPU adapter available");
        return;
    };

    // A full circle reaches the bottom band; an arc beginning at 0 and sweeping
    // clockwise through 90° only reaches the up→right quadrant. This is the
    // rendered counterpart of the collector's degree→radian convention.
    let full_layers = [ring_draw([1.0, 0.0, 0.0, 1.0])];
    let full = render_layers(&ctx, &full_layers);
    assert_eq!(full.at(32, 51), [255, 0, 0, 255]);
    let arc_layers = [open_arc_draw([0.0, 0.0, 1.0, 1.0])];
    let arc = render_layers(&ctx, &arc_layers);
    assert_eq!(
        arc.at(39, 14),
        [0, 0, 255, 255],
        "0° start plus positive sweep must occupy the up→right quadrant"
    );
    assert_eq!(
        arc.at(32, 51),
        [0, 0, 0, 255],
        "open arc must not wrap into the down quadrant"
    );

    // Regression: a ring whose thickness reaches its radius is a filled disc;
    // the center must not retain the annulus inner-edge anti-aliasing.
    let disc_layers = [disc_draw([1.0, 0.0, 1.0, 1.0])];
    let disc = render_layers(&ctx, &disc_layers);
    assert_eq!(
        disc.at(32, 32),
        [255, 0, 255, 255],
        "full-thickness ring must cover its center as an opaque disc"
    );

    // Both rings are fully opaque in their solid band. The upper blue ring must
    // blend after (and visibly replace) the lower red ring at the same depth
    // sample, proving the ordered ring-batch stream is encoded in painter order.
    let ordered_layers = [
        ring_draw([1.0, 0.0, 0.0, 1.0]),
        ring_draw([0.0, 0.0, 1.0, 1.0]),
    ];
    let ordered = render_layers(&ctx, &ordered_layers);
    assert_eq!(
        ordered.at(RING_SAMPLE.0, RING_SAMPLE.1),
        [0, 0, 255, 255],
        "upper ring must paint over lower ring at its painter depth"
    );

    // The composition records quads first, but its depth mapping must make this
    // upper opaque quad reject the lower ring even though the ring pipeline then
    // records later in command order.
    let occluded_layers = [
        ring_draw([1.0, 0.0, 0.0, 1.0]),
        opaque_cover_draw([0.0, 1.0, 0.0, 1.0]),
    ];
    let occluded = render_layers(&ctx, &occluded_layers);
    assert_eq!(
        occluded.at(RING_SAMPLE.0, RING_SAMPLE.1),
        [0, 255, 0, 255],
        "upper opaque quad must occlude the lower ring through shared UI depth"
    );
}
