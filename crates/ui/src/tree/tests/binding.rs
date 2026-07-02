// Slot-binding resolution and the retained relayout/redraw split.

use super::common::*;
/// A bound panel leaf, fallback `fill` plus a `bind` slot. Wrapped in a
/// stretch container so the panel leaf gets a non-zero laid-out rect (a bare
/// panel has no intrinsic size).
fn bound_panel_in_stack(fill: [f32; 4], slot: &str) -> Widget {
    Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(0.0),
        padding: SpacingValue::Literal(0.0),
        align: Align::Stretch,
        fill: Some(ColorValue::Literal([0.0, 0.0, 0.0, 1.0])),
        border: None,
        id: None,
        focus_neighbors: Default::default(),
        focus: None,
        restore_on_return: false,
        local_state: None,
        visible_when: None,
        role: None,
        children: vec![Widget::Panel(PanelWidget {
            fill: ColorValue::Literal(fill),
            border: None,
            bind: Some(PanelBind {
                source: BindSource::Slot { slot: slot.into() },
                tween: None,
            }),
            style_ranges: None,
            id: None,
            focus_neighbors: Default::default(),
            visible_when: None,
            role: None,
        })],
    })
}

#[test]
fn bound_text_resolves_slot_value_through_format_template() {
    // A text node bound to `player.health` with a "HP {}" template renders the
    // slot's numeric value substituted into the template. The integral Number
    // 87 formats without a trailing ".0".
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: bound_text("0", "player.health", Some("HP {}")),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut slots = HashMap::new();
    slots.insert("player.health".to_string(), SlotValue::Number(87.0));

    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &slots);

    assert_eq!(data.texts.len(), 1);
    assert_eq!(
        data.texts[0].content, "HP 87",
        "slot resolved into template"
    );
}

#[test]
fn bound_text_without_format_renders_bare_value() {
    // No template: the resolved value's bare string form is drawn. A
    // fractional Number keeps its decimals.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: bound_text("0", "player.ammo", None),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut slots = HashMap::new();
    slots.insert("player.ammo".to_string(), SlotValue::Number(12.5));

    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &slots);

    assert_eq!(data.texts[0].content, "12.5");
}

#[test]
fn bound_text_falls_back_to_literal_when_slot_absent() {
    // The slot is not present in the snapshot (not written this frame): the
    // node renders its literal `content` fallback rather than panicking.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: bound_text("fallback", "player.health", Some("HP {}")),
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
        data.texts[0].content, "fallback",
        "absent slot falls back to literal content, not the template",
    );
}

#[test]
fn bound_panel_resolves_color_slot_into_fill() {
    // A panel whose fill is bound to `intro.flashColor` (a length-4 linear
    // RGBA array) draws that color, overriding its literal fallback fill.
    let resolved = [0.25, 0.5, 0.75, 1.0];
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: bound_panel_in_stack([0.0, 0.0, 0.0, 1.0], "intro.flashColor"),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut slots = HashMap::new();
    slots.insert(
        "intro.flashColor".to_string(),
        SlotValue::Array(resolved.to_vec()),
    );

    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &slots);

    // Two quads: the container backdrop, then the bound panel leaf. Find the
    // one carrying the resolved color.
    let found = data.quads.instances.iter().any(|q| {
        q.color
            .iter()
            .zip(resolved.iter())
            .all(|(a, b)| approx(*a, *b))
    });
    assert!(found, "a panel quad carries the resolved flash color");
}

#[test]
fn bound_panel_falls_back_on_malformed_array_length() {
    // A present slot of the wrong shape (a length-3 array) is malformed: the
    // panel falls back to its literal fill (and warns once — not asserted).
    let fallback = [0.9, 0.1, 0.2, 1.0];
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: bound_panel_in_stack(fallback, "intro.flashColor"),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut slots = HashMap::new();
    slots.insert(
        "intro.flashColor".to_string(),
        SlotValue::Array(vec![0.1, 0.2, 0.3]),
    );

    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &slots);

    let found = data.quads.instances.iter().any(|q| {
        q.color
            .iter()
            .zip(fallback.iter())
            .all(|(a, b)| approx(*a, *b))
    });
    assert!(
        found,
        "malformed-length array falls back to the literal fill"
    );
}

#[test]
fn bound_panel_falls_back_when_slot_absent() {
    // No slot written: the panel draws its literal fill, silently (no warn).
    let fallback = [0.3, 0.6, 0.9, 1.0];
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: bound_panel_in_stack(fallback, "intro.flashColor"),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());

    let found = data.quads.instances.iter().any(|q| {
        q.color
            .iter()
            .zip(fallback.iter())
            .all(|(a, b)| approx(*a, *b))
    });
    assert!(found, "absent slot falls back to the literal fill");
}

