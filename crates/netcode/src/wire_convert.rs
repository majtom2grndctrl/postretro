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
/// `facing_yaw` carries through verbatim; `aim_pitch` is sampled from the camera at
/// the command send seam because movement simulation deliberately has no pitch
/// field. Fire/reload intent is preserved for downstream simulation.
pub(crate) fn sim_command_to_input(
    cmd: &SimCommand,
    client_tick: u32,
    aim_pitch: f32,
) -> InputCommand {
    InputCommand {
        secondary_button: WireFireButtonState {
            pressed: cmd.secondary_button.pressed,
            active: cmd.secondary_button.active,
        },
        activation: activation_input_to_wire(cmd.activation),
        client_tick,
        movement: WireMovementInput {
            wish_dir: [cmd.movement.wish_dir.x, cmd.movement.wish_dir.y],
            jump_pressed: cmd.movement.jump_pressed,
            dash_pressed: cmd.movement.dash_pressed,
            running: cmd.movement.running,
            crouch_intent: cmd.movement.crouch_intent,
            facing_yaw: cmd.movement.facing_yaw,
            use_pressed: cmd.use_pressed,
            drop_pressed: cmd.drop_pressed,
            aim_pitch,
            firing_slot: cmd.firing_slot,
        },
        fire_button: WireFireButtonState {
            pressed: cmd.fire_button.pressed,
            active: cmd.fire_button.active,
        },
        reload: cmd.reload,
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
        secondary_button: FireButtonState {
            pressed: input.secondary_button.pressed,
            active: input.secondary_button.active,
        },
        activation: activation_input_from_wire(input.activation),
        movement: MovementInput {
            wish_dir: Vec2::new(input.movement.wish_dir[0], input.movement.wish_dir[1]),
            jump_pressed: input.movement.jump_pressed,
            dash_pressed: input.movement.dash_pressed,
            running: input.movement.running,
            crouch_intent: input.movement.crouch_intent,
            facing_yaw: input.movement.facing_yaw,
            use_pressed: input.movement.use_pressed,
            drop_pressed: input.movement.drop_pressed,
        },
        fire_button: FireButtonState {
            pressed: input.fire_button.pressed,
            active: input.fire_button.active,
        },
        reload: input.reload,
        firing_slot: input.movement.firing_slot,
        select_slot: None,
        use_pressed: input.movement.use_pressed,
        drop_pressed: input.movement.drop_pressed,
    }
}

