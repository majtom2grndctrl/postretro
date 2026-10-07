use super::*;
use postretro_ui::tree::{FocusGroup, FocusKind, FocusNeighbors, FocusRect, RepeatPolicy};

fn rect(id: &str, r: [f32; 4], z: u32, group: Option<usize>) -> FocusRect {
    FocusRect {
        id: id.to_string(),
        rect: r,
        z,
        group,
        neighbors: FocusNeighbors::default(),
        interaction: None,
        // A11y readback fields. `disabled` is honored by the nav/pointer
        // paths; the `disabled_*` fixtures below flip it.
        selected: None,
        checked: None,
        disabled: false,
        tablist: None,
        clip: None,
    }
}

/// Like [`rect`] but `disabled` — the nav/pointer paths must skip it.
fn disabled_rect(id: &str, r: [f32; 4], z: u32, group: Option<usize>) -> FocusRect {
    let mut rect = rect(id, r, z, group);
    rect.disabled = true;
    rect
}

/// A vstack-style linear group of three stacked rects.
fn linear_list(wrap: bool, repeat: Option<RepeatPolicy>) -> FocusRectList {
    FocusRectList {
        rects: vec![
            rect("a", [0.0, 0.0, 100.0, 20.0], 0, Some(0)),
            rect("b", [0.0, 30.0, 100.0, 20.0], 1, Some(0)),
            rect("c", [0.0, 60.0, 100.0, 20.0], 2, Some(0)),
        ],
        groups: vec![FocusGroup {
            kind: FocusKind::Linear,
            wrap,
            repeat,
            members: vec![0, 1, 2],
            parent: None,
            bounds: [0.0; 4],
            axis: None,
        }],
        initial_focus: None,
        restore_on_return: false,
        owner: None,
    }
}

/// A 2x2 spatial grid: a b / c d.
fn grid_list() -> FocusRectList {
    FocusRectList {
        rects: vec![
            rect("a", [0.0, 0.0, 40.0, 40.0], 0, Some(0)),
            rect("b", [50.0, 0.0, 40.0, 40.0], 1, Some(0)),
            rect("c", [0.0, 50.0, 40.0, 40.0], 2, Some(0)),
            rect("d", [50.0, 50.0, 40.0, 40.0], 3, Some(0)),
        ],
        groups: vec![FocusGroup {
            kind: FocusKind::Spatial,
            wrap: false,
            repeat: None,
            members: vec![0, 1, 2, 3],
            parent: None,
            bounds: [0.0; 4],
            axis: None,
        }],
        initial_focus: None,
        restore_on_return: false,
        owner: None,
    }
}

// --- Slider nav-capture value step ---

fn slider_interaction(captures: &[&str]) -> NodeInteraction {
    NodeInteraction::Slider {
        slot: "audio.master".to_string(),
        min: 0.0,
        max: 1.0,
        step: 0.1,
        captures_nav: captures.iter().map(|s| s.to_string()).collect(),
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

#[test]
fn slider_captures_nav_steps_value_and_removes_captured_intents() {
    // nav.right increases by step; the captured intent is removed so the focus
    // engine never sees it (focus stays put). An uncaptured nav.down is retained.
    let interaction = slider_interaction(&["nav.left", "nav.right"]);
    let mut intents = vec![NavIntent::Right, NavIntent::Down];
    let next = capture_slider_step(&interaction, &mut intents).map(|c| c.value_from(0.5));
    assert!(close(next.unwrap(), 0.6), "right steps +step from 0.5");
    assert_eq!(intents, vec![NavIntent::Down], "uncaptured intent retained");
}

#[test]
fn slider_step_clamps_within_min_max() {
    // Stepping down at the floor stays at min; stepping up at the ceiling stays
    // at max — the value never escapes [min, max].
    let interaction = slider_interaction(&["nav.left", "nav.right"]);
    let mut down = vec![NavIntent::Left];
    assert!(close(
        capture_slider_step(&interaction, &mut down)
            .map(|c| c.value_from(0.0))
            .unwrap(),
        0.0
    ));
    let mut up = vec![NavIntent::Right];
    assert!(close(
        capture_slider_step(&interaction, &mut up)
            .map(|c| c.value_from(1.0))
            .unwrap(),
        1.0
    ));
}

#[test]
fn slider_with_no_captured_intents_returns_none_and_leaves_intents() {
    // A nav intent not in capturesNav is left for the focus engine and no step
    // happens (returns None — no setState write).
    let interaction = slider_interaction(&["nav.left", "nav.right"]);
    let mut intents = vec![NavIntent::Down];
    assert_eq!(
        capture_slider_step(&interaction, &mut intents).map(|c| c.value_from(0.5)),
        None
    );
    assert_eq!(intents, vec![NavIntent::Down]);
}

#[test]
fn input_mode_wire_names_match_the_slot_enum_values() {
    // Pins the strings the `input.mode` enum slot declares and a bound `text`
    // widget displays. A rename here is a slot-schema break.
    assert_eq!(InputMode::Pointer.wire_name(), "pointer");
    assert_eq!(InputMode::Focus.wire_name(), "focus");
}

#[test]
fn pointer_mode_shows_cursor_hides_ring_and_focus_mode_is_the_complement() {
    // The cursor/ring decision per mode (asserted CPU-side; OS cursor
    // visibility itself is manual). Pointer: cursor visible, ring hidden.
    // Focus: cursor hidden, ring visible. These are complements.
    assert!(InputMode::Pointer.cursor_visible());
    assert!(!InputMode::Pointer.ring_visible());
    assert!(!InputMode::Focus.cursor_visible());
    assert!(InputMode::Focus.ring_visible());
}

#[test]
fn confirm_and_click_both_report_activation_on_the_same_focused_node() {
    // A gamepad confirm and a pointer click both surface as `confirmed=true`
    // with the same focused node — the app fires that node's button `onPress`
    // off this single flag, so confirm and click have an identical effect.
    let list = linear_list(false, None);

    // Confirm path: focus a, confirm.
    let mut fe = UiFocusEngine::new();
    fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    let confirm = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Confirm],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert!(confirm.confirmed);
    assert_eq!(confirm.focused.as_deref(), Some("a"));

    // Click path: a click on a's rect (pointer mode) focuses + activates it.
    let mut fe = UiFocusEngine::new();
    let click = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[PointerPos { x: 10.0, y: 10.0 }],
        InputMode::Pointer,
        0.0,
    );
    assert!(click.confirmed, "a click also activates");
    assert_eq!(
        click.focused.as_deref(),
        confirm.focused.as_deref(),
        "click and confirm activate the same focused node"
    );
}

#[test]
fn button_interaction_never_captures_a_slider_step() {
    // A non-Slider interaction returns None and never touches the intent list.
    let interaction = NodeInteraction::Button {
        on_press: "fire".to_string(),
        repeat_on_hold: None,
    };
    let mut intents = vec![NavIntent::Right];
    assert_eq!(
        capture_slider_step(&interaction, &mut intents).map(|c| c.value_from(0.5)),
        None
    );
    assert_eq!(intents, vec![NavIntent::Right]);
}

#[test]
fn linear_down_moves_through_tree_order() {
    let mut fe = UiFocusEngine::new();
    let list = linear_list(false, None);
    // First tick initializes to the first focusable node.
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(r.focused.as_deref(), Some("a"));
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Down],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(r.focused.as_deref(), Some("b"));
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Down],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(r.focused.as_deref(), Some("c"));
}

#[test]
fn linear_wrap_wraps_past_the_end_when_enabled() {
    let mut fe = UiFocusEngine::new();
    let list = linear_list(true, None);
    fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    // a -> up wraps to c.
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Up],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("c"),
        "up from first wraps to last"
    );
}

