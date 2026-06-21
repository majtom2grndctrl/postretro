// Client-side movement prediction state for M15 Phase 3: the command/predicted-
// state ring for the local pawn, the armed `NetworkId -> EntityId` baseline, the
// forward-prediction tick, prune-through-ack, and the side-effect-free
// movement-only replay helper shared with Task 5's reconciliation.
// See: context/lib/networking.md · context/lib/movement.md
//
// Boundary: the replay helper lives HERE (not in `sim/`) because production
// movement prediction is a netcode concern and `movement::tick` is reachable from
// this module via `crate::movement`. Co-locating it with the prediction storage
// keeps the whole prediction seam in one place. It is the inverse of `sim/`'s
// `predict_reconcile.rs` dev harness: that one drives the FULL `simulate_tick`
// (AI/weapons/death) through two local sims; this is the movement-only production
// path that NEVER runs those systems and NEVER touches the `EntityRegistry`.

use std::collections::VecDeque;

use glam::Vec3;

use postretro_net::wire::{InputCommand, NetworkId};

use crate::collision::CollisionWorld;
use crate::movement::{self, MovementEvents, MovementInput};
use crate::netcode::wire_convert::input_command_to_sim;
use crate::scripting::components::player_movement::{MovementState, PlayerMovementComponent};
use crate::scripting::registry::{EntityId, Transform};

/// Upper bound on retained predicted-tick history. At 60 Hz this is one second of
/// unacked commands — well beyond any plausible host RTT for the loopback co-op
/// target. The ring drops its oldest entry when full so a stalled ack (lost
/// snapshots) can never grow the history unbounded.
const MAX_HISTORY: usize = 64;

// --- Correction classification thresholds (M15 Phase 3 Task 5 AC) ---
//
// A reconcile correction is the displacement between the pre-reconcile predicted
// pose and the post-reconcile authoritative pose. Its magnitude classifies how the
// local first-person *presentation* absorbs it. These are the pinned starting caps;
// the AC mandates that any change must be backed by measured harness rationale —
// they are not free tuning knobs. Derived expectations in tests read these consts,
// never re-typed magic numbers.

/// Ordinary correction cap: an offset magnitude at or below this (in metres) is a
/// small mispredict smoothed over a few render frames. The common case under
/// loopback latency.
pub(crate) const ORDINARY_CORRECTION_MAX_M: f32 = 0.5;

/// Dash correction cap: a correction on a tick whose history entry crossed a dash
/// (`included_dash`) tolerates a larger smoothed offset — a dash burst legitimately
/// produces a bigger visible delta than ordinary locomotion. Still smoothed, not
/// snapped, up to this magnitude.
pub(crate) const DASH_CORRECTION_MAX_M: f32 = 2.0;

/// Teleport threshold: a correction at or above this (in metres) is too large to
/// smooth without a visible rubber-band slide. It is snapped — history and the
/// presentation offset are cleared and the registry transform stamps prev == current
/// — so the pawn appears at the authoritative pose immediately.
pub(crate) const TELEPORT_CORRECTION_MIN_M: f32 = 3.0;

/// Fraction of the presentation offset retained per render frame as it decays to
/// zero. `offset *= DECAY_PER_FRAME` each frame, so a smoothed correction converges
/// over a bounded handful of frames (≈10 frames to fall below ~3% of its initial
/// magnitude at 0.7) — fast enough to feel responsive, slow enough to hide the
/// snap. Render-rate, not tick-rate: smoothing is presentation, not simulation.
const DECAY_PER_FRAME: f32 = 0.7;

/// Below this magnitude (metres) the presentation offset is treated as fully
/// converged and zeroed, so the decaying geometric tail does not leave a permanent
/// sub-visible bias on the rendered eye.
const OFFSET_SETTLED_EPSILON_M: f32 = 1e-4;

