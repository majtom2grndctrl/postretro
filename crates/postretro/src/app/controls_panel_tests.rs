// Controls panel tests: rows from the effective table, the capture prompt's
// intake and resolution, conflicts and guard refusals, reset, and saved rows.
// See: context/lib/input.md §2 · context/lib/player_options.md §6

use gilrs::Button;
use winit::keyboard::KeyCode;

use super::*;
use crate::input::{
    Activator, AuthorLayer, BindingSources, DeviceClass, EffectiveTable, PlayerLayer,
    RelevanceFacts,
};
use crate::startup::lifecycle::tests::test_app;
use postretro_ui::actions::ControlsAction;
use postretro_ui::demo::{CONTROLS_CAPTURE_NAME, CONTROLS_DIALOG_NAME, CONTROLS_PANEL_NAME};

const KBM: DeviceClass = DeviceClass::KeyboardMouse;
const PAD: DeviceClass = DeviceClass::Gamepad;

fn key(code: KeyCode) -> PhysicalInput {
    PhysicalInput::Key(code)
}

fn pad(button: Button) -> PhysicalInput {
    PhysicalInput::GamepadButton(button)
}

fn all_facts() -> RelevanceFacts {
    RelevanceFacts {
        dash: true,
        crouch: true,
        magazine: true,
        secondary: true,
    }
}

fn author_binding(input: PhysicalInput, kind: ActivatorKind) -> crate::input::AuthorBinding {
    crate::input::AuthorBinding {
        input,
        activator: Activator::new(kind),
    }
}

fn listed(rows: &[ControlsRow]) -> Vec<Command> {
    rows.iter().map(|row| row.command).collect()
}

fn descriptor_text(tree: &AnchoredTree) -> String {
    serde_json::to_string(tree).unwrap()
}

#[test]
fn rows_list_only_relevant_commands() {
    let author = AuthorLayer::default();
    let without = controls_rows(
        &EffectiveTable::build(&author, &PlayerLayer::default(), RelevanceFacts::default(), false),
        &author,
    );
    let with = controls_rows(
        &EffectiveTable::build(&author, &PlayerLayer::default(), all_facts(), false),
        &author,
    );
    assert!(!listed(&without).contains(&Command::Dash));
    assert!(listed(&with).contains(&Command::Dash));
    for rows in [&without, &with] {
        assert!(
            !listed(rows).contains(&Command::MoveUp),
            "the dev fly-cam commands are never listed"
        );
    }
}

#[test]
fn rows_follow_author_labels_categories_and_order() {
    let mut author = AuthorLayer::default();
    for (command, label, category, order) in [
        (Command::Reload, "RELOAD WEAPON", "COMBAT", 1.0),
        (Command::Jump, "HOP", "MOVEMENT", 2.0),
        (Command::Shoot, "FIRE", "COMBAT", 3.0),
    ] {
        author.presentation.insert(
            command,
            crate::input::CommandPresentation {
                label: Some(label.to_string()),
                category: Some(category.to_string()),
                order: Some(order),
            },
        );
    }
    let table = EffectiveTable::build(&author, &PlayerLayer::default(), all_facts(), false);
    let rows = controls_rows(&table, &author);
    let leading: Vec<(&str, &str)> = rows
        .iter()
        .take(3)
        .map(|row| (row.label.as_str(), row.category.as_str()))
        .collect();
    assert_eq!(
        leading,
        [
            ("RELOAD WEAPON", "COMBAT"),
            ("FIRE", "COMBAT"),
            ("HOP", "MOVEMENT"),
        ],
        "a category sits where its first command does and keeps its commands together"
    );
    let forward = rows
        .iter()
        .find(|row| row.command == Command::MoveForward)
        .unwrap();
    assert_eq!(forward.label, "MOVE FORWARD");
    assert_eq!(forward.category, "GAMEPLAY");
}

