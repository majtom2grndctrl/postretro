use winit::keyboard::KeyCode;

use super::*;

const SHIFT: KeyCode = KeyCode::ShiftLeft;

fn key(code: KeyCode) -> PhysicalInput {
    PhysicalInput::Key(code)
}

fn tap(threshold: f32) -> Activator {
    Activator::with_threshold(ActivatorKind::Tap, threshold)
}

fn hold(threshold: f32) -> Activator {
    Activator::with_threshold(ActivatorKind::Hold, threshold)
}

/// Shift taps dash and holds sprint; Space presses jump.
fn shift_tap_dash_hold_sprint(tap_max: f32, hold_min: f32) -> InputSystem {
    InputSystem::new(vec![
        Binding::new(key(SHIFT), Action::Dash).with_activator(tap(tap_max)),
        Binding::new(key(SHIFT), Action::Sprint).with_activator(hold(hold_min)),
        Binding::new(key(KeyCode::Space), Action::Jump),
    ])
}

fn press(sys: &mut InputSystem, code: KeyCode, t: f64) {
    sys.handle_keyboard_event_at(code, true, t);
}

fn release(sys: &mut InputSystem, code: KeyCode, t: f64) {
    sys.handle_keyboard_event_at(code, false, t);
}

/// Every button state an action shows across a run of snapshots.
fn states(sys: &mut InputSystem, action: Action, times: &[f64]) -> Vec<ButtonState> {
    times
        .iter()
        .map(|&t| sys.snapshot_at(t).button(action))
        .collect()
}

fn fired(states: &[ButtonState]) -> usize {
    states
        .iter()
        .filter(|state| **state == ButtonState::Pressed)
        .count()
}

// --- Tap and hold on one key (AV1) ---

#[test]
fn shared_key_release_within_tap_max_fires_tap_once_and_hold_never() {
    let mut sys = shift_tap_dash_hold_sprint(0.2, 0.2);
    press(&mut sys, SHIFT, 0.0);
    let pressing = sys.snapshot_at(0.05);
    assert_eq!(pressing.button(Action::Dash), ButtonState::Inactive);
    release(&mut sys, SHIFT, 0.1);
    let released = sys.snapshot_at(0.11);
    assert_eq!(released.button(Action::Dash), ButtonState::Pressed);
    assert_eq!(released.button(Action::Sprint), ButtonState::Inactive);
    let after = states(&mut sys, Action::Dash, &[0.2, 0.5, 1.0]);
    assert_eq!(after[0], ButtonState::Released);
    assert_eq!(fired(&after), 0);
}

#[test]
fn shared_key_held_past_hold_min_fires_hold_and_never_tap() {
    let mut sys = shift_tap_dash_hold_sprint(0.2, 0.2);
    press(&mut sys, SHIFT, 0.0);
    assert_eq!(
        sys.snapshot_at(0.1).button(Action::Sprint),
        ButtonState::Inactive
    );
    let crossed = sys.snapshot_at(0.25);
    assert_eq!(crossed.button(Action::Sprint), ButtonState::Pressed);
    assert_eq!(crossed.button(Action::Dash), ButtonState::Inactive);
    release(&mut sys, SHIFT, 0.4);
    let released = sys.snapshot_at(0.41);
    assert_eq!(released.button(Action::Sprint), ButtonState::Released);
    assert_eq!(released.button(Action::Dash), ButtonState::Inactive);
}

#[test]
fn shared_key_release_exactly_at_threshold_counts_as_the_release() {
    let mut sys = shift_tap_dash_hold_sprint(0.2, 0.2);
    press(&mut sys, SHIFT, 1.0);
    release(&mut sys, SHIFT, 1.2);
    let snap = sys.snapshot_at(1.3);
    assert_eq!(snap.button(Action::Dash), ButtonState::Pressed);
    assert_eq!(snap.button(Action::Sprint), ButtonState::Inactive);
}

#[test]
fn shared_key_release_between_tap_max_and_hold_min_fires_nothing() {
    let mut sys = shift_tap_dash_hold_sprint(0.1, 0.3);
    press(&mut sys, SHIFT, 0.0);
    release(&mut sys, SHIFT, 0.2);
    let dash = states(&mut sys, Action::Dash, &[0.21, 0.5]);
    assert_eq!(fired(&dash), 0);
    let mut sys = shift_tap_dash_hold_sprint(0.1, 0.3);
    press(&mut sys, SHIFT, 0.0);
    release(&mut sys, SHIFT, 0.2);
    let sprint = states(&mut sys, Action::Sprint, &[0.21, 0.5]);
    assert_eq!(fired(&sprint), 0);
}

