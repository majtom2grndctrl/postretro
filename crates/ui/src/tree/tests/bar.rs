// Bar fill fraction, styleRanges recolor, state-max, and tween easing.

use super::common::*;
fn bar_with_max(slot: &str, max: BarMax, style_ranges: Option<StyleRanges>) -> Widget {
    Widget::Bar(BarWidget {
        bind: SliderBind {
            source: BindSource::Slot { slot: slot.into() },
            tween: None,
        },
        max,
        fill: ColorValue::Literal([0.0, 1.0, 0.0, 1.0]),
        background: ColorValue::Literal([0.1, 0.1, 0.1, 1.0]),
        width: None,
        height: None,
        id: None,
        style_ranges,
        visible_when: None,
        exit_fade: None,
        role: None,
    })
}

fn bar(slot: &str, max: f32, style_ranges: Option<StyleRanges>) -> Widget {
    bar_with_max(slot, BarMax::Literal(max), style_ranges)
}

fn lifecycle_bar() -> Widget {
    Widget::Bar(BarWidget {
        bind: SliderBind {
            source: BindSource::Slot {
                slot: "player.reloadProgress".into(),
            },
            tween: None,
        },
        max: BarMax::Literal(1.0),
        fill: ColorValue::Literal([0.0, 1.0, 0.0, 1.0]),
        background: ColorValue::Literal([0.1, 0.1, 0.1, 1.0]),
        width: Some(120.0),
        height: Some(24.0),
        id: None,
        style_ranges: None,
        visible_when: Some(pred(
            "player.reloadActive",
            Some(PredicateValue::Boolean(true)),
        )),
        exit_fade: Some(BarExitFade { duration_ms: 500.0 }),
        role: None,
    })
}

fn reload_slots(active: bool, progress: f32) -> HashMap<String, SlotValue> {
    HashMap::from([
        (
            "player.reloadActive".to_string(),
            SlotValue::Boolean(active),
        ),
        (
            "player.reloadProgress".to_string(),
            SlotValue::Number(progress),
        ),
    ])
}

#[test]
fn bar_authored_size_is_used_instead_of_the_default() {
    let mut ui = UiTree::from_descriptor(&anchored(lifecycle_bar()), &theme());
    let mut fs = font_system();
    let draw = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &reload_slots(true, 1.0),
        &no_cells(),
        0.0,
    );
    let background = draw
        .quads
        .instances
        .first()
        .expect("visible bar background");
    // At this 1280×720 reference scale, logical pixels project 1:1.
    assert!(approx(background.rect[2], 120.0));
    assert!(approx(background.rect[3], 24.0));
}

#[test]
fn bar_exit_fade_hides_initial_false_and_retains_terminal_image_until_expiry() {
    let mut ui = UiTree::from_descriptor(&anchored(lifecycle_bar()), &theme());
    let mut fs = font_system();

    let initial_hidden = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &reload_slots(false, 0.0),
        &no_cells(),
        0.0,
    );
    assert!(
        initial_hidden.quads.instances.is_empty(),
        "false first resolution is non-rendering"
    );

    let active = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &reload_slots(true, 0.25),
        &no_cells(),
        0.1,
    );
    assert_eq!(
        active.quads.instances.len(),
        2,
        "active full-alpha bar draws background + fill"
    );
    assert!(approx(active.quads.instances[0].color[3], 1.0));
    assert!(approx(active.quads.instances[1].color[3], 1.0));

    // Completion snapshot publishes terminal progress and inactive together.
    let exit_start = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &reload_slots(false, 1.0),
        &no_cells(),
        0.2,
    );
    assert_eq!(exit_start.quads.instances.len(), 2);
    assert!(approx(
        exit_start.quads.instances[1].rect[2],
        exit_start.quads.instances[0].rect[2]
    ));
    assert!(approx(exit_start.quads.instances[0].color[3], 1.0));

    // The next gameplay frame may reset progress, but the retained fade keeps
    // the terminal full bar while alpha advances linearly from the UI clock.
    let mid_fade = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &reload_slots(false, 0.0),
        &no_cells(),
        0.45,
    );
    assert_eq!(mid_fade.quads.instances.len(), 2);
    assert!(approx(
        mid_fade.quads.instances[1].rect[2],
        mid_fade.quads.instances[0].rect[2]
    ));
    assert!(approx(mid_fade.quads.instances[0].color[3], 0.5));
    assert!(approx(mid_fade.quads.instances[1].color[3], 0.5));

    let expired = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &reload_slots(false, 0.0),
        &no_cells(),
        0.7,
    );
    assert!(
        expired.quads.instances.is_empty(),
        "expired fade emits no bar quads"
    );
}