#[test]
fn linear_no_wrap_clamps_at_the_end() {
    let mut fe = UiFocusEngine::new();
    let list = linear_list(false, None);
    fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    // a -> up with no wrap stays on a.
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Up],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("a"),
        "no-wrap up clamps at first"
    );
}

#[test]
fn spatial_picks_nearest_neighbor_by_direction() {
    let mut fe = UiFocusEngine::new();
    let list = grid_list();
    fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    // a -> right -> b, a -> down -> c.
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Right],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(r.focused.as_deref(), Some("b"));
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Down],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(r.focused.as_deref(), Some("d"), "right then down reaches d");
}

#[test]
fn focus_neighbors_override_wins_over_policy() {
    let mut fe = UiFocusEngine::new();
    let mut list = grid_list();
    // Override a's "right" to jump straight to d (bypassing the spatial b).
    list.rects[0].neighbors.right = Some("d".to_string());
    fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Right],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("d"),
        "focusNeighbors override beats the spatial nearest-neighbor",
    );
}

#[test]
fn initial_focus_selects_the_named_starting_node() {
    let mut fe = UiFocusEngine::new();
    let mut list = linear_list(false, None);
    list.initial_focus = Some("b".to_string());
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("b"),
        "initialFocus picks b, not a"
    );
}

#[test]
fn hold_to_repeat_fires_at_declared_delay_and_interval() {
    let mut fe = UiFocusEngine::new();
    let list = linear_list(
        false,
        Some(RepeatPolicy {
            initial_delay_ms: 300.0,
            interval_ms: 100.0,
        }),
    );
    // Init + press down (a -> b) and arm the clock.
    fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Down],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(r.focused.as_deref(), Some("b"), "press moves once");
    // Hold for 0.2s — under the 0.3s initial delay: no repeat yet.
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.2,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("b"),
        "no repeat before initial delay"
    );
    // Another 0.15s -> total 0.35s, past the 0.3s delay: one repeat (b -> c).
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.15,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("c"),
        "first repeat after the delay"
    );
}

#[test]
fn tap_release_does_not_drop_a_later_genuine_holds_repeat() {
    // Pins the release-edge vs N→N+1 press skew (reviewer ask): the gamepad
    // applies `directional_released` immediately in the input stage while a
    // press intent is queued a frame. A single-frame press+release TAP must
    // not poison a subsequent genuine HOLD's repeat. Mechanism: the tap's
    // release clears the clock that the deferred press re-arms next frame; a
    // genuine hold keeps the button down so the release edge stays false and
    // the clock free-runs on dt as designed.
    let mut fe = UiFocusEngine::new();
    let list = linear_list(
        true, // wrap so repeated Down keeps moving
        Some(RepeatPolicy {
            initial_delay_ms: 300.0,
            interval_ms: 100.0,
        }),
    );
    // Init focuses a.
    fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );

    // Frame N: a TAP. The deferred press intent lands (moves a -> b, arms the
    // clock), and the tap's release edge fires this same input stage.
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Down],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(r.focused.as_deref(), Some("b"), "tap press moves once");
    fe.release_repeat(); // tap released this frame: clock cleared.

    // Frame N+1: a GENUINE hold begins — its press intent arrives and re-arms
    // the clock (moves b -> c). The button stays down, so no release edge.
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Down],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(r.focused.as_deref(), Some("c"), "hold press moves once");

    // Hold past the initial delay with NO release edge: the repeat fires — the
    // tap's earlier release did not drop it.
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.35,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("a"),
        "the genuine hold's repeat fires (c -> a, wrapped) — not dropped by the tap's release"
    );
}

#[test]
fn confirm_and_cancel_without_repeat_on_hold_fire_exactly_once() {
    let mut fe = UiFocusEngine::new();
    let list = linear_list(
        false,
        Some(RepeatPolicy {
            initial_delay_ms: 100.0,
            interval_ms: 50.0,
        }),
    );
    fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Confirm],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert!(r.confirmed, "confirm fires");
    // Holding past the delay must NOT repeat confirm (no clock armed by it).
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        1.0,
    );
    assert!(!r.confirmed, "confirm never repeats");
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Cancel],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert!(r.cancelled, "cancel fires once");
}

#[test]
fn pointer_hover_moves_focus_in_pointer_mode() {
    let mut fe = UiFocusEngine::new();
    let list = linear_list(false, None);
    // Cursor over "b" (y in [30,50)). Pointer mode -> hover focuses b.
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        Some(PointerPos { x: 10.0, y: 40.0 }),
        &[],
        InputMode::Pointer,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("b"),
        "hover focuses the node under the cursor"
    );
}

#[test]
fn pointer_hover_ignored_in_focus_mode() {
    let mut fe = UiFocusEngine::new();
    let list = linear_list(false, None);
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        Some(PointerPos { x: 10.0, y: 40.0 }),
        &[],
        InputMode::Focus,
        0.0,
    );
    // Focus mode ignores hover: stays on the initial node a.
    assert_eq!(r.focused.as_deref(), Some("a"));
}

#[test]
fn click_resolves_to_topmost_z() {
    let mut fe = UiFocusEngine::new();
    // Two overlapping rects; the higher z must win the hit.
    let list = FocusRectList {
        rects: vec![
            rect("under", [0.0, 0.0, 100.0, 100.0], 0, None),
            rect("over", [0.0, 0.0, 100.0, 100.0], 5, None),
        ],
        groups: vec![],
        initial_focus: None,
        restore_on_return: false,
        owner: None,
    };
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[PointerPos { x: 50.0, y: 50.0 }],
        InputMode::Pointer,
        0.0,
    );
    assert_eq!(r.focused.as_deref(), Some("over"), "click hits topmost z");
    assert!(r.confirmed, "a click also activates the hit node");
}

#[test]
fn restore_on_return_restores_lower_tree_focus_on_pop() {
    let mut fe = UiFocusEngine::new();
    let mut lower = linear_list(false, None);
    lower.restore_on_return = true;
    // Lower tree ("hud") active; move focus to c.
    fe.tick(
        Some("hud"),
        Some(&lower),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    fe.tick(
        Some("hud"),
        Some(&lower),
        &[NavIntent::Down, NavIntent::Down],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(fe.focused_id("hud"), Some("c"));

    // Push a modal ("pause") on top; the lower tree freezes.
    let modal = linear_list(false, None);
    let r = fe.tick(
        Some("pause"),
        Some(&modal),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("a"),
        "modal starts at its own initial"
    );

    // Pop back to the lower tree: restoreOnReturn restores c, not a.
    let r = fe.tick(
        Some("hud"),
        Some(&lower),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("c"),
        "restoreOnReturn restores the lower tree's saved focus on pop",
    );
}

#[test]
fn lower_tree_freezes_while_a_modal_is_on_top() {
    let mut fe = UiFocusEngine::new();
    let lower = linear_list(false, None);
    fe.tick(
        Some("hud"),
        Some(&lower),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    // A modal is on top; nav intents drive ONLY the modal, never the frozen hud.
    let modal = linear_list(false, None);
    fe.tick(
        Some("pause"),
        Some(&modal),
        &[NavIntent::Down],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    // The lower tree's focus is unchanged (still a) — it never saw the intent.
    assert_eq!(
        fe.focused_id("hud"),
        Some("a"),
        "frozen lower tree keeps its focus"
    );
}

// --- Button activation-repeat (`repeatOnHold`) ---

/// A single-button focus list. `repeat_on_hold` opts the button into
/// activation-repeat (the on-screen keyboard backspace); `None` is a plain
/// single-fire button. The button is its own group member so it is focusable.
fn button_list(repeat_on_hold: Option<RepeatPolicy>) -> FocusRectList {
    let mut r = rect("bksp", [0.0, 0.0, 100.0, 20.0], 0, None);
    r.interaction = Some(NodeInteraction::Button {
        on_press: "backspace".to_string(),
        repeat_on_hold,
    });
    FocusRectList {
        rects: vec![r],
        groups: vec![],
        initial_focus: None,
        restore_on_return: false,
        owner: None,
    }
}

#[test]
fn flagged_button_held_confirm_repeats_at_declared_delay_and_interval() {
    // A `repeatOnHold` button: a held confirm re-fires activation on the focus
    // engine's repeat timer — one fire on press, then after the initial delay,
    // then each interval. Each fire surfaces as `confirmed` so the app re-fires
    // `on_press` through the single activation path.
    let mut fe = UiFocusEngine::new();
    let list = button_list(Some(RepeatPolicy {
        initial_delay_ms: 300.0,
        interval_ms: 100.0,
    }));
    // Init focuses the button.
    fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    // Press confirm: fires once and arms the activation-repeat clock.
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Confirm],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert!(r.confirmed, "initial confirm fires");
    // Hold 0.2s — under the 0.3s initial delay: no repeat yet.
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.2,
    );
    assert!(!r.confirmed, "no repeat before the initial delay");
    // Another 0.15s -> 0.35s total, past the delay: first repeat fires.
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.15,
    );
    assert!(r.confirmed, "first repeat after the initial delay");
    // Another 0.1s == one interval: second repeat fires.
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.1,
    );
    assert!(r.confirmed, "second repeat after one interval");
}

