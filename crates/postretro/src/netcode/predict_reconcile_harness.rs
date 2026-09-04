// Integrated, production-adjacent prediction/reconciliation tests plus the headline
// deterministic latency gate. Every test drives the real production seams through
// `LoopbackHarness` (see the sibling `predict_reconcile_harness_test_fixtures`);
// the prototype `sim::predict_reconcile` type is never instantiated.
// See: context/lib/networking.md · context/lib/testing_guide.md
//
// Replay-purity guard: the production replay path (`prediction::replay`) is
// registry-blind by signature (no `EntityRegistry` parameter), so AI/weapons/death
// are structurally unreachable. These tests additionally assert it at the seam: a
// zero-HP bystander entity that the full `simulate_tick` death sweep WOULD latch
// stays alive on both ends through every scenario, proving the movement-only path
// never ran the registry-wide systems.

#![cfg(test)]

use glam::{Vec2, Vec3};

use postretro_net::harness::{LinkConfig, PacketConditioner};
use postretro_net::wire::{
    ClientControlMessage, ClientMessage, ClientSwitchDeclaration, ServerControlMessage,
    ServerSwitchAccepted, ServerSwitchRefused,
};

use super::movement_state::movement_state_to_wire;
use super::predict_reconcile_harness_test_fixtures::{
    CLIENT_ID, DT, GRAVITY, LoopbackHarness, MOVING_PLATFORM_ID, MOVING_PLATFORM_SPEED_MPS,
    ROTATING_PLATFORM_RIDER_START, component as player_component, downhill_facet_normal,
    faceted_floor_height, faceted_slope_world, forward_command, idle_command, input_at,
    use_command,
};
use super::prediction::{ORDINARY_CORRECTION_MAX_M, TELEPORT_CORRECTION_MIN_M};
use super::reconcile::reconcile_local_pawn;
use crate::kinematic_mover::apply_mover_command;
use crate::movement::MovementInput;
use crate::netcode::host_handle_client_message;
use crate::netcode::{
    CurrentSwitchResolution, HostSwitchDecision, PendingSwitchDeclaration, SwitchOutcome,
    apply_host_switch_declaration, resolve_switch_outcome,
};
use crate::sim::SimCommand;
use postretro_entities::{
    KinematicMoverComponent, MoverCommand, Transform, TriggerVolumeComponent,
    components::inventory::Inventory, components::weapon::WeaponComponent,
    components::wieldable_state::WieldableState,
};
use postretro_foundation::{
    FireMode, GroundRef, MovementState, PlayerMovementComponent, ResolutionMode, WeaponDescriptor,
};
use std::collections::VecDeque;

/// The mandated automated harness profile (Task 6 §B), applied in BOTH directions:
/// 45 ms base + up to 60 ms jitter (a 45..105 ms one-way range, ≈150 ms mean RTT),
/// 5% loss, fixed seed. Matches the Phase 2 latency harness profile exactly.
fn mandated_link() -> LinkConfig {
    LinkConfig {
        delay: 45,
        jitter: 60,
        loss_probability: 0.05,
        seed: 0x1502,
    }
}

/// A near-perfect link for the scenario tests that exercise the gap policy /
/// stale / duplicate seams deterministically without the full latency profile's
/// loss obscuring the assertion. A small fixed delay keeps the predict→ack loop
/// realistic (the client predicts ahead of the host) without jitter or loss.
fn light_link() -> LinkConfig {
    LinkConfig {
        delay: 32,
        jitter: 0,
        loss_probability: 0.0,
        seed: 0x6010,
    }
}

/// Plain loopback at ~zero latency: no delay, jitter, or loss. The host playout
/// buffer still warms up (a few ticks), but once armed the host resolves the
/// client's commands the same tick they arrive, so reconcile corrections are ~0 and
/// the presented-eye smoothness test sees only the structural jitter it targets.
fn loopback_link() -> LinkConfig {
    LinkConfig {
        delay: 0,
        jitter: 0,
        loss_probability: 0.0,
        seed: 0x100b,
    }
}

// ---------------------------------------------------------------------------
// Section A — Integrated scenario tests (drive the real seams end to end)
// ---------------------------------------------------------------------------

fn run_ordered_switch_pair(refuse_final_for_reload: bool) -> (usize, Vec<CurrentSwitchResolution>) {
    let mut host_registry = postretro_entities::EntityRegistry::new();
    let host_pawn = host_registry.spawn(Transform::default());
    let slot_a = host_registry.spawn(Transform::default());
    let slot_b = host_registry.spawn(Transform::default());
    let slot_c = host_registry.spawn(Transform::default());
    let test_weapon = || {
        WeaponComponent::from_descriptor(&WeaponDescriptor {
            damage: 10.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            range: 64.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            projectile: None,
            credit_source: None,
            third_person_model: None,
            viewmodel: None,
            placement: None,
            muzzle_offset: None,
            resource: None,
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: Some(true),
        })
    };
    let mut slot_b_weapon = test_weapon();
    if refuse_final_for_reload {
        let reloading = &mut slot_b_weapon;
        reloading.state = WieldableState::Reloading;
        reloading.state_total_ms = 100;
        reloading.state_remaining_ms = 50;
    }
    host_registry.set_component(slot_a, test_weapon()).unwrap();
    host_registry.set_component(slot_b, slot_b_weapon).unwrap();
    host_registry.set_component(slot_c, test_weapon()).unwrap();
    let mut inventory = Inventory::default();
    inventory.wieldables[0] = Some(slot_a);
    inventory.wieldables[1] = Some(slot_b);
    inventory.wieldables[2] = Some(slot_c);
    host_registry.set_component(host_pawn, inventory).unwrap();
    let mut weapon_owners = crate::netcode::WeaponOwners::new();

    let link = LinkConfig {
        delay: 32,
        jitter: 0,
        loss_probability: 0.0,
        seed: 0xe16,
    };
    let mut to_host = PacketConditioner::new(link);
    let mut to_client = PacketConditioner::new(link);
    for declaration in [
        ClientSwitchDeclaration {
            declaration_id: 41,
            slot: 1,
        },
        ClientSwitchDeclaration {
            declaration_id: 42,
            slot: 2,
        },
    ] {
        to_host.enqueue(postretro_net::wire::encode(
            &ClientControlMessage::SwitchDeclaration(declaration),
        ));
    }

    to_host.advance(32);
    for packet in to_host.take_ready() {
        let ClientControlMessage::SwitchDeclaration(declaration) =
            postretro_net::wire::decode(&packet).expect("decode conditioned declaration")
        else {
            panic!("switch harness carries declarations only");
        };
        let outcome = match apply_host_switch_declaration(
            &mut host_registry,
            host_pawn,
            &mut weapon_owners,
            usize::from(declaration.slot),
            false,
        ) {
            HostSwitchDecision::Accepted => {
                ServerControlMessage::SwitchAccepted(ServerSwitchAccepted {
                    declaration_id: declaration.declaration_id,
                    slot: declaration.slot,
                })
            }
            HostSwitchDecision::Refused => {
                ServerControlMessage::SwitchRefused(ServerSwitchRefused {
                    declaration_id: declaration.declaration_id,
                    slot: declaration.slot,
                })
            }
        };
        to_client.enqueue(postretro_net::wire::encode(&outcome));
    }

    let mut pending = VecDeque::from([
        PendingSwitchDeclaration {
            declaration_id: 41,
            target_slot: 1,
            held_origin_slot: 0,
            rollback_slot: 0,
            rollback_last_weapon_slot: Some(9),
        },
        PendingSwitchDeclaration {
            declaration_id: 42,
            target_slot: 2,
            held_origin_slot: 0,
            rollback_slot: 0,
            rollback_last_weapon_slot: Some(0),
        },
    ]);
    to_client.advance(32);
    let resolutions = to_client
        .take_ready()
        .into_iter()
        .map(|packet| {
            let control: ServerControlMessage =
                postretro_net::wire::decode(&packet).expect("decode conditioned outcome");
            let outcome = match control {
                ServerControlMessage::SwitchAccepted(accepted) => SwitchOutcome::Accepted(accepted),
                ServerControlMessage::SwitchRefused(refused) => SwitchOutcome::Refused(refused),
                _ => panic!("switch harness carries switch outcomes only"),
            };
            resolve_switch_outcome(&mut pending, outcome)
        })
        .collect::<Vec<_>>();
    assert!(pending.is_empty(), "both reliable outcomes settle in order");
    let active_slot = host_registry
        .get_component::<Inventory>(host_pawn)
        .unwrap()
        .active_slot;
    (active_slot, resolutions)
}

#[test]
fn o29_two_peer_accept_accept_keeps_actual_held_slot_as_last_weapon() {
    // Regression: ordered A->B then A->C declarations made unpresented B become
    // the client's last weapon when both host validations accepted.
    let (host_active, resolutions) = run_ordered_switch_pair(false);
    assert_eq!(host_active, 2);
    assert_eq!(
        resolutions,
        vec![
            CurrentSwitchResolution::None,
            CurrentSwitchResolution::Accepted {
                last_weapon_slot: Some(0),
            },
        ]
    );
}

