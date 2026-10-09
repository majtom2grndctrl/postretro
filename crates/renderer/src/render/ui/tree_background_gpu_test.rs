// Headless GPU proof for `AnchoredTree.background`: the UI quad pass must sample
// the background's `uv_rect` for an image batch and fill the WHOLE backbuffer,
// including the letterbox margins outside the 1280x720 logical canvas, with a
// widget drawn on top.
//
// CPU tests in `postretro-ui` prove the draw list (rect `[0, 0, device_w,
// device_h]`, centered `uv_rect` crop, paint op 0). This drives the real
// retained tree path into a `UiComposition`, encodes it through `UiPass` against
// an offscreen target, and reads the pixels back.
//
// Fixture: a 16x9 synthetic image striped along x — 2 green columns on each outer
// edge, red across the left inner half, blue across the right inner half. At a
// 960x720 (4:3) target the cover fit shows the middle 12 of 16 columns, so the
// green outer columns must NOT appear, red must fill the left half, and blue the
// right half.
//
// Structural, not exact: sampling is linear, so the crop's outer edge and the
// red/blue seam blend with the neighbouring texel. Checks sample away from those
// seams, and the "no green" check is a dominance test.
//
// Self-skips when no GPU adapter is present (testing_guide §3).
//
// See: context/lib/testing_guide.md §3, context/lib/ui.md §1

use super::descriptor::{
    Align, AnchoredTree, CaptureMode, ColorValue, ContainerWidget, SpacingValue, TreeBackground,
    Widget,
};
use super::layout::Anchor;
use super::theme::UiTheme;
use super::tree::UiDrawData;
use super::{UiComposition, UiImageRegistry, UiPass};
use crate::render::gpu_test_harness::{GpuCtx, Readback, read_texture_rgba8_staged, try_init_gpu};
use crate::render::uploads::UploadQueue;

/// 4:3 target. The 1280x720 canvas scales by 0.75 to 960x540 and letterboxes 90 px
/// above and below, so the margin rows are the proof the background is not
/// canvas-sized.
const TARGET_W: u32 = 960;
const TARGET_H: u32 = 720;
const LETTERBOX_ROWS: u32 = 90;

const KEY: &str = "test/background-stripes";
const IMAGE_W: u32 = 16;
const IMAGE_H: u32 = 9;

/// Cover fit of a 16:9 image at 4:3 scales by `720 / 9 = 80` device px per texel.
const PX_PER_TEXEL: u32 = 80;

/// A fully opaque magenta widget: no green and distinct from the red and blue
/// stripes, so a widget pixel is unambiguous against the background.
const WIDGET_FILL: [f32; 4] = [1.0, 0.0, 1.0, 1.0];

/// Texel columns 0-1 and 14-15 green, 2-7 red, 8-13 blue.
fn stripe_image() -> postretro_ui::UiTexture {
    let mut data = Vec::with_capacity((IMAGE_W * IMAGE_H * 4) as usize);
    for _y in 0..IMAGE_H {
        for x in 0..IMAGE_W {
            let rgba: [u8; 4] = match x {
                0..=1 | 14..=15 => [0, 255, 0, 255],
                2..=7 => [255, 0, 0, 255],
                _ => [0, 0, 255, 255],
            };
            data.extend_from_slice(&rgba);
        }
    }
    postretro_ui::UiTexture {
        data,
        width: IMAGE_W,
        height: IMAGE_H,
    }
}

/// A fixed-width, filled container centered on screen: a small opaque widget drawn
/// above the background. The spacer gives it padding-driven height.
fn background_tree() -> AnchoredTree {
    let widget = Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(0.0),
        padding: SpacingValue::Literal(80.0),
        align: Align::Start,
        width: Some(240.0),
        scroll: None,
        fill: Some(ColorValue::Literal(WIDGET_FILL)),
        border: None,
        id: None,
        focus_neighbors: Default::default(),
        focus: None,
        local_state: None,
        visible_when: None,
        role: None,
        children: Vec::new(),
    });
    AnchoredTree {
        anchor: Anchor::Center,
        offset: [0.0, 0.0],
        root: widget,
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
        restore_on_return: None,
        background: Some(TreeBackground { image: KEY.into() }),
    }
}

