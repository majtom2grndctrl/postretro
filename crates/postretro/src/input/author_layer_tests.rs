use log::Level;
use postretro_scripting_core::runtime::{ModInputBinding, ModInputBlock, ModInputCommand};
use postretro_test_log_capture::LogCapture;
use winit::keyboard::KeyCode;

use super::binding_state::{BindingSources, BindingState};
use super::binding_table::{EffectiveTable, PlayerLayer};
use super::commands::Command;
use super::input_names::DeviceClass;
use super::relevance::{Relevance, RelevanceFacts};
use super::types::{Action, ActivatorKind, ButtonState, PhysicalInput};
use super::{InputSystem, author_layer_from_block, default_bindings};

const KBM: DeviceClass = DeviceClass::KeyboardMouse;
const PAD: DeviceClass = DeviceClass::Gamepad;

fn bind(input: &str) -> ModInputBinding {
    ModInputBinding {
        input: input.to_string(),
        activator: None,
        threshold: None,
    }
}

fn bind_with(input: &str, activator: &str, threshold: Option<f64>) -> ModInputBinding {
    ModInputBinding {
        input: input.to_string(),
        activator: Some(activator.to_string()),
        threshold,
    }
}

fn command(
    id: &str,
    keyboard_mouse: Option<Vec<ModInputBinding>>,
    gamepad: Option<Vec<ModInputBinding>>,
) -> ModInputCommand {
    ModInputCommand {
        id: id.to_string(),
        keyboard_mouse,
        gamepad,
        ..ModInputCommand::default()
    }
}

fn block(commands: Vec<ModInputCommand>) -> ModInputBlock {
    ModInputBlock {
        commands,
        ..ModInputBlock::default()
    }
}

fn all_facts() -> RelevanceFacts {
    RelevanceFacts {
        dash: true,
        crouch: true,
        magazine: true,
        secondary: true,
    }
}

fn table_for(block: &ModInputBlock) -> EffectiveTable {
    EffectiveTable::build(
        &author_layer_from_block(Some(block)),
        &PlayerLayer::default(),
        all_facts(),
        false,
    )
}

fn key(code: KeyCode) -> PhysicalInput {
    PhysicalInput::Key(code)
}

fn pad(button: gilrs::Button) -> PhysicalInput {
    PhysicalInput::GamepadButton(button)
}

// --- An author block rebinds one command onto another's key ---

#[test]
fn shift_dash_with_an_empty_sprint_keyboard_list_drives_dash_and_never_runs() {
    let table = table_for(&block(vec![
        command(
            "dash",
            Some(vec![bind_with("ShiftLeft", "tap", Some(0.2))]),
            None,
        ),
        command("sprint", Some(vec![]), None),
    ]));
    let mut sys = InputSystem::new(table.gameplay_bindings());
    sys.handle_keyboard_event_at(KeyCode::ShiftLeft, true, 0.0);
    sys.handle_keyboard_event_at(KeyCode::ShiftLeft, false, 0.05);
    assert_eq!(
        sys.snapshot_at(0.06).button(Action::Dash),
        ButtonState::Pressed
    );
    sys.handle_keyboard_event_at(KeyCode::KeyF, true, 0.1);
    let held = sys.snapshot_at(0.11);
    assert!(!held.button(Action::Dash).is_active(), "F drives nothing");
    sys.handle_keyboard_event_at(KeyCode::ShiftLeft, true, 0.2);
    assert!(!sys.snapshot_at(1.0).button(Action::Sprint).is_active());
}

// --- Accepted activators per command ---

#[test]
fn each_command_accepts_its_activators_and_diagnoses_the_rest() {
    let capture = LogCapture::start();
    let table = table_for(&block(vec![
        command("shoot", Some(vec![bind_with("KeyT", "tap", None)]), None),
        command(
            "alt_fire",
            Some(vec![bind_with("KeyY", "press", None)]),
            None,
        ),
        command("sprint", Some(vec![bind_with("KeyU", "hold", None)]), None),
        command("crouch", Some(vec![bind_with("KeyI", "tap", None)]), None),
    ]));
    capture.assert_logged_once(Level::Warn, "`shoot` does not accept the `tap` activator");
    capture.assert_logged_once(Level::Warn, "`crouch` does not accept the `tap` activator");
    capture.assert_not_logged(Level::Warn, "`alt_fire` does not accept");
    capture.assert_not_logged(Level::Warn, "`sprint` does not accept");
    // A diagnosed entry falls back for that command and device class only.
    assert_eq!(
        table.inputs(Command::Shoot, KBM),
        vec![PhysicalInput::MouseButton(winit::event::MouseButton::Left)]
    );
    assert_eq!(
        table.inputs(Command::Shoot, PAD),
        vec![pad(gilrs::Button::RightTrigger2)]
    );
    assert_eq!(
        table.inputs(Command::AltFire, KBM),
        vec![key(KeyCode::KeyY)]
    );
    let sprint = table
        .entries()
        .iter()
        .find(|e| e.command == Command::Sprint && e.class == KBM)
        .unwrap();
    assert_eq!(sprint.input, key(KeyCode::KeyU));
    assert_eq!(sprint.activator.kind, ActivatorKind::Hold);
    assert_eq!(table.inputs(Command::Crouch, KBM), vec![key(KeyCode::KeyC)]);
}

