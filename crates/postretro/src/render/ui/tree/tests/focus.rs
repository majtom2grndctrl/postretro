// Focus-rect export, interaction metadata, predicate, and a11y readback.

use super::common::*;

#[test]
fn focus_export_lists_ids_rects_and_a_linear_group() {
    use crate::render::ui::descriptor::{FocusKind, FocusPolicy};
    // A vstack declaring a linear focus policy over three id'd text leaves.
    let root = Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(10.0),
        padding: SpacingValue::Literal(0.0),
        align: Align::Start,
        fill: None,
        border: None,
        id: None,
        focus_neighbors: crate::render::ui::descriptor::FocusNeighbors::default(),
        focus: Some(FocusPolicy::Shorthand(FocusKind::Linear)),
        restore_on_return: false,
        local_state: None,
        visible_when: None,
        role: None,
        children: vec![text_id("A", "a"), text_id("B", "b"), text_id("C", "c")],
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

    // Three focusable nodes, one linear group with all three as members.
    let ids: Vec<&str> = focus.rects.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"], "ids in tree order");
    assert_eq!(focus.groups.len(), 1);
    assert_eq!(
        focus.groups[0].kind,
        crate::render::ui::tree::FocusKind::Linear
    );
    assert!(focus.groups[0].wrap, "shorthand defaults wrap on");
    assert_eq!(focus.groups[0].members, vec![0, 1, 2]);
    assert_eq!(focus.initial_focus.as_deref(), Some("b"));

    // z rises in tree order so a later node hit-tests as topmost.
    assert!(focus.rects[0].z < focus.rects[1].z && focus.rects[1].z < focus.rects[2].z);

    // The exported rect uses the SAME device-pixel projection as the draw: each
    // focusable text node's rect [x, y] matches its drawn text run position.
    for (i, run) in draw.texts.iter().enumerate() {
        assert!(
            approx(focus.rects[i].rect[0], run.position[0])
                && approx(focus.rects[i].rect[1], run.position[1]),
            "focus rect {i} top-left matches the drawn run position",
        );
    }
}

#[test]
fn focus_export_auto_generates_ids_from_tree_position() {
    use crate::render::ui::descriptor::{FocusKind, FocusPolicy};
    // Children with NO authored id, under a focus-policy container, get a
    // deterministic auto-id from their child-index path (runtime-only).
    let root = Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(0.0),
        padding: SpacingValue::Literal(0.0),
        align: Align::Start,
        fill: None,
        border: None,
        id: None,
        focus_neighbors: crate::render::ui::descriptor::FocusNeighbors::default(),
        focus: Some(FocusPolicy::Shorthand(FocusKind::Linear)),
        restore_on_return: false,
        local_state: None,
        visible_when: None,
        role: None,
        children: vec![text("X", 20.0), text("Y", 20.0)],
    });
    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root,
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    ui.build_draw_data([1280, 720], &mut fs, &no_images(), &no_slots());
    let focus = ui.export_focus_rects(&tree, [1280, 720], &no_slots(), &no_cells());
    let ids: Vec<&str> = focus.rects.iter().map(|r| r.id.as_str()).collect();
    // Auto-ids are the slash-joined child paths from the root.
    assert_eq!(ids, ["0", "1"], "auto-id is the tree-position path");
}

// --- Interactive widgets ---

fn button(id: &str, on_press: &str) -> Widget {
    Widget::Button(ButtonWidget {
        id: id.into(),
        label: Some(id.into()),
        labelled_by: None,
        on_press: on_press.into(),
        focus_neighbors: Default::default(),
        repeat_on_hold: None,
        selected: None,
        checked: None,
        bind: None,
        style_ranges: None,
        disabled: false,
        visible_when: None,
        role: None,
    })
}

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
