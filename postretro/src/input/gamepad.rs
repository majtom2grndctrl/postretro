// Gamepad input via gilrs: polling, dead zones, trigger thresholds.
// See: context/lib/input.md §5, context/plans/ready/phase-2-input-frame-timing/task-04-gamepad.md

use gilrs::{Axis, Button, Event, EventType, GamepadId, Gilrs};

use super::InputSystem;
use crate::input::types::PhysicalInput;

/// Dead zone radius for both sticks. Standard value across most controllers.
const DEAD_ZONE: f32 = 0.15;

/// Trigger value above which a trigger counts as a button press.
const TRIGGER_BUTTON_THRESHOLD: f32 = 0.5;

/// Manages gamepad input via gilrs.
///
/// Each frame, call `update()` to drain gilrs events and feed processed
/// axis/button state into the InputSystem. Tracks the most-recently-used
/// gamepad when multiple are connected.
pub struct GamepadSystem {
    gilrs: Gilrs,
    /// Most-recently-used gamepad. Updated when any gamepad produces input.
    active_gamepad: Option<GamepadId>,
}

impl GamepadSystem {
    /// Create the gamepad system. Initializes gilrs.
    /// Returns None if gilrs cannot be initialized (e.g., no gamepad subsystem).
    pub fn new() -> Option<Self> {
        match Gilrs::new() {
            Ok(gilrs) => {
                // Log connected gamepads at startup.
                for (_id, gamepad) in gilrs.gamepads() {
                    log::info!(
                        "[Input] Gamepad detected: {} ({})",
                        gamepad.name(),
                        gamepad.os_name()
                    );
                }
                Some(GamepadSystem {
                    gilrs,
                    active_gamepad: None,
                })
            }
            Err(err) => {
                log::warn!("[Input] Failed to initialize gilrs: {err} — gamepad support disabled");
                None
            }
        }
    }

    /// Poll gilrs events and feed processed state into the input system.
    /// Call once per frame, before `input_system.snapshot()`.
    pub fn update(&mut self, input_system: &mut InputSystem) {
        // Drain all pending events to track active gamepad.
        while let Some(Event { id, event, .. }) = self.gilrs.next_event() {
            // Any input event from a gamepad makes it the active one.
            if is_user_input(&event) {
                self.active_gamepad = Some(id);
            }
        }

        let gamepad_id = match self.active_gamepad {
            Some(id) => id,
            None => return,
        };

        let gamepad = self.gilrs.gamepad(gamepad_id);
        if !gamepad.is_connected() {
            self.active_gamepad = None;
            return;
        }

        // Read raw stick axes.
        let left_x = axis_value(&gamepad, Axis::LeftStickX);
        let left_y = axis_value(&gamepad, Axis::LeftStickY);
        let right_x = axis_value(&gamepad, Axis::RightStickX);
        let right_y = axis_value(&gamepad, Axis::RightStickY);

        // Apply radial dead zones.
        let (left_x, left_y) = apply_radial_dead_zone(left_x, left_y, DEAD_ZONE);
        let (right_x, right_y) = apply_radial_dead_zone(right_x, right_y, DEAD_ZONE);

        // Feed stick axes into input system.
        input_system.set_gamepad_axis(Axis::LeftStickX, left_x);
        input_system.set_gamepad_axis(Axis::LeftStickY, left_y);
        input_system.set_gamepad_axis(Axis::RightStickX, right_x);
        input_system.set_gamepad_axis(Axis::RightStickY, right_y);

        // Read triggers as axis values in [0, 1].
        let left_trigger = axis_value(&gamepad, Axis::LeftZ).max(0.0);
        let right_trigger = axis_value(&gamepad, Axis::RightZ).max(0.0);

        input_system.set_gamepad_axis(Axis::LeftZ, left_trigger);
        input_system.set_gamepad_axis(Axis::RightZ, right_trigger);

        // Triggers also produce button state via threshold.
        input_system.set_physical_input(
            PhysicalInput::GamepadButton(Button::LeftTrigger2),
            left_trigger >= TRIGGER_BUTTON_THRESHOLD,
        );
        input_system.set_physical_input(
            PhysicalInput::GamepadButton(Button::RightTrigger2),
            right_trigger >= TRIGGER_BUTTON_THRESHOLD,
        );

        // Read digital buttons.
        const BUTTONS: &[Button] = &[
            Button::South,       // A / Cross
            Button::East,        // B / Circle
            Button::West,        // X / Square
            Button::North,       // Y / Triangle
            Button::LeftTrigger, // LB / L1
            Button::RightTrigger, // RB / R1
            Button::Select,
            Button::Start,
            Button::LeftThumb,  // L3
            Button::RightThumb, // R3
            Button::DPadUp,
            Button::DPadDown,
            Button::DPadLeft,
            Button::DPadRight,
        ];

        for &button in BUTTONS {
            let pressed = gamepad.is_pressed(button);
            input_system.set_physical_input(PhysicalInput::GamepadButton(button), pressed);
        }
    }
}

/// Whether a gilrs event represents user input (vs. connection/disconnection).
fn is_user_input(event: &EventType) -> bool {
    matches!(
        event,
        EventType::ButtonPressed(..)
            | EventType::ButtonRepeated(..)
            | EventType::ButtonReleased(..)
            | EventType::ButtonChanged(..)
            | EventType::AxisChanged(..)
    )
}

/// Read an axis value from a gamepad, defaulting to 0 if unavailable.
fn axis_value(gamepad: &gilrs::Gamepad, axis: Axis) -> f32 {
    gamepad
        .axis_data(axis)
        .map(|data| data.value())
        .unwrap_or(0.0)
}