/// How a reconcile correction was classified by displacement magnitude. The
/// classification drives whether the registry-snapped correction is smoothed
/// (seeded as a decaying presentation offset) or snapped hard (teleport: offset
/// cleared, history cleared, prev == current stamped). Returned by the reconcile
/// entry point so Task 6's harness can assert the class directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CorrectionClass {
    /// `<= ORDINARY_CORRECTION_MAX_M`: a small mispredict, smoothed.
    Ordinary,
    /// `<= DASH_CORRECTION_MAX_M` on an `included_dash` tick: a larger but expected
    /// dash-window correction, smoothed.
    Dash,
    /// `>= TELEPORT_CORRECTION_MIN_M`: snapped, not smoothed.
    Teleport,
    /// In the gap between the smoothed caps and the teleport floor (or a dash-sized
    /// correction on a non-dash tick): smoothed like an ordinary correction, but the
    /// magnitude is logged as out-of-band. Kept distinct so the harness can see the
    /// engine took the seed-and-smooth path on a correction larger than the ordinary
    /// cap rather than silently widening `Ordinary`.
    OversizedSmoothed,
}

/// Classify a correction by its displacement `magnitude` (metres) and whether the
/// corrected tick crossed a dash. Pure: the single source of truth for the AC
/// thresholds, shared by the reconcile path and its tests.
///
/// - `>= TELEPORT_CORRECTION_MIN_M` → [`CorrectionClass::Teleport`] (checked first:
///   a teleport-sized delta is a teleport regardless of dash).
/// - `<= ORDINARY_CORRECTION_MAX_M` → [`CorrectionClass::Ordinary`].
/// - `<= DASH_CORRECTION_MAX_M` on a dash tick → [`CorrectionClass::Dash`].
/// - otherwise → [`CorrectionClass::OversizedSmoothed`] (still smoothed).
pub(crate) fn classify_correction(magnitude: f32, included_dash: bool) -> CorrectionClass {
    if magnitude >= TELEPORT_CORRECTION_MIN_M {
        CorrectionClass::Teleport
    } else if magnitude <= ORDINARY_CORRECTION_MAX_M {
        CorrectionClass::Ordinary
    } else if included_dash && magnitude <= DASH_CORRECTION_MAX_M {
        CorrectionClass::Dash
    } else {
        CorrectionClass::OversizedSmoothed
    }
}

/// One predicted fixed tick: the command frame sent to the host and the local
/// state it produced. Task 5 reconciliation reads `client_tick` to match the
/// host's `last_processed_client_tick` ack, restores from the authoritative
/// baseline, then replays the commands of every entry *after* the acked tick
/// through [`replay`].
//
// `client_tick`/`command`/`included_dash` are read by this module's tests and the
// Task 5 reconciliation (`reconcile.rs`) that walks the history.
#[derive(Debug, Clone)]
pub(crate) struct PredictedTick {
    /// The monotonic client command-frame number this tick was predicted at.
    /// Stamped into the outbound `InputCommand.client_tick` and matched against
    /// the host ack during reconciliation.
    pub(crate) client_tick: u32,
    /// The exact command frame sent to the host for this tick. Retained so
    /// reconciliation can re-feed it to [`replay`] verbatim.
    pub(crate) command: InputCommand,
    /// The predicted `Transform` after advancing this tick's movement.
    pub(crate) transform: Transform,
    /// The predicted `PlayerMovementComponent` after advancing this tick's
    /// movement.
    pub(crate) movement: PlayerMovementComponent,
    /// Whether the movement state entered or stayed in `Dash` while predicting
    /// this tick. Phase 3 instrumentation: reconciliation surfaces whether a
    /// rewound window crossed a dash so smoothing (Task 5) can special-case the
    /// larger visible correction a dash burst produces.
    pub(crate) included_dash: bool,
}

/// The armed local-pawn identity: the `NetworkId` the host flagged
/// `local_player: true` and the `EntityId` the client mapped it to. Prediction is
/// inert until this is set from an applied full `local_player` baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ArmedPawn {
    pub(crate) network_id: NetworkId,
    pub(crate) entity_id: EntityId,
}