fn layer_entry(tree: AnchoredTree) -> postretro_ui::UiTreeEntry {
    postretro_ui::UiTreeEntry {
        name: "background".into(),
        tier: postretro_ui::modal_stack::ScopeTier::Engine,
        capture_mode: tree.capture_mode,
        descriptor: tree,
        on_commit: None,
    }
}

/// The widget's device-pixel rect `[x, y, w, h]`, found as the plain quad in the
/// widget's fill color (the background rides the image list).
fn widget_rect(draw: &UiDrawData) -> [f32; 4] {
    draw.quads
        .instances
        .iter()
        .find(|quad| quad.color == WIDGET_FILL)
        .expect("the filled container emits a plain quad in its fill color")
        .rect
}

struct Frame {
    readback: Readback,
    widget: [f32; 4],
}

fn render(ctx: &GpuCtx) -> Frame {
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut pass = UiPass::new(&ctx.device, &ctx.queue, format);
    let mut font_system = postretro_ui::text::build_font_system();

    // The same registration path real uploads take (`register_ui_image`).
    let mut registry = UiImageRegistry::default();
    let (texture, bind_group) = pass.upload_image(&ctx.device, &ctx.queue, &stripe_image());
    registry.register_uploaded(KEY, texture, bind_group, [IMAGE_W, IMAGE_H]);

    let theme = UiTheme::engine_default();
    let slots = std::collections::HashMap::new();
    let cells = super::tree::CellValues::new();
    let draw = pass.layout_gameplay_tree(
        &mut font_system,
        0,
        &layer_entry(background_tree()),
        [TARGET_W, TARGET_H],
        registry.image_sizes(),
        registry.image_sizes_generation(),
        &slots,
        &cells,
        &theme,
        0,
        postretro_ui::tree::TweenClock::easing(0.0),
        postretro_ui::tree::ScrollInput::default(),
    );
    let widget = widget_rect(&draw);
    assert_eq!(
        draw.images.len(),
        1,
        "the background is the tree's only image batch"
    );

    let layers = [draw];
    let white = pass.white_bind_group().clone();
    let composition = UiComposition::from_layer_draws(&layers, &white, &registry);

    let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("tree_background offscreen target"),
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
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("tree_background encoder"),
        });
    let uploads = UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    pass.encode(
        &mut font_system,
        &ctx.device,
        &uploads,
        &mut encoder,
        &view,
        [TARGET_W, TARGET_H],
        // Black clear: any pixel the background fails to cover reads as black.
        wgpu::LoadOp::Clear(wgpu::Color::BLACK),
        &composition,
    );
    let readback = read_texture_rgba8_staged(ctx, &uploads, &target, TARGET_W, TARGET_H, encoder);
    Frame { readback, widget }
}

fn is_inside(rect: [f32; 4], x: u32, y: u32) -> bool {
    let (x, y) = (x as f32, y as f32);
    x >= rect[0] && x < rect[0] + rect[2] && y >= rect[1] && y < rect[1] + rect[3]
}

/// Strongly red / blue / green, with a margin that survives sRGB encoding and
/// linear filtering.
fn is_red(p: [u8; 4]) -> bool {
    p[0] > 200 && p[1] < 40 && p[2] < 40
}
fn is_blue(p: [u8; 4]) -> bool {
    p[2] > 200 && p[0] < 40 && p[1] < 40
}
fn is_green_dominant(p: [u8; 4]) -> bool {
    p[1] > p[0].saturating_add(32) && p[1] > p[2].saturating_add(32)
}
fn is_clear(p: [u8; 4]) -> bool {
    p[0].max(p[1]).max(p[2]) < 100
}

