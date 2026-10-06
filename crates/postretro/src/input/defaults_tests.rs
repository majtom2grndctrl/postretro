// Engine default table tests, against the pre-command default tables.
// The legacy tables below are the "today" reference for AL1: every default key,
// mouse, and button input must keep its action reads.

use std::collections::HashSet;

use gilrs::{Axis as GilrsAxis, Button as GilrsButton};
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

use super::*;
use crate::input::InputSystem;
use crate::input::commands::{Command, CommandContext};
use crate::input::input_names::DeviceClass;
use crate::input::types::{Action, AxisSource, Binding, PhysicalInput};

/// Every action the pre-command defaults bound, on either device.
pub(in crate::input) fn legacy_actions() -> Vec<Action> {
    let mut seen = HashSet::new();
    legacy_keyboard_mouse_bindings()
        .into_iter()
        .chain(legacy_gamepad_bindings())
        .map(|binding| binding.action)
        .filter(|action| seen.insert(*action))
        .collect()
}

fn legacy_keyboard_mouse_bindings() -> Vec<Binding> {
    vec![
        // Movement axes
        Binding::with_scale(PhysicalInput::Key(KeyCode::KeyW), Action::MoveForward, 1.0),
        Binding::with_scale(PhysicalInput::Key(KeyCode::KeyS), Action::MoveForward, -1.0),
        Binding::with_scale(PhysicalInput::Key(KeyCode::KeyD), Action::MoveRight, 1.0),
        Binding::with_scale(PhysicalInput::Key(KeyCode::KeyA), Action::MoveRight, -1.0),
        Binding::with_scale(PhysicalInput::Key(KeyCode::KeyQ), Action::MoveUp, 1.0),
        Binding::with_scale(PhysicalInput::Key(KeyCode::KeyZ), Action::MoveUp, -1.0),
        // Look axes (scale -1.0 for natural direction)
        Binding::with_scale(PhysicalInput::MouseAxisX, Action::LookYaw, -1.0),
        Binding::with_scale(PhysicalInput::MouseAxisY, Action::LookPitch, -1.0),
        // Button actions
        Binding::new(PhysicalInput::Key(KeyCode::ShiftLeft), Action::Sprint),
        Binding::new(PhysicalInput::Key(KeyCode::Space), Action::Jump),
        Binding::new(PhysicalInput::Key(KeyCode::KeyF), Action::Dash),
        Binding::new(PhysicalInput::Key(KeyCode::KeyC), Action::Crouch),
        Binding::new(PhysicalInput::Key(KeyCode::KeyE), Action::Use),
        Binding::new(PhysicalInput::Key(KeyCode::KeyG), Action::Drop),
        Binding::new(PhysicalInput::MouseButton(MouseButton::Left), Action::Shoot),
        Binding::new(
            PhysicalInput::MouseButton(MouseButton::Right),
            Action::AltFire,
        ),
        Binding::new(PhysicalInput::Key(KeyCode::KeyR), Action::Reload),
        Binding::new(
            PhysicalInput::Key(KeyCode::Digit1),
            Action::SelectWieldable1,
        ),
        Binding::new(
            PhysicalInput::Key(KeyCode::Digit2),
            Action::SelectWieldable2,
        ),
        Binding::new(
            PhysicalInput::Key(KeyCode::Digit3),
            Action::SelectWieldable3,
        ),
        Binding::new(
            PhysicalInput::Key(KeyCode::Digit4),
            Action::SelectWieldable4,
        ),
        Binding::new(
            PhysicalInput::Key(KeyCode::Digit5),
            Action::SelectWieldable5,
        ),
        Binding::new(
            PhysicalInput::Key(KeyCode::Digit6),
            Action::SelectWieldable6,
        ),
        Binding::new(
            PhysicalInput::Key(KeyCode::Digit7),
            Action::SelectWieldable7,
        ),
        Binding::new(
            PhysicalInput::Key(KeyCode::Digit8),
            Action::SelectWieldable8,
        ),
        Binding::new(
            PhysicalInput::Key(KeyCode::Digit9),
            Action::SelectWieldable9,
        ),
        Binding::new(
            PhysicalInput::Key(KeyCode::Digit0),
            Action::SelectWieldable10,
        ),
        Binding::new(PhysicalInput::MouseWheelDown, Action::CycleWieldableNext),
        Binding::new(PhysicalInput::MouseWheelUp, Action::CycleWieldablePrevious),
        Binding::new(
            PhysicalInput::Key(KeyCode::KeyX),
            Action::ToggleLastWieldable,
        ),
    ]
}