#[test]
fn unflagged_button_held_confirm_fires_once() {
    // A button WITHOUT `repeatOnHold` keeps F's single-fire rule: confirm fires
    // once on press and never repeats, no matter how long it is held.
    let mut fe = UiFocusEngine::new();
    let list = button_list(None);
    fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Confirm],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert!(r.confirmed, "confirm fires once on press");
    // Hold well past any plausible delay: no repeat (no clock armed).
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        5.0,
    );
    assert!(!r.confirmed, "an unflagged button never repeats");
}

#[test]
fn confirm_release_stops_the_activation_repeat() {
    // Releasing the confirm (the app drives `release_confirm_repeat`) clears the
    // clock, mirroring how a directional release clears the nav clock. After
    // release, holding past the delay yields no further repeats.
    let mut fe = UiFocusEngine::new();
    let list = button_list(Some(RepeatPolicy {
        initial_delay_ms: 100.0,
        interval_ms: 50.0,
    }));
    fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Confirm],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    // Release stops the clock.
    fe.release_confirm_repeat();
    // Hold past the delay: no repeat now that the clock is cleared.
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        1.0,
    );
    assert!(!r.confirmed, "release stops the activation-repeat");
}

// --- On-screen keyboard confirm flows through the focus engine
//     (end-to-end path) ---

use crate::input::text_entry::{TextEntryDisposition, resolve_text_entry};
use crate::input::ui_dispatch::{UiIntent, UiIntentPayload};

/// A two-key on-screen-keyboard-like focus list: `key_a` (a `kbAppend_a`
/// button, like a letter key) and `done` (the commit-sentinel button). Both
/// are focusable, activatable buttons in one linear group.
fn keyboard_like_list() -> FocusRectList {
    let mut key_a = rect("key_a", [0.0, 0.0, 40.0, 40.0], 0, Some(0));
    key_a.interaction = Some(NodeInteraction::Button {
        on_press: "kbAppend_a".to_string(),
        repeat_on_hold: None,
    });
    let mut done = rect("done", [50.0, 0.0, 40.0, 40.0], 1, Some(0));
    done.interaction = Some(NodeInteraction::Button {
        // The reserved commit sentinel the App intercepts in
        // `fire_focused_button_activation` → `commit_text_entry`.
        on_press: "ui.commitTextEntry".to_string(),
        repeat_on_hold: None,
    });
    FocusRectList {
        rects: vec![key_a, done],
        groups: vec![FocusGroup {
            kind: FocusKind::Linear,
            wrap: false,
            repeat: None,
            members: vec![0, 1],
            parent: None,
            bounds: [0.0; 4],
            axis: None,
        }],
        initial_focus: None,
        restore_on_return: false,
        owner: None,
    }
}

/// The button `on_press` for the currently-focused node, mirroring what the
/// App's `fire_focused_button_activation` reads off the focus result.
fn focused_on_press<'a>(rects: &'a FocusRectList, focused: Option<&str>) -> Option<&'a str> {
    let id = focused?;
    rects
        .rects
        .iter()
        .find(|r| r.id == id)
        .and_then(|r| match &r.interaction {
            Some(NodeInteraction::Button { on_press, .. }) => Some(on_press.as_str()),
            _ => None,
        })
}

/// The button `on_press` the App's `fire_focused_button_activation` would fire,
/// mirroring its disabled gate (`.filter(|r| !r.disabled)` before reading the
/// `Button` interaction). A disabled focused node yields `None` — no activation.
/// Used to pin the App-side activation block at this layer.
fn focused_activation<'a>(rects: &'a FocusRectList, focused: Option<&str>) -> Option<&'a str> {
    let id = focused?;
    rects
        .rects
        .iter()
        .find(|r| r.id == id)
        .filter(|r| !r.disabled)
        .and_then(|r| match &r.interaction {
            Some(NodeInteraction::Button { on_press, .. }) => Some(on_press.as_str()),
            _ => None,
        })
}

#[test]
fn confirm_on_key_button_types_and_keeps_keyboard_open_then_done_commits() {
    // End-to-end (Fix 1): with a text-entry tree open and an on-screen keyboard
    // key focused, a `Nav(Confirm)` must NOT be swallowed as a text-entry commit
    // — it has to flow through the focus engine so the key's `on_press` fires.
    // Models the App's per-frame order: `resolve_text_entry` first, then the
    // focus engine, then `fire_focused_button_activation`.
    let list = keyboard_like_list();
    let confirm = [UiIntent {
        seq: 0,
        payload: UiIntentPayload::Nav(NavIntent::Confirm),
    }];

    // 1) Focus `key_a` and confirm. `resolve_text_entry` is told focus is on a
    //    button (confirm_on_button = true): the confirm stays Open (NOT a
    //    Commit), so the keyboard tree does NOT pop.
    let res = resolve_text_entry(&confirm, true);
    assert_eq!(
        res.disposition,
        TextEntryDisposition::Open,
        "confirm on a key button is not a text-entry commit — the tree stays open"
    );
    assert!(!res.consumed_commit_or_cancel());

    // The focus engine then sees the (unfiltered) confirm and activates the
    // focused key — its `on_press` is the append reaction, so `ui.textEntry`
    // gains "a" and the keyboard stays open.
    let mut fe = UiFocusEngine::new();
    fe.tick(
        Some("keyboard"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    let r = fe.tick(
        Some("keyboard"),
        Some(&list),
        &[NavIntent::Confirm],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert!(r.confirmed, "the confirm activates the focused key");
    assert_eq!(r.focused.as_deref(), Some("key_a"));
    assert_eq!(
        focused_on_press(&list, r.focused.as_deref()),
        Some("kbAppend_a"),
        "the App fires key_a's kbAppend_a (types 'a') — the tree is NOT popped"
    );

    // 2) Move focus to `done` and confirm. Again `resolve_text_entry` stays Open
    //    (focus is still on a button), and the focus engine activates `done` —
    //    whose `on_press` is the commit sentinel the App routes to
    //    `commit_text_entry` (fire `on_commit`, then pop).
    let res = resolve_text_entry(&confirm, true);
    assert_eq!(res.disposition, TextEntryDisposition::Open);
    let r = fe.tick(
        Some("keyboard"),
        Some(&list),
        &[NavIntent::Right, NavIntent::Confirm],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert!(r.confirmed);
    assert_eq!(r.focused.as_deref(), Some("done"));
    assert_eq!(
        focused_on_press(&list, r.focused.as_deref()),
        Some("ui.commitTextEntry"),
        "done's sentinel routes to commit_text_entry (fires on_commit, pops)"
    );
}

// --- Disabled focus + activation honoring ---

#[test]
fn linear_nav_skips_a_run_of_consecutive_disabled_members() {
    // A 4-member vstack a [b c disabled] d: pressing Down from a must skip the
    // run of TWO consecutive disabled members (b, c) in one move and land on d
    // — not stop on b or c. No-wrap, so clamp semantics still apply at the edge.
    let list = FocusRectList {
        rects: vec![
            rect("a", [0.0, 0.0, 100.0, 20.0], 0, Some(0)),
            disabled_rect("b", [0.0, 30.0, 100.0, 20.0], 1, Some(0)),
            disabled_rect("c", [0.0, 60.0, 100.0, 20.0], 2, Some(0)),
            rect("d", [0.0, 90.0, 100.0, 20.0], 3, Some(0)),
        ],
        groups: vec![FocusGroup {
            kind: FocusKind::Linear,
            wrap: false,
            repeat: None,
            members: vec![0, 1, 2, 3],
            parent: None,
            bounds: [0.0; 4],
            axis: None,
        }],
        initial_focus: None,
        restore_on_return: false,
        owner: None,
    };
    let mut fe = UiFocusEngine::new();
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("a"),
        "init focuses first enabled"
    );
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Down],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("d"),
        "Down steps PAST both consecutive disabled members onto d"
    );
}

#[test]
fn linear_nav_clamps_when_only_disabled_lie_ahead() {
    // a [b c disabled] with no wrap: Down from a finds no enabled member ahead
    // (only the disabled run), so focus stays on a (clamp respected — no landing
    // on a disabled node, no wrap-around).
    let list = FocusRectList {
        rects: vec![
            rect("a", [0.0, 0.0, 100.0, 20.0], 0, Some(0)),
            disabled_rect("b", [0.0, 30.0, 100.0, 20.0], 1, Some(0)),
            disabled_rect("c", [0.0, 60.0, 100.0, 20.0], 2, Some(0)),
        ],
        groups: vec![FocusGroup {
            kind: FocusKind::Linear,
            wrap: false,
            repeat: None,
            members: vec![0, 1, 2],
            parent: None,
            bounds: [0.0; 4],
            axis: None,
        }],
        initial_focus: None,
        restore_on_return: false,
        owner: None,
    };
    let mut fe = UiFocusEngine::new();
    fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Down],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("a"),
        "no enabled member ahead: clamp keeps focus on a, never on a disabled node"
    );
}

