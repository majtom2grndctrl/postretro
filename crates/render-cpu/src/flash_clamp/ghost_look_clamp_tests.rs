// Ghost look clamp container: limiter off and capture, per-ghost history, and
// frames that never present.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use super::super::value::INTENSITY_RATE_PER_SECOND;
use super::*;
use crate::flash_limiter::HITCH_CEILING_SECONDS;

/// A ghost identity: link and generation-checked entity.
type Key = (u16, u32);

const GHOST: Key = (0, 7);
const WHITE: [f32; 3] = [1.0; 3];
const BLACK: [f32; 3] = [0.0; 3];
/// The dev map's `echo` style tint.
const ECHO: [f32; 3] = [0.6, 0.8, 1.0];

fn frame(dt: f32, enabled: bool) -> LimiterFrameUniform {
    pack_limiter_frame(
        LimiterFrameInput {
            elapsed_seconds: dt,
        },
        enabled,
        false,
    )
}

fn look(opacity: f32, tint: [f32; 3]) -> GhostInstanceLook {
    GhostInstanceLook {
        opacity,
        saturation: 0.5,
        tint,
    }
}

/// `a` for the first half of each period at `hz`, then `b`.
fn alternate<T>(hz: f32, t: f32, a: T, b: T) -> T {
    if (t * hz).fract() < 0.5 { a } else { b }
}

// The limiter turned off by the player, and a capture (no limiter input), pass
// every look through unchanged.
#[test]
fn limiter_off_and_capture_pass_the_tint_strobe_through() {
    let fps = 60.0;
    let dt = 1.0 / fps;
    let tints: Vec<_> = (0..120)
        .map(|n| alternate(10.0, n as f32 * dt, BLACK, WHITE))
        .collect();
    for limiter in [Some(frame(dt, false)), None] {
        let mut clamp = GhostLookClamp::default();
        for &tint in &tints {
            clamp.begin_frame(limiter.as_ref());
            let shown = clamp.present(GHOST, look(1.0, tint));
            clamp.end_frame();
            assert_eq!(shown, look(1.0, tint), "{limiter:?}");
        }
    }
}

// An opacity strobe at a steady tint spends the window but changes no tint, and
// a second ghost's history is its own.
#[test]
fn an_opacity_strobe_leaves_a_steady_tint_alone_and_each_ghost_keeps_its_own_history() {
    let fps = 60.0;
    let dt = 1.0 / fps;
    let frame = frame(dt, true);
    let other: Key = (1, 7);
    let mut clamp = GhostLookClamp::default();
    for n in 0..180 {
        let opacity = alternate(10.0, n as f32 * dt, 0.0, 1.0);
        clamp.begin_frame(Some(&frame));
        let opacity_strobe = clamp.present(GHOST, look(opacity, WHITE));
        let steady = clamp.present(other, look(0.5, ECHO));
        clamp.end_frame();
        assert_eq!(opacity_strobe.tint, WHITE, "frame {n}");
        assert_eq!(steady, look(0.5, ECHO), "frame {n}");
    }
}

// Regression: a frame whose surface acquire failed still spent the ghost
// clamp's presented time, so the first visible frame after a run of them
// stepped several rate caps at once.
#[test]
fn a_frame_that_never_presents_leaves_the_clamp_as_if_it_never_ran_and_merges_its_time() {
    let dt = 1.0 / 60.0;
    let step = frame(dt, true);
    let start = look(0.0, BLACK);
    let target = look(1.0, WHITE);
    let mut skipping = GhostLookClamp::default();
    let mut reference = GhostLookClamp::default();
    for clamp in [&mut skipping, &mut reference] {
        clamp.begin_frame(Some(&step));
        clamp.present(GHOST, start);
        clamp.end_frame();
        clamp.finish_presentation(true);
    }
    // Five frames that never present, then one that does. The reference never
    // saw the five: its next frame carries their time.
    let mut merged = dt;
    for _ in 0..5 {
        skipping.begin_frame(Some(&step));
        skipping.present(GHOST, target);
        skipping.end_frame();
        skipping.finish_presentation(false);
        merged += dt;
    }
    for n in 0..30 {
        let reference_frame = if n == 0 { frame(merged, true) } else { step };
        skipping.begin_frame(Some(&step));
        let shown = skipping.present(GHOST, target);
        skipping.end_frame();
        skipping.finish_presentation(true);
        reference.begin_frame(Some(&reference_frame));
        let expected = reference.present(GHOST, target);
        reference.end_frame();
        reference.finish_presentation(true);

        assert!((shown.opacity - expected.opacity).abs() < 1e-6, "frame {n}");
        for channel in 0..3 {
            assert!(
                (shown.tint[channel] - expected.tint[channel]).abs() < 1e-6,
                "frame {n}: {shown:?} vs {expected:?}"
            );
        }
        if n == 0 {
            // Six frames' time, hitch-clamped: one hitch ceiling of rate cap.
            let cap = INTENSITY_RATE_PER_SECOND * HITCH_CEILING_SECONDS;
            assert!((shown.opacity - cap).abs() < 1e-5, "{shown:?}");
        }
    }
}

// Regression: while acquires kept failing right after an install, each
// frame's snapshot cloned the never-allocated empty table, which freed the
// snapshot's own; the live table then allocated again on the next present.
// Both tables now start with room and keep it.
#[test]
fn frames_that_never_present_keep_both_tables_allocated() {
    let step = frame(1.0 / 60.0, true);
    let mut clamp = GhostLookClamp::<Key>::default();
    let room = (clamp.looks.capacity(), clamp.frame_start.capacity());
    assert!(room.0 > 0 && room.1 > 0, "{room:?}");
    for n in 0..30 {
        clamp.begin_frame(Some(&step));
        clamp.present(GHOST, look(0.5, WHITE));
        clamp.end_frame();
        clamp.finish_presentation(false);
        assert_eq!(
            (clamp.looks.capacity(), clamp.frame_start.capacity()),
            room,
            "frame {n}"
        );
    }
}