#[test]
fn each_slot_shows_its_activator_read_only() {
    let mut author = AuthorLayer::default();
    author.defaults.insert(
        (Command::Sprint, KBM),
        vec![author_binding(key(KeyCode::ShiftLeft), ActivatorKind::Hold)],
    );
    author.defaults.insert(
        (Command::Dash, KBM),
        vec![author_binding(key(KeyCode::ShiftLeft), ActivatorKind::Tap)],
    );
    let table = EffectiveTable::build(&author, &PlayerLayer::default(), all_facts(), false);
    let panel = descriptor_text(&build_controls_panel(&controls_rows(&table, &author), None));
    assert!(panel.contains("\"SHIFT LEFT (HOLD)\""), "{panel}");
    assert!(panel.contains("\"SHIFT LEFT (TAP)\""), "{panel}");
    assert!(
        !panel.contains("ui.controls.activator"),
        "the panel offers no activator edit"
    );
}

#[test]
fn a_player_binding_that_took_an_author_default_flags_the_displaced_row() {
    // PD7: the player bound Q to dash; a later author default puts reload on Q.
    let mut author = AuthorLayer::default();
    author.defaults.insert(
        (Command::Reload, KBM),
        vec![author_binding(key(KeyCode::KeyQ), ActivatorKind::Press)],
    );
    let mut player = PlayerLayer::default();
    player
        .rows
        .insert((Command::Dash, KBM), vec![Some(key(KeyCode::KeyQ))]);
    let table = EffectiveTable::build(&author, &player, all_facts(), false);
    let rows = controls_rows(&table, &author);
    let flagged: Vec<Command> = rows
        .iter()
        .filter(|row| row.displaced)
        .map(|row| row.command)
        .collect();
    assert_eq!(flagged, [Command::Reload]);
    let panel = descriptor_text(&build_controls_panel(&rows, None));
    assert!(panel.contains("\"RELOAD !\""), "{panel}");
    assert!(panel.contains("TAKEN BY ONE OF YOUR BINDINGS"));

    // PD8: a later author hold on the player's key yields and is flagged too.
    let mut author = AuthorLayer::default();
    author.defaults.insert(
        (Command::Reload, KBM),
        vec![author_binding(key(KeyCode::KeyQ), ActivatorKind::Hold)],
    );
    let table = EffectiveTable::build(&author, &player, all_facts(), false);
    assert!(
        controls_rows(&table, &author)
            .iter()
            .any(|row| row.command == Command::Reload && row.displaced)
    );
}

#[test]
fn changed_rows_write_names_and_remove_reset_rows() {
    let mut old = PlayerLayer::default();
    old.rows
        .insert((Command::Jump, KBM), vec![Some(key(KeyCode::KeyV))]);
    old.rows
        .insert((Command::Use, PAD), vec![Some(pad(Button::West))]);
    let mut new = old.clone();
    new.rows.remove(&(Command::Jump, KBM));
    new.rows.insert(
        (Command::Dash, PAD),
        vec![Some(pad(Button::North)), Some(pad(Button::LeftThumb))],
    );
    assert_eq!(
        changed_rows(&old, &new),
        [
            (
                PAD,
                Command::Dash,
                Some(vec!["north".to_string(), "left_stick_press".to_string()])
            ),
            (KBM, Command::Jump, None),
        ]
    );
}

// --- capture -------------------------------------------------------------

fn open_capture(app: &mut App, command: Command, class: DeviceClass, slot: usize) {
    app.open_controls_panel();
    app.apply_controls_action(ControlsAction::Capture {
        command: command.id(),
        class: class.settings_key(),
        slot,
    });
}

fn session(app: &App) -> &crate::session::Session {
    app.session.as_ref().unwrap()
}

fn inputs(app: &App, command: Command, class: DeviceClass) -> Vec<PhysicalInput> {
    session(app).bindings.table().inputs(command, class)
}

