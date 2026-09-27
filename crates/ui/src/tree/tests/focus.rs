// Focus-rect export, interaction metadata, predicate, and a11y readback.

use super::common::*;

use crate::modal_stack::{ModalStack, ScopeTier};
use log::Level;
use postretro_test_log_capture::LogCapture;

#[test]
fn focus_export_lists_ids_rects_and_a_linear_group() {
    use crate::descriptor::{FocusKind, FocusPolicy};
    // A vstack declaring a linear focus policy over three buttons.
    let root = Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(10.0),
        padding: SpacingValue::Literal(0.0),
        align: Align::Start,
        width: None,
        fill: None,
        border: None,
        id: None,
        focus_neighbors: crate::descriptor::FocusNeighbors::default(),
        focus: Some(FocusPolicy::Shorthand(FocusKind::Linear)),
        restore_on_return: false,
        local_state: None,
        visible_when: None,
        role: None,
        children: vec![
            button("a", "pressA"),
            button("b", "pressB"),
            button("c", "pressC"),
        ],
    });
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root,
        capture_mode: CaptureMode::Passthrough,
        initial_focus: Some("b".to_string()),
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let draw = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    let focus = ui.export_focus_rects(&tree, [1280, 720], &no_slots(), &no_cells());

    // Three focusable buttons, one linear group with all three as members.
    let ids: Vec<&str> = focus.rects.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"], "ids in tree order");
    assert_eq!(focus.groups.len(), 1);
    assert_eq!(focus.groups[0].kind, crate::tree::FocusKind::Linear);
    assert!(focus.groups[0].wrap, "shorthand defaults wrap on");
    assert_eq!(focus.groups[0].members, vec![0, 1, 2]);
    assert_eq!(focus.initial_focus.as_deref(), Some("b"));

    // z rises in tree order so a later node hit-tests as topmost.
    assert!(focus.rects[0].z < focus.rects[1].z && focus.rects[1].z < focus.rects[2].z);

    // The exported rect uses the SAME device-pixel projection as the draw: each
    // button's rect [x, y] matches its drawn label run position.
    for (i, run) in draw.texts.iter().enumerate() {
        assert!(
            approx(focus.rects[i].rect[0], run.position[0])
                && approx(focus.rects[i].rect[1], run.position[1]),
            "focus rect {i} top-left matches the drawn run position",
        );
    }
}

/// A linear-wrap focus group vstack over `children` (no authored id).
fn linear_group(children: Vec<Widget>) -> Widget {
    use crate::descriptor::{FocusKind, FocusPolicy};
    let mut root = vstack(0.0, 0.0, Align::Start, children);
    if let Widget::VStack(stack) = &mut root {
        stack.focus = Some(FocusPolicy::Shorthand(FocusKind::Linear));
    }
    root
}

fn export(tree: &AnchoredTree) -> FocusRectList {
    let mut ui = UiTree::from_descriptor(tree, &theme());
    let mut fs = font_system();
    ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    ui.export_focus_rects(tree, [1280, 720], &no_slots(), &no_cells())
}

// Regression: every passive node under a focus group (menu title, section label,
// nested layout stack) exported as a focus stop, so nav landed on the title.
#[test]
fn focus_export_skips_passive_nodes_inside_a_focus_group() {
    let tree = anchored(linear_group(vec![
        text("TITLE", 20.0),
        button("b1", "one"),
        vstack(
            0.0,
            0.0,
            Align::Start,
            vec![text("SECTION", 20.0), button("b2", "two")],
        ),
        button("b3", "three"),
    ]));
    let focus = export(&tree);

    let ids: Vec<&str> = focus.rects.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["b1", "b2", "b3"], "only the buttons are focus stops");
    assert_eq!(focus.groups.len(), 1);
    assert_eq!(
        focus.groups[0].members,
        vec![0, 1, 2],
        "all three buttons join the root group, in tree order",
    );
    assert!(focus.rects.iter().all(|r| r.group == Some(0)));
    assert!(
        focus.rects.iter().all(|r| r.interaction.is_some()),
        "every exported rect is interactive",
    );
}

