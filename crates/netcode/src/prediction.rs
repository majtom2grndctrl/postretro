// Client-side movement prediction state: the command-log ring for the local pawn
// (commands only — the registry is the pose chain), the armed
// `NetworkId -> EntityId` baseline, the forward-prediction tick, prune-through-ack,
// and the side-effect-free movement-only replay helper shared with reconciliation.
// See: context/lib/networking.md · context/lib/movement.md
//
// Boundary: the replay helper lives HERE (not in `sim/`) because production
// movement prediction is a netcode concern. Co-locating it with the prediction
// storage keeps the whole prediction seam in one place. It is the inverse of
// `sim/`'s `predict_reconcile.rs` dev harness: that one drives the FULL
// `simulate_tick` (AI/weapons/death) through two local sims; this is the
// movement-only production path that NEVER runs those systems. Its only registry
// read is the caller's teleporter armed-state closure (replicated armed state);
// it never writes the registry.

use std::collections::VecDeque;

use glam::Vec3;

use postretro_net::wire::{InputCommand, NetworkId};

use crate::movement::{self, MovementCollisionSource, MovementEvents, MovementInput};
use crate::netcode::wire_convert::input_command_to_sim_at_yaw;
use postretro_collision::EnvironmentView;
use postretro_entities::{EntityId, Transform};
use postretro_foundation::{MovementState, PlayerMovementComponent};

/// Upper bound on retained predicted-tick history. At 60 Hz this is one second of
/// unacked commands — well beyond any plausible host RTT for the loopback co-op
/// target. The ring drops its oldest entry when full so a stalled ack (lost
/// snapshots) can never grow the history unbounded.
const MAX_HISTORY: usize = 64;

/// Wrap-aware "is `a` at or before `b`" for the monotonic `client_tick` serial
/// number (`next_client_tick` allocates with `wrapping_add`, so the stream wraps at
/// `u32::MAX` ≈ 828 days at 60 Hz). Serial-number arithmetic (RFC 1982): the signed
/// `wrapping_sub` is negative-or-zero when `a` precedes-or-equals `b` within the
/// half-range window any plausible unacked tail occupies. A plain `a <= b` would
/// mis-order the one comparison that straddles the wrap; this is the single ordering
/// predicate the host stale-drop ([`crate::netcode::command_queue`]) and the client
/// [`ClientPrediction::prune_through_ack`] both route through so they agree across it.
pub(crate) fn client_tick_le(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) <= 0
}

// --- Correction classification thresholds ---
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

/// Wider-tolerance correction cap: a correction on a tick whose history entry crossed
/// dash, slide or vault (`included_dash`) tolerates a larger smoothed offset. All three
/// states can legitimately produce a bigger visible delta than ordinary locomotion.
pub(crate) const DASH_CORRECTION_MAX_M: f32 = 2.0;

/// Teleport threshold: a correction at or above this (in metres) is too large to
/// smooth without a visible rubber-band slide. It is snapped — history and the
/// presentation offset are cleared and the registry transform stamps prev == current
/// — so the pawn appears at the authoritative pose immediately. The same floor bounds
/// the composed presentation offset: a smoothed correction whose sum with the
/// in-flight remainder reaches it snaps the eye too (history kept).
pub(crate) const TELEPORT_CORRECTION_MIN_M: f32 = 3.0;

/// Fraction of the presentation offset retained per decay step as it decays to
/// zero. `offset *= DECAY_PER_FRAME` once per predicted fixed tick, after that tick's
/// camera pose has baked the offset in, so a smoothed correction converges over a
/// bounded handful of ticks (≈10 to fall below ~3% of its initial magnitude at 0.7)
/// — fast enough to feel responsive, slow enough to hide the snap. Smoothing is
/// presentation, not simulation: the registry never carries it.
const DECAY_PER_FRAME: f32 = 0.7;

/// Below this magnitude (metres) the presentation offset is treated as fully
/// converged and zeroed, so the decaying geometric tail does not leave a permanent
/// sub-visible bias on the rendered eye.
const OFFSET_SETTLED_EPSILON_M: f32 = 1e-4;

/// How a reconcile correction was classified. The classification drives whether
/// the registry-snapped correction is smoothed (seeded as a decaying presentation
/// offset) or snapped hard (teleport: offset cleared, prev == current stamped).
/// Returned by the reconcile entry point so the harness can assert the class
/// directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CorrectionClass {
    /// `<= ORDINARY_CORRECTION_MAX_M`: a small mispredict, smoothed.
    Ordinary,
    /// `<= DASH_CORRECTION_MAX_M` on an `included_dash` tick: a larger but expected
    /// wider-tolerance-state correction, smoothed.
    Dash,
    /// Snapped, not smoothed. Taken at any distance when the post-replay teleport
    /// record (generation, accumulated yaw or last teleporter) differs from the
    /// prediction: that identity change keeps the unacked history, since stamped
    /// rotation keeps its replay valid. A plain correction at or past
    /// `TELEPORT_CORRECTION_MIN_M` takes it too, and clears the history. So does a
    /// smoothed-class correction whose composition with the in-flight presentation
    /// offset reaches that floor; it keeps the history, since the reconcile itself
    /// was ordinary. Every way, reconcile reports a [`LocalTeleportAdoption`] for the
    /// app's camera hold.
    Teleport,
    /// In the gap between the smoothed caps and the teleport floor (or a dash-sized
    /// correction on a non-dash tick): smoothed like an ordinary correction, but the
    /// magnitude is logged as out-of-band. Kept distinct so the harness can see the
    /// engine took the seed-and-smooth path on a correction larger than the ordinary
    /// cap rather than silently widening `Ordinary`.
    OversizedSmoothed,
}

