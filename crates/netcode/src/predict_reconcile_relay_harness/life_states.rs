//! E18 AC17/O9: authoritative profile transitions through the production relay.
use super::super::predict_reconcile_harness_test_fixtures::{
    NO_CORRECTION_M, production_snapshot_stamp,
};
use super::super::prediction::TELEPORT_CORRECTION_MIN_M;
use super::*;
use postretro_entities::{LifeStatesDescriptor, ScriptCtx};
use postretro_foundation::{
    LifeStateMovementProfile, LifeStatePosture, LifeStateProfile, PlayerMovementComponent,
    enter_life_state,
};
use std::collections::BTreeMap;

fn life_harness(speed: f32, vaulting: bool) -> (RelayHarness, ScriptCtx) {
    let link = LinkConfig {
        delay: 75,
        ..LinkConfig::perfect()
    };
    let mut h = RelayHarness::new(
        link,
        LinkConfig {
            seed: link.seed ^ 1,
            ..link
        },
    );
    let down = LifeStateProfile {
        movement: LifeStateMovementProfile {
            speed_scale: 0.3,
            abilities: false,
            posture: LifeStatePosture::Crouch,
        },
        look: false,
        ..Default::default()
    };
    let descriptor = h
        .descriptors
        .iter_mut()
        .find(|descriptor| descriptor.movement.is_some())
        .unwrap();
    let movement = descriptor.movement.as_mut().unwrap();
    movement.ground.speed.run = speed;
    if vaulting {
        movement.vault = Some(vault(VaultEntry {
            ground: Some(VaultTrigger::Auto),
            ..Default::default()
        }));
        h.world = ledge_world();
    }
    descriptor.life_states = Some(LifeStatesDescriptor {
        initial: "alive".into(),
        states: BTreeMap::from([
            ("alive".into(), LifeStateProfile::default()),
            ("down".into(), down.clone()),
        ]),
        rejoin: Default::default(),
        state_ids: Default::default(),
        table_revision: 0,
    });
    let ctx = ScriptCtx::new();
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(h.descriptors.clone());
    h.descriptors = ctx.data_registry.borrow().entities.clone();
    let descriptor = h
        .descriptors
        .iter()
        .find(|descriptor| descriptor.movement.is_some())
        .unwrap();
    let block = descriptor.life_states.as_ref().unwrap();
    let (id, profile) = block.initial_state();
    let mut pawn = PlayerMovementComponent::from_descriptor(descriptor.movement.as_ref().unwrap());
    enter_life_state(&mut pawn, id, profile, block.table_revision);
    h.host_registry.set_component(h.host_pawn, pawn).unwrap();
    (h, ctx)
}

fn baseline(h: &RelayHarness) -> f32 {
    let samples: Vec<_> = h
        .corrections
        .iter()
        .rev()
        .filter(|correction| !correction.sample.first_record_adoption)
        .take(10)
        .collect();
    assert_eq!(
        samples.len(),
        10,
        "ten steady snapshots ground the no-transition bound"
    );
    let worst = samples
        .iter()
        .map(|correction| correction.sample.magnitude)
        .fold(0.0_f32, f32::max);
    assert!(
        worst <= NO_CORRECTION_M,
        "steady baseline is {worst}, above {NO_CORRECTION_M}"
    );
    worst
}

fn assert_one_correction(h: &RelayHarness, first_stamp: u32, baseline: f32, label: &str) {
    let samples: Vec<_> = h
        .corrections
        .iter()
        .filter(|correction| {
            correction.sample.server_tick >= first_stamp && !correction.sample.first_record_adoption
        })
        .collect();
    assert!(
        samples.len() >= 2,
        "{label}: snapshot k and k+1 must both arrive"
    );
    let limit = baseline + NO_CORRECTION_M;
    assert!(
        samples[0].sample.class != CorrectionClass::Teleport,
        "{label}: snapshot k must smooth"
    );
    assert!(
        samples[0].sample.magnitude > limit,
        "{label}: the transition must actually cause a correction at k: {samples:?}"
    );
    assert!(
        samples
            .windows(2)
            .all(|pair| pair[0].sample.server_tick < pair[1].sample.server_tick),
        "{label}: k+1 is a distinct accepted snapshot"
    );
    assert!(
        samples[1].sample.magnitude <= limit,
        "{label}: k+1 correction {} exceeds no-transition baseline {baseline} + {NO_CORRECTION_M}",
        samples[1].sample.magnitude
    );
    assert!(
        samples
            .iter()
            .skip(1)
            .all(|correction| correction.sample.magnitude <= limit),
        "{label}: repeated corrections while both peers share the state: {samples:?}"
    );
    assert!(
        samples
            .iter()
            .filter(|correction| correction.sample.magnitude > limit)
            .count()
            <= 1,
        "{label}: at most one transition correction"
    );
}

