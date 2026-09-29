// App-level accessibility panel tests: reserved field actions from any tree,
// engine-routed sliders, settled saves, the first-launch record and the
// missing-entry warning.
// See: context/lib/ui.md §4.1

use log::Level;
use postretro_test_log_capture::LogCapture;
use postretro_ui::demo::{ACCESSIBILITY_PANEL_NAME, build_accessibility_panel_descriptor};
use postretro_ui::modal_stack::ScopeTier;
use postretro_ui::tree::{FocusRect, FocusRectList, FocusRectOwner, NodeInteraction};

use crate::App;
use crate::startup::lifecycle::tests::test_app;

const MOD_MENU: &str = "modOptions";

fn app_with_panel() -> App {
    let mut app = test_app();
    let session = app.session.as_mut().unwrap();
    let registry = session.modal_stack.registry_mut();
    registry.register(
        ACCESSIBILITY_PANEL_NAME,
        build_accessibility_panel_descriptor(),
        ScopeTier::Engine,
        false,
    );
    registry.register(
        MOD_MENU,
        build_accessibility_panel_descriptor(),
        ScopeTier::Mod,
        false,
    );
    app
}

fn owner(name: &str, tier: ScopeTier) -> Option<FocusRectOwner> {
    Some(FocusRectOwner {
        name: name.to_string(),
        tier,
    })
}

fn rect(id: &str, interaction: NodeInteraction) -> FocusRect {
    FocusRect {
        id: id.to_string(),
        rect: [0.0, 0.0, 100.0, 20.0],
        z: 0,
        group: None,
        neighbors: Default::default(),
        interaction: Some(interaction),
        selected: None,
        checked: None,
        disabled: false,
    }
}

/// Last frame's focus export: one button firing `on_press`, owned by `from`.
fn export_button(app: &mut App, on_press: &str, from: Option<FocusRectOwner>) {
    app.session.as_mut().unwrap().ui_focus_rects = Some(FocusRectList {
        rects: vec![rect(
            "control",
            NodeInteraction::Button {
                on_press: on_press.to_string(),
                repeat_on_hold: None,
            },
        )],
        owner: from,
        ..Default::default()
    });
}

fn press(app: &mut App) {
    app.fire_focused_button_activation(Some("control"));
}

fn push(app: &mut App, name: &str) {
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .push_named(name, None);
}

fn limiter(app: &App) -> bool {
    app.session
        .as_ref()
        .unwrap()
        .player_options
        .accessibility
        .flash_limiter
}

const LIMITER_ACTION: &str = "ui.accessibility.cycle.flashLimiter";

#[test]
fn the_limiter_action_from_any_tree_toggles_the_limiter() {
    // The engine panel's own control, a mod tree's button, and a press with no
    // recorded owner all reach the setting the same way.
    let cases = [
        (
            ACCESSIBILITY_PANEL_NAME,
            owner(ACCESSIBILITY_PANEL_NAME, ScopeTier::Engine),
        ),
        (MOD_MENU, owner(MOD_MENU, ScopeTier::Mod)),
        (MOD_MENU, None),
    ];
    for (active, from) in cases {
        let capture = LogCapture::start();
        let mut app = app_with_panel();
        push(&mut app, active);
        export_button(&mut app, LIMITER_ACTION, from.clone());
        press(&mut app);
        assert!(!limiter(&app), "active {active}, owner {from:?}");
        press(&mut app);
        assert!(limiter(&app), "active {active}, owner {from:?}");
        capture.assert_not_logged(Level::Warn, "flash");
    }
}

#[test]
fn another_fields_action_from_a_mod_tree_writes_that_field() {
    let mut app = app_with_panel();
    push(&mut app, MOD_MENU);
    export_button(
        &mut app,
        "ui.accessibility.cycle.monoAudio",
        owner(MOD_MENU, ScopeTier::Mod),
    );
    press(&mut app);
    assert!(
        app.session
            .as_ref()
            .unwrap()
            .player_options
            .accessibility
            .mono_audio
    );
}

#[test]
fn a_mismatched_op_warns_and_writes_nothing() {
    let capture = LogCapture::start();
    let mut app = app_with_panel();
    push(&mut app, ACCESSIBILITY_PANEL_NAME);
    export_button(
        &mut app,
        "ui.accessibility.increase.monoAudio",
        owner(ACCESSIBILITY_PANEL_NAME, ScopeTier::Engine),
    );
    let before = app.session.as_ref().unwrap().player_options.clone();
    press(&mut app);
    assert_eq!(app.session.as_ref().unwrap().player_options, before);
    capture.assert_logged_once(Level::Warn, "does not apply to `monoAudio`");
}

#[test]
fn open_accessibility_pushes_the_panel_once() {
    let mut app = app_with_panel();
    push(&mut app, MOD_MENU);
    export_button(
        &mut app,
        postretro_ui::actions::OPEN_ACCESSIBILITY_ACTION,
        owner(MOD_MENU, ScopeTier::Mod),
    );
    press(&mut app);
    press(&mut app);
    let stack = &app.session.as_ref().unwrap().modal_stack;
    assert_eq!(stack.active_name(), Some(ACCESSIBILITY_PANEL_NAME));
    assert_eq!(stack.len(), 2, "a panel already open is not pushed again");
}

