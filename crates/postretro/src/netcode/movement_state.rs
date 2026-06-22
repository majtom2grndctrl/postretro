// Engine<->wire movement-state conversion + merge (M15 Phase 3). Extracts the
// mutable tick subset of a `PlayerMovementComponent` into a
// `WirePlayerMovementState`, and merges a wire payload back onto an EXISTING
// descriptor-derived component WITHOUT disturbing descriptor-owned tuning.
// Shared by host snapshot production (Task 4) and `ClientReplication` (Task 5).
// See: context/lib/networking.md

use glam::Vec3;

use postretro_net::wire::{WireMovementState, WirePlayerMovementState};

use crate::scripting::components::player_movement::{MovementState, PlayerMovementComponent};

/// Extract the mutable tick subset of a `PlayerMovementComponent` into its wire
/// mirror. Only the fields that change tick-to-tick cross the wire; descriptor
/// tuning, `view_feel`, standing dimensions, stuck-stop config, and
/// `dash_programs` stay local on both peers and are never read here.
// Called by Task 4's host snapshot production (`replication::collect_payloads`); also
// consumed by Task 5 reconciliation.
pub(crate) fn movement_state_to_wire(
    component: &PlayerMovementComponent,
) -> WirePlayerMovementState {
    WirePlayerMovementState {
        velocity: [
            component.velocity.x,
            component.velocity.y,
            component.velocity.z,
        ],
        is_grounded: component.is_grounded,
        air_jumps_remaining: component.air_jumps_remaining,
        air_dashes_remaining: component.air_dashes_remaining,
        dash_cooldown_ms: component.dash_cooldown_ms,
        air_ticks: component.air_ticks,
        movement_state: movement_state_enum_to_wire(component.movement_state),
        coyote_timer_ms: component.coyote_timer_ms,
        jump_buffer_timer_ms: component.jump_buffer_timer_ms,
        jump_spent: component.jump_spent,
        capsule_half_height: component.capsule.half_height,
        capsule_eye_height: component.capsule.eye_height,
    }
}

/// Merge the mutable tick subset of a `WirePlayerMovementState` onto an EXISTING
/// descriptor-derived `PlayerMovementComponent`. This is the inverse of
/// [`movement_state_to_wire`], but it is a *merge*, not a constructor: it writes
/// only the mutable tick fields and the live capsule dimensions onto `component`,
/// leaving everything the descriptor owns untouched — tuning (`ground`/`air`/
/// `fall`/`dash`/`crouch`), `view_feel`, `standing_*`, `stuck_stop_*`,
/// `cos_walkable`, the forgiveness *windows* (`coyote_ms`/`jump_buffer_ms`), and
/// the derived `dash_programs`. A reconcile/apply path materializes the component
/// from the local descriptor first, then merges authoritative tick state in here.
pub(crate) fn merge_wire_into_movement_state(
    component: &mut PlayerMovementComponent,
    wire: &WirePlayerMovementState,
) {
    component.velocity = Vec3::new(wire.velocity[0], wire.velocity[1], wire.velocity[2]);
    component.is_grounded = wire.is_grounded;
    component.air_jumps_remaining = wire.air_jumps_remaining;
    component.air_dashes_remaining = wire.air_dashes_remaining;
    component.dash_cooldown_ms = wire.dash_cooldown_ms;
    component.air_ticks = wire.air_ticks;
    component.movement_state = wire_to_movement_state_enum(wire.movement_state);
    component.coyote_timer_ms = wire.coyote_timer_ms;
    component.jump_buffer_timer_ms = wire.jump_buffer_timer_ms;
    component.jump_spent = wire.jump_spent;
    // The live capsule dimensions are mutable tick state (the crouch intent
    // shrinks them); the standing reference dimensions are descriptor-owned and
    // deliberately not touched.
    component.capsule.half_height = wire.capsule_half_height;
    component.capsule.eye_height = wire.capsule_eye_height;
}

/// Map the engine `MovementState` to its wire mirror. Exhaustive (no `_` arm) so
/// a new state variant is a compile error here until its wire mapping is written
/// — the drift-guard discipline (testing_guide §"Drift guards derive from the
/// source") applied to the conversion itself.
fn movement_state_enum_to_wire(state: MovementState) -> WireMovementState {
    match state {
        MovementState::Normal => WireMovementState::Normal,
        MovementState::Dash { elapsed_ms, boost } => WireMovementState::Dash {
            elapsed_ms,
            boost: [boost.x, boost.y, boost.z],
        },
        MovementState::Crouching { eye_current } => WireMovementState::Crouching { eye_current },
    }
}

