// InputSystem: raw input state, binding resolution, and per-frame snapshots.
// See: context/lib/input.md §2, §3

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use gilrs::Axis as GilrsAxis;
use winit::event::{MouseButton, MouseScrollDelta};
use winit::keyboard::KeyCode;

use super::bindings;
use super::look::LookInputs;
use super::scroll::{
    LINE_SCROLL_GESTURE_REPEAT, LineScrollGesture, ScrollNotchAccumulator,
    wheel_diagnostics_enabled,
};
use super::snapshot::ActionSnapshot;
use super::types::{Action, AxisSource, Binding, ButtonState, PhysicalInput};

/// Default sensitivity: radians per raw mouse unit. Tuned for 800 DPI mice.
pub const DEFAULT_MOUSE_SENSITIVITY: f32 = 0.002;

/// The input subsystem. Collects raw input events and resolves them into action snapshots.
pub struct InputSystem {
    bindings: Vec<Binding>,

    /// Unique actions referenced by `bindings`, cached at construction time so
    /// `snapshot()` does not rebuild the list every frame. `bindings` currently
    /// has no rebind write site outside `new()`; if one is added, refresh this
    /// cache alongside it.
    unique_actions: Vec<Action>,

    /// Current pressed/released state of each physical input (true = active).
    physical_state: HashMap<PhysicalInput, bool>,

    /// Button states from the previous snapshot, used for state machine transitions.
    /// Pre-sized in `new()` to the button-action count so the per-frame
    /// `clear() + extend()` path stays allocation-free after the first frame.
    prev_button_states: HashMap<Action, ButtonState>,

    /// Accumulated mouse delta since last snapshot. Reset after each snapshot.
    mouse_delta: (f64, f64),

    /// Mouse axis values resolved from accumulated delta for the current frame.
    mouse_axes: HashMap<Action, f32>,

    /// Explicit wheel notch counts, distinct from `ButtonState` so one frame
    /// can retain several wheel inputs.
    scroll_notches: ScrollNotchAccumulator,

    /// Debounces continuous/accelerated `LineDelta` events into weapon-cycle
    /// steps. Pixel deltas retain their separate configurable threshold.
    line_scroll_gesture: LineScrollGesture,

    /// Player-configured pixel distance corresponding to one wheel notch.
    scroll_notch_pixels: f64,

    /// Raw gamepad axis values keyed by gilrs Axis. Resolved through bindings.
    gamepad_axes: HashMap<GilrsAxis, f32>,

    /// Radians per raw mouse unit. Converts OS mouse units to look rotation.
    mouse_sensitivity: f32,

    /// When true, negate the Y (pitch) mouse axis. Applied after sensitivity.
    invert_y: bool,
}

