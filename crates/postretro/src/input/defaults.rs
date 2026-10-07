// Engine default bindings, per command and device class.
// See: context/lib/input.md §2

use gilrs::{Axis as GilrsAxis, Button as GilrsButton};
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

use super::commands::Command;
use super::input_names::{DeviceClass, analog_polarity, is_whole_axis};
use super::types::{Activator, AxisHalf, Binding, PhysicalInput};

const fn key(code: KeyCode) -> PhysicalInput {
    PhysicalInput::Key(code)
}

const fn mouse(button: MouseButton) -> PhysicalInput {
    PhysicalInput::MouseButton(button)
}

const fn pad(button: GilrsButton) -> PhysicalInput {
    PhysicalInput::GamepadButton(button)
}

const fn stick(axis: GilrsAxis) -> PhysicalInput {
    PhysicalInput::GamepadAxis(axis)
}

const fn half(axis: GilrsAxis, half: AxisHalf) -> PhysicalInput {
    PhysicalInput::GamepadAxisHalf(axis, half)
}

const NONE: &[PhysicalInput] = &[];

/// The engine's default inputs for a command on a device class: the last
/// layer under author defaults and player overrides.
pub fn engine_default_inputs(command: Command, class: DeviceClass) -> &'static [PhysicalInput] {
    let (keyboard_mouse, gamepad): (&[PhysicalInput], &[PhysicalInput]) = match command {
        Command::MoveForward => (
            const { &[key(KeyCode::KeyW)] },
            const { &[half(GilrsAxis::LeftStickY, AxisHalf::Positive)] },
        ),
        Command::MoveBack => (
            const { &[key(KeyCode::KeyS)] },
            const { &[half(GilrsAxis::LeftStickY, AxisHalf::Negative)] },
        ),
        Command::MoveLeft => (
            const { &[key(KeyCode::KeyA)] },
            const { &[half(GilrsAxis::LeftStickX, AxisHalf::Negative)] },
        ),
        Command::MoveRight => (
            const { &[key(KeyCode::KeyD)] },
            const { &[half(GilrsAxis::LeftStickX, AxisHalf::Positive)] },
        ),
        Command::MoveUp => (
            const { &[key(KeyCode::KeyQ)] },
            const { &[pad(GilrsButton::DPadUp)] },
        ),
        Command::MoveDown => (
            const { &[key(KeyCode::KeyZ)] },
            const { &[pad(GilrsButton::DPadDown)] },
        ),
        Command::LookX => (
            const { &[PhysicalInput::MouseAxisX] },
            const { &[stick(GilrsAxis::RightStickX)] },
        ),
        Command::LookY => (
            const { &[PhysicalInput::MouseAxisY] },
            const { &[stick(GilrsAxis::RightStickY)] },
        ),
        Command::Sprint => (
            const { &[key(KeyCode::ShiftLeft)] },
            const { &[pad(GilrsButton::LeftThumb)] },
        ),
        Command::Jump => (
            const { &[key(KeyCode::Space)] },
            const { &[pad(GilrsButton::South)] },
        ),
        Command::Dash => (
            const { &[key(KeyCode::KeyF)] },
            const { &[pad(GilrsButton::East)] },
        ),
        Command::Crouch => (
            const { &[key(KeyCode::KeyC)] },
            const { &[pad(GilrsButton::RightThumb)] },
        ),
        Command::Use => (
            const { &[key(KeyCode::KeyE)] },
            const { &[pad(GilrsButton::West)] },
        ),
        Command::Drop => (
            const { &[key(KeyCode::KeyG)] },
            const { &[pad(GilrsButton::Select)] },
        ),
        Command::Shoot => (
            const { &[mouse(MouseButton::Left)] },
            const { &[pad(GilrsButton::RightTrigger2)] },
        ),
        Command::AltFire => (
            const { &[mouse(MouseButton::Right)] },
            const { &[pad(GilrsButton::LeftTrigger2)] },
        ),
        Command::Reload => (
            const { &[key(KeyCode::KeyR)] },
            const { &[pad(GilrsButton::North)] },
        ),
        Command::SelectWieldable1 => (const { &[key(KeyCode::Digit1)] }, NONE),
        Command::SelectWieldable2 => (const { &[key(KeyCode::Digit2)] }, NONE),
        Command::SelectWieldable3 => (const { &[key(KeyCode::Digit3)] }, NONE),
        Command::SelectWieldable4 => (const { &[key(KeyCode::Digit4)] }, NONE),
        Command::SelectWieldable5 => (const { &[key(KeyCode::Digit5)] }, NONE),
        Command::SelectWieldable6 => (const { &[key(KeyCode::Digit6)] }, NONE),
        Command::SelectWieldable7 => (const { &[key(KeyCode::Digit7)] }, NONE),
        Command::SelectWieldable8 => (const { &[key(KeyCode::Digit8)] }, NONE),
        Command::SelectWieldable9 => (const { &[key(KeyCode::Digit9)] }, NONE),
        Command::SelectWieldable10 => (const { &[key(KeyCode::Digit0)] }, NONE),
        Command::CycleWieldableNext => (
            const { &[PhysicalInput::MouseWheelDown] },
            const { &[pad(GilrsButton::DPadRight)] },
        ),
        Command::CycleWieldablePrevious => (
            const { &[PhysicalInput::MouseWheelUp] },
            const { &[pad(GilrsButton::DPadLeft)] },
        ),
        Command::ToggleLastWieldable => (
            const { &[key(KeyCode::KeyX)] },
            const { &[pad(GilrsButton::LeftTrigger)] },
        ),
        Command::NavUp => (
            const { &[key(KeyCode::ArrowUp)] },
            const {
                &[
                    pad(GilrsButton::DPadUp),
                    half(GilrsAxis::LeftStickY, AxisHalf::Positive),
                ]
            },
        ),
        Command::NavDown => (
            const { &[key(KeyCode::ArrowDown)] },
            const {
                &[
                    pad(GilrsButton::DPadDown),
                    half(GilrsAxis::LeftStickY, AxisHalf::Negative),
                ]
            },
        ),
        Command::NavLeft => (
            const { &[key(KeyCode::ArrowLeft)] },
            const {
                &[
                    pad(GilrsButton::DPadLeft),
                    half(GilrsAxis::LeftStickX, AxisHalf::Negative),
                ]
            },
        ),
        Command::NavRight => (
            const { &[key(KeyCode::ArrowRight)] },
            const {
                &[
                    pad(GilrsButton::DPadRight),
                    half(GilrsAxis::LeftStickX, AxisHalf::Positive),
                ]
            },
        ),
        Command::NavNext => (const { &[key(KeyCode::Tab)] }, NONE),
        Command::NavPrev => (NONE, NONE),
        // The bumpers are tab keys; in a tree with no tablist they step
        // Next/Prev, so `nav_next`/`nav_prev` need no gamepad binding.
        Command::NavTabNext => (
            const { &[key(KeyCode::KeyE)] },
            const { &[pad(GilrsButton::RightTrigger)] },
        ),
        Command::NavTabPrev => (
            const { &[key(KeyCode::KeyQ)] },
            const { &[pad(GilrsButton::LeftTrigger)] },
        ),
        Command::NavConfirm => (
            const { &[key(KeyCode::Enter), key(KeyCode::NumpadEnter)] },
            const { &[pad(GilrsButton::South)] },
        ),
        Command::NavCancel => (
            const { &[key(KeyCode::Escape)] },
            const { &[pad(GilrsButton::East)] },
        ),
        Command::NavMenu => (
            const { &[key(KeyCode::Escape)] },
            const { &[pad(GilrsButton::Start)] },
        ),
        Command::NavOptions => (NONE, const { &[pad(GilrsButton::Select)] }),
        // Hardware Backspace, Space, and Enter already edit text and stay fixed.
        Command::TextBackspace => (NONE, const { &[pad(GilrsButton::West)] }),
        Command::TextSpace => (NONE, const { &[pad(GilrsButton::North)] }),
        Command::TextCommit => (NONE, const { &[pad(GilrsButton::Start)] }),
    };
    match class {
        DeviceClass::KeyboardMouse => keyboard_mouse,
        DeviceClass::Gamepad => gamepad,
    }
}

