// The frame-order contract between the client's replicated-state apply and the
// state-crossing detection that presents it: within one Game-logic phase, every
// snapshot received this frame is applied into the registry + slot table BEFORE
// crossing detection reads that slot table. A client that detected crossings first
// would present a replicated slot change one frame late (and, on a late join, would
// miss the accepting frame's baseline crossing entirely).
//
// The two stages sit ~400 lines apart in `App::window_event` — the apply stage runs
// in `net_poll_and_apply` before the catch-up tick loop, the crossing stage runs after
// game logic settles the frame's local slot writes — so the order cannot be expressed
// as one fused call without moving crossing detection ahead of the tick loop (a
// behavior change: client-local writes such as `screen.flash` decay must settle first).
// Instead the order is carried by a witness value: `run_snapshot_apply_stage` mints a
// `SnapshotsApplied`, and `run_crossing_stage` consumes it. `SnapshotsApplied` has a
// private field and no public constructor, so no caller outside this module can forge
// one — reaching the crossing stage before the apply stage is a type error, not a
// silent regression.
//
// See: context/lib/networking.md · context/lib/entity_model.md §5

/// Witness that this engine frame's received snapshots have already been applied.
///
/// Minted only by [`run_snapshot_apply_stage`] and consumed by [`run_crossing_stage`].
/// The `engine_frame` stamp closes the stale-witness hole: a witness held over from a
/// previous frame is not this frame's proof, and the crossing stage's debug assertion
/// trips on it (tests and the dev engine build both run with debug assertions on).
#[derive(Debug)]
#[must_use = "the crossing stage consumes this witness; dropping it skips crossing detection"]
pub(crate) struct SnapshotsApplied {
    engine_frame: u64,
}

/// The two ordered stages of a frame's replicated-state → presentation path.
///
/// Implemented by `App` (the production frame) and by the headless co-op harness, so
/// both drive the same production-owned stage order rather than each hand-sequencing
/// its own.
pub(crate) trait ReplicatedStateFrame {
    /// Poll the endpoint and apply every snapshot received this frame — entity records
    /// into the registry, replicated state-slot records into the slot table. Inert for
    /// single-player and for a host with no inbound snapshots.
    fn apply_received_snapshots(&mut self, frame_dt: f32);

    /// Detect this frame's state crossings over the settled slot table and dispatch the
    /// reactions they fire. Returns the crossing event names, in fire order.
    fn dispatch_state_crossings(&mut self) -> Vec<String>;
}

/// Run the frame's snapshot-apply stage and mint the witness the crossing stage needs.
pub(crate) fn run_snapshot_apply_stage<F>(
    frame: &mut F,
    engine_frame: u64,
    frame_dt: f32,
) -> SnapshotsApplied
where
    F: ReplicatedStateFrame + ?Sized,
{
    frame.apply_received_snapshots(frame_dt);
    SnapshotsApplied { engine_frame }
}

/// Run the frame's crossing-detection stage, consuming this frame's apply witness.
pub(crate) fn run_crossing_stage<F>(
    frame: &mut F,
    engine_frame: u64,
    applied: SnapshotsApplied,
) -> Vec<String>
where
    F: ReplicatedStateFrame + ?Sized,
{
    debug_assert_eq!(
        applied.engine_frame, engine_frame,
        "crossing detection must consume THIS frame's snapshot-apply witness; a stale \
         witness means the snapshot-apply stage did not run for this frame"
    );
    frame.dispatch_state_crossings()
}