#[test]
fn losing_focus_mid_hold_fires_neither_binding() {
    let mut sys = shift_tap_dash_hold_sprint(0.2, 0.3);
    press(&mut sys, SHIFT, 0.0);
    let _ = sys.snapshot_at(0.1);
    sys.clear_all_at(0.12);
    release(&mut sys, SHIFT, 0.15);
    let dash = states(&mut sys, Action::Dash, &[0.16, 0.5, 1.0]);
    assert_eq!(fired(&dash), 0);
    // The next press resolves normally.
    press(&mut sys, SHIFT, 2.0);
    release(&mut sys, SHIFT, 2.05);
    assert_eq!(
        sys.snapshot_at(2.06).button(Action::Dash),
        ButtonState::Pressed
    );
}

#[test]
fn losing_focus_after_the_hold_fired_releases_without_a_tap() {
    let mut sys = shift_tap_dash_hold_sprint(0.2, 0.2);
    press(&mut sys, SHIFT, 0.0);
    assert_eq!(
        sys.snapshot_at(0.3).button(Action::Sprint),
        ButtonState::Pressed
    );
    sys.clear_all_at(0.305);
    let snap = sys.snapshot_at(0.31);
    assert_eq!(snap.button(Action::Sprint), ButtonState::Released);
    release(&mut sys, SHIFT, 0.32);
    assert_eq!(fired(&states(&mut sys, Action::Dash, &[0.33, 0.6])), 0);
}

// --- Same-frame edges (AV2: P1, P2) ---

#[test]
fn press_and_release_between_frames_fire_the_tap_once_and_the_hold_never() {
    let mut sys = shift_tap_dash_hold_sprint(0.2, 0.2);
    press(&mut sys, SHIFT, 0.001);
    release(&mut sys, SHIFT, 0.009);
    let dash = states(&mut sys, Action::Dash, &[0.016, 0.033, 0.05]);
    assert_eq!(
        dash,
        [
            ButtonState::Pressed,
            ButtonState::Released,
            ButtonState::Inactive
        ]
    );
    let mut sys = shift_tap_dash_hold_sprint(0.2, 0.2);
    press(&mut sys, SHIFT, 0.001);
    release(&mut sys, SHIFT, 0.009);
    assert_eq!(fired(&states(&mut sys, Action::Sprint, &[0.016, 0.5])), 0);
}

#[test]
fn lone_press_pressed_and_released_between_frames_fires_once() {
    // Regression: key state was a level per key, so a press and release
    // between two snapshots left no edge at all.
    let mut sys = shift_tap_dash_hold_sprint(0.2, 0.2);
    press(&mut sys, KeyCode::Space, 0.001);
    release(&mut sys, KeyCode::Space, 0.009);
    let jump = states(&mut sys, Action::Jump, &[0.016, 0.033]);
    assert_eq!(jump, [ButtonState::Pressed, ButtonState::Released]);
}

#[test]
fn two_keys_tapped_on_one_frame_fire_their_command_once() {
    let mut sys = InputSystem::new(vec![
        Binding::new(key(KeyCode::KeyF), Action::Dash).with_activator(tap(0.2)),
        Binding::new(key(KeyCode::KeyV), Action::Dash).with_activator(tap(0.2)),
    ]);
    press(&mut sys, KeyCode::KeyF, 0.0);
    press(&mut sys, KeyCode::KeyV, 0.01);
    release(&mut sys, KeyCode::KeyF, 0.05);
    release(&mut sys, KeyCode::KeyV, 0.06);
    let dash = states(&mut sys, Action::Dash, &[0.07, 0.1]);
    assert_eq!(dash, [ButtonState::Pressed, ButtonState::Released]);
}

#[test]
fn gamepad_button_event_pressed_and_released_between_polls_fires_once() {
    let mut sys = InputSystem::new(vec![Binding::new(
        PhysicalInput::GamepadButton(gilrs::Button::South),
        Action::Jump,
    )]);
    sys.handle_gamepad_button_event(gilrs::Button::South, true, 0.0);
    sys.handle_gamepad_button_event(gilrs::Button::South, false, 0.0);
    // The poll that follows sees the button up and adds no edge.
    sys.set_physical_input(PhysicalInput::GamepadButton(gilrs::Button::South), false);
    assert_eq!(sys.snapshot().button(Action::Jump), ButtonState::Pressed);
    assert_eq!(sys.snapshot().button(Action::Jump), ButtonState::Released);
}