/// The input-system binding for a command on one input, or `None` for a UI
/// command (UI nav reads its own table). A whole axis takes its source's
/// physical polarity; a half-axis is already a physical direction.
pub fn gameplay_binding(
    command: Command,
    input: PhysicalInput,
    activator: Activator,
) -> Option<Binding> {
    let (action, sign) = command.gameplay_target()?;
    let polarity = if is_whole_axis(input) {
        analog_polarity(input)
    } else {
        1.0
    };
    Some(Binding::with_scale(input, action, sign * polarity).with_activator(activator))
}

/// Engine default gameplay bindings on one device class, every one `press`.
pub fn default_bindings_for(class: DeviceClass) -> Vec<Binding> {
    Command::ALL
        .into_iter()
        .flat_map(|command| {
            engine_default_inputs(command, class)
                .iter()
                .filter_map(move |input| gameplay_binding(command, *input, Activator::PRESS))
        })
        .collect()
}

/// All engine default gameplay bindings (keyboard/mouse, then gamepad).
pub fn default_bindings() -> Vec<Binding> {
    DeviceClass::ALL
        .into_iter()
        .flat_map(default_bindings_for)
        .collect()
}

#[cfg(test)]
#[path = "defaults_tests.rs"]
mod tests;
#[cfg(test)]
pub(super) use tests::legacy_actions;