// --- The guard over author defaults ---

#[test]
fn defaults_that_unbind_a_guarded_command_are_diagnosed_and_fall_back() {
    let capture = LogCapture::start();
    let table = table_for(&block(vec![command("nav_cancel", None, Some(vec![]))]));
    capture.assert_logged_once(Level::Warn, "leaves `nav_cancel` unbound on gamepad");
    assert_eq!(
        table.inputs(Command::NavCancel, PAD),
        vec![pad(gilrs::Button::East)]
    );

    capture.clear();
    let table = table_for(&block(vec![command(
        "nav_confirm",
        None,
        Some(vec![bind("north")]),
    )]));
    capture.assert_not_logged(Level::Warn, "unbound on");
    assert_eq!(
        table.inputs(Command::NavConfirm, PAD),
        vec![pad(gilrs::Button::North)]
    );
    assert!(table.guard_violations().is_empty());
}

// Regression: an author `use` on `start` beat menu's engine default, and the
// guard diagnosis had no menu entry to remove, so the pad could not pause.
#[test]
fn an_author_binding_on_menus_default_input_is_unbound_there_and_menu_keeps_it() {
    let capture = LogCapture::start();
    let table = table_for(&block(vec![command(
        "use",
        Some(vec![bind("Escape"), bind("KeyE")]),
        Some(vec![bind("start"), bind("west")]),
    )]));
    capture.assert_logged_once(
        Level::Warn,
        "`use` on `start` would leave `nav_menu` unbound on gamepad; `use` is unbound there",
    );
    capture.assert_logged_once(
        Level::Warn,
        "`use` on `Escape` would leave `nav_menu` unbound on keyboardMouse",
    );
    assert_eq!(
        table.inputs(Command::NavMenu, PAD),
        vec![pad(gilrs::Button::Start)]
    );
    assert_eq!(
        table.inputs(Command::NavMenu, KBM),
        vec![key(KeyCode::Escape)]
    );
    assert_eq!(
        table.inputs(Command::Use, PAD),
        vec![pad(gilrs::Button::West)],
        "use keeps its other binding"
    );
    assert_eq!(table.inputs(Command::Use, KBM), vec![key(KeyCode::KeyE)]);
    assert!(table.guard_violations().is_empty());
}

#[test]
fn an_author_cancel_on_escape_shares_menus_key_with_the_swap_off_and_on() {
    // The swap exchanges gamepad confirm and cancel only, so keyboard cancel
    // stays cancel, and menu and cancel may share a key.
    let capture = LogCapture::start();
    let layer = author_layer_from_block(Some(&block(vec![command(
        "nav_cancel",
        Some(vec![bind("Escape")]),
        None,
    )])));
    capture.assert_not_logged(Level::Warn, "would leave");
    capture.assert_not_logged(Level::Warn, "using the engine default");
    for swap in [false, true] {
        let table = EffectiveTable::build(&layer, &PlayerLayer::default(), all_facts(), swap);
        assert_eq!(
            table.inputs(Command::NavCancel, KBM),
            vec![key(KeyCode::Escape)]
        );
        assert_eq!(
            table.inputs(Command::NavMenu, KBM),
            vec![key(KeyCode::Escape)]
        );
    }
}

