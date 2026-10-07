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
        KBM,
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
    let panel = descriptor_text(&build_controls_panel(&shell(), &rows, KBM, None));
    assert!(panel.contains("\"RELOAD !\""), "{panel}");
    assert!(panel.contains("ANOTHER BINDING TOOK AN INPUT FROM THIS COMMAND"));
    let pad_panel = descriptor_text(&build_controls_panel(&shell(), &rows, PAD, None));
    assert!(
        pad_panel.contains("\"RELOAD\"") && !pad_panel.contains("\"RELOAD !\""),
        "the gamepad bindings lost nothing, so their row carries no mark: {pad_panel}"
    );
    assert!(
        !pad_panel.contains("controlsDisplacedNote"),
        "the note shows only for a flagged row on the class shown"
    );

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

/// The filled row grid of `panel`: its column count and its cells by row.
fn grid_rows(panel: &AnchoredTree) -> (usize, Vec<Vec<Value>>) {
    let mut value = serde_json::to_value(panel).unwrap();
    let grid = find_widget(&mut value["root"], ROWS_GRID_ID).unwrap();
    let cols = grid["cols"].as_u64().unwrap() as usize;
    let cells = grid["children"].as_array().unwrap();
    assert_eq!(cells.len() % cols, 0, "every grid row fills its columns");
    (cols, cells.chunks(cols).map(<[Value]>::to_vec).collect())
}

/// The grid row whose second cell is one of `command`'s stops.
fn command_row(rows: &[Vec<Value>], command: Command) -> Vec<Value> {
    let prefix = format!("ctl_{}_", command.id());
    let found: Vec<&Vec<Value>> = rows
        .iter()
        .filter(|row| {
            row[1]["id"]
                .as_str()
                .is_some_and(|id| id.starts_with(&prefix))
        })
        .collect();
    assert_eq!(found.len(), 1, "one row per command");
    found[0].clone()
}

#[test]
fn each_action_is_one_row_with_two_binds_and_a_reset() {
    let author = AuthorLayer::default();
    let table = EffectiveTable::build(&author, &PlayerLayer::default(), all_facts(), false);
    let rows = controls_rows(&table, &author);
    for class in [KBM, PAD] {
        let (cols, grid) = grid_rows(&build_controls_panel(&shell(), &rows, class, None));
        assert_eq!(cols, 4, "ACTION | BIND 1 | BIND 2 | RESET");
        assert_eq!(
            grid[0][0]["content"],
            json!("GAMEPLAY"),
            "a category heading leads, with no column-header row"
        );
        let forward = command_row(&grid, Command::MoveForward);
        assert_eq!(forward[0]["content"], json!("MOVE FORWARD"));
        let ids: Vec<&str> = forward[1..]
            .iter()
            .map(|cell| cell["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            [
                slot_id(Command::MoveForward, class, 0),
                slot_id(Command::MoveForward, class, 1),
                format!("ctl_{}_reset", Command::MoveForward.id()),
            ],
            "{class:?}: the two binds shown are the class the player used last"
        );
        let other = if class == KBM { PAD } else { KBM };
        let text = descriptor_text(&build_controls_panel(&shell(), &rows, class, None));
        assert!(
            !text.contains(&format!("_{}_", other.settings_key())),
            "{class:?}: no slot of the other class shows"
        );
    }
}

