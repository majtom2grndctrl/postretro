// Flex/grid layout, anchoring, backdrop, and shaped-text measurement tests.

use super::common::*;
#[test]
fn vstack_distributes_children_along_column_with_gap() {
    // A column of two sized text leaves: the second sits directly below the
    // first, separated by exactly the container gap. Cross-axis Start keeps
    // both at x = padding. The container content-sizes to its children, so the
    // column height is `h0 + gap + h1`.
    let gap = 20.0;
    let pad = 8.0;
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        // Two single-line text leaves; each is shaped to its real glyph
        // extent by the measure seam. Exact dimensions come from Inter; the
        // test asserts only the relative column layout (gap, stacking).
        root: vstack(
            gap,
            pad,
            Align::Start,
            vec![text("AB", 40.0), text("CD", 40.0)],
        ),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());

    let children: Vec<_> = ui.taffy.children(ui.root).unwrap();
    let c0 = *ui.taffy.layout(children[0]).unwrap();
    let c1 = *ui.taffy.layout(children[1]).unwrap();
    // Both children indent by the padding on the cross axis.
    assert!(approx(c0.location.x, pad) && approx(c1.location.x, pad));
    // First child sits at the padding top; second is one height + gap below.
    assert!(approx(c0.location.y, pad), "first child at top padding");
    assert!(
        approx(c1.location.y - (c0.location.y + c0.size.height), gap),
        "gap of {gap} between the two children (got {})",
        c1.location.y - (c0.location.y + c0.size.height),
    );
    // The column content-sizes to its children + gap + padding on both edges.
    let root = ui.taffy.layout(ui.root).unwrap();
    assert!(
        approx(
            root.size.height,
            c0.size.height + gap + c1.size.height + 2.0 * pad
        ),
        "column height is children + gap + vertical padding",
    );
    // Two text leaves produced two device-positioned text runs, no quads.
    assert_eq!(data.texts.len(), 2);
    assert!(data.quads.is_empty());
}

#[test]
fn nested_hstack_in_vstack_distributes_inner_row_along_x() {
    // Outer column holds one inner row; the row lays its two sized text leaves
    // left-to-right separated by the row gap. Asserts the nested container's
    // children flow on the main (x) axis with the gap applied — the
    // vstack-of-hstack composition the task calls out.
    let gap = 12.0;
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: vstack(
            0.0,
            0.0,
            Align::Start,
            vec![hstack(
                gap,
                0.0,
                Align::Start,
                vec![text("AB", 30.0), text("CD", 30.0)],
            )],
        ),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());

    let row = ui.taffy.children(ui.root).unwrap()[0];
    let cells: Vec<_> = ui.taffy.children(row).unwrap();
    let a = *ui.taffy.layout(cells[0]).unwrap();
    let b = *ui.taffy.layout(cells[1]).unwrap();
    // Both leaves share the row's top (same y); the second is one width + gap
    // to the right of the first.
    assert!(
        approx(a.location.y, b.location.y),
        "row children share a baseline row"
    );
    assert!(
        approx(b.location.x - a.location.x, a.size.width + gap),
        "second leaf is one width + gap right of the first (got {})",
        b.location.x - a.location.x,
    );
    // The inner row content-sizes to both leaves plus the single gap.
    let row_layout = ui.taffy.layout(row).unwrap();
    assert!(
        approx(row_layout.size.width, a.size.width + gap + b.size.width),
        "row width is both leaves + one gap",
    );
}

#[test]
fn spacer_maps_to_flex_grow_and_emits_no_draw_payload() {
    // A row of `text — spacer — text`: the spacer is a pure layout node
    // (flex_grow, no `NodeContext`) that sits between the two leaves without
    // overlapping them, while the leaves still produce their text runs.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: hstack(
            0.0,
            0.0,
            Align::Start,
            vec![text("X", 40.0), spacer(1.0), text("Y", 40.0)],
        ),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());

    let cells: Vec<_> = ui.taffy.children(ui.root).unwrap();
    let x = *ui.taffy.layout(cells[0]).unwrap();
    let s = *ui.taffy.layout(cells[1]).unwrap();
    let y = *ui.taffy.layout(cells[2]).unwrap();
    // Main-axis order is X, spacer, Y with no overlap.
    assert!(
        s.location.x >= x.location.x + x.size.width - EPS,
        "spacer after X"
    );
    assert!(
        y.location.x >= s.location.x + s.size.width - EPS,
        "Y after spacer"
    );
    // Spacer carries no draw payload; the two text leaves do.
    assert!(ui.taffy.get_node_context(cells[1]).is_none());
    assert_eq!(data.texts.len(), 2, "only the two text leaves draw");
    assert!(data.quads.is_empty());
}

