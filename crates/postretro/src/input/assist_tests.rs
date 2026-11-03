// The `assist` command: relevance, its `hold` engine default on Use's inputs,
// shared-key resolution, the rebuild rule, and the level reaching `SimCommand`.
// See: context/lib/input.md §2 (Relevance, Activators, Shared keys)
// Spec: E18--player-revive-assist AC7, AC8, O26, O52, O53.

use gilrs::Button as GilrsButton;
use winit::keyboard::KeyCode;

use super::binding_table::{
    AuthorBinding, AuthorLayer, BindingOrigin, EffectiveTable, PlayerLayer,
};
use super::commands::Command;
use super::device_family::DeviceFamily;
use super::glyphs::{GlyphView, resolve_glyph};
use super::input_names::{DeviceClass, input_label};
use super::relevance::{Relevance, RelevanceFacts};
use super::snapshot::ActionSnapshot;
use super::types::{
    Activator, ActivatorKind, ButtonState, DEFAULT_ACTIVATOR_THRESHOLD, PhysicalInput,
};
use super::{Action, InputSystem};

const KBM: DeviceClass = DeviceClass::KeyboardMouse;
const PAD: DeviceClass = DeviceClass::Gamepad;
const KEY_E: PhysicalInput = PhysicalInput::Key(KeyCode::KeyE);
const WEST: PhysicalInput = PhysicalInput::GamepadButton(GilrsButton::West);
/// Past the default hold threshold.
const PAST_HOLD: f64 = DEFAULT_ACTIVATOR_THRESHOLD as f64 + 0.1;

fn facts(assistable: bool) -> RelevanceFacts {
    RelevanceFacts {
        assistable,
        ..RelevanceFacts::default()
    }
}

fn table_with(author: &AuthorLayer, player: &PlayerLayer, assistable: bool) -> EffectiveTable {
    EffectiveTable::build(author, player, facts(assistable), false)
}

fn engine_table(assistable: bool) -> EffectiveTable {
    table_with(&AuthorLayer::default(), &PlayerLayer::default(), assistable)
}

fn on_input(table: &EffectiveTable, input: PhysicalInput) -> Vec<(Command, ActivatorKind)> {
    table
        .entries()
        .iter()
        .filter(|entry| {
            entry.input == input && entry.command.context() == super::CommandContext::Gameplay
        })
        .map(|entry| (entry.command, entry.activator.kind))
        .collect()
}

/// The command the frame loop builds from a snapshot: `Action::Assist`'s level
/// read as `frame_loop` reads it, passed through `build_sim_command`.
fn sim_command(snapshot: &ActionSnapshot) -> postretro_sim::sim::SimCommand {
    let camera = crate::camera::Camera::new(glam::Vec3::ZERO, 0.0, 0.0);
    crate::build_sim_command(
        snapshot,
        &camera,
        crate::sim_intents::SimIntents {
            use_: matches!(snapshot.button(Action::Use), ButtonState::Pressed),
            assist: snapshot.button(Action::Assist).is_active(),
            ..Default::default()
        },
    )
}

/// Snapshot `sys` at each time and collect `action`'s states.
fn states(sys: &mut InputSystem, action: Action, times: &[f64]) -> Vec<ButtonState> {
    times
        .iter()
        .map(|t| sys.snapshot_at(*t).button(action))
        .collect()
}

fn pressed_count(states: &[ButtonState]) -> usize {
    states
        .iter()
        .filter(|state| **state == ButtonState::Pressed)
        .count()
}

// --- Default activator ---

#[test]
fn default_activator_is_press_for_every_command_but_assist() {
    for command in Command::ALL {
        let expected = if command == Command::Assist {
            Activator::new(ActivatorKind::Hold)
        } else {
            Activator::PRESS
        };
        assert_eq!(command.default_activator(), expected, "{}", command.id());
    }
    assert_eq!(
        Command::Assist.default_activator().threshold,
        DEFAULT_ACTIVATOR_THRESHOLD
    );
}

