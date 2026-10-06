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
    Dash,
    Crouch,
    Use,
    Drop,
    Shoot,
    AltFire,
    Reload,
    SelectWieldable1,
    SelectWieldable2,
    SelectWieldable3,
    SelectWieldable4,
    SelectWieldable5,
    SelectWieldable6,
    SelectWieldable7,
    SelectWieldable8,
    SelectWieldable9,
    SelectWieldable10,
    CycleWieldableNext,
    CycleWieldablePrevious,
    ToggleLastWieldable,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonState {
    /// Just activated this frame.
    Pressed,
    /// Still active (was Pressed or Held last frame, still active).
    Held,
    /// Just deactivated this frame.
    Released,
    /// Not active (was Released or Inactive last frame, still inactive).
    #[default]
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
    /// Momentary physical wheel event. Unlike mouse buttons this is cleared by
    /// the input system after every snapshot because the window API has no
    /// matching release event.
    MouseWheelUp,
    /// Momentary physical wheel event. See [`PhysicalInput::MouseWheelUp`].
    MouseWheelDown,
    MouseAxisX,
    MouseAxisY,
    GamepadButton(GilrsButton),
    GamepadAxis(GilrsAxis),
}

/// How a binding resolves its input's press and release into command phases.
/// Authors set it per binding; players rebind keys only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
// Release and Tap reach production bindings through the manifest input block.
#[cfg_attr(not(test), allow(dead_code))]
pub enum ActivatorKind {
    /// Fires on the press edge; the command stays down while the input is held.
    #[default]
    Press,
    /// Fires on key-up.
    Release,
    /// Fires on key-up when the input was held no longer than the threshold.
    Tap,
    /// Fires once the threshold elapses with the input still down; the command
    /// stays down until release.
    Hold,
}

/// Threshold a binding uses when its author sets none: a tap's max or a hold's
/// min, in seconds, before `hold_timing_scale`.
pub const DEFAULT_ACTIVATOR_THRESHOLD: f32 = 0.2;

/// A binding's activator kind and threshold. The threshold is a tap's max or a
/// hold's min in seconds; `press` and `release` ignore it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Activator {
    pub kind: ActivatorKind,
    pub threshold: f32,
}

impl Activator {
    pub const PRESS: Self = Self::new(ActivatorKind::Press);

    pub const fn new(kind: ActivatorKind) -> Self {
        Self {
            kind,
            threshold: DEFAULT_ACTIVATOR_THRESHOLD,
        }
    }

    #[allow(dead_code)]
    pub const fn with_threshold(kind: ActivatorKind, threshold: f32) -> Self {
        Self { kind, threshold }
    }
}

impl Default for Activator {
    fn default() -> Self {
        Self::PRESS
    }
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
    /// When the bound input drives the command. Defaults to `press`.
    pub activator: Activator,
}

impl Binding {
    pub fn new(input: PhysicalInput, action: Action) -> Self {
        Self {
            input,
            action,
            scale: 1.0,
            activator: Activator::PRESS,
        }
    }

    pub fn with_scale(input: PhysicalInput, action: Action, scale: f32) -> Self {
        Self {
            input,
            action,
            scale,
            activator: Activator::PRESS,
        }
    }

    #[allow(dead_code)]
    pub fn with_activator(mut self, activator: Activator) -> Self {
        self.activator = activator;
        self
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
        assert!(!Action::Dash.is_axis());
        assert!(!Action::Crouch.is_axis());
        assert!(!Action::Use.is_axis());
        assert!(!Action::Drop.is_axis());
        assert!(!Action::Shoot.is_axis());
        assert!(!Action::AltFire.is_axis());
        assert!(!Action::Reload.is_axis());
    }
}
