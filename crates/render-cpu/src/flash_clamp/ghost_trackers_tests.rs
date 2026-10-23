// Ghost edge trackers: edge points, threshold slack, counting with no dark
// test, carrying a count on, points between samples, the onward guard,
// unknown history, the cap.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

use super::*;

/// A tiny xorshift generator: deterministic, uniform in [0, 1).
fn sequence(seed: u64) -> impl FnMut() -> f32 {
    let mut state = seed;
    move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32
    }
}

#[test]
fn edge_points_sample_both_outer_edges_of_the_albedo_and_background_square() {
    let points: Vec<(f32, f32)> = (0..EDGE_POINTS).map(edge_point).collect();
    for k in 0..=EDGE_SAMPLES {
        let at = k as f32 / EDGE_SAMPLES as f32;
        assert!(points.contains(&(1.0, at)), "a white ghost over {at}");
        assert!(points.contains(&(at, 1.0)), "albedo {at} over white");
    }
    for (i, point) in points.iter().enumerate() {
        assert!(!points[i + 1..].contains(point), "{point:?} twice");
    }
    // The flash threshold less half a sample step, less room for what the
    // draw rounds.
    assert_eq!(EDGE_THRESHOLD, 0.06875 - DRAWN_ROUNDING);
}

#[test]
fn neighbours_run_along_both_outer_edges_through_the_corner() {
    let along: Vec<usize> = (0..=EDGE_SAMPLES)
        .chain((EDGE_SAMPLES + 1..EDGE_POINTS).rev())
        .collect();
    for (i, &edge) in along.iter().enumerate() {
        let expected = [
            i.checked_sub(1).map(|i| along[i]),
            along.get(i + 1).copied(),
        ];
        assert_eq!(neighbours(edge), expected, "edge {edge}");
        // Neighbours differ by one sample step along a single axis.
        for neighbour in expected.into_iter().flatten() {
            let ((a, b), (c, d)) = (edge_point(edge), edge_point(neighbour));
            assert_eq!((a - c).abs() + (b - d).abs(), 1.0 / EDGE_SAMPLES as f32);
        }
    }
}

// Any (albedo, background) is a scale of an edge point, and a point between
// two edge samples draws within half a sample step of the nearer one, so a
// 0.1 swing anywhere is a swing of at least the edge threshold at some edge
// point. Random state pairs and random points.
#[test]
fn a_flash_anywhere_is_a_threshold_swing_at_an_edge_point() {
    let mut next = sequence(0x2545_F491_4F6C_DD1D);
    for _ in 0..20_000 {
        let from = Drawn::of(next(), next());
        let to = Drawn::of(next(), next());
        let (albedo, background) = (next(), next());
        let swing = to.at(albedo, background) - from.at(albedo, background);
        if swing.abs() < FLASH_LUMINANCE_THRESHOLD {
            continue;
        }
        let largest = (0..EDGE_POINTS)
            .map(|edge| {
                let (a, b) = edge_point(edge);
                (to.at(a, b) - from.at(a, b)).abs()
            })
            .fold(0.0, f32::max);
        assert!(
            largest >= EDGE_THRESHOLD - 1e-6,
            "a {swing} swing at ({albedo}, {background}) is {largest} at the edges"
        );
    }
}

// Regression: a dark-state test that assumed albedo 1 never counted a swing
// between near-white states, though at a lower albedo it draws a flash.
#[test]
fn an_edge_tracker_counts_a_near_white_swing() {
    let mut window = GhostWindow::default();
    let mut edges = EdgeTrackers::fresh(Drawn::of(1.0, 1.0));
    assert!(edges.step(
        Drawn::of(1.0, 0.92),
        SlotClass::Budgeted,
        true,
        0,
        &mut window
    ));
    assert_eq!(window.len(), 1, "1 → 0.92 over black counts");
}

// Carrying on a change, a tracker that counted it may not turn back, though
// the move is under the threshold and would count nothing.
#[test]
fn an_onward_tracker_may_carry_on_but_not_turn_back() {
    let mut window = GhostWindow::default();
    let mut edges = EdgeTrackers::fresh(Drawn::of(0.0, 1.0));
    assert!(edges.step(
        Drawn::of(0.2, 1.0),
        SlotClass::Budgeted,
        true,
        0,
        &mut window
    ));
    let counted = window.continuing();
    assert_ne!(counted, 0);
    let mut onward = edges;
    assert!(onward.step(
        Drawn::of(0.25, 1.0),
        SlotClass::Budgeted,
        false,
        counted,
        &mut window
    ));
    // A move back the size of rounding is not turning back.
    let mut rounded = edges;
    assert!(rounded.step(
        Drawn::of(0.2 - 1.0e-7, 1.0),
        SlotClass::Budgeted,
        false,
        counted,
        &mut window
    ));
    assert!(!edges.step(
        Drawn::of(0.19, 1.0),
        SlotClass::Budgeted,
        false,
        counted,
        &mut window
    ));
}