/// Client-side movement prediction state for the local pawn. Owns the command +
/// predicted-state ring and the armed baseline identity. Long-lived prediction
/// state lives here, not on `App` — the source-layout gate keeps it out of the
/// 6k-line `main.rs` and the 1.4k-line `client.rs`.
///
/// Phase 3 scope (this task): storage, pruning, arming, and the forward-prediction
/// tick. Snapshot reconciliation (restore-from-authority, prune-through-ack,
/// replay-the-rest, presentation-offset smoothing) is Task 5 — it consumes the
/// `history` and `replay` seams this module exposes.
#[derive(Debug, Default)]
pub(crate) struct ClientPrediction {
    /// The armed local-pawn identity, or `None` until a `local_player` baseline
    /// has been applied AND mapped. Prediction does nothing while this is `None`.
    armed: Option<ArmedPawn>,
    /// Predicted-tick ring, oldest-first. One entry per predicted fixed tick;
    /// `client_tick` is monotonic non-decreasing across the deque. Pruned through
    /// the host ack and bounded to [`MAX_HISTORY`].
    history: VecDeque<PredictedTick>,
    /// The next monotonic command-frame number to stamp on an outbound
    /// `InputCommand`. Advances once per sent command — including the pre-baseline
    /// commands sent before prediction is armed — so the host sees one strictly
    /// increasing `client_tick` stream. Lives here (not on `App`) so the send-stamp
    /// allocator stays with the prediction state it feeds.
    next_client_tick: u32,
    /// Local-pawn correction smoothing (Task 5). The decaying difference between the
    /// pre-reconcile predicted pose and the reconciled authoritative pose, in world
    /// space. The registry transform always snaps to the authoritative result
    /// (gameplay/collision/AI read it); only the local first-person *presentation*
    /// adds this offset, decaying it to zero over a bounded number of render frames
    /// so the correction is continuous on screen without lying to the simulation.
    /// Owned here, never on `App`/`main.rs` — the source-layout gate.
    presentation_offset: Vec3,
}

impl ClientPrediction {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Arm (or re-arm) prediction with the local pawn baseline. Called by
    /// `ClientReplication` when it applies a full `local_player: true` baseline and
    /// has the stable `NetworkId -> EntityId` mapping. Re-arming to the SAME pawn is
    /// a no-op that preserves history (a periodic re-baseline must not wipe unacked
    /// predictions); arming to a DIFFERENT pawn clears the history, since the old
    /// ring describes a pawn the client no longer drives.
    pub(crate) fn arm(&mut self, network_id: NetworkId, entity_id: EntityId) {
        let next = ArmedPawn {
            network_id,
            entity_id,
        };
        if self.armed == Some(next) {
            return;
        }
        self.armed = Some(next);
        self.history.clear();
    }

    /// The armed local-pawn identity, or `None` if prediction is not yet armed.
    pub(crate) fn armed(&self) -> Option<ArmedPawn> {
        self.armed
    }

    /// Allocate the next monotonic command-frame number for an outbound
    /// `InputCommand.client_tick`. Strictly increasing across the session, advancing
    /// once per sent command (one per predicted fixed tick). The matching predicted
    /// tick — when armed — is recorded under this same number by [`predict_tick`].
    pub(crate) fn next_client_tick(&mut self) -> u32 {
        let tick = self.next_client_tick;
        self.next_client_tick = self.next_client_tick.wrapping_add(1);
        tick
    }

    /// Whether prediction is armed and may drive the local pawn this frame. Before
    /// arming the client may still SEND input commands to the host, but it must not
    /// spawn or advance a provisional local pawn. Used by this module's tests and the
    /// Task 5 reconciliation gate; staged until that caller lands.
    #[allow(dead_code)]
    pub(crate) fn is_armed(&self) -> bool {
        self.armed.is_some()
    }

