// InputSystem: raw input state, binding resolution, and per-frame snapshots.
// See: context/lib/input.md §2, §3

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use gilrs::Axis as GilrsAxis;
use winit::event::{MouseButton, MouseScrollDelta};
use winit::keyboard::KeyCode;

use super::activator::ActivatorResolver;
use super::bindings;
use super::look::LookInputs;
use super::scroll::{
    LINE_SCROLL_GESTURE_REPEAT, LineScrollGesture, ScrollNotchAccumulator,
    wheel_diagnostics_enabled,
};
use super::snapshot::ActionSnapshot;
use super::types::{
    Action, AxisHalf, AxisSource, Binding, ButtonState, HALF_AXIS_PRESS_THRESHOLD, PhysicalInput,
};

/// Where an input edge came from. Event edges are authoritative fresh presses;
/// level edges are inferred from a poll and respect activator suppression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdgeSource {
    Event,
    Level,
}

/// One timestamped input edge, buffered until the next snapshot resolves it,
/// so a press and release between two frames still resolve in order (P1).
#[derive(Debug, Clone, Copy)]
struct InputEdge {
    input: PhysicalInput,
    down: bool,
    /// Seconds since the input system's epoch.
    t: f64,
    source: EdgeSource,
}

/// Default sensitivity: radians per raw mouse unit. Tuned for 800 DPI mice.
pub const DEFAULT_MOUSE_SENSITIVITY: f32 = 0.002;

/// The input subsystem. Collects raw input events and resolves them into action snapshots.
pub struct InputSystem {
    bindings: Vec<Binding>,

    /// Unique actions referenced by `bindings`, cached so `snapshot()` does not
    /// rebuild the list every frame. `new()` and `set_bindings()` refresh it.
    unique_actions: Vec<Action>,

    /// Current pressed/released state of each physical input (true = active).
    physical_state: HashMap<PhysicalInput, bool>,

    /// Edges since the last snapshot, resolved through activators in time order.
    pending_edges: Vec<InputEdge>,

    /// Activator resolution: per-binding down levels and per-command edges.
    resolver: ActivatorResolver,

    /// Per-binding activity for the current frame, aligned with `bindings`.
    /// Reused across frames so the snapshot path does not reallocate it.
    binding_active: Vec<bool>,

    /// Time origin for edge timestamps.
    epoch: Instant,

    /// Multiplier on every activator threshold, captured per press.
    hold_timing_scale: f32,

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
        let unique_actions = unique_actions(&bindings);

        // Pre-size `prev_button_states` to the count of button-type actions so
        // the first-frame `extend` fits without reallocation.
        let button_action_count = unique_actions.iter().filter(|a| !a.is_axis()).count();
        let mut resolver = ActivatorResolver::default();
        resolver.reset_for(bindings.len());
        let binding_active = vec![false; bindings.len()];