// --- Lone activators (AV4, AV5) ---

#[test]
fn lone_press_fires_on_the_press_frame() {
    let mut sys = InputSystem::new(vec![Binding::new(key(KeyCode::KeyF), Action::Dash)]);
    press(&mut sys, KeyCode::KeyF, 0.0);
    assert_eq!(
        sys.snapshot_at(0.01).button(Action::Dash),
        ButtonState::Pressed
    );
    assert_eq!(sys.snapshot_at(0.5).button(Action::Dash), ButtonState::Held);
}

#[test]
fn lone_tap_fires_on_its_release_frame_within_max_and_never_past_it() {
    let mut sys = InputSystem::new(vec![
        Binding::new(key(KeyCode::KeyF), Action::Dash).with_activator(tap(0.2)),
    ]);
    press(&mut sys, KeyCode::KeyF, 0.0);
    assert_eq!(
        sys.snapshot_at(0.1).button(Action::Dash),
        ButtonState::Inactive
    );
    release(&mut sys, KeyCode::KeyF, 0.15);
    assert_eq!(
        sys.snapshot_at(0.16).button(Action::Dash),
        ButtonState::Pressed
    );

    press(&mut sys, KeyCode::KeyF, 1.0);
    release(&mut sys, KeyCode::KeyF, 1.25);
    assert_eq!(fired(&states(&mut sys, Action::Dash, &[1.26, 1.5])), 0);
}

#[test]
fn lone_release_fires_on_key_up() {
    let mut sys = InputSystem::new(vec![
        Binding::new(key(KeyCode::KeyF), Action::Dash)
            .with_activator(Activator::new(ActivatorKind::Release)),
    ]);
    press(&mut sys, KeyCode::KeyF, 0.0);
    assert_eq!(
        sys.snapshot_at(0.5).button(Action::Dash),
        ButtonState::Inactive
    );
    release(&mut sys, KeyCode::KeyF, 0.9);
    assert_eq!(
        sys.snapshot_at(0.91).button(Action::Dash),
        ButtonState::Pressed
    );
    assert_eq!(
        sys.snapshot_at(0.92).button(Action::Dash),
        ButtonState::Released
    );
}

#[test]
fn authored_tap_threshold_bounds_the_tap_and_an_unset_one_uses_the_default() {
    let mut sys = InputSystem::new(vec![
        Binding::new(key(KeyCode::KeyF), Action::Dash).with_activator(tap(0.1)),
    ]);
    press(&mut sys, KeyCode::KeyF, 0.0);
    release(&mut sys, KeyCode::KeyF, 0.15);
    assert_eq!(fired(&states(&mut sys, Action::Dash, &[0.16, 0.3])), 0);

    let default_tap = Activator::new(ActivatorKind::Tap);
    assert_eq!(default_tap.threshold, DEFAULT_ACTIVATOR_THRESHOLD);
    assert_eq!(DEFAULT_ACTIVATOR_THRESHOLD, 0.2);
    let mut sys = InputSystem::new(vec![
        Binding::new(key(KeyCode::KeyF), Action::Dash).with_activator(default_tap),
    ]);
    press(&mut sys, KeyCode::KeyF, 0.0);
    release(&mut sys, KeyCode::KeyF, 0.15);
    assert_eq!(
        sys.snapshot_at(0.16).button(Action::Dash),
        ButtonState::Pressed
    );
}

// --- Held states (AV6) ---

#[test]
fn hold_sprint_shows_a_held_state_every_frame_past_the_threshold() {
    let mut sys = shift_tap_dash_hold_sprint(0.2, 0.2);
    press(&mut sys, SHIFT, 0.0);
    let held = states(&mut sys, Action::Sprint, &[0.1, 0.21, 0.3, 0.5, 0.8]);
    assert_eq!(
        held,
        [
            ButtonState::Inactive,
            ButtonState::Pressed,
            ButtonState::Held,
            ButtonState::Held,
            ButtonState::Held,
        ]
    );
    release(&mut sys, SHIFT, 0.85);
    assert_eq!(
        sys.snapshot_at(0.9).button(Action::Sprint),
        ButtonState::Released
    );
}

// --- hold_timing_scale (AV7, P23) ---