#[test]
fn linear_nav_wraps_past_disabled_run_to_an_enabled_member() {
    // a [b c disabled] d, WRAP enabled: Up from a wraps to d (skipping the
    // disabled b/c at the tail), proving the wrap walk skips disabled members.
    let list = FocusRectList {
        rects: vec![
            rect("a", [0.0, 0.0, 100.0, 20.0], 0, Some(0)),
            disabled_rect("b", [0.0, 30.0, 100.0, 20.0], 1, Some(0)),
            disabled_rect("c", [0.0, 60.0, 100.0, 20.0], 2, Some(0)),
            rect("d", [0.0, 90.0, 100.0, 20.0], 3, Some(0)),
        ],
        groups: vec![FocusGroup {
            kind: FocusKind::Linear,
            wrap: true,
            repeat: None,
            members: vec![0, 1, 2, 3],
            parent: None,
            bounds: [0.0; 4],
            axis: None,
        }],
        initial_focus: None,
        restore_on_return: false,
        owner: None,
    };
    let mut fe = UiFocusEngine::new();
    fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Up],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("d"),
        "Up wraps past the disabled tail run to the last enabled member d"
    );
}

#[test]
fn spatial_nav_never_returns_a_disabled_candidate() {
    // In the 2x2 grid a b / c d, disable the natural right-neighbor b. Right
    // from a must NOT pick b; with b excluded the only candidate to the right
    // is d (diagonal), so spatial resolves to d — never the disabled b.
    let mut list = grid_list();
    list.rects[1].disabled = true; // disable b
    let mut fe = UiFocusEngine::new();
    fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Right],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("d"),
        "disabled b is excluded from the spatial candidate set; d is chosen"
    );
    assert_ne!(r.focused.as_deref(), Some("b"), "never lands on disabled b");
}

#[test]
fn neighbor_override_onto_a_disabled_node_is_ignored() {
    // a's "right" override names the disabled b; the override must be ignored and
    // the spatial policy resolves instead (also skipping b → d).
    let mut list = grid_list();
    list.rects[1].disabled = true; // disable b
    list.rects[0].neighbors.right = Some("b".to_string());
    let mut fe = UiFocusEngine::new();
    fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[NavIntent::Right],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("d"),
        "an override onto a disabled node is ignored; policy resolves to d"
    );
}

#[test]
fn initial_focus_skips_a_leading_disabled_node() {
    // First member a is disabled: initial focus must fall through to the first
    // ENABLED member b, never selecting the leading disabled node.
    let list = FocusRectList {
        rects: vec![
            disabled_rect("a", [0.0, 0.0, 100.0, 20.0], 0, Some(0)),
            rect("b", [0.0, 30.0, 100.0, 20.0], 1, Some(0)),
            rect("c", [0.0, 60.0, 100.0, 20.0], 2, Some(0)),
        ],
        groups: vec![FocusGroup {
            kind: FocusKind::Linear,
            wrap: false,
            repeat: None,
            members: vec![0, 1, 2],
            parent: None,
            bounds: [0.0; 4],
            axis: None,
        }],
        initial_focus: None,
        restore_on_return: false,
        owner: None,
    };
    let mut fe = UiFocusEngine::new();
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("b"),
        "initial focus skips the leading disabled node and selects b"
    );
}

#[test]
fn initial_focus_ignores_initial_focus_naming_a_disabled_node() {
    // `initialFocus` explicitly names the disabled b: it must be ignored and the
    // first enabled member (a) chosen instead — a disabled node is never initial.
    let mut list = linear_list(false, None);
    list.rects[1].disabled = true; // disable b
    list.initial_focus = Some("b".to_string());
    let mut fe = UiFocusEngine::new();
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("a"),
        "initialFocus naming a disabled node is ignored; first enabled (a) wins"
    );
}

#[test]
fn pointer_click_on_a_disabled_node_does_not_focus_or_activate_it() {
    // A click directly over a disabled node falls through as if not focusable: it
    // neither focuses nor activates it. Here the disabled node is the only rect
    // under the cursor, so focus stays at the initial node and nothing activates.
    let list = FocusRectList {
        rects: vec![
            rect("a", [0.0, 0.0, 100.0, 20.0], 0, Some(0)),
            disabled_rect("b", [0.0, 30.0, 100.0, 20.0], 1, Some(0)),
        ],
        groups: vec![FocusGroup {
            kind: FocusKind::Linear,
            wrap: false,
            repeat: None,
            members: vec![0, 1],
            parent: None,
            bounds: [0.0; 4],
            axis: None,
        }],
        initial_focus: None,
        restore_on_return: false,
        owner: None,
    };
    let mut fe = UiFocusEngine::new();
    fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    // Click at y=40 → over disabled "b".
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[PointerPos { x: 10.0, y: 40.0 }],
        InputMode::Pointer,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("a"),
        "a click on a disabled node does not move focus to it"
    );
    assert!(
        !r.confirmed,
        "a click on a disabled node does not activate (it falls through)"
    );
}

