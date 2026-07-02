// Text-value tween easing: first-resolve, retarget, settle, relayout, fresh path.

use super::common::*;
#[test]
fn text_tween_first_resolve_with_from_starts_at_from_and_reaches_target_at_duration() {
    // First-resolve `from` flourish: a text bind with `from: 0.0`, target 100
    // (constant slot). Frame 0 renders 0; subsequent frames advance
    // monotonically toward 100; the value is EXACTLY 100 at durationMs.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: tweened_text(
            "0",
            "player.health",
            None,
            TextTween {
                duration_ms: 1000.0,
                easing: Easing::Linear,
                from: Some(0.0),
            },
        ),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let slots = number_slots("player.health", 100.0);

    // Frame 0: display starts at `from` = 0.
    let f0 =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
    assert_eq!(text_value(&f0), 0.0, "frame 0 renders the `from` value");

    // Advance through the tween; values rise monotonically toward 100.
    let mut prev = 0.0;
    for &t in &[0.25, 0.5, 0.75] {
        let f =
            ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), t);
        let v = text_value(&f);
        assert!(
            v >= prev && v <= 100.0,
            "value {v} at t={t} advances monotonically within [prev={prev}, 100]",
        );
        prev = v;
    }

    // At t == durationMs (1.0s) the display equals the target EXACTLY.
    let f_end =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 1.0);
    assert_eq!(
        text_value(&f_end),
        100.0,
        "display equals target at duration"
    );
}

#[test]
fn text_tween_without_from_renders_target_immediately_on_first_resolve() {
    // A tween with no `from` snaps to the target on first sight (no flourish):
    // frame 0 already renders the full target value.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: tweened_text(
            "0",
            "player.health",
            None,
            TextTween {
                duration_ms: 1000.0,
                easing: Easing::EaseOut,
                from: None,
            },
        ),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let slots = number_slots("player.health", 80.0);

    let f0 =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
    assert_eq!(
        text_value(&f0),
        80.0,
        "no `from` snaps to the target on first resolve",
    );
}

#[test]
fn text_tween_retarget_mid_flight_restarts_from_current_display() {
    // Mid-flight retarget: a tween from 0 -> 100 is interrupted at t=0.5 by a
    // new target of 0. The tween must restart from the CURRENT display value
    // (~50 under linear), not snap to `from` (0) nor jump to the new target.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: tweened_text(
            "0",
            "player.health",
            None,
            TextTween {
                duration_ms: 1000.0,
                easing: Easing::Linear,
                from: Some(0.0),
            },
        ),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();

    // Drive to mid-flight at t=0.5 with target 100: display ~= 50.
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("player.health", 100.0),
        &no_cells(),
        0.0,
    );
    let mid = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("player.health", 100.0),
        &no_cells(),
        0.5,
    );
    let mid_v = text_value(&mid);
    assert!(
        (40.0..=60.0).contains(&mid_v),
        "mid-flight value ~50 under linear easing, got {mid_v}",
    );

    // Retarget to 0 at t=0.5: the segment restarts from the current display
    // (~50) at this instant, so this very frame still reads ~50 (elapsed 0) —
    // it must NOT snap to `from`=0 nor jump to the new target 0.
    let retarget = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("player.health", 0.0),
        &no_cells(),
        0.5,
    );
    let retarget_v = text_value(&retarget);
    assert!(
        (40.0..=60.0).contains(&retarget_v),
        "retarget restarts from the current display ~{mid_v} (no snap to from/target), got {retarget_v}",
    );

    // A later frame eases DOWN from ~50 toward 0 — continuous, below the
    // retarget value and above the new target.
    let after = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("player.health", 0.0),
        &no_cells(),
        1.0,
    );
    let after_v = text_value(&after);
    assert!(
        after_v < retarget_v && after_v > 0.0,
        "retargeted tween eases continuously down from {retarget_v} toward 0, got {after_v}",
    );

    // And it reaches the new target exactly one duration after the retarget.
    let settled = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("player.health", 0.0),
        &no_cells(),
        1.5,
    );
    assert_eq!(text_value(&settled), 0.0, "retargeted tween settles at 0");
}

#[test]
fn text_tween_ease_out_advances_monotonically_toward_target() {
    // Easing monotonicity: under easeOut the in-flight display rises
    // monotonically toward the target across advancing frames.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: tweened_text(
            "0",
            "player.health",
            None,
            TextTween {
                duration_ms: 1000.0,
                easing: Easing::EaseOut,
                from: Some(0.0),
            },
        ),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let slots = number_slots("player.health", 100.0);

    let mut prev = -1.0;
    for &t in &[0.0, 0.1, 0.2, 0.4, 0.6, 0.8, 1.0] {
        let f =
            ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), t);
        let v = text_value(&f);
        assert!(
            v >= prev,
            "easeOut value must be monotonic non-decreasing: {v} < {prev} at t={t}",
        );
        prev = v;
    }
    assert_eq!(
        prev, 100.0,
        "easeOut reaches the target exactly at duration"
    );
}

