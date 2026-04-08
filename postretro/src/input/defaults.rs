// Default input bindings for keyboard/mouse and gamepad.
// See: context/lib/input.md §2

use gilrs::{Axis as GilrsAxis, Button as GilrsButton};
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

use crate::input::types::{Action, Binding, PhysicalInput};

/// Default keyboard and mouse bindings for all actions.
pub fn default_keyboard_mouse_bindings() -> Vec<Binding> {
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
        Binding::new(PhysicalInput::Key(KeyCode::KeyE), Action::Use),
        Binding::new(PhysicalInput::MouseButton(MouseButton::Left), Action::Shoot),
        Binding::new(
            PhysicalInput::MouseButton(MouseButton::Right),
            Action::AltFire,
        ),
        Binding::new(PhysicalInput::Key(KeyCode::KeyR), Action::Reload),
    ]
}

/// Default gamepad bindings for all actions.
pub fn default_gamepad_bindings() -> Vec<Binding> {
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
        Binding::new(PhysicalInput::GamepadButton(GilrsButton::West), Action::Use),
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
    ]
}

/// All default bindings (keyboard/mouse + gamepad combined).
pub fn default_bindings() -> Vec<Binding> {
    let mut bindings = default_keyboard_mouse_bindings();
    bindings.extend(default_gamepad_bindings());
    bindings
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// All Action variants, for exhaustive coverage checks.
    fn all_actions() -> Vec<Action> {
        vec![
            Action::MoveForward,
            Action::MoveRight,
            Action::MoveUp,
            Action::LookYaw,
            Action::LookPitch,
            Action::Sprint,
            Action::Jump,
            Action::Use,
            Action::Shoot,
            Action::AltFire,
            Action::Reload,
        ]
    }

    #[test]
    fn keyboard_mouse_bindings_cover_all_actions() {
        let bindings = default_keyboard_mouse_bindings();
        let bound_actions: HashSet<Action> = bindings.iter().map(|b| b.action).collect();
        for action in all_actions() {
            assert!(
                bound_actions.contains(&action),
                "Action {:?} has no keyboard/mouse binding",
                action,
            );
        }
    }

    #[test]
    fn gamepad_bindings_cover_all_actions() {
        let bindings = default_gamepad_bindings();
        let bound_actions: HashSet<Action> = bindings.iter().map(|b| b.action).collect();
        for action in all_actions() {
            assert!(
                bound_actions.contains(&action),
                "Action {:?} has no gamepad binding",
                action,
            );
        }
    }

    #[test]
    fn default_bindings_contain_both_keyboard_and_gamepad() {
        let combined = default_bindings();
        let kb = default_keyboard_mouse_bindings();
        let gp = default_gamepad_bindings();
        assert_eq!(combined.len(), kb.len() + gp.len());
    }

    #[test]
    fn axis_bindings_include_opposing_directions() {
        let bindings = default_keyboard_mouse_bindings();

        // MoveForward should have both +1.0 and -1.0 keyboard bindings.
        let forward_scales: Vec<f32> = bindings
            .iter()
            .filter(|b| b.action == Action::MoveForward)
            .filter(|b| matches!(b.input, PhysicalInput::Key(_)))
            .map(|b| b.scale)
            .collect();
        assert!(
            forward_scales.contains(&1.0),
            "MoveForward missing positive binding"
        );
        assert!(
            forward_scales.contains(&-1.0),
            "MoveForward missing negative binding"
        );

        // MoveRight should have both directions.
        let right_scales: Vec<f32> = bindings
            .iter()
            .filter(|b| b.action == Action::MoveRight)
            .filter(|b| matches!(b.input, PhysicalInput::Key(_)))
            .map(|b| b.scale)
            .collect();
        assert!(
            right_scales.contains(&1.0),
            "MoveRight missing positive binding"
        );
        assert!(
            right_scales.contains(&-1.0),
            "MoveRight missing negative binding"
        );

        // MoveUp should have both directions.
        let up_scales: Vec<f32> = bindings
            .iter()
            .filter(|b| b.action == Action::MoveUp)
            .filter(|b| matches!(b.input, PhysicalInput::Key(_)))
            .map(|b| b.scale)
            .collect();
        assert!(up_scales.contains(&1.0), "MoveUp missing positive binding");
        assert!(up_scales.contains(&-1.0), "MoveUp missing negative binding");
    }
}