#[test]
fn o25_o29_two_peer_accept_refuse_restores_host_slot_without_inventing_history() {
    let (host_active, resolutions) = run_ordered_switch_pair(true);
    assert_eq!(host_active, 1);
    assert_eq!(
        resolutions,
        vec![
            CurrentSwitchResolution::None,
            CurrentSwitchResolution::Refused {
                target_slot: 2,
                rollback_slot: 1,
                last_weapon_slot: Some(0),
            },
        ]
    );
}

// --- Ordered input: a steady forward-walk command stream converges; the client
// reconciled pawn tracks the host authority, with the local pawn driven by
// prediction (it is ahead of the host until the ack lands). ---
#[test]
fn ordered_input_converges_client_to_host_authority() {
    let mut h = LoopbackHarness::new(light_link());

    // 90 ticks of steady forward input.
    for _ in 0..90 {
        h.step(&forward_command(false));
    }
    // Drain to the explicit condition.
    drain(&mut h);

    assert!(h.is_drained(), "harness must reach the drain condition");
    let err = h.position_error();
    assert!(
        err <= 0.05,
        "ordered input: client converges to host authority within 0.05 m; error={err}"
    );
    // The pawn actually moved forward (the scenario is not a degenerate no-op).
    assert!(
        h.host_position().z < -1.0,
        "the forward-walk scenario advanced the host pawn along -Z"
    );
    assert!(
        h.bystanders_alive(),
        "death sweep never ran (movement-only path)"
    );
}

// --- Missing input: dropping a contiguous run of client commands triggers the
// host hold-3-then-neutral gap policy, but once input resumes and packets drain
// the client still converges to the authority. ---
#[test]
fn missing_input_gap_policy_still_converges() {
    let mut h = LoopbackHarness::new(light_link());

    // Arm (run until the first baseline arms prediction), then a few more clean ticks.
    h.step_until_armed(&forward_command(false));
    for _ in 0..10 {
        h.step(&forward_command(false));
    }
    assert!(
        h.prediction.is_armed(),
        "prediction armed after first baseline"
    );

    // Now SKIP sending input for several ticks (the client neither predicts nor
    // sends) while the host keeps ticking — this is a contiguous input gap. The
    // host holds, then synthesizes neutral.
    for _ in 0..8 {
        h.drain_step();
    }

    // Resume steady input.
    for _ in 0..40 {
        h.step(&forward_command(false));
    }
    drain(&mut h);

    assert!(h.is_drained());
    let err = h.position_error();
    assert!(
        err <= 0.05,
        "missing-input gap policy still converges within 0.05 m; error={err}"
    );
    assert!(h.bystanders_alive());
}

// --- Duplicate input injected directly at the host_handle_client_message drain
// seam: an exact-duplicate ClientMessage::Input collapses to one queued command
// and never mutates another client's state or panics the host. ---
#[test]
fn duplicate_input_at_drain_seam_is_inert() {
    let mut h = LoopbackHarness::new(light_link());

    // Inject the same input tick three times directly at the seam (no transport).
    let dup = input_at(0, 1.0);
    for _ in 0..3 {
        host_handle_client_message(
            &mut h.server,
            &mut h.server_replication,
            &mut h.server_state,
            &mut h.command_queues,
            CLIENT_ID,
            0,
            0,
            ClientMessage::Input(dup),
        );
    }
    // Fix A (playout buildup): the buildup latch withholds the first real command until
    // the pending queue reaches INPUT_BUFFER_TARGET, so a second distinct tick is queued
    // to disarm the latch and let the de-duplicated tick 0 resolve. The flood still
    // collapses to ONE tick-0 command — proven by the cursor advancing to exactly 0 (not
    // past it) on the first resolve.
    host_handle_client_message(
        &mut h.server,
        &mut h.server_replication,
        &mut h.server_state,
        &mut h.command_queues,
        CLIENT_ID,
        0,
        0,
        ClientMessage::Input(input_at(1, 1.0)),
    );

    // A second, unrelated client's queue is untouched by the flood. It likewise needs
    // INPUT_BUFFER_TARGET depth before its first command resolves.
    const OTHER: u64 = 99;
    for wish_tick in [(0u32, -1.0_f32), (1, -1.0)] {
        host_handle_client_message(
            &mut h.server,
            &mut h.server_replication,
            &mut h.server_state,
            &mut h.command_queues,
            OTHER,
            0,
            0,
            ClientMessage::Input(input_at(wish_tick.0, wish_tick.1)),
        );
    }

    // The duplicated tick resolves exactly once with the first-arrival intent.
    let resolved = h.command_queues.resolved_cursor(CLIENT_ID);
    // Resolve the single de-duplicated command and confirm the cursor advances to
    // exactly tick 0 (the collapse held — the flood did not queue three ticks).
    let r = run_resolve(&mut h, CLIENT_ID);
    assert!(r.is_some(), "the single de-duplicated command resolves");
    assert_eq!(
        h.command_queues.resolved_cursor(CLIENT_ID),
        Some(0),
        "the duplicate collapsed to one resolved tick"
    );
    assert!(
        resolved.is_none(),
        "cursor was unset before the first resolve"
    );

    // The other client is intact and resolves its own distinct intent.
    let other = run_resolve(&mut h, OTHER).expect("other client resolves its own command");
    assert!(
        (other.command.movement.wish_dir.y - (-1.0)).abs() < 1e-6,
        "the unrelated client kept its own intent through the duplicate flood"
    );
    assert!(h.bystanders_alive());
}

// --- Stale authoritative snapshot: an out-of-order older snapshot delivered after
// a newer one is rejected wholesale by apply_snapshot (sequence guard), so it never
// regresses the reconciled pawn or mutates unrelated entities. ---
#[test]
fn stale_snapshot_is_rejected_and_does_not_regress() {
    let mut h = LoopbackHarness::new(light_link());

    // Run forward so the client is armed and tracking, then drain.
    for _ in 0..40 {
        h.step(&forward_command(false));
    }
    drain(&mut h);
    let converged = h.client_position().expect("client armed");
    let bystander_before = h.bystanders_alive();

    // Capture the current latest sequence, then synthesize a STALE raw snapshot
    // (an older sequence) carrying a wildly different pose and feed it through the
    // real apply path. The sequence guard must reject it.
    let stale = stale_snapshot_for(&h);
    let outcome = h
        .client_replication
        .apply_snapshot(&mut h.client_registry, &stale);
    assert!(
        outcome.ack.is_none(),
        "a stale (old-sequence) snapshot is rejected wholesale — no ack"
    );
    assert!(
        outcome.local_reconcile.is_none(),
        "a rejected snapshot surfaces no reconcile input"
    );

    // The reconciled pawn did not move to the stale pose.
    let after = h.client_position().expect("client still armed");
    assert!(
        (after - converged).length() < 1e-4,
        "a stale snapshot does not regress the reconciled pawn"
    );
    assert_eq!(
        h.bystanders_alive(),
        bystander_before,
        "a stale snapshot mutates no unrelated entity"
    );
}

// --- Unknown local mapping: reconcile_local_pawn is a no-op when the record's
// entity is not the armed pawn (an unknown / stale local mapping). It returns None
// and touches no entity. ---
#[test]
fn unknown_local_mapping_reconcile_is_no_op() {
    let mut h = LoopbackHarness::new(light_link());
    h.step_until_armed(&forward_command(false));
    let pawn = h.client_pawn.expect("armed");
    let before = h.client_position().unwrap();

    // A bystander entity id that is NOT the armed pawn. Reconciling a record for it
    // must be ignored (the armed-entity guard).
    let stranger = h.client_bystander;
    assert_ne!(stranger, pawn);
    let class = reconcile_local_pawn(
        &mut h.client_registry,
        &mut h.prediction,
        stranger,
        Transform {
            position: Vec3::new(999.0, 1.0, 999.0),
            ..Transform::default()
        },
        None,
        Some(0),
        &h.world,
        GRAVITY,
        DT,
    );
    assert!(
        class.is_none(),
        "a record for an unknown/non-armed entity reconciles to nothing"
    );
    let after = h.client_position().unwrap();
    assert!(
        (after - before).length() < 1e-6,
        "the armed pawn is untouched by a foreign-entity reconcile"
    );
    assert!(
        h.client_registry.exists(stranger),
        "the bystander is not mutated into a pawn"
    );
    assert!(h.bystanders_alive());
}