/// Classify a correction by its displacement `magnitude` (metres) and whether the
/// corrected tick crossed a wider-tolerance movement state. Pure: the single source of truth for the AC
/// thresholds, shared by the reconcile path and its tests.
///
/// - `>= TELEPORT_CORRECTION_MIN_M` → [`CorrectionClass::Teleport`] (checked first:
///   a teleport-sized delta is a teleport regardless of dash).
/// - `<= ORDINARY_CORRECTION_MAX_M` → [`CorrectionClass::Ordinary`].
/// - `<= DASH_CORRECTION_MAX_M` on a wider-tolerance tick → [`CorrectionClass::Dash`].
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

/// One logged command frame: the input sent to the host for a single predicted
/// fixed tick. The ring is a pure command log, NOT a state log — it stores no
/// predicted pose. Reconciliation matches `client_tick` against the host's
/// `last_processed_client_tick` ack, prunes the acked prefix, then replays the
/// remaining entries' commands forward from the authoritative pose through
/// [`replay`]. Forward prediction likewise chains from the registry's current pose
/// (the caller-supplied `prev`), never from a stored entry — so the reconciled
/// registry state is what the next predicted tick builds on (the invariant the
/// stored-pose log silently violated: a reconcile correction was overwritten by the
/// next prediction chained off the stale pre-reconcile pose).
//
// `client_tick`/`command`/`included_dash` are read by this module's tests and the
// reconciliation (`reconcile.rs`) that walks the history.
#[derive(Debug, Clone)]
pub(crate) struct PredictedTick {
    /// The monotonic client command-frame number this tick was predicted at.
    /// Stamped into the outbound `InputCommand.client_tick` and matched against
    /// the host ack during reconciliation.
    pub(crate) client_tick: u32,
    /// The exact command frame sent to the host for this tick. Retained so
    /// reconciliation can re-feed it to [`replay`] verbatim.
    pub(crate) command: InputCommand,
    /// Whether the movement state entered or stayed in a wider-tolerance state
    /// (`Dash`, `Sliding` or `Vault`) while predicting this tick, including a tick
    /// that started in `Vault` and left it. Reconciliation uses the wider dash-tier
    /// visual correction band for any of them.
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

/// App-facing presentation facts from a teleport-class own-pawn correction. A
/// fixed arrival point prevents cues from resolving a delayed render pose. A
/// composed-offset snap reports an eye cut only: `camera_cut` with no view turn,
/// no carry detach and no cue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalTeleportAdoption {
    pub entity_id: EntityId,
    /// View turn: post-replay accumulated teleport yaw minus the prediction's.
    pub yaw_delta: f32,
    /// The camera holds across the snap rather than blending to it.
    pub camera_cut: bool,
    /// The post-replay teleport record (generation, accumulated yaw or last
    /// teleporter) differs from the prediction: the pawn was relocated by a
    /// teleport the client mispredicted, so mover carry detaches. `false` for a
    /// plain correction at or past the teleport floor, which keeps its carry.
    pub identity_changed: bool,
    pub arrival: Vec3,
    /// The authoritative last teleporter whose cue plays once, for an adoption
    /// whose generation is ahead of the prediction.
    pub cue_teleporter: Option<u16>,
}

/// Client-side movement prediction state for the local pawn. Owns the outbound
/// command-log ring and the armed baseline identity. Long-lived prediction state
/// lives here, not on `App` — the source-layout gate keeps it out of the 6k-line
/// `main.rs` and the 1.4k-line `client.rs`.
///
/// Invariant — **registry is truth, history is a command log**: the ring stores only
/// the commands (no predicted pose). Both forward prediction and reconcile chain from
/// the registry's current pose, so a reconcile that snaps the registry to authority is
/// the pose the next prediction builds on. Reconciliation (restore-from-authority,
/// prune-through-ack, replay-the-rest, presentation-offset smoothing) consumes the
/// `history` and `replay` seams this module exposes.
#[derive(Debug, Default)]
pub struct ClientPrediction {
    first_record_pending: bool,
    pub(crate) teleport_adoptions: Vec<LocalTeleportAdoption>,
    /// Set when a reconcile wrote the pawn while its first record was still
    /// pending: the camera snaps to that authoritative eye rather than blending
    /// from the stale gated pose. A hold only — no cue, view turn or carry change.
    pub(crate) first_record_hold: Option<EntityId>,
    /// The armed local-pawn identity, or `None` until a `local_player` baseline
    /// has been applied AND mapped. Prediction does nothing while this is `None`.
    armed: Option<ArmedPawn>,
    /// Command-log ring, oldest-first. One entry per predicted fixed tick recording
    /// the outbound command (no pose — the registry holds the chained state);
    /// `client_tick` is monotonic non-decreasing across the deque (modulo the u32
    /// wrap). Pruned through the host ack and bounded to [`MAX_HISTORY`].
    history: VecDeque<PredictedTick>,
    /// The next monotonic command-frame number to stamp on an outbound
    /// `InputCommand`. Advances once per sent command — including the pre-baseline
    /// commands sent before prediction is armed — so the host sees one strictly
    /// increasing `client_tick` stream. Lives here (not on `App`) so the send-stamp
    /// allocator stays with the prediction state it feeds.
    next_client_tick: u32,
    /// Local-pawn correction smoothing. The decaying sum of each smoothed
    /// correction's difference between the pre-reconcile predicted pose and the
    /// reconciled authoritative pose, in world space. The registry transform always
    /// snaps to the authoritative result (gameplay/collision/AI read it); only the
    /// local first-person *presentation* adds this offset, decaying it to zero over a
    /// bounded number of ticks so the correction is continuous on screen without
    /// lying to the simulation.
    /// Owned here, never on `App`/`main.rs` — the source-layout gate.
    presentation_offset: Vec3,
    /// Off-by-default client input send-rate diagnostics (see `netdiag`). Inert
    /// unless `postretro::netdiag=debug`; observability only.
    send_diag: crate::netcode::netdiag::ClientSendDiag,
}

