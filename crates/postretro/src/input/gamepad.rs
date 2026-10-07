// Gamepad input via gilrs: polling, dead zones, trigger thresholds.
// See: context/lib/input.md §6

use gilrs::ff::{BaseEffect, BaseEffectType, Effect, EffectBuilder, Replay, Ticks};
use gilrs::{Axis, Button, Event, EventType, GamepadId, Gilrs};

use super::InputSystem;
use crate::input::commands::Command;
use crate::input::types::{PhysicalInput, hysteresis_level};
use crate::input::ui_nav::{NavIntent, StickNavTrackers};
use crate::input::ui_nav_map::{StickSide, UiNavContext, UiNavMap, stick_half_for};

/// One frame's UI-relevant gamepad output: the nav intent down-edges harvested
/// this frame, plus the two release channels the focus engine's dt-clocked
/// repeat timers need to stop.
///
/// `confirm_released` is true when a button bound to `nav_confirm` was RELEASED
/// this frame — it stops a held `repeatOnHold` button, the gamepad twin of the
/// keyboard confirm-release path.
///
/// `directional_released` is true when NO input bound to a nav direction is
/// currently held (buttons, and either stick's half-axes past the dead zone). It
/// clears the focus engine's directional hold-to-repeat clock,
/// mirroring the keyboard arrow-key-up path; without it a press that armed the
/// repeat clock would free-run on dt until the next stack/intent change (runaway
/// focus-scroll on any tree declaring a `repeat` policy).
///
/// Both are needed because the press-edge stream on `nav_intents` (one per press,
/// repeats from the focus engine's dt clock) carries no release.
#[derive(Debug, Default)]
pub struct GamepadNavOutput {
    pub nav_intents: Vec<NavIntent>,
    /// Every named input pressed this frame: button presses, trigger
    /// crossings, and stick halves pushed from rest. The controls panel's
    /// capture prompt takes these instead of the nav intents.
    pub presses: Vec<PhysicalInput>,
    /// On-screen keyboard shortcuts pressed this frame (text-entry context).
    pub text_shortcuts: Vec<Command>,
    /// An input bound to a text shortcut released this frame; it stops a held
    /// backspace shortcut's repeat.
    pub text_shortcut_released: bool,
    /// The active pad's USB vendor id, for the glyph family.
    pub vendor_id: Option<u16>,
    pub confirm_released: bool,
    pub directional_released: bool,
}