fn export_slider(app: &mut App, slot: &str, from: Option<FocusRectOwner>) {
    app.session.as_mut().unwrap().ui_focus_rects = Some(FocusRectList {
        rects: vec![rect(
            "slider",
            NodeInteraction::Slider {
                slot: slot.to_string(),
                min: 0.0,
                max: 1.0,
                step: 0.1,
                captures_nav: vec!["nav.left".into(), "nav.right".into()],
            },
        )],
        owner: from,
        ..Default::default()
    });
    app.ui_focused_id = Some("slider".to_string());
}

#[test]
fn an_engine_tier_slider_steps_its_field_through_the_field_action() {
    let mut app = app_with_panel();
    push(&mut app, ACCESSIBILITY_PANEL_NAME);
    // The resolved slot the slider binds holds the current value.
    app.update_player_options(0.0, false);
    export_slider(
        &mut app,
        "accessibility.screenShakeScale",
        owner(ACCESSIBILITY_PANEL_NAME, ScopeTier::Engine),
    );
    let mut intents = vec![crate::input::NavIntent::Left];
    app.apply_slider_nav_capture(&mut intents);
    assert!(intents.is_empty(), "the captured step is consumed");
    let scale = app
        .session
        .as_ref()
        .unwrap()
        .player_options
        .accessibility
        .screen_shake_scale;
    assert!((scale - 0.9).abs() < 1e-6, "{scale}");
    assert!(
        app.session
            .as_ref()
            .unwrap()
            .scripting
            .script_ctx
            .system_commands
            .is_empty(),
        "no setState is queued for an engine-routed step"
    );
}

#[test]
fn a_mod_tier_slider_on_a_resolved_slot_warns_and_writes_nothing() {
    // Engine-routed slider steps are engine-tier only: a mod slider bound to a
    // readonly `accessibility.*` slot rides the ordinary `setState` path, where
    // the readonly write gate warns and no-ops.
    let capture = LogCapture::start();
    let mut app = app_with_panel();
    push(&mut app, MOD_MENU);
    app.update_player_options(0.0, false);
    export_slider(
        &mut app,
        "accessibility.screenShakeScale",
        owner(MOD_MENU, ScopeTier::Mod),
    );
    let mut intents = vec![crate::input::NavIntent::Left];
    app.apply_slider_nav_capture(&mut intents);
    assert!(
        !app.session
            .as_ref()
            .unwrap()
            .scripting
            .script_ctx
            .system_commands
            .is_empty(),
        "the step rides setState"
    );
    app.dispatch_system_commands();
    app.update_player_options(0.0, false);
    capture.assert_logged(
        Level::Warn,
        "rejected write to readonly slot `accessibility.screenShakeScale`",
    );
    let session = app.session.as_ref().unwrap();
    assert_eq!(session.player_options.accessibility.screen_shake_scale, 1.0);
}

#[test]
fn a_panel_write_persists_after_the_settle_or_at_once_when_the_panel_closes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let read = |path: &std::path::Path| std::fs::read_to_string(path).unwrap_or_default();

    // Settled: no menu open, the panel stays up.
    let mut app = app_with_panel();
    app.session.as_mut().unwrap().settings_path = Some(path.clone());
    push(&mut app, ACCESSIBILITY_PANEL_NAME);
    app.update_player_options(0.0, false);
    export_button(
        &mut app,
        LIMITER_ACTION,
        owner(ACCESSIBILITY_PANEL_NAME, ScopeTier::Engine),
    );
    press(&mut app);
    app.update_player_options(0.1, false);
    assert!(!read(&path).contains("flash_limiter = false"));
    for _ in 0..3 {
        app.update_player_options(0.1, false);
    }
    let settled = read(&path);
    assert!(settled.contains("flash_limiter = false"), "{settled}");
    assert!(
        !settled.contains("accessibility_panel_shown"),
        "the record waits for a close"
    );

    // Closing first flushes at once and writes the record.
    let mut app = app_with_panel();
    let path2 = dir.path().join("settings2.toml");
    app.session.as_mut().unwrap().settings_path = Some(path2.clone());
    push(&mut app, ACCESSIBILITY_PANEL_NAME);
    app.update_player_options(0.0, false);
    export_button(
        &mut app,
        "ui.accessibility.cycle.monoAudio",
        owner(ACCESSIBILITY_PANEL_NAME, ScopeTier::Engine),
    );
    press(&mut app);
    app.session.as_mut().unwrap().modal_stack.pop();
    app.update_player_options(0.0, false);
    let closed = read(&path2);
    assert!(closed.contains("mono_audio = true"), "{closed}");
    assert!(
        closed.contains("accessibility_panel_shown = true"),
        "{closed}"
    );
}