// --- Dash correction: a dash predicted on the client then reconciled against an
// authoritative baseline laterally offset within the dash band classifies as a Dash
// correction (smoothed, not snapped). Drives the real prediction + reconcile seams. ---
#[test]
fn dash_correction_classifies_as_dash_and_smooths() {
    let mut h = LoopbackHarness::new(light_link());
    // Arm first.
    h.step_until_armed(&forward_command(false));
    let pawn = h.client_pawn.expect("armed");

    // Predict a dash tick locally WITHOUT delivering its ack yet, so the dash entry
    // stays unacked and replays during reconcile (the unacked window crosses a dash).
    let dash_tick = h.prediction.next_client_tick();
    let dash_input =
        super::wire_convert::sim_command_to_input(&forward_command(true), dash_tick, 0.0);
    let prev = (
        *h.client_registry.get_component::<Transform>(pawn).unwrap(),
        h.client_registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone(),
    );
    let (t, m) = h
        .prediction
        .predict_tick(dash_input, prev, &h.world, GRAVITY, DT)
        .expect("armed dash predicts");
    h.client_registry.set_component(pawn, t).unwrap();
    h.client_registry.set_component(pawn, m).unwrap();
    let predicted = h
        .client_registry
        .get_component::<Transform>(pawn)
        .unwrap()
        .position;
    assert!(
        h.prediction.unacked_window_included_dash(),
        "the unacked window crosses the predicted dash"
    );

    // Reconcile against an authoritative pose laterally offset into the dash band
    // (above the ordinary cap, within the dash cap), acking the tick BEFORE the dash
    // so the dash entry replays. The classifier reads the pinned thresholds.
    let off = 1.0_f32;
    assert!(off > ORDINARY_CORRECTION_MAX_M);
    let auth = Transform {
        position: predicted + Vec3::new(off, 0.0, 0.0),
        ..Transform::default()
    };
    let class = reconcile_local_pawn(
        &mut h.client_registry,
        &mut h.prediction,
        pawn,
        auth,
        None,
        Some(dash_tick.saturating_sub(1)),
        &h.world,
        GRAVITY,
        DT,
    )
    .expect("armed pawn reconciles");

    use super::prediction::CorrectionClass;
    assert_eq!(
        class,
        CorrectionClass::Dash,
        "dash-window correction smooths as Dash"
    );
    // Smoothed (a nonzero decaying presentation offset), NOT a snap-teleport.
    assert!(
        h.prediction.presentation_offset().length() > 1e-4,
        "a dash correction seeds a smoothed presentation offset (not a snap)"
    );
    assert!(h.bystanders_alive());
}

fn install_armed_reconcile_pawn(
    h: &mut LoopbackHarness,
    transform: Transform,
    movement: PlayerMovementComponent,
) -> postretro_entities::EntityId {
    let pawn = h.client_registry.spawn(transform);
    h.client_registry.set_component(pawn, movement).unwrap();
    h.prediction.arm(h.host_pawn_network_id, pawn);
    h.client_pawn = Some(pawn);
    pawn
}

fn sliding_component(floor_normal: Vec3) -> PlayerMovementComponent {
    let mut movement = player_component();
    let crouch = movement.crouch.as_ref().expect("slide fixture has crouch");
    movement.capsule.half_height = crouch.half_height;
    movement.capsule.eye_height = crouch.eye_height;
    movement.velocity = Vec3::new(0.0, 0.0, -10.0);
    movement.ground = GroundRef::World;
    movement.last_floor_normal = Some(floor_normal);
    movement.movement_state = MovementState::Sliding {
        elapsed_ms: 300.0,
        boost: Vec3::new(0.0, 0.0, -10.0),
        eye_current: crouch.eye_height,
    };
    movement
}

#[test]
fn sliding_authoritative_baseline_restores_older_facet_normal_before_replay() {
    let slope = 0.3;
    let mut h = LoopbackHarness::new(light_link());
    h.world = faceted_slope_world(slope);
    let downhill_normal = downhill_facet_normal(slope);
    let predicted_start = Transform {
        position: Vec3::new(0.0, faceted_floor_height(slope, -2.0) + 0.81, -2.0),
        ..Transform::default()
    };
    let pawn =
        install_armed_reconcile_pawn(&mut h, predicted_start, sliding_component(downhill_normal));
    let mut slide = forward_command(false);
    slide.movement.crouch_intent = true;
    let _ = h.prediction.next_client_tick();
    let slide_tick = h.prediction.next_client_tick();
    let input = super::wire_convert::sim_command_to_input(&slide, slide_tick, 0.0);
    let predicted_prev = (
        *h.client_registry.get_component::<Transform>(pawn).unwrap(),
        h.client_registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone(),
    );
    let (predicted_transform, predicted_movement) = h
        .prediction
        .predict_tick(input, predicted_prev, &h.world, GRAVITY, DT)
        .expect("armed slide predicts");
    h.client_registry
        .set_component(pawn, predicted_transform)
        .unwrap();
    h.client_registry
        .set_component(pawn, predicted_movement)
        .unwrap();
    let newest_normal = h
        .client_registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .last_floor_normal
        .expect("predicted slide remains on downhill facet");
    assert!(
        newest_normal.dot(Vec3::Y) < 0.999,
        "newest prediction must carry the downhill facet normal"
    );

    let authoritative_transform = Transform {
        position: Vec3::new(0.0, 0.81, -0.5),
        ..Transform::default()
    };
    let authoritative_movement = sliding_component(Vec3::Y);
    let wire = movement_state_to_wire(&authoritative_movement, 0.0);
    let (expected_transform, expected_movement, _) = super::prediction::replay(
        authoritative_transform,
        authoritative_movement,
        slide.movement.clone(),
        &h.world,
        GRAVITY,
        DT,
    );
    let class = reconcile_local_pawn(
        &mut h.client_registry,
        &mut h.prediction,
        pawn,
        authoritative_transform,
        Some(&wire),
        Some(slide_tick - 1),
        &h.world,
        GRAVITY,
        DT,
    );

    assert!(
        class.is_some(),
        "sliding baseline must pass the reconcile gate"
    );
    let actual_transform = h.client_registry.get_component::<Transform>(pawn).unwrap();
    let actual_movement = h
        .client_registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap();
    assert!(
        (actual_transform.position - expected_transform.position).length() < 1.0e-4,
        "replay must use the restored flat-facet normal on its first tick"
    );
    assert!(
        (actual_movement.velocity - expected_movement.velocity).length() < 1.0e-4,
        "restored floor normal must reproduce authoritative slide velocity"
    );
}

#[test]
fn slide_entered_inside_replay_recomputes_the_faceted_floor_normal() {
    let slope = 0.3;
    let mut h = LoopbackHarness::new(light_link());
    h.world = faceted_slope_world(slope);
    let authoritative_transform = Transform {
        position: Vec3::new(0.0, 1.21, -0.75),
        ..Transform::default()
    };
    let mut authoritative_movement = player_component();
    authoritative_movement.velocity = Vec3::new(0.0, 0.0, -20.0);
    authoritative_movement.ground = GroundRef::World;
    authoritative_movement.last_floor_normal = Some(Vec3::Y);
    authoritative_movement.movement_state = MovementState::Normal;
    let pawn = install_armed_reconcile_pawn(
        &mut h,
        authoritative_transform,
        authoritative_movement.clone(),
    );
    let mut slide = forward_command(false);
    slide.movement.crouch_intent = true;
    let _ = h.prediction.next_client_tick();
    let mut inputs = Vec::new();
    for _ in 0..3 {
        let tick = h.prediction.next_client_tick();
        let input = super::wire_convert::sim_command_to_input(&slide, tick, 0.0);
        let prev = (
            *h.client_registry.get_component::<Transform>(pawn).unwrap(),
            h.client_registry
                .get_component::<PlayerMovementComponent>(pawn)
                .unwrap()
                .clone(),
        );
        let (next_transform, next_movement) = h
            .prediction
            .predict_tick(input, prev, &h.world, GRAVITY, DT)
            .expect("armed slide-entry tick predicts");
        h.client_registry
            .set_component(pawn, next_transform)
            .unwrap();
        h.client_registry
            .set_component(pawn, next_movement)
            .unwrap();
        inputs.push(tick);
    }
    let predicted_movement = h
        .client_registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap();
    assert!(
        matches!(
            predicted_movement.movement_state,
            MovementState::Sliding { .. }
        ),
        "the first retained command must enter slide inside the replay window"
    );
    assert!(
        predicted_movement
            .last_floor_normal
            .is_some_and(|normal| normal.dot(Vec3::Y) < 0.999),
        "forward prediction must reach the downhill facet"
    );

    let mut expected_transform = authoritative_transform;
    let mut expected_movement = authoritative_movement.clone();
    for _ in &inputs {
        let replayed = super::prediction::replay(
            expected_transform,
            expected_movement,
            slide.movement.clone(),
            &h.world,
            GRAVITY,
            DT,
        );
        expected_transform = replayed.0;
        expected_movement = replayed.1;
    }
    let wire = movement_state_to_wire(&authoritative_movement, 0.0);
    assert!(matches!(
        wire.movement_state,
        postretro_net::wire::WireMovementState::Normal
    ));
    let class = reconcile_local_pawn(
        &mut h.client_registry,
        &mut h.prediction,
        pawn,
        authoritative_transform,
        Some(&wire),
        Some(inputs[0] - 1),
        &h.world,
        GRAVITY,
        DT,
    );

    assert!(
        class.is_some(),
        "normal baseline must pass the reconcile gate"
    );
    let actual_transform = h.client_registry.get_component::<Transform>(pawn).unwrap();
    let actual_movement = h
        .client_registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap();
    assert!(
        (actual_transform.position - expected_transform.position).length() < 1.0e-4,
        "slide entry replay must reproduce the faceted-floor trajectory"
    );
    assert!(matches!(
        actual_movement.movement_state,
        MovementState::Sliding { .. }
    ));
    assert_eq!(
        actual_movement.last_floor_normal, expected_movement.last_floor_normal,
        "entry replay must recompute the facet normal through the movement substrate"
    );
}