#[test]
fn focus_export_skips_passive_node_with_authored_id_outside_a_group() {
    // A passive id is a `labelledBy` reference target, never a focus stop.
    let tree = anchored(vstack(
        0.0,
        0.0,
        Align::Start,
        vec![text_id("VOLUME", "volumeLabel"), button("go", "goNow")],
    ));
    let focus = export(&tree);

    let ids: Vec<&str> = focus.rects.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["go"], "the id-bearing text is not exported");
    assert!(focus.groups.is_empty());
}

#[test]
fn focus_export_nested_interactive_widgets_join_the_enclosing_group() {
    // The options-menu shape: a two-column grid (label | control) inside a
    // section stack inside the focus group. The grid declares no policy, so its
    // slider and button join the outer group; the labels do not.
    let grid = Widget::Grid(GridWidget {
        cols: 2,
        gap: SpacingValue::Literal(0.0),
        padding: SpacingValue::Literal(0.0),
        align: Align::Start,
        id: None,
        focus_neighbors: Default::default(),
        focus: None,
        restore_on_return: false,
        visible_when: None,
        role: None,
        children: vec![
            text_id("VOLUME", "volLabel"),
            slider("vol", "audio.master", &["nav.left", "nav.right"]),
            text_id("MUTE", "muteLabel"),
            button("mute", "toggleMute"),
        ],
    });
    let tree = anchored(linear_group(vec![
        text("OPTIONS", 20.0),
        vstack(0.0, 0.0, Align::Start, vec![text("AUDIO", 20.0), grid]),
        button("back", "closeOptions"),
    ]));
    let focus = export(&tree);

    let ids: Vec<&str> = focus.rects.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["vol", "mute", "back"]);
    assert_eq!(focus.groups.len(), 1, "passive containers open no group");
    assert_eq!(focus.groups[0].members, vec![0, 1, 2]);
    assert!(matches!(
        focus.rects[0].interaction,
        Some(NodeInteraction::Slider { .. })
    ));
    assert!(matches!(
        focus.rects[1].interaction,
        Some(NodeInteraction::Button { .. })
    ));
}

#[test]
fn focus_export_nested_policy_container_opens_its_own_group() {
    // The innermost focus-policy ancestor wins: the inner button joins the inner
    // group only, while the outer buttons on either side join the outer group.
    let inner_group = linear_group(vec![text("INNER", 20.0), button("inner", "pressInner")]);
    let tree = anchored(linear_group(vec![
        button("before", "pressBefore"),
        vstack(
            0.0,
            0.0,
            Align::Start,
            vec![text("SECTION", 20.0), inner_group],
        ),
        button("after", "pressAfter"),
    ]));
    let focus = export(&tree);

    let ids: Vec<&str> = focus.rects.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["before", "inner", "after"]);
    assert_eq!(focus.groups.len(), 2);
    assert_eq!(
        focus.rects[1].group,
        Some(1),
        "inner button carries the inner group"
    );
    assert_eq!(focus.groups[1].members, vec![1]);
    assert_eq!(
        focus.groups[0].members,
        vec![0, 2],
        "the outer group holds its own buttons, not the nested group's",
    );
    assert_eq!(focus.rects[0].group, Some(0));
    assert_eq!(focus.rects[2].group, Some(0));
}

#[test]
fn focus_export_passive_only_group_exports_an_empty_group_and_no_rects() {
    let tree = anchored(linear_group(vec![
        text("TITLE", 20.0),
        vstack(0.0, 0.0, Align::Start, vec![text_id("NOTE", "note")]),
    ]));
    let focus = export(&tree);

    assert!(
        focus.rects.is_empty(),
        "passive nodes are never focus stops"
    );
    assert_eq!(
        focus.groups.len(),
        1,
        "the declaring container still opens a group"
    );
    assert!(focus.groups[0].members.is_empty());
}

// --- Interactive widgets ---