fn transition_run(speed: f32) -> RelayHarness {
    let (mut h, ctx) = life_harness(speed, false);
    let revision = ctx.data_registry.borrow().life_state_table_revision;
    h.join();
    for _ in 0..60 {
        h.frame(&forward_command(false));
    }
    assert_eq!(
        h.client_tuning().unwrap().life_state_names,
        ["alive", "down"]
    );
    let alive_baseline = baseline(&h);
    h.corrections.clear();
    let down_stamp = production_snapshot_stamp(h.host_tick.wrapping_add(1));
    postretro_sim::life_state_commands::set_life_state(
        &ctx,
        &mut h.host_registry,
        &[h.host_pawn],
        "down",
    );
    // The host continues accepting commands made before the owner receives look:false.
    let mut turning = forward_command(false);
    turning.movement.facing_yaw = 0.1;
    turning.movement.facing_pitch = 0.25;
    for _ in 0..50 {
        h.frame(&turning);
    }
    let pawn = h.client_pawn.unwrap();
    let client = h
        .client_registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap();
    assert_eq!(
        (client.life_state, client.life_state_table_revision),
        (1, revision)
    );
    assert_eq!(client.life_speed_scale, 0.3);
    assert_eq!(client.life_posture, LifeStatePosture::Crouch);
    assert_eq!(
        client.capsule.half_height,
        client.crouch.as_ref().unwrap().half_height
    );
    let resolved = h
        .host_trace
        .iter()
        .rev()
        .find_map(|trace| trace.resolved.as_ref())
        .unwrap();
    assert!(
        (resolved.command.movement.facing_pitch - 0.25).abs() < 1.0e-5,
        "host never clamps facing made while the transition travels to the owner"
    );
    assert!((resolved.command.movement.facing_yaw - 0.1).abs() < 1.0e-5);
    assert!(
        h.corrections
            .iter()
            .all(|correction| correction.sample.class != CorrectionClass::Teleport),
        "life transitions never cut the owning eye: {:?}",
        h.corrections
    );
    assert_one_correction(&h, down_stamp, alive_baseline, "run to down");
    for _ in 0..120 {
        h.frame(&idle_command());
    }
    assert!(h.position_error().unwrap() < SETTLED_PAWN_TOLERANCE_M);
    // The inverse transition is the second AC17 case, under the same live link.
    let down_baseline = baseline(&h);
    let alive_stamp = production_snapshot_stamp(h.host_tick.wrapping_add(1));
    postretro_sim::life_state_commands::set_life_state(
        &ctx,
        &mut h.host_registry,
        &[h.host_pawn],
        "alive",
    );
    for _ in 0..60 {
        h.frame(&forward_command(false));
    }
    assert_eq!(
        h.client_registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .life_state,
        0
    );
    assert!(
        h.corrections
            .iter()
            .all(|correction| correction.sample.class != CorrectionClass::Teleport)
    );
    assert_one_correction(&h, alive_stamp, down_baseline, "down to full run");
    for _ in 0..120 {
        h.frame(&idle_command());
    }
    assert!(h.position_error().unwrap() < SETTLED_PAWN_TOLERANCE_M);
    h
}

#[test]
fn life_state_relay_150ms_transition_reconciles_without_a_camera_cut() {
    let h = transition_run(11.0);
    assert!(
        h.corrections
            .iter()
            .any(|correction| correction.sample.magnitude > 0.01)
    );
}

#[test]
fn life_state_relay_150ms_above_teleport_floor_still_glides() {
    let h = transition_run(80.0);
    assert!(
        h.corrections
            .iter()
            .any(
                |correction| correction.sample.magnitude >= TELEPORT_CORRECTION_MIN_M
                    && correction.sample.class == CorrectionClass::OversizedSmoothed
            ),
        "the high-speed transition must actually cross the floor: {:?}",
        h.corrections
    );
}