/// The gamepad defaults before commands existed.
fn legacy_gamepad_bindings() -> Vec<Binding> {
    vec![
        // Movement axes
        Binding::with_scale(
            PhysicalInput::GamepadAxis(GilrsAxis::LeftStickY),
            Action::MoveForward,
            -1.0,
        ),
        Binding::with_scale(
            PhysicalInput::GamepadAxis(GilrsAxis::LeftStickX),
            Action::MoveRight,
            1.0,
        ),
        Binding::with_scale(
            PhysicalInput::GamepadButton(GilrsButton::DPadUp),
            Action::MoveUp,
            1.0,
        ),
        Binding::with_scale(
            PhysicalInput::GamepadButton(GilrsButton::DPadDown),
            Action::MoveUp,
            -1.0,
        ),
        // Look axes
        Binding::with_scale(
            PhysicalInput::GamepadAxis(GilrsAxis::RightStickX),
            Action::LookYaw,
            1.0,
        ),
        Binding::with_scale(
            PhysicalInput::GamepadAxis(GilrsAxis::RightStickY),
            Action::LookPitch,
            -1.0,
        ),
        // Button actions
        Binding::new(
            PhysicalInput::GamepadButton(GilrsButton::LeftThumb),
            Action::Sprint,
        ),
        Binding::new(
            PhysicalInput::GamepadButton(GilrsButton::South),
            Action::Jump,
        ),
        Binding::new(
            PhysicalInput::GamepadButton(GilrsButton::East),
            Action::Dash,
        ),
        Binding::new(
            PhysicalInput::GamepadButton(GilrsButton::RightThumb),
            Action::Crouch,
        ),
        Binding::new(PhysicalInput::GamepadButton(GilrsButton::West), Action::Use),
        Binding::new(
            PhysicalInput::GamepadButton(GilrsButton::Select),
            Action::Drop,
        ),
        Binding::new(
            PhysicalInput::GamepadButton(GilrsButton::RightTrigger2),
            Action::Shoot,
        ),
        Binding::new(
            PhysicalInput::GamepadButton(GilrsButton::LeftTrigger2),
            Action::AltFire,
        ),
        Binding::new(
            PhysicalInput::GamepadButton(GilrsButton::North),
            Action::Reload,
        ),
        Binding::new(
            PhysicalInput::GamepadButton(GilrsButton::DPadRight),
            Action::CycleWieldableNext,
        ),
        Binding::new(
            PhysicalInput::GamepadButton(GilrsButton::DPadLeft),
            Action::CycleWieldablePrevious,
        ),
        Binding::new(
            PhysicalInput::GamepadButton(GilrsButton::LeftTrigger),
            Action::ToggleLastWieldable,
        ),
    ]
}

fn is_stick_axis(input: PhysicalInput) -> bool {
    matches!(input, PhysicalInput::GamepadAxis(_))
}

#[test]
fn engine_defaults_keep_every_legacy_key_mouse_and_button_read() {
    let current = default_bindings();
    let legacy: Vec<Binding> = legacy_keyboard_mouse_bindings()
        .into_iter()
        .chain(legacy_gamepad_bindings())
        .filter(|binding| !is_stick_axis(binding.input))
        .collect();
    for old in &legacy {
        assert!(
            current.iter().any(|new| new.input == old.input
                && new.action == old.action
                && new.scale == old.scale),
            "{:?} -> {:?} x{} is no longer bound",
            old.input,
            old.action,
            old.scale
        );
    }
    let current_non_stick = current
        .iter()
        .filter(|binding| !matches!(binding.input, PhysicalInput::GamepadAxisHalf(..)))
        .filter(|binding| !is_stick_axis(binding.input))
        .count();
    assert_eq!(
        current_non_stick,
        legacy.len(),
        "no extra key or button reads"
    );
}

/// Snapshot axis value with one stick deflected.
fn stick_axis_value(axis: GilrsAxis, raw: f32, action: Action) -> f32 {
    let mut sys = InputSystem::new(default_bindings());
    sys.set_gamepad_axis(axis, raw);
    sys.snapshot().axis_value(action)
}

fn key_axis_value(code: KeyCode, action: Action) -> f32 {
    let mut sys = InputSystem::new(default_bindings());
    sys.handle_keyboard_event(code, true);
    sys.snapshot().axis_value(action)
}