#[test]
fn text_tween_settles_at_exact_target_past_duration() {
    // Exact-target settle: at t >= duration the display equals the target
    // exactly (a frame well past the end stays pinned, no overshoot).
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: tweened_text(
            "0",
            "player.health",
            None,
            TextTween {
                duration_ms: 500.0,
                easing: Easing::EaseInOut,
                from: Some(10.0),
            },
        ),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let slots = number_slots("player.health", 42.0);

    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
    // t = 5.0s is ten durations past the end.
    let far =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 5.0);
    assert_eq!(text_value(&far), 42.0, "well past duration pins to target");
}

#[test]
fn text_tween_in_flight_relayouts_each_advancing_frame() {
    // In-flight text is content-changed each advancing frame (the rendered
    // integer string differs, re-measures): recompute_count increments per
    // frame while the eased value is still moving.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: tweened_text(
            "0",
            "player.health",
            None,
            TextTween {
                duration_ms: 1000.0,
                easing: Easing::Linear,
                from: Some(0.0),
            },
        ),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let slots = number_slots("player.health", 100.0);

    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
    let c0 = ui.recompute_count();
    // Each advancing frame moves the integer (0 -> 25 -> 50 -> 75), so each
    // relays out.
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &slots,
        &no_cells(),
        0.25,
    );
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.5);
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &slots,
        &no_cells(),
        0.75,
    );
    assert_eq!(
        ui.recompute_count(),
        c0 + 3,
        "an in-flight text tween relays out each advancing frame",
    );
}

#[test]
fn text_tween_settled_frame_skips_rebuild_and_recompute() {
    // Post-settle no-rebuild: once a text tween has settled, a no-change frame
    // (same target, time well past the end) returns the cached draw list with
    // NO relayout and NO draw-list rebuild.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: tweened_text(
            "0",
            "player.health",
            None,
            TextTween {
                duration_ms: 500.0,
                easing: Easing::Linear,
                from: Some(0.0),
            },
        ),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let slots = number_slots("player.health", 30.0);

    // Drive past the end so the display settles at 30.
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 1.0);
    let r_settled = ui.recompute_count();
    let d_settled = ui.draw_rebuild_count();

    // A further frame at a still-later time with the same target: the rounded
    // display is already 30 and stays 30, so nothing rebuilds.
    let f =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 2.0);
    assert_eq!(
        ui.recompute_count(),
        r_settled,
        "a settled text frame does not relayout",
    );
    assert_eq!(
        ui.draw_rebuild_count(),
        d_settled,
        "a settled text frame returns the cached list (no rebuild)",
    );
    assert_eq!(text_value(&f), 30.0, "cached list still carries the target");
}

#[test]
fn text_tween_on_string_slot_snaps_through_unchanged_path() {
    // Non-numeric snap-with-warn: a text tween whose slot resolves to a
    // `String` renders via the unchanged `resolve_text` path (the bare string),
    // not an eased number. (The once-per-frame warn is logged but not
    // asserted — log capture is out of scope for these CPU tests.)
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: tweened_text(
            "fallback",
            "hud.label",
            None,
            TextTween {
                duration_ms: 1000.0,
                easing: Easing::Linear,
                from: Some(0.0),
            },
        ),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let mut slots = HashMap::new();
    slots.insert("hud.label".to_string(), SlotValue::String("ALERT".into()));

    let f =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
    assert_eq!(
        f.texts[0].content, "ALERT",
        "a tween on a non-Number slot renders the raw string, not an eased number",
    );
}

#[test]
fn text_tween_fresh_path_resolves_target_directly_no_cross_frame_state() {
    // Fresh-path inertness: the same tweened descriptor through the fresh
    // `build_draw_data` (no time, no retained state) resolves the target
    // DIRECTLY — no flourish, no eased value, no cross-frame tween state.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: tweened_text(
            "0",
            "player.health",
            None,
            TextTween {
                duration_ms: 1000.0,
                easing: Easing::Linear,
                from: Some(0.0),
            },
        ),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let slots = number_slots("player.health", 100.0);

    // Fresh path renders the target (100) immediately — `from`=0 is ignored.
    let f = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &slots);
    assert_eq!(
        f.texts[0].content, "100",
        "the fresh path resolves the tween target directly (inert, no easing)",
    );
    // No tween state was born on the node (the fresh path never drives tweens).
    if let Some(NodeContext::Text { tween, .. }) = ui.taffy.get_node_context(ui.root) {
        assert!(
            tween.is_none(),
            "fresh path leaves no cross-frame tween state"
        );
    } else {
        panic!("root must be a text node");
    }
}

#[test]
fn untweened_bound_text_unaffected_by_time() {
    // Untweened binds keep the existing behavior regardless of `time_seconds`:
    // the resolved value is rendered directly, no easing, no tween state.
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
    let slots = number_slots("player.health", 73.0);

    let f =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 9.0);
    assert_eq!(
        f.texts[0].content, "HP 73",
        "an untweened bind renders the resolved value directly at any time",
    );
    if let Some(NodeContext::Text { tween, .. }) = ui.taffy.get_node_context(ui.root) {
        assert!(tween.is_none(), "an untweened bind grows no tween state");
    } else {
        panic!("root must be a text node");
    }
}