#[test]
fn life_state_vault_event_write_clears_t_record_and_next_snapshot_matches_baseline() {
    use super::super::movement_state::movement_state_to_wire;
    use postretro_net::wire::{WireMovementState, WirePlayerMovementState};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Transition {
        tick: u32,
        stamp: u32,
        wire: WirePlayerMovementState,
    }

    let (mut h, ctx) = life_harness(11.0, true);
    let (mut control, _) = life_harness(11.0, true);
    h.join();
    control.join();
    assert_eq!(
        h.host_tick, control.host_tick,
        "paired no-transition course starts on the same host tick"
    );
    let transition = Rc::new(RefCell::new(None::<Transition>));
    let written = Rc::clone(&transition);
    h.post_movement = Some(Box::new(move |registry, pawn, tick| {
        if written.borrow().is_some() {
            return;
        }
        let movement = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap();
        if !matches!(
            movement.movement_state,
            postretro_foundation::MovementState::Vault { .. }
        ) {
            return;
        }
        // Choose a real snapshot tick so T's record, rather than a later record,
        // witnesses the player-event write after movement and before serialize.
        let mut completed = tick;
        let mut due = false;
        super::super::complete_host_fixed_tick(&mut completed, &mut due);
        if !due {
            return;
        }
        assert!(matches!(
            movement_state_to_wire(movement, 0.0).movement_state,
            WireMovementState::Vault { .. }
        ));
        postretro_sim::life_state_commands::set_life_state(&ctx, registry, &[pawn], "down");
        let movement = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap();
        assert!(movement.vault_barred);
        let wire = movement_state_to_wire(movement, 0.0);
        assert!(
            !matches!(wire.movement_state, WireMovementState::Vault { .. }),
            "O43: the production state verb ends the active vault before T serializes"
        );
        assert_eq!(
            wire.life_state_elapsed_ticks, 0,
            "the write occurs after T movement"
        );
        *written.borrow_mut() = Some(Transition {
            tick,
            stamp: production_snapshot_stamp(completed),
            wire,
        });
    }));
    for _ in 0..180 {
        h.frame(&forward_command(false));
        control.frame(&forward_command(false));
    }
    let transition = transition.borrow();
    let transition = transition
        .as_ref()
        .expect("the host entered a vault and the event wrote down on a snapshot tick");
    assert_eq!(transition.wire.life_state, 1);
    assert!(
        h.host_trace
            .iter()
            .any(|trace| trace.server_tick == transition.tick
                && trace.movement_state == MovementStateKind::Vault),
        "T movement was vaulting before the event"
    );
    assert!(
        h.host_trace
            .iter()
            .filter(|trace| trace.server_tick > transition.tick)
            .all(|trace| trace.movement_state != MovementStateKind::Vault),
        "host stays out of vault from T+1"
    );
    let baseline = control
        .corrections
        .iter()
        .filter(|correction| correction.sample.server_tick >= transition.stamp)
        .map(|correction| correction.sample.magnitude)
        .fold(0.0_f32, f32::max);
    assert!(
        baseline <= NO_CORRECTION_M,
        "matched no-transition vault course baseline {baseline}"
    );
    assert_one_correction(&h, transition.stamp, baseline, "vault event interruption");
    let samples: Vec<_> = h
        .corrections
        .iter()
        .filter(|correction| correction.sample.server_tick >= transition.stamp)
        .collect();
    let next = samples[1];
    let control_next = control
        .corrections
        .iter()
        .find(|correction| correction.sample.server_tick == next.sample.server_tick)
        .expect("paired baseline has snapshot k+1");
    assert!(
        next.sample.magnitude <= control_next.sample.magnitude + NO_CORRECTION_M,
        "O43/O13: k+1 exceeds the matched no-transition snapshot baseline"
    );
    assert!(
        samples
            .iter()
            .all(|correction| correction.movement_state != Some(MovementStateKind::Vault)),
        "client merge/restamp/replay agrees with the host's interrupted vault"
    );
    let pawn = h.client_pawn.unwrap();
    let movement = h
        .client_registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap();
    assert!(movement.vault_barred);
    assert_eq!(movement.life_state, 1);
}