#[test]
fn hold_timing_scale_two_doubles_every_threshold() {
    let mut sys = shift_tap_dash_hold_sprint(0.2, 0.2);
    sys.set_hold_timing_scale(2.0);
    press(&mut sys, SHIFT, 0.0);
    assert_eq!(
        sys.snapshot_at(0.3).button(Action::Sprint),
        ButtonState::Inactive
    );
    assert_eq!(
        sys.snapshot_at(0.41).button(Action::Sprint),
        ButtonState::Pressed
    );
    release(&mut sys, SHIFT, 0.5);
    let _ = sys.snapshot_at(0.51);

    // A tap at 0.3 s is within the doubled 0.4 s max.
    press(&mut sys, SHIFT, 1.0);
    release(&mut sys, SHIFT, 1.3);
    assert_eq!(
        sys.snapshot_at(1.31).button(Action::Dash),
        ButtonState::Pressed
    );
}

#[test]
fn a_key_already_down_keeps_the_threshold_it_started_with() {
    let mut sys = shift_tap_dash_hold_sprint(0.2, 0.2);
    press(&mut sys, SHIFT, 0.0);
    let _ = sys.snapshot_at(0.05);
    sys.set_hold_timing_scale(3.0);
    // Still 0.2 s for this press, not 0.6 s.
    assert_eq!(
        sys.snapshot_at(0.25).button(Action::Sprint),
        ButtonState::Pressed
    );
}

// --- Held across a capturing menu (AV9, P24) ---

#[test]
fn a_polled_button_held_across_a_menu_does_nothing_until_pressed_again() {
    let south = PhysicalInput::GamepadButton(gilrs::Button::South);
    let mut sys = InputSystem::new(vec![Binding::new(south, Action::Jump)]);
    sys.set_physical_input_at(south, true, 0.0);
    assert_eq!(
        sys.snapshot_at(0.01).button(Action::Jump),
        ButtonState::Pressed
    );
    // A capturing menu opens, then closes, with the button still down.
    sys.clear_all_at(0.05);
    sys.set_physical_input_at(south, true, 0.1);
    let _ = sys.snapshot_at(0.11);
    sys.clear_all_at(0.15);
    sys.set_physical_input_at(south, true, 0.2);
    let held = states(&mut sys, Action::Jump, &[0.21, 0.3]);
    assert_eq!(fired(&held), 0);
    sys.set_physical_input_at(south, false, 0.4);
    let _ = sys.snapshot_at(0.41);
    sys.set_physical_input_at(south, true, 0.5);
    assert_eq!(
        sys.snapshot_at(0.51).button(Action::Jump),
        ButtonState::Pressed
    );
}

// --- Phases ride the gameplay latch (AV3, P3; the riskiest premise) ---

#[test]
fn a_tap_on_a_zero_tick_frame_reaches_the_next_tick_as_a_press() {
    let mut sys = shift_tap_dash_hold_sprint(0.2, 0.2);
    let mut latch = GameplayInputLatch::new();
    press(&mut sys, SHIFT, 0.0);
    release(&mut sys, SHIFT, 0.05);
    let frame = sys.snapshot_at(0.06);
    assert!(latch.snapshot_for_ticks(&frame, 0).is_none());
    let frame = sys.snapshot_at(0.07);
    let tick = latch
        .snapshot_for_ticks(&frame, 1)
        .expect("one tick runs on this frame");
    assert_eq!(tick.button(Action::Dash), ButtonState::Pressed);
    assert!(!tick.button(Action::Sprint).is_active());
}

#[test]
fn a_hold_crossing_on_a_zero_tick_frame_reaches_the_next_tick_and_its_tap_never_fires() {
    let mut sys = shift_tap_dash_hold_sprint(0.2, 0.2);
    let mut latch = GameplayInputLatch::new();
    press(&mut sys, SHIFT, 0.0);
    let frame = sys.snapshot_at(0.25);
    assert_eq!(frame.button(Action::Sprint), ButtonState::Pressed);
    assert!(latch.snapshot_for_ticks(&frame, 0).is_none());
    // Released before the next tick: the frame alone reads Released, but the
    // latched press keeps sprint active for that tick.
    release(&mut sys, SHIFT, 0.26);
    let frame = sys.snapshot_at(0.27);
    assert_eq!(frame.button(Action::Sprint), ButtonState::Released);
    let tick = latch
        .snapshot_for_ticks(&frame, 1)
        .expect("one tick runs on this frame");
    assert!(tick.button(Action::Sprint).is_active());
    assert_eq!(tick.button(Action::Dash), ButtonState::Inactive);
}
