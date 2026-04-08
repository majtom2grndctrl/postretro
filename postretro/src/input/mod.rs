// Input subsystem: action mapping, binding resolution, per-frame snapshots.
// See: context/lib/input.md

mod bindings;
pub mod cursor;
mod types;

pub use types::{Action, AxisSource, AxisValue, Binding, ButtonState, PhysicalInput};

/// Default sensitivity: radians per raw mouse unit. Tuned for 800 DPI mice.
pub const DEFAULT_MOUSE_SENSITIVITY: f32 = 0.002;

use std::collections::HashMap;

use winit::event::MouseButton;
use winit::keyboard::KeyCode;

/// Read-only snapshot of all action states for a single frame.
/// Game logic consumes this; nothing writes back to input mid-frame.
#[derive(Debug, Clone)]
pub struct ActionSnapshot {
    button_states: HashMap<Action, ButtonState>,
    axis_values: HashMap<Action, Vec<AxisValue>>,
}

impl ActionSnapshot {
    /// Query the button state for an action. Returns Inactive if unbound.
    pub fn button(&self, action: Action) -> ButtonState {
        self.button_states
            .get(&action)
            .copied()
            .unwrap_or(ButtonState::Inactive)
    }

    /// Query axis values for an action. Returns empty slice if no input active.
    /// Multiple values indicate additive sources (displacement + velocity).
    pub fn axis(&self, action: Action) -> &[AxisValue] {
        self.axis_values
            .get(&action)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Convenience: sum of all axis values for an action, regardless of source.
    /// Useful when you don't need to distinguish displacement from velocity.
    pub fn axis_value(&self, action: Action) -> f32 {
        self.axis(action).iter().map(|v| v.value).sum()
    }
}

/// The input subsystem. Collects raw input events and resolves them into action snapshots.
pub struct InputSystem {
    bindings: Vec<Binding>,

    /// Current pressed/released state of each physical input (true = active).
    physical_state: HashMap<PhysicalInput, bool>,

    /// Button states from the previous snapshot, used for state machine transitions.
    prev_button_states: HashMap<Action, ButtonState>,

    /// Accumulated mouse delta since last snapshot. Reset after each snapshot.
    mouse_delta: (f64, f64),

    /// Mouse axis values resolved from accumulated delta for the current frame.
    mouse_axes: HashMap<Action, f32>,

    /// Gamepad axis values for the current frame (Task 04 populates this).
    gamepad_axes: HashMap<Action, f32>,

    /// Radians per raw mouse unit. Converts OS mouse units to look rotation.
    mouse_sensitivity: f32,

    /// When true, negate the Y (pitch) mouse axis. Applied after sensitivity.
    invert_y: bool,
}

impl InputSystem {
    pub fn new(bindings: Vec<Binding>) -> Self {
        Self {
            bindings,
            physical_state: HashMap::new(),
            prev_button_states: HashMap::new(),
            mouse_delta: (0.0, 0.0),
            mouse_axes: HashMap::new(),
            gamepad_axes: HashMap::new(),
            mouse_sensitivity: DEFAULT_MOUSE_SENSITIVITY,
            invert_y: false,
        }
    }

    /// Set mouse sensitivity (radians per raw mouse unit).
    pub fn set_mouse_sensitivity(&mut self, sensitivity: f32) {
        self.mouse_sensitivity = sensitivity;
    }

    /// Get the current mouse sensitivity.
    pub fn mouse_sensitivity(&self) -> f32 {
        self.mouse_sensitivity
    }

    /// Enable or disable invert-Y for mouse look.
    pub fn set_invert_y(&mut self, invert: bool) {
        self.invert_y = invert;
    }

    /// Whether invert-Y is currently enabled.
    pub fn invert_y(&self) -> bool {
        self.invert_y
    }

    /// Process a winit keyboard event.
    pub fn handle_keyboard_event(&mut self, key: KeyCode, pressed: bool) {
        self.physical_state
            .insert(PhysicalInput::Key(key), pressed);
    }