    /// Read-only view of the predicted-tick ring (oldest-first). The reconcile path
    /// walks this to find the acked tick and replay the rest.
    pub(crate) fn history(&self) -> &VecDeque<PredictedTick> {
        &self.history
    }

    /// Advance the local pawn one predicted fixed tick: run the movement-only
    /// [`replay`] from the prior predicted state through `command`, record the
    /// result in the history ring, and return the resulting `(Transform,
    /// PlayerMovementComponent)` for the caller to write back into the registry.
    /// Returns `None` (and records nothing) when prediction is not armed — the
    /// before-baseline inert contract.
    ///
    /// The starting state is the most-recent history entry's predicted state, or
    /// `prev` (the registry's current applied state, seeded from the authoritative
    /// baseline) when the ring is empty. Pure with respect to the registry: the
    /// caller owns reading `prev` and writing the result back; this never touches
    /// the registry, AI, weapons, death, or reactions.
    pub(crate) fn predict_tick(
        &mut self,
        command: InputCommand,
        prev: (Transform, PlayerMovementComponent),
        collision: &CollisionWorld,
        gravity: f32,
        dt: f32,
    ) -> Option<(Transform, PlayerMovementComponent)> {
        // Inert until armed: before the local_player baseline, drive no pawn.
        self.armed?;

        let (start_transform, start_movement) = match self.history.back() {
            Some(last) => (last.transform, last.movement.clone()),
            None => prev,
        };

        let sim = input_command_to_sim(&command);
        // The command's dash request and the resulting movement state together cover
        // the whole dash window: the request bit catches the entry tick (before the
        // state has transitioned) and the resulting `Dash` state catches the ongoing
        // burst. Task 5 reads this to special-case the larger visible correction a
        // dash produces during a rewound replay.
        let dash_requested = command.movement.dash_pressed;
        let (transform, movement, _events) = replay(
            start_transform,
            start_movement,
            sim.movement,
            collision,
            gravity,
            dt,
        );

        let included_dash =
            dash_requested || matches!(movement.movement_state, MovementState::Dash { .. });

        if self.history.len() == MAX_HISTORY {
            // Drop the oldest unacked tick: a stalled ack must never grow history
            // unbounded. Task 5's reconciliation tolerates an ack older than the
            // ring (it simply finds no matching entry and trusts the authority).
            self.history.pop_front();
        }
        self.history.push_back(PredictedTick {
            client_tick: command.client_tick,
            command,
            transform,
            movement: movement.clone(),
            included_dash,
        });

        Some((transform, movement))
    }

    /// Prune every history entry whose `client_tick` is at or below `acked_tick`.
    /// The host has resolved those commands authoritatively, so their predictions
    /// are settled and need not be replayed again. Entries *after* the ack remain
    /// for Task 5 to replay on top of the authoritative baseline. Mechanism only:
    /// this task builds the prune; Task 5 invokes it as part of reconciliation.
    pub(crate) fn prune_through_ack(&mut self, acked_tick: u32) {
        while self
            .history
            .front()
            .is_some_and(|entry| entry.client_tick <= acked_tick)
        {
            self.history.pop_front();
        }
    }

    /// Clear all predicted history. Used by the reconcile path for the two cases that
    /// invalidate every retained prediction at once: an authoritative `None`-ack
    /// reset (the host has not resolved any of the client's commands, so the baseline
    /// supersedes the whole ring) and a teleport-class correction (the predicted
    /// trajectory diverged too far to replay onto).
    pub(crate) fn clear_history(&mut self) {
        self.history.clear();
    }

    /// Whether `included_dash` is set on any retained (unacked) history entry. The
    /// reconcile path reads this to classify the correction: a correction landing on
    /// a window that crossed a dash tolerates the larger [`DASH_CORRECTION_MAX_M`]
    /// smoothed cap. Checked over the post-prune ring (the unacked tail that was
    /// replayed onto the authoritative baseline).
    pub(crate) fn unacked_window_included_dash(&self) -> bool {
        self.history.iter().any(|entry| entry.included_dash)
    }

