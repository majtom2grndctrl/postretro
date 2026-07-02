// styleRanges value→color band evaluation in the draw list.

use super::common::*;
/// value the map evaluates; the literal `color` is the base color a no-color
/// or no-match band keeps.
fn styled_text(base: [f32; 4], slot: &str, ranges: StyleRanges) -> Widget {
    Widget::Text(TextWidget {
        content: "0".into(),
        font_size: 20.0,
        color: ColorValue::Literal(base),
        font: None,
        bind: Some(TextBind {
            source: BindSource::Slot { slot: slot.into() },
            format: None,
            tween: None,
        }),
        style_ranges: Some(ranges),
        id: None,
        focus_neighbors: Default::default(),
        visible_when: None,
        role: None,
    })
}

/// The three-band health map used across the integration tests: red ≤ 0.25,
/// amber ≤ 0.5, default green. Band colors are token literals so the draw
/// build's resolved sRGB is predictable.
fn health_style_ranges() -> StyleRanges {
    StyleRanges {
        max: 100.0,
        entries: vec![
            StyleEntry {
                up_to: Some(0.25),
                color: Some(ColorValue::Literal([1.0, 0.0, 0.0, 1.0])),
                pulse: None,
                flash: None,
            },
            StyleEntry {
                up_to: Some(0.5),
                color: Some(ColorValue::Literal([1.0, 1.0, 0.0, 1.0])),
                pulse: None,
                flash: None,
            },
            StyleEntry {
                up_to: None,
                color: Some(ColorValue::Literal([0.0, 1.0, 0.0, 1.0])),
                pulse: None,
                flash: None,
            },
        ],
    }
}

#[test]
fn style_ranges_change_text_color_at_the_declared_fraction() {
    // A `text` bound to `player.health` with the three-band map draws red at a
    // low value (10/100 = 0.10 → first band) and green at a high value
    // (90/100 = 0.90 → trailing default). The drawn color tracks the band.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: styled_text([1.0, 1.0, 1.0, 1.0], "player.health", health_style_ranges()),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut fs = font_system();

    let mut ui_low = UiTree::from_descriptor(&tree, &theme());
    let low = ui_low.build_draw_data(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("player.health", 10.0),
    );
    assert_eq!(
        low.texts[0].color,
        srgb_of([1.0, 0.0, 0.0, 1.0]),
        "low health (fraction 0.10) draws the first band's red",
    );

    let mut ui_high = UiTree::from_descriptor(&tree, &theme());
    let high = ui_high.build_draw_data(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("player.health", 90.0),
    );
    assert_eq!(
        high.texts[0].color,
        srgb_of([0.0, 1.0, 0.0, 1.0]),
        "high health (fraction 0.90) draws the trailing default green",
    );
}

#[test]
fn style_ranges_band_color_token_degrades_to_magenta_in_draw_list() {
    // A band naming an unknown color token degrades to opaque magenta through
    // the existing theme rule — pre-resolved to a literal at build, so the
    // drawn run carries magenta.
    let ranges = StyleRanges {
        max: 100.0,
        entries: vec![StyleEntry {
            up_to: None,
            color: Some(ColorValue::Token("no.such.color".into())),
            pulse: None,
            flash: None,
        }],
    };
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: styled_text([1.0, 1.0, 1.0, 1.0], "player.health", ranges),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let data = ui.build_draw_data(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("player.health", 50.0),
    );
    assert_eq!(
        data.texts[0].color,
        srgb_of([1.0, 0.0, 1.0, 1.0]),
        "unknown band token degrades to opaque magenta",
    );
}

#[test]
fn style_ranges_without_a_bind_are_dropped_and_keep_the_base_color() {
    // styleRanges without a `bind` have no value to map: the build drops them
    // (warning once) and the node draws its plain base color, never a band.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: Widget::Text(TextWidget {
            content: "X".into(),
            font_size: 20.0,
            color: ColorValue::Literal([0.2, 0.4, 0.6, 1.0]),
            font: None,
            bind: None,
            style_ranges: Some(health_style_ranges()),
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
    let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    assert_eq!(
        data.texts[0].color,
        srgb_of([0.2, 0.4, 0.6, 1.0]),
        "a bindless styleRanges is dropped; the base color is drawn",
    );
    // The node carries no styleRanges (it was dropped at build).
    if let Some(NodeContext::Text { style_ranges, .. }) = ui.taffy.get_node_context(ui.root) {
        assert!(
            style_ranges.is_none(),
            "bindless styleRanges is dropped from the node",
        );
    } else {
        panic!("root must be a text node");
    }
}

#[test]
fn style_ranges_evaluate_the_eased_display_value_mid_tween() {
    // styleRanges evaluate the value the widget RENDERS — the eased display
    // value mid-tween, not the authoritative target. A tween easing 0→100
    // (target 100) renders a low display early, so the band is red even though
    // the target's fraction (1.0) would resolve to green.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: Widget::Text(TextWidget {
            content: "0".into(),
            font_size: 20.0,
            color: ColorValue::Literal([1.0, 1.0, 1.0, 1.0]),
            font: None,
            bind: Some(TextBind {
                source: BindSource::Slot {
                    slot: "player.health".into(),
                },
                format: None,
                tween: Some(TextTween {
                    duration_ms: 1000.0,
                    easing: Easing::Linear,
                    from: Some(0.0),
                }),
            }),
            style_ranges: Some(health_style_ranges()),
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
    let slots = number_slots("player.health", 100.0);

    // Frame 0: display is at `from` = 0 (fraction 0) → red band, NOT the
    // target's green. The eased display value drives the band.
    let f0 =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
    assert_eq!(
        f0.texts[0].color,
        srgb_of([1.0, 0.0, 0.0, 1.0]),
        "mid-tween the band tracks the eased display value (0 → red), not the target",
    );

    // At t == duration the display equals the target (100, fraction 1.0) →
    // the trailing green band.
    let f_end =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 1.0);
    assert_eq!(
        f_end.texts[0].color,
        srgb_of([0.0, 1.0, 0.0, 1.0]),
        "settled at the target, the band resolves to the default green",
    );
}