#[test]
fn pointer_hover_over_a_disabled_node_does_not_focus_it() {
    // In pointer mode, hovering a disabled node must not hover-focus it (it falls
    // through as not-focusable); focus stays put at the initial node.
    let list = FocusRectList {
        rects: vec![
            rect("a", [0.0, 0.0, 100.0, 20.0], 0, Some(0)),
            disabled_rect("b", [0.0, 30.0, 100.0, 20.0], 1, Some(0)),
        ],
        groups: vec![FocusGroup {
            kind: FocusKind::Linear,
            wrap: false,
            repeat: None,
            members: vec![0, 1],
            parent: None,
            bounds: [0.0; 4],
            axis: None,
        }],
        initial_focus: None,
        restore_on_return: false,
        owner: None,
    };
    let mut fe = UiFocusEngine::new();
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        Some(PointerPos { x: 10.0, y: 40.0 }),
        &[],
        InputMode::Pointer,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("a"),
        "hover over a disabled node does not move focus to it"
    );
}

#[test]
fn click_falls_through_a_disabled_top_node_to_an_enabled_one_beneath() {
    // A disabled node on top must NOT mask an enabled node beneath it: the hit
    // resolves to the topmost ENABLED rect under the point. Here "over" (disabled,
    // z=5) overlaps "under" (enabled, z=0); a click resolves to "under".
    let list = FocusRectList {
        rects: vec![
            rect("under", [0.0, 0.0, 100.0, 100.0], 0, None),
            disabled_rect("over", [0.0, 0.0, 100.0, 100.0], 5, None),
        ],
        groups: vec![],
        initial_focus: None,
        restore_on_return: false,
        owner: None,
    };
    let mut fe = UiFocusEngine::new();
    let r = fe.tick(
        Some("t"),
        Some(&list),
        &[],
        None,
        &[PointerPos { x: 50.0, y: 50.0 }],
        InputMode::Pointer,
        0.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("under"),
        "a disabled top node does not mask the enabled node beneath it"
    );
    assert!(r.confirmed, "the fall-through enabled node activates");
}

#[test]
fn focused_activation_no_ops_on_a_disabled_focused_node() {
    // Mirrors the App-side `fire_focused_button_activation` gate: even if the
    // focus result somehow names a disabled button (e.g. a node that became
    // disabled after focus landed), the activation resolves to None — no
    // `on_press` fires.
    let mut enabled = rect("go", [0.0, 0.0, 40.0, 40.0], 0, Some(0));
    enabled.interaction = Some(NodeInteraction::Button {
        on_press: "fire".to_string(),
        repeat_on_hold: None,
    });
    let mut disabled = disabled_rect("blocked", [0.0, 50.0, 40.0, 40.0], 1, Some(0));
    disabled.interaction = Some(NodeInteraction::Button {
        on_press: "nope".to_string(),
        repeat_on_hold: None,
    });
    let list = FocusRectList {
        rects: vec![enabled, disabled],
        groups: vec![FocusGroup {
            kind: FocusKind::Linear,
            wrap: false,
            repeat: None,
            members: vec![0, 1],
            parent: None,
            bounds: [0.0; 4],
            axis: None,
        }],
        initial_focus: None,
        restore_on_return: false,
        owner: None,
    };
    // Enabled focused node activates normally.
    assert_eq!(
        focused_activation(&list, Some("go")),
        Some("fire"),
        "an enabled focused button activates"
    );
    // Disabled focused node is gated: no activation.
    assert_eq!(
        focused_activation(&list, Some("blocked")),
        None,
        "a disabled focused button does not activate (App-side gate)"
    );
}

/// Export a real descriptor's focus rects the way the renderer does each frame.
fn export_from_json(json: &str) -> FocusRectList {
    let tree: postretro_ui::descriptor::AnchoredTree =
        serde_json::from_str(json).expect("test descriptor parses");
    let theme = postretro_ui::theme::UiTheme::engine_default();
    let mut ui = postretro_ui::tree::UiTree::from_descriptor(&tree, &theme);
    let mut font_system = postretro_ui::text::build_font_system();
    let slots = std::collections::HashMap::new();
    let cells = postretro_ui::tree::CellValues::new();
    ui.build_draw_data(
        [1280, 720],
        &mut font_system,
        &postretro_ui::tree::ImageSizes::new(),
        &slots,
    );
    ui.export_focus_rects(&tree, [1280, 720], &slots, &cells)
}

// Regression: nav down from EXIT (wrap) landed on the non-interactive
// "POSTRETRO" title because the export made every group descendant a stop.
#[test]
fn linear_wrap_from_last_button_skips_the_title_to_the_first_button() {
    // The dev title-menu shape: a title text above three buttons in one
    // linear, wrapping focus group.
    let list = export_from_json(
        r#"{
            "anchor": "center",
            "offset": [0.0, 0.0],
            "captureMode": "capture",
            "initialFocus": "exit",
            "root": {
                "kind": "vstack",
                "gap": 12.0,
                "padding": 24.0,
                "align": "stretch",
                "focus": { "policy": "linear", "wrap": true },
                "children": [
                    { "kind": "text", "content": "POSTRETRO", "fontSize": 36.0, "color": "ok" },
                    { "kind": "button", "id": "play", "label": "PLAY", "onPress": "openPlay" },
                    { "kind": "button", "id": "options", "label": "OPTIONS", "onPress": "openOptions" },
                    { "kind": "button", "id": "exit", "label": "EXIT", "onPress": "ui.exitToDesktop" }
                ]
            }
        }"#,
    );
    let mut fe = UiFocusEngine::new();
    let tick = |fe: &mut UiFocusEngine, intents: &[NavIntent]| {
        fe.tick(
            Some("title"),
            Some(&list),
            intents,
            None,
            &[],
            InputMode::Focus,
            0.0,
        )
        .focused
    };
    assert_eq!(tick(&mut fe, &[]).as_deref(), Some("exit"));
    assert_eq!(
        tick(&mut fe, &[NavIntent::Down]).as_deref(),
        Some("play"),
        "down from the last button wraps to the first button, not the title",
    );
    assert_eq!(
        tick(&mut fe, &[NavIntent::Up]).as_deref(),
        Some("exit"),
        "up from the first button wraps to the last button, not the title",
    );
}

#[test]
fn passive_only_focus_group_yields_no_focus() {
    let list = export_from_json(
        r#"{
            "anchor": "center",
            "offset": [0.0, 0.0],
            "captureMode": "capture",
            "root": {
                "kind": "vstack",
                "gap": 12.0,
                "padding": 24.0,
                "align": "stretch",
                "focus": { "policy": "linear", "wrap": true },
                "children": [
                    { "kind": "text", "content": "POSTRETRO", "fontSize": 36.0, "color": "ok" },
                    { "kind": "vstack", "gap": 0.0, "padding": 0.0, "align": "start", "children": [
                        { "kind": "text", "id": "note", "content": "LOADING", "fontSize": 18.0, "color": "ok" }
                    ] }
                ]
            }
        }"#,
    );
    let mut fe = UiFocusEngine::new();
    let focused = fe
        .tick(
            Some("title"),
            Some(&list),
            &[NavIntent::Down],
            None,
            &[],
            InputMode::Focus,
            0.0,
        )
        .focused;
    assert_eq!(
        focused, None,
        "a tree with no interactive widget takes no focus"
    );
}

// --- E23 U3: engine-default repeat, one repeat per frame, slider repeat ---

fn long_list(repeat: Option<RepeatPolicy>, slider: Option<NodeInteraction>) -> FocusRectList {
    let rects = (0..10)
        .map(|i| {
            let mut r = rect(
                &format!("n{i}"),
                [0.0, 30.0 * i as f32, 100.0, 20.0],
                i,
                Some(0),
            );
            if i == 0 {
                r.interaction = slider.clone();
            }
            r
        })
        .collect();
    FocusRectList {
        rects,
        groups: vec![FocusGroup {
            kind: FocusKind::Linear,
            wrap: false,
            repeat,
            members: (0..10).collect(),
            parent: None,
            bounds: [0.0; 4],
            axis: None,
        }],
        initial_focus: None,
        restore_on_return: false,
        owner: None,
    }
}