impl ClientPrediction {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Drop level-scoped prediction state after a connected-client level unload.
    /// The outbound command tick is connection-scoped and remains monotonic.
    pub(crate) fn reset_for_level_unload(&mut self) {
        self.first_record_pending = false;
        self.teleport_adoptions.clear();
        self.first_record_hold = None;
        self.armed = None;
        self.history.clear();
        self.presentation_offset = Vec3::ZERO;
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
        self.first_record_pending = true;
        self.teleport_adoptions.clear();
        self.first_record_hold = None;
        self.history.clear();
    }

    /// Whether the newly armed pawn's first record is still to be adopted.
    /// Arming sets it; only reconcile's first merged movement record clears it.
    pub(crate) fn first_record_pending(&self) -> bool {
        self.first_record_pending
    }

    pub(crate) fn mark_first_record_adopted(&mut self) {
        self.first_record_pending = false;
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

    pub(crate) fn peek_next_client_tick(&self) -> u32 {
        self.next_client_tick
    }

    /// Diagnostics: one per-fixed-tick predicted `Input` send (`client_predict_tick`).
    pub(crate) fn diag_record_predict_send(&mut self) {
        self.send_diag.record_predict_send();
    }

    /// Diagnostics: one same-frame zero-tick fire `Input` send
    /// (`client_send_input_command_in_registry`).
    pub(crate) fn diag_record_fire_send(&mut self) {
        self.send_diag.record_fire_path_send();
    }

    /// Whether prediction is armed and may drive the local pawn this frame. Before
    /// arming the client may still SEND input commands to the host, but it must not
    /// spawn or advance a provisional local pawn. Test-only; no production caller
    /// (the reconcile path calls `armed()` directly).
    #[allow(dead_code)]
    pub(crate) fn is_armed(&self) -> bool {
        self.armed.is_some()
    }

    /// Read-only view of the command-log ring (oldest-first). The reconcile path
    /// walks this to prune the acked prefix and replay the remaining commands.
    pub(crate) fn history(&self) -> &VecDeque<PredictedTick> {
        &self.history
    }

    /// Advance the local pawn one predicted fixed tick: run the movement-only
    /// [`replay_with_armed`] from `prev` through `command`, log the command in the history
    /// ring, and return the resulting `(Transform, PlayerMovementComponent)` for the
    /// caller to write back into the registry. Returns `None` (and records nothing)
    /// when prediction is not armed — the before-baseline inert contract.
    ///
    /// The starting state is ALWAYS `prev` — the registry's current applied state,
    /// which `client_predict_tick` reads fresh each tick. Because the caller writes
    /// the result straight back, the registry is the chain: tick N+1 reads what tick
    /// N wrote, and a reconcile that snapped the registry to the authoritative pose
    /// is the pose the very next prediction builds on (the history ring stores no
    /// pose to chain off, so a correction can no longer be silently overwritten).
    /// The caller owns reading `prev` and writing the result back; this never
    /// writes the registry or runs AI, weapons, death, or reactions.
    #[cfg(test)]
    pub(crate) fn predict_tick(
        &mut self,
        command: InputCommand,
        prev: (Transform, PlayerMovementComponent),
        collision: &impl MovementCollisionSource,
        environment: EnvironmentView<'_>,
        dt: f32,
    ) -> Option<(Transform, PlayerMovementComponent)> {
        self.predict_tick_with_events(command, prev, collision, environment, dt)
            .map(|(transform, movement, _)| (transform, movement))
    }

    /// [`Self::predict_tick`] keeping the tick's movement events. Test-only: it
    /// reads each teleporter's `enabled_on_spawn`, never replicated armed state.
    /// Production forward prediction (`client_predict_tick`) and reconcile replay
    /// supply the registry's bound armed state through the `_and_armed` forms.
    #[cfg(test)]
    pub(crate) fn predict_tick_with_events(
        &mut self,
        command: InputCommand,
        prev: (Transform, PlayerMovementComponent),
        collision: &impl MovementCollisionSource,
        environment: EnvironmentView<'_>,
        dt: f32,
    ) -> Option<(Transform, PlayerMovementComponent, MovementEvents)> {
        let world = collision.combined_collision();
        self.predict_tick_with_events_and_armed(
            command,
            prev,
            collision,
            environment,
            dt,
            |index| world.static_world.teleporters.records()[index].enabled_on_spawn,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn predict_tick_with_events_and_armed(
        &mut self,
        command: InputCommand,
        prev: (Transform, PlayerMovementComponent),
        collision: &impl MovementCollisionSource,
        environment: EnvironmentView<'_>,
        dt: f32,
        armed: impl FnMut(usize) -> bool,
    ) -> Option<(Transform, PlayerMovementComponent, MovementEvents)> {
        // Inert until armed: before the local_player baseline, drive no pawn.
        // Inert too until the pawn's first record has merged: a joiner whose
        // arming record could not merge (its tuning installed later) holds no
        // teleport record yet, and predicting from an empty one would fire an
        // arrival region the host has it latched in, then cut back. Reconcile
        // alone clears the flag, so that record still adopts silently.
        self.armed?;
        if self.first_record_pending {
            return None;
        }

        let (start_transform, start_movement) = prev;

        let sim = input_command_to_sim_at_yaw(&command, start_movement.teleport.accumulated_yaw);
        // The command's dash request and the movement states around this tick
        // together cover every wider-tolerance window: the request bit catches a
        // dash's entry tick (before the state has transitioned), the resulting
        // state catches an ongoing dash, slide or vault, and the start state
        // catches a vault's exit tick (see below). Reconcile reads the flag to
        // allow these states' larger visible correction during a rewound replay.
        let dash_requested = command.movement.dash_pressed;
        let started_in_vault = matches!(start_movement.movement_state, MovementState::Vault { .. });
        let (transform, movement, events) = replay_with_armed(
            start_transform,
            start_movement,
            sim.movement,
            collision,
            environment,
            dt,
            armed,
        );
        if events.teleport.is_some() {
            self.clear_presentation_offset();
        }

        // A vault's path follows movers and death, so it shares dash's wider
        // tolerance for a replay that lands it somewhere a little different. The
        // exit tick starts in `Vault` and ends in `Normal`, so the start state counts
        // too; otherwise the tick that lands the vault would miss the widened band.
        let included_dash = dash_requested
            || started_in_vault
            || matches!(
                movement.movement_state,
                MovementState::Dash { .. }
                    | MovementState::Sliding { .. }
                    | MovementState::Vault { .. }
            );

        if self.history.len() == MAX_HISTORY {
            // Drop the oldest unacked tick: a stalled ack must never grow history
            // unbounded. Reconciliation tolerates an ack older than the ring (it
            // simply finds no matching entry and trusts the authority).
            self.history.pop_front();
        }
        self.history.push_back(PredictedTick {
            client_tick: command.client_tick,
            command,
            included_dash,
        });

        Some((transform, movement, events))
    }

    /// Prune every history entry whose `client_tick` is at or below `acked_tick`.
    /// The host has resolved those commands authoritatively, so their predictions
    /// are settled and need not be replayed again. Entries *after* the ack remain
    /// for reconciliation to replay on top of the authoritative baseline.
    pub(crate) fn prune_through_ack(&mut self, acked_tick: u32) {
        while self
            .history
            .front()
            .is_some_and(|entry| client_tick_le(entry.client_tick, acked_tick))
        {
            self.history.pop_front();
        }
    }

    /// Clear all predicted history. Used by the reconcile path for the two cases that
    /// invalidate every retained prediction at once: an authoritative `None`-ack
    /// reset (the host has not resolved any of the client's commands, so the baseline
    /// supersedes the whole ring) and a teleport-floor-sized plain correction (the
    /// predicted trajectory diverged too far to replay onto). A teleport identity
    /// change keeps the ring.
    pub(crate) fn clear_history(&mut self) {
        self.history.clear();
    }

    /// Whether `included_dash` is set on any retained (unacked) history entry. The
    /// reconcile path reads this to classify the correction: a correction landing on
    /// a window that crossed a wider-tolerance state tolerates the larger [`DASH_CORRECTION_MAX_M`]
    /// smoothed cap. Checked over the post-prune ring (the unacked tail that was
    /// replayed onto the authoritative baseline).
    pub(crate) fn unacked_window_included_dash(&self) -> bool {
        self.history.iter().any(|entry| entry.included_dash)
    }

    /// Compose a fresh smoothed correction onto the local-pawn presentation offset.
    /// `correction` is `predicted_pose - reconciled_pose`: the registry just moved by
    /// `-correction`, so adding it to whatever remainder is still decaying keeps the
    /// presented eye (registry pose + offset) exactly where it was before this
    /// reconcile, and decaying the sum glides it to the authority. That holds for a
    /// correction landing before the previous one decayed, and for two in one frame.
    /// Smoothing only — the caller has already snapped the registry.
    ///
    /// Returns `true` when the composed offset reaches [`TELEPORT_CORRECTION_MIN_M`]:
    /// too far to glide, so the offset is cleared and the caller cuts the camera, the
    /// same rule a single teleport-floor correction follows. A teleport-class
    /// correction, a first-record adoption and a level unload clear the offset
    /// instead, so nothing composes across them.
    #[must_use = "a composed offset at the teleport floor snaps; the caller emits the camera cut"]
    pub(crate) fn seed_presentation_offset(&mut self, correction: Vec3) -> bool {
        let composed = self.presentation_offset + correction;
        if composed.length() >= TELEPORT_CORRECTION_MIN_M {
            self.presentation_offset = Vec3::ZERO;
            return true;
        }
        self.presentation_offset = composed;
        false
    }

    /// Clear the presentation offset immediately (no decay). Used on a teleport and a
    /// first-record adoption: the pawn snaps to the authoritative pose with no
    /// smoothed glide, so any in-flight offset from a prior small correction is
    /// discarded too, and the next correction composes from zero.
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
    /// (not just the offset) is convenient. The `main.rs` render seam folds the same
    /// offset onto the interpolated *camera eye* via
    /// [`presentation_offset`](Self::presentation_offset) (it works with the
    /// interpolated position, not a registry Transform). Rotation/scale are not
    /// corrected (Phase 3 is movement-only; orientation comes from the live look
    /// input, not the reconciled snapshot). Test-only presentation-pose accessor;
    /// no production caller.
    #[allow(dead_code)]
    pub(crate) fn present_local_pose(&self, registry_pose: Transform) -> Transform {
        Transform {
            position: registry_pose.position + self.presentation_offset,
            ..registry_pose
        }
    }

    /// Decay the presentation offset one step toward zero. Production calls it once
    /// per predicted fixed tick, after that tick's presented camera pose has baked
    /// the offset in (`client_decay_local_correction`); the render stage interpolates
    /// those presented poses. Geometric decay by [`DECAY_PER_FRAME`]; snaps to exactly zero
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

/// [`replay_with_armed`] reading each teleporter's `enabled_on_spawn`. Test-only:
/// production replays with the registry's bound, replicated armed state.
#[cfg(test)]
pub(crate) fn replay(
    transform: Transform,
    movement: PlayerMovementComponent,
    input: MovementInput,
    collision: &impl MovementCollisionSource,
    environment: EnvironmentView<'_>,
    dt: f32,
) -> (Transform, PlayerMovementComponent, MovementEvents) {
    let world = collision.combined_collision();
    replay_with_armed(
        transform,
        movement,
        input,
        collision,
        environment,
        dt,
        |index| world.static_world.teleporters.records()[index].enabled_on_spawn,
    )
}

/// Movement-only replay: advance one `(Transform, PlayerMovementComponent)` pair
/// through a single `movement::tick_with_immersion_and_armed` and return the new
/// pair plus the tick's `MovementEvents`. Side-effect-free — it takes OWNED state
/// and has NO `EntityRegistry` parameter, so it cannot run AI, weapons, the death
/// sweep, or reactions (the guard is the signature itself). Teleporter armed state
/// comes from `armed`, which production backs with the registry's replicated state.
///
/// Shared by forward prediction and reconciliation replay. The movement position
/// is the `Transform.position`; this helper threads it through so callers only
/// ever see the `Transform`. A tick that teleports writes the arrival rotation
/// (facing plus the teleport's yaw delta).
#[allow(clippy::too_many_arguments)]
pub(crate) fn replay_with_armed(
    transform: Transform,
    mut movement: PlayerMovementComponent,
    input: MovementInput,
    collision: &impl MovementCollisionSource,
    environment: EnvironmentView<'_>,
    dt: f32,
    armed: impl FnMut(usize) -> bool,
) -> (Transform, PlayerMovementComponent, MovementEvents) {
    let (new_position, events, _) = movement::tick_with_immersion_and_armed(
        &mut movement,
        &input,
        collision,
        environment,
        dt,
        transform.position,
        armed,
    );
    let mut new_transform = transform;
    new_transform.position = new_position;
    if input.facing_yaw.is_finite() {
        new_transform.rotation = glam::Quat::from_rotation_y(
            input.facing_yaw
                + events
                    .teleport
                    .as_ref()
                    .map_or(0.0, |event| event.yaw_delta),
        );
    }
    (new_transform, movement, events)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::collision::CollisionWorld;
    use glam::{Vec2, Vec3};

    use postretro_net::wire::{InputCommand, WireFireButtonState, WireMovementInput};

    use postretro_foundation::{
        AirParams, BoolOrIr, CapsuleParams, CrouchParams, DashParams, FallParams,
        ForgivenessParams, GroundParams, MovementStateKind, NumberOrIr, PlayerMovementDescriptor,
        SlideParams, SpeedParams,
    };

    const EPSILON: f32 = 1e-4;
    const DT: f32 = 1.0 / 60.0;
    const GRAVITY: f32 = -20.0;
    const START: Vec3 = Vec3::new(0.0, 1.21, 0.0);

    fn floor_world() -> CollisionWorld {
        let points = vec![
            Vec3::new(-500.0, 0.0, -500.0),
            Vec3::new(500.0, 0.0, -500.0),
            Vec3::new(500.0, 0.0, 500.0),
            Vec3::new(-500.0, 0.0, 500.0),
        ];
        let triangles = vec![[0, 2, 1], [0, 3, 2]];
        CollisionWorld::from_triangles_for_test(points, triangles)
    }

    fn descriptor() -> PlayerMovementDescriptor {
        PlayerMovementDescriptor {
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
            slide: None,
            swim: None,
            vault: None,
            view_feel: None,
        }
    }

    fn component() -> PlayerMovementComponent {
        PlayerMovementComponent::from_descriptor(&descriptor())
    }

    fn sliding_component() -> PlayerMovementComponent {
        let mut descriptor = descriptor();
        descriptor.crouch = Some(CrouchParams {
            half_height: 0.4,
            eye_height: 0.2,
            transition_rate: 15.0,
        });
        descriptor.slide = Some(SlideParams {
            min_speed: 8.0,
            slide_drag: 2.0,
            slope_assist: 1.0,
            steer_rate: 180.0,
            entry_boost: 1.0,
            min_duration_ms: 100.0,
        });
        let mut component = PlayerMovementComponent::from_descriptor(&descriptor);
        component.set_grounded(true);
        component.velocity = Vec3::new(10.0, 0.0, 0.0);
        component.movement_state = MovementState::Sliding {
            elapsed_ms: 0.0,
            boost: Vec3::new(10.0, 0.0, 0.0),
            eye_current: component.capsule.eye_height,
        };
        component.last_floor_normal = Some(Vec3::Y);
        component
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
            teleport_yaw_stamp: 0.0,
            secondary_button: postretro_net::wire::WireFireButtonState {
                pressed: false,
                active: false,
            },
            activation: postretro_net::wire::WireActivationInput::default(),
            client_tick,
            movement: WireMovementInput {
                wish_dir: [0.0, 1.0],
                jump_pressed: false,
                dash_pressed,
                running: true,
                crouch_intent: false,
                facing_yaw: 0.0,
                use_pressed: false,
                drop_pressed: false,
                aim_pitch: 0.0,
                firing_slot: 0,
            },
            fire_button: WireFireButtonState {
                pressed: false,
                active: false,
            },
            reload: false,
        }
    }

    #[test]
    fn peek_next_client_tick_does_not_allocate_or_depend_on_history() {
        let mut prediction = ClientPrediction::new();
        assert_eq!(prediction.peek_next_client_tick(), 0);

        assert_eq!(prediction.next_client_tick(), 0);
        assert_eq!(prediction.peek_next_client_tick(), 1);

        prediction.clear_history();
        assert_eq!(
            prediction.peek_next_client_tick(),
            1,
            "current-frame tick selection must not depend on retained history"
        );
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
            facing_yaw: 0.65,
            facing_pitch: 0.0,
            use_pressed: false,
            drop_pressed: false,
        };

        let (transform, movement, _events) = replay(
            start_transform(),
            component(),
            input,
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            DT,
        );

        // The pair advanced: forward locomotion retained a negative-Z component at
        // this yaw, and the movement component is the same owned value
        // returned, not a registry read.
        assert!(
            transform.position.z < START.z,
            "forward command should move the pawn along -Z; z={}",
            transform.position.z
        );
        let (yaw, _, _) = transform.rotation.to_euler(glam::EulerRot::YXZ);
        assert!(
            (yaw - 0.65).abs() <= EPSILON,
            "predicted Transform retains the local camera yaw for presentation"
        );
        // The substrate snapped the grounded pawn to the floor; the returned
        // component carries live tick state (grounded), proving the pair round-tripped
        // through movement::tick rather than being echoed back unchanged.
        assert!(
            movement.is_grounded(),
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
        prediction.mark_first_record_adopted();

        // Chain `prev` forward each tick exactly as `client_predict_tick` does
        // (read current state, predict, write back). The registry — here the local
        // `prev` — is the chain; the history ring is now just a command log.
        let mut prev = (start_transform(), component());
        for tick in 0..5u32 {
            let out = prediction.predict_tick(
                forward_command(tick, false),
                prev.clone(),
                &world,
                postretro_collision::EnvironmentView::level_only(GRAVITY),
                DT,
            );
            let (t, m) = out.expect("armed prediction advances the pawn");
            prev = (t, m);
        }

        assert_eq!(
            prediction.history().len(),
            5,
            "one history entry is recorded per predicted tick"
        );
        // client_tick is monotonic non-decreasing across the ring.
        let ticks: Vec<u32> = prediction.history().iter().map(|e| e.client_tick).collect();
        assert_eq!(ticks, vec![0, 1, 2, 3, 4]);

        // The pawn actually advanced across the window (the chained pose moved
        // forward, not the per-tick recompute-from-START a stale chain would give).
        assert!(
            prev.0.position.z < START.z - EPSILON,
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
        let out = prediction.predict_tick(
            forward_command(0, false),
            prev.clone(),
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            DT,
        );
        assert!(out.is_none(), "unarmed prediction returns no driven state");
        assert!(
            prediction.history().is_empty(),
            "unarmed prediction records no history (no local pawn driven)"
        );

        // Armed, it stays inert until the pawn's first record has merged: the
        // client holds no authoritative teleport record to predict from yet.
        prediction.arm(NetworkId(1), EntityId::from_raw(0));
        let out = prediction.predict_tick(
            forward_command(0, false),
            prev.clone(),
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            DT,
        );
        assert!(out.is_none(), "no prediction before the first record");
        assert!(prediction.history().is_empty());

        // Once it has, the same call drives the pawn and records history.
        prediction.mark_first_record_adopted();
        let out = prediction.predict_tick(
            forward_command(0, false),
            prev,
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            DT,
        );
        assert!(out.is_some(), "armed prediction drives the pawn");
        assert_eq!(prediction.history().len(), 1);
    }

    // --- Prune-through-ack drops only history at or below the acked client_tick. ---
    #[test]
    fn prune_through_ack_drops_only_entries_at_or_below_ack() {
        let world = floor_world();
        let mut prediction = ClientPrediction::new();
        prediction.arm(NetworkId(2), EntityId::from_raw(1));
        prediction.mark_first_record_adopted();

        let prev = (start_transform(), component());
        for tick in 0..6u32 {
            prediction.predict_tick(
                forward_command(tick, false),
                prev.clone(),
                &world,
                postretro_collision::EnvironmentView::level_only(GRAVITY),
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
        prediction.mark_first_record_adopted();

        // A dash command advances movement state to Dash WITHOUT any weapon/AI/death
        // system running — there is no registry to run them against. The recorded
        // tick flags the dash, the Phase 3 instrumentation Task 5 reads.
        let mut initial_movement = component();
        initial_movement.set_grounded(true);
        let prev = (start_transform(), initial_movement);
        let (_, _, events) = prediction
            .predict_tick_with_events(
                forward_command(0, true),
                prev,
                &world,
                postretro_collision::EnvironmentView::level_only(GRAVITY),
                DT,
            )
            .expect("armed prediction advances the local pawn");
        assert_eq!(
            events.state_edges,
            vec![crate::movement::MovementStateEdge {
                from: MovementStateKind::Normal,
                to: MovementStateKind::Dash,
            }],
            "forward prediction retains its local Dash entry edge"
        );
        let entry = prediction.history().back().unwrap();
        assert!(
            entry.included_dash,
            "a dash command predicts a Dash state through the movement-only path"
        );
    }

    #[test]
    fn predicted_sliding_tick_uses_the_dash_tier_correction_band() {
        let world = floor_world();
        let mut prediction = ClientPrediction::new();
        prediction.arm(NetworkId(10), EntityId::from_raw(5));
        prediction.mark_first_record_adopted();

        let output = prediction.predict_tick(
            forward_command(0, false),
            (start_transform(), sliding_component()),
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            DT,
        );
        assert!(output.is_some(), "armed sliding pawn predicts a tick");
        assert!(
            prediction.history().back().unwrap().included_dash,
            "Sliding must mark the boost-carrying correction band"
        );
        assert!(
            prediction.unacked_window_included_dash(),
            "a retained Sliding tick earns dash-tier correction smoothing"
        );
    }

    // The vault's exit tick starts in `Vault` and ends in `Normal`; it must still
    // take the widened correction tolerance. A component with no `vault` block aborts
    // a carried vault to `Normal` within the tick, giving that start/end shape.
    #[test]
    fn a_tick_that_starts_in_vault_and_ends_normal_uses_the_wider_correction_band() {
        let world = floor_world();
        let mut prediction = ClientPrediction::new();
        prediction.arm(NetworkId(11), EntityId::from_raw(6));
        prediction.mark_first_record_adopted();

        let mut vaulting = component();
        vaulting.movement_state = MovementState::Vault {
            elapsed_ms: 0.0,
            target: Vec3::new(0.5, 1.0, 0.0),
            start_offset: Vec3::new(-0.9, -1.0, 0.0),
            mover: None,
            entry_speed: 3.0,
        };

        let (_, after, _) = prediction
            .predict_tick_with_events(
                forward_command(0, false),
                (start_transform(), vaulting),
                &world,
                postretro_collision::EnvironmentView::level_only(GRAVITY),
                DT,
            )
            .expect("armed vaulting pawn predicts a tick");
        assert!(
            !matches!(after.movement_state, MovementState::Vault { .. }),
            "the tick must end outside Vault for this to pin the exit-tick shape"
        );
        assert!(
            prediction.history().back().unwrap().included_dash,
            "the vault exit tick must take the wider correction band"
        );
        assert!(prediction.unacked_window_included_dash());
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

        assert!(!prediction.seed_presentation_offset(Vec3::new(0.3, 0.0, -0.4)));
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
        prediction.mark_first_record_adopted();
        prediction.predict_tick(
            forward_command(0, false),
            (start_transform(), component()),
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            DT,
        );
        assert!(!prediction.seed_presentation_offset(Vec3::new(1.0, 0.0, 0.0)));
        assert!(!prediction.history().is_empty());

        prediction.clear_history();
        assert!(
            prediction.history().is_empty(),
            "clear_history empties the ring"
        );
        prediction.clear_presentation_offset();
        assert_eq!(prediction.presentation_offset(), Vec3::ZERO);
    }

    // --- A level unload (or demotion, which shares the reset) clears an in-flight
    // offset, and the next correction starts from zero: nothing composes across it. ---
    #[test]
    fn level_unload_clears_in_flight_offset_and_next_correction_starts_from_zero() {
        let mut prediction = ClientPrediction::new();
        let before_unload = Vec3::new(0.4, 0.0, -0.2);
        assert!(!prediction.seed_presentation_offset(before_unload));
        prediction.decay_presentation_offset();
        assert!(
            prediction.presentation_offset().length() > EPSILON,
            "an offset is in flight at the unload"
        );

        prediction.reset_for_level_unload();
        assert_eq!(prediction.presentation_offset(), Vec3::ZERO);

        let after_unload = Vec3::new(0.0, 0.0, 0.3);
        assert!(!prediction.seed_presentation_offset(after_unload));
        assert!(
            (prediction.presentation_offset() - after_unload).length() < EPSILON,
            "the next correction does not compose with the pre-unload remainder: {:?}",
            prediction.presentation_offset()
        );
    }

    // --- Re-arming to the same pawn preserves unacked history; re-arming to a
    // different pawn clears it. ---
    #[test]
    fn rearm_same_pawn_preserves_history_different_pawn_clears_it() {
        let world = floor_world();
        let mut prediction = ClientPrediction::new();
        prediction.arm(NetworkId(5), EntityId::from_raw(2));
        prediction.mark_first_record_adopted();

        let prev = (start_transform(), component());
        prediction.predict_tick(
            forward_command(0, false),
            prev.clone(),
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            DT,
        );
        prediction.predict_tick(
            forward_command(1, false),
            prev.clone(),
            &world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            DT,
        );
        assert_eq!(prediction.history().len(), 2);

        // A periodic re-baseline for the SAME pawn must not wipe unacked predictions.
        prediction.arm(NetworkId(5), EntityId::from_raw(2));
        prediction.mark_first_record_adopted();
        assert_eq!(
            prediction.history().len(),
            2,
            "re-arming the same pawn preserves history"
        );

        // Arming a DIFFERENT pawn drops the stale ring.
        prediction.arm(NetworkId(6), EntityId::from_raw(8));
        prediction.mark_first_record_adopted();
        assert!(
            prediction.history().is_empty(),
            "arming a new pawn clears the old pawn's history"
        );
    }

    // --- The client_tick ordering predicate is wrap-aware: it agrees with plain
    // `<=` away from the wrap and orders the comparison that straddles u32::MAX
    // correctly (serial-number arithmetic), which a plain `<=` gets wrong. ---
    #[test]
    fn client_tick_le_orders_correctly_across_the_u32_wrap() {
        // Ordinary range: matches plain `<=`.
        assert!(client_tick_le(3, 5));
        assert!(client_tick_le(5, 5));
        assert!(!client_tick_le(6, 5));

        // Across the wrap: tick u32::MAX-1 precedes tick 2 (the stream wrapped). A
        // plain `(u32::MAX - 1) <= 2` is false; the wrap-aware predicate is true.
        let before = u32::MAX - 1;
        let after = 2u32; // a few ticks later, after the wrap
        assert!(
            client_tick_le(before, after),
            "a tick just before the wrap precedes one just after it"
        );
        assert!(
            !client_tick_le(after, before),
            "the post-wrap tick does not precede the pre-wrap one"
        );
    }

    // Regression: across the u32 client_tick wrap the prune used a plain `<=`, which
    // mis-ordered the straddling comparison — the unacked tail was mis-pruned (either
    // wiped or never dropped). prune_through_ack now routes through client_tick_le.
    #[test]
    fn prune_through_ack_is_wrap_aware_at_the_u32_boundary() {
        let world = floor_world();
        let mut prediction = ClientPrediction::new();
        prediction.arm(NetworkId(11), EntityId::from_raw(5));
        prediction.mark_first_record_adopted();

        // Log commands straddling the wrap: u32::MAX-1, u32::MAX, 0, 1.
        let ticks = [u32::MAX - 1, u32::MAX, 0, 1];
        let mut prev = (start_transform(), component());
        for &tick in &ticks {
            let (t, m) = prediction
                .predict_tick(
                    forward_command(tick, false),
                    prev.clone(),
                    &world,
                    postretro_collision::EnvironmentView::level_only(GRAVITY),
                    DT,
                )
                .unwrap();
            prev = (t, m);
        }
        assert_eq!(prediction.history().len(), 4);

        // Ack through u32::MAX: the two pre-wrap entries drop, the post-wrap 0 and 1
        // remain. A plain `<=` would have kept 0 and 1 ahead of u32::MAX incorrectly
        // pruned (0 <= u32::MAX is true), wiping the live tail.
        prediction.prune_through_ack(u32::MAX);
        let remaining: Vec<u32> = prediction.history().iter().map(|e| e.client_tick).collect();
        assert_eq!(
            remaining,
            vec![0, 1],
            "only the pre-wrap prefix is pruned; the post-wrap tail survives"
        );
    }

    /// Quads for the replay course along +X, each facing `normal`.
    fn course_world() -> CollisionWorld {
        let mut points = Vec::new();
        let mut tris = Vec::new();
        let mut quad = |corners: [Vec3; 4], normal: Vec3| {
            let base = points.len() as u32;
            points.extend(corners);
            let face = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
            if face.dot(normal) >= 0.0 {
                tris.extend([[base, base + 1, base + 2], [base, base + 2, base + 3]]);
            } else {
                tris.extend([[base, base + 2, base + 1], [base, base + 3, base + 2]]);
            }
        };
        let (z0, z1) = (-20.0, 20.0);
        let floor = |x0: f32, x1: f32, y: f32| {
            [
                Vec3::new(x0, y, z0),
                Vec3::new(x1, y, z0),
                Vec3::new(x1, y, z1),
                Vec3::new(x0, y, z1),
            ]
        };
        let riser = |x: f32, y0: f32, y1: f32| {
            [
                Vec3::new(x, y0, z0),
                Vec3::new(x, y1, z0),
                Vec3::new(x, y1, z1),
                Vec3::new(x, y0, z1),
            ]
        };
        // Floor, a step up at x = 3, a vault ledge at x = 8 whose lip is a
        // convex edge, its top, and a drop back down a step at x = 12.
        quad(floor(-20.0, 3.0, 0.0), Vec3::Y);
        quad(riser(3.0, 0.0, 0.25), Vec3::NEG_X);
        quad(floor(3.0, 8.0, 0.25), Vec3::Y);
        quad(riser(8.0, 0.25, 1.25), Vec3::NEG_X);
        quad(floor(8.0, 12.0, 1.25), Vec3::Y);
        quad(riser(12.0, 1.0, 1.25), Vec3::X);
        quad(floor(12.0, 40.0, 1.0), Vec3::Y);
        // A wall along the course at z = -0.45, against the capsule's side.
        quad(
            [
                Vec3::new(-20.0, 0.0, -0.45),
                Vec3::new(40.0, 0.0, -0.45),
                Vec3::new(40.0, 4.0, -0.45),
                Vec3::new(-20.0, 4.0, -0.45),
            ],
            Vec3::Z,
        );
        CollisionWorld::from_triangles_for_test(points, tris)
    }

    // Host movement and the client predictor run the same pure tick: replaying
    // the recorded commands reproduces the host's poses bit for bit, through a
    // step-up, a vault across a convex lip under held then neutral input, and a
    // ground-stick drop beside a wall.
    #[test]
    fn predictor_replay_reproduces_host_poses_exactly_over_a_vault_step_up_and_ground_stick() {
        use crate::sim::run_host_movement_tick;
        use postretro_entities::EntityRegistry;
        use postretro_foundation::{VaultEntry, VaultParams, VaultTrigger};

        let mut desc = descriptor();
        desc.vault = Some(VaultParams {
            max_height: 1.6,
            swim_max_height: 0.9,
            duration_ms: 320.0,
            from: VaultEntry {
                ground: Some(VaultTrigger::Auto),
                ..VaultEntry::default()
            },
            allow_fire: false,
        });
        let world = course_world();
        let environment = postretro_collision::EnvironmentView::level_only(GRAVITY);
        let start = Transform {
            position: Vec3::new(0.0, 1.22, -0.03),
            ..Transform::default()
        };
        let pawn_component = PlayerMovementComponent::from_descriptor(&desc);

        // Forward along +X (yaw -pi/2), walking: held into the ledge until the
        // vault exits (tick 93), neutral just after, then on over the drop.
        const NEUTRAL: std::ops::Range<u32> = 95..123;
        let mut commands = Vec::new();
        for tick in 0..240u32 {
            let mut command = forward_command(tick, false);
            command.movement.facing_yaw = -std::f32::consts::FRAC_PI_2;
            command.movement.running = false;
            if NEUTRAL.contains(&tick) {
                command.movement.wish_dir = [0.0, 0.0];
            }
            commands.push(command);
        }

        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(start);
        registry
            .set_component(pawn, pawn_component.clone())
            .unwrap();
        let mut host = Vec::new();
        for command in &commands {
            let yaw = registry
                .get_component::<PlayerMovementComponent>(pawn)
                .unwrap()
                .teleport
                .accumulated_yaw;
            let input = input_command_to_sim_at_yaw(command, yaw).movement;
            run_host_movement_tick(&mut registry, &world, environment, &[(pawn, input)], DT);
            host.push(registry.get_component::<Transform>(pawn).unwrap().position);
        }

        let mut prediction = ClientPrediction::new();
        prediction.arm(NetworkId(7), pawn);
        prediction.mark_first_record_adopted();
        let mut prev = (start, pawn_component);
        let mut states = Vec::new();
        for (command, host_position) in commands.iter().zip(&host) {
            prev = prediction
                .predict_tick(*command, prev, &world, environment, DT)
                .expect("armed prediction advances the pawn");
            assert_eq!(
                prev.0.position.to_array(),
                host_position.to_array(),
                "client tick {}: the replay diverged from the host",
                command.client_tick
            );
            states.push(prev.1.movement_state.kind());
        }
        let vault_ticks: Vec<usize> = (0..states.len())
            .filter(|&tick| states[tick] == MovementStateKind::Vault)
            .collect();
        assert!(
            vault_ticks
                .last()
                .is_some_and(|&last| last + 1 < NEUTRAL.start as usize),
            "the course vaults under held input and exits before the neutral window"
        );
        let end = host.last().unwrap();
        assert!(
            end.x > 13.0 && (end.y - 2.22).abs() < 1.0e-3,
            "the course ends past the drop, at {end:?}"
        );
    }
}
