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
use crate::startup::lifecycle::tests::test_app as bare_test_app;
use postretro_ui::actions::ControlsAction;
use postretro_ui::demo::{CONTROLS_CAPTURE_NAME, CONTROLS_DIALOG_NAME, CONTROLS_PANEL_NAME};

/// A test app with the engine's controls panel shell registered, as boot does.
pub(crate) fn test_app() -> App {
    let mut app = bare_test_app();
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .registry_mut()
        .register(
            CONTROLS_PANEL_NAME,
            shell(),
            postretro_ui::modal_stack::ScopeTier::Engine,
            false,
        );
    app
}

fn shell() -> AnchoredTree {
    postretro_ui::demo::build_controls_panel_shell()
}

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
        &EffectiveTable::build(
            &author,
            &PlayerLayer::default(),
            RelevanceFacts::default(),
            false,
        ),
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
    let panel = descriptor_text(&build_controls_panel(
        &shell(),
        &controls_rows(&table, &author),
        None,
    ));
    assert!(panel.contains("\"SHIFT LEFT (HOLD)\""), "{panel}");
    assert!(panel.contains("\"SHIFT LEFT (TAP)\""), "{panel}");
    assert!(
        !panel.contains("ui.controls.activator"),
        "the panel offers no activator edit"
    );
}

#[test]
fn a_player_binding_that_took_an_author_default_flags_the_displaced_row() {
    // The player bound Q to dash; a later author default puts reload on Q.
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
    let flagged: Vec<(Command, [bool; 2])> = rows
        .iter()
        .filter(|row| row.displaced.contains(&true))
        .map(|row| (row.command, row.displaced))
        .collect();
    assert_eq!(flagged, [(Command::Reload, [true, false])]);
    let panel = descriptor_text(&build_controls_panel(&shell(), &rows, None));
    assert!(panel.contains("\"RELOAD ! \u{b7} KEY 1\""), "{panel}");
    assert!(
        panel.contains("\"RELOAD ! \u{b7} KEY 2\""),
        "every slot row of the flagged class carries the mark: {panel}"
    );
    assert!(
        panel.contains("\"RELOAD \u{b7} PAD 1\""),
        "the gamepad rows lost nothing and carry no mark: {panel}"
    );
    assert!(panel.contains("ANOTHER BINDING TOOK AN INPUT FROM THIS COMMAND"));

    // A later author hold on the player's key yields and is flagged too.
    let mut author = AuthorLayer::default();
    author.defaults.insert(
        (Command::Reload, KBM),
        vec![author_binding(key(KeyCode::KeyQ), ActivatorKind::Hold)],
    );
    let table = EffectiveTable::build(&author, &player, all_facts(), false);
    assert!(
        controls_rows(&table, &author)
            .iter()
            .any(|row| row.command == Command::Reload && row.displaced[0])
    );
}

#[test]
fn each_slot_is_its_own_row_and_reset_sits_on_the_commands_first() {
    let author = AuthorLayer::default();
    let table = EffectiveTable::build(&author, &PlayerLayer::default(), all_facts(), false);
    let rows = controls_rows(&table, &author);
    let mut panel = serde_json::to_value(build_controls_panel(&shell(), &rows, None)).unwrap();
    let grid = find_widget(&mut panel["root"], ROWS_GRID_ID).unwrap();
    assert_eq!(grid["cols"], json!(3));
    let cells = grid["children"].as_array().unwrap();
    assert_eq!(cells.len() % 3, 0, "every grid row fills its three columns");
    let grid_rows: Vec<&[Value]> = cells.chunks(3).collect();
    assert_eq!(
        grid_rows[0][0]["content"],
        json!("GAMEPLAY"),
        "a category heading leads, with no column-header row"
    );
    let prefix = format!("ctl_{}_", Command::MoveForward.id());
    let forward: Vec<&[Value]> = grid_rows
        .iter()
        .copied()
        .filter(|row| {
            row[1]["id"]
                .as_str()
                .is_some_and(|id| id.starts_with(&prefix))
        })
        .collect();
    let labels: Vec<&str> = forward
        .iter()
        .map(|row| row[0]["content"].as_str().unwrap())
        .collect();
    assert_eq!(
        labels,
        [
            "MOVE FORWARD \u{b7} KEY 1",
            "MOVE FORWARD \u{b7} KEY 2",
            "MOVE FORWARD \u{b7} PAD 1",
            "MOVE FORWARD \u{b7} PAD 2",
        ]
    );
    let ids: Vec<&str> = forward
        .iter()
        .map(|row| row[1]["id"].as_str().unwrap())
        .collect();
    let expected: Vec<String> = [(KBM, 0), (KBM, 1), (PAD, 0), (PAD, 1)]
        .into_iter()
        .map(|(class, slot)| slot_id(Command::MoveForward, class, slot))
        .collect();
    assert_eq!(ids, expected);
    assert_eq!(
        forward[0][2]["id"],
        json!(format!("ctl_{}_reset", Command::MoveForward.id()))
    );
    for row in &forward[1..] {
        assert_eq!(
            row[2]["kind"],
            json!("spacer"),
            "RESET shows once per command"
        );
    }
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
    // The press that opens the prompt, and any press resolved before the
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
    assert_eq!(
        inputs(&app, Command::Jump, PAD),
        [pad(Button::RightTrigger)]
    );
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_PANEL_NAME)
    );
}