// --- Teleport correction: a correction at/above the teleport floor snaps hard —
// history + presentation offset cleared, registry snapped, prev == current stamped
// (no render slide). Uses the real reconcile seam. ---
#[test]
fn teleport_correction_snaps_without_smoothing() {
    let mut h = LoopbackHarness::new(light_link());
    h.step_until_armed(&forward_command(false));
    let pawn = h.client_pawn.expect("armed");
    // Seed a stale presentation offset to prove the teleport clears it.
    h.prediction
        .seed_presentation_offset(Vec3::new(0.1, 0.0, 0.0));
    let predicted = h
        .client_registry
        .get_component::<Transform>(pawn)
        .unwrap()
        .position;

    let far = TELEPORT_CORRECTION_MIN_M + 1.0;
    let auth = Transform {
        position: predicted + Vec3::new(far, 0.0, 0.0),
        ..Transform::default()
    };
    // Ack the latest predicted tick so nothing replays; the correction is purely the
    // teleport-distance baseline-vs-predicted delta.
    let ack = h.prediction.history().back().map(|e| e.client_tick);
    let class = reconcile_local_pawn(
        &mut h.client_registry,
        &mut h.prediction,
        pawn,
        auth,
        None,
        ack,
        &h.world,
        GRAVITY,
        DT,
    )
    .expect("armed pawn reconciles");

    use super::prediction::CorrectionClass;
    assert_eq!(class, CorrectionClass::Teleport);
    assert!(h.prediction.history().is_empty(), "teleport clears history");
    assert_eq!(
        h.prediction.presentation_offset(),
        Vec3::ZERO,
        "teleport clears the presentation offset (no smoothed glide)"
    );
    // Registry snapped to the authoritative pose; prev == current (no render slide).
    let at_zero = h.client_registry.interpolated_transform(pawn, 0.0).unwrap();
    let at_one = h.client_registry.interpolated_transform(pawn, 1.0).unwrap();
    assert!(
        (at_zero.position - at_one.position).length() < 1e-4,
        "teleport stamps prev == current (no slide across the snap)"
    );
    assert!(h.bystanders_alive());
}

// --- Malformed input at the drain seam: a non-finite ClientMessage::Input is
// rejected by sanitize, mutating no queue/cursor and never panicking the host. ---
#[test]
fn malformed_input_at_drain_seam_is_rejected() {
    let mut h = LoopbackHarness::new(light_link());

    let mut bad = input_at(0, 1.0);
    bad.movement.wish_dir[1] = f32::NAN;
    host_handle_client_message(
        &mut h.server,
        &mut h.server_replication,
        &mut h.server_state,
        &mut h.command_queues,
        CLIENT_ID,
        0,
        0,
        ClientMessage::Input(bad),
    );
    assert!(
        h.command_queues.resolved_cursor(CLIENT_ID).is_none(),
        "a malformed command created no queue/cursor state"
    );
    // Nothing to resolve: the rejected command never reached the queue.
    assert!(
        run_resolve(&mut h, CLIENT_ID).is_none(),
        "a rejected malformed command never resolves a tick"
    );
    assert!(h.bystanders_alive());
}

// --- Presented first-person eye smoothness (M15 Phase 3 playtest bug "Symptom 1").
//
// Bug: on a CONNECTED CLIENT the first-person camera vibrated with amplitude ∝ speed
// (zero at standstill). Root cause: the client `continue`s past `simulate_tick`, so
// the registry-wide stage-0 `snapshot_transforms` (the per-tick `previous = current`
// copy render interpolation relies on) never ran for the local pawn. The render-stage
// `interpolated_transform(localpawn, alpha)` then lerped the live-current pose against
// an ever-staler frozen-previous, and the per-snapshot reconcile snap perturbed the
// registry — together a velocity-proportional per-frame oscillation of the PRESENTED
// eye, even on plain loopback at ~zero latency.
//
// Why the existing harness misses it: every other test asserts on `position_error()` /
// `client_position()`, which read the registry's *current* Transform after the drain
// settles. They never sample the render-rate PRESENTED eye — the interpolated transform
// blended at a sub-tick alpha plus the decaying presentation offset — so the jitter is
// invisible to them (the pre-fix headline gate still reports 0.00000 m). This test
// samples that eye across render sub-steps under the real predict + reconcile cadence.
//
// Fix: `client_predict_tick` (mirrored in the harness `client_predict_and_send`) now
// stamps `previous = current` for the local pawn each predicted tick via
// `EntityRegistry::snapshot_transform`, and `reconcile_local_pawn` snaps the local
// pawn with `set_presentation_transform` (prev == current) so the transform-history is
// coherent the instant a reconcile lands — without rerunning `simulate_tick`.
#[test]
fn presented_first_person_eye_is_smooth_under_reconcile_cadence() {
    use crate::frame_timing::{FrameTiming, InterpolableState};

    // Plain loopback (the reported repro is plain loopback at ~zero latency). The host
    // tracks the client closely, so reconcile corrections stay small and the PRESENTED
    // first-person eye should advance smoothly. The pre-fix eye oscillated ∝ speed here.
    let mut h = LoopbackHarness::new(loopback_link());
    h.step_until_armed(&forward_command(false));
    assert!(h.prediction.is_armed(), "prediction armed before sampling");

    // Reconstruct the EXACT production first-person eye assembly (main.rs ~2175-2445):
    //   - each fixed tick, `follow_camera_to_local_pawn` reads the registry CURRENT eye
    //     PLUS the presentation offset (the PRESENTED pose) into `camera.position`, the
    //     offset decays once per tick, and `frame_timing.push_state(camera.pos)` pushes
    //     the presented pose;
    //   - each render frame, `presented_eye = frame_timing.interpolated_state()` directly
    //     (the offset is already baked into the pose — NOT re-added at render).
    // We drive a real `FrameTiming` with that PRESENTED eye stream. The offset is folded
    // via `present_local_pose` (the pure presentation accessor) using the same value
    // `client_local_presentation_offset` feeds the render seam. Several render frames per
    // tick at a ladder of alphas reconstruct the continuous eye a variable-rate renderer
    // draws. The reconcile cadence (inside `h.step`) perturbs the registry across the run;
    // a correct presented-pose assembly stays continuous across each snap.
    let seed_eye = h.local_pawn_eye().expect("armed pawn has an eye");
    let mut frame_timing = FrameTiming::new(InterpolableState::new(seed_eye));
    const ALPHAS: [f32; 4] = [0.0, 0.25, 0.5, 0.75];

    // The presented eye Z over the whole render timeline. Constant forward walk → the
    // eye must advance smoothly along -Z. The velocity-proportional bug shows up as
    // alternating forward/back substeps (high jerk) and a backward arc each reconcile
    // (the offset double-counting at alpha 0 / `frame_timing` interpolating the snap).
    let mut eye_z: Vec<f32> = Vec::new();

    for _ in 0..200 {
        // One fixed tick: predict + reconcile (inside step). Then camera-follow the
        // PRESENTED eye (registry CURRENT + offset) and push it into frame_timing, and
        // decay the offset once per tick — exactly the production tick-rate seam.
        h.step(&forward_command(false));
        let registry_eye = h.local_pawn_eye().expect("armed pawn has an eye");
        let camera_eye = h
            .prediction
            .present_local_pose(Transform {
                position: registry_eye,
                ..Transform::default()
            })
            .position;
        frame_timing.push_state(InterpolableState::new(camera_eye));
        h.prediction.decay_presentation_offset();

        // Several render frames this tick: presented eye = interpolated PRESENTED pose
        // read directly (offset already baked in — no re-add). `frame_timing`
        // interpolates between the last two pushed presented eyes at the render alpha
        // (the same `InterpolableState::lerp` the render seam reads).
        for &alpha in &ALPHAS {
            let presented = frame_timing
                .previous_state
                .lerp(&frame_timing.current_state, alpha)
                .position;
            assert!(presented.is_finite(), "presented eye is finite");
            eye_z.push(presented.z);
        }
    }

    assert!(eye_z.len() > 8, "sampled a meaningful timeline");

    // The pawn genuinely moved (not a degenerate at-rest run): real -Z travel.
    let total_travel = eye_z.first().unwrap() - eye_z.last().unwrap();
    assert!(
        total_travel > 5.0,
        "the forward-walk scenario advanced the presented eye along -Z (travel {total_travel:.3} m)"
    );

    let steps: Vec<f32> = eye_z.windows(2).map(|w| w[1] - w[0]).collect();
    // Expected smooth per-render-frame forward step: forward run speed (≈11 m/s) over
    // one tick (1/60 s) split across the per-tick render frames ≈ 0.046 m. Derive the
    // smooth scale from the observed mean so the gate is not a magic number.
    let mean_step = steps.iter().sum::<f32>() / steps.len() as f32; // negative (forward)
    let smooth_scale = mean_step.abs();

    // GATE 1: no large backward vibration. A small backward glide is legitimate while a
    // correction offset decays toward the (slightly-behind) authority; the BUG instead
    // snapped the eye backward by ~a full tick of locomotion every tick boundary. Bound
    // any backward substep to a fraction of the smooth forward step — an amplitude-∝-
    // speed shake blows straight past this; a decay glide stays well under it.
    let max_backward_step = steps.iter().copied().fold(f32::MIN, f32::max);
    assert!(
        max_backward_step <= smooth_scale * 0.5,
        "the presented eye must not jump backward ∝ speed (velocity-proportional shake); \
         worst backward substep {max_backward_step:.5} m exceeds 0.5x the smooth forward \
         step {smooth_scale:.5} m"
    );

    // GATE 2: bounded jerk. Under constant velocity the per-frame step barely varies;
    // the pre-fix oscillation produced huge swings (near-full-tick backward snap then an
    // over-long forward catch-up). Bound the worst per-frame step *change* to a small
    // multiple of the smooth step.
    let max_jerk = steps
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        max_jerk <= smooth_scale * 1.5,
        "presented-eye motion must be smooth (bounded jerk); worst per-frame step change \
         {max_jerk:.5} m exceeds 1.5x the smooth forward step {smooth_scale:.5} m"
    );

    assert!(
        h.bystanders_alive(),
        "movement-only path: the death sweep never ran"
    );
}

