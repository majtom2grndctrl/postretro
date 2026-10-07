// Scroll containers (`scroll: { maxHeight }`): sizing, clipping, the wheel,
// scroll-into-view, clamping on shrink, and the clipped focus export.
// See: context/lib/ui.md §4

use super::common::*;
use crate::UiWheelScroll;
use crate::tree::{ScrollInput, TweenClock, WHEEL_LINE_SCROLL, intersect_rects};
use log::Level;
use postretro_test_log_capture::LogCapture;

const VIEWPORT: [u32; 2] = [1280, 720];

fn button(id: &str) -> String {
    format!(r#"{{"kind":"button","id":"{id}","label":"Row {id}","onPress":"pick"}}"#)
}

/// `count` buttons `r0..r{count}`, each optionally gated on `menu.more` from
/// `hidden_from` on, inside a scroll VStack of `max_height`.
fn scroll_list(max_height: f32, count: usize, hidden_from: Option<usize>, focus: bool) -> String {
    let rows: Vec<String> = (0..count)
        .map(|i| match hidden_from {
            Some(from) if i >= from => format!(
                r#"{{"kind":"button","id":"r{i}","label":"Row r{i}","onPress":"pick","visibleWhen":{{"slot":"menu.more","equals":true}}}}"#
            ),
            _ => button(&format!("r{i}")),
        })
        .collect();
    let focus = if focus { r#""focus":"linear","# } else { "" };
    format!(
        r#"{{"kind":"vstack","gap":0.0,"padding":0.0,"align":"start","scroll":{{"maxHeight":{max_height}}},{focus}"children":[{}]}}"#,
        rows.join(",")
    )
}

/// A top-left tree: a linear focus group holding `body` and a trailing button.
fn menu(body: &str) -> AnchoredTree {
    let json = format!(
        r#"{{"anchor":"topLeft","offset":[0.0,0.0],"root":{{"kind":"vstack","gap":0.0,"padding":0.0,"align":"start","focus":"linear","children":[{body},{}]}}}}"#,
        button("after")
    );
    serde_json::from_str(&json).expect("scroll fixture parses")
}

struct Harness {
    tree: AnchoredTree,
    ui: UiTree,
    fs: cosmic_text::FontSystem,
    slots: HashMap<String, SlotValue>,
}

impl Harness {
    fn new(tree: AnchoredTree) -> Self {
        let ui = UiTree::from_descriptor(&tree, &theme());
        let mut slots = HashMap::new();
        slots.insert("menu.more".to_string(), SlotValue::Boolean(true));
        Self {
            tree,
            ui,
            fs: font_system(),
            slots,
        }
    }

    fn frame(&mut self, input: ScrollInput<'_>) -> UiDrawData {
        self.ui.build_draw_data_retained_with_image_generation(
            VIEWPORT,
            &mut self.fs,
            &no_images(),
            0,
            &self.slots,
            &no_cells(),
            TweenClock::easing(0.0),
            input,
        )
    }

    fn focused(&mut self, id: &str) -> UiDrawData {
        self.frame(ScrollInput {
            focused_id: Some(id),
            wheel: None,
        })
    }

    fn wheel(&mut self, position: [f32; 2], lines: f32) -> UiDrawData {
        self.frame(ScrollInput {
            focused_id: None,
            wheel: Some(UiWheelScroll {
                position,
                lines,
                pixels: 0.0,
            }),
        })
    }

    /// Replace the tree in place, as the renderer does when the same layer's
    /// descriptor changes: a fresh build that carries the old scroll state.
    fn rebuild(&mut self, tree: AnchoredTree) {
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        ui.carry_scroll_from(&self.ui);
        self.ui = ui;
        self.tree = tree;
    }

    fn rects(&self) -> FocusRectList {
        self.ui
            .export_focus_rects(&self.tree, VIEWPORT, &self.slots, &no_cells())
    }
}

fn rect<'a>(list: &'a FocusRectList, id: &str) -> &'a FocusRect {
    list.rects
        .iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("stop {id} exported"))
}

fn bottom(r: [f32; 4]) -> f32 {
    r[1] + r[3]
}

/// The visible part of a stop: its rect within its scroll clip.
fn visible(stop: &FocusRect) -> [f32; 4] {
    stop.clip
        .map_or(stop.rect, |clip| intersect_rects(stop.rect, clip))
}