#[test]
fn a_press_before_the_prompt_opens_is_not_captured() {
    // P7: the press that opens the prompt, and any press resolved before the
    // prompt is the active tree, never reach it.
    let mut app = test_app();
    app.open_controls_panel();
    let session_mut = app.session.as_mut().unwrap();
    assert!(!session_mut.offer_capture_press(pad(Button::South)));
    app.apply_controls_action(ControlsAction::Capture {
        command: Command::Jump.id(),
        class: PAD.settings_key(),
        slot: 0,
    });
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_CAPTURE_NAME)
    );
    app.update_controls_panel();
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_CAPTURE_NAME),
        "nothing captured on the frame the prompt opened"
    );
    // A press once the prompt is the active tree is captured.
    assert!(
        app.session
            .as_mut()
            .unwrap()
            .offer_capture_press(pad(Button::RightTrigger))
    );
    app.update_controls_panel();
    assert_eq!(inputs(&app, Command::Jump, PAD), [pad(Button::RightTrigger)]);
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_PANEL_NAME)
    );
}

#[test]
fn capturing_start_or_escape_binds_it_and_nothing_else() {
    // P8: a menu or cancel input binds and is routed nowhere else: no menu
    // opens or closes, and no prompt or dialog follows.
    // Escape and Start bind cleanly (menu and cancel never conflict with a
    // gameplay command); South and Select also drive jump and drop, so the
    // capture raises a question it does not answer.
    let cases = [
        (KBM, key(KeyCode::Escape), false),
        (PAD, pad(Button::Start), false),
        (PAD, pad(Button::South), true),
        (PAD, pad(Button::Select), true),
    ];
    for (class, input, asks) in cases {
        let mut app = test_app();
        open_capture(&mut app, Command::Use, class, 1);
        let session_mut = app.session.as_mut().unwrap();
        assert!(session_mut.capture_prompt_is_active());
        assert!(session_mut.offer_capture_press(input));
        app.update_controls_panel();
        // A later frame: the captured press reached no other consumer.
        app.update_controls_panel();
        let stack = &session(&app).modal_stack;
        if asks {
            assert_eq!(stack.active_name(), Some(CONTROLS_DIALOG_NAME), "{input:?}");
            assert_eq!(stack.len(), 2, "{input:?}: the prompt closed, no menu opened");
            assert!(session(&app).controls.pending_replace.is_some());
            assert_eq!(inputs(&app, Command::Use, class).len(), 1, "{input:?}");
        } else {
            assert_eq!(stack.active_name(), Some(CONTROLS_PANEL_NAME), "{input:?}");
            assert_eq!(stack.len(), 1, "{input:?}: no menu opened");
            assert_eq!(inputs(&app, Command::Use, class)[1], input);
        }
    }
}

#[test]
fn an_irrelevant_command_opens_no_prompt() {
    let mut app = test_app();
    // Reload is irrelevant without a magazine weapon.
    open_capture(&mut app, Command::Reload, PAD, 1);
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_PANEL_NAME)
    );
}

#[test]
fn a_conflict_replace_takes_the_input_from_its_holder() {
    let mut app = test_app();
    open_capture(&mut app, Command::Use, KBM, 0);
    app.session
        .as_mut()
        .unwrap()
        .offer_capture_press(key(KeyCode::Space));
    app.update_controls_panel();
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_DIALOG_NAME)
    );
    app.apply_controls_action(ControlsAction::Replace);
    assert_eq!(inputs(&app, Command::Use, KBM), [key(KeyCode::Space)]);
    assert!(inputs(&app, Command::Jump, KBM).is_empty());
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_PANEL_NAME)
    );
}