// --- Local-pawn render interpolation coherence (the structural Rank-3 term of the
// same playtest bug). The pawn MESH and the portal-visibility render eye read the
// registry's `interpolated_transform(localpawn, alpha)` (main.rs / mesh_render.rs), NOT
// the camera `frame_timing` path. The bug: the connected client skips `simulate_tick`,
// so the per-tick `previous = current` stamp never ran for the local pawn and
// `interpolated_transform` lerped live-current against an ever-staler frozen-previous —
// at alpha 0 it snapped back ~the whole accumulated distance every frame.
//
// Fix: `client_predict_tick` stamps `snapshot_transform` each predicted tick and
// `reconcile_local_pawn` snaps the pawn with `set_presentation_transform` (prev ==
// current), so previous is always exactly one tick (or the reconciled pose) behind
// current. The existing harness never read this surface (`position_error` reads only
// the CURRENT transform); this asserts the interpolated render pose advances smoothly.
#[test]
fn local_pawn_interpolated_render_eye_is_coherent_each_tick() {
    let mut h = LoopbackHarness::new(loopback_link());
    h.step_until_armed(&forward_command(false));
    assert!(h.prediction.is_armed(), "prediction armed before sampling");

    // For each tick, sweep the render alpha across the registry's interpolated local
    // pawn pose. With coherent transform-history the per-tick span (alpha 0 → 1) is one
    // tick of locomotion; the bug inflated it to the whole distance-since-spawn and made
    // alpha 0 snap backward. We assert the intra-tick span stays near one tick of walk.
    let mut max_intratick_span = 0.0_f32;
    let mut moved = false;
    for _ in 0..120 {
        h.step(&forward_command(false));
        let at0 = h
            .local_pawn_interpolated_eye(0.0)
            .expect("armed pawn interpolated eye");
        let at1 = h
            .local_pawn_interpolated_eye(1.0)
            .expect("armed pawn interpolated eye");
        // alpha 1 is the current pose, alpha 0 is the previous-tick pose. The forward
        // span (current ahead of previous along -Z) is one tick of locomotion when the
        // history is coherent; the bug made alpha 0 reach back to a frozen-stale pose.
        let span = (at1 - at0).length();
        max_intratick_span = max_intratick_span.max(span);
        if at1.z < -1.0 {
            moved = true;
        }
    }

    assert!(
        moved,
        "the forward-walk scenario advanced the interpolated eye"
    );
    // One tick of run locomotion is ≈ run_speed (11 m/s) / 60 Hz ≈ 0.18 m. A coherent
    // previous→current span is at most a few ticks (a reconcile that replays the unacked
    // tail snaps prev == current, so even then the span is bounded). The bug produced a
    // span of the WHOLE distance-since-spawn (many metres), growing without bound. Bound
    // the span well below that runaway while leaving headroom for the reconcile replay.
    assert!(
        max_intratick_span < 1.0,
        "the interpolated render eye's previous→current span must stay near one tick of \
         locomotion (coherent transform-history); worst span was {max_intratick_span:.3} m"
    );
    assert!(h.bystanders_alive());
}

#[test]
fn moving_platform_reconciles_under_mandated_profile_without_accumulating_drift() {
    let mut h = LoopbackHarness::with_moving_platform(mandated_link());
    const MOVER_TOLERANCE_M: f32 = 0.16;
    const RIDER_TOLERANCE_M: f32 = 0.35;
    const RELEASE_VELOCITY_TOLERANCE_MPS: f32 = 0.08;

    let mut mover_errors = Vec::new();
    let mut mover_corrections = Vec::new();
    let mut riding_rider_errors = Vec::new();
    let mut rider_corrections = Vec::new();

    for _ in 0..150 {
        h.step(&idle_command());
        if h.client_pawn.is_none() {
            continue;
        }

        let mover_error = h.mover_position_error();
        assert!(
            mover_error <= MOVER_TOLERANCE_M,
            "client-predicted mover must track host without interpolation lag; error={mover_error:.4}"
        );
        mover_errors.push(mover_error);
        mover_corrections.extend(h.latest_mover_corrections.iter().map(|c| c.magnitude));
        riding_rider_errors.push(h.position_error());
        rider_corrections.extend(h.latest_local_corrections.iter().copied());
    }

    assert_eq!(
        h.client_loaded_mover_count(),
        1,
        "mover baseline binds the pre-loaded mover instead of spawning a duplicate"
    );
    assert!(
        h.client_mover_history_samples() > 0,
        "client apply must feed authoritative mover samples into mover history"
    );
    assert_eq!(h.host_ground(), GroundRef::Mover(MOVING_PLATFORM_ID));
    assert_eq!(h.client_ground(), GroundRef::Mover(MOVING_PLATFORM_ID));
    assert!(
        riding_rider_errors
            .iter()
            .all(|error| *error <= RIDER_TOLERANCE_M),
        "rider should reconcile in place while standing on the platform; errors={riding_rider_errors:?}"
    );

    let mut release_velocity = None;
    let mut previous_ground = h.host_ground();
    for _ in 0..90 {
        h.step(&forward_command(false));
        if h.client_pawn.is_some() {
            let mover_error = h.mover_position_error();
            assert!(
                mover_error <= MOVER_TOLERANCE_M,
                "client-predicted mover must stay bounded through rider release; error={mover_error:.4}"
            );
            mover_errors.push(mover_error);
            mover_corrections.extend(h.latest_mover_corrections.iter().map(|c| c.magnitude));
            rider_corrections.extend(h.latest_local_corrections.iter().copied());
        }

        let ground = h.host_ground();
        if matches!(previous_ground, GroundRef::Mover(MOVING_PLATFORM_ID))
            && ground == GroundRef::Airborne
        {
            let host_velocity = h.host_velocity();
            let mover_velocity = h.host_mover_velocity().expect("host mover velocity");
            release_velocity = Some((host_velocity, mover_velocity));
            break;
        }
        previous_ground = ground;
    }

    let (host_release_velocity, mover_velocity) =
        release_velocity.expect("rider should leave the platform mid-ride");
    assert!(
        (host_release_velocity.x - mover_velocity.x).abs() <= RELEASE_VELOCITY_TOLERANCE_MPS,
        "release should add the mover's single-player linear velocity once; host vx={:.4}, mover vx={:.4}",
        host_release_velocity.x,
        mover_velocity.x
    );
    assert!(
        (host_release_velocity.x - MOVING_PLATFORM_SPEED_MPS).abs()
            <= RELEASE_VELOCITY_TOLERANCE_MPS,
        "release velocity should match the seeded platform speed; vx={:.4}",
        host_release_velocity.x
    );

    for _ in 0..80 {
        h.step(&forward_command(false));
        if h.client_pawn.is_none() {
            continue;
        }
        mover_errors.push(h.mover_position_error());
        mover_corrections.extend(h.latest_mover_corrections.iter().map(|c| c.magnitude));
        rider_corrections.extend(h.latest_local_corrections.iter().copied());
    }

    assert!(
        mover_corrections
            .iter()
            .all(|magnitude| *magnitude <= MOVER_TOLERANCE_M),
        "mover reconciler corrections must stay bounded; corrections={mover_corrections:?}"
    );
    assert!(
        rider_corrections
            .iter()
            .all(|magnitude| *magnitude <= RIDER_TOLERANCE_M),
        "rider reconcile corrections must stay bounded; corrections={rider_corrections:?}"
    );
    assert_non_accumulating("mover error", &mover_errors, MOVER_TOLERANCE_M * 0.5);
    assert_non_accumulating(
        "mover correction",
        &mover_corrections,
        MOVER_TOLERANCE_M * 0.5,
    );
    assert_non_accumulating(
        "riding rider error",
        &riding_rider_errors,
        RIDER_TOLERANCE_M * 0.5,
    );

    let client_vx = h.client_velocity().x;
    assert!(
        (client_vx - host_release_velocity.x).abs() <= RIDER_TOLERANCE_M,
        "client rider should reconcile to host release velocity; client vx={client_vx:.4}, host vx={:.4}",
        host_release_velocity.x
    );
    assert!(h.bystanders_alive());
}

