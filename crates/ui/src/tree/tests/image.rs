// Tests: `image` sizing — natural size, one authored axis keeping the source
// aspect, and an exact authored box.

use super::common::*;

fn image(width: Option<f32>, height: Option<f32>) -> Widget {
    Widget::Image(ImageWidget {
        asset: "ui/art".to_string(),
        width,
        height,
        id: None,
        focus_neighbors: Default::default(),
        label: None,
        decorative: true,
        visible_when: None,
        role: None,
    })
}

/// Lay `root` out at the reference viewport with a 200x100 `ui/art` asset and
/// return each image quad's `[w, h]` in draw order.
fn image_extents(root: Widget) -> Vec<[f32; 2]> {
    let mut ui = UiTree::from_descriptor(&anchored(root), &theme());
    let mut images = ImageSizes::new();
    images.insert("ui/art".to_string(), [200.0, 100.0]);
    let data = ui.build_draw_data([1280, 720], &mut font_system(), &images, &no_slots());
    data.images
        .iter()
        .flat_map(|(_, batch)| batch.instances.iter())
        .map(|instance| [instance.rect[2], instance.rect[3]])
        .collect()
}

fn assert_extent(got: [f32; 2], expected: [f32; 2]) {
    assert!(
        approx(got[0], expected[0]) && approx(got[1], expected[1]),
        "expected {expected:?}, got {got:?}"
    );
}

#[test]
fn unsized_image_takes_its_natural_size() {
    assert_extent(image_extents(image(None, None))[0], [200.0, 100.0]);
}

#[test]
fn width_only_image_keeps_the_source_aspect() {
    assert_extent(image_extents(image(Some(256.0), None))[0], [256.0, 128.0]);
}

#[test]
fn height_only_image_keeps_the_source_aspect() {
    assert_extent(image_extents(image(None, Some(50.0)))[0], [100.0, 50.0]);
}

#[test]
fn both_axes_give_the_exact_box() {
    assert_extent(
        image_extents(image(Some(64.0), Some(300.0)))[0],
        [64.0, 300.0],
    );
}

#[test]
fn sized_image_keeps_its_box_inside_a_centered_stack() {
    // A centered column must not stretch the pinned or aspect-derived axis.
    let stack = vstack(
        0.0,
        0.0,
        Align::Center,
        vec![image(Some(256.0), None), image(None, None)],
    );
    let extents = image_extents(stack);
    assert_extent(extents[0], [256.0, 128.0]);
    assert_extent(extents[1], [200.0, 100.0]);
}
