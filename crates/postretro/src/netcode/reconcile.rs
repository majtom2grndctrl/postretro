// Client-side reconciliation of the local predicted pawn against an authoritative
// snapshot record (M15 Phase 3 Task 5): merge the authoritative movement subset,
// restore the authoritative transform, prune command history through the host ack,
// replay the unacked tail, snap the reconciled gameplay state into the registry, and
// classify the resulting correction to seed (or skip) the decaying presentation
// offset that smooths the local first-person camera.
// See: context/lib/networking.md · context/lib/movement.md
//
// Boundary: this is the registry-touching orchestration; the durable prediction
// state (history ring, presentation offset, classification thresholds) lives in
// `prediction.rs`. `client.rs` only surfaces the authoritative record
// (`LocalReconcileInput`); this glue reconciles it. The movement-only `replay`
// helper (also in `prediction.rs`) is the sole movement path — no AI/weapons/death,
// guaranteed by its registry-blind signature.

use postretro_net::wire::WirePlayerMovementState;

use crate::collision::CollisionWorld;
use crate::netcode::movement_state::merge_wire_into_movement_state;
use crate::netcode::prediction::replay;
use crate::netcode::prediction::{
    ArmedPawn, ClientPrediction, CorrectionClass, classify_correction,
};
use crate::scripting::components::player_movement::PlayerMovementComponent;
use crate::scripting::registry::{EntityId, EntityRegistry, Transform};

