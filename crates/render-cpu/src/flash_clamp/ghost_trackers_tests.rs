// Ghost edge trackers: edge points and threshold slack, counting with no dark
// test, carrying a count on, the onward guard, and the straight-line cap.
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
    assert_eq!(EDGE_THRESHOLD, 0.06875);
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