fn slider(id: &str, slot: &str, captures: &[&str]) -> Widget {
    Widget::Slider(SliderWidget {
        id: id.into(),
        label: Some("Vol".into()),
        labelled_by: None,
        bind: SliderBind {
            source: BindSource::Slot { slot: slot.into() },
            tween: None,
        },
        min: 0.0,
        max: 1.0,
        step: 0.1,
        value_display: None,
        captures_nav: captures.iter().map(|s| s.to_string()).collect(),
        focus_neighbors: Default::default(),
        disabled: false,
        visible_when: None,
        role: None,
    })
}

#[test]
fn button_exports_focusable_rect_with_activation_interaction() {
    // A button always exports as focusable (required id) carrying its onPress
    // activation — the seam the app fires on a focus-engine confirm/click.
    let tree = anchored(vstack(
        0.0,
        0.0,
        Align::Start,
        vec![button("resume", "resumeGame")],
    ));
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    let focus = ui.export_focus_rects(&tree, [1280, 720], &no_slots(), &no_cells());
    let rect = focus
        .rects
        .iter()
        .find(|r| r.id == "resume")
        .expect("button is focusable");
    assert_eq!(
        rect.interaction,
        Some(NodeInteraction::Button {
            on_press: "resumeGame".to_string(),
            repeat_on_hold: None,
        }),
        "button carries its onPress activation"
    );
}

#[test]
fn button_label_uses_literal_white_default_color() {
    // Regression: interactive labels used the removed `body` color token and
    // therefore degraded to opaque magenta under the engine default theme.
    let tree = anchored(button("resume", "resumeGame"));
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());

    assert_eq!(data.texts.len(), 1);
    assert_eq!(
        data.texts[0].color,
        srgb_of(INTERACTIVE_LABEL_COLOR),
        "button label uses the renderer-owned literal white default",
    );
}

#[test]
fn slider_exports_focusable_rect_with_step_interaction() {
    // A slider always exports as focusable carrying its bound-value step params
    // and capturesNav wire names — the app drives the value step from these.
    let tree = anchored(vstack(
        0.0,
        0.0,
        Align::Start,
        vec![slider("vol", "audio.master", &["nav.left", "nav.right"])],
    ));
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    let focus = ui.export_focus_rects(&tree, [1280, 720], &no_slots(), &no_cells());
    let rect = focus
        .rects
        .iter()
        .find(|r| r.id == "vol")
        .expect("slider is focusable");
    assert_eq!(
        rect.interaction,
        Some(NodeInteraction::Slider {
            slot: "audio.master".to_string(),
            min: 0.0,
            max: 1.0,
            step: 0.1,
            captures_nav: vec!["nav.left".to_string(), "nav.right".to_string()],
        }),
    );
}

#[test]
fn slider_label_uses_literal_white_default_color() {
    // Regression: interactive labels used the removed `body` color token and
    // therefore degraded to opaque magenta under the engine default theme.
    let tree = anchored(slider("vol", "audio.master", &[]));
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());

    assert_eq!(data.texts.len(), 1);
    assert_eq!(
        data.texts[0].color,
        srgb_of(INTERACTIVE_LABEL_COLOR),
        "slider label uses the renderer-owned literal white default",
    );
}

#[test]
fn labelled_by_slider_renders_value_without_an_empty_label_prefix() {
    let mut value_only = slider("sensitivity", "options.mouseSensitivity", &[]);
    let Widget::Slider(slider) = &mut value_only else {
        unreachable!("slider helper returns a slider")
    };
    slider.label = None;
    slider.labelled_by = Some("sensitivityLabel".into());
    let tree = anchored(value_only);
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let data = ui.build_draw_data(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("options.mouseSensitivity", 0.002),
    );

    assert_eq!(data.texts.len(), 1);
    assert_eq!(data.texts[0].content, "0.002");
}

// --- M13 G2: predicate resolution + a11y state + FocusRect.disabled ---

fn bool_slots(slot: &str, value: bool) -> HashMap<String, SlotValue> {
    let mut m = HashMap::new();
    m.insert(slot.to_string(), SlotValue::Boolean(value));
    m
}

