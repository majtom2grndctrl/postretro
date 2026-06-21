// Engine<->wire conversion for the client input-command stream (M15 Phase 3):
// `sim::SimCommand` <-> `postretro_net::wire::InputCommand`, plus the inbound
// `sanitize_input_command` guard the host runs before queueing a client command.
// See: context/lib/networking.md

use glam::Vec2;

use postretro_net::wire::{InputCommand, WireFireButtonState, WireMovementInput};

use crate::movement::MovementInput;
use crate::sim::SimCommand;
use crate::weapon::FireButtonState;

/// Convert a `SimCommand` plus the issuing client's command-frame tick into the
/// wire `InputCommand`. `wish_dir` mirrors the engine `Vec2` (`x = right,
/// y = forward`) into the wire's `[right, forward]` array; the buttons and
/// `facing_yaw` carry through verbatim, and `FireButtonState` rides along
/// faithfully (Phase 5 consumes it — Phase 3 only round-trips it).
// Caller lands in Task 3 (client send) / Task 5 (reconciliation replay).
#[allow(dead_code)]
pub(crate) fn sim_command_to_input(cmd: &SimCommand, client_tick: u32) -> InputCommand {
    InputCommand {
        client_tick,
        movement: WireMovementInput {
            wish_dir: [cmd.movement.wish_dir.x, cmd.movement.wish_dir.y],
            jump_pressed: cmd.movement.jump_pressed,
            dash_pressed: cmd.movement.dash_pressed,
            running: cmd.movement.running,
            crouch_intent: cmd.movement.crouch_intent,
            facing_yaw: cmd.movement.facing_yaw,
        },
        fire_button: WireFireButtonState {
            pressed: cmd.fire_button.pressed,
            active: cmd.fire_button.active,
        },
    }
}

/// Inverse of [`sim_command_to_input`]: rebuild the engine `SimCommand` from a
/// wire `InputCommand`. `wish_dir`'s `[right, forward]` array maps back to the
/// engine `Vec2` (`x = right, y = forward`); the `client_tick` is wire-only
/// command-history bookkeeping and is not part of the `SimCommand`, so the caller
/// reads it off the `InputCommand` separately.
//
// Callers: Task 3 client prediction (`netcode::prediction` rebuilds the
// `MovementInput` for the movement-only replay) and Task 4 (host applies queued
// client commands to its sim).
pub(crate) fn input_command_to_sim(input: &InputCommand) -> SimCommand {
    SimCommand {
        movement: MovementInput {
            wish_dir: Vec2::new(input.movement.wish_dir[0], input.movement.wish_dir[1]),
            jump_pressed: input.movement.jump_pressed,
            dash_pressed: input.movement.dash_pressed,
            running: input.movement.running,
            crouch_intent: input.movement.crouch_intent,
            facing_yaw: input.movement.facing_yaw,
        },
        fire_button: FireButtonState {
            pressed: input.fire_button.pressed,
            active: input.fire_button.active,
        },
    }
}