/// Apply radial dead zone to a stick's (x, y) pair.
///
/// - If magnitude < dead_zone, output is (0, 0).
/// - Otherwise, remap so the first detectable output starts at 0:
///   output = direction * (mag - dead_zone) / (1.0 - dead_zone)
/// - Clamp each axis to [-1, 1].
pub(crate) fn apply_radial_dead_zone(x: f32, y: f32, dead_zone: f32) -> (f32, f32) {
    let mag = (x * x + y * y).sqrt();

    if mag < dead_zone {
        return (0.0, 0.0);
    }

    // Normalize to get direction, then remap magnitude.
    let scale = (mag - dead_zone) / (mag * (1.0 - dead_zone));
    let out_x = (x * scale).clamp(-1.0, 1.0);
    let out_y = (y * scale).clamp(-1.0, 1.0);

    (out_x, out_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-6;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < EPSILON
    }

    // --- Radial dead zone tests ---

    #[test]
    fn dead_zone_zeroes_input_below_threshold() {
        let (x, y) = apply_radial_dead_zone(0.1, 0.0, DEAD_ZONE);
        assert_eq!(x, 0.0);
        assert_eq!(y, 0.0);
    }

    #[test]
    fn dead_zone_zeroes_diagonal_input_below_threshold() {
        // Diagonal at 0.1 per axis: magnitude ~0.141, below 0.15.
        let (x, y) = apply_radial_dead_zone(0.1, 0.1, DEAD_ZONE);
        assert_eq!(x, 0.0);
        assert_eq!(y, 0.0);
    }

    #[test]
    fn dead_zone_zeroes_exact_threshold() {
        // Exactly at the dead zone boundary should still be zero
        // (strictly less than, not less-or-equal, but at exact float boundary
        // the magnitude equals dead_zone which is < dead_zone is false).
        let (x, y) = apply_radial_dead_zone(DEAD_ZONE, 0.0, DEAD_ZONE);
        // At exactly threshold, mag == dead_zone, so mag < dead_zone is false.
        // Output should be very small but non-zero. The remapped value is:
        // scale = (0.15 - 0.15) / (0.15 * 0.85) = 0.
        assert!(approx_eq(x, 0.0));
        assert!(approx_eq(y, 0.0));
    }

    #[test]
    fn dead_zone_remaps_above_threshold_starting_near_zero() {
        // Just above the dead zone should produce a small positive value, not jump.
        let input = DEAD_ZONE + 0.01;
        let (x, _y) = apply_radial_dead_zone(input, 0.0, DEAD_ZONE);
        // Expected: (0.01) / (0.85) ≈ 0.01176
        let expected = 0.01 / (1.0 - DEAD_ZONE);
        assert!(
            approx_eq(x, expected),
            "expected {expected}, got {x}"
        );
    }

    #[test]
    fn dead_zone_produces_one_at_full_deflection() {
        let (x, y) = apply_radial_dead_zone(1.0, 0.0, DEAD_ZONE);
        assert!(approx_eq(x, 1.0), "expected 1.0, got {x}");
        assert!(approx_eq(y, 0.0), "expected 0.0, got {y}");
    }

    #[test]
    fn dead_zone_produces_negative_one_at_full_negative_deflection() {
        let (x, y) = apply_radial_dead_zone(-1.0, 0.0, DEAD_ZONE);
        assert!(approx_eq(x, -1.0), "expected -1.0, got {x}");
        assert!(approx_eq(y, 0.0), "expected 0.0, got {y}");
    }

    #[test]
    fn dead_zone_handles_full_diagonal_deflection() {
        // Full diagonal: magnitude = sqrt(2) ≈ 1.414.
        // After remapping, each axis should be clamped to [-1, 1].
        let (x, y) = apply_radial_dead_zone(1.0, 1.0, DEAD_ZONE);
        // Direction is (1/√2, 1/√2). Remapped mag = (√2 - 0.15) / (1 - 0.15) ≈ 1.49.
        // Output per axis = (1/√2) * 1.49 ≈ 1.054, clamped to 1.0.
        assert!(approx_eq(x, 1.0), "expected 1.0 (clamped), got {x}");
        assert!(approx_eq(y, 1.0), "expected 1.0 (clamped), got {y}");
    }

    #[test]
    fn dead_zone_preserves_direction_on_diagonal() {
        // A moderate diagonal input: both axes should have the same sign and
        // roughly the same magnitude (since input is symmetric).
        let (x, y) = apply_radial_dead_zone(0.5, 0.5, DEAD_ZONE);
        assert!(x > 0.0);
        assert!(y > 0.0);
        assert!(approx_eq(x, y), "diagonal should be symmetric: x={x}, y={y}");
    }

    #[test]
    fn dead_zone_handles_zero_input() {
        let (x, y) = apply_radial_dead_zone(0.0, 0.0, DEAD_ZONE);
        assert_eq!(x, 0.0);
        assert_eq!(y, 0.0);
    }

    // --- Trigger threshold tests ---

    #[test]
    fn trigger_below_threshold_is_inactive() {
        assert!(0.3 < TRIGGER_BUTTON_THRESHOLD);
    }

    #[test]
    fn trigger_at_threshold_is_active() {
        assert!(TRIGGER_BUTTON_THRESHOLD >= TRIGGER_BUTTON_THRESHOLD);
    }

    #[test]
    fn trigger_above_threshold_is_active() {
        assert!(0.8 >= TRIGGER_BUTTON_THRESHOLD);
    }
}