/// Sanitize an inbound client `InputCommand` before it is queued for the host
/// sim (Task 4 calls this from `host_handle_client_messages`). Pure: it never
/// touches any queue or registry state — it returns a cleaned copy or rejects.
///
/// Rules:
/// - Reject (`None`) a non-finite `wish_dir` component or a non-finite
///   `facing_yaw`, or a non-finite `aim_pitch`. A NaN/inf would poison host
///   movement or presentation math, and an untrusted peer can send either.
/// - Clamp each finite `wish_dir` component into `[-1.0, 1.0]` — `MovementInput`
///   documents that the raw x/y drive magnitude-sensitive threshold checks, so an
///   out-of-range diagonal must be reined in before it reaches the tick.
/// - Preserve a finite `facing_yaw` as-is. Camera yaw is intentionally
///   unconstrained; Phase 3 introduces no wrapping policy.
/// - Boolean button fields are already typed by bitcode, so they need no
///   validation and carry through unchanged.
// Called by Task 4's host command-queue intake (`command_queue::HostCommandQueues::ingest`).
pub(crate) fn sanitize_input_command(cmd: &InputCommand) -> Option<InputCommand> {
    let [wish_right, wish_forward] = cmd.movement.wish_dir;
    if !wish_right.is_finite()
        || !wish_forward.is_finite()
        || !cmd.movement.facing_yaw.is_finite()
        || !cmd.movement.aim_pitch.is_finite()
    {
        return None;
    }

    for token in [
        cmd.activation.initiation,
        cmd.activation.release.map(|r| r.token),
        cmd.activation.cancel,
    ]
    .into_iter()
    .flatten()
    {
        postretro_foundation::ActivationLane::from_tag(token.lane)?;
    }
    if cmd
        .activation
        .initiation
        .is_some_and(|token| token.start_tick != cmd.client_tick)
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

    pub(super) fn sample_sim_command() -> SimCommand {
        SimCommand {
            secondary_button: crate::weapon::FireButtonState {
                pressed: false,
                active: false,
            },
            activation: postretro_foundation::ActivationInput::default(),
            movement: MovementInput {
                wish_dir: Vec2::new(0.5, -0.75),
                jump_pressed: true,
                dash_pressed: false,
                running: true,
                crouch_intent: true,
                facing_yaw: 1.234_5,
                use_pressed: true,
                drop_pressed: true,
            },
            fire_button: FireButtonState {
                pressed: true,
                active: false,
            },
            reload: true,
            firing_slot: 4,
            select_slot: None,
            use_pressed: true,
            drop_pressed: true,
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
        assert_eq!(a.movement.use_pressed, b.movement.use_pressed);
        assert_eq!(a.movement.drop_pressed, b.movement.drop_pressed);
        assert_eq!(a.fire_button.pressed, b.fire_button.pressed);
        assert_eq!(a.fire_button.active, b.fire_button.active);
        assert_eq!(a.reload, b.reload);
        assert_eq!(a.firing_slot, b.firing_slot);
        assert_eq!(a.use_pressed, b.use_pressed);
        assert_eq!(a.drop_pressed, b.drop_pressed);
    }

    #[test]
    fn sim_command_round_trips_through_input_command() {
        let original = sample_sim_command();
        let input = sim_command_to_input(&original, 4_242, -0.3);
        assert_eq!(input.client_tick, 4_242);
        assert!((input.movement.aim_pitch - (-0.3)).abs() < EPSILON);
        let rebuilt = input_command_to_sim(&input);
        assert_sim_eq(&original, &rebuilt);
    }

    #[test]
    fn sim_command_to_input_maps_wish_dir_right_forward_order() {
        let cmd = sample_sim_command();
        let input = sim_command_to_input(&cmd, 0, -0.3);
        // Engine Vec2 (x = right, y = forward) -> wire [right, forward].
        assert!((input.movement.wish_dir[0] - cmd.movement.wish_dir.x).abs() < EPSILON);
        assert!((input.movement.wish_dir[1] - cmd.movement.wish_dir.y).abs() < EPSILON);
    }

    #[test]
    fn sanitize_rejects_non_finite_wish_dir_component() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut cmd = sim_command_to_input(&sample_sim_command(), 1, -0.3);
            cmd.movement.wish_dir[0] = bad;
            assert!(sanitize_input_command(&cmd).is_none());
            let mut cmd = sim_command_to_input(&sample_sim_command(), 1, -0.3);
            cmd.movement.wish_dir[1] = bad;
            assert!(sanitize_input_command(&cmd).is_none());
        }
    }

    #[test]
    fn sanitize_rejects_non_finite_facing_yaw() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut cmd = sim_command_to_input(&sample_sim_command(), 1, -0.3);
            cmd.movement.facing_yaw = bad;
            assert!(sanitize_input_command(&cmd).is_none());
        }
    }

    #[test]
    fn sanitize_rejects_non_finite_aim_pitch() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let cmd = sim_command_to_input(&sample_sim_command(), 1, bad);
            assert!(sanitize_input_command(&cmd).is_none());
        }
    }

    #[test]
    fn sanitize_clamps_out_of_range_finite_wish_dir() {
        let mut cmd = sim_command_to_input(&sample_sim_command(), 1, -0.3);
        cmd.movement.wish_dir = [5.0, -3.0];
        let sanitized = sanitize_input_command(&cmd).expect("finite wish_dir is accepted");
        assert!((sanitized.movement.wish_dir[0] - 1.0).abs() < EPSILON);
        assert!((sanitized.movement.wish_dir[1] - (-1.0)).abs() < EPSILON);
    }

    #[test]
    fn sanitize_preserves_in_range_wish_dir_and_facing_yaw() {
        let cmd = sim_command_to_input(&sample_sim_command(), 1, -0.3);
        let sanitized = sanitize_input_command(&cmd).expect("finite in-range command is accepted");
        assert!((sanitized.movement.wish_dir[0] - cmd.movement.wish_dir[0]).abs() < EPSILON);
        assert!((sanitized.movement.wish_dir[1] - cmd.movement.wish_dir[1]).abs() < EPSILON);
        // facing_yaw is intentionally unconstrained: a finite value passes through
        // unchanged, with no wrapping.
        assert!((sanitized.movement.facing_yaw - cmd.movement.facing_yaw).abs() < EPSILON);
        assert!((sanitized.movement.aim_pitch - cmd.movement.aim_pitch).abs() < EPSILON);
        assert_eq!(sanitized.reload, cmd.reload);
    }
}

