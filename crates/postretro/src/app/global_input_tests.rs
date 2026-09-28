// The accessibility panel's global input: press-edge toggling, the stack
// rules, same-stage intent discard, and splash/Loading frames dropping input.
// See: context/lib/input.md §5

use postretro_ui::demo::{ACCESSIBILITY_PANEL_NAME, build_accessibility_panel_descriptor};
use postretro_ui::modal_stack::ScopeTier;
use winit::keyboard::KeyCode;

use super::global_input::{PANEL_TOGGLE_KEY, is_panel_toggle_press};
use crate::App;
use crate::input::{NavIntent, UiIntentPayload};
use crate::startup::BootState;
use crate::startup::lifecycle::tests::test_app;

const TEXT_ENTRY: &str = "nameEntry";

fn app() -> App {
    let mut app = test_app();
    let registry = app.session.as_mut().unwrap().modal_stack.registry_mut();
    registry.register(
        ACCESSIBILITY_PANEL_NAME,
        build_accessibility_panel_descriptor(),
        ScopeTier::Engine,
        false,
    );
    let mut text_entry = build_accessibility_panel_descriptor();
    text_entry.text_entry_target = Some("ui.textEntry".to_string());
    registry.register(TEXT_ENTRY, text_entry, ScopeTier::Mod, false);
    app
}

fn active(app: &App) -> Option<&str> {
    app.session.as_ref().unwrap().modal_stack.active_name()
}

/// One frame's intake of an F1 key event, then its game logic.
fn f1_frame(app: &mut App, pressed: bool, repeat: bool) {
    if is_panel_toggle_press(PANEL_TOGGLE_KEY, pressed, repeat) {
        app.request_panel_toggle();
    }
    app.apply_panel_toggle();
}

#[test]
fn only_the_f1_press_edge_is_the_global_input() {
    assert!(is_panel_toggle_press(KeyCode::F1, true, false));
    assert!(!is_panel_toggle_press(KeyCode::F1, true, true), "OS repeat");
    assert!(!is_panel_toggle_press(KeyCode::F1, false, false), "release");
    assert!(!is_panel_toggle_press(KeyCode::F2, true, false));
}

#[test]
fn f1_held_over_text_entry_toggles_once_each_way() {
    // G1: text entry is open; F1 opens the panel over it, repeats do nothing,
    // and after the release a second press closes the panel, revealing the
    // text-entry modal, which repeats never reopen it over.
    let mut app = app();
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .push_named(TEXT_ENTRY, None);

    f1_frame(&mut app, true, false);
    assert_eq!(active(&app), Some(ACCESSIBILITY_PANEL_NAME));
    for _ in 0..5 {
        f1_frame(&mut app, true, true);
    }
    assert_eq!(active(&app), Some(ACCESSIBILITY_PANEL_NAME));
    f1_frame(&mut app, false, false);

    f1_frame(&mut app, true, false);
    assert_eq!(active(&app), Some(TEXT_ENTRY));
    for _ in 0..5 {
        f1_frame(&mut app, true, true);
    }
    assert_eq!(
        active(&app),
        Some(TEXT_ENTRY),
        "a held F1 does not reopen the panel"
    );
}

#[test]
fn the_global_input_does_nothing_while_the_panel_is_open_beneath_another_tree() {
    let mut app = app();
    let stack = &mut app.session.as_mut().unwrap().modal_stack;
    stack.push_named(ACCESSIBILITY_PANEL_NAME, None);
    stack.push_named(TEXT_ENTRY, None);
    f1_frame(&mut app, true, false);
    let stack = &app.session.as_ref().unwrap().modal_stack;
    assert_eq!(stack.active_name(), Some(TEXT_ENTRY));
    assert_eq!(stack.len(), 2);
}

#[test]
fn input_pressed_on_a_loading_frame_opens_nothing_on_the_first_running_frame() {
    // G2: F1 or Select on a Loading frame is dropped, never latched.
    let mut app = app();
    app.boot_state = BootState::Loading;
    app.request_panel_toggle();
    {
        let dispatch = &mut app.session.as_mut().unwrap().ui_dispatch;
        dispatch.enqueue_intent(UiIntentPayload::Nav(NavIntent::Confirm));
    }
    app.drop_ui_input_on_non_ui_frame();

    app.boot_state = BootState::Running;
    app.apply_panel_toggle();
    assert_eq!(active(&app), None);
    let dispatch = &mut app.session.as_mut().unwrap().ui_dispatch;
    dispatch.advance_frame();
    assert!(
        dispatch.take_ready().is_empty(),
        "no queued UI input survives"
    );
}

#[test]
fn a_confirm_in_the_toggles_input_stage_activates_nothing_in_either_direction() {
    // G3: the confirm was captured in the same Input stage as F1, so after the
    // frame's promotion it waits in `ready`. The toggle discards it, whether it
    // opened the panel or closed it to reveal the tree beneath.
    let mut app = app();
    for expected in [Some(ACCESSIBILITY_PANEL_NAME), None] {
        {
            let dispatch = &mut app.session.as_mut().unwrap().ui_dispatch;
            dispatch.enqueue_intent(UiIntentPayload::Nav(NavIntent::Confirm));
            let _ = dispatch.take_ready();
            dispatch.advance_frame();
        }
        f1_frame(&mut app, true, false);
        assert_eq!(active(&app), expected);
        let dispatch = &mut app.session.as_mut().unwrap().ui_dispatch;
        assert!(dispatch.take_ready().is_empty());
    }
}