#[test]
fn child_rects_scale_uniformly_at_4k() {
    // The same tree at 3840x2160 (3x the reference) produces device rects 3x
    // the size and position of the 1280x720 result. Mirrors layout.rs's
    // `center_panel_scales_uniformly_at_4k`. Sized text leaves give the row a
    // non-zero extent to scale.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: hstack(
            40.0,
            0.0,
            Align::Start,
            vec![text("AAAA", 20.0), text("BBBB", 20.0)],
        ),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut fs = font_system();
    let mut ui_ref = UiTree::from_descriptor(&tree, &theme());
    let data_ref = ui_ref.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    let mut ui_4k = UiTree::from_descriptor(&tree, &theme());
    let data_4k = ui_4k.build_draw_data([3840, 2160], &mut fs, &no_images(), &no_slots());

    assert_eq!(data_ref.texts.len(), 2);
    assert_eq!(data_4k.texts.len(), 2);
    // Each text run's device position + font size scale by exactly 3.
    for i in 0..2 {
        let p_ref = data_ref.texts[i].position;
        let p_4k = data_4k.texts[i].position;
        assert!(
            approx(p_4k[0], p_ref[0] * 3.0) && approx(p_4k[1], p_ref[1] * 3.0),
            "text {i} position scales 3x: {p_ref:?} -> {p_4k:?}",
        );
        assert!(
            approx(
                data_4k.texts[i].font_size,
                data_ref.texts[i].font_size * 3.0
            ),
            "text {i} font size scales 3x",
        );
    }
}