// A counted swing that reaches further carries its count on, counting again
// once its slot aged out; one that only holds still counts nothing.
#[test]
fn a_counted_swing_reaching_further_carries_its_count_on() {
    let mut window = GhostWindow::default();
    let mut edges = EdgeTrackers::fresh(Drawn::of(0.0, 1.0));
    assert!(edges.step(
        Drawn::of(0.101, 1.0),
        SlotClass::Budgeted,
        true,
        0,
        &mut window
    ));
    assert_eq!(window.len(), 1, "a white ghost fading in");
    window.age(1.0);
    assert!(window.is_empty());
    assert!(edges.step(
        Drawn::of(0.101, 1.0),
        SlotClass::Budgeted,
        false,
        0,
        &mut window
    ));
    assert!(window.is_empty(), "holding still counts nothing");
    // Reaching further crosses no new edge point's threshold, yet counts.
    let mut refused = edges;
    let mut scratch = window;
    assert!(!refused.step(
        Drawn::of(0.104, 1.0),
        SlotClass::Budgeted,
        false,
        0,
        &mut scratch
    ));
    assert!(edges.step(
        Drawn::of(0.104, 1.0),
        SlotClass::Budgeted,
        true,
        0,
        &mut window
    ));
    assert_eq!(window.len(), 1);
}

#[test]
fn the_straight_line_cap_scales_the_whole_step_in_the_flash_norm() {
    let from = Drawn::of(1.0, 0.4);
    // Opaque grey to clear white: over black p falls 0.4, over white q rises
    // 1, and a white ghost over white moves 0.6.
    let to = Drawn::of(0.0, 1.0);
    let scale = rate_cap_scale(from, to, 0.1).expect("over the cap");
    assert!((scale - 0.1).abs() < 1e-6, "{scale}");
    let lit = (to.white_over_black - from.white_over_black) * scale;
    let covered = (to.black_over_white - from.black_over_white) * scale;
    let largest = lit.abs().max(covered.abs()).max((lit + covered).abs());
    assert!((largest - 0.1).abs() < 1e-6);
    // Both parts scale together: the step stays on the straight line.
    assert!((lit / covered - (-0.4)).abs() < 1e-5);
    assert_eq!(rate_cap_scale(from, Drawn::of(0.95, 0.4), 0.1), None);
}

/// What a ghost draws, from p (a white ghost over black) and q (a black
/// ghost over white) directly.
fn drawn(white_over_black: f32, black_over_white: f32) -> Drawn {
    Drawn {
        white_over_black,
        black_over_white,
    }
}

// Regression: over the background 0.025, between the edge samples 0 and 1/16,
// one frame took the sample at 1/16 to its low and the next the sample at 0,
// and a third drew the point between lower than either while each sample sat
// short of its own low. No edge point reached further, so the slot counting
// the fall aged out from the second frame and the flash completed on the
// third went uncounted. A counted swing stopping short of its peak on a frame
// that may bring a point beside it further restarts its slot; a frame that
// repeats it does not.
#[test]
fn a_point_between_samples_reaching_further_than_either_keeps_the_count() {
    let mut window = GhostWindow::default();
    let mut edges = EdgeTrackers::fresh(drawn(0.4, 0.37));
    let low_above = drawn(0.3, 0.37);
    let low_below = drawn(0.2995, 0.39);
    let crossing = drawn(0.29952, 0.38);
    let between = |state: Drawn| state.at(1.0, 0.025);
    assert!(between(crossing) < between(low_above).min(between(low_below)));
    assert!(edges.step(low_above, SlotClass::Budgeted, true, 0, &mut window));
    assert_eq!(window.len(), 1, "the fall counts once");
    window.age(0.5);
    assert!(edges.step(low_below, SlotClass::Budgeted, true, 0, &mut window));
    assert_eq!(window.len(), 1);
    window.age(0.9);
    for edge in 0..EDGE_POINTS {
        let (albedo, background) = edge_point(edge);
        let tracker = edges.trackers[edge];
        let beyond = (crossing.at(albedo, background) - tracker.peak) * tracker.dir;
        assert!(beyond <= 0.0, "edge {edge} reaches further");
    }
    assert!(edges.step(crossing, SlotClass::Budgeted, false, 0, &mut window));
    window.age(0.5);
    assert_eq!(window.len(), 1, "the crossing frame restarted the slot");
    // Nothing moves on a frame repeating the look, so nothing restarts.
    for _ in 0..4 {
        assert!(edges.step(crossing, SlotClass::Budgeted, false, 0, &mut window));
        window.age(0.2);
    }
    assert!(window.is_empty());
}