// --- AC7: relevance, listing, glyphs ---

#[test]
fn irrelevant_assist_is_unbound_unlisted_and_draws_no_glyph() {
    let table = engine_table(false);
    assert_eq!(table.relevance(Command::Assist), Relevance::Irrelevant);
    assert!(
        table
            .entries()
            .iter()
            .all(|entry| entry.command != Command::Assist)
    );
    assert!(
        table
            .conflicting_pairs()
            .iter()
            .all(|(a, b)| a.command != Command::Assist && b.command != Command::Assist)
    );
    assert_eq!(
        on_input(&table, KEY_E),
        [(Command::Use, ActivatorKind::Press)]
    );
    assert_eq!(
        on_input(&table, WEST),
        [(Command::Use, ActivatorKind::Press)]
    );
    let glyph = resolve_glyph(
        &table,
        &Default::default(),
        DeviceFamily::KeyboardMouse,
        Command::Assist,
        |_| false,
    );
    assert_eq!(glyph, None);
}

#[test]
fn relevant_assist_binds_hold_on_key_e_and_west_beside_use_press_with_no_conflict() {
    let table = engine_table(true);
    assert_eq!(table.relevance(Command::Assist), Relevance::Relevant);
    for input in [KEY_E, WEST] {
        assert_eq!(
            on_input(&table, input),
            [
                (Command::Use, ActivatorKind::Press),
                (Command::Assist, ActivatorKind::Hold),
            ],
            "{input:?}"
        );
    }
    let assist = table
        .entries()
        .iter()
        .find(|entry| entry.command == Command::Assist)
        .unwrap();
    assert_eq!(assist.activator.threshold, DEFAULT_ACTIVATOR_THRESHOLD);
    assert_eq!(assist.origin, BindingOrigin::Engine);
    assert_eq!(table.conflicting_pairs(), []);
    assert!(table.suppressed().is_empty());

    // Every other engine default stays `press`.
    for entry in table.entries() {
        if entry.origin == BindingOrigin::Engine && entry.command != Command::Assist {
            assert_eq!(entry.activator, Activator::PRESS, "{}", entry.command.id());
        }
    }

    // Both glyphs draw the shared input.
    for (family, input) in [
        (DeviceFamily::KeyboardMouse, KEY_E),
        (DeviceFamily::Xbox, WEST),
    ] {
        assert_eq!(
            resolve_glyph(&table, &Default::default(), family, Command::Assist, |_| {
                false
            }),
            Some(GlyphView::Label(input_label(input))),
        );
    }
}

#[test]
fn a_player_row_binding_use_on_key_e_drops_the_engine_assist_hold_there() {
    let player = PlayerLayer {
        rows: [((Command::Use, KBM), vec![Some(KEY_E)])]
            .into_iter()
            .collect(),
    };
    let table = table_with(&AuthorLayer::default(), &player, true);
    assert_eq!(
        on_input(&table, KEY_E),
        [(Command::Use, ActivatorKind::Press)]
    );
    assert!(table.inputs(Command::Assist, KBM).is_empty());
    assert!(table.displaced().contains(&(Command::Assist, KBM)));
    // The gamepad keeps its engine pair, and Use on E fires on press again.
    assert_eq!(table.inputs(Command::Assist, PAD), [WEST]);
    let mut sys = InputSystem::new(table.gameplay_bindings());
    sys.handle_keyboard_event_at(KeyCode::KeyE, true, 0.0);
    assert_eq!(
        sys.snapshot_at(0.01).button(Action::Use),
        ButtonState::Pressed
    );
}

// --- AC8: shared-key resolution ---