// MC15: content that fits sizes the container to it and never scrolls.
#[test]
fn a_scroll_container_whose_content_fits_sizes_to_its_content_and_does_not_scroll() {
    let mut h = Harness::new(menu(&scroll_list(400.0, 3, None, false)));
    h.frame(ScrollInput::default());
    let before = h.rects();
    let viewport = rect(&before, "r0")
        .clip
        .expect("rows sit in a scroll viewport");
    assert!(
        approx(viewport[1], rect(&before, "r0").rect[1])
            && approx(bottom(viewport), bottom(rect(&before, "r2").rect)),
        "the viewport is exactly the content: {viewport:?}"
    );
    assert!(viewport[3] < 400.0);
    assert!(
        approx(rect(&before, "after").rect[1], bottom(viewport)),
        "the next sibling follows the content, not maxHeight"
    );

    h.wheel([viewport[0] + 4.0, viewport[1] + 4.0], -3.0);
    assert_eq!(
        h.rects(),
        before,
        "a wheel over content that fits moves nothing"
    );
}

// MC15: the wheel scrolls an overflowing container under the cursor, and only
// while the cursor is over it.
#[test]
fn the_pointer_wheel_scrolls_the_overflowing_container_under_the_cursor() {
    let mut h = Harness::new(menu(&scroll_list(100.0, 10, None, false)));
    h.frame(ScrollInput::default());
    let before = h.rects();
    let viewport = rect(&before, "r0").clip.expect("clipped");
    assert!(
        approx(viewport[3], 100.0),
        "overflowing content clamps to maxHeight"
    );
    let r0 = rect(&before, "r0").rect;

    // A wheel outside the viewport scrolls nothing.
    h.wheel([viewport[0] + 4.0, bottom(viewport) + 10.0], -1.0);
    assert_eq!(rect(&h.rects(), "r0").rect, r0);

    // One notch down scrolls the content up by one wheel line.
    h.wheel([viewport[0] + 4.0, viewport[1] + 4.0], -1.0);
    let after = h.rects();
    assert!(approx(
        rect(&after, "r0").rect[1],
        r0[1] - WHEEL_LINE_SCROLL
    ));
    assert_eq!(
        rect(&after, "r0").clip,
        Some(viewport),
        "the viewport itself does not move"
    );
    assert!(
        approx(
            rect(&after, "after").rect[1],
            rect(&before, "after").rect[1]
        ),
        "scrolling never relays out the content after the container"
    );

    // Far past the end, the offset stops where the last row meets the bottom.
    h.wheel([viewport[0] + 4.0, viewport[1] + 4.0], -100.0);
    assert!(approx(
        bottom(rect(&h.rects(), "r9").rect),
        bottom(viewport)
    ));
}

// MC15: scroll opens no focus group; its stops join the enclosing group unless
// the container itself declares `focus`.
#[test]
fn scroll_children_join_the_enclosing_focus_group_unless_the_container_declares_focus() {
    let h = {
        let mut h = Harness::new(menu(&scroll_list(100.0, 4, None, false)));
        h.frame(ScrollInput::default());
        h
    };
    let list = h.rects();
    assert_eq!(list.groups.len(), 1, "scroll creates no group");
    let ids: Vec<&str> = list.groups[0]
        .members
        .iter()
        .map(|&i| list.rects[i].id.as_str())
        .collect();
    assert_eq!(ids, ["r0", "r1", "r2", "r3", "after"]);

    let mut h = Harness::new(menu(&scroll_list(100.0, 4, None, true)));
    h.frame(ScrollInput::default());
    let list = h.rects();
    assert_eq!(
        list.groups.len(),
        2,
        "a scroll container with focus opens its group"
    );
    assert_eq!(list.groups[1].parent, Some(0));
    assert_eq!(rect(&list, "r0").group, Some(1));
    assert_eq!(rect(&list, "after").group, Some(0));
}

// I5: a passive Text inside a scroll container is never a focus stop.
#[test]
fn a_text_inside_a_scroll_container_is_never_focused() {
    let body = format!(
        r#"{{"kind":"vstack","gap":0.0,"padding":0.0,"align":"start","scroll":{{"maxHeight":60.0}},"children":[{{"kind":"text","id":"caption","content":"Levels","fontSize":18.0,"color":[1.0,1.0,1.0,1.0]}},{}]}}"#,
        button("r0")
    );
    let mut h = Harness::new(menu(&body));
    h.frame(ScrollInput::default());
    let list = h.rects();
    assert!(list.rects.iter().all(|r| r.id != "caption"));
    assert_eq!(list.rects.len(), 2);
}