    /// Seed the local-pawn presentation offset from a fresh correction: the
    /// `correction` vector is `predicted_pose - reconciled_pose`, so adding it back to
    /// the (snapped) registry pose reproduces the pre-reconcile predicted pose, and
    /// decaying it to zero glides the rendered eye from where the client *thought* it
    /// was to where the authority says it is. Replaces any in-flight offset outright
    /// (the newest correction is authoritative over a stale one). Smoothing only —
    /// the caller has already snapped the registry to the reconciled result.
    pub(crate) fn seed_presentation_offset(&mut self, correction: Vec3) {
        self.presentation_offset = correction;
    }

    /// Clear the presentation offset immediately (no decay). Used on a teleport: the
    /// pawn snaps to the authoritative pose with no smoothed glide, so any in-flight
    /// offset from a prior small correction is discarded too.
    pub(crate) fn clear_presentation_offset(&mut self) {
        self.presentation_offset = Vec3::ZERO;
    }

    /// The current local-pawn presentation offset (decaying toward zero). The render
    /// seam adds it to the registry transform to produce the presented first-person
    /// pose; gameplay never reads it. `ZERO` when no correction is in flight.
    pub(crate) fn presentation_offset(&self) -> Vec3 {
        self.presentation_offset
    }

    /// Produce the presented local pose: the gameplay-authoritative `registry_pose`
    /// with the decaying correction offset added to its position. The
    /// presentation-pose accessor in `Transform` form — used wherever a full pose
    /// (not just the offset) is convenient, including the Task 6 latency harness
    /// asserting the presented pose lags the snapped registry pose. The `main.rs`
    /// render seam folds the same offset onto the interpolated *camera eye* via
    /// [`presentation_offset`](Self::presentation_offset) (it works with the
    /// interpolated position, not a registry Transform). Rotation/scale are not
    /// corrected (Phase 3 is movement-only; orientation comes from the live look
    /// input, not the reconciled snapshot). Staged until the harness lands.
    #[allow(dead_code)]
    pub(crate) fn present_local_pose(&self, registry_pose: Transform) -> Transform {
        Transform {
            position: registry_pose.position + self.presentation_offset,
            ..registry_pose
        }
    }

    /// Decay the presentation offset one render frame toward zero. Called at render
    /// rate (not tick rate): correction smoothing is presentation, decoupled from the
    /// fixed sim step. Geometric decay by [`DECAY_PER_FRAME`]; snaps to exactly zero
    /// once below [`OFFSET_SETTLED_EPSILON_M`] so the rendered eye fully converges
    /// rather than carrying a permanent sub-visible bias. No-op when already zero.
    pub(crate) fn decay_presentation_offset(&mut self) {
        if self.presentation_offset == Vec3::ZERO {
            return;
        }
        self.presentation_offset *= DECAY_PER_FRAME;
        if self.presentation_offset.length() < OFFSET_SETTLED_EPSILON_M {
            self.presentation_offset = Vec3::ZERO;
        }
    }
}

/// Movement-only replay: advance one `(Transform, PlayerMovementComponent)` pair
/// through a single `movement::tick` and return the new pair plus the tick's
/// `MovementEvents`. Side-effect-free and registry-blind by construction — it
/// takes OWNED state and has NO `EntityRegistry` parameter, so it provably cannot
/// run AI, weapons, the death sweep, or reactions (the guard the plan asks for is
/// the signature itself).
///
/// Shared by this module's forward prediction and Task 5's reconciliation replay.
/// The `position` `movement::tick` consumes/returns is the `Transform.position`;
/// this helper threads it through so callers only ever see the `Transform`.
pub(crate) fn replay(
    transform: Transform,
    mut movement: PlayerMovementComponent,
    input: MovementInput,
    collision: &CollisionWorld,
    gravity: f32,
    dt: f32,
) -> (Transform, PlayerMovementComponent, MovementEvents) {
    let (new_position, events) = movement::tick(
        &mut movement,
        &input,
        collision,
        gravity,
        dt,
        transform.position,
    );
    let mut new_transform = transform;
    new_transform.position = new_position;
    (new_transform, movement, events)
}

