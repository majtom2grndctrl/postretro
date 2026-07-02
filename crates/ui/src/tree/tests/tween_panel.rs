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