#[test]
fn a_class_bound_past_two_slots_shows_a_column_for_every_binding() {
    // An author may give a command more defaults than two: every one stays in
    // reach, so the class shows as many slot columns as its longest row.
    let mut author = AuthorLayer::default();
    author.defaults.insert(
        (Command::Jump, KBM),
        [KeyCode::Space, KeyCode::KeyJ, KeyCode::KeyK]
            .map(|code| author_binding(key(code), ActivatorKind::Press))
            .to_vec(),
    );
    let table = EffectiveTable::build(&author, &PlayerLayer::default(), all_facts(), false);
    let rows = controls_rows(&table, &author);
    let (cols, grid) = grid_rows(&build_controls_panel(&shell(), &rows, KBM, None));
    assert_eq!(cols, 5);
    let jump = command_row(&grid, Command::Jump);
    assert_eq!(jump[3]["label"], json!("K"));
    let forward = command_row(&grid, Command::MoveForward);
    assert_eq!(
        forward[3]["label"],
        json!("---"),
        "a shorter row shows an empty slot"
    );
    let (cols, _) = grid_rows(&build_controls_panel(&shell(), &rows, PAD, None));
    assert_eq!(cols, 4, "the gamepad view keeps its two");
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
        changed_rows(&old, &new, &StoredRowText::new()),
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

// Regression: a replace that took V from a dormant dash row saved `[None]` as
// `[]`, so the reloaded dash was unbound where the session showed its default,
// and the unreadable name was lost.
#[test]
fn changed_rows_write_an_unreadable_slot_back_as_the_text_it_was_loaded_from() {
    let stored_text = |inputs: &[Option<&str>]| -> Vec<Option<String>> {
        inputs.iter().map(|i| i.map(str::to_string)).collect()
    };
    let saved = [
        (
            (Command::Dash, KBM),
            stored_text(&[Some("KeyFromANewerBuild"), Some("KeyV"), None]),
        ),
        ((Command::Jump, KBM), stored_text(&[Some("KeyV")])),
    ];
    let stored: StoredRowText = saved.iter().cloned().collect();
    let rows: Vec<(&str, &str, &[Option<String>])> = saved
        .iter()
        .map(|((command, class), inputs)| (class.settings_key(), command.id(), inputs.as_slice()))
        .collect();
    let old = crate::input::player_layer_from_rows(rows);
    assert_eq!(
        old.rows[&(Command::Dash, KBM)],
        [None, Some(key(KeyCode::KeyV)), None]
    );
    // A replace takes V from the dash row, as it does from a dormant one.
    let mut new = old.clone();
    new.rows.insert((Command::Dash, KBM), vec![None, None]);
    assert_eq!(
        changed_rows(&old, &new, &stored),
        [(
            KBM,
            Command::Dash,
            Some(vec!["KeyFromANewerBuild".to_string(), String::new()])
        )]
    );
    // What reloads is what the session holds.
    let written = ["KeyFromANewerBuild".to_string(), String::new()].map(Some);
    let reloaded = crate::input::player_layer_from_rows([(
        KBM.settings_key(),
        Command::Dash.id(),
        written.as_slice(),
    )]);
    assert_eq!(
        reloaded.rows[&(Command::Dash, KBM)],
        new.rows[&(Command::Dash, KBM)]
    );
}

/// One saved row as `stored` text, and the player layer it loads as.
fn load_row(
    command: Command,
    class: DeviceClass,
    inputs: &[Option<&str>],
) -> (StoredRowText, PlayerLayer) {
    let text: Vec<Option<String>> = inputs.iter().map(|i| i.map(str::to_string)).collect();
    let layer = crate::input::player_layer_from_rows([(
        class.settings_key(),
        command.id(),
        text.as_slice(),
    )]);
    let stored: StoredRowText = [((command, class), text)].into_iter().collect();
    (stored, layer)
}

/// What capturing `input` into `command`'s `slot` on `class` saves.
fn saved_after_capture(
    stored: &StoredRowText,
    old: &PlayerLayer,
    command: Command,
    class: DeviceClass,
    slot: usize,
    input: PhysicalInput,
) -> Vec<(DeviceClass, Command, Option<Vec<String>>)> {
    let author = AuthorLayer::default();
    let table = EffectiveTable::build(&author, old, all_facts(), false);
    let proposal = propose_rebind(
        &table,
        &author,
        old,
        all_facts(),
        false,
        command,
        class,
        slot,
        input,
    );
    let RebindProposal::Clean { player } = proposal else {
        panic!("expected a clean capture, got {proposal:?}");
    };
    changed_rows(old, &player, stored)
}

// Regression: capturing K into jump's second slot over a saved
// `["<unreadable>", "KeyJ"]` saved `["Space", "KeyK"]`, the default the
// unreadable slot showed.
#[test]
fn a_capture_into_another_slot_writes_an_unreadable_slot_back_unchanged() {
    let (stored, old) = load_row(
        Command::Jump,
        KBM,
        &[Some("KeyFromANewerBuild"), Some("KeyJ")],
    );
    assert_eq!(
        saved_after_capture(&stored, &old, Command::Jump, KBM, 1, key(KeyCode::KeyK)),
        [(
            KBM,
            Command::Jump,
            Some(vec!["KeyFromANewerBuild".to_string(), "KeyK".to_string()])
        )]
    );
}

#[test]
fn a_capture_over_one_unreadable_slot_writes_the_next_back_in_its_place() {
    // Slot 2 has no default to fall back to, so the panel shows only slots 0
    // and 1; capturing into slot 0 replaces that slot's string alone.
    let (stored, old) = load_row(
        Command::Jump,
        KBM,
        &[
            Some("KeyFromANewerBuild"),
            Some("KeyJ"),
            Some("KeyFromANewestBuild"),
        ],
    );
    assert_eq!(
        saved_after_capture(&stored, &old, Command::Jump, KBM, 0, key(KeyCode::KeyK)),
        [(
            KBM,
            Command::Jump,
            Some(vec![
                "KeyK".to_string(),
                "KeyJ".to_string(),
                "KeyFromANewestBuild".to_string()
            ])
        )]
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
        session.controls.built = Some((session.bindings.generation(), KBM));
        session.modal_stack.push(
            CONTROLS_PANEL_NAME,
            build_controls_panel(&shell(), &rows, KBM, None),
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
        KBM,
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
fn a_prompt_opens_with_no_partial_wheel_travel() {
    use winit::event::MouseScrollDelta;
    let half_notch = MouseScrollDelta::LineDelta(0.0, 0.5);
    let mut app = test_app();
    let session_mut = app.session.as_mut().unwrap();
    assert_eq!(
        session_mut.input_system.capture_wheel_notch(half_notch),
        None
    );
    open_capture(&mut app, Command::Jump, KBM, 1);
    let session_mut = app.session.as_mut().unwrap();
    assert_eq!(
        session_mut.input_system.capture_wheel_notch(half_notch),
        None,
        "travel from before the prompt does not complete a notch"
    );
    assert_eq!(
        session_mut.input_system.capture_wheel_notch(half_notch),
        Some(PhysicalInput::MouseWheelUp)
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
    session_mut.track_capture_pad(|_| true);
    assert!(session_mut.capture_prompt_is_active(), "a pad is connected");
    session_mut.track_capture_pad(|_| false);
    assert!(!session_mut.capture_prompt_is_active());
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_PANEL_NAME)
    );
    assert!(session(&app).bindings.player().rows.is_empty());

    // A keyboard prompt ignores the pad.
    open_capture(&mut app, Command::Jump, KBM, 1);
    let session_mut = app.session.as_mut().unwrap();
    session_mut.track_capture_pad(|_| true);
    session_mut.track_capture_pad(|_| false);
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
    let panel = build_controls_panel(&shell(), &rows, KBM, None);
    let text = descriptor_text(&panel);
    assert!(text.contains("\"id\":\"controlsRows\""));
    assert!(text.contains("\"scroll\":{\"maxHeight\":420.0}"), "{text}");
    assert!(text.contains("\"cols\":4"), "{text}");
    assert!(
        text.contains("\"KEYBOARD AND MOUSE \u{b7} USE A GAMEPAD TO SEE ITS BINDINGS\""),
        "the caption names the class shown: {text}"
    );
    assert!(
        descriptor_text(&build_controls_panel(&shell(), &rows, PAD, None))
            .contains("\"GAMEPAD \u{b7} USE THE KEYBOARD OR MOUSE TO SEE THEIR BINDINGS\"")
    );
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
        descriptor_text(&shell()).contains("\"cols\":4,"),
        "the shell ships the four columns the default slots fill"
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
    for (class, input) in [
        (KBM, "NUMPAD MULTIPLY (RELEASE)"),
        (PAD, "RIGHT STICK PRESS (RELEASE)"),
    ] {
        let panel = build_controls_panel(&shell(), &rows, class, None);
        assert_fits_the_canvas(&format!("controls panel {class:?}"), &panel);
        let text = descriptor_text(&panel);
        assert!(text.contains(&format!("\"{input}\"")), "{text}");
        assert!(text.contains("\"CYCLE WIELDABLE PREVIOUS !\""), "{text}");
        assert!(text.contains("controlsDisplacedNote"), "{text}");
    }
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
    for class in [KBM, PAD] {
        spatial_nav_reaches_every_stop(class);
    }
}

fn spatial_nav_reaches_every_stop(class: DeviceClass) {
    use crate::input::{InputMode, NavIntent, UiFocusEngine};
    let app = test_app();
    let bindings = &session(&app).bindings;
    let rows = controls_rows(bindings.table(), bindings.author());
    let panel = build_controls_panel(&shell(), &rows, class, None);
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
        for slot in 0..MIN_SLOTS {
            ids.push(slot_id(row.command, class, slot));
        }
        for id in ids {
            assert!(reached.contains(&id), "{class:?}: nav never reaches {id}");
        }
    }
    for id in ["ctl_resetAll", "ctl_back"] {
        assert!(reached.contains(&id.to_string()), "nav never reaches {id}");
    }
}

// --- device class --------------------------------------------------------

/// The pushed panel's descriptor.
fn pushed_panel(app: &App) -> AnchoredTree {
    let session = session(app);
    assert!(session.modal_stack.contains_pushed(CONTROLS_PANEL_NAME));
    session
        .modal_stack
        .retained_descriptors()
        .next()
        .expect("the panel is the bottom tree in these tests")
        .clone()
}

/// The focus engine's key for the pushed panel.
fn panel_key(app: &App) -> String {
    crate::session::modal_focus_key(
        CONTROLS_PANEL_NAME,
        session(app)
            .controls
            .instance
            .expect("the panel was pushed"),
    )
}

/// One focus tick on the pushed panel's laid-out stops, as a frame with the
/// panel on top runs it; `land_on` stands in for the stop focus moved to.
/// Returns the focused stop.
fn tick_panel_focus(app: &mut App, land_on: Option<&str>) -> Option<String> {
    use crate::input::InputMode;
    let (_, mut stops) = lay_out(&pushed_panel(app));
    if let Some(id) = land_on {
        stops.initial_focus = Some(id.to_string());
    }
    let key = panel_key(app);
    let session = app.session.as_mut().unwrap();
    session
        .ui_focus
        .tick(
            Some(&key),
            Some(&stops),
            &[],
            None,
            &[],
            InputMode::Focus,
            0.0,
        )
        .focused
}

fn use_pad(app: &mut App) {
    let family = &mut app.session.as_mut().unwrap().device_family;
    family.note_pad(None);
    family.end_frame();
}

fn use_keyboard(app: &mut App) {
    let family = &mut app.session.as_mut().unwrap().device_family;
    family.note_keyboard_mouse();
    family.end_frame();
}

#[test]
fn switching_device_family_swaps_the_shown_class_and_keeps_focus_on_the_same_slot() {
    let mut app = test_app();
    app.open_controls_panel();
    let jump_key_2 = slot_id(Command::Jump, KBM, 1);
    assert!(descriptor_text(&pushed_panel(&app)).contains(&jump_key_2));
    assert_eq!(
        tick_panel_focus(&mut app, Some(&jump_key_2)).as_deref(),
        Some(jump_key_2.as_str())
    );

    // A frame with no family change rebuilds nothing.
    let before = descriptor_text(&pushed_panel(&app));
    app.update_controls_panel();
    assert_eq!(descriptor_text(&pushed_panel(&app)), before);

    use_pad(&mut app);
    app.update_controls_panel();
    let jump_pad_2 = slot_id(Command::Jump, PAD, 1);
    let panel = pushed_panel(&app);
    let text = descriptor_text(&panel);
    assert!(text.contains(&jump_pad_2), "{text}");
    assert!(!text.contains(&jump_key_2), "{text}");
    assert!(text.contains("\"GAMEPAD \u{b7} "), "{text}");
    assert_eq!(session(&app).modal_stack.len(), 1, "rebuilt in place");
    assert_eq!(
        tick_panel_focus(&mut app, None).as_deref(),
        Some(jump_pad_2.as_str()),
        "focus stays on the same command and slot"
    );

    use_keyboard(&mut app);
    app.update_controls_panel();
    assert_eq!(
        tick_panel_focus(&mut app, None).as_deref(),
        Some(jump_key_2.as_str())
    );

    // RESET keeps its id across the switch, so focus on it stays.
    let mut app = test_app();
    app.open_controls_panel();
    let jump_reset = format!("ctl_{}_reset", Command::Jump.id());
    tick_panel_focus(&mut app, Some(&jump_reset));
    use_pad(&mut app);
    app.update_controls_panel();
    assert_eq!(
        tick_panel_focus(&mut app, None).as_deref(),
        Some(jump_reset.as_str())
    );
}

#[test]
fn a_family_switch_under_an_open_prompt_keeps_the_prompt_and_its_class() {
    let mut app = test_app();
    use_pad(&mut app);
    app.open_controls_panel();
    let jump_pad_2 = slot_id(Command::Jump, PAD, 1);
    tick_panel_focus(&mut app, Some(&jump_pad_2));
    app.apply_controls_action(ControlsAction::Capture {
        command: Command::Jump.id(),
        class: PAD.settings_key(),
        slot: 1,
    });
    assert!(session(&app).capture_prompt_is_active());

    // The mouse moves while the gamepad prompt is open.
    use_keyboard(&mut app);
    app.update_controls_panel();
    assert!(
        session(&app).capture_prompt_is_active(),
        "the prompt stays open on its gamepad slot"
    );
    let panel = pushed_panel(&app);
    assert_eq!(
        panel.initial_focus.as_deref(),
        Some(slot_id(Command::Jump, KBM, 1).as_str()),
        "the covered panel shows the keyboard and returns to the same slot"
    );

    capture(&mut app, pad(Button::DPadDown));
    if session(&app).modal_stack.active_name() == Some(CONTROLS_DIALOG_NAME) {
        app.apply_controls_action(ControlsAction::Replace);
    }
    assert_eq!(inputs(&app, Command::Jump, PAD)[1], pad(Button::DPadDown));
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_PANEL_NAME)
    );
}

#[test]
fn a_family_switch_under_a_conflict_dialog_keeps_the_question() {
    let mut app = test_app();
    open_capture(&mut app, Command::Use, KBM, 0);
    capture(&mut app, key(KeyCode::Space));
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_DIALOG_NAME)
    );
    use_pad(&mut app);
    app.update_controls_panel();
    assert_eq!(
        session(&app).modal_stack.active_name(),
        Some(CONTROLS_DIALOG_NAME)
    );
    assert!(descriptor_text(&pushed_panel(&app)).contains(&slot_id(Command::Use, PAD, 0)));
    app.apply_controls_action(ControlsAction::Replace);
    assert_eq!(inputs(&app, Command::Use, KBM), [key(KeyCode::Space)]);
}
