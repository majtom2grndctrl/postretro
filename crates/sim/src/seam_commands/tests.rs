//! Host-only proofs for level-seam arm/disarm: ordered next-tick application,
//! newly-armed edges, untouched latches, and the client role.
//! See: context/lib/entity_model.md §7e (Level Seams)

use super::*;
use crate::trigger_commands::{BoundTarget, BoundTriggerCommand, TriggerFireContext};
use postretro_entities::Transform;
use postretro_foundation::{
    AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementComponent,
    PlayerMovementDescriptor, SpeedParams,
};
use postretro_scripting_core::ir_scopes::DispatchScope;

fn seam(ctx: &ScriptCtx, index: u16, armed: bool) -> EntityId {
    let mut registry = ctx.registry.borrow_mut();
    let id = registry.spawn(Transform::default());
    registry
        .set_component(
            id,
            SeamComponent {
                index,
                source_entity: u32::from(index) + 1,
                name: format!("seam{index}"),
                armed,
                on_arrive: String::new(),
            },
        )
        .unwrap();
    id
}

fn armed(ctx: &ScriptCtx, id: EntityId) -> bool {
    ctx.registry
        .borrow()
        .get_component::<SeamComponent>(id)
        .unwrap()
        .armed
}

fn movement() -> PlayerMovementComponent {
    PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
        sounds: None,
        knockback: Default::default(),
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
            friction: GroundParams::DEFAULT_FRICTION,
            stop_speed: GroundParams::DEFAULT_STOP_SPEED,
        },
        air: AirParams {
            forward_steer: 0.0,
            accel: 0.7,
            max_control_speed: 0.5,
            bunny_hop: false,
            jumps: 0,
            jump_velocity: 5.5,
            jump_ceiling: 0.0,
        },
        fall: FallParams {
            terminal_velocity: 40.0,
        },
        stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
        stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
        dash: None,
        forgiveness: None,
        crouch: None,
        slide: None,
        swim: None,
        vault: None,
        view_feel: None,
    })
}

#[test]
fn seam_member_commands_apply_at_next_tick_in_order_and_report_newly_armed_edges() {
    let ctx = ScriptCtx::new();
    let first = seam(&ctx, 0, false);
    let second = seam(&ctx, 1, true);
    let mut steps = SequencedPrimitiveRegistry::new();
    register_sequenced_seam_primitives(&mut steps, ctx.clone());
    for name in ["armSeam", "disarmSeam"] {
        steps.get(name).unwrap()(first, &serde_json::json!({})).unwrap();
    }
    steps.get("disarmSeam").unwrap()(second, &serde_json::json!({})).unwrap();
    assert!(!armed(&ctx, first), "a queued arm waits for the next tick");
    assert!(
        armed(&ctx, second),
        "a queued disarm waits for the next tick"
    );
    assert_eq!(ctx.pending_seam_mutations.borrow().len(), 3);

    let mut newly_armed = Vec::new();
    apply_pending_seam_mutations(&ctx, &mut newly_armed);
    assert!(!armed(&ctx, first), "arm then disarm applies in order");
    assert!(!armed(&ctx, second));
    assert_eq!(
        newly_armed,
        [0],
        "the arm before the disarm was a false-to-true edge"
    );
    assert!(ctx.pending_seam_mutations.borrow().is_empty());

    // Arming an armed seam is no edge; arming a disarmed one is.
    newly_armed.clear();
    steps.get("armSeam").unwrap()(first, &serde_json::json!({})).unwrap();
    steps.get("armSeam").unwrap()(first, &serde_json::json!({})).unwrap();
    steps.get("armSeam").unwrap()(second, &serde_json::json!({})).unwrap();
    apply_pending_seam_mutations(&ctx, &mut newly_armed);
    assert!(armed(&ctx, first) && armed(&ctx, second));
    assert_eq!(newly_armed, [0, 1]);
}

