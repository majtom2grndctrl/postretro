// Binding resolution: resolves physical input state into action values.
// See: context/lib/input.md §2

use std::collections::HashMap;

use crate::input::types::{Action, AxisSource, AxisValue, Binding, ButtonState, PhysicalInput};

/// Accumulated axis contributions for a single action, separated by source type.
#[derive(Debug, Default)]
struct AxisAccumulator {
    /// Best displacement value (highest magnitude wins within this source).
    displacement: f32,
    /// Best velocity value (highest magnitude wins within this source).
    velocity: f32,
}

/// Resolves bindings against current physical input state to produce action values.
///
/// Button resolution: logical OR across all bindings for an action.
/// Axis resolution: highest-magnitude wins within the same source type;
/// displacement and velocity sources are additive for look axes.
pub(crate) fn resolve_button_state(
    action: Action,
    bindings: &[Binding],
    key_state: &HashMap<PhysicalInput, bool>,
    prev_button_states: &HashMap<Action, ButtonState>,
) -> ButtonState {
    // OR across all bindings: if any bound input is active, the action is active.
    let any_active = bindings.iter().any(|b| {
        b.action == action && *key_state.get(&b.input).unwrap_or(&false)
    });

    let prev = prev_button_states
        .get(&action)
        .copied()
        .unwrap_or(ButtonState::Inactive);

    prev.advance(any_active)
}