#[test]
fn rotating_platform_reconciles_phase_rider_and_tangential_release_under_mandated_profile() {
    let mut h = LoopbackHarness::with_rotating_platform(mandated_link());
    const ANGULAR_TOLERANCE_RAD: f32 = 0.12;
    const RIDER_TOLERANCE_M: f32 = 0.35;
    const RELEASE_VELOCITY_TOLERANCE_MPS: f32 = 0.08;

    let mut orientation_errors = Vec::new();
    let mut spin_rate_errors = Vec::new();
    let mut rider_errors = Vec::new();
    let mut set_spin_rate_fired = false;
    let mut max_host_revolution_chord = 0.0_f32;
    let mut max_client_revolution_chord = 0.0_f32;

    for tick in 0..300 {
        if tick == 140 {
            let mover_id = h.host_mover.expect("rotating fixture has a host mover");
            let mut mover = h
                .host_registry
                .get_component::<KinematicMoverComponent>(mover_id)
                .expect("host rotating mover component")
                .clone();
            apply_mover_command(&mut mover, &MoverCommand::SetSpinRate(-90.0));
            h.host_registry
                .set_component(mover_id, mover)
                .expect("update host spin target");
            set_spin_rate_fired = true;
        }

        h.step(&idle_command());
        let Some(client_pawn) = h.client_pawn else {
            continue;
        };

        let host_rotation = h
            .host_registry
            .get_component::<Transform>(h.host_mover.expect("host mover"))
            .expect("host mover transform")
            .rotation;
        let client_rotation = h
            .client_registry
            .get_component::<Transform>(h.client_mover.expect("client mover"))
            .expect("client mover transform")
            .rotation;
        let orientation_error = wrapped_yaw_distance_rad(
            yaw_from_rotation(host_rotation),
            yaw_from_rotation(client_rotation),
        );
        assert!(
            orientation_error <= ANGULAR_TOLERANCE_RAD,
            "client-predicted rotating mover must stay within the angular tolerance; error={orientation_error:.4} rad"
        );
        orientation_errors.push(orientation_error);

        if set_spin_rate_fired {
            let host_rate = h
                .host_registry
                .get_component::<KinematicMoverComponent>(h.host_mover.expect("host mover"))
                .expect("host mover phase")
                .spin_rate_rad_s;
            let client_rate = h
                .client_registry
                .get_component::<KinematicMoverComponent>(h.client_mover.expect("client mover"))
                .expect("client mover phase")
                .spin_rate_rad_s;
            spin_rate_errors.push((host_rate - client_rate).abs());
        }

        let rider_error =
            (h.host_position() - h.client_position().expect("armed client pawn")).length();
        max_host_revolution_chord = max_host_revolution_chord.max(
            (horizontal(h.host_position()) - horizontal(ROTATING_PLATFORM_RIDER_START)).length(),
        );
        max_client_revolution_chord = max_client_revolution_chord.max(
            (horizontal(h.client_position().expect("armed client pawn"))
                - horizontal(ROTATING_PLATFORM_RIDER_START))
            .length(),
        );
        assert!(
            rider_error <= RIDER_TOLERANCE_M,
            "rider must reconcile its revolved position in place; error={rider_error:.4} m"
        );
        rider_errors.push(rider_error);
        assert_eq!(h.host_ground(), GroundRef::Mover(MOVING_PLATFORM_ID));
        assert_eq!(h.client_ground(), GroundRef::Mover(MOVING_PLATFORM_ID));
        assert!(
            h.client_registry.exists(client_pawn),
            "rotating mover reconciliation must not replace the client pawn"
        );
    }

    assert!(
        set_spin_rate_fired,
        "the scenario must issue a mid-ride set_spin_rate"
    );
    let host_phase = h
        .host_registry
        .get_component::<KinematicMoverComponent>(h.host_mover.expect("host mover"))
        .expect("host mover phase");
    let client_phase = h
        .client_registry
        .get_component::<KinematicMoverComponent>(h.client_mover.expect("client mover"))
        .expect("client mover phase");
    assert!(
        (host_phase.spin_target_rate_rad_s - client_phase.spin_target_rate_rad_s).abs() <= 1.0e-4,
        "the host-issued spin target must reconcile to the client"
    );
    assert!(
        (host_phase.spin_rate_rad_s - client_phase.spin_rate_rad_s).abs() <= ANGULAR_TOLERANCE_RAD,
        "the ramped current spin rate must reconcile after the mid-scenario command"
    );
    assert!(
        h.client_mover_history_samples() > 0,
        "rotating mover replay must use the existing full-transform mover history"
    );
    assert!(
        max_host_revolution_chord > 1.0 && max_client_revolution_chord > 1.0,
        "both riders must actually revolve instead of merely agreeing while stationary: host_chord={max_host_revolution_chord:.3}, client_chord={max_client_revolution_chord:.3}"
    );
    assert_non_accumulating(
        "rotating mover orientation correction",
        &orientation_errors,
        ANGULAR_TOLERANCE_RAD * 0.5,
    );
    assert_non_accumulating(
        "rotating mover spin-rate correction",
        &spin_rate_errors,
        ANGULAR_TOLERANCE_RAD * 0.5,
    );
    assert_non_accumulating(
        "rotating rider position correction",
        &rider_errors,
        RIDER_TOLERANCE_M * 0.5,
    );

    // A jump leaves the platform mid-ride without adding horizontal locomotion, so
    // the horizontal release velocity isolates the tangential mover term.
    let mut jump_command = idle_command();
    jump_command.movement.jump_pressed = true;
    let mut previous_host_ground = h.host_ground();
    let mut previous_client_ground = h.client_ground();
    let mut host_release = None;
    let mut client_release = None;
    for _ in 0..90 {
        h.step(&jump_command);

        let host_ground = h.host_ground();
        if matches!(previous_host_ground, GroundRef::Mover(MOVING_PLATFORM_ID))
            && !matches!(host_ground, GroundRef::Mover(MOVING_PLATFORM_ID))
        {
            host_release = Some((h.host_velocity(), tangential_release_velocity(&h, true)));
        }
        previous_host_ground = host_ground;

        let client_ground = h.client_ground();
        if matches!(previous_client_ground, GroundRef::Mover(MOVING_PLATFORM_ID))
            && !matches!(client_ground, GroundRef::Mover(MOVING_PLATFORM_ID))
        {
            client_release = Some((h.client_velocity(), tangential_release_velocity(&h, false)));
        }
        previous_client_ground = client_ground;

        if host_release.is_some() && client_release.is_some() {
            break;
        }
    }

    let (host_release_velocity, host_tangential_velocity) =
        host_release.expect("host rider leaves the rotating platform");
    let (client_release_velocity, client_tangential_velocity) =
        client_release.expect("client rider predicts the rotating-platform release");
    assert!(
        (horizontal(host_release_velocity) - host_tangential_velocity).length()
            <= RELEASE_VELOCITY_TOLERANCE_MPS,
        "host release must preserve tangential velocity; actual={host_release_velocity:?}, expected={host_tangential_velocity:?}"
    );
    assert!(
        (horizontal(client_release_velocity) - client_tangential_velocity).length()
            <= RELEASE_VELOCITY_TOLERANCE_MPS,
        "client release must preserve tangential velocity; actual={client_release_velocity:?}, expected={client_tangential_velocity:?}"
    );
    assert!(
        (client_tangential_velocity - host_tangential_velocity).length() <= RIDER_TOLERANCE_M,
        "client and host must agree on the tangential release velocity"
    );
    assert!(h.bystanders_alive());
}

