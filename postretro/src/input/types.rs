// Core input types: button state, axis values, physical inputs, bindings.
// See: context/lib/input.md

use gilrs::{Axis as GilrsAxis, Button as GilrsButton};
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

/// Logical actions the player can perform. Game logic reads these, never raw inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    MoveForward,
    MoveRight,
    MoveUp,
    LookYaw,
    LookPitch,
    Sprint,
    Jump,
    Use,
    Shoot,
    AltFire,
    Reload,
}

impl Action {
    /// Whether this action is inherently an axis (continuous value) rather than a button.
    pub fn is_axis(&self) -> bool {
        matches!(
            self,
            Action::MoveForward
                | Action::MoveRight
                | Action::MoveUp
                | Action::LookYaw
                | Action::LookPitch
        )
    }
}

/// Button state machine: tracks pressed/held/released/inactive transitions per frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    /// Just activated this frame.
    Pressed,
    /// Still active (was Pressed or Held last frame, still active).
    Held,
    /// Just deactivated this frame.
    Released,
    /// Not active (was Released or Inactive last frame, still inactive).
    Inactive,
}

impl ButtonState {
    /// Advance the button state given whether the input is currently active.
    pub fn advance(self, active: bool) -> ButtonState {
        match (self, active) {
            (ButtonState::Inactive, true) => ButtonState::Pressed,
            (ButtonState::Pressed, true) => ButtonState::Held,
            (ButtonState::Held, true) => ButtonState::Held,
            (ButtonState::Held, false) => ButtonState::Released,
            (ButtonState::Released, false) => ButtonState::Inactive,
            (ButtonState::Pressed, false) => ButtonState::Released,
            (ButtonState::Released, true) => ButtonState::Pressed,
            (ButtonState::Inactive, false) => ButtonState::Inactive,
        }
    }

    /// Whether this state counts as "active" (Pressed or Held).
    pub fn is_active(self) -> bool {
        matches!(self, ButtonState::Pressed | ButtonState::Held)
    }
}

impl Default for ButtonState {
    fn default() -> Self {
        ButtonState::Inactive
    }
}

/// Tags axis values with their source semantics for correct integration.
/// Mouse delta = displacement (apply directly), gamepad stick = velocity (multiply by dt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AxisSource {
    /// Raw displacement (e.g., mouse delta). Apply directly as rotation in radians.
    Displacement,
    /// Velocity (e.g., gamepad stick). Multiply by tick delta to get displacement.
    Velocity,
}

/// An axis value tagged with its source type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisValue {
    pub value: f32,
    pub source: AxisSource,
}

impl AxisValue {
    pub fn new(value: f32, source: AxisSource) -> Self {
        Self { value, source }
    }
}

/// A physical input device event that can be bound to an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhysicalInput {
    Key(KeyCode),
    MouseButton(MouseButton),
    MouseAxisX,
    MouseAxisY,
    GamepadButton(GilrsButton),
    GamepadAxis(GilrsAxis),
}

/// Maps a physical input to a logical action, with an optional scale factor.
/// Scale factor is used for axis direction: e.g., KeyS maps to MoveForward with scale -1.0.
#[derive(Debug, Clone, Copy)]
pub struct Binding {
    pub input: PhysicalInput,
    pub action: Action,
    /// Scale factor applied to the input value. Defaults to 1.0.
    /// For keyboard axis bindings, the key produces 1.0 * scale when pressed.
    pub scale: f32,
}

impl Binding {
    pub fn new(input: PhysicalInput, action: Action) -> Self {
        Self {
            input,
            action,
            scale: 1.0,
        }
    }

    pub fn with_scale(input: PhysicalInput, action: Action, scale: f32) -> Self {
        Self {
            input,
            action,
            scale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- ButtonState transitions ---

    #[test]
    fn button_state_advances_from_inactive_to_pressed_when_active() {
        assert_eq!(ButtonState::Inactive.advance(true), ButtonState::Pressed);
    }

    #[test]
    fn button_state_advances_from_pressed_to_held_when_active() {
        assert_eq!(ButtonState::Pressed.advance(true), ButtonState::Held);
    }

    #[test]
    fn button_state_stays_held_when_active() {
        assert_eq!(ButtonState::Held.advance(true), ButtonState::Held);
    }

    #[test]
    fn button_state_advances_from_held_to_released_when_inactive() {
        assert_eq!(ButtonState::Held.advance(false), ButtonState::Released);
    }

    #[test]
    fn button_state_advances_from_released_to_inactive_when_inactive() {
        assert_eq!(ButtonState::Released.advance(false), ButtonState::Inactive);
    }

    #[test]
    fn button_state_advances_from_pressed_to_released_when_inactive() {
        assert_eq!(ButtonState::Pressed.advance(false), ButtonState::Released);
    }

    #[test]
    fn button_state_advances_from_released_to_pressed_when_active() {
        assert_eq!(ButtonState::Released.advance(true), ButtonState::Pressed);
    }

    #[test]
    fn button_state_stays_inactive_when_inactive() {
        assert_eq!(ButtonState::Inactive.advance(false), ButtonState::Inactive);
    }

    // --- ButtonState::is_active ---

    #[test]
    fn button_state_is_active_returns_true_for_pressed_and_held() {
        assert!(ButtonState::Pressed.is_active());
        assert!(ButtonState::Held.is_active());
        assert!(!ButtonState::Released.is_active());
        assert!(!ButtonState::Inactive.is_active());
    }

    // --- Action::is_axis ---

    #[test]
    fn action_is_axis_returns_true_for_movement_and_look_actions() {
        assert!(Action::MoveForward.is_axis());
        assert!(Action::MoveRight.is_axis());
        assert!(Action::MoveUp.is_axis());
        assert!(Action::LookYaw.is_axis());
        assert!(Action::LookPitch.is_axis());
    }

    #[test]
    fn action_is_axis_returns_false_for_button_actions() {
        assert!(!Action::Sprint.is_axis());
        assert!(!Action::Jump.is_axis());
        assert!(!Action::Use.is_axis());
        assert!(!Action::Shoot.is_axis());
        assert!(!Action::AltFire.is_axis());
        assert!(!Action::Reload.is_axis());
    }
}