// Regression: validation ran with the swap off only, where cancel on `start`
// shares menu's button; with the swap on that binding drives confirm, which
// took `start` from menu, and the pad could not pause.
#[test]
fn an_author_cancel_on_start_is_unbound_there_because_the_swap_makes_it_confirm() {
    let capture = LogCapture::start();
    let layer = author_layer_from_block(Some(&block(vec![command(
        "nav_cancel",
        None,
        Some(vec![bind("north"), bind("start")]),
    )])));
    capture.assert_logged_once(
        Level::Warn,
        "`nav_cancel` on `start` would leave `nav_menu` unbound on gamepad with the \
         confirm/cancel swap on; `nav_cancel` is unbound there",
    );
    for swap in [false, true] {
        let table = EffectiveTable::build(&layer, &PlayerLayer::default(), all_facts(), swap);
        assert_eq!(
            table.inputs(Command::NavMenu, PAD),
            vec![pad(gilrs::Button::Start)],
            "swap {swap}"
        );
        // Cancel's row keeps `north`; with the swap on it drives confirm.
        let shown = if swap {
            Command::NavConfirm
        } else {
            Command::NavCancel
        };
        assert_eq!(
            table.inputs(shown, PAD),
            vec![pad(gilrs::Button::North)],
            "swap {swap}"
        );
        assert!(table.guard_violations().is_empty(), "swap {swap}");
    }
}

#[test]
fn a_gameplay_binding_on_menus_input_is_unbound_there_with_the_swap_on_too() {
    let layer = author_layer_from_block(Some(&block(vec![command(
        "use",
        Some(vec![bind("Escape"), bind("KeyE")]),
        Some(vec![bind("start"), bind("west")]),
    )])));
    let table = EffectiveTable::build(&layer, &PlayerLayer::default(), all_facts(), true);
    assert_eq!(
        table.inputs(Command::NavMenu, PAD),
        vec![pad(gilrs::Button::Start)]
    );
    assert_eq!(
        table.inputs(Command::NavMenu, KBM),
        vec![key(KeyCode::Escape)]
    );
    assert_eq!(
        table.inputs(Command::Use, PAD),
        vec![pad(gilrs::Button::West)]
    );
    assert_eq!(table.inputs(Command::Use, KBM), vec![key(KeyCode::KeyE)]);
    assert!(table.guard_violations().is_empty());
}

#[test]
fn a_guarded_entry_that_takes_menus_input_falls_back_once_it_is_emptied() {
    // Confirm on Start takes menu's only pad input. Start is dropped from
    // confirm, which leaves confirm's entry empty, so it falls back too. The
    // swap-on pass may see the empty entry first (it drives cancel there); the
    // warning names the entry either way.
    let capture = LogCapture::start();
    let table = table_for(&block(vec![command(
        "nav_confirm",
        None,
        Some(vec![bind("start")]),
    )]));
    capture.assert_logged_once(
        Level::Warn,
        "`nav_confirm` on `start` would leave `nav_menu`",
    );
    capture.assert_logged_once(Level::Warn, "leaves `nav_confirm` unbound on gamepad");
    assert_eq!(
        table.inputs(Command::NavMenu, PAD),
        vec![pad(gilrs::Button::Start)]
    );
    assert_eq!(
        table.inputs(Command::NavConfirm, PAD),
        vec![pad(gilrs::Button::South)]
    );
    assert!(table.guard_violations().is_empty());
}

// --- Unknown inputs and command IDs ---

#[test]
fn an_unknown_input_falls_back_for_that_entry_alone_and_unknown_commands_are_ignored() {
    let capture = LogCapture::start();
    let table = table_for(&block(vec![
        command(
            "dash",
            Some(vec![bind("KeyNope")]),
            Some(vec![bind("west")]),
        ),
        command("sprint", Some(vec![bind_with("KeyV", "hold", None)]), None),
        command("grapple", Some(vec![bind("KeyB")]), None),
        command("postretro.dev.dash", Some(vec![bind("KeyN")]), None),
    ]));
    capture.assert_logged_once(
        Level::Warn,
        "`dash` on keyboardMouse: `KeyNope` is not a known input",
    );
    capture.assert_logged_once(Level::Warn, "unknown command `grapple`");
    capture.assert_logged_once(Level::Warn, "unknown command `postretro.dev.dash`");
    assert_eq!(table.inputs(Command::Dash, KBM), vec![key(KeyCode::KeyF)]);
    assert_eq!(
        table.inputs(Command::Dash, PAD),
        vec![pad(gilrs::Button::West)]
    );
    assert_eq!(table.inputs(Command::Sprint, KBM), vec![key(KeyCode::KeyV)]);
    assert!(table.inputs(Command::Jump, KBM) == vec![key(KeyCode::Space)]);
}

// --- Fallbacks and conflicts between entries ---