#[test]
fn pad_sticks_move_in_the_keyboard_directions() {
    // Regression: the stick defaults were inverted against keyboard and mouse;
    // pushing the left stick up walked backward.
    let forward = stick_axis_value(GilrsAxis::LeftStickY, 0.8, Action::MoveForward);
    assert!((forward - 0.8).abs() < 1e-6);
    assert_eq!(
        forward.signum(),
        key_axis_value(KeyCode::KeyW, Action::MoveForward).signum()
    );
    let back = stick_axis_value(GilrsAxis::LeftStickY, -0.5, Action::MoveForward);
    assert_eq!(
        back.signum(),
        key_axis_value(KeyCode::KeyS, Action::MoveForward).signum()
    );
    let right = stick_axis_value(GilrsAxis::LeftStickX, 0.6, Action::MoveRight);
    assert_eq!(
        right.signum(),
        key_axis_value(KeyCode::KeyD, Action::MoveRight).signum()
    );
}

#[test]
fn pad_look_turns_and_pitches_in_the_mouse_directions() {
    let mut mouse = InputSystem::new(default_bindings());
    mouse.handle_mouse_delta(10.0, 0.0); // mouse right
    let mouse_right = mouse.drain_look_inputs().yaw_displacement;
    let mut mouse = InputSystem::new(default_bindings());
    mouse.handle_mouse_delta(0.0, -10.0); // mouse up (the OS reports up as -)
    let mouse_up = mouse.drain_look_inputs().pitch_displacement;

    let mut pad = InputSystem::new(default_bindings());
    pad.set_gamepad_axis(GilrsAxis::RightStickX, 0.7); // stick right
    pad.set_gamepad_axis(GilrsAxis::RightStickY, 0.4); // stick up
    let look = pad.drain_look_inputs();
    assert_eq!(look.yaw_velocity.signum(), mouse_right.signum());
    assert_eq!(look.pitch_velocity.signum(), mouse_up.signum());
    assert!((look.yaw_velocity.abs() - 0.7).abs() < 1e-6);
    assert!((look.pitch_velocity.abs() - 0.4).abs() < 1e-6);
}

#[test]
fn stick_look_is_velocity_and_mouse_look_is_displacement() {
    let mut sys = InputSystem::new(default_bindings());
    sys.set_gamepad_axis(GilrsAxis::RightStickX, 0.5);
    let snap = sys.snapshot();
    assert!(
        snap.axis(Action::LookYaw)
            .iter()
            .all(|value| value.source == AxisSource::Velocity)
    );
}

#[test]
fn engine_defaults_bind_every_gameplay_command_but_pad_wieldable_slots() {
    for command in Command::ALL {
        if command.context() != CommandContext::Gameplay {
            continue;
        }
        assert!(
            !engine_default_inputs(command, DeviceClass::KeyboardMouse).is_empty(),
            "{} has no keyboard/mouse default",
            command.id()
        );
        let is_slot = command.id().starts_with("select_wieldable_");
        assert_eq!(
            engine_default_inputs(command, DeviceClass::Gamepad).is_empty(),
            is_slot,
            "{} gamepad default",
            command.id()
        );
    }
}

#[test]
fn engine_default_inputs_belong_to_their_device_class() {
    for command in Command::ALL {
        for class in DeviceClass::ALL {
            for input in engine_default_inputs(command, class) {
                assert_eq!(DeviceClass::of(*input), class, "{}", command.id());
            }
        }
    }
}

#[test]
fn drop_defaults_bind_key_g_and_gamepad_select() {
    assert!(default_bindings().iter().any(|binding| {
        binding.input == PhysicalInput::Key(KeyCode::KeyG) && binding.action == Action::Drop
    }));
    assert!(default_bindings().iter().any(|binding| {
        binding.input == PhysicalInput::GamepadButton(GilrsButton::Select)
            && binding.action == Action::Drop
    }));
}

#[test]
fn default_bindings_for_splits_by_device_class() {
    let keyboard_mouse = default_bindings_for(DeviceClass::KeyboardMouse);
    assert!(
        keyboard_mouse
            .iter()
            .any(|binding| binding.input == PhysicalInput::MouseButton(MouseButton::Left))
    );
    assert!(
        keyboard_mouse
            .iter()
            .all(|binding| DeviceClass::of(binding.input) == DeviceClass::KeyboardMouse)
    );
    assert_eq!(
        default_bindings().len(),
        keyboard_mouse.len() + default_bindings_for(DeviceClass::Gamepad).len()
    );
}