#[test]
fn holding_key_e_or_west_past_the_threshold_drives_assist_and_never_use() {
    let table = engine_table(true);
    for pad in [false, true] {
        let mut sys = InputSystem::new(table.gameplay_bindings());
        let edge = |sys: &mut InputSystem, down: bool, t: f64| {
            if pad {
                sys.handle_gamepad_button_event_at(GilrsButton::West, down, t);
            } else {
                sys.handle_keyboard_event_at(KeyCode::KeyE, down, t);
            }
        };
        edge(&mut sys, true, 0.0);
        let mut use_states = Vec::new();
        let mut held = Vec::new();
        for t in [0.05, 0.1, PAST_HOLD, PAST_HOLD + 0.1] {
            let snap = sys.snapshot_at(t);
            use_states.push(snap.button(Action::Use));
            held.push(sim_command(&snap).assist_held);
        }
        assert_eq!(held, [false, false, true, true], "pad: {pad}");
        edge(&mut sys, false, PAST_HOLD + 0.2);
        for t in [PAST_HOLD + 0.25, PAST_HOLD + 0.3] {
            let snap = sys.snapshot_at(t);
            use_states.push(snap.button(Action::Use));
            assert!(!sim_command(&snap).assist_held, "release clears the level");
        }
        assert_eq!(pressed_count(&use_states), 0, "pad: {pad}: {use_states:?}");
    }
}

#[test]
fn releasing_key_e_before_the_threshold_fires_use_once_and_never_assist() {
    let table = engine_table(true);
    let mut sys = InputSystem::new(table.gameplay_bindings());
    sys.handle_keyboard_event_at(KeyCode::KeyE, true, 0.0);
    let before = states(&mut sys, Action::Use, &[0.05]);
    sys.handle_keyboard_event_at(KeyCode::KeyE, false, 0.1);
    let mut after = Vec::new();
    for t in [0.11, 0.2, PAST_HOLD + 0.1] {
        let snap = sys.snapshot_at(t);
        after.push(snap.button(Action::Use));
        assert!(!sim_command(&snap).assist_held);
        assert!(!snap.button(Action::Assist).is_active());
    }
    assert_eq!(before, [ButtonState::Inactive], "Use waits for the release");
    assert_eq!(pressed_count(&after), 1, "{after:?}");
}

#[test]
fn irrelevant_assist_leaves_use_firing_on_press() {
    let table = engine_table(false);
    let mut sys = InputSystem::new(table.gameplay_bindings());
    sys.handle_keyboard_event_at(KeyCode::KeyE, true, 0.0);
    let snap = sys.snapshot_at(0.01);
    assert_eq!(snap.button(Action::Use), ButtonState::Pressed);
    let snap = sys.snapshot_at(PAST_HOLD);
    assert!(!sim_command(&snap).assist_held);
}

// --- AC7: any binding sets the level ---

#[test]
fn an_author_press_or_hold_row_sets_assist_held_while_down_and_clears_on_release() {
    for (kind, fires_at) in [
        (ActivatorKind::Press, 0.01),
        (ActivatorKind::Hold, PAST_HOLD),
    ] {
        let author = AuthorLayer {
            defaults: [(
                (Command::Assist, KBM),
                vec![AuthorBinding {
                    input: PhysicalInput::Key(KeyCode::KeyV),
                    activator: Activator::new(kind),
                }],
            )]
            .into_iter()
            .collect(),
            manifest_order: vec![Command::Assist],
            ..AuthorLayer::default()
        };
        let table = table_with(&author, &PlayerLayer::default(), true);
        let mut sys = InputSystem::new(table.gameplay_bindings());
        sys.handle_keyboard_event_at(KeyCode::KeyV, true, 0.0);
        let held: Vec<bool> = [fires_at, fires_at + 0.05, fires_at + 0.1]
            .iter()
            .map(|t| sim_command(&sys.snapshot_at(*t)).assist_held)
            .collect();
        assert_eq!(held, [true, true, true], "{kind:?}");
        sys.handle_keyboard_event_at(KeyCode::KeyV, false, fires_at + 0.2);
        assert!(!sim_command(&sys.snapshot_at(fires_at + 0.25)).assist_held);
    }
}

// --- Orderings ---