/// Reconcile the local predicted pawn against the authoritative record carried in a
/// snapshot. Returns the [`CorrectionClass`] of the applied correction, or `None`
/// when nothing was reconciled (prediction not armed to this pawn, the pawn is
/// missing/has no movement component, or the record was for a different pawn). The
/// returned class is the Task 6 harness seam — it asserts the engine took the
/// expected smoothed/snapped path.
///
/// Steps (the AC contract):
/// 1. **Merge** the `PlayerMovementState` subset onto the EXISTING descriptor-derived
///    component via [`merge_wire_into_movement_state`] — never reconstructs one.
/// 2. **Restore** the authoritative `Transform`.
/// 3. **Prune** command history through `acked_tick` ([`ClientPrediction::prune_through_ack`]).
///    Special case: `acked_tick == None` after prediction has started is an
///    authoritative RESET — clear history and apply the baseline WITHOUT pruning by
///    tick.
/// 4. **Replay** the remaining (unacked) commands with the movement-only [`replay`].
/// 5. **Write back** the reconciled `(Transform, PlayerMovementComponent)` to the
///    registry — the gameplay-authoritative snap, read by collision/AI/future prediction.
/// 6. **Classify + smooth** the correction (`predicted_pose - reconciled_pose`):
///    seed a decaying presentation offset for a smoothed correction; for a teleport,
///    clear history + offset and stamp the registry transform prev == current.
#[allow(clippy::too_many_arguments)]
pub(crate) fn reconcile_local_pawn(
    registry: &mut EntityRegistry,
    prediction: &mut ClientPrediction,
    entity_id: EntityId,
    authoritative_transform: Transform,
    movement: Option<&WirePlayerMovementState>,
    acked_tick: Option<u32>,
    collision: &CollisionWorld,
    gravity: f32,
    dt: f32,
) -> Option<CorrectionClass> {
    // Only reconcile the armed local pawn, and only the entity it maps to. A record
    // for a stale/other entity is ignored (the caller filters by NetworkId, but the
    // armed-entity match is the load-bearing guard).
    let armed: ArmedPawn = prediction.armed()?;
    if armed.entity_id != entity_id {
        return None;
    }

    // The local pawn must already carry a descriptor-derived component to merge onto
    // (entity_model.md §7b: the wire subset never constructs one). If it is absent the
    // client has not materialized the pawn's movement component yet; skip this record.
    let mut component: PlayerMovementComponent = registry
        .get_component::<PlayerMovementComponent>(entity_id)
        .ok()?
        .clone();

    // The pre-reconcile predicted pose: what the client currently shows for the pawn
    // (the latest forward-predicted state in the registry). The correction is measured
    // against this so the presentation offset glides from the predicted pose to the
    // reconciled one.
    let predicted_transform = registry.get_component::<Transform>(entity_id).ok().copied();

    // 1. Merge the authoritative movement subset onto the existing component.
    if let Some(wire) = movement {
        merge_wire_into_movement_state(&mut component, wire);
    }

    // 2. Restore from the authoritative transform.
    let mut reconciled_transform = authoritative_transform;

    // 3. Prune / reset. `None` ack after prediction started is an authoritative reset:
    //    the host has resolved none of the client's commands, so the baseline replaces
    //    the entire predicted ring — clear history, do NOT prune by tick, do NOT replay.
    let prediction_started = !prediction.history().is_empty();
    let is_reset = acked_tick.is_none() && prediction_started;
    if is_reset {
        prediction.clear_history();
    } else if let Some(tick) = acked_tick {
        prediction.prune_through_ack(tick);
    }

    // 4. Replay the remaining (unacked) commands on top of the authoritative baseline.
    //    Skipped on a reset (history was cleared). Threads collision/gravity/dt through
    //    the movement-only replay so the reconciled tail matches the forward prediction.
    if !is_reset {
        for entry in prediction.history().clone() {
            let sim = crate::netcode::wire_convert::input_command_to_sim(&entry.command);
            let (next_transform, next_movement, _events) = replay(
                reconciled_transform,
                component,
                sim.movement,
                collision,
                gravity,
                dt,
            );
            reconciled_transform = next_transform;
            component = next_movement;
        }
    }

    // 5. Snap the gameplay-authoritative state into the registry: transform + movement
    //    component. Collision, AI, and future prediction read this immediately — and
    //    "registry is truth, history is a command log" means the NEXT predict_tick
    //    chains from THIS reconciled pose (it reads `prev` from the registry, never a
    //    stored history pose), so the correction is not silently overwritten by a
    //    prediction chained off the stale pre-reconcile pose.
    let _ = registry.set_component(entity_id, reconciled_transform);
    let _ = registry.set_component(entity_id, component.clone());

    // 6. Classify the correction (predicted - reconciled) and smooth or snap. With no
    //    prior predicted pose (first arming snapshot) there is nothing to correct.
    let Some(predicted) = predicted_transform else {
        return Some(CorrectionClass::Ordinary);
    };
    let correction = predicted.position - reconciled_transform.position;
    let magnitude = correction.length();
    let included_dash = prediction.unacked_window_included_dash();
    let class = classify_correction(magnitude, included_dash);

    match class {
        CorrectionClass::Teleport => {
            // Snap hard: no smoothed glide. Clear the predicted ring (the trajectory
            // diverged too far to replay onto) and the presentation offset, and stamp
            // the registry transform prev == current so the render blend leaves no
            // visible slide across the teleport. Generalizes the remote-presentation
            // transform-history reset for the local pawn.
            prediction.clear_history();
            prediction.clear_presentation_offset();
            let _ = registry.set_presentation_transform(entity_id, reconciled_transform);
        }
        CorrectionClass::Ordinary | CorrectionClass::Dash | CorrectionClass::OversizedSmoothed => {
            // Smooth: the registry snapped to the reconciled pose; seed the decaying
            // presentation offset so the rendered first-person eye glides from where
            // the client predicted to the authoritative pose over a few render frames.
            if matches!(class, CorrectionClass::OversizedSmoothed) {
                log::debug!(
                    "[Net] oversized local correction {magnitude:.3} m smoothed (above the ordinary cap, below the teleport floor)"
                );
            }
            prediction.seed_presentation_offset(correction);
        }
    }

    Some(class)
}

#[cfg(test)]
mod tests {
    use super::*;

    use glam::Vec3;
    use parry3d::math::{Isometry, Point};
    use parry3d::shape::TriMesh;

    use postretro_net::wire::{
        InputCommand, NetworkId, WireFireButtonState, WireMovementInput, WireMovementState,
        WirePlayerMovementState,
    };

    use crate::netcode::movement_state::movement_state_to_wire;
    use crate::netcode::prediction::{
        DASH_CORRECTION_MAX_M, ORDINARY_CORRECTION_MAX_M, TELEPORT_CORRECTION_MIN_M,
    };
    use crate::scripting::data_descriptors::{
        AirParams, BoolOrIr, CapsuleParams, DashParams, FallParams, ForgivenessParams,
        GroundParams, NumberOrIr, PlayerMovementDescriptor, SpeedParams,
    };

    const EPSILON: f32 = 1e-4;
    const DT: f32 = 1.0 / 60.0;
    const GRAVITY: f32 = -20.0;
    const START: Vec3 = Vec3::new(0.0, 1.21, 0.0);