impl InputSystem {
    pub fn new(bindings: Vec<Binding>) -> Self {
        // Build the unique-action cache once at construction. Using a local
        // HashSet for dedup keeps the constructor cost a one-time hit.
        let mut seen = HashSet::with_capacity(bindings.len());
        let mut unique_actions: Vec<Action> = Vec::with_capacity(bindings.len());
        for binding in &bindings {
            if seen.insert(binding.action) {
                unique_actions.push(binding.action);
            }
        }

        // Pre-size `prev_button_states` to the count of button-type actions so
        // the first-frame `extend` fits without reallocation.
        let button_action_count = unique_actions.iter().filter(|a| !a.is_axis()).count();

        Self {
            bindings,
            unique_actions,
            physical_state: HashMap::new(),
            prev_button_states: HashMap::with_capacity(button_action_count),
            mouse_delta: (0.0, 0.0),
            mouse_axes: HashMap::new(),
            scroll_notches: ScrollNotchAccumulator::default(),
            line_scroll_gesture: LineScrollGesture::default(),
            scroll_notch_pixels: 120.0,
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
    #[allow(dead_code)]
    pub fn mouse_sensitivity(&self) -> f32 {
        self.mouse_sensitivity
    }

    /// Enable or disable invert-Y for mouse look.
    pub fn set_invert_y(&mut self, invert: bool) {
        self.invert_y = invert;
    }

    /// Set the pixel-scroll normalization threshold loaded from player options.
    pub fn set_scroll_notch_pixels(&mut self, pixels: f32) {
        self.scroll_notch_pixels = if pixels.is_finite() && pixels > 0.0 {
            f64::from(pixels)
        } else {
            120.0
        };
    }

    /// Whether invert-Y is currently enabled.
    #[allow(dead_code)]
    pub fn invert_y(&self) -> bool {
        self.invert_y
    }

    /// Process a winit keyboard event.
    pub fn handle_keyboard_event(&mut self, key: KeyCode, pressed: bool) {
        self.physical_state.insert(PhysicalInput::Key(key), pressed);
    }

    /// Accumulate mouse delta. Called for each DeviceEvent::MouseMotion.
    pub fn handle_mouse_delta(&mut self, dx: f64, dy: f64) {
        self.mouse_delta.0 += dx;
        self.mouse_delta.1 += dy;
    }

    /// Process a mouse button event.
    pub fn handle_mouse_button(&mut self, button: MouseButton, pressed: bool) {
        self.physical_state
            .insert(PhysicalInput::MouseButton(button), pressed);
    }

    /// Normalize a winit wheel event into explicit per-frame notches. Wheel
    /// inputs are momentary, so `snapshot` clears their physical states itself.
    pub fn handle_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        self.handle_mouse_wheel_at(delta, Instant::now());
    }

    pub(super) fn handle_mouse_wheel_at(&mut self, delta: MouseScrollDelta, now: Instant) {
        match delta {
            MouseScrollDelta::LineDelta(_, vertical) => {
                let up_before = self.scroll_notches.up;
                let down_before = self.scroll_notches.down;
                let emitted = self.line_scroll_gesture.accepts(f64::from(vertical), now);
                if emitted {
                    self.scroll_notches
                        .add_signed_notches(if vertical.is_sign_positive() { 1 } else { -1 });
                }
                if wheel_diagnostics_enabled() {
                    log::info!(
                        "[Input] wheel diagnostic: LineDelta vertical={vertical:.4}; gesture emitted={emitted} (repeat {} ms); emitted up={} down={}",
                        LINE_SCROLL_GESTURE_REPEAT.as_millis(),
                        self.scroll_notches.up - up_before,
                        self.scroll_notches.down - down_before,
                    );
                }
            }
            MouseScrollDelta::PixelDelta(position) => {
                let remainder_before = self.scroll_notches.pixel_remainder;
                let up_before = self.scroll_notches.up;
                let down_before = self.scroll_notches.down;
                self.scroll_notches
                    .add_pixel_delta(position.y, self.scroll_notch_pixels);
                if wheel_diagnostics_enabled() {
                    log::info!(
                        "[Input] wheel diagnostic: PixelDelta vertical={:.4}; pixel remainder {remainder_before:.4} -> {:.4} (threshold {:.1}); emitted up={} down={}",
                        position.y,
                        self.scroll_notches.pixel_remainder,
                        self.scroll_notch_pixels,
                        self.scroll_notches.up - up_before,
                        self.scroll_notches.down - down_before,
                    );
                }
            }
        }
        if self.scroll_notches.up != 0 {
            self.physical_state
                .insert(PhysicalInput::MouseWheelUp, true);
        }
        if self.scroll_notches.down != 0 {
            self.physical_state
                .insert(PhysicalInput::MouseWheelDown, true);
        }
    }

    /// Set a raw gamepad axis value. Called by GamepadSystem after dead zone processing.
    /// The value is resolved through bindings to produce action axis values.
    pub fn set_gamepad_axis(&mut self, axis: GilrsAxis, value: f32) {
        if value.abs() > f32::EPSILON {
            self.gamepad_axes.insert(axis, value);
        } else {
            self.gamepad_axes.remove(&axis);
        }
    }

    /// Set the state of a physical input directly. Used by GamepadSystem for buttons.
    pub fn set_physical_input(&mut self, input: PhysicalInput, active: bool) {
        self.physical_state.insert(input, active);
    }

    /// Clear all physical input state. Useful when window loses focus.
    pub fn clear_all(&mut self) {
        self.physical_state.clear();
        self.mouse_delta = (0.0, 0.0);
        self.mouse_axes.clear();
        self.gamepad_axes.clear();
        self.scroll_notches.clear_all();
        self.line_scroll_gesture.clear();
    }

    /// Resolve all bindings and produce the action snapshot for this frame.
    /// Advances button state machines and resets per-frame accumulators.
    pub fn snapshot(&mut self) -> ActionSnapshot {
        // Convert accumulated mouse delta into axis values for bound actions.
        self.resolve_mouse_axes();

        let mut button_states = HashMap::new();
        let mut axis_values = HashMap::new();
        let mut notch_counts = HashMap::new();

        for &action in &self.unique_actions {
            if action.is_axis() {
                let values = bindings::resolve_axis_values(
                    action,
                    &self.bindings,
                    &self.physical_state,
                    &self.mouse_axes,
                    &self.gamepad_axes,
                );
                if !values.is_empty() {
                    axis_values.insert(action, values);
                }
            } else {
                let state = bindings::resolve_button_state(
                    action,
                    &self.bindings,
                    &self.physical_state,
                    &self.prev_button_states,
                );
                button_states.insert(action, state);
                let count = self
                    .bindings
                    .iter()
                    .filter(|binding| binding.action == action)
                    .map(|binding| self.scroll_notches.count(binding.input))
                    .sum();
                if count != 0 {
                    notch_counts.insert(action, count);
                }
            }
        }

        // Store button states for next frame's transitions. Reuse the existing
        // backing allocation — `clear()` retains capacity and `extend` with
        // `Copy` key/value is a cheap in-place fill. `std::mem::swap` would
        // leave stale prev-state data in the local we need to return fresh.
        self.prev_button_states.clear();
        self.prev_button_states
            .extend(button_states.iter().map(|(&k, &v)| (k, v)));

        // Reset per-frame accumulators.
        self.mouse_delta = (0.0, 0.0);
        self.mouse_axes.clear();
        self.scroll_notches.clear_frame();
        self.physical_state.remove(&PhysicalInput::MouseWheelUp);
        self.physical_state.remove(&PhysicalInput::MouseWheelDown);

        ActionSnapshot {
            button_states,
            axis_values,
            notch_counts,
        }
    }

    /// Drain the evanescent look-axis contributions accumulated since the last
    /// drain into a `LookInputs` value. Consumers apply this at render rate,
    /// before the fixed-tick loop, so mouse motion is never lost on zero-tick
    /// frames. Must be called before `snapshot()` in the same frame — it
    /// zeroes the mouse-displacement entries that `snapshot()` would otherwise
    /// re-emit as `LookYaw` / `LookPitch`.
    ///
    /// Mouse displacement is refreshed from `mouse_delta`, copied out, and
    /// cleared. Gamepad stick
    /// state in `gamepad_axes` is deliberately left intact — stick deflection
    /// is persistent, not evanescent, and a subsequent `snapshot()` will still
    /// see it. The render loop does not read look axes from `snapshot()`, so
    /// that re-emission has no reader.
    pub fn drain_look_inputs(&mut self) -> LookInputs {
        // Refresh mouse_axes from the accumulated delta so the displacement
        // branch below sees the latest motion.
        self.resolve_mouse_axes();

        let mut look = LookInputs::default();

        // Walk LookYaw / LookPitch through the same resolver snapshot() uses.
        // This shares the binding-table lookup path — no parallel code.
        for action in [Action::LookYaw, Action::LookPitch] {
            let values = bindings::resolve_axis_values(
                action,
                &self.bindings,
                &self.physical_state,
                &self.mouse_axes,
                &self.gamepad_axes,
            );
            for av in values {
                match (action, av.source) {
                    (Action::LookYaw, AxisSource::Displacement) => {
                        look.yaw_displacement += av.value;
                    }
                    (Action::LookYaw, AxisSource::Velocity) => {
                        look.yaw_velocity += av.value;
                    }
                    (Action::LookPitch, AxisSource::Displacement) => {
                        look.pitch_displacement += av.value;
                    }
                    (Action::LookPitch, AxisSource::Velocity) => {
                        look.pitch_velocity += av.value;
                    }
                    _ => {}
                }
            }
        }

        // Zero out the evanescent mouse state so a same-frame snapshot() does
        // not re-emit it. Non-look mouse-axis entries (none today, but the
        // resolver permits them) are left alone.
        self.mouse_delta = (0.0, 0.0);
        self.mouse_axes.remove(&Action::LookYaw);
        self.mouse_axes.remove(&Action::LookPitch);

        look
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
