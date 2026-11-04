// Tests: the logical canvas spans the whole backbuffer at every aspect, so an
// anchored tree's quads, text, focus rects, and scroll viewports sit on the
// backbuffer's real corners and edges; at 16:9 the projection is exactly the
// 1280x720 reference.
// See: context/lib/ui.md §1

use super::super::draw::anchor_fractions;
use super::common::*;
use crate::UiWheelScroll;
use crate::layout::{device_scale, logical_canvas_size};
use crate::tree::{ScrollInput, TweenClock};

const ANCHORS: [Anchor; 9] = [
    Anchor::TopLeft,
    Anchor::Top,
    Anchor::TopRight,
    Anchor::Left,
    Anchor::Center,
    Anchor::Right,
    Anchor::BottomLeft,
    Anchor::Bottom,
    Anchor::BottomRight,
];

/// Wider than 16:9: height bounds the scale, the canvas widens.
const WIDE: [[u32; 2]; 2] = [[2560, 1080], [3440, 1440]];
/// Narrower than 16:9: width bounds the scale, the canvas grows taller.
const TALL: [[u32; 2]; 2] = [[1080, 1920], [1000, 1000]];
/// Exact 16:9 backbuffers.
const SIXTEEN_NINE: [[u32; 2]; 5] = [
    [1280, 720],
    [1920, 1080],
    [2560, 1440],
    [3840, 2160],
    [1600, 900],
];

/// A filled, unpadded column holding one button, anchored at `anchor`. The
/// column content-sizes to the button, so its backdrop quad and the button's
/// focus rect share the root's edges.
fn anchored_button(anchor: Anchor, offset: [f32; 2]) -> AnchoredTree {
    let Widget::VStack(mut column) = vstack(0.0, 0.0, Align::Start, vec![button("stop", "noop")])
    else {
        unreachable!("vstack builds a VStack");
    };
    column.fill = Some(ColorValue::Literal([0.0, 0.0, 0.0, 0.5]));
    let mut tree = anchored(Widget::VStack(column));
    tree.anchor = anchor;
    tree.offset = offset;
    tree
}

struct Built {
    data: UiDrawData,
    focus: FocusRectList,
    root_size: [f32; 2],
}

fn build(tree: &AnchoredTree, device: [u32; 2]) -> Built {
    let mut ui = UiTree::from_descriptor(tree, &theme());
    let data = ui.build_draw_data(device, &mut font_system(), &no_images(), &no_slots());
    let focus = ui.export_focus_rects(tree, device, &no_slots(), &no_cells());
    let size = ui.taffy.layout(ui.root).expect("root has layout").size;
    Built {
        data,
        focus,
        root_size: [size.width, size.height],
    }
}

/// `rect`'s pivot (the point at the anchor's fractions of its own size) sits on
/// the backbuffer's matching point, within the one device pixel edge snapping
/// may move it.
fn assert_on_backbuffer_anchor(rect: [f32; 4], device: [u32; 2], anchor: Anchor, what: &str) {
    let (fx, fy) = anchor_fractions(anchor);
    let pivot = [rect[0] + rect[2] * fx, rect[1] + rect[3] * fy];
    let target = [device[0] as f32 * fx, device[1] as f32 * fy];
    assert!(
        (pivot[0] - target[0]).abs() <= 1.0 && (pivot[1] - target[1]).abs() <= 1.0,
        "{what} at {anchor:?} on {device:?}: pivot {pivot:?} is not on the backbuffer's {target:?} (rect {rect:?})",
    );
}

fn assert_every_anchor_lands_on_the_backbuffer(devices: &[[u32; 2]]) {
    for &device in devices {
        for anchor in ANCHORS {
            let built = build(&anchored_button(anchor, [0.0, 0.0]), device);
            let root_quad = built.data.quads.instances[0].rect;
            assert_on_backbuffer_anchor(root_quad, device, anchor, "root quad");

            let stop = built
                .focus
                .rects
                .iter()
                .find(|r| r.id == "stop")
                .expect("button exports a focus stop");
            assert_on_backbuffer_anchor(stop.rect, device, anchor, "focus rect");

            let label = &built.data.texts[0];
            let [x, y] = label.position;
            assert!(
                x >= root_quad[0]
                    && x < root_quad[0] + root_quad[2]
                    && y >= root_quad[1]
                    && y < root_quad[1] + root_quad[3],
                "label at {anchor:?} on {device:?} sits inside its root {root_quad:?}, got {:?}",
                label.position,
            );
        }
    }
}

#[test]
fn every_anchor_lands_on_the_backbuffer_edge_on_wide_windows() {
    assert_every_anchor_lands_on_the_backbuffer(&WIDE);
}

#[test]
fn every_anchor_lands_on_the_backbuffer_edge_on_tall_windows() {
    assert_every_anchor_lands_on_the_backbuffer(&TALL);
}

#[test]
fn offset_and_text_scale_uniformly_on_a_wide_window() {
    // A top-right offset is logical px times the uniform scale, measured from
    // the real corner; text keeps its 16:9 size at the same height.
    let device = [3440, 1440];
    let scale = device_scale(device);
    let built = build(&anchored_button(Anchor::TopRight, [-20.0, 10.0]), device);
    let rect = built.data.quads.instances[0].rect;
    assert!(
        (rect[0] + rect[2] - (3440.0 - 20.0 * scale)).abs() <= 1.0,
        "right edge is 20 logical px in from the corner: {rect:?}"
    );
    assert!((rect[1] - 10.0 * scale).abs() <= 1.0, "top edge: {rect:?}");

    let same_height = build(
        &anchored_button(Anchor::TopRight, [-20.0, 10.0]),
        [2560, 1440],
    );
    assert_eq!(
        built.data.texts[0].font_size,
        same_height.data.texts[0].font_size
    );
    assert_eq!(built.root_size, same_height.root_size);
}