fn step(
    fe: &mut UiFocusEngine,
    list: &FocusRectList,
    intents: &[NavIntent],
    dt: f32,
) -> FocusTickResult {
    fe.tick(
        Some("t"),
        Some(list),
        intents,
        None,
        &[],
        InputMode::Focus,
        dt,
    )
}

fn focused_index(r: &FocusTickResult) -> usize {
    r.focused.as_deref().unwrap()[1..].parse().unwrap()
}

#[test]
fn a_tap_moves_one_step_with_no_repeat_and_a_hold_repeats_at_the_engine_default() {
    let mut fe = UiFocusEngine::new();
    let list = long_list(None, None);
    step(&mut fe, &list, &[], 0.0);
    let r = step(&mut fe, &list, &[NavIntent::Down], 0.0);
    assert_eq!(focused_index(&r), 1);
    fe.release_repeat();
    let r = step(&mut fe, &list, &[], 1.0);
    assert_eq!(focused_index(&r), 1, "a tap never repeats");

    let r = step(&mut fe, &list, &[NavIntent::Down], 0.0);
    assert_eq!(focused_index(&r), 2);
    let r = step(&mut fe, &list, &[], 0.39);
    assert_eq!(
        focused_index(&r),
        2,
        "no repeat before the 400 ms default delay"
    );
    let r = step(&mut fe, &list, &[], 0.02);
    assert_eq!(focused_index(&r), 3, "first repeat after the delay");
    let r = step(&mut fe, &list, &[], 0.1);
    assert_eq!(focused_index(&r), 4, "then every 100 ms");
    fe.release_repeat();
    let r = step(&mut fe, &list, &[], 0.5);
    assert_eq!(focused_index(&r), 4, "release stops the repeat");
}

#[test]
fn an_authored_zero_delay_never_repeats_and_an_authored_cadence_wins() {
    let mut fe = UiFocusEngine::new();
    let zero = long_list(
        Some(RepeatPolicy {
            initial_delay_ms: 0.0,
            interval_ms: 50.0,
        }),
        None,
    );
    step(&mut fe, &zero, &[], 0.0);
    step(&mut fe, &zero, &[NavIntent::Down], 0.0);
    let r = step(&mut fe, &zero, &[], 2.0);
    assert_eq!(focused_index(&r), 1);

    let mut fe = UiFocusEngine::new();
    let authored = long_list(
        Some(RepeatPolicy {
            initial_delay_ms: 200.0,
            interval_ms: 50.0,
        }),
        None,
    );
    step(&mut fe, &authored, &[], 0.0);
    step(&mut fe, &authored, &[NavIntent::Down], 0.0);
    let r = step(&mut fe, &authored, &[], 0.21);
    assert_eq!(focused_index(&r), 2);
    let r = step(&mut fe, &authored, &[], 0.05);
    assert_eq!(focused_index(&r), 3);
}

#[test]
fn a_held_direction_across_a_one_second_frame_moves_at_most_one_step() {
    let mut fe = UiFocusEngine::new();
    let list = long_list(None, None);
    step(&mut fe, &list, &[], 0.0);
    step(&mut fe, &list, &[NavIntent::Down], 0.0);
    let r = step(&mut fe, &list, &[], 1.0);
    assert_eq!(focused_index(&r), 2, "one repeat, not the whole backlog");
    let r = step(&mut fe, &list, &[], 0.016);
    assert_eq!(
        focused_index(&r),
        2,
        "the backlog was dropped, not deferred"
    );
}