#[test]
fn seam_reaction_primitives_enqueue_and_skip_non_seam_targets() {
    let ctx = ScriptCtx::new();
    let gate = seam(&ctx, 2, false);
    let stranger = ctx.registry.borrow_mut().spawn(Transform::default());
    let mut reactions = ReactionPrimitiveRegistry::new();
    register_seam_reaction_primitives(&mut reactions, ctx.clone());
    let arm = reactions.get("armSeam").expect("armSeam registers");
    {
        let mut registry = ctx.registry.borrow_mut();
        arm(&mut registry, &[gate, stranger], &serde_json::json!({})).unwrap();
    }
    assert_eq!(
        ctx.pending_seam_mutations.borrow().as_slice(),
        [(gate, true)],
        "a target without a seam component is skipped"
    );
    let mut newly_armed = Vec::new();
    apply_pending_seam_mutations(&ctx, &mut newly_armed);
    assert!(armed(&ctx, gate));
    assert_eq!(newly_armed, [2]);
}

#[test]
fn arming_a_seam_keeps_every_latch() {
    let ctx = ScriptCtx::new();
    let gate = seam(&ctx, 0, false);
    let pawn = {
        let mut registry = ctx.registry.borrow_mut();
        let pawn = registry.spawn(Transform::default());
        let mut component = movement();
        // A teleporter latch sharing the seam's index: a teleporter's arm
        // clears it, a seam's never does.
        component.teleport.arrival_latches =
            postretro_foundation::TeleportLatches::from_slice(&[0]);
        registry.set_component(pawn, component).unwrap();
        pawn
    };
    ctx.pending_seam_mutations.borrow_mut().push((gate, true));
    let mut newly_armed = Vec::new();
    apply_pending_seam_mutations(&ctx, &mut newly_armed);
    assert!(armed(&ctx, gate));
    assert_eq!(newly_armed, [0]);
    assert_eq!(
        ctx.registry
            .borrow()
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .teleport
            .arrival_latches,
        [0],
        "arming keeps latches"
    );
}

#[test]
fn a_connected_client_queues_nothing_and_clears_a_stale_queue() {
    let ctx = ScriptCtx::new();
    let gate = seam(&ctx, 0, false);
    let mut steps = SequencedPrimitiveRegistry::new();
    register_sequenced_seam_primitives(&mut steps, ctx.clone());
    // A queue left from before the role changed is dropped, not applied.
    ctx.pending_seam_mutations.borrow_mut().push((gate, true));
    ctx.owner_slot_writes_enabled.set(false);
    steps.get("armSeam").unwrap()(gate, &serde_json::json!({})).unwrap();
    assert_eq!(ctx.pending_seam_mutations.borrow().len(), 1);
    let mut newly_armed = Vec::new();
    apply_pending_seam_mutations(&ctx, &mut newly_armed);
    assert!(ctx.pending_seam_mutations.borrow().is_empty());
    assert!(!armed(&ctx, gate));
    assert!(newly_armed.is_empty());
}

#[test]
fn bound_seam_commands_queue_against_the_fired_seam() {
    let ctx = ScriptCtx::new();
    let gate = seam(&ctx, 0, true);
    let command = BoundTriggerCommand::SeamArmed {
        target: BoundTarget::FiredSeam,
        armed: false,
    };
    let mut scope = DispatchScope::script(ctx.clone(), &[]);
    let fire = TriggerFireContext {
        fired_seam: Some(gate),
        ..TriggerFireContext::default()
    };
    {
        let mut registry = ctx.registry.borrow_mut();
        command.execute_with_script_ctx(
            &mut registry,
            &ctx,
            &mut scope,
            &Default::default(),
            &Default::default(),
            &fire,
        );
    }
    assert_eq!(
        ctx.pending_seam_mutations.borrow().as_slice(),
        [(gate, false)]
    );
    // Without a fired seam the token resolves to nothing.
    {
        let mut registry = ctx.registry.borrow_mut();
        command.execute_with_script_ctx(
            &mut registry,
            &ctx,
            &mut scope,
            &Default::default(),
            &Default::default(),
            &TriggerFireContext::default(),
        );
    }
    assert_eq!(ctx.pending_seam_mutations.borrow().len(), 1);
}