/// Sanitize an inbound client `InputCommand` before it is queued for the host
/// sim (Task 4 calls this from `host_handle_client_messages`). Pure: it never
/// touches any queue or registry state — it returns a cleaned copy or rejects.
///
/// Rules:
/// - Reject (`None`) a non-finite `wish_dir` component or a non-finite
///   `facing_yaw`. A NaN/inf would poison the host movement math, and an
///   untrusted peer can send either.
/// - Clamp each finite `wish_dir` component into `[-1.0, 1.0]` — `MovementInput`
///   documents that the raw x/y drive magnitude-sensitive threshold checks, so an
///   out-of-range diagonal must be reined in before it reaches the tick.
/// - Preserve a finite `facing_yaw` as-is. Camera yaw is intentionally
///   unconstrained; Phase 3 introduces no wrapping policy.
/// - Boolean button fields are already typed by bitcode, so they need no
///   validation and carry through unchanged.
// Caller lands in Task 4 (`host_handle_client_messages` before queueing).
#[allow(dead_code)]
pub(crate) fn sanitize_input_command(cmd: &InputCommand) -> Option<InputCommand> {
    let [wish_right, wish_forward] = cmd.movement.wish_dir;
    if !wish_right.is_finite() || !wish_forward.is_finite() || !cmd.movement.facing_yaw.is_finite()
    {
        return None;
    }

    let mut sanitized = *cmd;
    sanitized.movement.wish_dir = [wish_right.clamp(-1.0, 1.0), wish_forward.clamp(-1.0, 1.0)];
    Some(sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;

    // wish_dir / facing_yaw are finite values we author and pass through without
    // computation, so exact equality is the right assertion for the integer/bool
    // fields; the float fields use an explicit epsilon (testing_guide
    // §Floating-point).
    const EPSILON: f32 = 1e-6;

    fn sample_sim_command() -> SimCommand {
        SimCommand {
            movement: MovementInput {
                wish_dir: Vec2::new(0.5, -0.75),
                jump_pressed: true,
                dash_pressed: false,
                running: true,
                crouch_intent: true,
                facing_yaw: 1.234_5,
            },
            fire_button: FireButtonState {
                pressed: true,
                active: false,
            },
        }
    }

    fn assert_sim_eq(a: &SimCommand, b: &SimCommand) {
        assert!((a.movement.wish_dir.x - b.movement.wish_dir.x).abs() < EPSILON);
        assert!((a.movement.wish_dir.y - b.movement.wish_dir.y).abs() < EPSILON);
        assert_eq!(a.movement.jump_pressed, b.movement.jump_pressed);
        assert_eq!(a.movement.dash_pressed, b.movement.dash_pressed);
        assert_eq!(a.movement.running, b.movement.running);
        assert_eq!(a.movement.crouch_intent, b.movement.crouch_intent);
        assert!((a.movement.facing_yaw - b.movement.facing_yaw).abs() < EPSILON);
        assert_eq!(a.fire_button.pressed, b.fire_button.pressed);
        assert_eq!(a.fire_button.active, b.fire_button.active);
    }

    #[test]
    fn sim_command_round_trips_through_input_command() {
        let original = sample_sim_command();
        let input = sim_command_to_input(&original, 4_242);
        assert_eq!(input.client_tick, 4_242);
        let rebuilt = input_command_to_sim(&input);
        assert_sim_eq(&original, &rebuilt);
    }

    #[test]
    fn sim_command_to_input_maps_wish_dir_right_forward_order() {
        let cmd = sample_sim_command();
        let input = sim_command_to_input(&cmd, 0);
        // Engine Vec2 (x = right, y = forward) -> wire [right, forward].
        assert!((input.movement.wish_dir[0] - cmd.movement.wish_dir.x).abs() < EPSILON);
        assert!((input.movement.wish_dir[1] - cmd.movement.wish_dir.y).abs() < EPSILON);
    }

    #[test]
    fn sanitize_rejects_non_finite_wish_dir_component() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut cmd = sim_command_to_input(&sample_sim_command(), 1);
            cmd.movement.wish_dir[0] = bad;
            assert!(sanitize_input_command(&cmd).is_none());
            let mut cmd = sim_command_to_input(&sample_sim_command(), 1);
            cmd.movement.wish_dir[1] = bad;
            assert!(sanitize_input_command(&cmd).is_none());
        }
    }

    #[test]
    fn sanitize_rejects_non_finite_facing_yaw() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut cmd = sim_command_to_input(&sample_sim_command(), 1);
            cmd.movement.facing_yaw = bad;
            assert!(sanitize_input_command(&cmd).is_none());
        }
    }

    #[test]
    fn sanitize_clamps_out_of_range_finite_wish_dir() {
        let mut cmd = sim_command_to_input(&sample_sim_command(), 1);
        cmd.movement.wish_dir = [5.0, -3.0];
        let sanitized = sanitize_input_command(&cmd).expect("finite wish_dir is accepted");
        assert!((sanitized.movement.wish_dir[0] - 1.0).abs() < EPSILON);
        assert!((sanitized.movement.wish_dir[1] - (-1.0)).abs() < EPSILON);
    }

    #[test]
    fn sanitize_preserves_in_range_wish_dir_and_facing_yaw() {
        let cmd = sim_command_to_input(&sample_sim_command(), 1);
        let sanitized = sanitize_input_command(&cmd).expect("finite in-range command is accepted");
        assert!((sanitized.movement.wish_dir[0] - cmd.movement.wish_dir[0]).abs() < EPSILON);
        assert!((sanitized.movement.wish_dir[1] - cmd.movement.wish_dir[1]).abs() < EPSILON);
        // facing_yaw is intentionally unconstrained: a finite value passes through
        // unchanged, with no wrapping.
        assert!((sanitized.movement.facing_yaw - cmd.movement.facing_yaw).abs() < EPSILON);
    }
}