#[test]
fn bar_exit_fade_retrigger_cancels_capture_and_restores_full_opacity() {
    let mut ui = UiTree::from_descriptor(&anchored(lifecycle_bar()), &theme());
    let mut fs = font_system();
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &reload_slots(true, 1.0),
        &no_cells(),
        0.0,
    );
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &reload_slots(false, 1.0),
        &no_cells(),
        0.1,
    );
    let retriggered = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &reload_slots(true, 0.25),
        &no_cells(),
        0.3,
    );
    assert_eq!(retriggered.quads.instances.len(), 2);
    assert!(approx(retriggered.quads.instances[0].color[3], 1.0));
    assert!(approx(retriggered.quads.instances[1].color[3], 1.0));
    assert!(
        approx(
            retriggered.quads.instances[1].rect[2],
            retriggered.quads.instances[0].rect[2] * 0.25
        ),
        "retrigger uses the fresh live target instead of stale terminal capture"
    );
}

/// A slot map binding `player.health` to a Number value.
fn health_slots(value: f32) -> HashMap<String, SlotValue> {
    let mut m = HashMap::new();
    m.insert("player.health".to_string(), SlotValue::Number(value));
    m
}

fn health_slots_with_max(value: f32, max: f32) -> HashMap<String, SlotValue> {
    let mut m = health_slots(value);
    m.insert("player.maxHealth".to_string(), SlotValue::Number(max));
    m
}

#[test]
fn bar_fill_fraction_is_value_over_max_clamped() {
    // A bar with max 100 and value 50 draws a fill quad half the background's
    // width; value 150 clamps to the full width (fraction 1).
    let tree = anchored(bar("player.health", 100.0, None));

    for (value, expected_fraction) in [(50.0_f32, 0.5_f32), (150.0, 1.0), (0.0, 0.0)] {
        let mut ui = UiTree::from_descriptor(&tree, &theme());
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &health_slots(value));
        // The background quad is always present (first); the fill quad follows
        // only when the fraction is > 0.
        let background = &data.quads.instances[0];
        let bg_width = background.rect[2];
        if expected_fraction == 0.0 {
            assert_eq!(
                data.quads.instances.len(),
                1,
                "zero fraction draws no fill quad"
            );
        } else {
            let fill = &data.quads.instances[1];
            let expected_width = (bg_width * expected_fraction).round();
            assert!(
                approx(fill.rect[2], expected_width),
                "value {value}: fill width {} ≈ {expected_width} (fraction {expected_fraction})",
                fill.rect[2],
            );
            // Fill shares the background's top-left and height.
            assert!(approx(fill.rect[0], background.rect[0]));
            assert!(approx(fill.rect[1], background.rect[1]));
            assert!(approx(fill.rect[3], background.rect[3]));
        }
    }
}

#[test]
fn retained_reload_meter_fill_grows_while_background_stays_fixed() {
    // Models the dev HUD's named reload meter at the smallest CPU seam: a
    // state-bound Bar with `max: 1`, rebuilt through the retained path as its
    // published `player.reloadProgress` slot changes.
    let tree = anchored(bar("player.reloadProgress", 1.0, None));
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();

    let mut empty = HashMap::new();
    empty.insert("player.reloadProgress".to_string(), SlotValue::Number(0.0));
    let zero =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &empty, &no_cells(), 0.0);
    assert_eq!(
        zero.quads.instances.len(),
        1,
        "zero progress has no fill quad"
    );
    let zero_background = zero.quads.instances[0];

    let mut full = HashMap::new();
    full.insert("player.reloadProgress".to_string(), SlotValue::Number(1.0));
    let one =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &full, &no_cells(), 0.1);
    assert_eq!(
        one.quads.instances.len(),
        2,
        "full progress emits a fill quad"
    );
    let one_background = one.quads.instances[0];
    let fill = one.quads.instances[1];
    for index in 0..4 {
        assert!(
            approx(zero_background.rect[index], one_background.rect[index]),
            "background rect component {index} stays fixed: {} vs {}",
            zero_background.rect[index],
            one_background.rect[index],
        );
    }
    assert!(
        approx(fill.rect[2], one_background.rect[2]),
        "full reload progress fills the full background width: {} vs {}",
        fill.rect[2],
        one_background.rect[2],
    );
    assert!(
        approx(fill.rect[3], one_background.rect[3]),
        "fill keeps the background height: {} vs {}",
        fill.rect[3],
        one_background.rect[3],
    );
}

