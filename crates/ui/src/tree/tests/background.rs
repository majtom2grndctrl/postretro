// Tests: tree `background` — full-backbuffer cover-fit image drawn as paint op 0
// beneath the root, on the fresh and retained build paths.

use super::common::*;
use crate::descriptor::TreeBackground;

const KEY: &str = "dev/loading/test";

/// A bottom-anchored, offset tree whose root is a filled column holding one
/// text line, so it emits a quad and a text op. The background must ignore the
/// anchor, offset, and root size.
fn background_tree(background: Option<&str>) -> AnchoredTree {
    let Widget::VStack(mut column) = vstack(0.0, 8.0, Align::Start, vec![text("Loading", 20.0)])
    else {
        unreachable!("vstack builds a VStack");
    };
    column.fill = Some(ColorValue::Literal([0.0, 0.0, 0.0, 0.5]));
    let mut tree = anchored(Widget::VStack(column));
    tree.anchor = Anchor::Bottom;
    tree.offset = [0.0, -40.0];
    tree.background = background.map(|image| TreeBackground {
        image: image.to_string(),
    });
    tree
}

fn sized_images() -> ImageSizes {
    let mut images = ImageSizes::new();
    images.insert(KEY.to_string(), [1920.0, 1080.0]);
    images
}

fn fresh(tree: &AnchoredTree, device: [u32; 2], images: &ImageSizes) -> UiDrawData {
    let mut ui = UiTree::from_descriptor(tree, &theme());
    ui.build_draw_data(device, &mut font_system(), images, &no_slots())
}

fn retained(
    ui: &mut UiTree,
    fs: &mut cosmic_text::FontSystem,
    device: [u32; 2],
    images: &ImageSizes,
    generation: u64,
) -> UiDrawData {
    ui.build_draw_data_retained_with_image_generation(
        device,
        fs,
        images,
        generation,
        &no_slots(),
        &no_cells(),
        crate::tree::TweenClock::easing(0.0),
        crate::tree::ScrollInput::default(),
    )
}

/// The background instance when paint op 0 is the background image.
fn background_op(data: &UiDrawData) -> Option<crate::UiInstance> {
    match data.paint_order.first()? {
        UiPaintOp::Image { batch, index } if data.images[*batch].0 == KEY => {
            Some(data.images[*batch].1.instances[*index])
        }
        _ => None,
    }
}

fn assert_uv(actual: [f32; 4], expected: [f32; 4]) {
    for (a, e) in actual.iter().zip(expected) {
        assert!(approx(*a, e), "uv_rect {actual:?} != {expected:?}");
    }
}

#[test]
fn background_rect_covers_the_device_backbuffer_not_the_letterboxed_canvas() {
    // 1280x1024 letterboxes the 1280x720 canvas 152 px down from the top; a
    // canvas-sized image would leave bands above and below.
    let data = fresh(&background_tree(Some(KEY)), [1280, 1024], &sized_images());
    let background = background_op(&data).expect("background is paint op 0");
    assert_eq!(background.rect, [0.0, 0.0, 1280.0, 1024.0]);
    let uw = 1350.0 / 1920.0;
    assert_uv(background.uv_rect, [(1.0 - uw) / 2.0, 0.0, uw, 1.0]);

    // The root itself still sits inside the letterboxed canvas.
    let root_quad = data.quads.instances[0];
    assert!(
        root_quad.rect[1] >= 152.0,
        "root quad must stay on the canvas, got {:?}",
        root_quad.rect
    );

    // Pillarboxed ultrawide: the rect spans the full width too.
    let wide = fresh(&background_tree(Some(KEY)), [2560, 1080], &sized_images());
    let background = background_op(&wide).expect("background is paint op 0");
    assert_eq!(background.rect, [0.0, 0.0, 2560.0, 1080.0]);
    assert_uv(background.uv_rect, [0.0, 0.125, 1.0, 0.75]);
}

#[test]
fn background_is_paint_op_zero_ahead_of_root_content() {
    let without = fresh(&background_tree(None), [1920, 1080], &sized_images());
    let with = fresh(&background_tree(Some(KEY)), [1920, 1080], &sized_images());

    assert!(background_op(&without).is_none());
    let background = background_op(&with).expect("background is paint op 0");
    assert_eq!(background.color, [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(background.margin, [0.0; 4]);
    assert_uv(background.uv_rect, [0.0, 0.0, 1.0, 1.0]);

    // Exactly one op more, and the root's ops follow it unchanged.
    assert_eq!(with.paint_order.len(), without.paint_order.len() + 1);
    assert_eq!(with.paint_order[1..], without.paint_order[..]);
    assert!(matches!(with.paint_order[1], UiPaintOp::Quad { .. }));
}

#[test]
fn background_with_unknown_image_size_emits_no_quad_and_widgets_still_draw() {
    let data = fresh(&background_tree(Some(KEY)), [1920, 1080], &no_images());
    assert!(
        data.images.is_empty(),
        "no background batch: {:?}",
        data.images
    );
    assert!(matches!(
        data.paint_order.first(),
        Some(UiPaintOp::Quad { .. })
    ));
    assert_eq!(data.texts.len(), 1, "the tree's text still draws");
}

#[test]
fn background_survives_a_retained_settled_frame() {
    let mut ui = UiTree::from_descriptor(&background_tree(Some(KEY)), &theme());
    let mut fs = font_system();
    let images = sized_images();

    let first = retained(&mut ui, &mut fs, [1920, 1080], &images, 1);
    let rebuilds = ui.draw_rebuild_count();
    let settled = retained(&mut ui, &mut fs, [1920, 1080], &images, 1);

    assert_eq!(
        ui.draw_rebuild_count(),
        rebuilds,
        "a settled frame returns the cached list"
    );
    assert_eq!(background_op(&settled), background_op(&first));
    assert!(background_op(&settled).is_some());
}

#[test]
fn background_crop_follows_a_retained_viewport_change() {
    let mut ui = UiTree::from_descriptor(&background_tree(Some(KEY)), &theme());
    let mut fs = font_system();
    let images = sized_images();

    let native = retained(&mut ui, &mut fs, [1920, 1080], &images, 1);
    assert_uv(
        background_op(&native).expect("background").uv_rect,
        [0.0, 0.0, 1.0, 1.0],
    );
    let wide = retained(&mut ui, &mut fs, [2560, 1080], &images, 1);
    let background = background_op(&wide).expect("background");
    assert_eq!(background.rect, [0.0, 0.0, 2560.0, 1080.0]);
    assert_uv(background.uv_rect, [0.0, 0.125, 1.0, 0.75]);
}

#[test]
fn background_appears_after_an_image_size_generation_change() {
    let mut ui = UiTree::from_descriptor(&background_tree(Some(KEY)), &theme());
    let mut fs = font_system();

    let missing = retained(&mut ui, &mut fs, [1920, 1080], &no_images(), 0);
    assert!(background_op(&missing).is_none());

    let uploaded = retained(&mut ui, &mut fs, [1920, 1080], &sized_images(), 1);
    let background = background_op(&uploaded).expect("background after upload");
    assert_eq!(background.rect, [0.0, 0.0, 1920.0, 1080.0]);
}
