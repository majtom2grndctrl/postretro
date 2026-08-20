// E18 Task 7: Ordering rows that are genuinely tick/frame-timing-dependent and
// so need `SimHarness::frame` (Task 1) rather than a scheduler driven by hand.
// A child module of `determinism_tests` — see that file's trailing `mod`
// declaration for why (private `SimHarness`/`SimFixture` construction access).
//
// Owner map for this file: O13, O15, O25 — all via `SimHarness::frame`, using
// the `SimFixture::LevelLoadWait` fixture Task 1 added
// (`[note(presA), wait(ms), note(presB)]`, fired at install). Seeded at a fixed
// `1.0/60.0` DT (`super::DT`), matching the harness's own tick length.
//
// See: context/plans/in-progress/E18--timed-reaction-steps/index.md — Ordering
// scenarios O13, O15, O25.

use super::{SimFixture, SimHarness, SpawnOrder};

/// A neutral per-tick command: no movement, no fire, facing +Z. What every row
/// in this file cares about is the scheduler's landing timing, not movement —
/// the determinism proptest elsewhere already covers movement/AI/weapon
/// determinism at the tick seam.
fn neutral() -> super::RecordedCommand {
    super::RecordedCommand {
        wish_dir: glam::Vec2::ZERO,
        jump_pressed: false,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
        fire_pressed: false,
        fire_active: false,
    }
}

/// `ticks = max(1, ceil(durationMs * 1000 / 16_667))` — the Boundary
/// inventory's conversion rule, restated here so each row's duration is
/// traceable to a tick count without re-deriving it inline.
fn ticks_for(duration_ms: f64) -> u32 {
    let micros = duration_ms * 1000.0;
    (((micros / 16_667.0).ceil()) as u32).max(1)
}

// ---------------------------------------------------------------------------
// O13: multi-tick frame, short wait. `[note(presA), wait(17), note(presB)]`
// fired at install: presA runs in the install-time drain (before frame 1
// exists); the tail lands no earlier than frame 2's drain, so authored order
// across the wait always holds even though frame 2 delivers more ticks than
// the wait needs.
// ---------------------------------------------------------------------------
#[test]
fn multi_tick_frame_preserves_authored_order_across_a_short_wait() {
    assert_eq!(ticks_for(17.0), 2, "a 17ms wait is 2 ticks");
    let mut harness = SimHarness::new(
        SpawnOrder::AlphaThenBeta,
        SimFixture::LevelLoadWait { duration_ms: 17.0 },
    );
    assert_eq!(
        harness.note_log(),
        vec!["presA".to_string()],
        "presA already ran at install, before any frame"
    );

    // Frame 1: three ticks. The instance was enrolled at frame-counter 0, and
    // `frame()`'s `begin_frame()` runs at the END of the frame, so frame 1's
    // own ticks still see counter 0 — the enrollment frame is skipped in full,
    // however many ticks it delivers.
    harness.frame(&[neutral(), neutral(), neutral()]);
    assert_eq!(
        harness.note_log(),
        vec!["presA".to_string()],
        "no advance during the enrollment frame, regardless of its tick count"
    );

    // Frame 2: three ticks, a 2-tick wait — lands mid-frame, not needing a
    // third frame.
    harness.frame(&[neutral(), neutral(), neutral()]);
    assert_eq!(
        harness.note_log(),
        vec!["presA".to_string(), "presB".to_string()],
        "the tail lands at frame 2's drain; authored order (presA before presB) holds"
    );
}