    fn floor_world() -> CollisionWorld {
        let points = vec![
            Point::new(-500.0, 0.0, -500.0),
            Point::new(500.0, 0.0, -500.0),
            Point::new(500.0, 0.0, 500.0),
            Point::new(-500.0, 0.0, 500.0),
        ];
        let triangles = vec![[0, 2, 1], [0, 3, 2]];
        CollisionWorld {
            mesh: TriMesh::new(points, triangles),
            isometry: Isometry::identity(),
        }
    }

    fn descriptor() -> PlayerMovementDescriptor {
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
            dash: Some(DashParams {
                boost_speed: NumberOrIr::Literal(18.0),
                momentum_retention: NumberOrIr::Literal(0.65),
                steer_control: NumberOrIr::Literal(0.2),
                dash_drag: NumberOrIr::Literal(18.0),
                cooldown_ms: NumberOrIr::Literal(250.0),
                air_dashes: 0,
                preserve_vertical: BoolOrIr::Literal(false),
            }),
            forgiveness: Some(ForgivenessParams {
                coyote_ms: 0.0,
                jump_buffer_ms: 0.0,
            }),
            crouch: None,
            view_feel: None,
        }
    }

    fn component() -> PlayerMovementComponent {
        PlayerMovementComponent::from_descriptor(&descriptor())
    }

    /// Spawn an armed local pawn at `START` carrying both a Transform and a
    /// descriptor-derived movement component, and arm `prediction` to it. Returns the
    /// pawn's `EntityId`.
    fn spawn_armed_pawn(
        registry: &mut EntityRegistry,
        prediction: &mut ClientPrediction,
        network_id: NetworkId,
    ) -> EntityId {
        let id = registry.spawn(Transform {
            position: START,
            ..Transform::default()
        });
        registry.set_component(id, component()).unwrap();
        prediction.arm(network_id, id);
        id
    }

    fn forward_command(client_tick: u32, dash_pressed: bool) -> InputCommand {
        InputCommand {
            client_tick,
            movement: WireMovementInput {
                wish_dir: [0.0, 1.0],
                jump_pressed: false,
                dash_pressed,
                running: true,
                crouch_intent: false,
                facing_yaw: 0.0,
            },
            fire_button: WireFireButtonState {
                pressed: false,
                active: false,
            },
        }
    }

    /// The authoritative movement wire payload for a fresh grounded pawn (the host's
    /// resolved state). Velocity zeroed so the merge is observable.
    fn authoritative_movement() -> WirePlayerMovementState {
        let mut wire = movement_state_to_wire(&component());
        wire.velocity = [0.0, 0.0, 0.0];
        wire.is_grounded = true;
        wire.movement_state = WireMovementState::Normal;
        wire
    }

    fn predicted_position(registry: &EntityRegistry, id: EntityId) -> Vec3 {
        registry.get_component::<Transform>(id).unwrap().position
    }

    // --- Reconcile merges movement state, restores the transform, prunes through the
    // ack, replays the remaining commands, and writes the reconciled state back. ---
    #[test]
    fn reconcile_merges_restores_prunes_replays_and_writes_back() {
        let world = floor_world();
        let mut registry = EntityRegistry::new();
        let mut prediction = ClientPrediction::new();
        let net = NetworkId(7);
        let id = spawn_armed_pawn(&mut registry, &mut prediction, net);

        // Forward-predict 4 ticks (client_tick 0..=3), writing the predicted state to
        // the registry like `client_predict_tick` does.
        let mut prev = (
            *registry.get_component::<Transform>(id).unwrap(),
            registry
                .get_component::<PlayerMovementComponent>(id)
                .unwrap()
                .clone(),
        );
        for tick in 0..4u32 {
            let (t, m) = prediction
                .predict_tick(
                    forward_command(tick, false),
                    prev.clone(),
                    &world,
                    GRAVITY,
                    DT,
                )
                .unwrap();
            registry.set_component(id, t).unwrap();
            registry.set_component(id, m.clone()).unwrap();
            prev = (t, m);
        }
        assert_eq!(prediction.history().len(), 4);

        // The authoritative record acks through tick 1 and restores a transform a hair
        // behind the predicted pose (a small ordinary correction).
        let predicted_before = predicted_position(&registry, id);
        let auth_transform = Transform {
            position: Vec3::new(0.0, 1.21, predicted_before.z + 0.05),
            ..Transform::default()
        };
        let mut auth_move = authoritative_movement();
        auth_move.velocity = [0.0, 0.0, -7.0];

        let class = reconcile_local_pawn(
            &mut registry,
            &mut prediction,
            id,
            auth_transform,
            Some(&auth_move),
            Some(1),
            &world,
            GRAVITY,
            DT,
        )
        .expect("armed pawn reconciles");

        // Pruned through ack 1: ticks 0,1 dropped, ticks 2,3 replayed and remain.
        let remaining: Vec<u32> = prediction.history().iter().map(|e| e.client_tick).collect();
        assert_eq!(remaining, vec![2, 3], "history pruned through the ack");

        // The merge wrote the authoritative velocity onto the EXISTING component
        // (then replay advanced it). Grounded forward replay keeps the pawn grounded.
        let reconciled = registry
            .get_component::<PlayerMovementComponent>(id)
            .unwrap();
        assert!(reconciled.is_grounded, "reconciled pawn is grounded");

        // The reconciled transform replayed the 2 unacked commands from the
        // authoritative baseline: it advanced forward (-Z) from auth_transform.
        let reconciled_z = predicted_position(&registry, id).z;
        assert!(
            reconciled_z < auth_transform.position.z - EPSILON,
            "two unacked forward commands replayed onto the authoritative baseline (z={reconciled_z})"
        );

        // A small correction classifies as ordinary (or oversized-smoothed if the
        // replay diverged), never a teleport.
        assert_ne!(
            class,
            CorrectionClass::Teleport,
            "small correction is smoothed"
        );
    }

    // --- Ordinary correction: seeds a nonzero, decaying presentation offset; the
    // registry transform snaps immediately while the presented pose lags + converges. ---
    #[test]
    fn ordinary_correction_seeds_decaying_offset_registry_snaps() {
        let world = floor_world();
        let mut registry = EntityRegistry::new();
        let mut prediction = ClientPrediction::new();
        let id = spawn_armed_pawn(&mut registry, &mut prediction, NetworkId(1));

        // One predicted tick so there is a predicted pose to correct against.
        let prev = (
            *registry.get_component::<Transform>(id).unwrap(),
            component(),
        );
        let (t, m) = prediction
            .predict_tick(forward_command(0, false), prev, &world, GRAVITY, DT)
            .unwrap();
        registry.set_component(id, t).unwrap();
        registry.set_component(id, m).unwrap();
        let predicted = predicted_position(&registry, id);

        // Authoritative pose 0.2 m off the predicted pose (well under the ordinary cap),
        // acking all commands so nothing replays — the correction is purely the
        // baseline-vs-predicted delta.
        let off = 0.2_f32;
        assert!(off <= ORDINARY_CORRECTION_MAX_M);
        let auth_transform = Transform {
            position: predicted + Vec3::new(off, 0.0, 0.0),
            ..Transform::default()
        };
        let class = reconcile_local_pawn(
            &mut registry,
            &mut prediction,
            id,
            auth_transform,
            Some(&authoritative_movement()),
            Some(0),
            &world,
            GRAVITY,
            DT,
        )
        .unwrap();
        assert_eq!(class, CorrectionClass::Ordinary);

        // Registry snapped to the authoritative pose immediately.
        assert!(
            (predicted_position(&registry, id) - auth_transform.position).length() < EPSILON,
            "registry transform snaps to the reconciled pose"
        );
        // A nonzero presentation offset was seeded; the presented pose still shows the
        // pre-reconcile predicted pose.
        let offset = prediction.presentation_offset();
        assert!(offset.length() > EPSILON, "a nonzero offset is seeded");
        let presented = prediction.present_local_pose(auth_transform);
        assert!(
            (presented.position - predicted).length() < EPSILON,
            "the presented pose lags at the predicted pose the frame the correction lands"
        );

        // It decays toward zero across render frames and converges.
        let before = offset.length();
        prediction.decay_presentation_offset();
        let after = prediction.presentation_offset().length();
        assert!(after < before, "the offset decays each render frame");
        for _ in 0..64 {
            prediction.decay_presentation_offset();
        }
        assert_eq!(
            prediction.presentation_offset(),
            Vec3::ZERO,
            "the offset converges to exactly zero"
        );
    }

    // --- Dash correction (above the ordinary cap, within the dash cap) on a window
    // whose unacked tail crossed a dash classifies as Dash. The dash entry stays
    // unacked (client_tick 5, ack 4), so it replays and the window crosses a dash;
    // a lateral baseline offset puts the resulting correction in the dash band. ---
    #[test]
    fn dash_correction_on_dash_tick_classifies_as_dash() {
        let world = floor_world();
        let mut registry = EntityRegistry::new();
        let mut prediction = ClientPrediction::new();
        let id = spawn_armed_pawn(&mut registry, &mut prediction, NetworkId(2));

        // One dash tick at client_tick 5; ack 4 leaves it unacked so it replays.
        let prev = (
            *registry.get_component::<Transform>(id).unwrap(),
            component(),
        );
        let (t, m) = prediction
            .predict_tick(forward_command(5, true), prev, &world, GRAVITY, DT)
            .unwrap();
        registry.set_component(id, t).unwrap();
        registry.set_component(id, m).unwrap();
        let predicted = predicted_position(&registry, id);

        // Baseline laterally offset so the post-replay correction lands above the
        // ordinary cap and within the dash cap. The lateral component dominates the
        // small per-tick replay delta, keeping the magnitude in the dash band.
        let off = 1.0_f32;
        let auth_transform = Transform {
            position: predicted + Vec3::new(off, 0.0, 0.0),
            ..Transform::default()
        };

        let class = reconcile_local_pawn(
            &mut registry,
            &mut prediction,
            id,
            auth_transform,
            Some(&authoritative_movement()),
            Some(4),
            &world,
            GRAVITY,
            DT,
        )
        .unwrap();
        assert!(
            prediction.unacked_window_included_dash(),
            "the unacked tail still crosses a dash"
        );
        // The measured correction must sit in the dash band for the classification to
        // be meaningful (derived from the named caps, not a magic number).
        let correction = prediction.presentation_offset().length();
        assert!(
            correction > ORDINARY_CORRECTION_MAX_M && correction <= DASH_CORRECTION_MAX_M,
            "correction {correction} m lands in the dash band ({ORDINARY_CORRECTION_MAX_M}..={DASH_CORRECTION_MAX_M})"
        );
        assert_eq!(
            class,
            CorrectionClass::Dash,
            "dash-window correction classifies as Dash"
        );
    }

    // --- Teleport (>= TELEPORT floor): clears history + offset, snaps the registry,
    // and stamps previous/current transform equal (no render-blend slide). ---
    #[test]
    fn teleport_correction_clears_history_offset_and_stamps_prev_equal_current() {
        let world = floor_world();
        let mut registry = EntityRegistry::new();
        let mut prediction = ClientPrediction::new();
        let id = spawn_armed_pawn(&mut registry, &mut prediction, NetworkId(3));

        // Predict two ticks and seed a stale offset to prove the teleport clears it.
        let prev = (
            *registry.get_component::<Transform>(id).unwrap(),
            component(),
        );
        let (t, m) = prediction
            .predict_tick(forward_command(0, false), prev.clone(), &world, GRAVITY, DT)
            .unwrap();
        registry.set_component(id, t).unwrap();
        registry.set_component(id, m.clone()).unwrap();
        prediction
            .predict_tick(forward_command(1, false), (t, m), &world, GRAVITY, DT)
            .unwrap();
        prediction.seed_presentation_offset(Vec3::new(0.1, 0.0, 0.0));
        let predicted = predicted_position(&registry, id);

        // Authoritative pose a teleport-distance away, acking all so nothing replays.
        let far = TELEPORT_CORRECTION_MIN_M + 1.0;
        let auth_transform = Transform {
            position: predicted + Vec3::new(far, 0.0, 0.0),
            ..Transform::default()
        };
        let class = reconcile_local_pawn(
            &mut registry,
            &mut prediction,
            id,
            auth_transform,
            Some(&authoritative_movement()),
            Some(1),
            &world,
            GRAVITY,
            DT,
        )
        .unwrap();
        assert_eq!(class, CorrectionClass::Teleport);

        // History + offset cleared; registry snapped to the authoritative pose.
        assert!(prediction.history().is_empty(), "teleport clears history");
        assert_eq!(
            prediction.presentation_offset(),
            Vec3::ZERO,
            "teleport clears the presentation offset"
        );
        assert!(
            (predicted_position(&registry, id) - auth_transform.position).length() < EPSILON,
            "registry snaps to the authoritative pose"
        );
        // prev == current: the render blend reproduces the snapped pose at any alpha.
        let at_zero = registry.interpolated_transform(id, 0.0).unwrap();
        let at_one = registry.interpolated_transform(id, 1.0).unwrap();
        assert!(
            (at_zero.position - at_one.position).length() < EPSILON,
            "previous and current transform are stamped equal (no teleport slide)"
        );
    }

    // --- A `local_player` record with a `None` ack AFTER prediction started is an
    // authoritative reset: clears history, applies the baseline, does NOT prune/replay. ---
    #[test]
    fn none_ack_after_prediction_started_resets_to_baseline() {
        let world = floor_world();
        let mut registry = EntityRegistry::new();
        let mut prediction = ClientPrediction::new();
        let id = spawn_armed_pawn(&mut registry, &mut prediction, NetworkId(4));

        // Prediction has started: a few predicted ticks in the ring.
        let mut prev = (
            *registry.get_component::<Transform>(id).unwrap(),
            component(),
        );
        for tick in 0..3u32 {
            let (t, m) = prediction
                .predict_tick(
                    forward_command(tick, false),
                    prev.clone(),
                    &world,
                    GRAVITY,
                    DT,
                )
                .unwrap();
            registry.set_component(id, t).unwrap();
            registry.set_component(id, m.clone()).unwrap();
            prev = (t, m);
        }
        assert!(!prediction.history().is_empty());

        // A None-ack baseline well away from the predicted pose.
        let baseline = Transform {
            position: Vec3::new(5.0, 1.21, -2.0),
            ..Transform::default()
        };
        reconcile_local_pawn(
            &mut registry,
            &mut prediction,
            id,
            baseline,
            Some(&authoritative_movement()),
            None,
            &world,
            GRAVITY,
            DT,
        )
        .unwrap();

        // History cleared (reset), and the registry holds the baseline verbatim — NOT
        // a replayed-forward pose (no commands were replayed).
        assert!(
            prediction.history().is_empty(),
            "None ack after start clears history"
        );
        assert!(
            (predicted_position(&registry, id) - baseline.position).length() < EPSILON,
            "the baseline is applied verbatim with no replay"
        );
    }

    // --- A `None` ack BEFORE prediction has started (empty ring) applies the baseline
    // without treating it as a reset (nothing to prune/replay anyway). ---
    #[test]
    fn none_ack_before_prediction_started_applies_baseline() {
        let world = floor_world();
        let mut registry = EntityRegistry::new();
        let mut prediction = ClientPrediction::new();
        let id = spawn_armed_pawn(&mut registry, &mut prediction, NetworkId(5));
        assert!(prediction.history().is_empty());

        // The realistic arming snapshot: the baseline matches the spawn pose closely
        // (here a small step from START), so it is an ordinary correction — NOT a reset
        // (the None-ack reset path only triggers once prediction has started).
        let baseline = Transform {
            position: START + Vec3::new(0.1, 0.0, 0.0),
            ..Transform::default()
        };
        let class = reconcile_local_pawn(
            &mut registry,
            &mut prediction,
            id,
            baseline,
            Some(&authoritative_movement()),
            None,
            &world,
            GRAVITY,
            DT,
        )
        .unwrap();
        assert_eq!(
            class,
            CorrectionClass::Ordinary,
            "arming-snapshot baseline applies cleanly"
        );
        assert!(
            (predicted_position(&registry, id) - baseline.position).length() < EPSILON,
            "baseline applied"
        );
    }

    // Regression: a reconcile that landed while the command tail was still unacked
    // was silently overwritten by the NEXT predicted tick, because predict_tick
    // chained from the stored pre-reconcile history pose instead of the reconciled
    // registry state. Gameplay/collision/future-prediction diverged from authority
    // while only the presentation offset reflected the correction. With "registry is
    // truth, history is a command log", the next predict_tick chains from the
    // reconciled registry pose. This FAILS against the old chain-from-history.rs.
    #[test]
    fn next_predict_chains_from_reconciled_pose_with_unacked_tail() {
        let world = floor_world();
        let mut registry = EntityRegistry::new();
        let mut prediction = ClientPrediction::new();
        let id = spawn_armed_pawn(&mut registry, &mut prediction, NetworkId(8));

        // Predict 4 ticks (client_tick 0..=3), chaining through the registry exactly
        // as client_predict_tick does.
        let mut prev = (
            *registry.get_component::<Transform>(id).unwrap(),
            registry
                .get_component::<PlayerMovementComponent>(id)
                .unwrap()
                .clone(),
        );
        for tick in 0..4u32 {
            let (t, m) = prediction
                .predict_tick(
                    forward_command(tick, false),
                    prev.clone(),
                    &world,
                    GRAVITY,
                    DT,
                )
                .unwrap();
            registry.set_component(id, t).unwrap();
            registry.set_component(id, m.clone()).unwrap();
            prev = (t, m);
        }
        let pre_reconcile_pose = predicted_position(&registry, id);

        // Reconcile mid-stream: ack ONLY through tick 1, so ticks 2 and 3 stay UNACKED
        // (the realistic in-flight case). Apply a non-trivial baseline shift (+1.0 m
        // along +X) at the acked baseline so the reconciled pose is clearly distinct
        // from the pre-reconcile prediction.
        let auth_transform = Transform {
            position: Vec3::new(1.0, 1.21, pre_reconcile_pose.z),
            ..Transform::default()
        };
        reconcile_local_pawn(
            &mut registry,
            &mut prediction,
            id,
            auth_transform,
            Some(&authoritative_movement()),
            Some(1),
            &world,
            GRAVITY,
            DT,
        )
        .expect("armed pawn reconciles");

        // The registry now holds the reconciled pose (baseline + the 2 replayed unacked
        // forward commands): clearly shifted +X from the pre-reconcile prediction.
        let reconciled_pose = predicted_position(&registry, id);
        assert!(
            (reconciled_pose.x - pre_reconcile_pose.x) > 0.5,
            "the reconciled pose carries the +1.0 m baseline shift (x={})",
            reconciled_pose.x
        );

        // Predict the NEXT tick (client_tick 4) the way the real caller does: read
        // `prev` FRESH from the reconciled registry, predict, write back.
        let prev_next = (
            *registry.get_component::<Transform>(id).unwrap(),
            registry
                .get_component::<PlayerMovementComponent>(id)
                .unwrap()
                .clone(),
        );
        let (t, m) = prediction
            .predict_tick(forward_command(4, false), prev_next, &world, GRAVITY, DT)
            .unwrap();
        registry.set_component(id, t).unwrap();
        registry.set_component(id, m).unwrap();
        let next_pose = predicted_position(&registry, id);

        // The next predicted pose must be chained from the RECONCILED pose: it keeps
        // the +X baseline shift and advances forward (-Z) one more tick from there.
        // The old chain-from-history would discard the shift, landing back near the
        // pre-reconcile trajectory (x ~ 0).
        assert!(
            (next_pose.x - reconciled_pose.x).abs() < EPSILON,
            "the next prediction keeps the reconciled +X shift (next.x={}, reconciled.x={})",
            next_pose.x,
            reconciled_pose.x
        );
        assert!(
            next_pose.z < reconciled_pose.z - EPSILON,
            "the next prediction advances forward from the reconciled pose, not the stale one"
        );
        assert!(
            (next_pose.x - pre_reconcile_pose.x) > 0.5,
            "the correction was NOT overwritten by chaining off the pre-reconcile pose"
        );
    }

    // --- Reconcile is a no-op for an unarmed prediction or a mismatched entity. ---
    #[test]
    fn reconcile_no_ops_when_unarmed_or_entity_mismatch() {
        let world = floor_world();
        let mut registry = EntityRegistry::new();
        let mut prediction = ClientPrediction::new();
        let id = registry.spawn(Transform {
            position: START,
            ..Transform::default()
        });
        registry.set_component(id, component()).unwrap();

        // Unarmed: None.
        assert!(
            reconcile_local_pawn(
                &mut registry,
                &mut prediction,
                id,
                Transform::default(),
                Some(&authoritative_movement()),
                Some(0),
                &world,
                GRAVITY,
                DT,
            )
            .is_none(),
            "unarmed prediction does not reconcile"
        );

        // Armed to a DIFFERENT entity: a record for `id` is ignored.
        let other = registry.spawn(Transform::default());
        prediction.arm(NetworkId(9), other);
        assert!(
            reconcile_local_pawn(
                &mut registry,
                &mut prediction,
                id,
                Transform::default(),
                Some(&authoritative_movement()),
                Some(0),
                &world,
                GRAVITY,
                DT,
            )
            .is_none(),
            "a record for a non-armed entity is ignored"
        );
    }
}
