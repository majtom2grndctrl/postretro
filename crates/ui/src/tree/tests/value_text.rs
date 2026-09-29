// A button's `valueText`: its visible text follows state while its id, and so
// its focus, stays put.

use super::common::*;
use crate::descriptor::ValueTextCase;

const FOLLOWS: &str = "accessibility.reduceMotionFollowsSystem";
const VALUE: &str = "accessibility.reduceMotion";

fn case(text: &str, when: &[(&str, bool)]) -> ValueTextCase {
    ValueTextCase {
        when: when
            .iter()
            .map(|(slot, value)| pred(slot, Some(PredicateValue::Boolean(*value))))
            .collect(),
        text: text.to_string(),
    }
}

/// A label on the left and one value button on the right, named by the label.
fn row() -> AnchoredTree {
    let mut value = button("reduceMotion", "ui.accessibility.cycle.reduceMotion");
    if let Widget::Button(b) = &mut value {
        b.label = None;
        b.labelled_by = Some("reduceMotionLabel".to_string());
        b.value_text = vec![
            case("SYSTEM (ON)", &[(FOLLOWS, true), (VALUE, true)]),
            case("SYSTEM (OFF)", &[(FOLLOWS, true), (VALUE, false)]),
            case("ON", &[(VALUE, true)]),
            case("OFF", &[]),
        ];
    }
    anchored(hstack(
        16.0,
        0.0,
        Align::Center,
        vec![text_id("REDUCE MOTION", "reduceMotionLabel"), value],
    ))
}

fn slots(follows: bool, value: bool) -> HashMap<String, SlotValue> {
    HashMap::from([
        (FOLLOWS.to_string(), SlotValue::Boolean(follows)),
        (VALUE.to_string(), SlotValue::Boolean(value)),
    ])
}

fn texts(data: &UiDrawData) -> Vec<&str> {
    data.texts.iter().map(|t| t.content.as_str()).collect()
}

#[test]
fn value_text_follows_state_and_the_control_keeps_its_id() {
    let tree = row();
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();

    let following = slots(true, true);
    let draw = ui.build_draw_data_retained(
        [1280, 720],
        &mut fs,
        &no_images(),
        &following,
        &no_cells(),
        0.0,
    );
    assert_eq!(texts(&draw), ["REDUCE MOTION", "SYSTEM (ON)"]);
    let before = ui.export_focus_rects(&tree, [1280, 720], &following, &no_cells());

    // One cycle step: player-set On. The same control shows the new value.
    let on = slots(false, true);
    let relayouts = ui.recompute_count();
    let draw =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &on, &no_cells(), 0.1);
    assert_eq!(texts(&draw), ["REDUCE MOTION", "ON"]);
    assert_eq!(
        ui.recompute_count(),
        relayouts + 1,
        "a text change re-measures"
    );
    let after = ui.export_focus_rects(&tree, [1280, 720], &on, &no_cells());
    let ids = |list: &FocusRectList| list.rects.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&before), ["reduceMotion"]);
    assert_eq!(
        ids(&after),
        ids(&before),
        "one control, one id, before and after"
    );

    // Off falls through to the default case.
    let off = slots(false, false);
    let draw =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &off, &no_cells(), 0.2);
    assert_eq!(texts(&draw), ["REDUCE MOTION", "OFF"]);
}

#[test]
fn a_settled_value_text_rebuilds_nothing() {
    let tree = row();
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let state = slots(true, false);
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &state, &no_cells(), 0.0);
    let relayouts = ui.recompute_count();
    let rebuilds = ui.draw_rebuild_count();
    for frame in 1..4 {
        ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &state,
            &no_cells(),
            f64::from(frame) * 0.1,
        );
    }
    assert_eq!(ui.recompute_count(), relayouts);
    assert_eq!(ui.draw_rebuild_count(), rebuilds);
}

#[test]
fn with_no_matching_case_the_button_shows_its_label() {
    let mut widget = button("mono", "ui.accessibility.cycle.monoAudio");
    if let Widget::Button(b) = &mut widget {
        b.label = Some("MONO".to_string());
        b.value_text = vec![case("ON", &[("accessibility.monoAudio", true)])];
    }
    let tree = anchored(widget);
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let off = HashMap::from([(
        "accessibility.monoAudio".to_string(),
        SlotValue::Boolean(false),
    )]);
    let draw =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &off, &no_cells(), 0.0);
    assert_eq!(texts(&draw), ["MONO"]);
}

#[test]
fn the_fresh_path_measures_and_draws_the_resolved_value_text() {
    // The non-retained build resolves `valueText` before layout, so the button
    // is sized for the text it draws — the same rect the retained path gives.
    let tree = row();
    let state = slots(true, true);
    let mut fs = font_system();
    let width = |list: &FocusRectList| list.rects[0].rect[2];

    let mut fresh = UiTree::from_descriptor(&tree, &theme());
    let draw = fresh.build_draw_data([1280, 720], &mut fs, &no_images(), &state);
    assert_eq!(texts(&draw), ["REDUCE MOTION", "SYSTEM (ON)"]);
    let fresh_rects = fresh.export_focus_rects(&tree, [1280, 720], &state, &no_cells());

    let mut retained = UiTree::from_descriptor(&tree, &theme());
    retained.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &state, &no_cells(), 0.0);
    let retained_rects = retained.export_focus_rects(&tree, [1280, 720], &state, &no_cells());
    assert_eq!(fresh_rects.rects[0].rect, retained_rects.rects[0].rect);

    // A shorter value measures narrower on the fresh path too.
    let off = slots(false, false);
    let mut short = UiTree::from_descriptor(&tree, &theme());
    let draw = short.build_draw_data([1280, 720], &mut fs, &no_images(), &off);
    assert_eq!(texts(&draw), ["REDUCE MOTION", "OFF"]);
    let short_rects = short.export_focus_rects(&tree, [1280, 720], &off, &no_cells());
    assert!(width(&short_rects) < width(&fresh_rects));
}