#[test]
fn retained_tweened_bar_rebuilds_when_its_numeric_slot_changes() {
    // Regression: a retained Bar with a no-`from` tween must redraw from a
    // later snapshot instead of keeping its initial empty fill.
    let mut widget = bar("player.reloadProgress", 1.0, None);
    let Widget::Bar(bar) = &mut widget else {
        unreachable!("bar helper returns a Bar widget");
    };
    bar.bind.tween = Some(TextTween {
        duration_ms: 90.0,
        easing: Easing::EaseOut,
        from: None,
    });
    let tree = anchored(widget);
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();

    let mut zero = HashMap::new();
    zero.insert("player.reloadProgress".to_string(), SlotValue::Number(0.0));
    let empty =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &zero, &no_cells(), 0.0);
    assert_eq!(empty.quads.instances.len(), 1);

    let mut quarter = HashMap::new();
    quarter.insert("player.reloadProgress".to_string(), SlotValue::Number(0.25));
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &quarter,
        &no_cells(),
        0.1,
    );
    let mut half = HashMap::new();
    half.insert("player.reloadProgress".to_string(), SlotValue::Number(0.5));
    let filled =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &half, &no_cells(), 0.2);
    assert_eq!(filled.quads.instances.len(), 2);
    assert!(approx(
        filled.quads.instances[1].rect[2],
        filled.quads.instances[0].rect[2] * 0.25
    ));
}

#[test]
fn tweened_bar_recovers_from_a_missing_slot_with_a_fresh_segment() {
    // A raw fallback must replace an in-flight tween segment. Otherwise when the
    // same target returns, the bar resumes its pre-fallback display and jumps
    // ahead of its visible zero fill.
    let mut widget = bar("player.reloadProgress", 1.0, None);
    let Widget::Bar(bar) = &mut widget else {
        unreachable!("bar helper returns a Bar widget");
    };
    bar.bind.tween = Some(TextTween {
        duration_ms: 1000.0,
        easing: Easing::Linear,
        from: Some(0.0),
    });
    let tree = anchored(widget);
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();

    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("player.reloadProgress", 1.0),
        &no_cells(),
        0.0,
    );
    let mid = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("player.reloadProgress", 1.0),
        &no_cells(),
        0.5,
    );
    assert!(
        mid.quads.instances[1].rect[2] / mid.quads.instances[0].rect[2] > 0.49,
        "the initial segment is mid-flight before the fallback"
    );

    let fallback = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &no_slots(),
        &no_cells(),
        0.5,
    );
    assert_eq!(
        fallback.quads.instances.len(),
        1,
        "missing slot draws raw zero fill"
    );

    let recovered = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("player.reloadProgress", 1.0),
        &no_cells(),
        0.6,
    );
    assert_eq!(
        recovered.quads.instances.len(),
        1,
        "recovery starts a fresh segment at the visible fallback instead of resuming mid-flight",
    );
}

#[test]
fn retained_tweened_bar_advances_through_rapid_retargets() {
    // Each target arrives 20ms after the last, well before this 90ms tween can
    // settle. The retained display must carry forward the previous segment's
    // progress; restarting from its stale prior-frame display would keep the
    // bar at zero and emit no fill quad.
    let mut widget = bar("player.reloadProgress", 1.0, None);
    let Widget::Bar(bar) = &mut widget else {
        unreachable!("bar helper returns a Bar widget");
    };
    bar.bind.tween = Some(TextTween {
        duration_ms: 90.0,
        easing: Easing::Linear,
        from: None,
    });
    let tree = anchored(widget);
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();

    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("player.reloadProgress", 0.0),
        &no_cells(),
        0.0,
    );
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("player.reloadProgress", 0.25),
        &no_cells(),
        0.02,
    );
    let second = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("player.reloadProgress", 0.5),
        &no_cells(),
        0.04,
    );
    let second_fraction = second.quads.instances[1].rect[2] / second.quads.instances[0].rect[2];
    let third = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("player.reloadProgress", 0.75),
        &no_cells(),
        0.06,
    );
    let third_fraction = third.quads.instances[1].rect[2] / third.quads.instances[0].rect[2];

    assert!(
        second_fraction > EPS && second_fraction < 0.5 - EPS,
        "the second rapid retarget preserves visible in-flight progress, got {second_fraction}"
    );
    assert!(
        third_fraction > second_fraction + EPS && third_fraction < 0.75 - EPS,
        "successive rapid retargets continue advancing the displayed fill: {second_fraction} -> {third_fraction}"
    );
}