#[test]
fn a_captured_menu_or_cancel_input_reaches_nothing_else() {
    // A menu or cancel input the prompt captures is routed nowhere else: no
    // menu opens or closes, and only the capture's own answer follows.
    // Menu is live in gameplay too, so Escape or Start on `use` would take
    // menu's only input there: refused. South and Select also drive jump and
    // drop, so those captures ask. Start on cancel takes it from text commit
    // and, replaced, binds.
    enum Expect {
        Refused,
        Asks,
    }
    let cases = [
        (Command::Use, KBM, key(KeyCode::Escape), Expect::Refused),
        (Command::Use, PAD, pad(Button::Start), Expect::Refused),
        (Command::Use, PAD, pad(Button::South), Expect::Asks),
        (Command::Use, PAD, pad(Button::Select), Expect::Asks),
        (Command::NavCancel, PAD, pad(Button::Start), Expect::Asks),
    ];
    for (command, class, input, expect) in cases {
        let mut app = test_app();
        open_capture(&mut app, command, class, 1);
        let session_mut = app.session.as_mut().unwrap();
        assert!(session_mut.capture_prompt_is_active());
        assert!(session_mut.offer_capture_press(input));
        app.update_controls_panel();
        // A later frame: the captured press reached no other consumer.
        app.update_controls_panel();
        let stack = &session(&app).modal_stack;
        assert_eq!(stack.active_name(), Some(CONTROLS_DIALOG_NAME), "{input:?}");
        assert_eq!(
            stack.len(),
            2,
            "{input:?}: the prompt closed, no menu opened"
        );
        assert_eq!(inputs(&app, command, class).len(), 1, "{input:?}");
        match expect {
            Expect::Refused => {
                assert!(
                    session(&app).controls.pending_replace.is_none(),
                    "{input:?}"
                );
            }
            Expect::Asks => {
                assert!(
                    session(&app).controls.pending_replace.is_some(),
                    "{input:?}"
                );
                app.apply_controls_action(ControlsAction::Replace);
                let stack = &session(&app).modal_stack;
                assert_eq!(stack.active_name(), Some(CONTROLS_PANEL_NAME), "{input:?}");
                assert_eq!(stack.len(), 1, "{input:?}: no menu opened");
                assert_eq!(inputs(&app, command, class)[1], input);
            }
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
    // East is cancel's only gamepad binding; confirm on East would leave
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
    assert_eq!(
        inputs(&app, Command::NavCancel, PAD),
        [pad(Button::LeftThumb)]
    );
    assert_eq!(
        session(&app)
            .bindings
            .ui_nav()
            .command_for(pad(Button::South), crate::input::UiNavContext::Capture),
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
    // A prompt closes unchanged when its command leaves the list: here dash
    // becomes irrelevant while its prompt is open.
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
        session.modal_stack.push(
            CONTROLS_PANEL_NAME,
            build_controls_panel(&shell(), &rows, None),
        );
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
        &shell(),
        &controls_rows(
            session(&app).bindings.table(),
            session(&app).bindings.author(),
        ),
        None,
    ));
    assert!(!panel.contains("ctl_dash_"));
    let pushed = stack.retained_descriptors().last().unwrap();
    let next = Command::ALL
        .into_iter()
        .skip_while(|command| *command != Command::Dash)
        .skip(1)
        .find(|command| session(&app).bindings.table().relevance(*command) == Relevance::Relevant)
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

fn capture(app: &mut App, input: PhysicalInput) {
    app.session.as_mut().unwrap().offer_capture_press(input);
    app.update_controls_panel();
}

#[test]
fn reset_that_would_leave_cancel_unbound_is_refused_and_changes_nothing() {
    // Cancel gives East to confirm and keeps North; resetting cancel would
    // bring back its East default, which confirm's player binding holds.
    let mut app = test_app();
    open_capture(&mut app, Command::NavCancel, PAD, 1);
    capture(&mut app, pad(Button::North));
    if session(&app).modal_stack.active_name() == Some(CONTROLS_DIALOG_NAME) {
        app.apply_controls_action(ControlsAction::Replace);
    }
    assert_eq!(
        inputs(&app, Command::NavCancel, PAD),
        [pad(Button::East), pad(Button::North)]
    );
    open_capture(&mut app, Command::NavConfirm, PAD, 1);
    capture(&mut app, pad(Button::East));
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_DIALOG_NAME),
        "East is cancel's, so the player is asked"
    );
    app.apply_controls_action(ControlsAction::Replace);
    assert_eq!(inputs(&app, Command::NavCancel, PAD), [pad(Button::North)]);
    let before = session(&app).bindings.player().clone();

    app.apply_controls_action(ControlsAction::Reset {
        command: Command::NavCancel.id(),
    });
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_DIALOG_NAME),
        "the refusal explains itself"
    );
    assert_eq!(session(&app).bindings.player(), &before);
    assert_eq!(inputs(&app, Command::NavCancel, PAD), [pad(Button::North)]);
    assert_eq!(
        inputs(&app, Command::NavConfirm, PAD),
        [pad(Button::South), pad(Button::East)]
    );
}