        Self {
            bindings,
            unique_actions,
            physical_state: HashMap::new(),
            pending_edges: Vec::new(),
            resolver,
            binding_active,
            epoch: Instant::now(),
            hold_timing_scale: 1.0,
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

    /// Replace the binding table (mod init, hot reload, rebind, host tuning).
    /// Input state and preferences survive; held inputs follow the rebuild
    /// rules in `ActivatorResolver::rebind` (P4).
    pub fn set_bindings(&mut self, bindings: Vec<Binding>) {
        let now = self.now();
        self.set_bindings_at(bindings, now);
    }

    pub(crate) fn set_bindings_at(&mut self, bindings: Vec<Binding>, now: f64) {
        // Edges already buffered resolve against the table they arrived under.
        self.resolve_pending_edges(now);
        self.resolver.rebind(&self.bindings, &bindings);
        self.unique_actions = unique_actions(&bindings);
        self.binding_active = vec![false; bindings.len()];
        self.bindings = bindings;
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

    /// Set the multiplier on activator thresholds. A press already down keeps
    /// the scale it started with.
    #[allow(dead_code)]
    pub fn set_hold_timing_scale(&mut self, scale: f32) {
        self.hold_timing_scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
    }

    /// Seconds since this input system's epoch: the timebase for edges.
    pub(crate) fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    /// Record a physical level change as a timestamped edge. Unchanged levels
    /// (an OS key repeat, a re-polled held button) produce no edge.
    fn record_edge(&mut self, input: PhysicalInput, down: bool, t: f64, source: EdgeSource) {
        let was_down = self.physical_state.get(&input).copied().unwrap_or(false);
        self.physical_state.insert(input, down);
        if was_down != down {
            self.pending_edges.push(InputEdge {
                input,
                down,
                t,
                source,
            });
        }
    }

    /// Whether invert-Y is currently enabled.
    #[allow(dead_code)]
    pub fn invert_y(&self) -> bool {
        self.invert_y
    }

    /// Process a winit keyboard event.
    pub fn handle_keyboard_event(&mut self, key: KeyCode, pressed: bool) {
        let t = self.now();
        self.handle_keyboard_event_at(key, pressed, t);
    }

    pub(crate) fn handle_keyboard_event_at(&mut self, key: KeyCode, pressed: bool, t: f64) {
        self.record_edge(PhysicalInput::Key(key), pressed, t, EdgeSource::Event);
    }

    /// Accumulate mouse delta. Called for each DeviceEvent::MouseMotion.
    pub fn handle_mouse_delta(&mut self, dx: f64, dy: f64) {
        self.mouse_delta.0 += dx;
        self.mouse_delta.1 += dy;
    }

    /// Process a mouse button event.
    pub fn handle_mouse_button(&mut self, button: MouseButton, pressed: bool) {
        let t = self.now();
        self.record_edge(
            PhysicalInput::MouseButton(button),
            pressed,
            t,
            EdgeSource::Event,
        );
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
        let t = self.now();
        self.set_gamepad_axis_at(axis, value, t);
    }

    pub(crate) fn set_gamepad_axis_at(&mut self, axis: GilrsAxis, value: f32, t: f64) {
        if value.abs() > f32::EPSILON {
            self.gamepad_axes.insert(axis, value);
        } else {
            self.gamepad_axes.remove(&axis);
        }
        // Each half of the axis is also a digital input for button commands.
        for half in [AxisHalf::Positive, AxisHalf::Negative] {
            let pressed = half.magnitude(value) >= HALF_AXIS_PRESS_THRESHOLD;
            self.record_edge(
                PhysicalInput::GamepadAxisHalf(axis, half),
                pressed,
                t,
                EdgeSource::Level,
            );
        }
    }

    /// Set the state of a physical input directly. Used by GamepadSystem for buttons.
    pub fn set_physical_input(&mut self, input: PhysicalInput, active: bool) {
        let t = self.now();
        self.set_physical_input_at(input, active, t);
    }

    pub(crate) fn set_physical_input_at(&mut self, input: PhysicalInput, active: bool, t: f64) {
        self.record_edge(input, active, t, EdgeSource::Level);
    }

    /// A gamepad button event from the event stream, `age` seconds old. Event
    /// edges keep a press and release between two polls (P1); the per-frame
    /// poll through `set_physical_input` then reconciles the level.
    pub fn handle_gamepad_button_event(&mut self, button: gilrs::Button, pressed: bool, age: f64) {
        let t = (self.now() - age.max(0.0)).max(0.0);
        self.record_edge(
            PhysicalInput::GamepadButton(button),
            pressed,
            t,
            EdgeSource::Event,
        );
    }

    /// Clear all physical input state. Useful when window loses focus.
    ///
    /// Cancels every pending activator resolution: neither binding of a pending
    /// tap/hold pair fires, and an input held through the clear does nothing
    /// until it is pressed again.
    pub fn clear_all(&mut self) {
        let now = self.now();
        self.clear_all_at(now);
    }

    pub(crate) fn clear_all_at(&mut self, now: f64) {
        self.resolve_pending_edges(now);
        self.resolver.cancel_all(&self.bindings);
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
        let now = self.now();
        self.snapshot_at(now)
    }

    /// Feed buffered edges to the activator resolver in time order, then fire
    /// every hold whose min elapsed by `now`.
    fn resolve_pending_edges(&mut self, now: f64) {
        self.pending_edges.sort_by(|a, b| a.t.total_cmp(&b.t));
        for edge in self.pending_edges.drain(..) {
            match edge.source {
                EdgeSource::Event => self.resolver.event_edge(
                    &self.bindings,
                    edge.input,
                    edge.down,
                    edge.t,
                    self.hold_timing_scale,
                ),
                EdgeSource::Level => self.resolver.level_edge(
                    &self.bindings,
                    edge.input,
                    edge.down,
                    edge.t,
                    self.hold_timing_scale,
                ),
            }
        }
        self.resolver.advance(&self.bindings, now);
    }

    /// Refresh `binding_active` from the resolver. Wheel inputs are momentary
    /// and bypass activators: a notch drives its binding for the one frame.
    fn refresh_binding_activity(&mut self) {
        for (index, binding) in self.bindings.iter().enumerate() {
            self.binding_active[index] = match binding.input {
                PhysicalInput::MouseWheelUp | PhysicalInput::MouseWheelDown => self
                    .physical_state
                    .get(&binding.input)
                    .copied()
                    .unwrap_or(false),
                _ => self.resolver.binding_down(index),
            };
        }
    }

    pub(crate) fn snapshot_at(&mut self, now: f64) -> ActionSnapshot {
        self.resolve_pending_edges(now);
        self.refresh_binding_activity();

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
                    &self.binding_active,
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
                    &self.binding_active,
                    &self.prev_button_states,
                    self.resolver.command_went_down(action),
                );
                button_states.insert(action, state);
                // A notch-read command counts wheel notches plus one step per
                // press edge from any other bound input (the D-pad cycles).
                let count = self
                    .bindings
                    .iter()
                    .filter(|binding| binding.action == action)
                    .map(|binding| self.scroll_notches.count(binding.input))
                    .sum::<u32>()
                    + u32::from(self.resolver.command_went_down(action));
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
        self.resolver.take_frame();
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
                &self.binding_active,
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

/// Unique actions referenced by a binding table, in first-seen order.
fn unique_actions(bindings: &[Binding]) -> Vec<Action> {
    let mut seen = HashSet::with_capacity(bindings.len());
    let mut unique = Vec::with_capacity(bindings.len());
    for binding in bindings {
        if seen.insert(binding.action) {
            unique.push(binding.action);
        }
    }
    unique
}