    /// Accumulate mouse delta. Called for each DeviceEvent::MouseMotion.
    /// Task 03 extends this with sensitivity, invert-Y, and raw motion handling.
    pub fn handle_mouse_delta(&mut self, dx: f64, dy: f64) {
        self.mouse_delta.0 += dx;
        self.mouse_delta.1 += dy;
    }

    /// Process a mouse button event.
    pub fn handle_mouse_button(&mut self, button: MouseButton, pressed: bool) {
        self.physical_state
            .insert(PhysicalInput::MouseButton(button), pressed);
    }

    /// Hook for gamepad input. Task 04 will implement full gilrs polling here.
    /// For now, this allows setting gamepad axis values directly for testing.
    pub fn set_gamepad_axis(&mut self, action: Action, value: f32) {
        if value.abs() > f32::EPSILON {
            self.gamepad_axes.insert(action, value);
        } else {
            self.gamepad_axes.remove(&action);
        }
    }

    /// Clear all physical input state. Useful when window loses focus.
    pub fn clear_all(&mut self) {
        self.physical_state.clear();
        self.mouse_delta = (0.0, 0.0);
        self.mouse_axes.clear();
        self.gamepad_axes.clear();
    }

    /// Resolve all bindings and produce the action snapshot for this frame.
    /// Advances button state machines and resets per-frame accumulators.
    pub fn snapshot(&mut self) -> ActionSnapshot {
        // Convert accumulated mouse delta into axis values for bound actions.
        self.resolve_mouse_axes();

        // Collect all actions referenced by bindings.
        let actions: Vec<Action> = self
            .bindings
            .iter()
            .map(|b| b.action)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        let mut button_states = HashMap::new();
        let mut axis_values = HashMap::new();

        for action in &actions {
            if action.is_axis() {
                let values = bindings::resolve_axis_values(
                    *action,
                    &self.bindings,
                    &self.physical_state,
                    &self.mouse_axes,
                    &self.gamepad_axes,
                );
                if !values.is_empty() {
                    axis_values.insert(*action, values);
                }
            } else {
                let state = bindings::resolve_button_state(
                    *action,
                    &self.bindings,
                    &self.physical_state,
                    &self.prev_button_states,
                );
                button_states.insert(*action, state);
            }
        }

        // Store button states for next frame's transitions.
        self.prev_button_states = button_states.clone();

        // Reset per-frame accumulators.
        self.mouse_delta = (0.0, 0.0);
        self.mouse_axes.clear();

        ActionSnapshot {
            button_states,
            axis_values,
        }
    }