#[test]
fn resolve_predicate_boolean_no_equals_is_truthiness() {
    // A bare (no-`equals`) predicate over a Boolean source resolves to its
    // truthiness: 1.0 when true, 0.0 when false.
    let p = pred("flag.on", None);
    assert_eq!(
        resolve_predicate(
            &p.source,
            None,
            None,
            &bool_slots("flag.on", true),
            &no_cells()
        ),
        1.0,
    );
    assert_eq!(
        resolve_predicate(
            &p.source,
            None,
            None,
            &bool_slots("flag.on", false),
            &no_cells()
        ),
        0.0,
    );
}

#[test]
fn resolve_predicate_non_boolean_no_equals_is_zero() {
    // A bare predicate over a non-Boolean source has no defined truthiness → 0.0.
    let p = pred("player.health", None);
    assert_eq!(
        resolve_predicate(
            &p.source,
            None,
            None,
            &number_slots("player.health", 100.0),
            &no_cells(),
        ),
        0.0,
    );
}

#[test]
fn resolve_predicate_equals_matches_and_mismatches() {
    // With `equals`, the predicate is 1.0 iff the resolved value equals the
    // comparand (number exact), else 0.0.
    let p = pred("hud.tab", Some(PredicateValue::Number(2.0)));
    let comparand = PredicateValue::Number(2.0);
    assert_eq!(
        resolve_predicate(
            &p.source,
            Some(&comparand),
            None,
            &number_slots("hud.tab", 2.0),
            &no_cells(),
        ),
        1.0,
        "exact number match → 1.0",
    );
    assert_eq!(
        resolve_predicate(
            &p.source,
            Some(&comparand),
            None,
            &number_slots("hud.tab", 3.0),
            &no_cells(),
        ),
        0.0,
        "number mismatch → 0.0",
    );
}

#[test]
fn resolve_predicate_string_and_enum_match_by_name() {
    // A String comparand matches both a String slot and an Enum slot by name.
    let comparand = PredicateValue::String("stats".into());
    let source = BindSource::Slot {
        slot: "hud.tab".into(),
    };
    let mut string_slot = HashMap::new();
    string_slot.insert("hud.tab".to_string(), SlotValue::String("stats".into()));
    assert_eq!(
        resolve_predicate(&source, Some(&comparand), None, &string_slot, &no_cells()),
        1.0,
        "String slot matches by name",
    );
    let mut enum_slot = HashMap::new();
    enum_slot.insert("hud.tab".to_string(), SlotValue::Enum("stats".into()));
    assert_eq!(
        resolve_predicate(&source, Some(&comparand), None, &enum_slot, &no_cells()),
        1.0,
        "Enum slot matches by name",
    );
    let mut other = HashMap::new();
    other.insert("hud.tab".to_string(), SlotValue::Enum("inventory".into()));
    assert_eq!(
        resolve_predicate(&source, Some(&comparand), None, &other, &no_cells()),
        0.0,
        "by-name mismatch → 0.0",
    );
}

#[test]
fn resolve_predicate_type_mismatch_is_zero() {
    // A type mismatch (Number slot vs String comparand) does not match → 0.0.
    let source = BindSource::Slot {
        slot: "hud.tab".into(),
    };
    let comparand = PredicateValue::String("stats".into());
    assert_eq!(
        resolve_predicate(
            &source,
            Some(&comparand),
            None,
            &number_slots("hud.tab", 1.0),
            &no_cells(),
        ),
        0.0,
    );
    // An absent slot also resolves to 0.0 (no value to compare).
    assert_eq!(
        resolve_predicate(&source, Some(&comparand), None, &no_slots(), &no_cells()),
        0.0,
        "absent slot → 0.0",
    );
}

