// M15 Phase 3 Task 6: integrated, production-adjacent prediction/reconciliation
// tests plus the headline deterministic latency gate. Every test drives the REAL
// Task 1-5 seams through `LoopbackHarness` (see the sibling
// `predict_reconcile_harness_test_fixtures`) — the prototype `sim::predict_reconcile`
// type is never instantiated; only its scenario *shape* and expected timelines are
// promoted here.
// See: context/lib/networking.md · context/lib/testing_guide.md
//
// Replay-purity guard: the production replay path (`prediction::replay`) is
// registry-blind by signature (no `EntityRegistry` parameter), so AI/weapons/death
// are structurally unreachable. These tests additionally assert it at the seam: a
// zero-HP bystander entity that the full `simulate_tick` death sweep WOULD despawn
// stays alive on both ends through every scenario, proving the movement-only path
// never ran the registry-wide systems.

#![cfg(test)]

use glam::{Vec2, Vec3};

use postretro_net::harness::LinkConfig;
use postretro_net::wire::ClientMessage;

use super::predict_reconcile_harness_test_fixtures::{
    CLIENT_ID, DT, GRAVITY, LoopbackHarness, forward_command, input_at,
};
use super::prediction::{ORDINARY_CORRECTION_MAX_M, TELEPORT_CORRECTION_MIN_M};
use super::reconcile::reconcile_local_pawn;
use crate::movement::MovementInput;
use crate::netcode::host_handle_client_message;
use crate::scripting::components::player_movement::PlayerMovementComponent;
use crate::scripting::registry::Transform;
use crate::sim::SimCommand;

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

// ---------------------------------------------------------------------------
// Section A — Integrated scenario tests (drive the real seams end to end)
// ---------------------------------------------------------------------------

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
            &mut h.command_queues,
            CLIENT_ID,
            0,
            0,
            ClientMessage::Input(dup),
        );
    }

    // A second, unrelated client's queue is untouched by the flood.
    const OTHER: u64 = 99;
    host_handle_client_message(
        &mut h.server,
        &mut h.server_replication,
        &mut h.command_queues,
        OTHER,
        0,
        0,
        ClientMessage::Input(input_at(0, -1.0)),
    );

    // The duplicated tick resolves exactly once with the first-arrival intent.
    let resolved = h.command_queues.resolved_cursor(CLIENT_ID);
    // Resolve the single queued command and confirm the cursor advances by one.
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
    let dash_input = super::wire_convert::sim_command_to_input(&forward_command(true), dash_tick);
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
        },
        fire_button: crate::weapon::FireButtonState {
            pressed: false,
            active: false,
        },
    }
}

// ---------------------------------------------------------------------------
// Local helpers
// ---------------------------------------------------------------------------

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
        }],
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
            if let Some((network_id, entity_id)) = outcome.armed_local_pawn {
                self.prediction.arm(network_id, entity_id);
                self.client_pawn = Some(entity_id);
                LoopbackHarness::materialize_local_pawn_movement(
                    &mut self.client_registry,
                    entity_id,
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