pub fn shot_id_to_wire(id: postretro_foundation::ShotId) -> postretro_net::wire::WireShotId {
    postretro_net::wire::WireShotId {
        pawn: id.pawn,
        start_tick: id.start_tick,
        lane: id.lane as u8,
        ordinal: id.ordinal,
    }
}
/// Call only after intake validation (or for locally produced wire records).
pub fn shot_id_from_wire(id: postretro_net::wire::WireShotId) -> postretro_foundation::ShotId {
    postretro_foundation::ShotId::from_parts(
        id.pawn,
        id.start_tick,
        postretro_foundation::ActivationLane::from_tag(id.lane).expect("validated activation lane"),
        id.ordinal,
    )
}
pub fn valid_wire_shot_id(id: postretro_net::wire::WireShotId) -> bool {
    id.ordinal < 16 && postretro_foundation::ActivationLane::from_tag(id.lane).is_some()
}
fn token_to_wire(
    token: postretro_foundation::ActivationToken,
) -> postretro_net::wire::WireActivationToken {
    postretro_net::wire::WireActivationToken {
        start_tick: token.start_tick,
        lane: token.lane as u8,
    }
}
fn token_from_wire(
    token: postretro_net::wire::WireActivationToken,
) -> postretro_foundation::ActivationToken {
    postretro_foundation::ActivationToken {
        start_tick: token.start_tick,
        lane: postretro_foundation::ActivationLane::from_tag(token.lane)
            .expect("validated activation lane"),
    }
}
fn activation_input_to_wire(
    input: postretro_foundation::ActivationInput,
) -> postretro_net::wire::WireActivationInput {
    postretro_net::wire::WireActivationInput {
        initiation: input.initiation.map(token_to_wire),
        release: input
            .release
            .map(|r| postretro_net::wire::WireActivationRelease {
                token: token_to_wire(r.token),
                release_tick: r.release_tick,
            }),
        cancel: input.cancel.map(token_to_wire),
    }
}
pub(crate) fn activation_input_from_wire(
    input: postretro_net::wire::WireActivationInput,
) -> postretro_foundation::ActivationInput {
    postretro_foundation::ActivationInput {
        initiation: input.initiation.map(token_from_wire),
        release: input
            .release
            .map(|r| postretro_foundation::ActivationRelease {
                token: token_from_wire(r.token),
                release_tick: r.release_tick,
            }),
        cancel: input.cancel.map(token_from_wire),
    }
}

#[cfg(test)]
mod activation_tests {
    use super::*;
    #[test]
    fn activation_wire_rejects_unknown_lane_and_mismatched_initiation_tick() {
        let mut input = sim_command_to_input(&super::tests::sample_sim_command(), 4, 0.0);
        input.activation.initiation = Some(postretro_net::wire::WireActivationToken {
            start_tick: 4,
            lane: 2,
        });
        assert!(sanitize_input_command(&input).is_none());
        input.activation.initiation = Some(postretro_net::wire::WireActivationToken {
            start_tick: 3,
            lane: 0,
        });
        assert!(sanitize_input_command(&input).is_none());
        assert!(!valid_wire_shot_id(postretro_net::wire::WireShotId {
            pawn: 4,
            start_tick: 4,
            lane: 0,
            ordinal: 16
        }));
    }
}

pub(crate) fn valid_activation_outcome(outcome: &postretro_net::wire::ActivationOutcome) -> bool {
    use postretro_net::wire::ActivationOutcome;
    let token = match outcome {
        ActivationOutcome::InitiationAccepted { token, .. } => *token,
        ActivationOutcome::InitiationRejected { token, .. } => *token,
        ActivationOutcome::ExecutionAccepted {
            token,
            charge_millionths,
            ..
        } => {
            if *charge_millionths > 1_000_000 {
                return false;
            }
            *token
        }
        ActivationOutcome::Cancelled { token, .. } => *token,
        ActivationOutcome::Completed { token, .. } => *token,
    };
    postretro_foundation::ActivationLane::from_tag(token.lane).is_some()
}