// A remote Use edge must cross the real client-input wire path before only the host
// evaluates the trigger. The resulting target phase then returns through ordinary
// mover replication for the client's locally predicted mover to reconcile against.
#[test]
fn remote_use_trigger_fires_on_host_and_reconciles_mover_target() {
    let mut h = LoopbackHarness::with_moving_platform(light_link());
    const MOVER_TOLERANCE_M: f32 = 0.16;

    h.step_until_armed(&idle_command());
    assert!(
        h.client_pawn.is_some(),
        "client must arm before sending Use"
    );

    // Exactly one client input carries the Use edge; subsequent commands are neutral.
    h.step(&use_command());

    let use_trigger = h
        .host_use_trigger
        .expect("moving-platform fixture has Use trigger");
    let mut observed_client_target = false;
    let mut mover_errors = Vec::new();
    for _ in 0..120 {
        h.step(&idle_command());

        let host_fired = h
            .host_registry
            .get_component::<TriggerVolumeComponent>(use_trigger)
            .expect("host Use trigger remains live")
            .latched;
        let host_target = h
            .host_registry
            .get_component::<KinematicMoverComponent>(h.host_mover.expect("host mover"))
            .expect("host mover component")
            .target_segment;
        let client_target = h
            .client_registry
            .get_component::<KinematicMoverComponent>(h.client_mover.expect("client mover"))
            .expect("client mover component")
            .target_segment;

        if host_fired && host_target == Some(1) && client_target == Some(1) {
            observed_client_target = true;
            mover_errors.push(h.mover_position_error());
            break;
        }
    }

    assert!(
        h.host_registry
            .get_component::<TriggerVolumeComponent>(use_trigger)
            .expect("host Use trigger remains live")
            .latched,
        "the remote Use edge must fire the host-only Use trigger"
    );
    assert_eq!(
        h.host_registry
            .get_component::<KinematicMoverComponent>(h.host_mover.expect("host mover"))
            .expect("host mover component")
            .target_segment,
        Some(1),
        "host trigger command targets the finish waypoint"
    );
    assert!(
        observed_client_target,
        "the host target phase must replicate to the client mover"
    );
    assert_eq!(
        h.client_registry
            .iter_with_kind(postretro_entities::ComponentKind::TriggerVolume)
            .count(),
        0,
        "the client holds no trigger state and never evaluates trigger commands"
    );
    assert!(
        mover_errors.iter().all(|error| *error <= MOVER_TOLERANCE_M),
        "client mover must reconcile to the host-issued target; errors={mover_errors:?}"
    );
    assert!(h.bystanders_alive());
}

// ---------------------------------------------------------------------------
// Section B — Headline deterministic latency gate
// ---------------------------------------------------------------------------

// The headline acceptance test (Task 6 §B). The full loop runs under the mandated
// profile in both directions for >5 s of simulated time after time-sync convergence
// is assumed (this harness's master clock IS the converged shared clock — time sync
// is validated separately in `net::harness`). HARD GATES:
//  - final position error after drain <= 0.05 m;
//  - sub-teleport corrections smooth (the run never takes a snap-teleport path);
//  - no stale/duplicate/malformed input mutates unrelated entities (bystanders live).
// Deterministic: seeded conditioner (0x1502) + caller-advanced virtual clock; no
// wall-clock read anywhere.
#[test]
fn latency_harness_converges_within_tolerance_under_mandated_profile() {
    let measured = run_latency_gate(mandated_link());

    println!(
        "[Task6 gate] error={:.5}m drained={} teleport={} max_smoothed={:.4}m \
         smoothed_count={} host_travel={:.2}m drain_iters={} drop_to_server={} drop_to_client={}",
        measured.final_error,
        measured.drained,
        measured.took_teleport,
        measured.max_smoothed_correction,
        measured.smoothed_correction_count,
        measured.host_travel,
        measured.drain_iters,
        measured.dropped_to_server,
        measured.dropped_to_client,
    );

    assert!(
        measured.drained,
        "the harness must reach the explicit drain condition before asserting the gate"
    );
    // HARD GATE 1: final position error after drain <= 0.05 m.
    assert!(
        measured.final_error <= 0.05,
        "HARD GATE: final client/server position error after drain must be <= 0.05 m; \
         measured {:.5} m (seed 0x1502, {} active ticks)",
        measured.final_error,
        measured.active_ticks
    );
    // HARD GATE 2: every correction below the teleport threshold takes the smoothed
    // (seed-a-decaying-offset) path, never a snap-teleport. Under the mandated profile
    // the client predicts ahead of the (playout-lagged) authority, so each snapshot
    // reconciles a correction the size of that lead; the gate's invariant is that the
    // engine *smooths* every such correction (decaying presentation offset) rather than
    // snapping — and that the magnitude stays in the smoothed band, below the teleport
    // floor. Smoothing was actually exercised (corrections occurred and were seeded as
    // decaying offsets). Steady locomotion never escalates to a teleport snap.
    assert!(
        !measured.took_teleport,
        "HARD GATE: corrections below the teleport threshold must smooth, never snap-teleport \
         (max smoothed correction {:.4} m over {} corrections)",
        measured.max_smoothed_correction, measured.smoothed_correction_count
    );
    assert!(
        measured.smoothed_correction_count > 0,
        "the conditioned link should produce real smoothed corrections to absorb \
         (none observed — the scenario did not exercise reconciliation)"
    );
    assert!(
        measured.max_smoothed_correction < TELEPORT_CORRECTION_MIN_M,
        "every smoothed correction stays below the teleport floor; worst was {:.4} m",
        measured.max_smoothed_correction
    );
    // HARD GATE 3: no stale/duplicate/malformed input mutated an unrelated entity.
    assert!(
        measured.bystanders_alive,
        "HARD GATE: no stale/duplicate/malformed input mutated an unrelated entity \
         (the death-sweep bystanders survived — the movement-only path never ran simulate_tick)"
    );
    // The scenario was non-trivial: the pawn actually traversed the map, and the
    // conditioned link actually dropped packets (loss was exercised).
    assert!(
        measured.host_travel > 5.0,
        "the 5 s scenario produced real motion (host traveled {:.2} m)",
        measured.host_travel
    );
    assert!(
        measured.dropped_to_server > 0 && measured.dropped_to_client > 0,
        "the 5% loss model dropped packets in both directions (to_server={}, to_client={})",
        measured.dropped_to_server,
        measured.dropped_to_client
    );
}

// The same run is bit-for-bit reproducible under the fixed seed: two independent
// runs produce identical final error, travel, and tick counts.
#[test]
fn latency_harness_is_deterministic_under_seed_0x1502() {
    let a = run_latency_gate(mandated_link());
    let b = run_latency_gate(mandated_link());
    assert_eq!(a.active_ticks, b.active_ticks, "tick count is reproducible");
    assert_eq!(
        a.final_error.to_bits(),
        b.final_error.to_bits(),
        "final position error is bit-identical across runs (seed 0x1502)"
    );
    assert_eq!(
        a.host_travel.to_bits(),
        b.host_travel.to_bits(),
        "host travel is bit-identical across runs"
    );
    assert_eq!(
        a.dropped_to_server, b.dropped_to_server,
        "drop pattern reproducible"
    );
    assert_eq!(
        a.dropped_to_client, b.dropped_to_client,
        "drop pattern reproducible"
    );
}

struct GateResult {
    final_error: f32,
    drained: bool,
    took_teleport: bool,
    max_smoothed_correction: f32,
    smoothed_correction_count: u32,
    bystanders_alive: bool,
    host_travel: f32,
    active_ticks: u32,
    drain_iters: u32,
    dropped_to_server: u64,
    dropped_to_client: u64,
}

/// Run the full loop under `link` for a varied >5 s movement scenario, drain to the
/// explicit condition, and measure the gate quantities. The scenario weaves
/// forward / strafing / turning / dashing so reconciliation has real corrections to
/// absorb under the conditioned link.
fn run_latency_gate(link: LinkConfig) -> GateResult {
    let mut h = LoopbackHarness::new(link);

    // 5 s at 60 Hz = 300 active ticks. Run a varied command stream so the
    // prediction/reconcile path is genuinely exercised (not a straight line a
    // perfect predictor never mis-predicts).
    const ACTIVE_TICKS: u32 = 360; // 6 s of active input, comfortably past 5 s
    let start = h.host_position();
    let mut took_teleport = false;
    let mut max_smoothed_correction = 0.0_f32;
    let mut smoothed_correction_count = 0u32;

    for tick in 0..ACTIVE_TICKS {
        let command = scripted_command(tick);
        let (teleport, correction) = h.step_and_watch_correction(&command);
        if teleport {
            took_teleport = true;
        }
        if correction > 1e-4 {
            smoothed_correction_count += 1;
            max_smoothed_correction = max_smoothed_correction.max(correction);
        }
    }

    // Drain: stop sending new input, keep the loop running until the explicit drain
    // condition holds (no packets in flight, host cursor caught up to the last sent
    // tick, client acked the frozen target tick). Cap iterations so a regression
    // cannot hang.
    let mut drain_iters = 0;
    while !h.is_drained() && drain_iters < 4_000 {
        h.drain_step();
        drain_iters += 1;
    }

    let final_error = h.position_error();
    let host_travel = (h.host_position() - start).length();

    GateResult {
        final_error,
        drained: h.is_drained(),
        took_teleport,
        max_smoothed_correction,
        smoothed_correction_count,
        bystanders_alive: h.bystanders_alive(),
        host_travel,
        active_ticks: ACTIVE_TICKS,
        drain_iters,
        dropped_to_server: h.to_server.dropped(),
        dropped_to_client: h.to_client.dropped(),
    }
}