/// Digital buttons polled each frame. The analog triggers are not here: they
/// become buttons through `TRIGGER_PRESS_THRESHOLD`, not gilrs's own threshold.
const BUTTONS: &[Button] = &[
    Button::South,        // A / Cross
    Button::East,         // B / Circle
    Button::West,         // X / Square
    Button::North,        // Y / Triangle
    Button::LeftTrigger,  // LB / L1
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

/// The text shortcut `input` drives in `context`: one only while a text-entry
/// tree is on top; shortcuts do nothing once a commit has popped it.
fn text_shortcut(
    ui_nav: &UiNavMap,
    input: PhysicalInput,
    context: UiNavContext,
) -> Option<Command> {
    if context != UiNavContext::TextEntry {
        return None;
    }
    ui_nav.command_for(input, context).filter(|command| {
        matches!(
            command,
            Command::TextBackspace | Command::TextSpace | Command::TextCommit
        )
    })
}

/// Trigger value at which a released trigger counts as a button press.
const TRIGGER_PRESS_THRESHOLD: f32 = 0.5;

/// Trigger value below which a pressed trigger releases. The gap to
/// [`TRIGGER_PRESS_THRESHOLD`] keeps a trigger resting near the press point
/// from chattering press edges.
const TRIGGER_RELEASE_THRESHOLD: f32 = 0.4;

/// Whether a trigger reads as a pressed button, given whether it did last poll.
fn trigger_is_active(was_down: bool, value: f32) -> bool {
    hysteresis_level(
        was_down,
        value,
        TRIGGER_PRESS_THRESHOLD,
        TRIGGER_RELEASE_THRESHOLD,
    )
}

/// Combine a trigger's button-reported and axis-reported analog values into one
/// value in [0, 1]. Absent readings count as zero.
fn trigger_value(button: Option<f32>, axis: Option<f32>) -> f32 {
    button
        .unwrap_or(0.0)
        .max(axis.unwrap_or(0.0))
        .clamp(0.0, 1.0)
}

/// Read a trigger's analog value from a gamepad.
///
/// gilrs reports triggers as `Button::*Trigger2` with an analog button value on
/// the WGI and SDL-mapping paths, never as `Axis::*Z`; the axis stays as a
/// fallback for backends that do report it.
fn trigger_reading(gamepad: &gilrs::Gamepad, button: Button, axis: Axis) -> f32 {
    trigger_value(
        gamepad.button_data(button).map(|data| data.value()),
        gamepad.axis_data(axis).map(|data| data.value()),
    )
}

/// Manages gamepad input via gilrs.
///
/// Each frame, call `update()` to drain gilrs events and feed processed
/// axis/button state into the InputSystem. Tracks the most-recently-used
/// gamepad when multiple are connected.
pub struct GamepadSystem {
    gilrs: Gilrs,
    /// Most-recently-used gamepad. Updated when any gamepad produces input.
    active_gamepad: Option<GamepadId>,
    /// The currently-playing rumble effect and how long it has left to run
    /// (milliseconds). gilrs reference-counts the [`Effect`] handle, so holding
    /// it keeps the effect alive; dropping it (when the timeout elapses or a new
    /// rumble replaces it) stops the vibration. `None` when nothing is rumbling.
    active_rumble: Option<ActiveRumble>,
    /// Latches once after a force-feedback no-op so the unsupported-backend
    /// warning is logged at most once, not on every `rumble` call.
    ff_warned: bool,
    /// Whether each trigger (left, right) was past its button threshold last
    /// poll, so a crossing reports one press.
    triggers_down: [bool; 2],
}

/// A live rumble effect plus its remaining duration. The effect handle is kept
/// alive for `remaining_ms`; `tick_rumble` drops it once the time elapses.
struct ActiveRumble {
    effect: Effect,
    remaining_ms: f32,
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
                    active_rumble: None,
                    ff_warned: false,
                    triggers_down: [false; 2],
                })
            }
            Err(err) => {
                log::warn!("[Input] Failed to initialize gilrs: {err} — gamepad support disabled");
                None
            }
        }
    }

    /// Whether any gamepad is connected, as of the last event drain.
    pub fn any_connected(&self) -> bool {
        self.gilrs.gamepads().next().is_some()
    }

    /// Drain buffered gilrs events without acting on them, keeping the active
    /// gamepad current. Frames that draw no UI call this so a press made during
    /// a splash or Loading frame never surfaces on the first frame that does.
    pub fn discard_pending_events(&mut self) {
        while let Some(Event { id, event, .. }) = self.gilrs.next_event() {
            if is_user_input(&event) {
                self.active_gamepad = Some(id);
            }
        }
    }

    /// Poll gilrs events and feed processed state into the input system,
    /// returning the UI nav intents produced this frame (button down-edges and
    /// either stick's half-axis crossing past the dead zone, resolved through
    /// the binding table).
    ///
    /// Call once per frame, before `input_system.snapshot()` and — critically —
    /// before the `UiDispatch` `take_ready`/`advance_frame` pair, so the returned
    /// nav intents can be enqueued ahead of promotion and ride the same N→N+1
    /// contract as keyboard captures. The caller enqueues them only while a
    /// capturing tree owns input. `nav_sticks` holds the per-stick edge
    /// detectors, owned by the caller so their latches persist across frames.
    /// See: context/lib/input.md §7
    ///
    /// Nav intents resolve through `ui_nav`, the UI slice of the effective
    /// binding table, in `context`: a remapped direction navigates from its new
    /// input and no longer from the old one.
    pub fn update(
        &mut self,
        input_system: &mut InputSystem,
        nav_sticks: &mut StickNavTrackers,
        ui_nav: &UiNavMap,
        context: UiNavContext,
    ) -> GamepadNavOutput {
        let mut out = GamepadNavOutput::default();

        // Drain all pending events to track the active gamepad and harvest
        // button-down edges as nav intents. gilrs delivers a discrete
        // `ButtonPressed` per press, so this is the natural edge source — one
        // intent per press, repeats handled by the focus engine's timer.
        // A `ButtonReleased` of a button bound to `nav_confirm` surfaces the
        // confirm-release edge so a held `repeatOnHold` button stops re-firing.
        let received_at = std::time::SystemTime::now();
        while let Some(Event {
            id, event, time, ..
        }) = self.gilrs.next_event()
        {
            // Any input event from a gamepad makes it the active one.
            if is_user_input(&event) {
                self.active_gamepad = Some(id);
            }
            // Button events are the gameplay edge source too, so a press and
            // release between two polls still resolve (a shared tap/hold key
            // still taps rather than losing the press). The poll below
            // reconciles the level and adds no edge when it agrees.
            let age = received_at
                .duration_since(time)
                .map_or(0.0, |age| age.as_secs_f64());
            match event {
                EventType::ButtonPressed(button, _) => {
                    let input = PhysicalInput::GamepadButton(button);
                    // Triggers press through the engine's own threshold below.
                    if BUTTONS.contains(&button) {
                        out.presses.push(input);
                    }
                    if let Some(intent) = ui_nav.intent_for(input, context) {
                        out.nav_intents.push(intent);
                    }
                    if let Some(command) = text_shortcut(ui_nav, input, context) {
                        out.text_shortcuts.push(command);
                    }
                    if BUTTONS.contains(&button) {
                        input_system.handle_gamepad_button_event(button, true, age);
                    }
                }
                EventType::ButtonReleased(button, _) => {
                    // Whichever button confirm is bound to now ends its repeat,
                    // even after a remap or confirm/cancel swap.
                    if ui_nav.is_bound_to(PhysicalInput::GamepadButton(button), Command::NavConfirm)
                    {
                        out.confirm_released = true;
                    }
                    if text_shortcut(
                        ui_nav,
                        PhysicalInput::GamepadButton(button),
                        UiNavContext::TextEntry,
                    )
                    .is_some()
                    {
                        out.text_shortcut_released = true;
                    }
                    if BUTTONS.contains(&button) {
                        input_system.handle_gamepad_button_event(button, false, age);
                    }
                }
                _ => {}
            }
        }

        let gamepad_id = match self.active_gamepad {
            Some(id) => id,
            None => {
                // No active gamepad: still clear the stick latch so a stick that
                // was held when the pad disconnected re-arms cleanly. With no pad
                // nothing is held, so the directional repeat clock may release.
                nav_sticks.clear();
                out.directional_released = true;
                return out;
            }
        };

        let gamepad = self.gilrs.gamepad(gamepad_id);
        out.vendor_id = gamepad.vendor_id();
        if !gamepad.is_connected() {
            // The pad is gone mid-hold: nothing it held may stay down, and a
            // pending hold on it never fires.
            self.active_gamepad = None;
            self.triggers_down = [false; 2];
            input_system.release_gamepad();
            nav_sticks.clear();
            out.directional_released = true;
            return out;
        }

        // Read raw stick axes.
        let left_x = axis_value(&gamepad, Axis::LeftStickX);
        let left_y = axis_value(&gamepad, Axis::LeftStickY);
        let right_x = axis_value(&gamepad, Axis::RightStickX);
        let right_y = axis_value(&gamepad, Axis::RightStickY);

        // Apply radial dead zones.
        // The stick bound to look takes the player's look dead zone; the other
        // keeps the engine's.
        let left_dz = input_system.stick_dead_zone(Axis::LeftStickX, Axis::LeftStickY);
        let right_dz = input_system.stick_dead_zone(Axis::RightStickX, Axis::RightStickY);
        let (left_x, left_y) = apply_radial_dead_zone(left_x, left_y, left_dz);
        let (right_x, right_y) = apply_radial_dead_zone(right_x, right_y, right_dz);

        // A stick navigates through its half-axis inputs: a push past the dead
        // zone fires one directional crossing, which resolves to whatever nav
        // command that half is bound to (the left stick by default).
        for (side, crossing) in [
            (StickSide::Left, nav_sticks.left.update(left_x, left_y)),
            (StickSide::Right, nav_sticks.right.update(right_x, right_y)),
        ] {
            if let Some(half) = crossing.and_then(|direction| stick_half_for(side, direction)) {
                out.presses.push(half);
                if let Some(intent) = ui_nav.intent_for(half, context) {
                    out.nav_intents.push(intent);
                }
            }
        }

        // Feed stick axes into input system.
        input_system.set_gamepad_axis(Axis::LeftStickX, left_x);
        input_system.set_gamepad_axis(Axis::LeftStickY, left_y);
        input_system.set_gamepad_axis(Axis::RightStickX, right_x);
        input_system.set_gamepad_axis(Axis::RightStickY, right_y);

        // Read triggers as values in [0, 1].
        let left_trigger = trigger_reading(&gamepad, Button::LeftTrigger2, Axis::LeftZ);
        let right_trigger = trigger_reading(&gamepad, Button::RightTrigger2, Axis::RightZ);

        input_system.set_gamepad_axis(Axis::LeftZ, left_trigger);
        input_system.set_gamepad_axis(Axis::RightZ, right_trigger);

        // Triggers also produce button state via threshold, with hysteresis.
        for (index, (button, value)) in [
            (Button::LeftTrigger2, left_trigger),
            (Button::RightTrigger2, right_trigger),
        ]
        .into_iter()
        .enumerate()
        {
            let down = trigger_is_active(self.triggers_down[index], value);
            if down && !self.triggers_down[index] {
                out.presses.push(PhysicalInput::GamepadButton(button));
            }
            self.triggers_down[index] = down;
            input_system.set_physical_input(PhysicalInput::GamepadButton(button), down);
        }

        // Read digital buttons.
        for &button in BUTTONS {
            let pressed = gamepad.is_pressed(button);
            input_system.set_physical_input(PhysicalInput::GamepadButton(button), pressed);
        }

        // Directional-release channel: true when no input bound to a nav
        // direction is held. The focus engine consumes this to clear its
        // hold-to-repeat clock, the gamepad twin of a direction key's release.
        // Stick values are already dead-zoned, so a stick at rest reads zero.
        let stick_value = |axis: Axis| match axis {
            Axis::LeftStickX => left_x,
            Axis::LeftStickY => left_y,
            Axis::RightStickX => right_x,
            Axis::RightStickY => right_y,
            _ => 0.0,
        };
        let direction_held = ui_nav.direction_inputs().any(|input| match input {
            PhysicalInput::GamepadButton(button) => gamepad.is_pressed(button),
            PhysicalInput::GamepadAxisHalf(axis, half) => half.magnitude(stick_value(axis)) > 0.0,
            _ => false,
        });
        out.directional_released = !direction_held;

        out
    }

    /// Start a force-feedback rumble on the active gamepad: `strong`/`weak` are
    /// the strong/weak motor magnitudes in `[0, 1]`, `duration_ms` the play
    /// length. An absent `weak` mirrors `strong` (the system-command contract).
    /// A fresh rumble replaces any in-flight one (latest wins).
    ///
    /// No-ops (warn-once) when there is no active gamepad or the active gamepad's
    /// backend does not support force feedback — vibration is best-effort, never
    /// an error. Driven by the drained `Rumble` system-reaction command.
    pub fn rumble(&mut self, strong: f32, weak: Option<f32>, duration_ms: f32) {
        let Some(gamepad_id) = self.active_gamepad else {
            // No gamepad has produced input yet; nothing to vibrate.
            self.warn_ff_once("no active gamepad");
            return;
        };

        if !self.gilrs.gamepad(gamepad_id).is_ff_supported() {
            self.warn_ff_once("active gamepad does not support force feedback");
            return;
        }

        if !(duration_ms.is_finite() && duration_ms > 0.0) {
            log::warn!("[Input] rumble ignored: non-positive/non-finite durationMs {duration_ms}");
            return;
        }

        let strong_mag = magnitude_u16(strong);
        // `weak` absent ⇒ mirror `strong`, per the Rumble command contract.
        let weak_mag = magnitude_u16(weak.unwrap_or(strong));
        let play_for = Ticks::from_ms(duration_ms.max(0.0) as u32);

        let effect = EffectBuilder::new()
            .add_effect(BaseEffect {
                kind: BaseEffectType::Strong {
                    magnitude: strong_mag,
                },
                scheduling: Replay {
                    play_for,
                    ..Default::default()
                },
                envelope: Default::default(),
            })
            .add_effect(BaseEffect {
                kind: BaseEffectType::Weak {
                    magnitude: weak_mag,
                },
                scheduling: Replay {
                    play_for,
                    ..Default::default()
                },
                envelope: Default::default(),
            })
            .gamepads(&[gamepad_id])
            .finish(&mut self.gilrs);

        let effect = match effect {
            Ok(effect) => effect,
            Err(err) => {
                self.warn_ff_once(&format!("effect build failed: {err}"));
                return;
            }
        };

        if let Err(err) = effect.play() {
            self.warn_ff_once(&format!("effect play failed: {err}"));
            return;
        }

        // Replacing `active_rumble` drops the previous effect handle, stopping
        // any prior vibration so the new one is the only force feedback playing.
        self.active_rumble = Some(ActiveRumble {
            effect,
            remaining_ms: duration_ms,
        });
    }

    /// Advance the active rumble's timeout by the frame delta (seconds) and stop
    /// it once its duration elapses. Called once per frame in the input stage,
    /// where the rumble duration timeout is tracked. A no-op when nothing is
    /// rumbling.
    pub fn tick_rumble(&mut self, dt: f32) {
        let Some(rumble) = self.active_rumble.as_mut() else {
            return;
        };
        rumble.remaining_ms -= dt * 1000.0;
        if rumble.remaining_ms <= 0.0 {
            // Stop explicitly, then drop the handle. gilrs's `play_for` already
            // bounds the motor output, but stopping releases the effect promptly
            // rather than waiting on the server's own scheduling.
            let _ = rumble.effect.stop();
            self.active_rumble = None;
        }
    }

    /// Log the force-feedback unsupported/no-op warning at most once. Subsequent
    /// no-ops are silent so a rumble-heavy script does not spam the log on a
    /// gamepad-less or ff-less machine.
    fn warn_ff_once(&mut self, reason: &str) {
        if !self.ff_warned {
            log::warn!("[Input] rumble no-op: {reason} (force feedback unavailable)");
            self.ff_warned = true;
        }
    }
}