// A counted fall that turns back by just over the edge threshold, then
// returns within half a step of its low, counts a fall again: a point beside
// it that never saw the small rise may reach a new low of the fall there. A
// frame repeating that look counts nothing more.
#[test]
fn a_small_turn_back_returning_near_its_extremum_counts_the_old_swing_again() {
    let mut window = GhostWindow::default();
    let mut edges = EdgeTrackers::fresh(drawn(0.5, 0.5));
    let steps = [
        (drawn(0.4, 0.5), 1, "the fall"),
        (drawn(0.4 + EDGE_THRESHOLD + 1e-3, 0.5), 2, "the small rise"),
        (drawn(0.41, 0.5), 3, "back near the low"),
        (drawn(0.41, 0.5), 3, "repeated"),
    ];
    for (state, slots, what) in steps {
        assert!(
            edges.step(state, SlotClass::Budgeted, true, 0, &mut window),
            "{what}"
        );
        assert_eq!(window.len(), slots, "{what}");
    }
    assert_eq!(edges.trackers[0].last_sign, -1, "counted as a fall");
}

// With unknown history an edge point counts its first move as a transition
// that way, however small, where a fresh one counts nothing; a point that has
// not moved counts nothing yet. From then on it runs as usual: carrying on its
// way restarts its slot, and holding still counts nothing.
#[test]
fn an_unknown_history_tracker_counts_its_first_move_of_any_size_then_runs_as_usual() {
    // A white ghost appearing faintly: over black it rises, at an albedo
    // under 1 over white it falls, and at the corner (1, 1) it draws 1 at
    // every opacity, so the corner never moves.
    let (corner, albedo_under_one) = (EDGE_SAMPLES, EDGE_POINTS - 1);
    let faint = Drawn::of(1e-4, 1.0);
    let mut fresh_window = GhostWindow::default();
    let mut fresh = EdgeTrackers::fresh(Drawn::of(0.0, 1.0));
    assert!(fresh.step(faint, SlotClass::Budgeted, true, 0, &mut fresh_window));
    assert!(
        fresh_window.is_empty(),
        "a fresh tracker counts no faint move"
    );

    let mut window = GhostWindow::default();
    let mut edges = EdgeTrackers::unknown_history(Drawn::of(0.0, 1.0));
    assert!(edges.step(faint, SlotClass::Budgeted, true, 0, &mut window));
    assert_eq!(window.len(), 1, "every first move counts, in one slot");
    assert_eq!(edges.trackers[0].last_sign, 1, "a rise over black");
    assert_eq!(edges.trackers[albedo_under_one].last_sign, -1, "a fall");
    assert_eq!(
        edges.trackers[corner].last_sign, UNKNOWN_HISTORY,
        "a point that has not moved counts nothing yet"
    );

    // Carrying on its way restarts the slot, a second from the carry-on.
    window.age(0.9);
    assert!(edges.step(
        Drawn::of(2e-4, 1.0),
        SlotClass::Budgeted,
        true,
        0,
        &mut window
    ));
    window.age(0.5);
    assert_eq!(window.len(), 1, "carrying on keeps the count");
    // Holding still counts nothing, so the slot ages out.
    window.age(0.5);
    assert!(edges.step(
        Drawn::of(2e-4, 1.0),
        SlotClass::Budgeted,
        true,
        0,
        &mut window
    ));
    assert!(window.is_empty(), "holding still counts nothing");

    // Either way: a faint fall from a presented look counts too.
    let mut window = GhostWindow::default();
    let mut edges = EdgeTrackers::unknown_history(Drawn::of(0.5, 1.0));
    assert!(edges.step(
        Drawn::of(0.4999, 1.0),
        SlotClass::Budgeted,
        true,
        0,
        &mut window
    ));
    assert_eq!(window.len(), 1, "a faint fall counts");
    assert_eq!(edges.trackers[0].last_sign, -1, "a fall over black");

    // A first move is a transition like any other: with no slot to join and
    // none to open, it does not fit.
    let mut window = GhostWindow::default();
    let mut edges = EdgeTrackers::unknown_history(Drawn::of(0.0, 1.0));
    assert!(!edges.step(faint, SlotClass::Budgeted, false, 0, &mut window));
}