#[cfg(test)]
mod tests {
    use super::*;

    use glam::{Vec2, Vec3};
    use parry3d::math::{Isometry, Point};
    use parry3d::shape::TriMesh;

    use postretro_net::wire::{InputCommand, WireFireButtonState, WireMovementInput};

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

    fn start_transform() -> Transform {
        Transform {
            position: START,
            ..Transform::default()
        }
    }

    /// Forward command at the given monotonic client tick, dash optional.
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

    // --- Replay helper purity: it advances a Transform+movement pair through
    // movement::tick and returns events with NO registry in sight (the signature
    // is the guard — there is no EntityRegistry parameter to pass). ---
    #[test]
    fn replay_advances_pair_through_movement_tick_without_a_registry() {
        let world = floor_world();
        let input = MovementInput {
            wish_dir: Vec2::new(0.0, 1.0),
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
        };

        let (transform, movement, _events) =
            replay(start_transform(), component(), input, &world, GRAVITY, DT);

        // The pair advanced: forward locomotion moved the pawn along -Z (facing_yaw
        // 0 looks down -Z), and the movement component is the same owned value
        // returned, not a registry read.
        assert!(
            transform.position.z < START.z - EPSILON,
            "forward command should move the pawn along -Z; z={}",
            transform.position.z
        );
        // The substrate snapped the grounded pawn to the floor; the returned
        // component carries live tick state (grounded), proving the pair round-tripped
        // through movement::tick rather than being echoed back unchanged.
        assert!(
            movement.is_grounded,
            "a floored pawn is grounded after a tick"
        );
    }

    // --- Forward prediction records exactly one history entry per predicted tick,
    // with monotonic client_tick. ---
    #[test]
    fn predict_tick_records_one_entry_per_tick_with_monotonic_client_tick() {
        let world = floor_world();
        let mut prediction = ClientPrediction::new();
        prediction.arm(NetworkId(7), EntityId::from_raw(3));

        let prev = (start_transform(), component());
        for tick in 0..5u32 {
            let out = prediction.predict_tick(
                forward_command(tick, false),
                prev.clone(),
                &world,
                GRAVITY,
                DT,
            );
            assert!(out.is_some(), "armed prediction advances the pawn");
        }

        assert_eq!(
            prediction.history().len(),
            5,
            "one history entry is recorded per predicted tick"
        );
        // client_tick is monotonic non-decreasing across the ring.
        let ticks: Vec<u32> = prediction.history().iter().map(|e| e.client_tick).collect();
        assert_eq!(ticks, vec![0, 1, 2, 3, 4]);

        // The pawn actually advanced across the window (state is chained tick-to-tick,
        // not recomputed from prev each time).
        let last = prediction.history().back().unwrap();
        assert!(
            last.transform.position.z < START.z - EPSILON,
            "chained prediction moves the pawn forward across ticks"
        );
    }

    // --- Before-baseline inert: prediction drives nothing until armed. ---
    #[test]
    fn predict_tick_is_inert_until_armed() {
        let world = floor_world();
        let mut prediction = ClientPrediction::new();
        assert!(!prediction.is_armed());

        let prev = (start_transform(), component());
        let out =
            prediction.predict_tick(forward_command(0, false), prev.clone(), &world, GRAVITY, DT);
        assert!(out.is_none(), "unarmed prediction returns no driven state");
        assert!(
            prediction.history().is_empty(),
            "unarmed prediction records no history (no local pawn driven)"
        );

        // After arming, the same call drives the pawn and records history.
        prediction.arm(NetworkId(1), EntityId::from_raw(0));
        let out = prediction.predict_tick(forward_command(0, false), prev, &world, GRAVITY, DT);
        assert!(out.is_some(), "armed prediction drives the pawn");
        assert_eq!(prediction.history().len(), 1);
    }