#[test]
fn a_fallback_that_would_collide_unbinds_and_the_later_conflicting_entry_loses() {
    let capture = LogCapture::start();
    let table = table_for(&block(vec![
        command(
            "dash",
            Some(vec![bind_with("ShiftLeft", "tap", None)]),
            None,
        ),
        command("sprint", Some(vec![bind("ShiftLeftt")]), None),
    ]));
    capture.assert_logged_once(Level::Warn, "`sprint` on keyboardMouse: `ShiftLeftt`");
    capture.assert_logged_once(
        Level::Warn,
        "`sprint` on `ShiftLeft` conflicts with `dash`; `sprint` is unbound there",
    );
    assert!(table.inputs(Command::Sprint, KBM).is_empty());

    capture.clear();
    let table = table_for(&block(vec![
        command("reload", Some(vec![bind("KeyV")]), None),
        command("use", Some(vec![bind("KeyV")]), None),
    ]));
    capture.assert_logged_once(Level::Warn, "`use` on `KeyV` conflicts with `reload`");
    assert_eq!(table.inputs(Command::Reload, KBM), vec![key(KeyCode::KeyV)]);
    assert!(table.inputs(Command::Use, KBM).is_empty());
}

// --- Device classes left out or emptied ---

#[test]
fn a_keyboard_only_entry_keeps_gamepad_defaults_and_an_empty_gamepad_list_unbinds() {
    let table = table_for(&block(vec![command(
        "jump",
        Some(vec![bind("KeyJ")]),
        None,
    )]));
    assert_eq!(
        table.inputs(Command::Jump, PAD),
        vec![pad(gilrs::Button::South)]
    );
    let table = table_for(&block(vec![command(
        "jump",
        Some(vec![bind("KeyJ")]),
        Some(vec![]),
    )]));
    assert!(table.inputs(Command::Jump, PAD).is_empty());
}

// --- `show` on UI commands ---

#[test]
fn hiding_nav_confirm_is_diagnosed_and_it_stays_bound_and_listed() {
    let capture = LogCapture::start();
    let mut confirm = command("nav_confirm", None, None);
    confirm.show = Some(false);
    let table = table_for(&block(vec![confirm]));
    capture.assert_logged_once(Level::Warn, "`nav_confirm` cannot be shown or hidden");
    assert_eq!(table.relevance(Command::NavConfirm), Relevance::Relevant);
    assert!(!table.inputs(Command::NavConfirm, PAD).is_empty());
}

// --- Inputs that cannot drive the command ---

#[test]
fn a_key_on_an_analog_command_or_an_axis_on_a_digital_one_is_diagnosed() {
    let capture = LogCapture::start();
    let table = table_for(&block(vec![
        command("look_x", Some(vec![bind("KeyL")]), None),
        command("jump", None, Some(vec![bind("left_stick_y")])),
    ]));
    capture.assert_logged_once(Level::Warn, "`KeyL` cannot drive `look_x` on keyboardMouse");
    capture.assert_logged_once(Level::Warn, "`left_stick_y` cannot drive `jump` on gamepad");
    assert_eq!(
        table.inputs(Command::LookX, KBM),
        vec![PhysicalInput::MouseAxisX]
    );
    assert_eq!(
        table.inputs(Command::Jump, PAD),
        vec![pad(gilrs::Button::South)]
    );
}

#[test]
fn a_tap_max_past_the_hold_min_on_one_input_is_diagnosed() {
    let capture = LogCapture::start();
    let table = table_for(&block(vec![
        command(
            "dash",
            Some(vec![bind_with("ShiftLeft", "tap", Some(0.4))]),
            None,
        ),
        command(
            "sprint",
            Some(vec![bind_with("ShiftLeft", "hold", Some(0.3))]),
            None,
        ),
    ]));
    capture.assert_logged_once(
        Level::Warn,
        "`dash` on keyboardMouse taps `ShiftLeft` for up to 0.4s",
    );
    assert_eq!(table.inputs(Command::Dash, KBM), vec![key(KeyCode::KeyF)]);
}

#[test]
fn a_non_positive_threshold_or_an_unknown_activator_falls_back() {
    let capture = LogCapture::start();
    let table = table_for(&block(vec![
        command(
            "dash",
            Some(vec![bind_with("KeyV", "tap", Some(f64::NAN))]),
            None,
        ),
        command("use", Some(vec![bind_with("KeyB", "double", None)]), None),
    ]));
    capture.assert_logged_once(Level::Warn, "`threshold` must be a positive number");
    capture.assert_logged_once(Level::Warn, "activator `double` is unknown");
    assert_eq!(table.inputs(Command::Dash, KBM), vec![key(KeyCode::KeyF)]);
    assert_eq!(table.inputs(Command::Use, KBM), vec![key(KeyCode::KeyE)]);
}

// --- Hot reload ---