/// A scripted per-tick command: continuous locomotion with phases of forward,
/// strafing, and turning so the reconcile path sees ordinary and turning corrections
/// under the conditioned link. No dash: a dash burst (18 m/s) during a snapshot-loss
/// window legitimately produces a teleport-sized correction (the designed snap escape
/// hatch, validated separately in `dash_correction_classifies_as_dash_and_smooths`);
/// the headline "no visible rubber-banding under normal *locomotion* latency" gate is
/// about steady movement, where every correction must stay in the smoothed band.
fn scripted_command(tick: u32) -> SimCommand {
    let phase = tick % 120;
    let wish_dir = if phase < 60 {
        Vec2::new(0.0, 1.0) // forward
    } else if phase < 90 {
        Vec2::new(0.6, 0.8) // strafe-forward
    } else {
        Vec2::new(-0.5, 0.85) // strafe the other way
    };
    let facing_yaw = if phase < 80 { 0.0 } else { 0.4 };
    SimCommand {
        movement: MovementInput {
            wish_dir,
            jump_pressed: false,
            dash_pressed: false,
            running: phase < 100,
            crouch_intent: false,
            facing_yaw,
            use_pressed: false,
            drop_pressed: false,
        },
        fire_button: crate::weapon::FireButtonState {
            pressed: false,
            active: false,
        },
        reload: false,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    }
}

// ---------------------------------------------------------------------------
// Local helpers
// ---------------------------------------------------------------------------

fn assert_non_accumulating(label: &str, samples: &[f32], tolerance: f32) {
    assert!(
        samples.len() >= 8,
        "{label}: expected enough samples to check accumulation, got {}",
        samples.len()
    );
    let midpoint = samples.len() / 2;
    let first = samples[..midpoint].iter().sum::<f32>() / midpoint as f32;
    let second = samples[midpoint..].iter().sum::<f32>() / (samples.len() - midpoint) as f32;
    assert!(
        second <= first + tolerance,
        "{label}: second-half mean accumulated beyond tolerance; first={first:.5}, second={second:.5}, tolerance={tolerance:.5}"
    );
}

fn yaw_from_rotation(rotation: glam::Quat) -> f32 {
    (2.0 * rotation.y.atan2(rotation.w)).rem_euclid(std::f32::consts::TAU)
}

fn wrapped_yaw_distance_rad(a: f32, b: f32) -> f32 {
    ((a - b + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI).abs()
}

fn tangential_release_velocity(h: &LoopbackHarness, host: bool) -> Vec3 {
    let (registry, mover_id, pawn) = if host {
        (
            &h.host_registry,
            h.host_mover.expect("host mover"),
            h.host_pawn,
        )
    } else {
        (
            &h.client_registry,
            h.client_mover.expect("client mover"),
            h.client_pawn.expect("client pawn"),
        )
    };
    let mover = registry
        .get_component::<KinematicMoverComponent>(mover_id)
        .expect("rotating mover phase");
    let mover_transform = registry
        .get_component::<Transform>(mover_id)
        .expect("rotating mover transform");
    let player_position = registry
        .get_component::<Transform>(pawn)
        .expect("rider transform")
        .position;
    (mover.spin_axis * mover.spin_rate_rad_s).cross(player_position - mover_transform.position)
}

fn horizontal(velocity: Vec3) -> Vec3 {
    Vec3::new(velocity.x, 0.0, velocity.z)
}

/// Drain `h` to the explicit drain condition, sending no new input. Caps iterations.
fn drain(h: &mut LoopbackHarness) {
    let mut iters = 0;
    while !h.is_drained() && iters < 4_000 {
        h.drain_step();
        iters += 1;
    }
    if !h.is_drained() {
        println!(
            "[drain debug] gave up after {iters}: in_flight(to_server={}, to_client={}) \
             cursor={:?} last_sent={:?} target={:?} client_acked={} server_tick={}",
            h.to_server.in_flight(),
            h.to_client.in_flight(),
            h.command_queues.resolved_cursor(CLIENT_ID),
            h.last_sent_client_tick,
            h.drain_target_tick,
            h.client_acked_server_tick,
            h.server_tick,
        );
    }
}

/// Resolve one command for `client_id` directly off the harness command queues —
/// the host gap-policy resolution seam, used by the inject-at-seam scenario tests.
fn run_resolve(
    h: &mut LoopbackHarness,
    client_id: u64,
) -> Option<super::command_queue::ResolvedCommand> {
    h.command_queues.resolve_tick(client_id)
}

/// Synthesize a STALE raw snapshot: an older sequence than the client's latest,
/// carrying a far-off pose for the host pawn. The real apply path's sequence guard
/// must reject it wholesale.
fn stale_snapshot_for(h: &LoopbackHarness) -> postretro_net::wire::SnapshotMessage {
    use postretro_net::wire::{ComponentPayload, EntityRecord};

    let latest = h
        .client_replication
        .latest_sequence()
        .expect("client has applied at least one snapshot");
    // An older sequence: guaranteed <= latest, so rejected.
    let stale_sequence = latest.saturating_sub(1);

    let net = h.host_pawn_network_id.0;
    postretro_net::wire::SnapshotMessage {
        sequence: stale_sequence,
        server_tick: 0,
        records: vec![EntityRecord::Delta {
            network_id: net,
            baseline_ref: 0,
            new_baseline_id: 0,
            components: vec![ComponentPayload::Transform(
                crate::netcode::transform_to_wire(&Transform {
                    position: Vec3::new(-999.0, 1.0, -999.0),
                    ..Transform::default()
                }),
            )],
            local_player: true,
            last_processed_client_tick: Some(0),
            // A synthetic STALE fixture rejected by the sequence guard before apply;
            // it never exercises client materialization, so no class is stamped.
            entity_class: None,
            active_weapon_archetype: None,
        }],
        state_schema_fingerprint: [0u8; 32],
        state_records: Vec::new(),
    }
}

impl LoopbackHarness {
    /// A full step that additionally observes the reconcile correction taken on each
    /// snapshot applied this step. Returns `(took_teleport, max_correction_magnitude)`
    /// where the magnitude is the largest seeded presentation offset (the smoothed
    /// `predicted - reconciled` delta) over the snapshots applied this step. The gate
    /// uses this to assert sub-teleport corrections smooth and to report the worst
    /// per-snapshot correction under the conditioned link.
    pub(crate) fn step_and_watch_correction(&mut self, command: &SimCommand) -> (bool, f32) {
        self.client_predict_and_send(command);
        self.advance_clock();
        self.host_tick();

        // Wrap client_receive to observe the correction class via the public reconcile
        // return. We replicate client_receive here so we can capture the class.
        let mut took_teleport = false;
        let mut max_correction = 0.0_f32;
        let mut acks = Vec::new();
        for packet in self.to_client.take_ready() {
            let Ok(raw) =
                postretro_net::wire::decode::<postretro_net::wire::RawSnapshotMessage>(&packet)
            else {
                continue;
            };
            let Ok(snapshot) = raw.validate() else {
                continue;
            };
            let outcome = self
                .client_replication
                .apply_snapshot(&mut self.client_registry, &snapshot);
            if let Some(armed) = &outcome.armed_local_pawn {
                self.prediction.arm(armed.network_id, armed.entity_id);
                self.client_pawn = Some(armed.entity_id);
                let entity_class = armed.entity_class.as_deref().unwrap_or("player");
                let entity_class =
                    super::descriptor_class::decode_replicated_descriptor_class(entity_class)
                        .canonical_name();
                crate::scripting::builtins::net_descriptor::materialize_net_local_movement_component(
                    entity_class,
                    &self.descriptors,
                    &mut self.client_registry,
                    armed.entity_id,
                );
            }
            if let Some(reconcile) = outcome.local_reconcile {
                let class = reconcile_local_pawn(
                    &mut self.client_registry,
                    &mut self.prediction,
                    reconcile.entity_id,
                    reconcile.transform,
                    reconcile.movement.as_ref(),
                    reconcile.acked_tick,
                    &self.world,
                    GRAVITY,
                    DT,
                );
                match class {
                    Some(super::prediction::CorrectionClass::Teleport) => took_teleport = true,
                    // A smoothed correction seeds the presentation offset; its length is
                    // the magnitude of this correction.
                    Some(_) => {
                        max_correction =
                            max_correction.max(self.prediction.presentation_offset().length());
                    }
                    None => {}
                }
            }
            if let Some(ack) = outcome.ack {
                self.client_acked_server_tick =
                    self.client_acked_server_tick.max(ack.acked_server_tick);
                acks.push(ack);
            }
        }
        self.apply_acks(&acks);
        (took_teleport, max_correction)
    }
}