    // --- Prune-through-ack drops only history at or below the acked client_tick. ---
    #[test]
    fn prune_through_ack_drops_only_entries_at_or_below_ack() {
        let world = floor_world();
        let mut prediction = ClientPrediction::new();
        prediction.arm(NetworkId(2), EntityId::from_raw(1));

        let prev = (start_transform(), component());
        for tick in 0..6u32 {
            prediction.predict_tick(
                forward_command(tick, false),
                prev.clone(),
                &world,
                GRAVITY,
                DT,
            );
        }
        assert_eq!(prediction.history().len(), 6);

        // Ack through tick 3: ticks 0..=3 drop, ticks 4 and 5 remain.
        prediction.prune_through_ack(3);
        let remaining: Vec<u32> = prediction.history().iter().map(|e| e.client_tick).collect();
        assert_eq!(
            remaining,
            vec![4, 5],
            "only ticks at/below the ack are pruned"
        );

        // An ack for a tick older than the ring head drops nothing.
        prediction.prune_through_ack(3);
        assert_eq!(prediction.history().len(), 2, "a stale ack is a no-op");

        // Acking beyond the ring clears it entirely.
        prediction.prune_through_ack(100);
        assert!(
            prediction.history().is_empty(),
            "an ack past the ring empties it"
        );
    }

    // --- Forward prediction never invokes full simulate_tick side effects. The
    // proof is structural: predict_tick advances state only through `replay`, whose
    // signature takes owned Transform/PlayerMovementComponent and a &CollisionWorld
    // with NO EntityRegistry — so AI, weapons, and the death sweep (all of which
    // require &mut EntityRegistry in simulate_tick) are unreachable from this path. ---
    #[test]
    fn forward_prediction_cannot_reach_registry_driven_systems() {
        let world = floor_world();
        let mut prediction = ClientPrediction::new();
        prediction.arm(NetworkId(9), EntityId::from_raw(4));

        // A dash command advances movement state to Dash WITHOUT any weapon/AI/death
        // system running — there is no registry to run them against. The recorded
        // tick flags the dash, the Phase 3 instrumentation Task 5 reads.
        let prev = (start_transform(), component());
        let out = prediction.predict_tick(forward_command(0, true), prev, &world, GRAVITY, DT);
        assert!(out.is_some());
        let entry = prediction.history().back().unwrap();
        assert!(
            entry.included_dash,
            "a dash command predicts a Dash state through the movement-only path"
        );
    }

    // --- Correction classification reads the pinned AC thresholds (no duplicated
    // magic numbers): the boundary cases derive their expectations from the consts. ---
    #[test]
    fn classify_correction_uses_pinned_thresholds() {
        // At/below the ordinary cap → Ordinary (dash flag irrelevant).
        assert_eq!(
            classify_correction(ORDINARY_CORRECTION_MAX_M, false),
            CorrectionClass::Ordinary
        );
        assert_eq!(
            classify_correction(ORDINARY_CORRECTION_MAX_M, true),
            CorrectionClass::Ordinary
        );
        // Above the ordinary cap, within the dash cap, on a dash tick → Dash.
        let dash_mid = (ORDINARY_CORRECTION_MAX_M + DASH_CORRECTION_MAX_M) * 0.5;
        assert_eq!(classify_correction(dash_mid, true), CorrectionClass::Dash);
        // Same magnitude on a non-dash tick → OversizedSmoothed (still smoothed).
        assert_eq!(
            classify_correction(dash_mid, false),
            CorrectionClass::OversizedSmoothed
        );
        // At/above the teleport floor → Teleport regardless of the dash flag.
        assert_eq!(
            classify_correction(TELEPORT_CORRECTION_MIN_M, true),
            CorrectionClass::Teleport
        );
        assert_eq!(
            classify_correction(TELEPORT_CORRECTION_MIN_M, false),
            CorrectionClass::Teleport
        );
        // Between the dash cap and the teleport floor on a dash tick → OversizedSmoothed.
        let between = (DASH_CORRECTION_MAX_M + TELEPORT_CORRECTION_MIN_M) * 0.5;
        assert_eq!(
            classify_correction(between, true),
            CorrectionClass::OversizedSmoothed
        );
    }