#[test]
fn grid_places_children_across_equal_columns() {
    // A 2-column grid with four sized cells: cells 0/1 share row 0, cells 2/3
    // share row 1. Columns are equal width; cell 1 sits to the right of cell
    // 0 by one column width + gap.
    let cell = || {
        Widget::Text(TextWidget {
            content: "XX".into(),
            font_size: 10.0,
            color: ColorValue::Literal([1.0; 4]),
            font: None,
            bind: None,
            style_ranges: None,
            id: None,
            focus_neighbors: Default::default(),
            visible_when: None,
            role: None,
        })
    };
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: Widget::Grid(GridWidget {
            gap: SpacingValue::Literal(8.0),
            padding: SpacingValue::Literal(0.0),
            align: Align::Start,
            cols: 2,
            id: None,
            focus_neighbors: Default::default(),
            focus: None,
            restore_on_return: false,
            visible_when: None,
            role: None,
            children: vec![cell(), cell(), cell(), cell()],
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    let cells: Vec<_> = ui.taffy.children(ui.root).unwrap();
    assert_eq!(cells.len(), 4);
    let l = |n: NodeId| {
        let lay = ui.taffy.layout(n).unwrap();
        (
            lay.location.x,
            lay.location.y,
            lay.size.width,
            lay.size.height,
        )
    };
    let (x0, y0, w0, _) = l(cells[0]);
    let (x1, y1, _, _) = l(cells[1]);
    let (x2, y2, _, _) = l(cells[2]);
    // Cells 0 and 1 are on the same row; 1 is one column + gap to the right.
    assert!(approx(y0, y1), "cells 0 and 1 share a row");
    assert!(
        approx(x1 - x0, w0 + 8.0),
        "column 1 is one track + gap right of column 0 (got {})",
        x1 - x0
    );
    // Cell 2 wraps to row 1, back at column 0's x.
    assert!(approx(x2, x0), "cell 2 wraps to column 0");
    assert!(y2 > y0, "cell 2 is on a lower row");
}

#[test]
fn anchored_tree_centers_against_non_16_9_letterbox() {
    // At 1280x1440 the canvas letterboxes vertically: scale = min(1.0, 2.0) =
    // 1.0, canvas origin y = (1440 - 720)/2 = 360. A center-anchored sized
    // panel lands centered in the 1280x720 canvas, then shifted down by 360.
    let tree = AnchoredTree {
        anchor: Anchor::Center,
        offset: [0.0, 0.0],
        // A single text leaf so the root has a finite measured size to center.
        // Its size is the real shaped extent — the test derives the expected
        // centered position from that measured size, not a fixed number.
        root: Widget::Text(TextWidget {
            content: "ABCDEFGH".into(),
            font_size: 40.0,
            color: ColorValue::Literal([1.0, 1.0, 1.0, 1.0]),
            font: None,
            bind: None,
            style_ranges: None,
            id: None,
            focus_neighbors: Default::default(),
            visible_when: None,
            role: None,
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let data = ui.build_draw_data([1280, 1440], &mut fs, &no_images(), &no_slots());
    // Read back the root's measured size and recompute the centered top-left
    // in the 1280x720 canvas, then apply the +360 vertical letterbox offset.
    // Scale is 1.0 here, so device px == reference px. `project_rect` snaps
    // the device top-left to a whole pixel, so round to match.
    let root_size = ui.taffy.layout(ui.root).unwrap().size;
    let expected_x = ((REFERENCE_WIDTH - root_size.width) / 2.0).round();
    let expected_y = ((REFERENCE_HEIGHT - root_size.height) / 2.0 + 360.0).round();
    let t = &data.texts[0];
    assert!(
        approx(t.position[0], expected_x),
        "centered x in canvas: {} != {}",
        t.position[0],
        expected_x,
    );
    assert!(
        approx(t.position[1], expected_y),
        "centered y plus vertical letterbox offset: {} != {}",
        t.position[1],
        expected_y,
    );
}

#[test]
fn container_backdrop_quad_rects_snap_to_integer_device_pixels() {
    // A container with a backdrop `fill` content-sizes to its text children
    // and emits a backdrop quad; at a fractional scale that quad's rect must
    // still snap to whole device pixels. (Bare panel leaves have no intrinsic
    // size now, so the backdrop is the canonical quad-producing path.)
    let filled = Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(7.0),
        padding: SpacingValue::Literal(5.0),
        align: Align::Start,
        fill: Some(ColorValue::Literal([0.2, 0.4, 0.6, 1.0])),
        border: None,
        id: None,
        focus_neighbors: Default::default(),
        focus: None,
        restore_on_return: false,
        local_state: None,
        visible_when: None,
        role: None,
        children: vec![text("x", 13.0), text("y", 13.0)],
    });
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [3.5, 7.25],
        root: filled,
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    // Fractional scale: 1281x721 -> scale ~1.00078.
    let data = ui.build_draw_data([1281, 721], &mut fs, &no_images(), &no_slots());
    assert!(!data.quads.is_empty(), "container backdrop produced a quad");
    for q in &data.quads.instances {
        for v in q.rect {
            assert!(
                approx(v, v.round()),
                "quad rect component {v} not snapped to a whole device pixel",
            );
        }
    }
}

#[test]
fn container_backdrop_draws_beneath_children_sized_to_full_rect() {
    // A filled container emits ONE backdrop quad sized to its own full laid-out
    // rect, and its children draw on top (painter's order). The backdrop is the
    // first draw entry; the text children produce runs over it.
    let filled = Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(0.0),
        padding: SpacingValue::Literal(10.0),
        align: Align::Start,
        fill: Some(ColorValue::Literal([0.1, 0.2, 0.3, 1.0])),
        border: None,
        id: None,
        focus_neighbors: Default::default(),
        focus: None,
        restore_on_return: false,
        local_state: None,
        visible_when: None,
        role: None,
        children: vec![text("AB", 40.0)],
    });
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: filled,
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());

    // Exactly one backdrop quad (the container), one text run on top.
    assert_eq!(data.quads.instances.len(), 1, "one container backdrop quad");
    assert_eq!(data.texts.len(), 1, "one child text run drawn over it");

    // The backdrop spans the container's full rect: it covers the child run
    // (which is inset by the padding), so the quad is wider+taller than the run.
    let quad = data.quads.instances[0].rect;
    let run_top = data.texts[0].position[1];
    assert!(
        quad[1] < run_top,
        "backdrop top {} sits above the padded child run top {run_top}",
        quad[1],
    );
}

