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
        id: None,
        style_ranges,
        visible_when: None,
        role: None,
    })
}

fn bar(slot: &str, max: f32, style_ranges: Option<StyleRanges>) -> Widget {
    bar_with_max(slot, BarMax::Literal(max), style_ranges)
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
    use crate::render::ui::descriptor::{Easing, TextTween};
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
        id: None,
        style_ranges: None,
        visible_when: None,
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