/// A button carrying a `Predicate` `bind` + a styleRanges map that highlights
/// the label when the predicate is true (value 1.0) vs false (0.0).
fn predicate_button(id: &str, bind: Predicate) -> Widget {
    // Two bands: value < 0.5 → unselected gray; value >= 0.5 → selected cyan.
    let ranges = StyleRanges {
        max: 1.0,
        entries: vec![
            StyleEntry {
                up_to: Some(0.5),
                color: Some(ColorValue::Literal([0.2, 0.2, 0.2, 1.0])),
                pulse: None,
                flash: None,
            },
            StyleEntry {
                up_to: None,
                color: Some(ColorValue::Literal([0.0, 1.0, 1.0, 1.0])),
                pulse: None,
                flash: None,
            },
        ],
    };
    Widget::Button(ButtonWidget {
        id: id.into(),
        label: Some(id.into()),
        labelled_by: None,
        on_press: "noop".into(),
        focus_neighbors: Default::default(),
        repeat_on_hold: None,
        selected: None,
        checked: None,
        bind: Some(bind),
        style_ranges: Some(ranges),
        disabled: false,
        visible_when: None,
        role: None,
    })
}

#[test]
fn button_predicate_bind_drives_style_ranges_highlight() {
    // A tab Button whose `bind` Predicate matches self-highlights through its
    // styleRanges (the author-wired highlight, no new visual primitive): the
    // label color tracks the predicate's 0/1 value.
    let tree = anchored(vstack(
        0.0,
        0.0,
        Align::Start,
        vec![predicate_button(
            "tab.stats",
            pred("hud.tab", Some(PredicateValue::Number(1.0))),
        )],
    ));
    let mut fs = font_system();

    // Predicate true → value 1.0 → trailing cyan band.
    let mut ui_on = UiTree::from_descriptor(&tree, &theme());
    let on = ui_on.build_draw_data(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("hud.tab", 1.0),
    );
    assert_eq!(
        on.texts[0].color,
        srgb_of([0.0, 1.0, 1.0, 1.0]),
        "matching predicate (1.0) highlights with the cyan band",
    );

    // Predicate false → value 0.0 → first (gray) band.
    let mut ui_off = UiTree::from_descriptor(&tree, &theme());
    let off = ui_off.build_draw_data(
        [1280, 720],
        &mut fs,
        &no_images(),
        &number_slots("hud.tab", 2.0),
    );
    assert_eq!(
        off.texts[0].color,
        srgb_of([0.2, 0.2, 0.2, 1.0]),
        "non-matching predicate (0.0) draws the unselected band",
    );
}

// Regression: option radio highlights stayed stale until the menu was closed and reopened.
#[test]
fn retained_button_predicate_change_repaints_selection_without_relayout() {
    let tree = anchored(hstack(
        6.0,
        0.0,
        Align::Start,
        vec![
            predicate_button(
                "shadow.low",
                pred(
                    "options.shadowQuality",
                    Some(PredicateValue::String("low".into())),
                ),
            ),
            predicate_button(
                "shadow.high",
                pred(
                    "options.shadowQuality",
                    Some(PredicateValue::String("high".into())),
                ),
            ),
        ],
    ));
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let mut slots = HashMap::from([(
        "options.shadowQuality".to_string(),
        SlotValue::Enum("low".into()),
    )]);

    let first =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);
    assert_eq!(ui.recompute_count(), 1, "first frame computes layout once");
    assert_eq!(ui.draw_rebuild_count(), 1, "first frame builds draw data");
    assert_eq!(first.texts[0].color, srgb_of([0.0, 1.0, 1.0, 1.0]));
    assert_eq!(first.texts[1].color, srgb_of([0.2, 0.2, 0.2, 1.0]));

    slots.insert(
        "options.shadowQuality".to_string(),
        SlotValue::Enum("high".into()),
    );
    let changed =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 1.0);
    assert_eq!(
        ui.recompute_count(),
        1,
        "predicate-only appearance changes must not relayout",
    );
    assert_eq!(
        ui.draw_rebuild_count(),
        2,
        "the changed predicate rebuilds cached draw data",
    );
    assert_eq!(changed.texts[0].color, srgb_of([0.2, 0.2, 0.2, 1.0]));
    assert_eq!(changed.texts[1].color, srgb_of([0.0, 1.0, 1.0, 1.0]));

    let settled =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 2.0);
    assert_eq!(ui.recompute_count(), 1, "settled frame still skips layout");
    assert_eq!(
        ui.draw_rebuild_count(),
        2,
        "unchanged predicate reuses cached draw data",
    );
    assert_eq!(settled.texts[0].color, changed.texts[0].color);
    assert_eq!(settled.texts[1].color, changed.texts[1].color);
}