#[test]
fn a_key_closes_a_gamepad_prompt_without_binding() {
    let mut app = test_app();
    open_capture(&mut app, Command::Jump, PAD, 1);
    let prompt = descriptor_text(
        session(&app)
            .modal_stack
            .retained_descriptors()
            .last()
            .unwrap(),
    );
    assert!(prompt.contains("PRESS ANY KEY TO CANCEL"), "{prompt}");
    capture(&mut app, key(KeyCode::KeyJ));
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_PANEL_NAME)
    );
    assert!(session(&app).bindings.player().rows.is_empty());
}

#[test]
fn a_gamepad_prompt_closes_when_the_pad_goes_away() {
    let mut app = test_app();
    open_capture(&mut app, Command::Jump, PAD, 1);
    let session_mut = app.session.as_mut().unwrap();
    session_mut.track_capture_pad(true);
    assert!(session_mut.capture_prompt_is_active(), "a pad is connected");
    session_mut.track_capture_pad(false);
    assert!(!session_mut.capture_prompt_is_active());
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_PANEL_NAME)
    );
    assert!(session(&app).bindings.player().rows.is_empty());

    // A keyboard prompt ignores the pad.
    open_capture(&mut app, Command::Jump, KBM, 1);
    let session_mut = app.session.as_mut().unwrap();
    session_mut.track_capture_pad(true);
    session_mut.track_capture_pad(false);
    assert!(session_mut.capture_prompt_is_active());
}

#[test]
fn an_input_with_no_stored_name_never_binds_and_the_prompt_keeps_waiting() {
    let mut app = test_app();
    open_capture(&mut app, Command::NavCancel, KBM, 1);
    capture(
        &mut app,
        PhysicalInput::MouseButton(winit::event::MouseButton::Other(9)),
    );
    assert!(session(&app).capture_prompt_is_active());
    assert!(session(&app).bindings.player().rows.is_empty());
}

