use std::time::{Duration, Instant};

use winit::event::{MouseButton, MouseScrollDelta};
use winit::keyboard::KeyCode;

use super::scroll::{LINE_SCROLL_GESTURE_REPEAT, wheel_diagnostics_enabled_from};
use super::*;

/// Returns the default keyboard/mouse bindings for use in tests.
fn test_bindings() -> Vec<Binding> {
    defaults::default_bindings_for(input_names::DeviceClass::KeyboardMouse)
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
    assert!((yaw - (-0.02)).abs() < 1e-6, "expected -0.02, got {}", yaw);

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

#[test]
fn line_scroll_gesture_caps_an_accelerated_event_at_one_notch() {
    let mut sys = InputSystem::new(test_bindings());
    let now = Instant::now();
    sys.handle_mouse_wheel_at(MouseScrollDelta::LineDelta(0.0, -16.389_328), now);

    let snap = sys.snapshot();
    assert_eq!(snap.notch_count(Action::CycleWieldableNext), 1);
    assert_eq!(
        snap.button(Action::CycleWieldableNext),
        ButtonState::Pressed,
        "line-scroll magnitude represents accelerated motion, not a weapon-step count"
    );

    let next = sys.snapshot();
    assert_eq!(next.notch_count(Action::CycleWieldableNext), 0);
    assert_eq!(
        next.button(Action::CycleWieldableNext),
        ButtonState::Released,
        "wheel physical state must be explicitly cleared because winit emits no release event"
    );
}

#[test]
fn line_scroll_gesture_repeats_only_every_128_ms_while_input_continues() {
    let mut sys = InputSystem::new(test_bindings());
    let now = Instant::now();

    sys.handle_mouse_wheel_at(MouseScrollDelta::LineDelta(0.0, 0.1), now);
    assert_eq!(
        sys.snapshot().notch_count(Action::CycleWieldablePrevious),
        1,
        "a gesture steps immediately"
    );

    sys.handle_mouse_wheel_at(
        MouseScrollDelta::LineDelta(0.0, 12.4),
        now + Duration::from_millis(64),
    );
    assert_eq!(
        sys.snapshot().notch_count(Action::CycleWieldablePrevious),
        0,
        "a sustained gesture is debounced between repeat intervals"
    );

    sys.handle_mouse_wheel_at(
        MouseScrollDelta::LineDelta(0.0, 0.1),
        now + LINE_SCROLL_GESTURE_REPEAT,
    );
    assert_eq!(
        sys.snapshot().notch_count(Action::CycleWieldablePrevious),
        1,
        "continued same-direction input repeats on the configured cadence"
    );
}

#[test]
fn line_scroll_gesture_direction_change_starts_a_new_gesture() {
    let mut sys = InputSystem::new(test_bindings());
    let now = Instant::now();
    sys.handle_mouse_wheel_at(MouseScrollDelta::LineDelta(0.0, 0.1), now);
    let _ = sys.snapshot();

    sys.handle_mouse_wheel_at(
        MouseScrollDelta::LineDelta(0.0, -0.1),
        now + Duration::from_millis(1),
    );
    assert_eq!(
        sys.snapshot().notch_count(Action::CycleWieldableNext),
        1,
        "reversing direction must not wait for the prior gesture's debounce"
    );
}

#[test]
fn pixel_scroll_accumulates_against_configured_notch_threshold() {
    let mut sys = InputSystem::new(test_bindings());
    sys.set_scroll_notch_pixels(120.0);
    sys.handle_mouse_wheel(MouseScrollDelta::PixelDelta(
        winit::dpi::PhysicalPosition::new(0.0, 50.0),
    ));
    assert_eq!(
        sys.snapshot().notch_count(Action::CycleWieldablePrevious),
        0
    );

    sys.handle_mouse_wheel(MouseScrollDelta::PixelDelta(
        winit::dpi::PhysicalPosition::new(0.0, 70.0),
    ));
    assert_eq!(
        sys.snapshot().notch_count(Action::CycleWieldablePrevious),
        1,
        "pixel residual from the prior frame completes one 120-pixel notch"
    );
}

#[test]
fn wheel_diagnostics_enable_only_for_explicit_one() {
    assert!(wheel_diagnostics_enabled_from(Some("1")));
    assert!(!wheel_diagnostics_enabled_from(None));
    assert!(!wheel_diagnostics_enabled_from(Some("true")));
    assert!(!wheel_diagnostics_enabled_from(Some("0")));
}

#[test]
fn gameplay_input_latch_carries_pressed_button_across_zero_tick_frame() {
    let mut sys = InputSystem::new(test_bindings());
    let mut latch = GameplayInputLatch::new();

    sys.handle_mouse_button(MouseButton::Left, true);
    let zero_tick_snapshot = sys.snapshot();
    assert_eq!(
        zero_tick_snapshot.button(Action::Shoot),
        ButtonState::Pressed
    );
    assert!(latch.snapshot_for_ticks(&zero_tick_snapshot, 0).is_none());

    let tick_frame_snapshot = sys.snapshot();
    assert_eq!(tick_frame_snapshot.button(Action::Shoot), ButtonState::Held);

    let gameplay_snapshot = latch
        .snapshot_for_ticks(&tick_frame_snapshot, 1)
        .expect("fixed tick should receive a gameplay snapshot");
    assert_eq!(
        gameplay_snapshot.button(Action::Shoot),
        ButtonState::Pressed
    );

    let later_snapshot = sys.snapshot();
    let later_gameplay = latch
        .snapshot_for_ticks(&later_snapshot, 1)
        .expect("fixed tick should receive a gameplay snapshot");
    assert_eq!(later_gameplay.button(Action::Shoot), ButtonState::Held);
}

#[test]
fn gameplay_input_latch_preserves_quick_tap_until_fixed_tick_consumes_it() {
    let mut sys = InputSystem::new(test_bindings());
    let mut latch = GameplayInputLatch::new();

    sys.handle_mouse_button(MouseButton::Left, true);
    let pressed = sys.snapshot();
    assert!(latch.snapshot_for_ticks(&pressed, 0).is_none());

    sys.handle_mouse_button(MouseButton::Left, false);
    let released = sys.snapshot();
    assert_eq!(released.button(Action::Shoot), ButtonState::Released);
    assert!(latch.snapshot_for_ticks(&released, 0).is_none());

    let inactive = sys.snapshot();
    assert_eq!(inactive.button(Action::Shoot), ButtonState::Inactive);

    let gameplay_snapshot = latch
        .snapshot_for_ticks(&inactive, 1)
        .expect("fixed tick should receive a gameplay snapshot");
    assert_eq!(
        gameplay_snapshot.button(Action::Shoot),
        ButtonState::Pressed
    );
}

#[test]
fn gameplay_input_latch_coalesces_zero_tick_drop_presses_into_one_edge() {
    let mut latch = GameplayInputLatch::new();
    let drop_press = ActionSnapshot::with_button_state(Action::Drop, ButtonState::Pressed);

    assert!(latch.snapshot_for_ticks(&drop_press, 0).is_none());
    assert!(latch.snapshot_for_ticks(&drop_press, 0).is_none());

    let gameplay_snapshot = latch
        .snapshot_for_ticks(&ActionSnapshot::neutral(), 2)
        .expect("fixed ticks should receive a gameplay snapshot");
    assert_eq!(
        gameplay_snapshot.button(Action::Drop),
        ButtonState::Pressed,
        "consecutive zero-tick presses collapse into one tick-zero drop edge"
    );

    let following_snapshot = latch
        .snapshot_for_ticks(&ActionSnapshot::neutral(), 1)
        .expect("later fixed ticks still receive a gameplay snapshot");
    assert_eq!(
        following_snapshot.button(Action::Drop),
        ButtonState::Inactive,
        "the latched drop edge drains onto the first tick-bearing frame only"
    );
}

#[test]
fn gameplay_input_latch_clear_discards_pending_wieldable_commit() {
    let mut latch = GameplayInputLatch::new();
    let occupied = [true, true];
    latch.wieldable_selection_mut().advance_frame(
        &ActionSnapshot::with_button_state(Action::SelectWieldable2, ButtonState::Pressed),
        &occupied,
        Some(0),
        WieldableSelectionPolicy {
            commit_on_direct_select: true,
            cycle_dwell_ms: 0.0,
        },
        0.0,
    );

    latch.clear();
    assert_eq!(
        latch
            .wieldable_selection_mut()
            .take_pending_commit(&occupied, Some(0)),
        None,
        "modal/focus input clears must discard the local declaration holder"
    );
}

// --- Cross-source additive resolution ---

#[test]
fn input_system_combines_mouse_displacement_and_gamepad_velocity_additively() {
    let mut bindings = test_bindings();
    // Add a gamepad binding for LookYaw so gamepad axis resolves to the action.
    bindings.push(Binding::with_scale(
        PhysicalInput::GamepadAxis(gilrs::Axis::RightStickX),
        Action::LookYaw,
        1.0,
    ));
    let mut sys = InputSystem::new(bindings);

    // Mouse contributes displacement to LookYaw.
    sys.handle_mouse_delta(10.0, 0.0);

    // Gamepad contributes velocity to LookYaw via raw axis.
    sys.set_gamepad_axis(gilrs::Axis::RightStickX, 0.5);

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

// --- drain_look_inputs ---

#[test]
fn drain_look_inputs_returns_mouse_displacement() {
    let mut sys = InputSystem::new(test_bindings());
    sys.handle_mouse_delta(10.0, -5.0);

    let look = sys.drain_look_inputs();

    // Yaw:  10.0 * 0.002 * -1.0 = -0.02
    // Pitch: -5.0 * 0.002 * -1.0 =  0.01
    assert!(
        (look.yaw_displacement - (-0.02)).abs() < 1e-6,
        "expected -0.02, got {}",
        look.yaw_displacement
    );
    assert!(
        (look.pitch_displacement - 0.01).abs() < 1e-6,
        "expected 0.01, got {}",
        look.pitch_displacement
    );
    assert!(look.yaw_velocity.abs() < f32::EPSILON);
    assert!(look.pitch_velocity.abs() < f32::EPSILON);
}

#[test]
fn seeded_sensitivity_and_invert_y_apply_to_look_output() {
    // Boot seam: main.rs reads PlayerOptions and calls these setters on
    // InputSystem at startup. This test does not exercise that wiring —
    // it pins that the setters themselves have the expected effect on look
    // output: doubled sensitivity scales the magnitude; invert_y flips the
    // pitch sign.
    let mut sys = InputSystem::new(test_bindings());
    sys.set_mouse_sensitivity(DEFAULT_MOUSE_SENSITIVITY * 2.0);
    sys.set_invert_y(true);

    sys.handle_mouse_delta(10.0, -5.0);
    let look = sys.drain_look_inputs();

    // Yaw is unaffected by invert_y; magnitude doubles vs. the default-
    // sensitivity case (-0.02 → -0.04). Pitch doubles and flips:
    // default-sensitivity non-inverted is 0.01, so here -0.02.
    assert!(
        (look.yaw_displacement - (-0.04)).abs() < 1e-6,
        "expected -0.04, got {}",
        look.yaw_displacement
    );
    assert!(
        (look.pitch_displacement - (-0.02)).abs() < 1e-6,
        "expected -0.02, got {}",
        look.pitch_displacement
    );
}

#[test]
fn drain_look_inputs_returns_gamepad_velocity() {
    let mut bindings = test_bindings();
    // Bind right stick to look axes so gamepad state resolves through bindings.
    bindings.push(Binding::with_scale(
        PhysicalInput::GamepadAxis(gilrs::Axis::RightStickX),
        Action::LookYaw,
        1.0,
    ));
    bindings.push(Binding::with_scale(
        PhysicalInput::GamepadAxis(gilrs::Axis::RightStickY),
        Action::LookPitch,
        -1.0,
    ));
    let mut sys = InputSystem::new(bindings);

    sys.set_gamepad_axis(gilrs::Axis::RightStickX, 0.5);
    sys.set_gamepad_axis(gilrs::Axis::RightStickY, 0.25);

    let look = sys.drain_look_inputs();

    // Resolution walks the binding table: raw * scale.
    assert!(
        (look.yaw_velocity - 0.5).abs() < 1e-6,
        "expected 0.5, got {}",
        look.yaw_velocity
    );
    assert!(
        (look.pitch_velocity - (-0.25)).abs() < 1e-6,
        "expected -0.25, got {}",
        look.pitch_velocity
    );
    assert!(look.yaw_displacement.abs() < f32::EPSILON);
    assert!(look.pitch_displacement.abs() < f32::EPSILON);
}

#[test]
fn drain_look_inputs_clears_mouse_displacement_for_subsequent_snapshot() {
    let mut sys = InputSystem::new(test_bindings());
    sys.handle_mouse_delta(10.0, -5.0);

    let _ = sys.drain_look_inputs();

    // A same-frame snapshot must not re-emit the mouse displacement.
    let snap = sys.snapshot();
    assert!(
        snap.axis(Action::LookYaw).is_empty(),
        "expected no LookYaw entries after drain, got {:?}",
        snap.axis(Action::LookYaw)
    );
    assert!(
        snap.axis(Action::LookPitch).is_empty(),
        "expected no LookPitch entries after drain, got {:?}",
        snap.axis(Action::LookPitch)
    );
}

#[test]
fn drain_look_inputs_leaves_gamepad_velocity_in_subsequent_snapshot() {
    // Pin the intentional non-clearing of gamepad_axes: stick deflection
    // is persistent, so a snapshot() after drain still sees it. The
    // render loop does not consume look axes from snapshot(), so the
    // re-emission has no reader and is harmless.
    let mut bindings = test_bindings();
    bindings.push(Binding::with_scale(
        PhysicalInput::GamepadAxis(gilrs::Axis::RightStickX),
        Action::LookYaw,
        1.0,
    ));
    let mut sys = InputSystem::new(bindings);

    sys.set_gamepad_axis(gilrs::Axis::RightStickX, 0.75);

    let look = sys.drain_look_inputs();
    assert!((look.yaw_velocity - 0.75).abs() < 1e-6);

    let snap = sys.snapshot();
    let yaw = snap.axis(Action::LookYaw);
    assert_eq!(yaw.len(), 1, "expected one Velocity entry, got {:?}", yaw);
    assert_eq!(yaw[0].source, AxisSource::Velocity);
    assert!((yaw[0].value - 0.75).abs() < 1e-6);
}

#[test]
fn drain_look_inputs_does_not_clear_movement_axes() {
    let mut sys = InputSystem::new(test_bindings());
    sys.handle_keyboard_event(KeyCode::KeyW, true);
    sys.handle_mouse_delta(10.0, 0.0);

    let look = sys.drain_look_inputs();
    assert!((look.yaw_displacement - (-0.02)).abs() < 1e-6);

    // W is still held — movement must resolve normally in the snapshot.
    let snap = sys.snapshot();
    assert!(
        (snap.axis_value(Action::MoveForward) - 1.0).abs() < f32::EPSILON,
        "MoveForward should still be active after drain"
    );
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

#[test]
fn clear_all_lifts_a_held_action_to_inactive_not_released() {
    let mut sys = InputSystem::new(test_bindings());
    sys.handle_keyboard_event(KeyCode::Space, true);
    let _ = sys.snapshot();
    assert_eq!(sys.snapshot().button(Action::Jump), ButtonState::Held);
    sys.clear_all();
    assert_eq!(sys.snapshot().button(Action::Jump), ButtonState::Inactive);
}

// --- Gamepad edges ---

fn pad(button: gilrs::Button) -> PhysicalInput {
    PhysicalInput::GamepadButton(button)
}

#[test]
fn a_pad_lost_mid_hold_releases_its_inputs_and_fires_no_pending_hold() {
    let stick_up =
        PhysicalInput::GamepadAxisHalf(gilrs::Axis::LeftStickY, types::AxisHalf::Positive);
    let mut sys = InputSystem::new(vec![
        Binding::new(pad(gilrs::Button::South), Action::Jump),
        Binding::new(pad(gilrs::Button::East), Action::Sprint)
            .with_activator(Activator::with_threshold(ActivatorKind::Hold, 0.2)),
        Binding::new(stick_up, Action::MoveForward),
        Binding::new(PhysicalInput::Key(KeyCode::Space), Action::Use),
    ]);
    sys.set_physical_input_at(pad(gilrs::Button::South), true, 0.0);
    sys.set_physical_input_at(pad(gilrs::Button::East), true, 0.0);
    sys.set_gamepad_axis_at(gilrs::Axis::LeftStickY, 0.8, 0.0);
    sys.handle_keyboard_event_at(KeyCode::Space, true, 0.0);
    let held = sys.snapshot_at(0.05);
    assert_eq!(held.button(Action::Jump), ButtonState::Pressed);
    assert!((held.axis_value(Action::MoveForward) - 0.8).abs() < 1e-6);

    sys.release_gamepad();
    let gone = sys.snapshot_at(0.5);
    assert!(!gone.button(Action::Jump).is_active());
    assert!(
        !gone.button(Action::Sprint).is_active(),
        "the pending hold never fires"
    );
    assert_eq!(gone.axis_value(Action::MoveForward), 0.0);
    assert_eq!(
        gone.button(Action::Use),
        ButtonState::Held,
        "keyboard state is untouched"
    );

    // A button still held when the pad reconnects and claims the role with
    // another input stays inert until it is released and pressed again.
    sys.switch_gamepad_at(&[pad(gilrs::Button::South)], 1.0);
    sys.set_physical_input_at(pad(gilrs::Button::South), true, 1.0);
    assert_eq!(
        sys.snapshot_at(1.01).button(Action::Jump),
        ButtonState::Inactive
    );
    sys.set_physical_input_at(pad(gilrs::Button::South), false, 1.1);
    let _ = sys.snapshot_at(1.11);
    sys.set_physical_input_at(pad(gilrs::Button::South), true, 1.2);
    assert_eq!(
        sys.snapshot_at(1.21).button(Action::Jump),
        ButtonState::Pressed
    );
}

#[test]
fn a_stick_released_while_its_pad_is_away_moves_on_the_next_push() {
    // A stick reports no events, so its suppression lifts only on a polled
    // release; the claim must seed the level that release is read against.
    let stick_up =
        PhysicalInput::GamepadAxisHalf(gilrs::Axis::LeftStickY, types::AxisHalf::Positive);
    let mut sys = InputSystem::new(vec![Binding::new(stick_up, Action::MoveForward)]);
    sys.set_gamepad_axis_at(gilrs::Axis::LeftStickY, 0.8, 0.0);
    let _ = sys.snapshot_at(0.01);
    sys.release_gamepad();

    // Reconnected still pushed: inert.
    sys.switch_gamepad_at(&[stick_up], 1.0);
    sys.set_gamepad_axis_at(gilrs::Axis::LeftStickY, 0.8, 1.0);
    assert_eq!(sys.snapshot_at(1.01).axis_value(Action::MoveForward), 0.0);
    sys.set_gamepad_axis_at(gilrs::Axis::LeftStickY, 0.0, 1.1);
    let _ = sys.snapshot_at(1.11);
    sys.set_gamepad_axis_at(gilrs::Axis::LeftStickY, 0.8, 1.2);
    assert!((sys.snapshot_at(1.21).axis_value(Action::MoveForward) - 0.8).abs() < 1e-6);
}

#[test]
fn a_pad_lost_while_holding_shoot_lifts_it_to_neutral_never_a_release() {
    // Regression: the disconnect left the previous state Held, so the next
    // snapshot read Released and a charge weapon fired on the disconnect.
    let trigger = pad(gilrs::Button::RightTrigger2);
    let mut sys = InputSystem::new(vec![
        Binding::new(trigger, Action::Shoot),
        Binding::new(pad(gilrs::Button::East), Action::AltFire),
        Binding::new(PhysicalInput::Key(KeyCode::KeyF), Action::AltFire),
    ]);
    let mut capture = ActivationInputCapture::default();
    sys.set_physical_input_at(trigger, true, 0.0);
    capture.observe(&sys.snapshot_at(0.01));
    assert!(capture.command(1).initiation.is_some());
    sys.set_physical_input_at(pad(gilrs::Button::East), true, 0.02);
    sys.handle_keyboard_event_at(KeyCode::KeyF, true, 0.02);
    let held = sys.snapshot_at(0.03);
    assert_eq!(held.button(Action::Shoot), ButtonState::Held);
    capture.observe(&held);

    assert_eq!(
        sys.release_gamepad(),
        vec![Action::Shoot],
        "AltFire stays down on the keyboard"
    );
    let gone = sys.snapshot_at(0.04);
    assert_eq!(gone.button(Action::Shoot), ButtonState::Inactive);
    assert_eq!(gone.button(Action::AltFire), ButtonState::Held);
    capture.observe(&gone);
    assert!(
        capture.command(2).release.is_none(),
        "a disconnect is never a charge release"
    );
}

// --- Switching the active pad ---

#[test]
fn switching_pads_never_refires_a_button_the_first_pad_kept_down() {
    // Regression: a second pad took the active role, its poll read South up,
    // and switching back read the first pad's still-held South as a new press.
    let south = pad(gilrs::Button::South);
    let mut sys = InputSystem::new(vec![Binding::new(south, Action::Jump)]);
    sys.set_physical_input_at(south, true, 0.0);
    assert_eq!(
        sys.snapshot_at(0.01).button(Action::Jump),
        ButtonState::Pressed
    );

    // Pad B takes the role while pad A holds South.
    assert_eq!(sys.switch_gamepad_at(&[], 0.1), vec![Action::Jump]);
    sys.set_physical_input_at(south, false, 0.1);
    assert_eq!(
        sys.snapshot_at(0.11).button(Action::Jump),
        ButtonState::Inactive,
        "lifted without a release"
    );

    // Pad A takes the role back with another input, South still down.
    sys.switch_gamepad_at(&[south], 0.2);
    sys.set_physical_input_at(south, true, 0.2);
    for t in [0.21, 0.3] {
        assert_eq!(
            sys.snapshot_at(t).button(Action::Jump),
            ButtonState::Inactive
        );
    }

    // Released and pressed again, it jumps.
    sys.set_physical_input_at(south, false, 0.4);
    let _ = sys.snapshot_at(0.41);
    sys.set_physical_input_at(south, true, 0.5);
    assert_eq!(
        sys.snapshot_at(0.51).button(Action::Jump),
        ButtonState::Pressed
    );
}

#[test]
fn a_pad_taking_the_role_moves_from_its_first_push() {
    // Only inputs the outgoing pad held wait for a release: the stick that
    // claimed the role is not one of them.
    let stick_up =
        PhysicalInput::GamepadAxisHalf(gilrs::Axis::LeftStickY, types::AxisHalf::Positive);
    let south = pad(gilrs::Button::South);
    let mut sys = InputSystem::new(vec![
        Binding::new(stick_up, Action::MoveForward),
        Binding::new(south, Action::Jump),
    ]);
    sys.set_physical_input_at(south, true, 0.0);
    let _ = sys.snapshot_at(0.01);
    sys.switch_gamepad_at(&[], 0.1);
    sys.set_physical_input_at(south, false, 0.1);
    sys.set_gamepad_axis_at(gilrs::Axis::LeftStickY, 0.8, 0.1);
    assert!((sys.snapshot_at(0.11).axis_value(Action::MoveForward) - 0.8).abs() < 1e-6);
}

// Regression: the switch kept the outgoing pad's South level down, so the
// press that claimed the role back recorded no edge and did nothing.
#[test]
fn a_pad_reclaiming_the_role_with_a_button_it_released_while_idle_presses_it() {
    let south = gilrs::Button::South;
    let mut sys = InputSystem::new(vec![Binding::new(pad(south), Action::Jump)]);
    sys.handle_gamepad_button_event_at(south, true, 0.0);
    assert_eq!(
        sys.snapshot_at(0.01).button(Action::Jump),
        ButtonState::Pressed
    );

    // Pad B claims with a button of its own while pad A holds South; A then
    // releases South while idle, which reaches nothing.
    assert_eq!(sys.switch_gamepad_at(&[], 0.1), vec![Action::Jump]);
    assert_eq!(
        sys.snapshot_at(0.11).button(Action::Jump),
        ButtonState::Inactive
    );

    // Pad A presses South to claim the role back: the claiming press acts.
    sys.switch_gamepad_at(&[], 0.2);
    sys.handle_gamepad_button_event_at(south, true, 0.2);
    assert_eq!(
        sys.snapshot_at(0.21).button(Action::Jump),
        ButtonState::Pressed
    );
}

// Regression: a disconnect kept the held South level down, so after a release
// while unplugged the first press on reconnect recorded no edge.
#[test]
fn a_button_released_while_its_pad_was_unplugged_presses_on_reconnect() {
    let south = gilrs::Button::South;
    let mut sys = InputSystem::new(vec![Binding::new(pad(south), Action::Jump)]);
    sys.handle_gamepad_button_event_at(south, true, 0.0);
    let _ = sys.snapshot_at(0.01);
    assert_eq!(sys.release_gamepad(), vec![Action::Jump]);
    assert_eq!(
        sys.snapshot_at(0.5).button(Action::Jump),
        ButtonState::Inactive
    );

    // Reconnected, the pad claims the role with a South press.
    sys.switch_gamepad_at(&[], 1.0);
    sys.handle_gamepad_button_event_at(south, true, 1.0);
    sys.set_physical_input_at(pad(south), true, 1.0);
    assert_eq!(
        sys.snapshot_at(1.01).button(Action::Jump),
        ButtonState::Pressed
    );
}

// Regression: the incoming pad's press of a button the outgoing pad still held
// was suppressed as if the incoming pad had held it through the switch.
#[test]
fn a_pad_claiming_with_a_button_the_outgoing_pad_holds_presses_it() {
    let south = gilrs::Button::South;
    let mut sys = InputSystem::new(vec![Binding::new(pad(south), Action::Jump)]);
    sys.handle_gamepad_button_event_at(south, true, 0.0);
    let _ = sys.snapshot_at(0.01);
    assert_eq!(sys.snapshot_at(0.1).button(Action::Jump), ButtonState::Held);

    // Pad B presses South, claiming the role while pad A still holds it.
    assert_eq!(sys.switch_gamepad_at(&[], 0.2), vec![Action::Jump]);
    sys.handle_gamepad_button_event_at(south, true, 0.2);
    sys.set_physical_input_at(pad(south), true, 0.2);
    assert_eq!(
        sys.snapshot_at(0.21).button(Action::Jump),
        ButtonState::Pressed
    );
}

#[test]
fn a_button_the_incoming_pad_holds_stays_inert_through_its_event_release() {
    let south = gilrs::Button::South;
    let mut sys = InputSystem::new(vec![
        Binding::new(pad(south), Action::Jump),
        Binding::new(pad(gilrs::Button::East), Action::Use)
            .with_activator(Activator::new(ActivatorKind::Release)),
    ]);
    sys.switch_gamepad_at(&[pad(south), pad(gilrs::Button::East)], 0.0);
    sys.set_physical_input_at(pad(south), true, 0.0);
    sys.set_physical_input_at(pad(gilrs::Button::East), true, 0.0);
    assert_eq!(
        sys.snapshot_at(0.01).button(Action::Jump),
        ButtonState::Inactive
    );

    sys.handle_gamepad_button_event_at(south, false, 0.1);
    sys.handle_gamepad_button_event_at(gilrs::Button::East, false, 0.1);
    let released = sys.snapshot_at(0.11);
    assert_eq!(released.button(Action::Jump), ButtonState::Inactive);
    assert_eq!(
        released.button(Action::Use),
        ButtonState::Inactive,
        "a release-bound command never fires on a held-through release"
    );

    sys.handle_gamepad_button_event_at(south, true, 0.2);
    assert_eq!(
        sys.snapshot_at(0.21).button(Action::Jump),
        ButtonState::Pressed
    );
}

#[test]
fn a_stick_the_incoming_pad_releases_before_its_first_poll_moves_on_the_next_push() {
    let stick_up =
        PhysicalInput::GamepadAxisHalf(gilrs::Axis::LeftStickY, types::AxisHalf::Positive);
    let mut sys = InputSystem::new(vec![Binding::new(stick_up, Action::MoveForward)]);
    sys.switch_gamepad_at(&[stick_up], 0.0);
    sys.set_gamepad_axis_at(gilrs::Axis::LeftStickY, 0.0, 0.0);
    let _ = sys.snapshot_at(0.01);
    sys.set_gamepad_axis_at(gilrs::Axis::LeftStickY, 0.8, 0.1);
    assert!((sys.snapshot_at(0.11).axis_value(Action::MoveForward) - 0.8).abs() < 1e-6);
}

// Regression: a press resolved by the switch's own edge flush stayed in the
// frame's press record, so the lifted command read Pressed, then Released.
#[test]
fn a_press_resolved_in_the_drain_that_switches_pads_never_reads_pressed() {
    let south = gilrs::Button::South;
    let mut sys = InputSystem::new(vec![Binding::new(pad(south), Action::Jump)]);
    sys.handle_gamepad_button_event_at(south, true, 0.0);
    assert_eq!(sys.switch_gamepad_at(&[], 0.01), vec![Action::Jump]);
    for t in [0.02, 0.03] {
        assert_eq!(
            sys.snapshot_at(t).button(Action::Jump),
            ButtonState::Inactive
        );
    }
}

// --- Wheel capture ---

#[test]
fn trackpad_travel_short_of_a_notch_never_reaches_the_capture_prompt() {
    let mut sys = InputSystem::new(test_bindings());
    sys.set_scroll_notch_pixels(120.0);
    let pixels = |y: f64| MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition::new(0.0, y));
    assert_eq!(sys.capture_wheel_notch(pixels(40.0)), None);
    assert_eq!(sys.capture_wheel_notch(pixels(40.0)), None);
    assert_eq!(
        sys.capture_wheel_notch(pixels(-30.0)),
        None,
        "reversing drops the partial travel"
    );
    assert_eq!(sys.capture_wheel_notch(pixels(-80.0)), None);
    assert_eq!(
        sys.capture_wheel_notch(pixels(-20.0)),
        Some(PhysicalInput::MouseWheelDown)
    );
    sys.reset_capture_wheel();
    assert_eq!(sys.capture_wheel_notch(pixels(-110.0)), None);
}

#[test]
fn a_wheel_click_reaches_the_capture_prompt_as_one_notch() {
    let mut sys = InputSystem::new(test_bindings());
    assert_eq!(
        sys.capture_wheel_notch(MouseScrollDelta::LineDelta(0.0, 1.0)),
        Some(PhysicalInput::MouseWheelUp)
    );
    assert_eq!(
        sys.capture_wheel_notch(MouseScrollDelta::LineDelta(0.0, 0.5)),
        None
    );
    assert_eq!(
        sys.capture_wheel_notch(MouseScrollDelta::LineDelta(0.0, 0.5)),
        Some(PhysicalInput::MouseWheelUp)
    );
    assert_eq!(
        sys.snapshot().notch_count(Action::CycleWieldablePrevious),
        0,
        "capture travel never steps gameplay"
    );
}

#[test]
fn a_stick_half_resting_near_the_press_point_presses_a_button_command_once() {
    // Regression: one threshold made a stick resting near it chatter, so the
    // command re-pressed on every crossing.
    let stick_up =
        PhysicalInput::GamepadAxisHalf(gilrs::Axis::RightStickY, types::AxisHalf::Positive);
    let mut sys = InputSystem::new(vec![Binding::new(stick_up, Action::Jump)]);
    let mut presses = 0;
    for (frame, value) in [0.0, 0.52, 0.47, 0.51, 0.45, 0.49, 0.42, 0.5]
        .into_iter()
        .enumerate()
    {
        let t = frame as f64 * 0.016;
        sys.set_gamepad_axis_at(gilrs::Axis::RightStickY, value, t);
        if sys.snapshot_at(t + 0.001).button(Action::Jump) == ButtonState::Pressed {
            presses += 1;
        }
    }
    assert_eq!(presses, 1);
    sys.set_gamepad_axis_at(gilrs::Axis::RightStickY, 0.3, 1.0);
    assert_eq!(
        sys.snapshot_at(1.001).button(Action::Jump),
        ButtonState::Released
    );
}

#[test]
fn two_dpad_presses_inside_one_frame_step_weapon_cycling_twice() {
    // Regression: press edges were a set per frame, so a hitch frame holding
    // two D-pad presses stepped once.
    let mut sys = InputSystem::new(vec![Binding::new(
        pad(gilrs::Button::DPadRight),
        Action::CycleWieldableNext,
    )]);
    for pressed in [true, false, true, false] {
        sys.handle_gamepad_button_event(gilrs::Button::DPadRight, pressed, 0.0);
    }
    assert_eq!(sys.snapshot().notch_count(Action::CycleWieldableNext), 2);
    assert_eq!(sys.snapshot().notch_count(Action::CycleWieldableNext), 0);
}