/// Map a force-feedback motor magnitude in `[0, 1]` to gilrs's `u16` motor
/// range. Out-of-range or non-finite inputs clamp into `[0, 1]` first so a stray
/// command can never wrap the cast.
fn magnitude_u16(value: f32) -> u16 {
    let clamped = if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    };
    (clamped * u16::MAX as f32).round() as u16
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
    use crate::input::system::DEFAULT_STICK_DEAD_ZONE as DEAD_ZONE;

    #[test]
    fn text_shortcuts_resolve_only_in_the_text_entry_context() {
        let nav = crate::input::BindingState::default().ui_nav().clone();
        let west = PhysicalInput::GamepadButton(Button::West);
        assert_eq!(
            text_shortcut(&nav, west, UiNavContext::TextEntry),
            Some(Command::TextBackspace)
        );
        for context in [UiNavContext::Open, UiNavContext::Capture] {
            assert_eq!(text_shortcut(&nav, west, context), None, "{context:?}");
        }
        assert_eq!(
            text_shortcut(
                &nav,
                PhysicalInput::GamepadButton(Button::South),
                UiNavContext::TextEntry
            ),
            None,
            "confirm is not a shortcut"
        );
    }

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
        assert!(approx_eq(x, expected), "expected {expected}, got {x}");
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
        assert!(
            approx_eq(x, y),
            "diagonal should be symmetric: x={x}, y={y}"
        );
    }

    #[test]
    fn dead_zone_handles_zero_input() {
        let (x, y) = apply_radial_dead_zone(0.0, 0.0, DEAD_ZONE);
        assert_eq!(x, 0.0);
        assert_eq!(y, 0.0);
    }

    // --- Directional-release edge tests ---

    // --- Trigger threshold tests ---

    #[test]
    fn trigger_below_threshold_is_inactive() {
        assert!(!trigger_is_active(false, 0.3));
    }

    #[test]
    fn trigger_at_threshold_is_active() {
        assert!(trigger_is_active(false, TRIGGER_PRESS_THRESHOLD));
    }

    #[test]
    fn trigger_above_threshold_is_active() {
        assert!(trigger_is_active(false, 0.8));
    }

    #[test]
    fn a_trigger_resting_inside_the_hysteresis_band_presses_once() {
        // Regression: one threshold made a trigger resting near it chatter, so
        // Shoot alternated press and release every poll.
        let mut down = false;
        let mut presses = 0;
        for value in [0.0, 0.52, 0.47, 0.51, 0.45, 0.49, 0.42, 0.5, 0.41] {
            let now = trigger_is_active(down, value);
            if now && !down {
                presses += 1;
            }
            down = now;
        }
        assert_eq!(presses, 1);
        assert!(down, "still pressed above the release threshold");
        assert!(!trigger_is_active(true, 0.39), "releases below it");
    }

    // --- Trigger value sourcing tests ---

    #[test]
    fn trigger_button_value_alone_is_active() {
        assert!(trigger_is_active(false, trigger_value(Some(1.0), None)));
    }

    #[test]
    fn trigger_axis_value_alone_is_active() {
        assert!(trigger_is_active(false, trigger_value(None, Some(1.0))));
    }

    #[test]
    fn trigger_without_readings_is_zero() {
        assert_eq!(trigger_value(None, None), 0.0);
    }

    #[test]
    fn trigger_zero_button_value_is_inactive() {
        assert!(!trigger_is_active(false, trigger_value(Some(0.0), None)));
    }

    #[test]
    fn trigger_partial_values_below_threshold_are_inactive() {
        assert!(!trigger_is_active(false, trigger_value(Some(0.3), None)));
        assert!(!trigger_is_active(false, trigger_value(None, Some(0.3))));
        assert!(!trigger_is_active(
            false,
            trigger_value(Some(0.2), Some(0.4))
        ));
    }

    #[test]
    fn trigger_takes_the_larger_reading_and_clamps_to_unit_range() {
        assert_eq!(trigger_value(Some(0.25), Some(0.75)), 0.75);
        assert_eq!(trigger_value(None, Some(-1.0)), 0.0);
        assert_eq!(trigger_value(Some(1.5), None), 1.0);
    }

    #[test]
    fn active_right_trigger_button_activates_shoot() {
        use crate::input::types::{Action, Binding, ButtonState};
        let mut sys = InputSystem::new(vec![Binding::new(
            PhysicalInput::GamepadButton(Button::RightTrigger2),
            Action::Shoot,
        )]);
        sys.set_physical_input(
            PhysicalInput::GamepadButton(Button::RightTrigger2),
            trigger_is_active(false, trigger_value(Some(1.0), None)),
        );
        assert_eq!(sys.snapshot().button(Action::Shoot), ButtonState::Pressed);
    }

    // --- Rumble magnitude mapping tests ---

    #[test]
    fn magnitude_maps_unit_range_to_u16_endpoints() {
        assert_eq!(magnitude_u16(0.0), 0);
        assert_eq!(magnitude_u16(1.0), u16::MAX);
        // Midpoint rounds to ~half scale.
        assert_eq!(magnitude_u16(0.5), (u16::MAX as f32 * 0.5).round() as u16);
    }

    #[test]
    fn magnitude_clamps_out_of_range_and_non_finite() {
        assert_eq!(magnitude_u16(2.0), u16::MAX, "above 1.0 clamps to full");
        assert_eq!(magnitude_u16(-1.0), 0, "below 0.0 clamps to zero");
        assert_eq!(magnitude_u16(f32::NAN), 0, "NaN coerces to zero");
        assert_eq!(magnitude_u16(f32::INFINITY), 0, "infinity coerces to zero");
    }
}