    /// Convert accumulated mouse delta into action axis values.
    /// Applies sensitivity (raw units -> radians) and invert-Y, then the binding's
    /// scale factor for direction. Sensitivity converts units; binding scale handles
    /// direction (e.g., -1 for inverted axis mapping).
    fn resolve_mouse_axes(&mut self) {
        let (dx, dy) = self.mouse_delta;

        // Apply sensitivity to convert raw mouse units to radians.
        let dx_rad = dx as f32 * self.mouse_sensitivity;
        let mut dy_rad = dy as f32 * self.mouse_sensitivity;

        // Invert-Y negates pitch after sensitivity, before binding scale.
        if self.invert_y {
            dy_rad = -dy_rad;
        }

        for binding in &self.bindings {
            match binding.input {
                PhysicalInput::MouseAxisX => {
                    let value = dx_rad * binding.scale;
                    let entry = self.mouse_axes.entry(binding.action).or_insert(0.0);
                    *entry += value;
                }
                PhysicalInput::MouseAxisY => {
                    let value = dy_rad * binding.scale;
                    let entry = self.mouse_axes.entry(binding.action).or_insert(0.0);
                    *entry += value;
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default bindings matching the current free-fly camera controls.
    fn test_bindings() -> Vec<Binding> {
        vec![
            Binding::with_scale(PhysicalInput::Key(KeyCode::KeyW), Action::MoveForward, 1.0),
            Binding::with_scale(PhysicalInput::Key(KeyCode::KeyS), Action::MoveForward, -1.0),
            Binding::with_scale(PhysicalInput::Key(KeyCode::KeyD), Action::MoveRight, 1.0),
            Binding::with_scale(PhysicalInput::Key(KeyCode::KeyA), Action::MoveRight, -1.0),
            Binding::with_scale(PhysicalInput::Key(KeyCode::KeyE), Action::MoveUp, 1.0),
            Binding::with_scale(PhysicalInput::Key(KeyCode::KeyQ), Action::MoveUp, -1.0),
            Binding::new(PhysicalInput::Key(KeyCode::ShiftLeft), Action::Sprint),
            Binding::new(PhysicalInput::Key(KeyCode::Space), Action::Jump),
            Binding::new(PhysicalInput::MouseButton(MouseButton::Left), Action::Shoot),
            Binding::new(PhysicalInput::MouseButton(MouseButton::Right), Action::AltFire),
            Binding::with_scale(PhysicalInput::MouseAxisX, Action::LookYaw, -1.0),
            Binding::with_scale(PhysicalInput::MouseAxisY, Action::LookPitch, -1.0),
        ]
    }

    // --- InputSystem keyboard handling ---

    #[test]
    fn input_system_produces_pressed_state_on_first_key_event() {
        let mut sys = InputSystem::new(test_bindings());
        sys.handle_keyboard_event(KeyCode::Space, true);
        let snap = sys.snapshot();
        assert_eq!(snap.button(Action::Jump), ButtonState::Pressed);
    }

    #[test]
    fn input_system_transitions_to_held_on_subsequent_frame() {
        let mut sys = InputSystem::new(test_bindings());
        sys.handle_keyboard_event(KeyCode::Space, true);
        let _ = sys.snapshot(); // frame 1: Pressed

        // Key still held (no new event needed, physical_state persists).
        let snap = sys.snapshot(); // frame 2: Held
        assert_eq!(snap.button(Action::Jump), ButtonState::Held);
    }

    #[test]
    fn input_system_transitions_to_released_when_key_released() {
        let mut sys = InputSystem::new(test_bindings());
        sys.handle_keyboard_event(KeyCode::Space, true);
        let _ = sys.snapshot(); // Pressed

        sys.handle_keyboard_event(KeyCode::Space, false);
        let snap = sys.snapshot();
        assert_eq!(snap.button(Action::Jump), ButtonState::Released);
    }

    #[test]
    fn input_system_transitions_to_inactive_after_release() {
        let mut sys = InputSystem::new(test_bindings());
        sys.handle_keyboard_event(KeyCode::Space, true);
        let _ = sys.snapshot(); // Pressed

        sys.handle_keyboard_event(KeyCode::Space, false);
        let _ = sys.snapshot(); // Released

        let snap = sys.snapshot(); // Inactive
        assert_eq!(snap.button(Action::Jump), ButtonState::Inactive);
    }

    // --- Axis from keyboard ---

    #[test]
    fn input_system_produces_axis_value_from_keyboard() {
        let mut sys = InputSystem::new(test_bindings());
        sys.handle_keyboard_event(KeyCode::KeyW, true);
        let snap = sys.snapshot();
        let values = snap.axis(Action::MoveForward);
        assert_eq!(values.len(), 1);
        assert!((values[0].value - 1.0).abs() < f32::EPSILON);
        assert_eq!(values[0].source, AxisSource::Velocity);
    }

    #[test]
    fn input_system_produces_negative_axis_from_reverse_key() {
        let mut sys = InputSystem::new(test_bindings());
        sys.handle_keyboard_event(KeyCode::KeyS, true);
        let snap = sys.snapshot();
        assert!((snap.axis_value(Action::MoveForward) - (-1.0)).abs() < f32::EPSILON);
    }

    // --- Mouse delta ---

    #[test]
    fn input_system_accumulates_mouse_delta_into_look_axes() {
        let mut sys = InputSystem::new(test_bindings());
        sys.handle_mouse_delta(10.0, -5.0);
        let snap = sys.snapshot();

        // MouseAxisX -> LookYaw with scale -1.0
        // Value = 10.0 * sensitivity(0.002) * scale(-1.0) = -0.02
        let yaw = snap.axis(Action::LookYaw);
        assert_eq!(yaw.len(), 1);
        assert_eq!(yaw[0].source, AxisSource::Displacement);
        assert!(
            (yaw[0].value - (-0.02)).abs() < 1e-6,
            "expected -0.02, got {}",
            yaw[0].value
        );

        // MouseAxisY -> LookPitch with scale -1.0
        // Value = -5.0 * sensitivity(0.002) * scale(-1.0) = 0.01
        let pitch = snap.axis(Action::LookPitch);
        assert_eq!(pitch.len(), 1);
        assert!(
            (pitch[0].value - 0.01).abs() < 1e-6,
            "expected 0.01, got {}",
            pitch[0].value
        );
    }

    #[test]
    fn input_system_resets_mouse_delta_after_snapshot() {
        let mut sys = InputSystem::new(test_bindings());
        sys.handle_mouse_delta(10.0, 5.0);
        let _ = sys.snapshot();

        // Next frame with no new mouse input should have no look axis values.
        let snap = sys.snapshot();
        assert!(snap.axis(Action::LookYaw).is_empty());
        assert!(snap.axis(Action::LookPitch).is_empty());
    }

    // --- Sensitivity ---

    #[test]
    fn sensitivity_scales_mouse_delta_to_radians() {
        let mut sys = InputSystem::new(test_bindings());
        sys.set_mouse_sensitivity(0.004); // double the default
        sys.handle_mouse_delta(100.0, 0.0);
        let snap = sys.snapshot();

        // 100.0 * 0.004 * scale(-1.0) = -0.4
        let yaw = snap.axis(Action::LookYaw);
        assert_eq!(yaw.len(), 1);
        assert!(
            (yaw[0].value - (-0.4)).abs() < 1e-6,
            "expected -0.4, got {}",
            yaw[0].value
        );
    }

    #[test]
    fn sensitivity_change_affects_look_speed() {
        let bindings = test_bindings();

        // Low sensitivity
        let mut low = InputSystem::new(bindings.clone());
        low.set_mouse_sensitivity(0.001);
        low.handle_mouse_delta(100.0, 0.0);
        let snap_low = low.snapshot();

        // High sensitivity
        let mut high = InputSystem::new(bindings);
        high.set_mouse_sensitivity(0.004);
        high.handle_mouse_delta(100.0, 0.0);
        let snap_high = high.snapshot();

        let yaw_low = snap_low.axis_value(Action::LookYaw).abs();
        let yaw_high = snap_high.axis_value(Action::LookYaw).abs();
        assert!(
            yaw_high > yaw_low,
            "higher sensitivity should produce larger axis value"
        );
    }

    // --- Invert Y ---

    #[test]
    fn invert_y_negates_pitch_axis() {
        let bindings = test_bindings();

        // Normal
        let mut normal = InputSystem::new(bindings.clone());
        normal.handle_mouse_delta(0.0, 10.0);
        let snap_normal = normal.snapshot();

        // Inverted
        let mut inverted = InputSystem::new(bindings);
        inverted.set_invert_y(true);
        inverted.handle_mouse_delta(0.0, 10.0);
        let snap_inverted = inverted.snapshot();

        let pitch_normal = snap_normal.axis_value(Action::LookPitch);
        let pitch_inverted = snap_inverted.axis_value(Action::LookPitch);
        assert!(
            (pitch_normal + pitch_inverted).abs() < 1e-6,
            "inverted pitch should negate normal: {} vs {}",
            pitch_normal,
            pitch_inverted
        );
    }

    #[test]
    fn invert_y_does_not_affect_yaw() {
        let bindings = test_bindings();

        let mut normal = InputSystem::new(bindings.clone());
        normal.handle_mouse_delta(10.0, 0.0);
        let snap_normal = normal.snapshot();

        let mut inverted = InputSystem::new(bindings);
        inverted.set_invert_y(true);
        inverted.handle_mouse_delta(10.0, 0.0);
        let snap_inverted = inverted.snapshot();

        let yaw_normal = snap_normal.axis_value(Action::LookYaw);
        let yaw_inverted = snap_inverted.axis_value(Action::LookYaw);
        assert!(
            (yaw_normal - yaw_inverted).abs() < 1e-6,
            "invert-Y should not affect yaw"
        );
    }

    // --- Delta accumulation ---

    #[test]
    fn multiple_mouse_deltas_accumulate_between_snapshots() {
        let mut sys = InputSystem::new(test_bindings());
        sys.handle_mouse_delta(5.0, 3.0);
        sys.handle_mouse_delta(7.0, -1.0);
        sys.handle_mouse_delta(-2.0, 4.0);
        let snap = sys.snapshot();

        // Total dx=10.0, dy=6.0
        // Yaw = 10.0 * 0.002 * -1.0 = -0.02
        let yaw = snap.axis_value(Action::LookYaw);
        assert!(
            (yaw - (-0.02)).abs() < 1e-6,
            "expected -0.02, got {}",
            yaw
        );

        // Pitch = 6.0 * 0.002 * -1.0 = -0.012
        let pitch = snap.axis_value(Action::LookPitch);
        assert!(
            (pitch - (-0.012)).abs() < 1e-6,
            "expected -0.012, got {}",
            pitch
        );
    }

    // --- Mouse button ---

    #[test]
    fn input_system_handles_mouse_button_as_action() {
        let mut sys = InputSystem::new(test_bindings());
        sys.handle_mouse_button(MouseButton::Left, true);
        let snap = sys.snapshot();
        assert_eq!(snap.button(Action::Shoot), ButtonState::Pressed);
    }

    // --- Cross-source additive resolution ---

    #[test]
    fn input_system_combines_mouse_displacement_and_gamepad_velocity_additively() {
        let mut sys = InputSystem::new(test_bindings());

        // Mouse contributes displacement to LookYaw.
        sys.handle_mouse_delta(10.0, 0.0);

        // Gamepad contributes velocity to LookYaw.
        sys.set_gamepad_axis(Action::LookYaw, 0.5);

        let snap = sys.snapshot();
        let yaw = snap.axis(Action::LookYaw);
        // Should have both displacement and velocity entries.
        assert_eq!(yaw.len(), 2);

        let displacement = yaw.iter().find(|v| v.source == AxisSource::Displacement);
        let velocity = yaw.iter().find(|v| v.source == AxisSource::Velocity);
        assert!(displacement.is_some());
        assert!(velocity.is_some());
        // Mouse: 10.0 * sensitivity(0.002) * scale(-1.0) = -0.02
        assert!(
            (displacement.unwrap().value - (-0.02)).abs() < 1e-6,
            "expected -0.02, got {}",
            displacement.unwrap().value
        );
        assert!((velocity.unwrap().value - 0.5).abs() < f32::EPSILON);
    }

    // --- Snapshot immutability ---

    #[test]
    fn snapshot_is_independent_of_subsequent_input_events() {
        let mut sys = InputSystem::new(test_bindings());
        sys.handle_keyboard_event(KeyCode::Space, true);
        let snap = sys.snapshot();

        // Mutate input state after snapshot.
        sys.handle_keyboard_event(KeyCode::Space, false);

        // Original snapshot is unchanged.
        assert_eq!(snap.button(Action::Jump), ButtonState::Pressed);
    }

    // --- clear_all ---

    #[test]
    fn clear_all_resets_physical_state() {
        let mut sys = InputSystem::new(test_bindings());
        sys.handle_keyboard_event(KeyCode::KeyW, true);
        sys.handle_mouse_delta(10.0, 5.0);
        sys.clear_all();

        let snap = sys.snapshot();
        assert!(snap.axis(Action::MoveForward).is_empty());
        assert!(snap.axis(Action::LookYaw).is_empty());
    }
}