/// Resolve axis values for an action from all matching bindings.
///
/// Returns a list of AxisValues. For look axes (LookYaw, LookPitch), displacement
/// and velocity are additive. For movement axes, highest-magnitude within the same
/// source wins and only velocity source is typical.
pub(crate) fn resolve_axis_values(
    action: Action,
    bindings: &[Binding],
    key_state: &HashMap<PhysicalInput, bool>,
    mouse_axes: &HashMap<Action, f32>,
    _gamepad_axes: &HashMap<Action, f32>,
) -> Vec<AxisValue> {
    let mut acc = AxisAccumulator::default();

    // Keyboard contributions (produce Velocity source with discrete -1/0/+1).
    for binding in bindings.iter().filter(|b| b.action == action) {
        match binding.input {
            PhysicalInput::Key(_) | PhysicalInput::MouseButton(_) => {
                let active = *key_state.get(&binding.input).unwrap_or(&false);
                if active {
                    let value = binding.scale; // key produces 1.0 * scale
                    if value.abs() > acc.velocity.abs() {
                        acc.velocity = value;
                    }
                }
            }
            PhysicalInput::MouseAxisX | PhysicalInput::MouseAxisY => {
                // Mouse axis contributions come from the mouse_axes map.
            }
            PhysicalInput::GamepadAxis(_) | PhysicalInput::GamepadButton(_) => {
                // Gamepad contributions come from gamepad_axes map (Task 04).
            }
        }
    }

    // Mouse axis contributions (Displacement source).
    if let Some(&mouse_val) = mouse_axes.get(&action) {
        if mouse_val.abs() > acc.displacement.abs() {
            acc.displacement = mouse_val;
        }
    }

    // Gamepad axis contributions (Velocity source) — Task 04 will fill this in.
    if let Some(&gamepad_val) = _gamepad_axes.get(&action) {
        // Gamepad is velocity source; pick highest magnitude between keyboard and gamepad.
        if gamepad_val.abs() > acc.velocity.abs() {
            acc.velocity = gamepad_val;
        }
    }

    // Build result: include non-zero sources.
    let mut result = Vec::new();
    if acc.displacement != 0.0 {
        result.push(AxisValue::new(acc.displacement, AxisSource::Displacement));
    }
    if acc.velocity != 0.0 {
        result.push(AxisValue::new(acc.velocity, AxisSource::Velocity));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::KeyCode;

    fn key_binding(key: KeyCode, action: Action) -> Binding {
        Binding::new(PhysicalInput::Key(key), action)
    }

    fn key_binding_scaled(key: KeyCode, action: Action, scale: f32) -> Binding {
        Binding::with_scale(PhysicalInput::Key(key), action, scale)
    }

    // --- Button resolution ---

    #[test]
    fn resolve_button_or_across_multiple_bindings_activates_when_any_pressed() {
        let bindings = vec![
            key_binding(KeyCode::Space, Action::Jump),
            key_binding(KeyCode::KeyC, Action::Jump),
        ];
        let mut key_state = HashMap::new();
        key_state.insert(PhysicalInput::Key(KeyCode::Space), false);
        key_state.insert(PhysicalInput::Key(KeyCode::KeyC), true);

        let state = resolve_button_state(
            Action::Jump,
            &bindings,
            &key_state,
            &HashMap::new(),
        );
        assert_eq!(state, ButtonState::Pressed);
    }

    #[test]
    fn resolve_button_stays_inactive_when_no_bindings_active() {
        let bindings = vec![key_binding(KeyCode::Space, Action::Jump)];
        let mut key_state = HashMap::new();
        key_state.insert(PhysicalInput::Key(KeyCode::Space), false);

        let state = resolve_button_state(
            Action::Jump,
            &bindings,
            &key_state,
            &HashMap::new(),
        );
        assert_eq!(state, ButtonState::Inactive);
    }

    #[test]
    fn resolve_button_transitions_through_full_lifecycle() {
        let bindings = vec![key_binding(KeyCode::Space, Action::Jump)];

        let mut key_state = HashMap::new();
        let mut prev = HashMap::new();

        // Frame 1: press
        key_state.insert(PhysicalInput::Key(KeyCode::Space), true);
        let state = resolve_button_state(Action::Jump, &bindings, &key_state, &prev);
        assert_eq!(state, ButtonState::Pressed);
        prev.insert(Action::Jump, state);

        // Frame 2: hold
        let state = resolve_button_state(Action::Jump, &bindings, &key_state, &prev);
        assert_eq!(state, ButtonState::Held);
        prev.insert(Action::Jump, state);

        // Frame 3: release
        key_state.insert(PhysicalInput::Key(KeyCode::Space), false);
        let state = resolve_button_state(Action::Jump, &bindings, &key_state, &prev);
        assert_eq!(state, ButtonState::Released);
        prev.insert(Action::Jump, state);

        // Frame 4: inactive
        let state = resolve_button_state(Action::Jump, &bindings, &key_state, &prev);
        assert_eq!(state, ButtonState::Inactive);
    }

    // --- Axis resolution ---

    #[test]
    fn resolve_axis_keyboard_produces_scaled_velocity_value() {
        let bindings = vec![
            key_binding_scaled(KeyCode::KeyW, Action::MoveForward, 1.0),
            key_binding_scaled(KeyCode::KeyS, Action::MoveForward, -1.0),
        ];
        let mut key_state = HashMap::new();
        key_state.insert(PhysicalInput::Key(KeyCode::KeyW), true);
        key_state.insert(PhysicalInput::Key(KeyCode::KeyS), false);

        let values = resolve_axis_values(
            Action::MoveForward,
            &bindings,
            &key_state,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].source, AxisSource::Velocity);
        assert!((values[0].value - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn resolve_axis_highest_magnitude_wins_within_same_source() {
        // Both W and S pressed: W=+1.0, S=-1.0. Highest magnitude is 1.0 (tied).
        // The first one encountered with highest magnitude wins.
        let bindings = vec![
            key_binding_scaled(KeyCode::KeyW, Action::MoveForward, 1.0),
            key_binding_scaled(KeyCode::KeyS, Action::MoveForward, -1.0),
        ];
        let mut key_state = HashMap::new();
        key_state.insert(PhysicalInput::Key(KeyCode::KeyW), true);
        key_state.insert(PhysicalInput::Key(KeyCode::KeyS), true);

        let values = resolve_axis_values(
            Action::MoveForward,
            &bindings,
            &key_state,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(values.len(), 1);
        // Both have magnitude 1.0; W is encountered first so it wins.
        assert!((values[0].value - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn resolve_axis_mouse_displacement_and_keyboard_velocity_are_additive() {
        let bindings = vec![
            key_binding_scaled(KeyCode::KeyA, Action::LookYaw, -1.0),
        ];
        let mut key_state = HashMap::new();
        key_state.insert(PhysicalInput::Key(KeyCode::KeyA), true);

        let mut mouse_axes = HashMap::new();
        mouse_axes.insert(Action::LookYaw, 0.05_f32);

        let values = resolve_axis_values(
            Action::LookYaw,
            &bindings,
            &key_state,
            &mouse_axes,
            &HashMap::new(),
        );
        // Should have both displacement (mouse) and velocity (keyboard).
        assert_eq!(values.len(), 2);
        let displacement = values.iter().find(|v| v.source == AxisSource::Displacement).unwrap();
        let velocity = values.iter().find(|v| v.source == AxisSource::Velocity).unwrap();
        assert!((displacement.value - 0.05).abs() < f32::EPSILON);
        assert!((velocity.value - (-1.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn resolve_axis_returns_empty_when_no_input_active() {
        let bindings = vec![
            key_binding_scaled(KeyCode::KeyW, Action::MoveForward, 1.0),
        ];
        let key_state = HashMap::new();

        let values = resolve_axis_values(
            Action::MoveForward,
            &bindings,
            &key_state,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(values.is_empty());
    }

    #[test]
    fn resolve_axis_gamepad_velocity_wins_over_keyboard_when_higher_magnitude() {
        let bindings = vec![
            key_binding_scaled(KeyCode::KeyW, Action::MoveForward, 1.0),
        ];
        let mut key_state = HashMap::new();
        key_state.insert(PhysicalInput::Key(KeyCode::KeyW), true);

        let mut gamepad_axes = HashMap::new();
        // Gamepad reports -0.5 velocity, keyboard reports +1.0. Keyboard wins (higher magnitude).
        gamepad_axes.insert(Action::MoveForward, -0.5_f32);

        let values = resolve_axis_values(
            Action::MoveForward,
            &bindings,
            &key_state,
            &HashMap::new(),
            &gamepad_axes,
        );
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].source, AxisSource::Velocity);
        // Keyboard magnitude (1.0) > gamepad magnitude (0.5), keyboard wins.
        assert!((values[0].value - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn resolve_axis_gamepad_velocity_wins_over_keyboard_when_keyboard_inactive() {
        let bindings = vec![
            key_binding_scaled(KeyCode::KeyW, Action::MoveForward, 1.0),
        ];
        let key_state = HashMap::new(); // No keys pressed.

        let mut gamepad_axes = HashMap::new();
        gamepad_axes.insert(Action::MoveForward, 0.75_f32);

        let values = resolve_axis_values(
            Action::MoveForward,
            &bindings,
            &key_state,
            &HashMap::new(),
            &gamepad_axes,
        );
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].source, AxisSource::Velocity);
        assert!((values[0].value - 0.75).abs() < f32::EPSILON);
    }
}