#[test]
fn sixteen_nine_projection_matches_the_reference_canvas_formula() {
    // At an exact 16:9 backbuffer the canvas is exactly 1280x720 from the
    // origin, so each rect equals the centered-reference-canvas projection
    // bit for bit.
    for device in SIXTEEN_NINE {
        assert_eq!(
            logical_canvas_size(device),
            [REFERENCE_WIDTH, REFERENCE_HEIGHT]
        );
        let scale = device_scale(device);
        let origin = [
            (device[0] as f32 - REFERENCE_WIDTH * scale) * 0.5,
            (device[1] as f32 - REFERENCE_HEIGHT * scale) * 0.5,
        ];
        for anchor in ANCHORS {
            let offset = [13.5, -7.25];
            let built = build(&anchored_button(anchor, offset), device);
            let (fx, fy) = anchor_fractions(anchor);
            let left = REFERENCE_WIDTH * fx + offset[0] - built.root_size[0] * fx;
            let top = REFERENCE_HEIGHT * fy + offset[1] - built.root_size[1] * fy;
            let dev_left = origin[0] + left * scale;
            let dev_top = origin[1] + top * scale;
            let x = dev_left.round();
            let y = dev_top.round();
            let expected = [
                x,
                y,
                (dev_left + built.root_size[0] * scale).round() - x,
                (dev_top + built.root_size[1] * scale).round() - y,
            ];
            assert_eq!(
                built.data.quads.instances[0].rect, expected,
                "{anchor:?} on {device:?}"
            );
        }
    }
}

#[test]
fn an_aspect_change_at_the_same_scale_relays_out_and_rebuilds() {
    // 1280x720 -> 2560x720 keeps scale 1.0 but widens the canvas, so the
    // retained gates must recompute layout and rebuild the draw list.
    let tree = anchored_button(Anchor::BottomRight, [0.0, 0.0]);
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let frame = |ui: &mut UiTree, fs: &mut cosmic_text::FontSystem, device| {
        ui.build_draw_data_retained(device, fs, &no_images(), &no_slots(), &no_cells(), 0.0)
    };
    let narrow = frame(&mut ui, &mut fs, [1280, 720]);
    assert_eq!(device_scale([2560, 720]), device_scale([1280, 720]));
    let wide = frame(&mut ui, &mut fs, [2560, 720]);
    assert_eq!(ui.recompute_count(), 2, "aspect change relays out");
    assert_eq!(
        ui.draw_rebuild_count(),
        2,
        "aspect change rebuilds the draw list"
    );
    let shift = wide.quads.instances[0].rect[0] - narrow.quads.instances[0].rect[0];
    assert_eq!(shift, 1280.0, "bottom-right root follows the real corner");
    assert_on_backbuffer_anchor(
        wide.quads.instances[0].rect,
        [2560, 720],
        Anchor::BottomRight,
        "root",
    );
}

/// A top-right capturing menu whose rows scroll inside a 100 px viewport.
fn scroll_menu() -> AnchoredTree {
    let rows: Vec<String> = (0..8)
        .map(|i| format!(r#"{{"kind":"button","id":"r{i}","label":"Row r{i}","onPress":"pick"}}"#))
        .collect();
    let json = format!(
        r#"{{"anchor":"topRight","offset":[0.0,0.0],"root":{{"kind":"vstack","gap":0.0,"padding":0.0,"align":"start","focus":"linear","scroll":{{"maxHeight":100.0}},"children":[{}]}}}}"#,
        rows.join(",")
    );
    serde_json::from_str(&json).expect("scroll fixture parses")
}

#[test]
fn scroll_viewport_clip_and_wheel_follow_the_real_corner_on_a_wide_window() {
    let device = [3440, 1440];
    let tree = scroll_menu();
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let mut frame = |ui: &mut UiTree, wheel: Option<UiWheelScroll>| {
        ui.build_draw_data_retained_with_image_generation(
            device,
            &mut fs,
            &no_images(),
            0,
            &no_slots(),
            &no_cells(),
            TweenClock::easing(0.0),
            ScrollInput {
                focused_id: None,
                wheel,
            },
        )
    };
    frame(&mut ui, None);
    let before = ui.export_focus_rects(&tree, device, &no_slots(), &no_cells());
    let clip = before.rects[0].clip.expect("rows sit in a scroll viewport");
    assert_on_backbuffer_anchor(clip, device, Anchor::TopRight, "scroll viewport");
    assert!(
        (clip[3] - 100.0 * device_scale(device)).abs() <= 1.0,
        "viewport is maxHeight logical px tall: {clip:?}"
    );

    // A wheel notch over the viewport (in device px, at the real corner)
    // scrolls it; the drawn clip span is the exported viewport.
    let over = [clip[0] + clip[2] * 0.5, clip[1] + clip[3] * 0.5];
    let scrolled = frame(
        &mut ui,
        Some(UiWheelScroll {
            position: over,
            lines: -1.0,
            pixels: 0.0,
        }),
    );
    let after = ui.export_focus_rects(&tree, device, &no_slots(), &no_cells());
    assert!(
        after.rects[0].rect[1] < before.rects[0].rect[1],
        "the wheel at the real corner scrolls the rows up"
    );
    assert!(
        scrolled.clip_spans.iter().any(|span| span.rect == clip),
        "the drawn clip is the exported viewport"
    );
}