#[test]
fn a_capture_for_a_slot_the_panel_does_not_show_opens_no_prompt() {
    let mut app = test_app();
    open_capture(&mut app, Command::Jump, KBM, usize::MAX);
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_PANEL_NAME)
    );
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
    assert_eq!(
        stack.len(),
        1,
        "the panel is updated in place, not pushed again"
    );
    let panel = descriptor_text(stack.retained_descriptors().last().unwrap());
    assert!(panel.contains("\"label\":\"V\""), "{panel}");
}

#[test]
fn a_mod_tree_under_the_reserved_name_never_replaces_the_engine_panel() {
    // The registry rejects the name at mod scope, and ui.openControls
    // still opens the engine panel.
    use log::Level;
    use postretro_scripting_core::data_descriptors::RegisteredUiTree;
    use postretro_test_log_capture::LogCapture;
    use postretro_ui::modal_stack::ScopeTier;

    let capture = LogCapture::start();
    let mut app = test_app();
    let impostor = build_refusal_dialog("IMPOSTOR", KBM);
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .register_script_trees(
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

#[test]
fn the_shipped_shell_frames_the_rows_the_engine_fills() {
    let author = AuthorLayer::default();
    let table = EffectiveTable::build(&author, &PlayerLayer::default(), all_facts(), false);
    let rows = controls_rows(&table, &author);
    let panel = build_controls_panel(&shell(), &rows, None);
    let text = descriptor_text(&panel);
    assert!(text.contains("\"id\":\"controlsRows\""));
    assert!(text.contains("\"scroll\":{\"maxHeight\":420.0}"), "{text}");
    assert!(text.contains("\"cols\":3"), "{text}");
    assert!(text.contains("ctl_resetAll") && text.contains("ctl_back"));
    assert!(
        !text.contains("controlsDisplacedNote"),
        "the note shows only when a row is flagged"
    );
    assert_eq!(
        panel.initial_focus.as_deref(),
        Some(slot_id(rows[0].command, KBM, 0).as_str())
    );
    assert!(
        descriptor_text(&shell()).contains("\"children\":[]"),
        "the shell ships an empty row grid"
    );
}

// --- layout --------------------------------------------------------------

/// The reference canvas at 1:1, so device pixels are logical pixels.
const CANVAS: [u32; 2] = [1280, 720];
/// The least clear canvas every panel leaves on each side.
const CANVAS_MARGIN: f32 = 40.0;

/// The rows of the default table with the first made the worst case the
/// engine's own vocabulary produces: the longest command label, flagged, with
/// the longest input name and activator in every slot.
fn worst_case_rows(app: &App) -> Vec<ControlsRow> {
    let bindings = &session(app).bindings;
    let mut rows = controls_rows(bindings.table(), bindings.author());
    let worst = &mut rows[0];
    worst.label = command_label(&AuthorLayer::default(), Command::CycleWieldablePrevious);
    worst.displaced = [true; 2];
    worst.inputs = [
        vec![(key(KeyCode::NumpadMultiply), ActivatorKind::Release); MIN_SLOTS],
        vec![(pad(Button::RightThumb), ActivatorKind::Release); MIN_SLOTS],
    ];
    rows
}

/// Lay `tree` out on the reference canvas: its backdrop rect and focus stops.
fn lay_out(tree: &AnchoredTree) -> ([f32; 4], postretro_ui::tree::FocusRectList) {
    use postretro_ui::tree::{CellValues, ImageSizes, UiTree};
    let mut ui = UiTree::from_descriptor(tree, &postretro_ui::theme::UiTheme::engine_default());
    let mut fonts = postretro_ui::text::build_font_system();
    let drawn = ui.build_draw_data(
        CANVAS,
        &mut fonts,
        &ImageSizes::new(),
        &std::collections::HashMap::new(),
    );
    let backdrop = drawn
        .quads
        .instances
        .iter()
        .map(|quad| quad.rect)
        .max_by(|a, b| (a[2] * a[3]).total_cmp(&(b[2] * b[3])))
        .expect("the panel draws its backdrop");
    let stops = ui.export_focus_rects(
        tree,
        CANVAS,
        &std::collections::HashMap::new(),
        &CellValues::new(),
    );
    (backdrop, stops)
}

/// `tree` lies inside the canvas with margin, and no focus stop pokes out of
/// its panel sideways. Returns the panel rect.
fn assert_fits_the_canvas(name: &str, tree: &AnchoredTree) -> [f32; 4] {
    let (panel, stops) = lay_out(tree);
    let [x, y, w, h] = panel;
    let [width, height] = CANVAS.map(|v| v as f32);
    assert!(
        x >= CANVAS_MARGIN
            && y >= CANVAS_MARGIN
            && x + w <= width - CANVAS_MARGIN
            && y + h <= height - CANVAS_MARGIN,
        "{name}: panel {panel:?} leaves less than {CANVAS_MARGIN} px of the {width}x{height} canvas"
    );
    for stop in &stops.rects {
        let [sx, _, sw, _] = stop.rect;
        assert!(
            sx >= x && sx + sw <= x + w,
            "{name}: {} at {:?} is clipped by the panel {panel:?}",
            stop.id,
            stop.rect
        );
    }
    panel
}

#[test]
fn the_controls_panel_fits_the_reference_canvas() {
    let app = test_app();
    let rows = worst_case_rows(&app);
    let panel = build_controls_panel(&shell(), &rows, None);
    assert_fits_the_canvas("controls panel", &panel);
    let text = descriptor_text(&panel);
    assert!(text.contains("\"RIGHT STICK PRESS (RELEASE)\""), "{text}");
    assert!(
        text.contains("\"CYCLE WIELDABLE PREVIOUS ! \u{b7} PAD 2\""),
        "{text}"
    );
}

#[test]
fn the_capture_prompt_and_dialogs_fit_the_reference_canvas() {
    let label = command_label(&AuthorLayer::default(), Command::CycleWieldablePrevious);
    let prompt = build_capture_prompt(
        &label,
        CaptureTarget {
            command: Command::CycleWieldablePrevious,
            class: KBM,
            slot: 1,
        },
        Some(key(KeyCode::NumpadMultiply)),
    );
    assert_fits_the_canvas("capture prompt", &prompt);
    let holders = [
        label.clone(),
        command_label(&AuthorLayer::default(), Command::ToggleLastWieldable),
    ];
    assert_fits_the_canvas(
        "conflict dialog",
        &build_conflict_dialog(pad(Button::RightThumb), &holders),
    );
    assert_fits_the_canvas("refusal dialog", &build_refusal_dialog(&label, KBM));
}

#[test]
fn spatial_nav_reaches_every_slot_and_every_reset() {
    use crate::input::{InputMode, NavIntent, UiFocusEngine};
    let app = test_app();
    let bindings = &session(&app).bindings;
    let rows = controls_rows(bindings.table(), bindings.author());
    let panel = build_controls_panel(&shell(), &rows, None);
    let (_, stops) = lay_out(&panel);
    // Where one nav step from `from` lands: a fresh engine opened on `from`.
    let step = |from: &str, nav: NavIntent| -> Option<String> {
        let mut list = stops.clone();
        list.initial_focus = Some(from.to_string());
        let mut engine = UiFocusEngine::new();
        let mut tick = |intents: &[NavIntent]| {
            engine.tick(
                Some("controls"),
                Some(&list),
                intents,
                None,
                &[],
                InputMode::Focus,
                0.0,
            )
        };
        tick(&[]);
        tick(&[nav]).focused
    };
    let start = panel.initial_focus.clone().unwrap();
    let mut reached = vec![start.clone()];
    let mut frontier = vec![start];
    while let Some(from) = frontier.pop() {
        for nav in [
            NavIntent::Up,
            NavIntent::Down,
            NavIntent::Left,
            NavIntent::Right,
        ] {
            if let Some(to) = step(&from, nav)
                && !reached.contains(&to)
            {
                reached.push(to.clone());
                frontier.push(to);
            }
        }
    }
    for row in &rows {
        let mut ids = vec![format!("ctl_{}_reset", row.command.id())];
        for (class, slot) in [(KBM, 0), (KBM, 1), (PAD, 0), (PAD, 1)] {
            ids.push(slot_id(row.command, class, slot));
        }
        for id in ids {
            assert!(reached.contains(&id), "nav never reaches {id}");
        }
    }
    for id in ["ctl_resetAll", "ctl_back"] {
        assert!(reached.contains(&id.to_string()), "nav never reaches {id}");
    }
}