#[test]
fn bar_style_ranges_recolor_the_fill() {
    // A health bar with a red ≤ 0.25 normalized band: at 10/100 the fill quad
    // is red, not the base green. Bar styleRanges evaluate the displayed fill
    // fraction so authored bands can stay normalized even when max is a state
    // reference.
    let ranges = StyleRanges {
        max: 1.0,
        entries: vec![
            StyleEntry {
                up_to: Some(0.25),
                color: Some(ColorValue::Literal([1.0, 0.0, 0.0, 1.0])),
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
    };
    let tree = anchored(bar("player.health", 100.0, Some(ranges)));
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &health_slots(10.0));
    let fill = &data.quads.instances[1];
    assert!(
        approx(fill.color[0], 1.0) && approx(fill.color[1], 0.0),
        "low health recolors the fill red, got {:?}",
        fill.color
    );
}

#[test]
fn retained_bar_state_max_change_rebuilds_fill_and_style_without_relayout() {
    let ranges = StyleRanges {
        max: 1.0,
        entries: vec![
            StyleEntry {
                up_to: Some(0.25),
                color: Some(ColorValue::Literal([1.0, 0.0, 0.0, 1.0])),
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
    };
    let tree = anchored(bar_with_max(
        "player.health",
        BarMax::State(BarMaxStateRef {
            slot: "player.maxHealth".into(),
        }),
        Some(ranges),
    ));
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();

    let first = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &health_slots_with_max(50.0, 100.0),
        &no_cells(),
        0.0,
    );
    assert_eq!(ui.recompute_count(), 1, "first frame computes layout");
    assert_eq!(ui.draw_rebuild_count(), 1, "first frame builds draw data");
    let first_background = &first.quads.instances[0];
    let first_fill = &first.quads.instances[1];
    assert!(
        approx(first_fill.rect[2], (first_background.rect[2] * 0.5).round()),
        "50/100 draws a half-width fill",
    );
    assert!(
        approx(first_fill.color[0], 0.0) && approx(first_fill.color[1], 1.0),
        "50/100 uses the healthy band, got {:?}",
        first_fill.color,
    );

    let second = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &health_slots_with_max(50.0, 200.0),
        &no_cells(),
        0.0,
    );
    assert_eq!(
        ui.recompute_count(),
        1,
        "max-only bar changes are appearance-only",
    );
    assert_eq!(
        ui.draw_rebuild_count(),
        2,
        "state-backed max changes must invalidate cached bar draw data",
    );
    let second_background = &second.quads.instances[0];
    let second_fill = &second.quads.instances[1];
    assert!(
        approx(
            second_fill.rect[2],
            (second_background.rect[2] * 0.25).round()
        ),
        "50/200 redraws at quarter width, got {} of {}",
        second_fill.rect[2],
        second_background.rect[2],
    );
    assert!(
        approx(second_fill.color[0], 1.0) && approx(second_fill.color[1], 0.0),
        "50/200 crosses into the critical band, got {:?}",
        second_fill.color,
    );
}

#[test]
fn bar_bind_tween_eases_the_displayed_fraction() {
    // A bar bind carrying a tween eases the displayed value toward each new
    // target. Retained path: from a full 100 health, retarget to 0 over 1000ms;
    // mid-tween (500ms, linear) the displayed value is ~50, so the fill width is
    // ~half — not the snapped 0.
    use crate::descriptor::{Easing, TextTween};
    let tree = anchored(Widget::Bar(BarWidget {
        bind: SliderBind {
            source: BindSource::Slot {
                slot: "player.health".into(),
            },
            tween: Some(TextTween {
                duration_ms: 1000.0,
                easing: Easing::Linear,
                from: None,
            }),
        },
        max: BarMax::Literal(100.0),
        fill: ColorValue::Literal([0.0, 1.0, 0.0, 1.0]),
        background: ColorValue::Literal([0.1, 0.1, 0.1, 1.0]),
        width: None,
        height: None,
        id: None,
        style_ranges: None,
        visible_when: None,
        exit_fade: None,
        role: None,
    }));
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    // Frame 0: first resolution at full health (no `from`, snaps to 100).
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &health_slots(100.0),
        &no_cells(),
        0.0,
    );
    // Frame 1: retarget to 0 at t=0 — the segment starts easing from 100.
    ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &health_slots(0.0),
        &no_cells(),
        0.0,
    );
    // Frame 2: half the duration later, the eased display is ~50 (linear).
    let data = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &health_slots(0.0),
        &no_cells(),
        0.5,
    );
    let bg_width = data.quads.instances[0].rect[2];
    let fill_width = data.quads.instances[1].rect[2];
    let fraction = fill_width / bg_width;
    assert!(
        (fraction - 0.5).abs() < 0.05,
        "mid-tween fill fraction eases to ~0.5, got {fraction}"
    );
}