/// Inverse of [`movement_state_enum_to_wire`]. Exhaustive (no `_` arm) for the
/// same drift-guard reason.
fn wire_to_movement_state_enum(state: WireMovementState) -> MovementState {
    match state {
        WireMovementState::Normal => MovementState::Normal,
        WireMovementState::Dash { elapsed_ms, boost } => MovementState::Dash {
            elapsed_ms,
            boost: Vec3::new(boost[0], boost[1], boost[2]),
        },
        WireMovementState::Crouching { eye_current } => MovementState::Crouching { eye_current },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::data_descriptors::{
        AirParams, BoolOrIr, CapsuleParams, CrouchParams, DashParams, FallParams,
        ForgivenessParams, GroundParams, NumberOrIr, PlayerMovementDescriptor, SpeedParams,
        ViewFeelParams,
    };

    const EPSILON: f32 = 1e-6;

    /// A descriptor with dash, crouch, view-feel, and non-default tuning all set,
    /// so the merge test can prove the merge leaves every descriptor-owned field
    /// untouched (a bare minimal descriptor would have many fields at defaults,
    /// hiding an accidental overwrite).
    fn rich_descriptor() -> PlayerMovementDescriptor {
        PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.4,
                half_height: 0.8,
                eye_height: 0.5,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                    crouch: 3.0,
                },
                accel: 10.0,
                step_height: 0.3,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 0.1,
                accel: 0.7,
                max_control_speed: 0.5,
                bunny_hop: true,
                jumps: 2,
                jump_velocity: 5.5,
                jump_ceiling: 0.0,
            },
            fall: FallParams {
                terminal_velocity: 40.0,
            },
            stuck_stop_enabled: true,
            stuck_stop_threshold: 0.01,
            dash: Some(DashParams {
                boost_speed: NumberOrIr::Literal(18.0),
                momentum_retention: NumberOrIr::Literal(0.5),
                steer_control: NumberOrIr::Literal(0.3),
                dash_drag: NumberOrIr::Literal(2.0),
                cooldown_ms: NumberOrIr::Literal(900.0),
                air_dashes: 1,
                preserve_vertical: BoolOrIr::Literal(false),
            }),
            forgiveness: Some(ForgivenessParams {
                coyote_ms: 120.0,
                jump_buffer_ms: 100.0,
            }),
            crouch: Some(CrouchParams {
                half_height: 0.4,
                eye_height: 0.25,
                transition_rate: 8.0,
            }),
            view_feel: Some(ViewFeelParams {
                bob: None,
                tilt: None,
                sway: None,
            }),
        }
    }

    fn sample_wire_state() -> WirePlayerMovementState {
        WirePlayerMovementState {
            velocity: [1.0, -2.0, 3.5],
            is_grounded: true,
            air_jumps_remaining: 1,
            air_dashes_remaining: 0,
            dash_cooldown_ms: 250.0,
            air_ticks: 4,
            movement_state: WireMovementState::Dash {
                elapsed_ms: 33.0,
                boost: [4.0, 0.0, -1.0],
            },
            coyote_timer_ms: 12.0,
            jump_buffer_timer_ms: 8.0,
            jump_spent: true,
            capsule_half_height: 0.4,
            capsule_eye_height: 0.25,
        }
    }

    #[test]
    fn component_to_wire_round_trips_mutable_subset() {
        let mut component = PlayerMovementComponent::from_descriptor(&rich_descriptor());
        // Drive the mutable subset to non-default values, then extract and merge
        // back into a fresh component and confirm the subset survives.
        component.velocity = Vec3::new(2.0, -0.5, 9.0);
        component.is_grounded = false;
        component.air_jumps_remaining = 1;
        component.air_dashes_remaining = 1;
        component.dash_cooldown_ms = 400.0;
        component.air_ticks = 6;
        component.movement_state = MovementState::Crouching { eye_current: 0.33 };
        component.coyote_timer_ms = 50.0;
        component.jump_buffer_timer_ms = 20.0;
        component.jump_spent = true;
        component.capsule.half_height = 0.4;
        component.capsule.eye_height = 0.25;

        let wire = movement_state_to_wire(&component);
        let mut rebuilt = PlayerMovementComponent::from_descriptor(&rich_descriptor());
        merge_wire_into_movement_state(&mut rebuilt, &wire);

        assert!((rebuilt.velocity - component.velocity).length() < EPSILON);
        assert_eq!(rebuilt.is_grounded, component.is_grounded);
        assert_eq!(rebuilt.air_jumps_remaining, component.air_jumps_remaining);
        assert_eq!(rebuilt.air_dashes_remaining, component.air_dashes_remaining);
        assert!((rebuilt.dash_cooldown_ms - component.dash_cooldown_ms).abs() < EPSILON);
        assert_eq!(rebuilt.air_ticks, component.air_ticks);
        assert_eq!(rebuilt.movement_state, component.movement_state);
        assert!((rebuilt.coyote_timer_ms - component.coyote_timer_ms).abs() < EPSILON);
        assert!((rebuilt.jump_buffer_timer_ms - component.jump_buffer_timer_ms).abs() < EPSILON);
        assert_eq!(rebuilt.jump_spent, component.jump_spent);
        assert!((rebuilt.capsule.half_height - component.capsule.half_height).abs() < EPSILON);
        assert!((rebuilt.capsule.eye_height - component.capsule.eye_height).abs() < EPSILON);
    }

    #[test]
    fn merge_writes_mutable_subset_and_leaves_descriptor_fields_untouched() {
        let mut component = PlayerMovementComponent::from_descriptor(&rich_descriptor());

        // Snapshot the descriptor-owned (non-mutable) fields before the merge.
        let ground = component.ground.clone();
        let air = component.air.clone();
        let fall = component.fall.clone();
        let dash = component.dash.clone();
        let crouch = component.crouch.clone();
        let view_feel = component.view_feel.clone();
        let cos_walkable = component.cos_walkable;
        let standing_half_height = component.standing_half_height;
        let standing_eye_height = component.standing_eye_height;
        let stuck_stop_enabled = component.stuck_stop_enabled;
        let stuck_stop_threshold = component.stuck_stop_threshold;
        let coyote_ms = component.coyote_ms;
        let jump_buffer_ms = component.jump_buffer_ms;

        let wire = sample_wire_state();
        merge_wire_into_movement_state(&mut component, &wire);

        // Mutable subset updated from the wire payload.
        assert!((component.velocity - Vec3::new(1.0, -2.0, 3.5)).length() < EPSILON);
        assert!(component.is_grounded);
        assert_eq!(component.air_jumps_remaining, 1);
        assert_eq!(component.air_dashes_remaining, 0);
        assert!((component.dash_cooldown_ms - 250.0).abs() < EPSILON);
        assert_eq!(component.air_ticks, 4);
        assert_eq!(
            component.movement_state,
            MovementState::Dash {
                elapsed_ms: 33.0,
                boost: Vec3::new(4.0, 0.0, -1.0),
            }
        );
        assert!((component.coyote_timer_ms - 12.0).abs() < EPSILON);
        assert!((component.jump_buffer_timer_ms - 8.0).abs() < EPSILON);
        assert!(component.jump_spent);
        assert!((component.capsule.half_height - 0.4).abs() < EPSILON);
        assert!((component.capsule.eye_height - 0.25).abs() < EPSILON);

        // Descriptor-owned fields untouched by the merge.
        assert_eq!(component.ground, ground);
        assert_eq!(component.air, air);
        assert_eq!(component.fall, fall);
        assert_eq!(component.dash, dash);
        assert_eq!(component.crouch, crouch);
        assert_eq!(component.view_feel, view_feel);
        assert!((component.cos_walkable - cos_walkable).abs() < EPSILON);
        assert!((component.standing_half_height - standing_half_height).abs() < EPSILON);
        assert!((component.standing_eye_height - standing_eye_height).abs() < EPSILON);
        assert_eq!(component.stuck_stop_enabled, stuck_stop_enabled);
        assert!((component.stuck_stop_threshold - stuck_stop_threshold).abs() < EPSILON);
        assert!((component.coyote_ms - coyote_ms).abs() < EPSILON);
        assert!((component.jump_buffer_ms - jump_buffer_ms).abs() < EPSILON);
        // The standing reference dimensions are NOT the live capsule the wire wrote.
        assert!((component.standing_half_height - 0.8).abs() < EPSILON);
        assert!((component.standing_eye_height - 0.5).abs() < EPSILON);
    }

    /// Per-variant `MovementState` round-trip (Normal/Dash/Crouching) through the
    /// wire enum. Constructed from the source enum via an exhaustive list so a new
    /// variant must be added here to compile-cover it.
    #[test]
    fn movement_state_variants_round_trip_through_wire() {
        let variants = [
            MovementState::Normal,
            MovementState::Dash {
                elapsed_ms: 12.5,
                boost: Vec3::new(1.0, -2.0, 3.0),
            },
            MovementState::Crouching { eye_current: 0.9 },
        ];
        for state in variants {
            let wire = movement_state_enum_to_wire(state);
            let rebuilt = wire_to_movement_state_enum(wire);
            assert_eq!(rebuilt, state, "movement state must round-trip: {state:?}");
        }
    }
}