// MC15: an HStack ignores `scroll` and the tree's registration diagnoses it.
#[test]
fn an_hstack_scroll_is_ignored_and_diagnosed_once_at_registration() {
    let row = |scroll: &str| {
        format!(
            r#"{{"kind":"hstack","gap":0.0,"padding":0.0,"align":"start","id":"strip",{scroll}"children":[{},{}]}}"#,
            button("a"),
            button("b")
        )
    };
    let capture = LogCapture::start();
    let scrolled = menu(&row(r#""scroll":{"maxHeight":4.0},"#));
    crate::tree::warn_focus_authoring("strip", &scrolled);
    capture.assert_logged_once(Level::Warn, "HStack 'strip' authors `scroll`; ignored");

    let mut with = Harness::new(scrolled);
    with.frame(ScrollInput::default());
    let mut without = Harness::new(menu(&row("")));
    without.frame(ScrollInput::default());
    assert_eq!(with.rects().rects, without.rects().rects);
    assert!(with.rects().rects.iter().all(|r| r.clip.is_none()));
}

// MC16: one row past the bottom scrolls by exactly one row; one row above the
// top aligns that row's top with the viewport's.
#[test]
fn moving_focus_one_row_below_the_viewport_scrolls_by_one_row_and_above_aligns_tops() {
    let mut h = Harness::new(menu(&scroll_list(100.0, 10, None, false)));
    h.focused("r0");
    let start = h.rects();
    let viewport = rect(&start, "r0").clip.expect("clipped");
    let pitch = rect(&start, "r1").rect[1] - rect(&start, "r0").rect[1];
    let first_hidden = (0..10)
        .map(|i| format!("r{i}"))
        .find(|id| bottom(rect(&start, id).rect) > bottom(viewport) + EPS)
        .expect("a row starts below the viewport");
    // The rows fully inside the viewport do not move the offset.
    h.focused("r1");
    assert_eq!(h.rects(), start);

    h.focused(&first_hidden);
    let moved = h.rects();
    let focused = rect(&moved, &first_hidden).rect;
    assert!(
        approx(bottom(focused), bottom(viewport)),
        "the row's bottom meets the viewport's bottom"
    );
    let scrolled_by = rect(&start, "r0").rect[1] - rect(&moved, "r0").rect[1];
    assert!(
        scrolled_by > 0.0 && scrolled_by <= pitch + EPS,
        "moving one row scrolls at most one row ({scrolled_by} vs {pitch})"
    );

    // Scroll to the end, then focus a row above the viewport: tops align.
    h.focused("r9");
    h.focused("r2");
    let up = h.rects();
    assert!(approx(rect(&up, "r2").rect[1], viewport[1]));
}

// MC17 / P18: content shrinking while scrolled to the end draws from a clamped
// offset with no empty band; under maxHeight the container shrinks to it.
#[test]
fn a_container_scrolled_to_its_end_whose_content_shrinks_draws_with_no_empty_band() {
    let mut h = Harness::new(menu(&scroll_list(100.0, 10, Some(2), false)));
    h.focused("r9");
    let scrolled = h.rects();
    let viewport = rect(&scrolled, "r0").clip.expect("clipped");
    assert!(
        rect(&scrolled, "r0").rect[1] < viewport[1],
        "scrolled to the end"
    );

    // Shrink below the viewport: the container fits its two rows, drawn from
    // its top.
    h.slots
        .insert("menu.more".to_string(), SlotValue::Boolean(false));
    h.frame(ScrollInput {
        focused_id: Some("r1"),
        wheel: None,
    });
    let shrunk = h.rects();
    let r0 = rect(&shrunk, "r0");
    let fitted = r0.clip.expect("still a scroll viewport");
    assert!(
        approx(r0.rect[1], fitted[1]),
        "the first row draws at the top"
    );
    assert!(
        approx(bottom(rect(&shrunk, "r1").rect), bottom(fitted)),
        "the container shrinks to its content: no empty band"
    );
    assert!(fitted[3] < viewport[3]);
}

// P18: a partial shrink that still overflows clamps the offset so the last
// row meets the viewport bottom.
#[test]
fn a_partial_shrink_clamps_the_offset_so_the_last_row_meets_the_bottom() {
    let mut h = Harness::new(menu(&scroll_list(100.0, 10, Some(6), false)));
    h.focused("r9");
    h.slots
        .insert("menu.more".to_string(), SlotValue::Boolean(false));
    h.frame(ScrollInput {
        focused_id: Some("r5"),
        wheel: None,
    });
    let list = h.rects();
    let viewport = rect(&list, "r5").clip.expect("clipped");
    assert!(approx(bottom(rect(&list, "r5").rect), bottom(viewport)));
}

// MC17 / P19: a stop scrolled out of view exports a clip that excludes it, so
// a click there reaches nothing hidden; the visible stops stay hittable.
#[test]
fn a_stop_scrolled_out_of_view_exports_no_visible_area() {
    let mut h = Harness::new(menu(&scroll_list(100.0, 10, None, false)));
    h.focused("r9");
    let list = h.rects();
    let hidden = visible(rect(&list, "r0"));
    assert!(
        hidden[2] == 0.0 || hidden[3] == 0.0,
        "a fully clipped stop has no clickable area: {hidden:?}"
    );
    let shown = visible(rect(&list, "r9"));
    assert!(shown[2] > 0.0 && shown[3] > 0.0);
    // The stop after the container is not clipped at all.
    assert_eq!(rect(&list, "after").clip, None);
}

// P19: a restored focus outside the viewport (a covered tree revealed with a
// saved focus below it) scrolls into view by the minimum distance.
#[test]
fn a_restored_focus_outside_the_viewport_scrolls_into_view_by_the_minimum_distance() {
    let mut h = Harness::new(menu(&scroll_list(100.0, 10, None, false)));
    h.focused("r0");
    let viewport = rect(&h.rects(), "r0").clip.expect("clipped");
    // Covered by a modal: a lower layer receives no focus.
    h.frame(ScrollInput::default());
    // Revealed with its saved focus restored to a row far below.
    h.focused("r7");
    let list = h.rects();
    assert!(approx(bottom(rect(&list, "r7").rect), bottom(viewport)));
    assert!(
        rect(&list, "r6").rect[1] < bottom(viewport),
        "minimum distance: the row above stays (partly) in view"
    );
}

// The draw list clips the container's children to its viewport; the
// container's own backdrop and its siblings are unclipped.
#[test]
fn scroll_children_draw_clipped_to_the_viewport_and_its_siblings_do_not() {
    let mut h = Harness::new(menu(&scroll_list(100.0, 10, None, false)));
    let draw = h.focused("r0");
    let viewport = rect(&h.rects(), "r0").clip.expect("clipped");
    let row_texts = draw
        .texts
        .iter()
        .position(|t| t.content == "Row r0")
        .expect("row text drawn");
    let after_text = draw
        .texts
        .iter()
        .position(|t| t.content == "Row after")
        .expect("after text drawn");
    let op_of = |text: usize| {
        draw.paint_order
            .iter()
            .position(|op| *op == UiPaintOp::Text { index: text })
            .expect("text has a paint op")
    };
    assert_eq!(draw.paint_clip(op_of(row_texts)), Some(viewport));
    assert_eq!(draw.paint_clip(op_of(after_text)), None);
    assert_eq!(draw.clip_spans.len(), 1, "one contiguous clipped run");
}

// A settled frame with a scroll container rebuilds nothing.
#[test]
fn a_settled_frame_with_a_scroll_container_rebuilds_no_draw_list() {
    let mut h = Harness::new(menu(&scroll_list(100.0, 10, None, false)));
    h.focused("r9");
    let rebuilds = h.ui.draw_rebuild_count();
    let relayouts = h.ui.recompute_count();
    h.focused("r9");
    assert_eq!(h.ui.draw_rebuild_count(), rebuilds);
    assert_eq!(h.ui.recompute_count(), relayouts);
    // A wheel scroll redraws without relaying out.
    let viewport = rect(&h.rects(), "r9").clip.expect("clipped");
    h.wheel([viewport[0] + 2.0, viewport[1] + 2.0], 1.0);
    assert_eq!(h.ui.draw_rebuild_count(), rebuilds + 1);
    assert_eq!(h.ui.recompute_count(), relayouts);
}

// A Grid scrolls its rows the same way.
#[test]
fn a_scrolling_grid_clamps_to_max_height_and_scrolls_its_rows() {
    let cells: Vec<String> = (0..12).map(|i| button(&format!("g{i}"))).collect();
    let body = format!(
        r#"{{"kind":"grid","gap":0.0,"padding":0.0,"align":"start","cols":3,"scroll":{{"maxHeight":50.0}},"children":[{}]}}"#,
        cells.join(",")
    );
    let mut h = Harness::new(menu(&body));
    h.focused("g0");
    let list = h.rects();
    let viewport = rect(&list, "g0").clip.expect("grid rows clip");
    assert!(approx(viewport[3], 50.0));
    h.focused("g11");
    let list = h.rects();
    assert!(approx(bottom(rect(&list, "g11").rect), bottom(viewport)));
    assert!(approx(bottom(rect(&list, "g9").rect), bottom(viewport)));
}

// A rebuild of the same tree (a rebound control relabels a row) keeps the
// offset: the focused row stays where it was instead of snapping to an edge.
#[test]
fn a_rebuilt_tree_with_a_changed_leaf_keeps_its_scroll_offset() {
    let list = scroll_list(100.0, 10, None, false);
    let mut h = Harness::new(menu(&list));
    h.focused("r9");
    h.focused("r7");
    let before = h.rects();
    let viewport = rect(&before, "r7").clip.expect("clipped");
    assert!(
        approx(bottom(rect(&before, "r9").rect), bottom(viewport)),
        "scrolled to the end"
    );

    h.rebuild(menu(&list.replace("Row r3", "Rebound")));
    h.focused("r7");
    let after = h.rects();
    assert_eq!(rect(&after, "r7").rect, rect(&before, "r7").rect);
    assert!(approx(bottom(rect(&after, "r9").rect), bottom(viewport)));
}

// A carried offset past the rebuilt content's end clamps to it.
#[test]
fn a_carried_offset_clamps_to_the_rebuilt_content() {
    let mut h = Harness::new(menu(&scroll_list(100.0, 10, None, false)));
    h.frame(ScrollInput::default());
    let viewport = rect(&h.rects(), "r0").clip.expect("clipped");
    h.wheel([viewport[0] + 4.0, viewport[1] + 4.0], -100.0);

    h.rebuild(menu(&scroll_list(100.0, 6, None, false)));
    h.frame(ScrollInput::default());
    let list = h.rects();
    let viewport = rect(&list, "r0").clip.expect("still a scroll viewport");
    assert!(
        approx(bottom(rect(&list, "r5").rect), bottom(viewport)),
        "the last row meets the bottom: no empty band"
    );
    assert!(
        rect(&list, "r0").rect[1] < viewport[1],
        "still scrolled, not reset to the top"
    );
}

// A container with an authored id matches by id, so a sibling inserted ahead
// of it (its child-index path changes) still carries its offset.
#[test]
fn a_container_with_an_id_carries_its_offset_when_its_path_changes() {
    let list = scroll_list(100.0, 10, None, false).replacen(
        r#"{"kind":"vstack","#,
        r#"{"kind":"vstack","id":"levels","#,
        1,
    );
    let mut h = Harness::new(menu(&list));
    h.frame(ScrollInput::default());
    let viewport = rect(&h.rects(), "r0").clip.expect("clipped");
    h.wheel([viewport[0] + 4.0, viewport[1] + 4.0], -2.0);
    let before = h.rects();
    let scrolled_by = rect(&before, "r0").clip.expect("clipped")[1] - rect(&before, "r0").rect[1];
    assert!(scrolled_by > 0.0);

    h.rebuild(menu(&format!("{},{list}", button("before"))));
    h.frame(ScrollInput::default());
    let after = h.rects();
    let r0 = rect(&after, "r0");
    let viewport = r0.clip.expect("clipped");
    assert!(approx(viewport[1] - r0.rect[1], scrolled_by));
}

// A relayout that does not move focus leaves a wheel scroll alone when the
// wheel had already taken the focused stop out of full view.
#[test]
fn a_relayout_without_a_focus_change_keeps_a_wheel_scroll() {
    let mut h = Harness::new(menu(&scroll_list(100.0, 10, Some(9), false)));
    h.focused("r0");
    let viewport = rect(&h.rects(), "r0").clip.expect("clipped");
    h.frame(ScrollInput {
        focused_id: Some("r0"),
        wheel: Some(UiWheelScroll {
            position: [viewport[0] + 4.0, viewport[1] + 4.0],
            lines: -1.0,
            pixels: 0.0,
        }),
    });
    let wheeled = rect(&h.rects(), "r0").rect;
    assert!(wheeled[1] < viewport[1] - 1.0, "the wheel scrolled r0 up");

    // r9 hides: the content relays out, focus stays on r0.
    h.slots
        .insert("menu.more".to_string(), SlotValue::Boolean(false));
    h.focused("r0");
    let after = rect(&h.rects(), "r0").rect;
    assert!(
        approx(after[1], wheeled[1]),
        "the relayout did not scroll r0 back into view: {after:?} vs {wheeled:?}"
    );
}