#[test]
fn rebinding_confirm_to_cancels_only_button_is_refused() {
    // AV10: East is cancel's only gamepad binding; confirm on East would leave
    // cancel unbound, so the panel refuses and both bindings stay.
    let mut app = test_app();
    open_capture(&mut app, Command::NavConfirm, PAD, 0);
    app.session
        .as_mut()
        .unwrap()
        .offer_capture_press(pad(Button::East));
    app.update_controls_panel();
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_DIALOG_NAME)
    );
    assert!(session(&app).controls.pending_replace.is_none());
    assert_eq!(inputs(&app, Command::NavConfirm, PAD), [pad(Button::South)]);
    assert_eq!(inputs(&app, Command::NavCancel, PAD), [pad(Button::East)]);
}

#[test]
fn with_the_swap_on_capturing_south_for_confirm_makes_south_confirm() {
    let mut app = test_app();
    app.session
        .as_mut()
        .unwrap()
        .bindings
        .set_swap_confirm_cancel(true);
    app.refresh_effective_bindings();
    assert_eq!(inputs(&app, Command::NavConfirm, PAD), [pad(Button::East)]);
    // Cancel keeps a second button, so taking South from it breaks no guard.
    open_capture(&mut app, Command::NavCancel, PAD, 1);
    app.session
        .as_mut()
        .unwrap()
        .offer_capture_press(pad(Button::LeftThumb));
    app.update_controls_panel();
    assert_eq!(
        inputs(&app, Command::NavCancel, PAD),
        [pad(Button::South), pad(Button::LeftThumb)]
    );
    open_capture(&mut app, Command::NavConfirm, PAD, 0);
    app.session
        .as_mut()
        .unwrap()
        .offer_capture_press(pad(Button::South));
    app.update_controls_panel();
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_DIALOG_NAME),
        "South is cancel's, so the player is asked"
    );
    app.apply_controls_action(ControlsAction::Replace);
    assert_eq!(inputs(&app, Command::NavConfirm, PAD), [pad(Button::South)]);
    assert_eq!(inputs(&app, Command::NavCancel, PAD), [pad(Button::LeftThumb)]);
    assert_eq!(
        session(&app).bindings.ui_nav().command_for(
            pad(Button::South),
            crate::input::UiNavContext::Capture
        ),
        Some(Command::NavConfirm)
    );
}

#[test]
fn pressing_the_current_input_again_keeps_it() {
    let mut app = test_app();
    open_capture(&mut app, Command::Jump, KBM, 0);
    app.session
        .as_mut()
        .unwrap()
        .offer_capture_press(key(KeyCode::Space));
    app.update_controls_panel();
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_PANEL_NAME)
    );
    assert!(session(&app).bindings.player().rows.is_empty());
}

#[test]
fn a_prompt_whose_command_becomes_irrelevant_closes_without_binding() {
    // P25: dash leaves the list while its prompt is open.
    let mut app = test_app();
    {
        let session = app.session.as_mut().unwrap();
        session.bindings.rebuild(
            BindingSources {
                entity_types_generation: u64::MAX,
                tuning: None,
            },
            all_facts(),
            &mut session.input_system,
        );
        session.controls.listed.clear();
        let rows = controls_rows(session.bindings.table(), session.bindings.author());
        session.controls.listed = rows.iter().map(|row| row.command).collect();
        session.controls.built_generation = Some(session.bindings.generation());
        session
            .modal_stack
            .push(CONTROLS_PANEL_NAME, build_controls_panel(&rows, None));
    }
    app.apply_controls_action(ControlsAction::Capture {
        command: Command::Dash.id(),
        class: KBM.settings_key(),
        slot: 0,
    });
    assert!(session(&app).capture_prompt_is_active());
    // The registry rebuild drops dash, and a press lands on the same frame.
    app.session
        .as_mut()
        .unwrap()
        .offer_capture_press(key(KeyCode::KeyV));
    app.refresh_effective_bindings();
    app.update_controls_panel();
    let stack = &session(&app).modal_stack;
    assert_eq!(stack.active_name(), Some(CONTROLS_PANEL_NAME));
    assert!(session(&app).bindings.player().rows.is_empty());
    let panel = descriptor_text(&build_controls_panel(
        &controls_rows(session(&app).bindings.table(), session(&app).bindings.author()),
        None,
    ));
    assert!(!panel.contains("ctl_dash_"));
    let pushed = stack.retained_descriptors().last().unwrap();
    let next = Command::ALL
        .into_iter()
        .skip_while(|command| *command != Command::Dash)
        .skip(1)
        .find(|command| {
            session(&app).bindings.table().relevance(*command) == Relevance::Relevant
        })
        .unwrap();
    assert_eq!(
        pushed.initial_focus.as_deref(),
        Some(slot_id(next, KBM, 0).as_str()),
        "focus moves to the nearest remaining row"
    );
}