#[test]
fn an_unreadable_settings_file_is_never_replaced_by_panel_writes_or_close() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let malformed = "not valid toml = = =\n";
    std::fs::write(&path, malformed).unwrap();

    let mut app = app_with_panel();
    let (options, _) = crate::options::PlayerOptions::load_with_status(&path);
    let session = app.session.as_mut().unwrap();
    session.player_options = options;
    session.settings_path = Some(path.clone());
    push(&mut app, ACCESSIBILITY_PANEL_NAME);
    app.update_player_options(0.0, false);
    export_button(
        &mut app,
        "ui.accessibility.cycle.monoAudio",
        owner(ACCESSIBILITY_PANEL_NAME, ScopeTier::Engine),
    );
    press(&mut app);
    for _ in 0..4 {
        app.update_player_options(0.1, false);
    }
    app.session.as_mut().unwrap().modal_stack.pop();
    app.update_player_options(0.0, false);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), malformed);
}

/// A registered menu tree: with or without an accessibility entry button.
fn menu(with_entry: bool) -> postretro_ui::descriptor::AnchoredTree {
    let mut tree = postretro_ui::demo::build_frontend_menu_descriptor();
    if !with_entry {
        let postretro_ui::descriptor::Widget::VStack(root) = &mut tree.root else {
            panic!("fallback frontend root is a vstack");
        };
        root.children.pop();
    }
    tree
}

#[test]
fn a_mod_offering_no_accessibility_warns_once_per_state() {
    use super::accessibility_panel::AccessibilityEntryCheck;
    use postretro_ui::demo::{FRONTEND_MENU_NAME, PAUSE_MENU_NAME};
    use postretro_ui::modal_stack::ModalStack;

    const WARNING: &str = "no mod or level UI tree offers accessibility settings";
    let mut stack = ModalStack::new();
    stack
        .registry_mut()
        .register(FRONTEND_MENU_NAME, menu(true), ScopeTier::Engine, false);
    stack
        .registry_mut()
        .register(PAUSE_MENU_NAME, menu(true), ScopeTier::Engine, false);
    stack.registry_mut().register(
        ACCESSIBILITY_PANEL_NAME,
        build_accessibility_panel_descriptor(),
        ScopeTier::Engine,
        false,
    );
    let mut check = AccessibilityEntryCheck::default();

    // A mod with no trees of its own uses the engine fallback menus, which
    // carry the entry: silent.
    let capture = LogCapture::start();
    check.evaluate(&stack, FRONTEND_MENU_NAME);
    capture.assert_not_logged(Level::Warn, WARNING);

    // Mod init commits a title and pause menu without an entry. The engine
    // panel's own field actions do not count.
    stack
        .registry_mut()
        .register("title", menu(false), ScopeTier::Mod, false);
    stack
        .registry_mut()
        .register(PAUSE_MENU_NAME, menu(false), ScopeTier::Mod, false);
    capture.clear();
    check.evaluate(&stack, "title");
    capture.assert_logged_once(Level::Warn, WARNING);
    capture.assert_logged_once(
        Level::Warn,
        "Players still see the accessibility panel on first launch",
    );

    // A later evaluation in the same state (a level install, a reload that
    // still offers nothing) does not warn again.
    capture.clear();
    check.evaluate(&stack, "title");
    capture.assert_not_logged(Level::Warn, WARNING);

    // UO8: a staged reload adds an options tree with one field action. Any
    // mod or level tree counts, not only the frontend and pause menus.
    let mut options = menu(false);
    let postretro_ui::descriptor::Widget::VStack(root) = &mut options.root else {
        panic!("vstack root");
    };
    root.children.push(postretro_ui::descriptor::Widget::Button(
        serde_json::from_value(serde_json::json!({
            "id": "monoAudio",
            "label": "MONO AUDIO",
            "onPress": "ui.accessibility.cycle.monoAudio"
        }))
        .unwrap(),
    ));
    stack
        .registry_mut()
        .register("modOptions", options, ScopeTier::Mod, false);
    capture.clear();
    check.evaluate(&stack, "title");
    capture.assert_not_logged(Level::Warn, WARNING);

    // A reload that removes it enters the missing state again: one warning.
    stack.replace_script_tree_tier(
        [("title", menu(false)), (PAUSE_MENU_NAME, menu(false))].map(|(name, tree)| {
            postretro_scripting_core::data_descriptors::RegisteredUiTree {
                name: name.to_string(),
                tree,
                always_on: false,
                hide_below: false,
            }
        }),
        ScopeTier::Mod,
    );
    capture.clear();
    check.evaluate(&stack, "title");
    capture.assert_logged_once(Level::Warn, WARNING);

    // A level tree with an entry satisfies the check too.
    stack
        .registry_mut()
        .register("levelMenu", menu(true), ScopeTier::Level, false);
    capture.clear();
    check.evaluate(&stack, "title");
    capture.assert_not_logged(Level::Warn, WARNING);
}