/// O52: relevance gained while `KeyE` is held. Use fired on the press; the
/// rebuild that adds `assist` waits for a fresh press, so the held key never
/// sets `assist_held` and its release fires nothing.
#[test]
fn o52_relevance_gained_mid_press_waits_for_a_fresh_press() {
    let mut sys = InputSystem::new(engine_table(false).gameplay_bindings());
    sys.handle_keyboard_event_at(KeyCode::KeyE, true, 0.0);
    let mut use_states = states(&mut sys, Action::Use, &[0.01]);
    assert_eq!(use_states, [ButtonState::Pressed]);

    sys.set_bindings_at(engine_table(true).gameplay_bindings(), 0.05);
    for t in [0.1, PAST_HOLD, PAST_HOLD + 0.5] {
        let snap = sys.snapshot_at(t);
        use_states.push(snap.button(Action::Use));
        assert!(!sim_command(&snap).assist_held, "t = {t}");
    }
    sys.handle_keyboard_event_at(KeyCode::KeyE, false, 1.0);
    for t in [1.01, 1.1] {
        let snap = sys.snapshot_at(t);
        use_states.push(snap.button(Action::Use));
        assert!(!snap.button(Action::Assist).is_active());
    }
    assert_eq!(pressed_count(&use_states), 1, "{use_states:?}");

    // A fresh press resolves against the new table.
    sys.handle_keyboard_event_at(KeyCode::KeyE, true, 2.0);
    assert!(sim_command(&sys.snapshot_at(2.0 + PAST_HOLD)).assist_held);
}

/// O53: relevance lost while `assist` is down. The rebuild lifts it at once,
/// the next command carries `assist_held` false as a release does, and Use
/// does not fire on the release.
#[test]
fn o53_relevance_lost_mid_hold_lifts_assist_at_once() {
    let mut sys = InputSystem::new(engine_table(true).gameplay_bindings());
    sys.handle_keyboard_event_at(KeyCode::KeyE, true, 0.0);
    assert!(sim_command(&sys.snapshot_at(PAST_HOLD)).assist_held);

    sys.set_bindings_at(engine_table(false).gameplay_bindings(), PAST_HOLD + 0.05);
    let mut use_states = Vec::new();
    for t in [PAST_HOLD + 0.06, PAST_HOLD + 0.2] {
        let snap = sys.snapshot_at(t);
        use_states.push(snap.button(Action::Use));
        assert!(!sim_command(&snap).assist_held, "t = {t}");
    }
    sys.handle_keyboard_event_at(KeyCode::KeyE, false, 1.0);
    use_states.extend(states(&mut sys, Action::Use, &[1.01, 1.1]));
    assert_eq!(pressed_count(&use_states), 0, "{use_states:?}");
}

/// O26: focus loss mid-hold. `clear_all` lifts `assist` without a pulse, the
/// next command carries `assist_held` false, and Use does not fire, whether
/// the hold had resolved or was still pending.
#[test]
fn o26_focus_loss_mid_hold_lifts_assist_without_a_pulse_and_fires_no_use() {
    for resolved in [true, false] {
        let mut sys = InputSystem::new(engine_table(true).gameplay_bindings());
        sys.handle_keyboard_event_at(KeyCode::KeyE, true, 0.0);
        let clear_at = if resolved { PAST_HOLD } else { 0.1 };
        assert_eq!(
            sim_command(&sys.snapshot_at(clear_at)).assist_held,
            resolved
        );
        sys.clear_all_at(clear_at + 0.01);
        let snap = sys.snapshot_at(clear_at + 0.02);
        assert_eq!(snap.button(Action::Assist), ButtonState::Inactive);
        assert!(!sim_command(&snap).assist_held);
        let mut use_states = vec![snap.button(Action::Use)];
        sys.handle_keyboard_event_at(KeyCode::KeyE, false, 1.0);
        use_states.extend(states(&mut sys, Action::Use, &[1.01, 1.1]));
        assert_eq!(pressed_count(&use_states), 0, "resolved: {resolved}");
    }
}