#[test]
fn reset_returns_a_command_to_its_defaults_and_reset_all_clears_every_row() {
    let mut app = test_app();
    open_capture(&mut app, Command::Use, KBM, 1);
    app.session
        .as_mut()
        .unwrap()
        .offer_capture_press(key(KeyCode::KeyV));
    app.update_controls_panel();
    assert_eq!(
        inputs(&app, Command::Use, KBM),
        [key(KeyCode::KeyE), key(KeyCode::KeyV)]
    );
    app.apply_controls_action(ControlsAction::Reset {
        command: Command::Use.id(),
    });
    assert_eq!(inputs(&app, Command::Use, KBM), [key(KeyCode::KeyE)]);

    open_capture(&mut app, Command::Use, PAD, 1);
    app.session
        .as_mut()
        .unwrap()
        .offer_capture_press(pad(Button::DPadUp));
    app.update_controls_panel();
    if session(&app).modal_stack.active_name() == Some(CONTROLS_DIALOG_NAME) {
        app.apply_controls_action(ControlsAction::Replace);
    }
    assert!(!session(&app).bindings.player().rows.is_empty());
    app.apply_controls_action(ControlsAction::ResetAll);
    assert!(session(&app).bindings.player().rows.is_empty());
}

#[test]
fn the_pushed_panel_rebuilds_when_the_table_changes() {
    let mut app = test_app();
    open_capture(&mut app, Command::Use, KBM, 1);
    app.session
        .as_mut()
        .unwrap()
        .offer_capture_press(key(KeyCode::KeyV));
    app.update_controls_panel();
    let stack = &session(&app).modal_stack;
    assert_eq!(stack.len(), 1, "the panel is updated in place, not pushed again");
    let panel = descriptor_text(stack.retained_descriptors().last().unwrap());
    assert!(panel.contains("\"label\":\"V\""), "{panel}");
}

#[test]
fn a_mod_tree_under_the_reserved_name_never_replaces_the_engine_panel() {
    // MC19: the registry rejects the name at mod scope, and ui.openControls
    // still opens the engine panel.
    use log::Level;
    use postretro_scripting_core::data_descriptors::RegisteredUiTree;
    use postretro_test_log_capture::LogCapture;
    use postretro_ui::modal_stack::ScopeTier;

    let capture = LogCapture::start();
    let mut app = test_app();
    let impostor = build_refusal_dialog("IMPOSTOR", KBM);
    app.session.as_mut().unwrap().modal_stack.register_script_trees(
        [RegisteredUiTree {
            name: CONTROLS_PANEL_NAME.to_string(),
            tree: impostor,
            always_on: false,
            hide_below: false,
        }],
        ScopeTier::Mod,
    );
    capture.assert_logged(Level::Warn, "reserved for an engine panel or dialog");
    app.open_controls_panel();
    let stack = &session(&app).modal_stack;
    assert_eq!(stack.active_name(), Some(CONTROLS_PANEL_NAME));
    assert_eq!(stack.active_tier(), Some(ScopeTier::Engine));
    let panel = descriptor_text(stack.retained_descriptors().last().unwrap());
    assert!(panel.contains("ctl_resetAll") && !panel.contains("IMPOSTOR"));
}