#[test]
fn tree_background_fills_backbuffer_with_centered_cover_crop_beneath_widgets() {
    let Some(ctx) = try_init_gpu() else {
        eprintln!("[tree_background_gpu_test] skipping: no GPU adapter available");
        return;
    };
    let Frame {
        readback: rb,
        widget,
    } = render(&ctx);
    assert_eq!((rb.width, rb.height), (TARGET_W, TARGET_H));

    // The widget must sit inside the canvas and well clear of the image seams, so
    // the background-only samples below can be taken outside it.
    assert!(
        widget[2] > 100.0 && widget[3] > 100.0,
        "widget rect {widget:?} is degenerate"
    );

    // Whole frame: nothing is clear black, and nothing outside the widget is
    // green-dominant. Pixels inside the widget are checked separately.
    let mut clear = Vec::new();
    let mut green = Vec::new();
    for y in 0..TARGET_H {
        for x in 0..TARGET_W {
            let p = rb.at(x, y);
            if is_clear(p) {
                clear.push((x, y, p));
            }
            if !is_inside(widget, x, y) && is_green_dominant(p) {
                green.push((x, y, p));
            }
        }
    }
    assert!(
        clear.is_empty(),
        "{} pixels left at the clear color (first: {:?}); the background does not \
         cover the backbuffer",
        clear.len(),
        clear.first()
    );
    assert!(
        green.is_empty(),
        "{} green-dominant pixels (first: {:?}); the outer image columns are visible, \
         so the cover crop is wrong or off-center",
        green.len(),
        green.first()
    );

    // Corners and the letterbox margins (rows outside the scaled canvas) are image
    // colored: left side leans red, right side leans blue.
    let last_x = TARGET_W - 1;
    let last_y = TARGET_H - 1;
    for (x, y) in [
        (0, 0),
        (0, last_y),
        (0, LETTERBOX_ROWS / 2),
        (0, last_y - LETTERBOX_ROWS / 2),
    ] {
        let p = rb.at(x, y);
        assert!(
            p[0] > 150 && p[0] >= p[1] && p[0] > p[2],
            "left edge sample ({x}, {y}) = {p:?} is not red-led image color"
        );
    }
    for (x, y) in [
        (last_x, 0),
        (last_x, last_y),
        (last_x, LETTERBOX_ROWS / 2),
        (last_x, last_y - LETTERBOX_ROWS / 2),
    ] {
        let p = rb.at(x, y);
        assert!(
            p[2] > 150 && p[2] >= p[1] && p[2] > p[0],
            "right edge sample ({x}, {y}) = {p:?} is not blue-led image color"
        );
    }

    // Away from the crop edge and the red/blue seam (both blend across half a
    // texel, 40 px), the stripes read as their pure colors, in the letterbox
    // margins as well as the canvas band.
    let half_texel = PX_PER_TEXEL / 2;
    let seam = TARGET_W / 2;
    let rows = [
        LETTERBOX_ROWS / 2,
        TARGET_H / 2 - 130,
        last_y - LETTERBOX_ROWS / 2,
    ];
    for y in rows {
        for x in [half_texel + 20, 200, seam - half_texel - 20] {
            let p = rb.at(x, y);
            assert!(is_red(p), "left sample ({x}, {y}) = {p:?} is not red");
        }
        for x in [seam + half_texel + 20, 760, last_x - half_texel - 20] {
            let p = rb.at(x, y);
            assert!(is_blue(p), "right sample ({x}, {y}) = {p:?} is not blue");
        }
    }

    // The widget draws over the background. Sample its interior at its center and
    // near its corners, which straddle the red/blue seam in the background.
    let [wx, wy, ww, wh] = widget;
    for (fx, fy) in [(0.5, 0.5), (0.1, 0.1), (0.9, 0.1), (0.1, 0.9), (0.9, 0.9)] {
        let x = (wx + ww * fx) as u32;
        let y = (wy + wh * fy) as u32;
        let p = rb.at(x, y);
        assert!(
            p[0] > 245 && p[1] < 10 && p[2] > 245,
            "widget sample ({x}, {y}) = {p:?} is not the opaque widget fill"
        );
    }
    // Just outside the widget the background is back.
    let left_of_widget = rb.at(wx as u32 - 4, (wy + wh * 0.5) as u32);
    assert!(
        is_red(left_of_widget),
        "pixel left of the widget = {left_of_widget:?} is not background red"
    );
    let right_of_widget = rb.at((wx + ww) as u32 + 4, (wy + wh * 0.5) as u32);
    assert!(
        is_blue(right_of_widget),
        "pixel right of the widget = {right_of_widget:?} is not background blue"
    );
}
