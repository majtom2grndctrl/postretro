// Text shortcut tests: activation through the keyboard tree's keys, no effect
// without text entry or after a same-frame commit (P26), and held repeat.
// See: context/lib/input.md §7

use postretro_ui::descriptor::{AnchoredTree, CaptureMode};
use postretro_ui::modal_stack::ScopeTier;
use postretro_ui::tree::{FocusRect, FocusRectList, FocusRectOwner, NodeInteraction, RepeatPolicy};

use crate::App;
use crate::input::{Command, NavIntent, UiIntent, UiIntentPayload};
use crate::startup::lifecycle::tests::test_app;

const KEYBOARD: &str = "testKeyboard";
/// An observable stand-in for the authored key reaction: each activation
/// steps screen shake down by 0.1 from 1.0.
const COUNTING_ACTION: &str = "ui.accessibility.decrease.screenShakeScale";

fn key(id: &str, on_press: &str, repeat_on_hold: Option<RepeatPolicy>) -> FocusRect {
    FocusRect {
        id: id.to_string(),
        rect: [0.0, 0.0, 40.0, 20.0],
        z: 0,
        group: None,
        neighbors: Default::default(),
        interaction: Some(NodeInteraction::Button {
            on_press: on_press.to_string(),
            repeat_on_hold,
        }),
        selected: None,
        checked: None,
        disabled: false,
        tablist: None,
        clip: None,
    }
}

fn tree(text_entry: bool) -> AnchoredTree {
    let mut tree: AnchoredTree = serde_json::from_value(serde_json::json!({
        "anchor": "center",
        "offset": [0.0, 0.0],
        "captureMode": "capture",
        "root": { "kind": "text", "content": "", "fontSize": 12.0, "color": "ok" },
    }))
    .unwrap();
    assert_eq!(tree.capture_mode, CaptureMode::Capture);
    if text_entry {
        tree.text_entry_target = Some("ui.textEntry".to_string());
    }
    tree
}

/// A keyboard tree on top whose focus export carries the three shortcut keys.
fn app_with_keyboard(text_entry: bool) -> App {
    let mut app = test_app();
    let session = app.session.as_mut().unwrap();
    session.modal_stack.push(KEYBOARD, tree(text_entry));
    session.ui_focus_rects = Some(FocusRectList {
        rects: vec![
            key("key_q", COUNTING_ACTION, None),
            key(
                "key_backspace",
                COUNTING_ACTION,
                Some(RepeatPolicy {
                    initial_delay_ms: 400.0,
                    interval_ms: 100.0,
                }),
            ),
            key("key_space", COUNTING_ACTION, None),
            key(
                "key_done",
                postretro_ui::actions::COMMIT_TEXT_ENTRY_ACTION,
                None,
            ),
        ],
        owner: Some(FocusRectOwner {
            name: KEYBOARD.to_string(),
            tier: ScopeTier::Engine,
        }),
        ..Default::default()
    });
    app.ui_focused_id = Some("key_q".to_string());
    app
}

fn activations(app: &App) -> u32 {
    let scale = app
        .session
        .as_ref()
        .unwrap()
        .player_options
        .accessibility
        .screen_shake_scale;
    ((1.0 - scale) * 10.0).round() as u32
}

fn shortcut(seq: u64, command: Command) -> UiIntent {
    UiIntent {
        seq,
        payload: UiIntentPayload::TextShortcut(command),
    }
}

#[test]
fn a_shortcut_activates_its_key_without_moving_focus() {
    let mut app = app_with_keyboard(true);
    app.apply_text_shortcuts(&[shortcut(0, Command::TextSpace)], 0.016);
    assert_eq!(activations(&app), 1);
    assert_eq!(app.ui_focused_id.as_deref(), Some("key_q"));

    app.apply_text_shortcuts(&[shortcut(1, Command::TextCommit)], 0.016);
    assert_eq!(
        app.session.as_ref().unwrap().modal_stack.active_name(),
        None,
        "the commit key closes text entry"
    );
}

#[test]
fn shortcuts_do_nothing_without_a_text_entry_tree_on_top() {
    let mut app = app_with_keyboard(false);
    app.apply_text_shortcuts(
        &[
            shortcut(0, Command::TextSpace),
            shortcut(1, Command::TextBackspace),
        ],
        0.016,
    );
    assert_eq!(activations(&app), 0);
}

#[test]
fn a_space_on_the_frame_text_entry_commits_adds_nothing() {
    // P26: the commit resolves first and pops the tree.
    let mut app = app_with_keyboard(true);
    app.ui_focused_id = None;
    let intents = [
        UiIntent {
            seq: 0,
            payload: UiIntentPayload::Nav(NavIntent::Confirm),
        },
        shortcut(1, Command::TextSpace),
    ];
    assert!(app.resolve_text_entry_intents(&intents));
    app.apply_text_shortcuts(&intents, 0.016);
    assert_eq!(activations(&app), 0);
}

#[test]
fn a_held_backspace_shortcut_repeats_as_the_key_does() {
    let mut app = app_with_keyboard(true);
    app.apply_text_shortcuts(&[shortcut(0, Command::TextBackspace)], 0.0);
    assert_eq!(activations(&app), 1);
    app.apply_text_shortcuts(&[], 0.3);
    assert_eq!(activations(&app), 1, "inside the initial delay");
    app.apply_text_shortcuts(&[], 0.15);
    assert_eq!(activations(&app), 2);
    app.apply_text_shortcuts(&[], 0.1);
    assert_eq!(activations(&app), 3);
    app.session
        .as_mut()
        .unwrap()
        .ui_focus
        .release_shortcut_repeat();
    app.apply_text_shortcuts(&[], 1.0);
    assert_eq!(activations(&app), 3, "release stops the repeat");

    // A key without repeatOnHold fires once.
    app.apply_text_shortcuts(&[shortcut(1, Command::TextSpace)], 0.0);
    app.apply_text_shortcuts(&[], 1.0);
    assert_eq!(activations(&app), 4);
}
