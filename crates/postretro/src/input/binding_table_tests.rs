use gilrs::{Axis as GilrsAxis, Button as GilrsButton};
use winit::keyboard::KeyCode;

use super::binding_table::*;
use super::commands::{Command, CommandContext};
use super::input_names::DeviceClass;
use super::relevance::{Relevance, RelevanceFacts};
use super::types::{Action, Activator, ActivatorKind, AxisHalf, ButtonState, PhysicalInput};
use super::{GameplayInputLatch, InputSystem};

const KBM: DeviceClass = DeviceClass::KeyboardMouse;
const PAD: DeviceClass = DeviceClass::Gamepad;

fn key(code: KeyCode) -> PhysicalInput {
    PhysicalInput::Key(code)
}

fn pad(button: GilrsButton) -> PhysicalInput {
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

fn press(input: PhysicalInput) -> AuthorBinding {
    AuthorBinding {
        input,
        activator: Activator::PRESS,
    }
}

fn with(input: PhysicalInput, kind: ActivatorKind, threshold: f32) -> AuthorBinding {
    AuthorBinding {
        input,
        activator: Activator::with_threshold(kind, threshold),
    }
}

fn author(entries: &[(Command, DeviceClass, Vec<AuthorBinding>)]) -> AuthorLayer {
    AuthorLayer {
        defaults: entries
            .iter()
            .map(|(command, class, list)| ((*command, *class), list.clone()))
            .collect(),
        manifest_order: entries.iter().map(|(command, _, _)| *command).collect(),
        ..AuthorLayer::default()
    }
}

fn player(rows: &[(Command, DeviceClass, Vec<Option<PhysicalInput>>)]) -> PlayerLayer {
    PlayerLayer {
        rows: rows
            .iter()
            .map(|(command, class, row)| ((*command, *class), row.clone()))
            .collect(),
    }
}

fn build(author: &AuthorLayer, player: &PlayerLayer, facts: RelevanceFacts) -> EffectiveTable {
    EffectiveTable::build(author, player, facts, false)
}

fn commands_on(table: &EffectiveTable, input: PhysicalInput) -> Vec<Command> {
    table
        .entries()
        .iter()
        .filter(|e| e.input == input)
        .map(|e| e.command)
        .collect()
}

/// The command a key drives in gameplay, through a real input system.
fn press_and_read(table: &EffectiveTable, code: KeyCode, action: Action) -> ButtonState {
    let mut sys = InputSystem::new(table.gameplay_bindings());
    sys.handle_keyboard_event_at(code, true, 0.0);
    sys.handle_keyboard_event_at(code, false, 0.05);
    sys.snapshot_at(0.06).button(action)
}

// --- Engine defaults ---

#[test]
fn engine_defaults_raise_no_conflict_and_keep_the_guard() {
    let table = build(
        &AuthorLayer::default(),
        &PlayerLayer::default(),
        all_facts(),
    );
    assert!(
        table.conflicting_pairs().is_empty(),
        "{:?}",
        table.conflicting_pairs()
    );
    assert!(table.guard_violations().is_empty());
}

#[test]
fn escape_is_menu_with_no_tree_and_cancel_under_one_without_a_conflict() {
    let table = build(
        &AuthorLayer::default(),
        &PlayerLayer::default(),
        all_facts(),
    );
    let escape = commands_on(&table, key(KeyCode::Escape));
    assert!(escape.contains(&Command::NavMenu) && escape.contains(&Command::NavCancel));
}

#[test]
fn south_on_jump_and_confirm_is_not_a_conflict() {
    let table = build(
        &AuthorLayer::default(),
        &PlayerLayer::default(),
        all_facts(),
    );
    let south = commands_on(&table, pad(GilrsButton::South));
    assert!(south.contains(&Command::Jump) && south.contains(&Command::NavConfirm));
    assert!(table.conflicting_pairs().is_empty());
}

#[test]
fn fly_cam_stays_bound_hidden_and_out_of_conflicts() {
    // An author reload on Q leaves move_up on Q too, with no conflict.
    let layer = author(&[(Command::Reload, KBM, vec![press(key(KeyCode::KeyQ))])]);
    let table = build(&layer, &PlayerLayer::default(), all_facts());
    let q = commands_on(&table, key(KeyCode::KeyQ));
    assert!(q.contains(&Command::MoveUp) && q.contains(&Command::Reload));
    assert_eq!(table.relevance(Command::MoveUp), Relevance::DevOnly);
    assert!(commands_on(&table, key(KeyCode::KeyZ)).contains(&Command::MoveDown));
    assert!(table.conflicting_pairs().is_empty());

    let mut sys = InputSystem::new(table.gameplay_bindings());
    sys.handle_keyboard_event(KeyCode::KeyQ, true);
    assert!(sys.snapshot().axis_value(Action::MoveUp) > 0.0);
}

// --- Author layer ---

#[test]
fn author_shift_dash_with_empty_sprint_keyboard_drives_dash_only() {
    let layer = author(&[
        (
            Command::Dash,
            KBM,
            vec![with(key(KeyCode::ShiftLeft), ActivatorKind::Tap, 0.2)],
        ),
        (Command::Sprint, KBM, vec![]),
    ]);
    let table = build(&layer, &PlayerLayer::default(), all_facts());
    assert_eq!(
        press_and_read(&table, KeyCode::ShiftLeft, Action::Dash),
        ButtonState::Pressed
    );
    assert_eq!(
        press_and_read(&table, KeyCode::KeyF, Action::Dash),
        ButtonState::Inactive
    );
    let mut sys = InputSystem::new(table.gameplay_bindings());
    sys.handle_keyboard_event_at(KeyCode::ShiftLeft, true, 0.0);
    assert!(!sys.snapshot_at(1.0).button(Action::Sprint).is_active());
}

#[test]
fn an_absent_device_class_keeps_engine_defaults_and_an_empty_list_unbinds() {
    let layer = author(&[(Command::Dash, KBM, vec![press(key(KeyCode::KeyV))])]);
    let table = build(&layer, &PlayerLayer::default(), all_facts());
    assert_eq!(
        table.inputs(Command::Dash, PAD),
        vec![pad(GilrsButton::East)]
    );
    let layer = author(&[
        (Command::Dash, KBM, vec![press(key(KeyCode::KeyV))]),
        (Command::Dash, PAD, vec![]),
    ]);
    let table = build(&layer, &PlayerLayer::default(), all_facts());
    assert!(table.inputs(Command::Dash, PAD).is_empty());
}

#[test]
fn an_author_binding_displaces_a_colliding_engine_default() {
    // Reload on F: dash's engine default F gives way.
    let layer = author(&[(Command::Reload, KBM, vec![press(key(KeyCode::KeyF))])]);
    let table = build(&layer, &PlayerLayer::default(), all_facts());
    assert_eq!(
        commands_on(&table, key(KeyCode::KeyF)),
        vec![Command::Reload]
    );
    assert!(
        table.displaced().is_empty(),
        "only a player displaces visibly"
    );
}

// --- Relevance ---

#[test]
fn an_irrelevant_dash_is_unbound_and_never_conflicts_with_an_author_f() {
    let layer = author(&[(Command::Reload, KBM, vec![press(key(KeyCode::KeyF))])]);
    let no_dash = RelevanceFacts {
        dash: false,
        ..all_facts()
    };
    let table = build(&layer, &PlayerLayer::default(), no_dash);
    assert_eq!(table.relevance(Command::Dash), Relevance::Irrelevant);
    assert!(table.inputs(Command::Dash, KBM).is_empty());
    assert!(table.inputs(Command::Dash, PAD).is_empty());
    assert_eq!(
        commands_on(&table, key(KeyCode::KeyF)),
        vec![Command::Reload]
    );
}

#[test]
fn force_show_binds_an_underived_command() {
    let mut layer = AuthorLayer::default();
    layer.show.insert(Command::Dash, true);
    let table = build(&layer, &PlayerLayer::default(), RelevanceFacts::default());
    assert_eq!(table.inputs(Command::Dash, KBM), vec![key(KeyCode::KeyF)]);
}

// --- Player layer ---

#[test]
fn a_command_never_rebound_follows_the_author_default() {
    let rows = player(&[(Command::Jump, KBM, vec![Some(key(KeyCode::KeyJ))])]);
    let layer = author(&[(Command::Dash, KBM, vec![press(key(KeyCode::KeyV))])]);
    let table = build(&layer, &rows, all_facts());
    assert_eq!(table.inputs(Command::Dash, KBM), vec![key(KeyCode::KeyV)]);
    assert_eq!(table.inputs(Command::Jump, KBM), vec![key(KeyCode::KeyJ)]);
}

#[test]
fn an_unreadable_slot_falls_back_to_its_author_default_alone() {
    let layer = author(&[(
        Command::Dash,
        KBM,
        vec![
            with(key(KeyCode::KeyV), ActivatorKind::Tap, 0.2),
            with(key(KeyCode::KeyB), ActivatorKind::Hold, 0.4),
        ],
    )]);
    let rows = player(&[(Command::Dash, KBM, vec![None, Some(key(KeyCode::KeyN))])]);
    let table = build(&layer, &rows, all_facts());
    let dash: Vec<_> = table
        .entries()
        .iter()
        .filter(|e| e.command == Command::Dash && e.class == KBM)
        .collect();
    assert_eq!(dash.len(), 2);
    assert_eq!(dash[0].input, key(KeyCode::KeyV));
    assert_eq!(dash[0].origin, BindingOrigin::Author);
    assert_eq!(dash[1].input, key(KeyCode::KeyN));
    assert_eq!(
        dash[1].activator.kind,
        ActivatorKind::Hold,
        "slot 2 keeps its activator"
    );
}

#[test]
fn a_rebound_key_keeps_its_slot_activator_and_an_extra_slot_takes_press() {
    let layer = author(&[(
        Command::Dash,
        KBM,
        vec![with(key(KeyCode::ShiftLeft), ActivatorKind::Tap, 0.15)],
    )]);
    let rows = player(&[(
        Command::Dash,
        KBM,
        vec![Some(key(KeyCode::KeyV)), Some(key(KeyCode::KeyB))],
    )]);
    let table = build(&layer, &rows, all_facts());
    let dash: Vec<_> = table
        .entries()
        .iter()
        .filter(|e| e.command == Command::Dash && e.class == KBM)
        .collect();
    assert_eq!(
        dash[0].activator,
        Activator::with_threshold(ActivatorKind::Tap, 0.15)
    );
    assert_eq!(dash[1].activator, Activator::PRESS);
}

#[test]
fn a_player_key_beats_a_later_author_default_on_it_and_flags_the_author_command() {
    let rows = player(&[(Command::Dash, KBM, vec![Some(key(KeyCode::KeyQ))])]);
    let layer = author(&[(Command::Reload, KBM, vec![press(key(KeyCode::KeyQ))])]);
    let table = build(&layer, &rows, all_facts());
    let q: Vec<_> = commands_on(&table, key(KeyCode::KeyQ))
        .into_iter()
        .filter(|c| *c != Command::MoveUp && c.context() == CommandContext::Gameplay)
        .collect();
    assert_eq!(q, vec![Command::Dash]);
    assert!(table.displaced().contains(&(Command::Reload, KBM)));
    assert_eq!(
        press_and_read(&table, KeyCode::KeyQ, Action::Reload),
        ButtonState::Inactive
    );

    // Without the player row, Q drives reload.
    let table = build(&layer, &PlayerLayer::default(), all_facts());
    assert_eq!(
        press_and_read(&table, KeyCode::KeyQ, Action::Reload),
        ButtonState::Pressed
    );
}

#[test]
fn a_later_author_hold_on_a_player_press_is_displaced_so_the_press_stays_immediate() {
    let rows = player(&[(Command::Use, KBM, vec![Some(key(KeyCode::KeyQ))])]);
    let layer = author(&[(
        Command::Reload,
        KBM,
        vec![with(key(KeyCode::KeyQ), ActivatorKind::Hold, 0.3)],
    )]);
    let table = build(&layer, &rows, all_facts());
    assert!(table.displaced().contains(&(Command::Reload, KBM)));
    let mut sys = InputSystem::new(table.gameplay_bindings());
    sys.handle_keyboard_event_at(KeyCode::KeyQ, true, 0.0);
    assert_eq!(
        sys.snapshot_at(0.01).button(Action::Use),
        ButtonState::Pressed
    );
}

// --- Conflicts ---

fn entry(command: Command, input: PhysicalInput, kind: ActivatorKind) -> EffectiveBinding {
    EffectiveBinding {
        command,
        class: DeviceClass::of(input),
        slot: 0,
        input,
        activator: Activator::new(kind),
        origin: BindingOrigin::Author,
    }
}

#[test]
fn conflict_rules_follow_the_steam_pairing() {
    use ActivatorKind::*;
    let f = key(KeyCode::KeyF);
    assert!(conflicts(
        &entry(Command::Dash, f, Press),
        &entry(Command::Jump, f, Press)
    ));
    assert!(conflicts(
        &entry(Command::Dash, f, Press),
        &entry(Command::Jump, f, Tap)
    ));
    assert!(!conflicts(
        &entry(Command::Dash, f, Tap),
        &entry(Command::Sprint, f, Hold)
    ));
    assert!(conflicts(
        &entry(Command::Shoot, f, Press),
        &entry(Command::Sprint, f, Hold)
    ));
    assert!(conflicts(
        &entry(Command::Dash, f, Hold),
        &entry(Command::Sprint, f, Hold)
    ));
    let south = pad(GilrsButton::South);
    assert!(!conflicts(
        &entry(Command::Jump, south, Press),
        &entry(Command::NavConfirm, south, Press)
    ));
}

#[test]
fn a_guard_violation_is_reported_per_device_class() {
    let layer = author(&[(Command::NavCancel, PAD, vec![])]);
    let table = build(&layer, &PlayerLayer::default(), all_facts());
    assert_eq!(table.guard_violations(), vec![(Command::NavCancel, PAD)]);
}

// --- Confirm/cancel swap and stick swap ---

#[test]
fn the_confirm_cancel_swap_applies_after_player_overrides_on_gamepad_only() {
    let rows = player(&[(Command::NavConfirm, PAD, vec![Some(pad(GilrsButton::West))])]);
    let table = EffectiveTable::build(&AuthorLayer::default(), &rows, all_facts(), true);
    assert_eq!(
        table.inputs(Command::NavCancel, PAD),
        vec![pad(GilrsButton::West)]
    );
    assert_eq!(
        table.inputs(Command::NavConfirm, PAD),
        vec![pad(GilrsButton::East)]
    );
    assert_eq!(
        table.inputs(Command::NavConfirm, KBM),
        vec![key(KeyCode::Enter), key(KeyCode::NumpadEnter)]
    );
}

#[test]
fn binding_look_and_move_to_the_other_sticks_swaps_them() {
    let half = |axis, half| PhysicalInput::GamepadAxisHalf(axis, half);
    let layer = author(&[
        (
            Command::LookX,
            PAD,
            vec![press(PhysicalInput::GamepadAxis(GilrsAxis::LeftStickX))],
        ),
        (
            Command::LookY,
            PAD,
            vec![press(PhysicalInput::GamepadAxis(GilrsAxis::LeftStickY))],
        ),
        (
            Command::MoveForward,
            PAD,
            vec![press(half(GilrsAxis::RightStickY, AxisHalf::Positive))],
        ),
        (
            Command::MoveBack,
            PAD,
            vec![press(half(GilrsAxis::RightStickY, AxisHalf::Negative))],
        ),
        (
            Command::MoveLeft,
            PAD,
            vec![press(half(GilrsAxis::RightStickX, AxisHalf::Negative))],
        ),
        (
            Command::MoveRight,
            PAD,
            vec![press(half(GilrsAxis::RightStickX, AxisHalf::Positive))],
        ),
    ]);
    let table = build(&layer, &PlayerLayer::default(), all_facts());
    let mut sys = InputSystem::new(table.gameplay_bindings());
    sys.set_gamepad_axis(GilrsAxis::RightStickY, 0.6);
    sys.set_gamepad_axis(GilrsAxis::LeftStickX, 0.5);
    let look = sys.drain_look_inputs();
    let snap = sys.snapshot();
    assert!((snap.axis_value(Action::MoveForward) - 0.6).abs() < 1e-6);
    assert!(look.yaw_velocity < 0.0, "left stick right looks right");
    assert_eq!(look.pitch_velocity, 0.0);
    // The look dead zone moves to the left stick with look.
    sys.set_gamepad_look_dead_zone(0.35);
    assert_eq!(
        sys.stick_dead_zone(GilrsAxis::LeftStickX, GilrsAxis::LeftStickY),
        0.35
    );
    assert_eq!(
        sys.stick_dead_zone(GilrsAxis::RightStickX, GilrsAxis::RightStickY),
        0.15
    );
}

// --- Rebuilds while a key is held ---

#[test]
fn a_tuning_rebuild_that_adds_a_shift_tap_keeps_a_held_sprint_and_fires_no_dash() {
    let sprint_hold = author(&[
        (
            Command::Sprint,
            KBM,
            vec![with(key(KeyCode::ShiftLeft), ActivatorKind::Hold, 0.2)],
        ),
        (
            Command::Dash,
            KBM,
            vec![with(key(KeyCode::ShiftLeft), ActivatorKind::Tap, 0.2)],
        ),
    ]);
    let no_dash = RelevanceFacts {
        dash: false,
        ..all_facts()
    };
    let before = build(&sprint_hold, &PlayerLayer::default(), no_dash);
    let after = build(&sprint_hold, &PlayerLayer::default(), all_facts());
    let mut sys = InputSystem::new(before.gameplay_bindings());
    let mut latch = GameplayInputLatch::new();
    sys.handle_keyboard_event_at(KeyCode::ShiftLeft, true, 0.0);
    assert_eq!(
        sys.snapshot_at(0.3).button(Action::Sprint),
        ButtonState::Pressed
    );
    sys.set_bindings_at(after.gameplay_bindings(), 0.4);
    assert_eq!(
        sys.snapshot_at(0.5).button(Action::Sprint),
        ButtonState::Held
    );
    sys.handle_keyboard_event_at(KeyCode::ShiftLeft, false, 0.6);
    let release = sys.snapshot_at(0.61);
    assert_eq!(release.button(Action::Sprint), ButtonState::Released);
    assert_eq!(release.button(Action::Dash), ButtonState::Inactive);
    let tick = latch.snapshot_for_ticks(&release, 1).unwrap();
    assert_eq!(tick.button(Action::Dash), ButtonState::Inactive);
}

#[test]
fn a_rebuild_cancels_a_pending_resolution_on_a_changed_key() {
    let tap_dash = author(&[(
        Command::Dash,
        KBM,
        vec![with(key(KeyCode::ShiftLeft), ActivatorKind::Tap, 0.2)],
    )]);
    let table = build(&tap_dash, &PlayerLayer::default(), all_facts());
    let mut sys = InputSystem::new(table.gameplay_bindings());
    sys.handle_keyboard_event_at(KeyCode::ShiftLeft, true, 0.0);
    let _ = sys.snapshot_at(0.05);
    let retimed = author(&[(
        Command::Dash,
        KBM,
        vec![with(key(KeyCode::ShiftLeft), ActivatorKind::Tap, 0.3)],
    )]);
    let rebuilt = build(&retimed, &PlayerLayer::default(), all_facts());
    sys.set_bindings_at(rebuilt.gameplay_bindings(), 0.06);
    sys.handle_keyboard_event_at(KeyCode::ShiftLeft, false, 0.1);
    assert_eq!(
        sys.snapshot_at(0.11).button(Action::Dash),
        ButtonState::Inactive
    );
}

#[test]
fn a_rebuild_that_leaves_a_key_unchanged_resolves_it_as_before() {
    let tap_dash = author(&[(
        Command::Dash,
        KBM,
        vec![with(key(KeyCode::ShiftLeft), ActivatorKind::Tap, 0.2)],
    )]);
    let table = build(&tap_dash, &PlayerLayer::default(), all_facts());
    let mut sys = InputSystem::new(table.gameplay_bindings());
    sys.handle_keyboard_event_at(KeyCode::ShiftLeft, true, 0.0);
    let _ = sys.snapshot_at(0.05);
    let mut with_jump = tap_dash.clone();
    with_jump
        .defaults
        .insert((Command::Jump, KBM), vec![press(key(KeyCode::KeyJ))]);
    let rebuilt = build(&with_jump, &PlayerLayer::default(), all_facts());
    sys.set_bindings_at(rebuilt.gameplay_bindings(), 0.06);
    sys.handle_keyboard_event_at(KeyCode::ShiftLeft, false, 0.1);
    assert_eq!(
        sys.snapshot_at(0.11).button(Action::Dash),
        ButtonState::Pressed
    );
}

#[test]
fn a_key_held_through_a_rebuild_that_newly_binds_it_fires_only_on_its_next_press() {
    let table = build(
        &AuthorLayer::default(),
        &PlayerLayer::default(),
        all_facts(),
    );
    let mut sys = InputSystem::new(table.gameplay_bindings());
    sys.handle_keyboard_event_at(KeyCode::KeyV, true, 0.0);
    let _ = sys.snapshot_at(0.01);
    let v_dash = author(&[(Command::Dash, KBM, vec![press(key(KeyCode::KeyV))])]);
    let rebuilt = build(&v_dash, &PlayerLayer::default(), all_facts());
    sys.set_bindings_at(rebuilt.gameplay_bindings(), 0.02);
    assert!(!sys.snapshot_at(0.03).button(Action::Dash).is_active());
    sys.handle_keyboard_event_at(KeyCode::KeyV, false, 0.04);
    let _ = sys.snapshot_at(0.05);
    sys.handle_keyboard_event_at(KeyCode::KeyV, true, 0.06);
    assert_eq!(
        sys.snapshot_at(0.07).button(Action::Dash),
        ButtonState::Pressed
    );
}

#[test]
fn a_dpad_press_steps_weapon_cycling() {
    // Regression: notch counts came from the wheel alone, so the D-pad's
    // default cycle bindings did nothing.
    let table = build(
        &AuthorLayer::default(),
        &PlayerLayer::default(),
        all_facts(),
    );
    let mut sys = InputSystem::new(table.gameplay_bindings());
    sys.set_physical_input(pad(GilrsButton::DPadRight), true);
    assert_eq!(sys.snapshot().notch_count(Action::CycleWieldableNext), 1);
    assert_eq!(sys.snapshot().notch_count(Action::CycleWieldableNext), 0);
}

#[test]
fn a_player_dash_row_waits_out_an_irrelevant_session_and_applies_when_dash_returns() {
    let rows = player(&[(Command::Dash, KBM, vec![Some(key(KeyCode::KeyV))])]);
    let no_dash = RelevanceFacts {
        dash: false,
        ..all_facts()
    };
    let table = build(&AuthorLayer::default(), &rows, no_dash);
    assert!(table.inputs(Command::Dash, KBM).is_empty());
    assert!(table.displaced().is_empty());
    let table = build(&AuthorLayer::default(), &rows, all_facts());
    assert_eq!(table.inputs(Command::Dash, KBM), vec![key(KeyCode::KeyV)]);
}

// --- Guard over the player layer, player-vs-player, menu, swap order ---

#[test]
fn a_hand_edited_empty_cancel_row_gets_its_default_back() {
    let rows = player(&[(Command::NavCancel, PAD, vec![])]);
    let table = build(&AuthorLayer::default(), &rows, all_facts());
    assert_eq!(
        table.inputs(Command::NavCancel, PAD),
        [pad(GilrsButton::East)]
    );
    assert_eq!(table.guard_restored(), [(Command::NavCancel, PAD)]);
    assert!(table.guard_violations().is_empty());
}

#[test]
fn a_saved_confirm_on_escape_yields_it_back_to_cancel_and_menu() {
    // Confirm on Escape would leave cancel and menu with no keyboard input.
    let rows = player(&[(Command::NavConfirm, KBM, vec![Some(key(KeyCode::Escape))])]);
    let table = build(&AuthorLayer::default(), &rows, all_facts());
    assert_eq!(
        table.inputs(Command::NavCancel, KBM),
        [key(KeyCode::Escape)]
    );
    assert_eq!(table.inputs(Command::NavMenu, KBM), [key(KeyCode::Escape)]);
    assert_eq!(
        table.inputs(Command::NavConfirm, KBM),
        [key(KeyCode::Enter), key(KeyCode::NumpadEnter)],
        "confirm, left with nothing, gets its own default back"
    );
    assert!(table.displaced().contains(&(Command::NavConfirm, KBM)));
    assert!(
        !table.displaced().contains(&(Command::NavCancel, KBM)),
        "cancel has its default again, so it is not flagged"
    );
    assert!(table.guard_violations().is_empty());
}

#[test]
fn a_newer_author_cancel_on_a_players_input_takes_it_back_and_flags_the_player_row() {
    let layer = author(&[(
        Command::NavCancel,
        PAD,
        vec![press(pad(GilrsButton::North))],
    )]);
    let rows = player(&[(
        Command::NavOptions,
        PAD,
        vec![Some(pad(GilrsButton::North))],
    )]);
    let table = build(&layer, &rows, all_facts());
    assert_eq!(
        table.inputs(Command::NavCancel, PAD),
        [pad(GilrsButton::North)]
    );
    assert!(table.inputs(Command::NavOptions, PAD).is_empty());
    assert!(table.displaced().contains(&(Command::NavOptions, PAD)));
}

#[test]
fn the_guard_restore_warns_once_and_an_unchanged_rebuild_keeps_the_generation() {
    use super::binding_state::{BindingSources, BindingState};
    use log::Level;
    use postretro_test_log_capture::LogCapture;

    let capture = LogCapture::start();
    let mut state = BindingState::default();
    state.set_player_layer(player(&[(Command::NavCancel, PAD, vec![])]));
    let mut input = InputSystem::new(Vec::new());
    let sources = BindingSources {
        entity_types_generation: 1,
        tuning: None,
    };
    state.rebuild(sources, all_facts(), &mut input);
    let generation = state.generation();
    state.rebuild(
        BindingSources {
            entity_types_generation: 2,
            ..sources
        },
        all_facts(),
        &mut input,
    );
    capture.assert_logged_once(Level::Warn, "leave `nav_cancel` unbound on gamepad");
    assert_eq!(
        state.generation(),
        generation,
        "a rebuild that changes nothing keeps the generation"
    );
}

#[test]
fn a_player_row_losing_to_another_player_row_is_flagged() {
    // Dash's saved row wakes up on the key the player later gave jump.
    let rows = player(&[
        (Command::Jump, KBM, vec![Some(key(KeyCode::KeyV))]),
        (Command::Dash, KBM, vec![Some(key(KeyCode::KeyV))]),
    ]);
    let table = build(&AuthorLayer::default(), &rows, all_facts());
    assert_eq!(table.inputs(Command::Jump, KBM), [key(KeyCode::KeyV)]);
    assert!(table.inputs(Command::Dash, KBM).is_empty());
    assert!(table.displaced().contains(&(Command::Dash, KBM)));
}

#[test]
fn menu_and_a_gameplay_command_on_one_input_conflict_whatever_the_activators() {
    use ActivatorKind::*;
    let space = key(KeyCode::Space);
    assert!(conflicts(
        &entry(Command::NavMenu, space, Press),
        &entry(Command::Jump, space, Press)
    ));
    assert!(conflicts(
        &entry(Command::NavMenu, space, Press),
        &entry(Command::Jump, space, Hold)
    ));
    let escape = key(KeyCode::Escape);
    assert!(!conflicts(
        &entry(Command::NavMenu, escape, Press),
        &entry(Command::NavCancel, escape, Press)
    ));
    // A player's menu on Space takes it from jump, flagged.
    let rows = player(&[(Command::NavMenu, KBM, vec![Some(space)])]);
    let table = build(&AuthorLayer::default(), &rows, all_facts());
    assert!(table.inputs(Command::Jump, KBM).is_empty());
    assert!(table.displaced().contains(&(Command::Jump, KBM)));
}

#[test]
fn the_swap_applies_before_collisions_so_menu_never_shares_confirms_button() {
    // Menu on East pairs with cancel; with the swap on, East is confirm, so
    // menu yields it back to confirm (the guard) and is flagged.
    // Dash, which also defaults to East, stays irrelevant here.
    let rows = player(&[(Command::NavMenu, PAD, vec![Some(pad(GilrsButton::East))])]);
    let facts = RelevanceFacts::default();
    let unswapped = EffectiveTable::build(&AuthorLayer::default(), &rows, facts, false);
    assert_eq!(
        unswapped.inputs(Command::NavMenu, PAD),
        [pad(GilrsButton::East)]
    );
    assert!(unswapped.displaced().is_empty());

    let swapped = EffectiveTable::build(&AuthorLayer::default(), &rows, facts, true);
    let on_east: Vec<Command> = swapped
        .entries()
        .iter()
        .filter(|e| e.input == pad(GilrsButton::East) && e.command.context() == CommandContext::Ui)
        .map(|e| e.command)
        .collect();
    assert_eq!(on_east, [Command::NavConfirm]);
    assert_eq!(
        swapped.inputs(Command::NavMenu, PAD),
        [pad(GilrsButton::Start)]
    );
    assert!(swapped.displaced().contains(&(Command::NavMenu, PAD)));
}

#[test]
fn a_default_input_keeps_its_own_activator_wherever_the_row_puts_it() {
    let layer = author(&[(
        Command::Use,
        KBM,
        vec![
            press(key(KeyCode::KeyE)),
            with(key(KeyCode::KeyF), ActivatorKind::Hold, 0.3),
        ],
    )]);
    let rows = player(&[(Command::Use, KBM, vec![Some(key(KeyCode::KeyF))])]);
    let table = build(&layer, &rows, all_facts());
    let use_kbm: Vec<_> = table
        .entries()
        .iter()
        .filter(|e| e.command == Command::Use && e.class == KBM)
        .map(|e| (e.input, e.activator))
        .collect();
    assert_eq!(
        use_kbm,
        [(
            key(KeyCode::KeyF),
            Activator::with_threshold(ActivatorKind::Hold, 0.3)
        )]
    );
}

#[test]
fn a_wheel_notch_in_a_player_row_always_presses() {
    let layer = author(&[(
        Command::Sprint,
        KBM,
        vec![with(key(KeyCode::ShiftLeft), ActivatorKind::Hold, 0.3)],
    )]);
    let rows = player(&[(
        Command::Sprint,
        KBM,
        vec![Some(PhysicalInput::MouseWheelDown)],
    )]);
    let table = build(&layer, &rows, all_facts());
    let wheel = table
        .entries()
        .iter()
        .find(|e| e.command == Command::Sprint && e.input == PhysicalInput::MouseWheelDown)
        .unwrap();
    assert_eq!(wheel.activator, Activator::PRESS);
}

// Regression: a swap within sprint's row moved its hold onto the other key.
#[test]
fn a_non_default_input_moved_into_a_defaults_slot_by_a_swap_takes_press() {
    let layer = author(&[(
        Command::Sprint,
        KBM,
        vec![with(key(KeyCode::ShiftLeft), ActivatorKind::Hold, 0.3)],
    )]);
    let rows = player(&[(
        Command::Sprint,
        KBM,
        vec![Some(key(KeyCode::KeyN)), Some(key(KeyCode::ShiftLeft))],
    )]);
    let table = build(&layer, &rows, all_facts());
    let sprint: Vec<_> = table
        .entries()
        .iter()
        .filter(|e| e.command == Command::Sprint && e.class == KBM)
        .map(|e| (e.input, e.activator))
        .collect();
    assert_eq!(
        sprint,
        [
            (key(KeyCode::KeyN), Activator::PRESS),
            (
                key(KeyCode::ShiftLeft),
                Activator::with_threshold(ActivatorKind::Hold, 0.3)
            ),
        ],
        "the hold stays on Shift and is never copied onto N"
    );
}

#[test]
fn an_input_that_replaced_a_default_takes_its_activator_wherever_another_default_sits() {
    // `use` defaults to E (press) and F (hold). N replaced F; E is still in
    // the row, so N takes F's hold and E keeps its press.
    let layer = author(&[(
        Command::Use,
        KBM,
        vec![
            press(key(KeyCode::KeyE)),
            with(key(KeyCode::KeyF), ActivatorKind::Hold, 0.3),
        ],
    )]);
    let rows = player(&[(
        Command::Use,
        KBM,
        vec![Some(key(KeyCode::KeyE)), Some(key(KeyCode::KeyN))],
    )]);
    let table = build(&layer, &rows, all_facts());
    let use_kbm: Vec<_> = table
        .entries()
        .iter()
        .filter(|e| e.command == Command::Use && e.class == KBM)
        .map(|e| (e.input, e.activator.kind))
        .collect();
    assert_eq!(
        use_kbm,
        [
            (key(KeyCode::KeyE), ActivatorKind::Press),
            (key(KeyCode::KeyN), ActivatorKind::Hold),
        ]
    );
}