#[test]
fn a_hot_reload_of_the_block_recomputes_bindings_and_keeps_player_overrides() {
    let mut state = BindingState::default();
    let mut input = InputSystem::new(default_bindings());
    let sources = BindingSources {
        entity_types_generation: 1,
        tuning: None,
    };
    state.set_author_layer(author_layer_from_block(Some(&block(vec![command(
        "reload",
        Some(vec![bind("KeyV")]),
        None,
    )]))));
    let mut player = PlayerLayer::default();
    player
        .rows
        .insert((Command::Jump, KBM), vec![Some(key(KeyCode::KeyJ))]);
    state.set_player_layer(player);
    state.rebuild(sources, all_facts(), &mut input);
    assert_eq!(
        state.table().inputs(Command::Reload, KBM),
        vec![key(KeyCode::KeyV)]
    );

    state.set_author_layer(author_layer_from_block(Some(&block(vec![command(
        "reload",
        Some(vec![bind("KeyB")]),
        None,
    )]))));
    assert!(state.needs_rebuild(sources), "a changed block rebuilds");
    state.rebuild(sources, all_facts(), &mut input);
    assert_eq!(
        state.table().inputs(Command::Reload, KBM),
        vec![key(KeyCode::KeyB)]
    );
    assert_eq!(
        state.table().inputs(Command::Jump, KBM),
        vec![key(KeyCode::KeyJ)]
    );
}

#[test]
fn no_block_is_the_engine_table() {
    let none = author_layer_from_block(None);
    let table = EffectiveTable::build(&none, &PlayerLayer::default(), all_facts(), false);
    assert_eq!(table.inputs(Command::Dash, KBM), vec![key(KeyCode::KeyF)]);
    assert!(table.conflicting_pairs().is_empty());
}

// Regression: 1e-50 and 1e300 rounded to 0 and infinity as f32 and were
// refused as non-positive instead of clamped, and 0.01 s let a tap expire
// within one frame.
#[test]
fn a_threshold_out_of_range_clamps_to_a_twentieth_of_a_second_to_five_seconds() {
    let capture = LogCapture::start();
    let table = table_for(&block(vec![
        command(
            "dash",
            Some(vec![bind_with("KeyV", "tap", Some(1e-50))]),
            Some(vec![bind_with("west", "tap", Some(1e300))]),
        ),
        command(
            "use",
            Some(vec![bind_with("KeyB", "hold", Some(0.001))]),
            Some(vec![bind_with("north", "hold", Some(60.0))]),
        ),
    ]));
    capture.assert_not_logged(Level::Warn, "`threshold` must be a positive number");
    let threshold = |command: Command, input: PhysicalInput| {
        table
            .entries()
            .iter()
            .find(|e| e.command == command && e.input == input)
            .map(|e| e.activator.threshold)
            .expect("the clamped binding is kept")
    };
    for (command, input, expected) in [
        (Command::Dash, key(KeyCode::KeyV), 0.05),
        (Command::Dash, pad(gilrs::Button::West), 5.0),
        (Command::Use, key(KeyCode::KeyB), 0.05),
        (Command::Use, pad(gilrs::Button::North), 5.0),
    ] {
        let got = threshold(command, input);
        assert!((got - expected).abs() < 1e-6, "{input:?}: {got}");
    }
    capture.assert_logged_once(Level::Warn, "`dash` on keyboardMouse: `threshold`");
    capture.assert_logged_once(Level::Warn, "`dash` on gamepad: `threshold`");
    capture.assert_logged(Level::Warn, "is outside 0.05s..=5s; using 0.05s");
    capture.assert_logged(Level::Warn, "is outside 0.05s..=5s; using 5s");
    capture.assert_logged(
        Level::Warn,
        "`use` on keyboardMouse: `threshold` 0.001s is outside",
    );
}

#[test]
fn a_command_authored_twice_keeps_its_first_entry() {
    let capture = LogCapture::start();
    let table = table_for(&block(vec![
        command("dash", Some(vec![bind("KeyV")]), None),
        command("dash", Some(vec![bind("KeyB")]), None),
    ]));
    capture.assert_logged_once(
        Level::Warn,
        "`dash` appears more than once; keeping the first",
    );
    assert_eq!(table.inputs(Command::Dash, KBM), vec![key(KeyCode::KeyV)]);
}

#[test]
fn a_binding_that_is_not_an_object_is_named_as_such() {
    let capture = LogCapture::start();
    let table = table_for(&block(vec![command("dash", Some(vec![bind("")]), None)]));
    capture.assert_logged_once(
        Level::Warn,
        "`dash` on keyboardMouse: a binding must be an object with an `input` string",
    );
    assert_eq!(table.inputs(Command::Dash, KBM), vec![key(KeyCode::KeyF)]);
}