/// A button declaring `selected`/`checked` predicates and a `disabled` bit, for
/// the focus-rect a11y readback test.
fn a11y_button(
    id: &str,
    selected: Option<Predicate>,
    checked: Option<Predicate>,
    disabled: bool,
) -> Widget {
    Widget::Button(ButtonWidget {
        id: id.into(),
        label: Some(id.into()),
        labelled_by: None,
        on_press: "noop".into(),
        focus_neighbors: Default::default(),
        repeat_on_hold: None,
        selected,
        checked,
        bind: None,
        style_ranges: None,
        disabled,
        visible_when: None,
        role: None,
    })
}

#[test]
fn focus_rect_carries_resolved_selected_checked_and_disabled() {
    // selected/checked predicates resolve in the focus-rect build and ride the
    // exported FocusRectList as a11y metadata; the disabled bit is populated
    // from the widget. The engine draws no highlight from selected/checked.
    let tree = anchored(vstack(
        0.0,
        0.0,
        Align::Start,
        vec![
            a11y_button(
                "tab.stats",
                Some(pred("hud.tab", Some(PredicateValue::Number(2.0)))),
                Some(pred("flag.checked", None)),
                false,
            ),
            a11y_button("tab.off", None, None, true),
        ],
    ));
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    // hud.tab == 2 (selected true), flag.checked == true (checked true).
    let mut slots = number_slots("hud.tab", 2.0);
    slots.insert("flag.checked".to_string(), SlotValue::Boolean(true));
    ui.build_draw_data([1280, 720], &mut fs, &no_images(), &slots);
    let focus = ui.export_focus_rects(&tree, [1280, 720], &slots, &no_cells());

    let stats = focus
        .rects
        .iter()
        .find(|r| r.id == "tab.stats")
        .expect("selected button is focusable");
    assert_eq!(
        stats.selected,
        Some(1.0),
        "matching selected predicate → 1.0"
    );
    assert_eq!(stats.checked, Some(1.0), "true checked predicate → 1.0");
    assert!(!stats.disabled, "enabled button is not disabled");

    let off = focus
        .rects
        .iter()
        .find(|r| r.id == "tab.off")
        .expect("disabled button is still focusable (nav/activation honor the bit separately)");
    assert_eq!(off.selected, None, "no selected predicate → None");
    assert_eq!(off.checked, None, "no checked predicate → None");
    assert!(off.disabled, "disabled bit is populated from the widget");
}

#[test]
fn focus_rect_selected_predicate_resolves_false_when_unmatched() {
    // A declared selected predicate that does NOT match resolves to 0.0 (not
    // None) — the metadata is present and reads false.
    let tree = anchored(vstack(
        0.0,
        0.0,
        Align::Start,
        vec![a11y_button(
            "tab.stats",
            Some(pred("hud.tab", Some(PredicateValue::Number(2.0)))),
            None,
            false,
        )],
    ));
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let slots = number_slots("hud.tab", 5.0);
    ui.build_draw_data([1280, 720], &mut fs, &no_images(), &slots);
    let focus = ui.export_focus_rects(&tree, [1280, 720], &slots, &no_cells());
    let stats = focus.rects.iter().find(|r| r.id == "tab.stats").unwrap();
    assert_eq!(
        stats.selected,
        Some(0.0),
        "unmatched selected predicate resolves to 0.0, not None",
    );
}

// --- Focus-authoring diagnostics (fire at registration, once per tree) ---

/// Register `tree` under `name` the way the boot and mod paths do.
fn register(name: &str, tree: AnchoredTree) {
    ModalStack::new()
        .registry_mut()
        .register(name, tree, ScopeTier::Engine, false);
}

