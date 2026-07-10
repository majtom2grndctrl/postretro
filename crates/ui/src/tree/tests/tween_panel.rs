// Panel-fill RGBA tween easing: in-flight per-channel redraw and exact settle.

use super::common::*;
#[test]
fn panel_tween_in_flight_redraws_without_relayout() {
    // In-flight panel eases per-channel and is appearance-only: the draw list
    // rebuilds each advancing frame but layout NEVER recomputes.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: tweened_panel_in_stack(
            [0.0, 0.0, 0.0, 1.0],
            "intro.flashColor",
            PanelTween {
                duration_ms: 1000.0,
                easing: Easing::Linear,
                from: Some([0.0, 0.0, 0.0, 1.0]),
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
    let target = [1.0, 0.5, 0.25, 1.0];

    let f0 = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &flash_slots(target),
        &no_cells(),
        0.0,
    );
    assert_eq!(ui.recompute_count(), 1, "first frame computes once");
    // Frame 0 starts at the `from` color (all-black-but-alpha is the backdrop
    // color too, so just assert the panel hasn't reached the target yet).
    let c0 = flash_quad_color(&f0);

    let r0 = ui.recompute_count();
    let d0 = ui.draw_rebuild_count();
    let mid = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &flash_slots(target),
        &no_cells(),
        0.5,
    );
    // Per-channel eased halfway under linear: ~[0.5, 0.25, 0.125, 1.0].
    let mid_c = mid
        .quads
        .instances
        .iter()
        .map(|q| q.color)
        .find(|c| !colors_eq(*c, [0.0, 0.0, 0.0, 1.0]))
        .expect("an eased panel quad");
    assert!(
        mid_c[0] > 0.0 && mid_c[0] < 1.0 && mid_c[1] > 0.0 && mid_c[1] < 0.5,
        "panel eased per channel mid-flight: {mid_c:?}",
    );
    assert_eq!(
        ui.recompute_count(),
        r0,
        "an in-flight panel tween must NOT relayout",
    );
    assert!(
        ui.draw_rebuild_count() > d0,
        "an in-flight panel tween rebuilds the draw list (redraw)",
    );
    let _ = c0;
}

#[test]
fn panel_tween_eases_alpha_channel_and_settles_exactly() {
    // The panel tween eases all four channels (alpha included) and settles at
    // the exact target past the duration.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: tweened_panel_in_stack(
            [0.0, 0.0, 0.0, 1.0],
            "intro.flashColor",
            PanelTween {
                duration_ms: 1000.0,
                easing: Easing::Linear,
                from: Some([0.0, 0.0, 0.0, 0.0]),
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
    let target = [0.2, 0.4, 0.6, 1.0];

    // Mid-flight: alpha is between the from (0.0) and target (1.0).
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &flash_slots(target),
        &no_cells(),
        0.0,
    );
    let mid = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &flash_slots(target),
        &no_cells(),
        0.5,
    );
    let mid_c = mid
        .quads
        .instances
        .iter()
        .map(|q| q.color)
        .find(|c| c[3] > 0.0 && c[3] < 1.0)
        .expect("a panel quad with eased mid alpha");
    assert!(
        (0.4..=0.6).contains(&mid_c[3]),
        "alpha eased ~0.5 mid-flight under linear, got {}",
        mid_c[3],
    );

    // Past duration: settles to the exact target.
    let end = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &flash_slots(target),
        &no_cells(),
        2.0,
    );
    let end_c = flash_quad_color(&end).expect("a settled panel quad");
    assert!(
        end_c.iter().zip(target.iter()).all(|(a, b)| approx(*a, *b)),
        "panel settles at the exact target {target:?}, got {end_c:?}",
    );
}

#[test]
fn panel_tween_advances_through_rapid_retargets() {
    // As with a Bar's numeric display, a panel whose color target changes every
    // 20ms must advance its old segment before retargeting. Each interval is
    // shorter than the 90ms duration, so stale-display restarts would leave the
    // panel at its initial black fill indefinitely.
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: tweened_panel_in_stack(
            [0.0, 0.0, 0.0, 1.0],
            "intro.flashColor",
            PanelTween {
                duration_ms: 90.0,
                easing: Easing::Linear,
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

    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &flash_slots([0.0, 0.0, 0.0, 1.0]),
        &no_cells(),
        0.0,
    );
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &flash_slots([0.25, 0.0, 0.0, 1.0]),
        &no_cells(),
        0.02,
    );
    let second = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &flash_slots([0.5, 0.0, 0.0, 1.0]),
        &no_cells(),
        0.04,
    );
    let second_color = flash_quad_color(&second).expect("rapid retarget emits a non-black panel");
    let third = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &flash_slots([0.75, 0.0, 0.0, 1.0]),
        &no_cells(),
        0.06,
    );
    let third_color =
        flash_quad_color(&third).expect("continued rapid retarget keeps the panel visible");

    assert!(
        second_color[0] > EPS && second_color[0] < 0.5 - EPS,
        "the second rapid retarget preserves visible red progress, got {second_color:?}"
    );
    assert!(
        third_color[0] > second_color[0] + EPS && third_color[0] < 0.75 - EPS,
        "successive rapid retargets continue advancing the displayed panel color: {second_color:?} -> {third_color:?}"
    );
}

#[test]
fn panel_tween_recovers_from_a_missing_slot_with_a_fresh_segment() {
    // A raw panel fallback must seed the next segment. If the prior target is
    // restored, the visible fallback color is the new segment's start—not the
    // old in-flight color.
    let fallback = [0.1, 0.2, 0.3, 1.0];
    let target = [1.0, 0.0, 0.0, 1.0];
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: tweened_panel_in_stack(
            fallback,
            "intro.flashColor",
            PanelTween {
                duration_ms: 1000.0,
                easing: Easing::Linear,
                from: Some(fallback),
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

    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &flash_slots(target),
        &no_cells(),
        0.0,
    );
    let mid = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &flash_slots(target),
        &no_cells(),
        0.5,
    );
    let mid_color = flash_quad_color(&mid).expect("mid-flight panel quad");
    assert!(
        mid_color[0] > fallback[0] + EPS,
        "initial segment advances before fallback"
    );

    let snapped = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &no_slots(),
        &no_cells(),
        0.5,
    );
    let snapped_color = flash_quad_color(&snapped).expect("literal fallback panel quad");
    assert!(
        snapped_color
            .iter()
            .zip(fallback.iter())
            .all(|(actual, expected)| approx(*actual, *expected)),
        "missing slot renders the raw fallback {fallback:?}, got {snapped_color:?}",
    );

    let recovered = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &flash_slots(target),
        &no_cells(),
        0.6,
    );
    let recovered_color = flash_quad_color(&recovered).expect("recovered panel quad");
    assert!(
        recovered_color
            .iter()
            .zip(fallback.iter())
            .all(|(actual, expected)| approx(*actual, *expected)),
        "recovery starts from the visible fallback {fallback:?}, got {recovered_color:?}",
    );
}