// --- Retained-tree diff + relayout/redraw split ---------------------------

#[test]
fn retained_panel_fill_change_rebuilds_draw_list_without_recompute() {
    // Acceptance (a): an appearance-only bound change (the panel flash color)
    // refreshes the draw list WITHOUT a taffy relayout. The first frame
    // computes once; a frame that only changes the bound fill rebuilds the
    // draw list (new color visible) but leaves `recompute_count` flat.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: bound_panel_in_stack([0.0, 0.0, 0.0, 1.0], "intro.flashColor"),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();

    let red = [1.0, 0.0, 0.0, 1.0];
    let first = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &flash_slots(red),
        &no_cells(),
        0.0,
    );
    assert_eq!(ui.recompute_count(), 1, "first frame computes once");
    assert!(
        flash_quad_color(&first).is_some_and(|c| colors_eq(c, red)),
        "first frame draws the red flash",
    );

    let green = [0.0, 1.0, 0.0, 1.0];
    let second = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &flash_slots(green),
        &no_cells(),
        0.0,
    );
    assert_eq!(
        ui.recompute_count(),
        1,
        "appearance-only fill change must not relayout",
    );
    assert!(
        flash_quad_color(&second).is_some_and(|c| colors_eq(c, green)),
        "draw list reflects the new flash color",
    );
}

#[test]
fn retained_bound_text_content_change_triggers_relayout() {
    // Acceptance (b): a bound text-content change (which re-measures) DOES
    // trigger a relayout — `recompute_count` increments.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: bound_text("0", "player.health", Some("HP {}")),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();

    let mut slots = HashMap::new();
    slots.insert("player.health".to_string(), SlotValue::Number(100.0));
    let first =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
    assert_eq!(ui.recompute_count(), 1, "first frame computes once");
    assert_eq!(first.texts[0].content, "HP 100");

    slots.insert("player.health".to_string(), SlotValue::Number(75.0));
    let second =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
    assert_eq!(
        ui.recompute_count(),
        2,
        "a bound text-content change relays out",
    );
    assert_eq!(second.texts[0].content, "HP 75", "new content is drawn");
}

#[test]
fn retained_unbound_slot_change_invalidates_nothing() {
    // Acceptance (c): the diff is subscriber-aware — a slot with no binding in
    // the tree changing value must invalidate nothing: no relayout, no
    // draw-list rebuild. The tree binds `player.health`; we change an unrelated
    // `world.kills` slot.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: bound_text("0", "player.health", Some("HP {}")),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();

    let mut slots = HashMap::new();
    slots.insert("player.health".to_string(), SlotValue::Number(100.0));
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
    assert_eq!(ui.recompute_count(), 1);
    assert_eq!(
        ui.draw_rebuild_count(),
        1,
        "first frame builds the draw list"
    );

    // Change only an unbound slot; the bound `player.health` is untouched.
    slots.insert("world.kills".to_string(), SlotValue::Number(7.0));
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
    assert_eq!(
        ui.recompute_count(),
        1,
        "an unbound slot change must not relayout",
    );
    assert_eq!(
        ui.draw_rebuild_count(),
        1,
        "an unbound slot change must not rebuild the draw list",
    );
}

#[test]
fn retained_settled_frame_skips_draw_rebuild_and_recompute() {
    // Acceptance (d): after the flash settles to a constant color, a no-change
    // frame performs NO draw-list rebuild and NO relayout — the dirty-gate
    // short-circuits and the cached list is returned.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: bound_panel_in_stack([0.0, 0.0, 0.0, 1.0], "intro.flashColor"),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();

    let settled = [0.2, 0.4, 0.6, 1.0];
    let first = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &flash_slots(settled),
        &no_cells(),
        0.0,
    );
    assert_eq!(ui.recompute_count(), 1);
    assert_eq!(ui.draw_rebuild_count(), 1, "first frame builds the list");

    // Same color again: nothing changed, so neither the layout nor the draw
    // list rebuild — the cached list is returned unchanged.
    let second = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &flash_slots(settled),
        &no_cells(),
        0.0,
    );
    assert_eq!(ui.recompute_count(), 1, "settled frame does not relayout");
    assert_eq!(
        ui.draw_rebuild_count(),
        1,
        "settled frame returns the cached draw list (no rebuild)",
    );
    // The returned (cached) list still carries the settled color.
    assert!(
        flash_quad_color(&second).is_some_and(|c| colors_eq(c, settled)),
        "cached draw list still reflects the settled color",
    );
    assert_eq!(
        first.quads.instances.len(),
        second.quads.instances.len(),
        "cached list matches the first build",
    );
}