// ---------------------------------------------------------------------------
// O15: max-ticks frame after a stall. A 2s stall clamps the accumulator to
// `MAX_ACCUMULATOR` (250ms), delivering at most `floor(250ms / 16_667us) = 14`
// ticks in one frame. A countdown may reach zero mid-frame under exactly that
// many ticks and still lands at that frame's drain — the wall-clock delay
// becomes lossy under the stall (authored ms plus stalled time), which is
// stated in the spec, not corrected here.
// ---------------------------------------------------------------------------
#[test]
fn max_stall_frame_lands_a_countdown_that_expires_on_its_last_tick() {
    assert_eq!(
        ticks_for(233.0),
        14,
        "a 233ms wait is exactly the 14-tick stall ceiling"
    );
    let mut harness = SimHarness::new(
        SpawnOrder::AlphaThenBeta,
        SimFixture::LevelLoadWait { duration_ms: 233.0 },
    );
    assert_eq!(harness.note_log(), vec!["presA".to_string()]);

    // Frame 1: skipped in full (enrollment frame), any tick count works —
    // use the same 14-tick ceiling to also prove a max-size frame is not
    // itself special-cased for the skip.
    let max_stall_ticks: Vec<super::RecordedCommand> = (0..14).map(|_| neutral()).collect();
    harness.frame(&max_stall_ticks);
    assert_eq!(harness.note_log(), vec!["presA".to_string()]);

    // Frame 2: a full 14-tick stall frame. The 14-tick countdown expires on
    // the LAST tick of this frame and still lands at ITS drain — not
    // stranded to a third frame.
    harness.frame(&max_stall_ticks);
    assert_eq!(
        harness.note_log(),
        vec!["presA".to_string(), "presB".to_string()],
        "a countdown that expires on a max-stall frame's last tick lands at that frame's drain"
    );
}

// ---------------------------------------------------------------------------
// O25: landing-order determinism. N instances enrolled at different times,
// under different origins/reaction addresses, landing on the same tick, drain
// in ascending `InstanceId` order — never `HashMap`/key iteration order — and
// two independent runs with identical inputs produce identical landing order
// and identical final `note_log` content.
// ---------------------------------------------------------------------------
#[test]
fn landing_order_is_deterministic_across_two_independent_runs() {
    fn run() -> Vec<String> {
        let mut harness = SimHarness::new(SpawnOrder::AlphaThenBeta, SimFixture::Determinism);
        // Enroll five instances directly (bypassing the control-arm dispatch,
        // exactly like `reaction_scheduler.rs`'s own unit tests), with names
        // that would sort differently by key than by enrollment order — "e"
        // enrolls first (lowest InstanceId) but sorts LAST by `InstanceKey`
        // (BTreeMap key order), and vice versa for "a". If landing order ever
        // used key order instead of ascending `InstanceId`, this would drain
        // "a".."e" instead of enrollment order.
        for name in ["e", "d", "c", "b", "a"] {
            harness.enroll_for_test(name, 0, None, 1);
        }
        // Frame 1 is skipped in full (enrolled at frame-counter 0, and
        // `begin_frame()` runs at frame()'s END — see O61/O12); frame 2's
        // single tick decrements the 1-tick countdown to zero and its drain
        // resumes all five in one landing batch.
        harness.frame(&[neutral()]);
        harness.frame(&[neutral()]);
        harness.landed_order_for_test()
    }

    let first = run();
    let second = run();
    assert_eq!(
        first,
        vec!["e", "d", "c", "b", "a"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        "landing order follows ascending InstanceId (enrollment order), not key order"
    );
    assert_eq!(
        first, second,
        "two independent runs with identical inputs produce an identical landing order"
    );
}

/// A zero-instance run is the boundary case the acceptance criterion names
/// explicitly ("identical across runs for all N including 0"): two empty runs
/// still agree, trivially, and the scheduler's landing queue drains to nothing
/// without panicking.
#[test]
fn zero_instances_land_identically_and_trivially() {
    fn run() -> Vec<String> {
        let mut harness = SimHarness::new(SpawnOrder::AlphaThenBeta, SimFixture::Determinism);
        harness.frame(&[neutral()]);
        harness.landed_order_for_test()
    }
    assert_eq!(run(), Vec::<String>::new());
    assert_eq!(run(), run());
}