    // --- The presentation offset seeds, decays geometrically toward zero, and
    // settles exactly at zero (no permanent sub-visible bias). ---
    #[test]
    fn presentation_offset_seeds_decays_and_settles_to_zero() {
        let mut prediction = ClientPrediction::new();
        assert_eq!(prediction.presentation_offset(), Vec3::ZERO);

        prediction.seed_presentation_offset(Vec3::new(0.3, 0.0, -0.4));
        let seeded = prediction.presentation_offset().length();
        assert!(seeded > EPSILON, "a nonzero offset is seeded");

        // present_local_pose adds the offset to the registry pose's position.
        let pose = Transform {
            position: Vec3::new(1.0, 2.0, 3.0),
            ..Transform::default()
        };
        let presented = prediction.present_local_pose(pose);
        assert!(
            (presented.position - (pose.position + prediction.presentation_offset())).length()
                < EPSILON
        );

        // Each decay strictly shrinks it; it converges to exactly zero.
        let after_one = {
            prediction.decay_presentation_offset();
            prediction.presentation_offset().length()
        };
        assert!(after_one < seeded, "decay shrinks the offset");
        for _ in 0..64 {
            prediction.decay_presentation_offset();
        }
        assert_eq!(prediction.presentation_offset(), Vec3::ZERO);
        // Decaying a zero offset is a no-op.
        prediction.decay_presentation_offset();
        assert_eq!(prediction.presentation_offset(), Vec3::ZERO);
    }

    // --- clear_presentation_offset / clear_history wipe state for the teleport +
    // reset paths. ---
    #[test]
    fn clear_helpers_wipe_offset_and_history() {
        let world = floor_world();
        let mut prediction = ClientPrediction::new();
        prediction.arm(NetworkId(1), EntityId::from_raw(0));
        prediction.predict_tick(
            forward_command(0, false),
            (start_transform(), component()),
            &world,
            GRAVITY,
            DT,
        );
        prediction.seed_presentation_offset(Vec3::new(1.0, 0.0, 0.0));
        assert!(!prediction.history().is_empty());

        prediction.clear_history();
        assert!(
            prediction.history().is_empty(),
            "clear_history empties the ring"
        );
        prediction.clear_presentation_offset();
        assert_eq!(prediction.presentation_offset(), Vec3::ZERO);
    }

    // --- Re-arming to the same pawn preserves unacked history; re-arming to a
    // different pawn clears it. ---
    #[test]
    fn rearm_same_pawn_preserves_history_different_pawn_clears_it() {
        let world = floor_world();
        let mut prediction = ClientPrediction::new();
        prediction.arm(NetworkId(5), EntityId::from_raw(2));

        let prev = (start_transform(), component());
        prediction.predict_tick(forward_command(0, false), prev.clone(), &world, GRAVITY, DT);
        prediction.predict_tick(forward_command(1, false), prev.clone(), &world, GRAVITY, DT);
        assert_eq!(prediction.history().len(), 2);

        // A periodic re-baseline for the SAME pawn must not wipe unacked predictions.
        prediction.arm(NetworkId(5), EntityId::from_raw(2));
        assert_eq!(
            prediction.history().len(),
            2,
            "re-arming the same pawn preserves history"
        );

        // Arming a DIFFERENT pawn drops the stale ring.
        prediction.arm(NetworkId(6), EntityId::from_raw(8));
        assert!(
            prediction.history().is_empty(),
            "arming a new pawn clears the old pawn's history"
        );
    }
}