#[test]
fn text_color_converts_linear_rgba_to_srgb_u8() {
    // Linear 1.0 -> sRGB 255; linear 0.0 -> 0; alpha is linear-scaled. A
    // mid-gray linear 0.5 encodes to ~188 in sRGB (not 128).
    assert_eq!(
        linear_rgba_to_srgb_u8([1.0, 0.0, 1.0, 1.0]),
        [255, 0, 255, 255]
    );
    let mid = linear_rgba_to_srgb_u8([0.5, 0.5, 0.5, 0.5]);
    assert!(
        (185..=192).contains(&mid[0]),
        "linear 0.5 encodes to ~188 sRGB, got {}",
        mid[0],
    );
    assert_eq!(mid[3], 128, "alpha stays linear (0.5 -> 128)");
}

/// Lay out a single text leaf and return its taffy-computed size — the size
/// the measure seam produced from shaped glyph metrics.
fn measured_text_size(content: &str, font_size: f32) -> taffy::geometry::Size<f32> {
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: text(content, font_size),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    ui.taffy.layout(ui.root).unwrap().size
}

#[test]
fn text_node_width_differs_with_content_via_shaped_measurement() {
    // Construct two trees whose text leaves differ only in content (same font
    // size). Real shaping gives them different advances, so the measure seam
    // must report different widths. Content is immutable on the descriptor — this is a
    // two-tree comparison, not runtime mutation.
    let narrow = measured_text_size("i", 40.0);
    let wide = measured_text_size("WWWWWWWW", 40.0);

    assert!(
        wide.width > narrow.width + EPS,
        "eight wide glyphs must shape wider than a single narrow one ({} vs {})",
        wide.width,
        narrow.width,
    );
    // Both single-line runs report a positive line-box height.
    assert!(
        narrow.height > 0.0 && wide.height > 0.0,
        "shaped text reports a positive line height",
    );
}

#[test]
fn text_node_width_tracks_proportional_glyph_advances() {
    // The glyph-count placeholder this replaced sized every glyph identically
    // (`chars * font_size * 0.5`). Real shaping is proportional: a string of
    // narrow glyphs ("ll") shapes narrower than the same count of wide glyphs
    // ("WW"). Equal width here would mean we were still counting chars.
    let narrow = measured_text_size("llll", 40.0);
    let wide = measured_text_size("WWWW", 40.0);

    assert!(
        wide.width > narrow.width + EPS,
        "four wide glyphs must shape wider than four narrow glyphs ({} vs {}) \
             — proportional advances, not a glyph count",
        wide.width,
        narrow.width,
    );
}

#[test]
fn text_node_size_is_not_the_glyph_count_estimate() {
    // The replaced placeholder was exactly `chars * font_size * 0.5` wide by
    // `font_size` tall. Assert the shaped size does NOT coincide with that
    // formula, proving the size comes from glyph metrics. Inter's "MMMM" is
    // wide and the line box is `font_size * 1.25` tall, so neither axis lands
    // on the old estimate.
    let content = "MMMM";
    let font_size = 40.0;
    let size = measured_text_size(content, font_size);

    let placeholder_w = content.chars().count() as f32 * font_size * 0.5;
    let placeholder_h = font_size;
    assert!(
        (size.width - placeholder_w).abs() > 1.0,
        "shaped width {} must not match the old glyph-count estimate {}",
        size.width,
        placeholder_w,
    );
    assert!(
        (size.height - placeholder_h).abs() > 1.0,
        "shaped line-box height {} must not match the old font-size estimate {}",
        size.height,
        placeholder_h,
    );
}