/// A button whose `focusNeighbors.down` names `target`.
fn button_with_down_neighbor(id: &str, target: &str) -> Widget {
    let mut widget = button(id, "press");
    if let Widget::Button(b) = &mut widget {
        b.focus_neighbors.down = Some(target.into());
    }
    widget
}

#[test]
fn focus_authoring_warns_when_initial_focus_names_a_passive_widget() {
    let mut tree = anchored(linear_group(vec![
        text_id("TITLE", "title"),
        button("play", "openPlay"),
    ]));
    tree.initial_focus = Some("title".into());

    let capture = LogCapture::start();
    register("titleMenu", tree.clone());
    capture.assert_logged_once(
        Level::Warn,
        "[UI] tree 'titleMenu': initialFocus 'title' is not an interactive widget",
    );

    capture.clear();
    tree.initial_focus = Some("play".into());
    register("titleMenu", tree);
    capture.assert_not_logged(Level::Warn, "initialFocus");
}

#[test]
fn focus_authoring_warns_when_a_neighbor_target_is_not_interactive() {
    let passive_target = anchored(linear_group(vec![
        text_id("TITLE", "title"),
        button_with_down_neighbor("play", "title"),
    ]));

    let capture = LogCapture::start();
    register("titleMenu", passive_target);
    capture.assert_logged_once(
        Level::Warn,
        "[UI] tree 'titleMenu': widget 'play' focusNeighbors.down 'title' is not an interactive widget",
    );

    capture.clear();
    let interactive_target = anchored(linear_group(vec![
        button_with_down_neighbor("play", "exit"),
        button("exit", "ui.exitToDesktop"),
    ]));
    register("titleMenu", interactive_target);
    capture.assert_not_logged(Level::Warn, "focusNeighbors");
}

#[test]
fn focus_authoring_warns_when_a_passive_widget_authors_neighbors() {
    let mut title = text_id("TITLE", "title");
    if let Widget::Text(t) = &mut title {
        t.focus_neighbors.down = Some("play".into());
    }
    let tree = anchored(linear_group(vec![title, button("play", "openPlay")]));

    let capture = LogCapture::start();
    register("titleMenu", tree);
    capture.assert_logged_once(
        Level::Warn,
        "[UI] tree 'titleMenu': passive widget 'title' authors focusNeighbors; ignored",
    );

    // Clean case: neighbors authored only on the interactive widget, targeting
    // another interactive widget — no warning.
    capture.clear();
    let clean = anchored(linear_group(vec![
        button_with_down_neighbor("play", "exit"),
        button("exit", "ui.exitToDesktop"),
    ]));
    register("titleMenu", clean);
    capture.assert_not_logged(Level::Warn, "focusNeighbors");
}

#[test]
fn focus_authoring_initial_focus_naming_a_hidden_button_does_not_warn() {
    // `visibleWhen` is a runtime concern; the authoring check only asks whether
    // the id names an interactive widget in the descriptor, not whether it is
    // currently shown.
    let mut hidden = button("play", "openPlay");
    if let Widget::Button(b) = &mut hidden {
        b.visible_when = Some(pred("menu.showPlay", None));
    }
    let mut tree = anchored(linear_group(vec![
        hidden,
        button("exit", "ui.exitToDesktop"),
    ]));
    tree.initial_focus = Some("play".into());

    let capture = LogCapture::start();
    register("titleMenu", tree);
    capture.assert_not_logged(Level::Warn, "initialFocus");
}

#[test]
fn focus_authoring_warns_on_duplicate_interactive_id() {
    let duplicate = anchored(linear_group(vec![
        button("play", "openPlay"),
        button("play", "openPlayAgain"),
    ]));

    let capture = LogCapture::start();
    register("titleMenu", duplicate);
    capture.assert_logged_once(
        Level::Warn,
        "[UI] tree 'titleMenu': interactive id 'play' is registered more than once",
    );

    capture.clear();
    let distinct = anchored(linear_group(vec![
        button("play", "openPlay"),
        button("exit", "ui.exitToDesktop"),
    ]));
    register("titleMenu", distinct);
    capture.assert_not_logged(Level::Warn, "registered more than once");
}