#[test]
fn a_direction_held_while_a_tree_is_pushed_does_not_move_its_focus() {
    let mut fe = UiFocusEngine::new();
    let list = long_list(None, None);
    step(&mut fe, &list, &[], 0.0);
    step(&mut fe, &list, &[NavIntent::Down], 0.0);
    let dialog = linear_list(false, None);
    let r = fe.tick(
        Some("dialog"),
        Some(&dialog),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert_eq!(r.focused.as_deref(), Some("a"));
    let r = fe.tick(
        Some("dialog"),
        Some(&dialog),
        &[],
        None,
        &[],
        InputMode::Focus,
        1.0,
    );
    assert_eq!(
        r.focused.as_deref(),
        Some("a"),
        "the held direction stays inert"
    );
}

#[test]
fn a_held_slider_repeats_its_step_and_accelerates_with_hold_time() {
    let mut fe = UiFocusEngine::new();
    let list = long_list(None, Some(slider_interaction(&["nav.right", "nav.left"])));
    step(&mut fe, &list, &[], 0.0);
    // The press itself was captured and stepped by the app before the tick.
    fe.arm_slider_repeat(&list, NavIntent::Right);
    let mut steps = Vec::new();
    for _ in 0..40 {
        // 100 ms frames: one repeat per frame once the 400 ms delay passes.
        let r = step(&mut fe, &list, &[], 0.1);
        assert_eq!(focused_index(&r), 0, "a slider repeat never moves focus");
        steps.push(r.slider_steps);
    }
    // The first tick is the press frame; the 400 ms delay runs from the next.
    assert_eq!(&steps[..4], &[0, 0, 0, 0]);
    assert!(steps[4..13].iter().all(|s| *s == 1), "{steps:?}");
    assert!(steps[15..23].iter().all(|s| *s == 2), "{steps:?}");
    assert!(steps[26..].iter().all(|s| *s == 4), "{steps:?}");
}

#[test]
fn a_held_slider_across_a_one_second_frame_steps_once() {
    let mut fe = UiFocusEngine::new();
    let list = long_list(None, Some(slider_interaction(&["nav.right", "nav.left"])));
    step(&mut fe, &list, &[], 0.0);
    fe.arm_slider_repeat(&list, NavIntent::Left);
    // The press frame never advances the clock, however long it ran.
    let r = step(&mut fe, &list, &[], 1.0);
    assert_eq!(r.slider_steps, 0);
    let r = step(&mut fe, &list, &[], 1.0);
    assert_eq!(r.slider_steps, -1);
}

// A slider's held press repeats on the same frame a held directional press
// would: the arming frame does not advance the clock.
#[test]
fn a_slider_press_frame_does_not_advance_the_repeat_clock() {
    let mut fe = UiFocusEngine::new();
    let list = long_list(None, Some(slider_interaction(&["nav.right", "nav.left"])));
    step(&mut fe, &list, &[], 0.0);
    fe.arm_slider_repeat(&list, NavIntent::Right);
    assert_eq!(
        step(&mut fe, &list, &[], 0.3).slider_steps,
        0,
        "press frame"
    );
    assert_eq!(
        step(&mut fe, &list, &[], 0.3).slider_steps,
        0,
        "300 ms held"
    );
    assert_eq!(
        step(&mut fe, &list, &[], 0.15).slider_steps,
        1,
        "450 ms held"
    );
}

#[test]
fn a_slider_step_clamps_at_its_bounds_without_overshoot() {
    assert!(close(slider_value(0.95, 4, 0.1, 0.0, 1.0), 1.0));
    assert!(close(slider_value(0.05, -2, 0.1, 0.0, 1.0), 0.0));
}

// --- E23 U3: nested focus groups ---

/// A tabbed menu: a horizontal wrapping strip of tabs above a vertical,
/// non-wrapping panel, both nested in a vertical root group.
fn tabbed_list() -> FocusRectList {
    let group = |kind, wrap, members: Vec<usize>, parent, bounds, axis| FocusGroup {
        kind,
        wrap,
        repeat: None,
        members,
        parent,
        bounds,
        axis,
    };
    use postretro_ui::tree::FocusAxis::{Horizontal, Vertical};
    FocusRectList {
        rects: vec![
            rect("tab0", [0.0, 0.0, 50.0, 20.0], 0, Some(1)),
            rect("tab1", [60.0, 0.0, 50.0, 20.0], 1, Some(1)),
            rect("tab2", [120.0, 0.0, 50.0, 20.0], 2, Some(1)),
            rect("p0", [0.0, 40.0, 170.0, 20.0], 3, Some(2)),
            rect("p1", [0.0, 70.0, 170.0, 20.0], 4, Some(2)),
            rect("back", [0.0, 110.0, 170.0, 20.0], 5, Some(0)),
        ],
        groups: vec![
            group(
                FocusKind::Linear,
                false,
                vec![5],
                None,
                [0.0, 0.0, 170.0, 130.0],
                Some(Vertical),
            ),
            group(
                FocusKind::Linear,
                true,
                vec![0, 1, 2],
                Some(0),
                [0.0, 0.0, 170.0, 20.0],
                Some(Horizontal),
            ),
            group(
                FocusKind::Linear,
                false,
                vec![3, 4],
                Some(0),
                [0.0, 40.0, 170.0, 50.0],
                Some(Vertical),
            ),
        ],
        initial_focus: Some("tab0".to_string()),
        restore_on_return: false,
        owner: None,
    }
}

fn nav(fe: &mut UiFocusEngine, list: &FocusRectList, intent: NavIntent) -> Option<String> {
    step(fe, list, &[intent], 0.0).focused
}

#[test]
fn down_from_any_tab_enters_the_panel_and_up_returns_to_the_tab_last_focused() {
    let list = tabbed_list();
    for tab in ["tab0", "tab1", "tab2"] {
        let mut fe = UiFocusEngine::new();
        step(&mut fe, &list, &[], 0.0);
        while fe.focused_id("t") != Some(tab) {
            nav(&mut fe, &list, NavIntent::Right);
        }
        assert_eq!(
            nav(&mut fe, &list, NavIntent::Down).as_deref(),
            Some("p0"),
            "from {tab}"
        );
        assert_eq!(nav(&mut fe, &list, NavIntent::Up).as_deref(), Some(tab));
    }
}

#[test]
fn right_on_the_last_tab_wraps_within_the_strip() {
    let list = tabbed_list();
    let mut fe = UiFocusEngine::new();
    step(&mut fe, &list, &[], 0.0);
    nav(&mut fe, &list, NavIntent::Right);
    nav(&mut fe, &list, NavIntent::Right);
    assert_eq!(
        nav(&mut fe, &list, NavIntent::Right).as_deref(),
        Some("tab0")
    );
}

#[test]
fn moving_back_into_a_nested_group_lands_on_its_last_focused_member() {
    let list = tabbed_list();
    let mut fe = UiFocusEngine::new();
    step(&mut fe, &list, &[], 0.0);
    nav(&mut fe, &list, NavIntent::Down); // p0
    nav(&mut fe, &list, NavIntent::Down); // p1
    assert_eq!(
        nav(&mut fe, &list, NavIntent::Down).as_deref(),
        Some("back")
    );
    assert_eq!(nav(&mut fe, &list, NavIntent::Up).as_deref(), Some("p1"));
}

#[test]
fn next_at_the_end_of_a_non_wrapping_nested_group_does_nothing() {
    let list = tabbed_list();
    let mut fe = UiFocusEngine::new();
    step(&mut fe, &list, &[], 0.0);
    nav(&mut fe, &list, NavIntent::Down);
    nav(&mut fe, &list, NavIntent::Down);
    assert_eq!(nav(&mut fe, &list, NavIntent::Next).as_deref(), Some("p1"));
}

#[test]
fn a_focus_neighbors_target_in_another_group_wins_over_the_escape() {
    let mut list = tabbed_list();
    list.rects[3].neighbors.up = Some("tab2".to_string());
    let mut fe = UiFocusEngine::new();
    step(&mut fe, &list, &[], 0.0);
    nav(&mut fe, &list, NavIntent::Down);
    assert_eq!(nav(&mut fe, &list, NavIntent::Up).as_deref(), Some("tab2"));
}

#[test]
fn re_entering_a_group_whose_last_member_is_gone_or_disabled_lands_on_its_first_enabled() {
    let list = tabbed_list();
    let mut fe = UiFocusEngine::new();
    step(&mut fe, &list, &[], 0.0);
    nav(&mut fe, &list, NavIntent::Down);
    nav(&mut fe, &list, NavIntent::Down); // p1 remembered
    nav(&mut fe, &list, NavIntent::Down); // back
    let mut disabled = list.clone();
    disabled.rects[4].disabled = true;
    assert_eq!(
        nav(&mut fe, &disabled, NavIntent::Up).as_deref(),
        Some("p0")
    );

    let mut fe = UiFocusEngine::new();
    step(&mut fe, &list, &[], 0.0);
    nav(&mut fe, &list, NavIntent::Down);
    nav(&mut fe, &list, NavIntent::Down);
    nav(&mut fe, &list, NavIntent::Down);
    let mut rebuilt = list.clone();
    rebuilt.rects[4].id = "p1-renamed".to_string();
    let mut first_disabled = rebuilt.clone();
    first_disabled.rects[3].disabled = true;
    assert_eq!(
        nav(&mut fe, &first_disabled, NavIntent::Up).as_deref(),
        Some("p1-renamed")
    );
}

#[test]
fn a_root_linear_group_still_steps_on_either_axis() {
    // Back-compat: with no enclosing group a cross-axis move keeps today's
    // sequential step instead of going nowhere.
    let mut fe = UiFocusEngine::new();
    let mut list = linear_list(false, None);
    list.groups[0].axis = Some(postretro_ui::tree::FocusAxis::Vertical);
    step(&mut fe, &list, &[], 0.0);
    assert_eq!(nav(&mut fe, &list, NavIntent::Right).as_deref(), Some("b"));
}

// --- E23 U3: restore on return ---

fn restoring(restore: bool) -> FocusRectList {
    let mut list = linear_list(false, None);
    list.restore_on_return = restore;
    list
}

fn tick_on(
    fe: &mut UiFocusEngine,
    key: &str,
    rects: Option<&FocusRectList>,
    intents: &[NavIntent],
) -> Option<String> {
    fe.tick(Some(key), rects, intents, None, &[], InputMode::Focus, 0.0)
        .focused
}

#[test]
fn a_pop_that_reveals_a_tree_restores_its_focus_unless_it_opts_out() {
    for (restore, expected) in [(true, "b"), (false, "a")] {
        let menu = restoring(restore);
        let dialog = linear_list(false, None);
        let mut fe = UiFocusEngine::new();
        tick_on(&mut fe, "menu", Some(&menu), &[]);
        tick_on(&mut fe, "menu", Some(&menu), &[NavIntent::Down]);
        tick_on(&mut fe, "dialog#2", Some(&dialog), &[]);
        assert_eq!(
            tick_on(&mut fe, "menu", Some(&menu), &[]).as_deref(),
            Some(expected),
            "restore {restore}"
        );
    }
}

#[test]
fn a_fresh_push_of_a_tree_visited_before_lands_on_its_initial_focus() {
    // A confirmation closed and reopened (even on one frame) is a
    // new instance, so it opens on its safe choice, not the one taken last time.
    let menu = restoring(true);
    let dialog = restoring(true);
    let mut fe = UiFocusEngine::new();
    tick_on(&mut fe, "menu", Some(&menu), &[]);
    tick_on(&mut fe, "confirm#2", Some(&dialog), &[]);
    assert_eq!(
        tick_on(&mut fe, "confirm#2", Some(&dialog), &[NavIntent::Down]).as_deref(),
        Some("b")
    );
    assert_eq!(
        tick_on(&mut fe, "confirm#3", Some(&dialog), &[]).as_deref(),
        Some("a")
    );
}

#[test]
fn a_stale_export_from_a_popped_tree_neither_resets_nor_overwrites_the_revealed_focus() {
    // The text-entry commit pops before the focus tick, whose export still
    // describes the keyboard. The app withholds that export; the revealed tree
    // keeps its saved focus once its own export arrives.
    let menu = restoring(true);
    let keyboard = linear_list(false, None);
    let mut fe = UiFocusEngine::new();
    tick_on(&mut fe, "menu", Some(&menu), &[]);
    tick_on(&mut fe, "menu", Some(&menu), &[NavIntent::Down]);
    tick_on(&mut fe, "keyboard#5", Some(&keyboard), &[]);
    assert_eq!(tick_on(&mut fe, "menu", None, &[]), None);
    assert_eq!(
        tick_on(&mut fe, "menu", Some(&menu), &[]).as_deref(),
        Some("b")
    );
}

#[test]
fn a_saved_focus_now_disabled_or_rebuilt_away_lands_on_initial_focus_on_return() {
    let menu = restoring(true);
    let dialog = linear_list(false, None);
    let mut fe = UiFocusEngine::new();
    tick_on(&mut fe, "menu", Some(&menu), &[]);
    tick_on(&mut fe, "menu", Some(&menu), &[NavIntent::Down]);
    tick_on(&mut fe, "dialog#2", Some(&dialog), &[]);
    let mut disabled = menu.clone();
    disabled.rects[1].disabled = true;
    assert_eq!(
        tick_on(&mut fe, "menu", Some(&disabled), &[]).as_deref(),
        Some("a")
    );

    let mut fe = UiFocusEngine::new();
    tick_on(&mut fe, "menu", Some(&menu), &[]);
    tick_on(&mut fe, "menu", Some(&menu), &[NavIntent::Down]);
    tick_on(&mut fe, "dialog#2", Some(&dialog), &[]);
    let mut rebuilt = menu.clone();
    rebuilt.rects[1].id = "b2".to_string();
    assert_eq!(
        tick_on(&mut fe, "menu", Some(&rebuilt), &[]).as_deref(),
        Some("a")
    );
}

#[test]
fn an_export_owned_by_another_tree_is_withheld_from_the_focus_tick() {
    use postretro_ui::modal_stack::ScopeTier;
    use postretro_ui::tree::FocusRectOwner;
    let mut list = linear_list(false, None);
    list.owner = Some(FocusRectOwner {
        name: "keyboard".to_string(),
        tier: ScopeTier::Engine,
    });
    assert!(crate::session::focus_rects_for(Some(&list), "menu").is_none());
    assert!(crate::session::focus_rects_for(Some(&list), "keyboard").is_some());
    list.owner = None;
    assert!(crate::session::focus_rects_for(Some(&list), "menu").is_some());
}

// --- E23 U3: tabs ---

/// Three tabs in tablist 0 with `selected` on one of them, plus a panel button.
fn tabs(selected: Option<usize>, disabled: &[usize]) -> FocusRectList {
    let mut list = linear_list(true, None);
    for (i, rect) in list.rects.iter_mut().enumerate() {
        rect.tablist = Some(0);
        rect.selected = Some(if selected == Some(i) { 1.0 } else { 0.0 });
        rect.disabled = disabled.contains(&i);
    }
    let mut panel = rect("panel", [0.0, 100.0, 100.0, 20.0], 3, Some(0));
    panel.tablist = None;
    list.rects.push(panel);
    list.groups[0].members.push(3);
    list
}

fn tab_tick(
    fe: &mut UiFocusEngine,
    list: &FocusRectList,
    intents: &[NavIntent],
) -> FocusTickResult {
    fe.tick(
        Some("menu"),
        Some(list),
        intents,
        None,
        &[],
        InputMode::Focus,
        0.0,
    )
}

#[test]
fn a_bumper_activates_the_adjacent_tab_wrapping_and_moves_focus_to_it() {
    let list = tabs(Some(2), &[]);
    let mut fe = UiFocusEngine::new();
    tab_tick(&mut fe, &list, &[]);
    let r = tab_tick(&mut fe, &list, &[NavIntent::TabNext]);
    assert_eq!(
        r.activations,
        vec!["a".to_string()],
        "wraps from the last tab"
    );
    assert_eq!(r.focused.as_deref(), Some("a"));
    let r = tab_tick(&mut fe, &tabs(Some(0), &[]), &[NavIntent::TabPrev]);
    assert_eq!(r.activations, vec!["c".to_string()]);
}

#[test]
fn two_bumper_presses_on_one_frame_advance_two_tabs_and_skip_a_disabled_one() {
    let list = tabs(Some(0), &[]);
    let mut fe = UiFocusEngine::new();
    tab_tick(&mut fe, &list, &[]);
    let r = tab_tick(&mut fe, &list, &[NavIntent::TabNext, NavIntent::TabNext]);
    assert_eq!(r.activations, vec!["b".to_string(), "c".to_string()]);
    assert_eq!(r.focused.as_deref(), Some("c"));

    let list = tabs(Some(0), &[1]);
    let mut fe = UiFocusEngine::new();
    tab_tick(&mut fe, &list, &[]);
    let r = tab_tick(&mut fe, &list, &[NavIntent::TabNext]);
    assert_eq!(
        r.activations,
        vec!["c".to_string()],
        "the disabled tab is skipped"
    );
}

#[test]
fn with_no_tab_selected_rb_activates_the_first_tab_and_lb_the_last() {
    let list = tabs(None, &[]);
    let mut fe = UiFocusEngine::new();
    tab_tick(&mut fe, &list, &[]);
    assert_eq!(
        tab_tick(&mut fe, &list, &[NavIntent::TabNext]).activations,
        vec!["a".to_string()]
    );
    let mut fe = UiFocusEngine::new();
    tab_tick(&mut fe, &list, &[]);
    assert_eq!(
        tab_tick(&mut fe, &list, &[NavIntent::TabPrev]).activations,
        vec!["c".to_string()]
    );
}

#[test]
fn a_lone_tab_takes_focus_without_activating() {
    let list = tabs(Some(0), &[1, 2]);
    let mut fe = UiFocusEngine::new();
    tab_tick(&mut fe, &list, &[]);
    tab_tick(
        &mut fe,
        &list,
        &[NavIntent::Down, NavIntent::Down, NavIntent::Down],
    );
    let r = tab_tick(&mut fe, &list, &[NavIntent::TabNext]);
    assert!(r.activations.is_empty());
    assert_eq!(r.focused.as_deref(), Some("a"));
}

#[test]
fn in_a_tree_with_no_tablist_the_bumpers_step_next_and_prev() {
    let list = linear_list(false, None);
    let mut fe = UiFocusEngine::new();
    tab_tick(&mut fe, &list, &[]);
    let r = tab_tick(&mut fe, &list, &[NavIntent::TabNext]);
    assert!(r.activations.is_empty());
    assert_eq!(r.focused.as_deref(), Some("b"));
    assert_eq!(
        tab_tick(&mut fe, &list, &[NavIntent::TabPrev])
            .focused
            .as_deref(),
        Some("a")
    );
}

#[test]
fn with_a_dialog_over_a_tabbed_menu_the_bumpers_act_on_the_dialog_only() {
    let menu = tabs(Some(0), &[]);
    let dialog = linear_list(false, None);
    let mut fe = UiFocusEngine::new();
    tab_tick(&mut fe, &menu, &[]);
    fe.tick(
        Some("dialog#2"),
        Some(&dialog),
        &[],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    let r = fe.tick(
        Some("dialog#2"),
        Some(&dialog),
        &[NavIntent::TabNext],
        None,
        &[],
        InputMode::Focus,
        0.0,
    );
    assert!(r.activations.is_empty(), "the menu's tabs stay put");
    assert_eq!(r.focused.as_deref(), Some("b"));
}
