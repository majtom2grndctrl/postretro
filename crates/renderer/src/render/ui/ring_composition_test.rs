// Headless regression for ring painter depth and opaque-quad occlusion. Rings
// use a second pipeline but must still ride the one whole-composition encode.

use super::gpu_test_harness::{GpuCtx, Readback, read_texture_rgba8, try_init_gpu};
use super::{UiComposition, UiImageRegistry, UiInstance, UiPass, UiRingInstance, UiText, tree};

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

fn text_draw() -> tree::UiDrawData {
    let mut draw = tree::UiDrawData::default();
    draw.push_text(UiText::new(
        "MMMM",
        [0.0, 0.0],
        36.0,
        [255, 255, 255, 255],
        postretro_ui::text::UI_FONT_FAMILY,
    ));
    draw
}

fn text_then_ring_draw() -> tree::UiDrawData {
    let mut draw = text_draw();
    draw.push_ring(UiRingInstance {
        rect: [0.0, 0.0, TARGET as f32, TARGET as f32],
        color: [1.0, 0.0, 0.0, 0.5],
        radius: 20.0,
        thickness: 8.0,
        start_angle: 0.0,
        sweep: std::f32::consts::TAU,
    });
    draw
}

fn ring_then_translucent_quad_draw() -> tree::UiDrawData {
    let mut draw = ring_draw([0.0, 0.0, 1.0, 1.0]);
    draw.push_quad(UiInstance::panel(
        [0.0, 0.0, TARGET as f32, TARGET as f32],
        [1.0, 0.0, 0.0, 0.5],
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

    // A full circle reaches the lower solid band; an arc beginning at 0 and
    // sweeping clockwise through 90° only reaches the up→right quadrant. This
    // is the rendered counterpart of the collector's degree→radian convention.
    let full_layers = [ring_draw([1.0, 0.0, 0.0, 1.0])];
    let full = render_layers(&ctx, &full_layers);
    // Keep the exact-color sample inside the annulus's solid band: y=51 lands
    // on its intentionally anti-aliased outer edge and is therefore blended.
    assert_eq!(full.at(32, 48), [255, 0, 0, 255]);
    let arc_layers = [open_arc_draw([0.0, 0.0, 1.0, 1.0])];
    let arc = render_layers(&ctx, &arc_layers);
    // Sample well within the annulus, away from both radial AA edges and the
    // arc endpoints, while still in the clockwise up→right quadrant.
    assert_eq!(
        arc.at(40, 16),
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

    // An upper opaque quad records after the lower ring and also writes the
    // smaller painter depth, so it must fully replace the ring at the sample.
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

#[test]
fn mixed_ring_commands_follow_source_over_painter_order() {
    let Some(ctx) = try_init_gpu() else {
        eprintln!("[ring_composition_test] skipping: no GPU adapter available");
        return;
    };

    // Regression: all glyphon text used to record after every ring, so a ring
    // authored after text could not alpha-blend over it. Locate a backend-stable
    // overlap from the two isolated draws instead of pinning one AA glyph pixel.
    let text_layers = [text_draw()];
    let text = render_layers(&ctx, &text_layers);
    let ring_layers = [ring_draw([1.0, 0.0, 0.0, 1.0])];
    let ring = render_layers(&ctx, &ring_layers);
    let overlap = (0..TARGET)
        .flat_map(|y| (0..TARGET).map(move |x| (x, y)))
        .find(|&(x, y)| {
            let text_pixel = text.at(x, y);
            text_pixel[0] >= 220 && text_pixel[1] >= 220 && ring.at(x, y)[0] >= 250
        })
        .expect("test glyphs must overlap the ring's solid annulus");
    let text_then_ring_layers = [text_then_ring_draw()];
    let text_then_ring = render_layers(&ctx, &text_then_ring_layers).at(overlap.0, overlap.1);
    assert!(
        text_then_ring[0] > text_then_ring[1].saturating_add(20)
            && text_then_ring[1] < text.at(overlap.0, overlap.1)[1].saturating_sub(20),
        "later translucent red ring must tint earlier white text; got {text_then_ring:?} at {overlap:?}",
    );

    // Regression: quads used to record before every ring, so a translucent quad
    // authored after a ring was instead covered by the opaque ring.
    let ring_then_quad_layers = [ring_then_translucent_quad_draw()];
    let ring_then_quad = render_layers(&ctx, &ring_then_quad_layers);
    let pixel = ring_then_quad.at(RING_SAMPLE.0, RING_SAMPLE.1);
    assert!(
        pixel[0] > 100 && pixel[2] > 100 && pixel[2] < 250,
        "later translucent red quad must blend over earlier blue ring; got {pixel:?}",
    );
}
