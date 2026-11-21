// Smoothed routes keep to the walk's cells, run speed and turn cap.
// See: context/lib/testing_guide.md

use glam::Vec3;
use postretro_level_loader::LevelWorld;

use crate::test_fixtures::{portalless_world, smoothed_world, solid_world};
use crate::{
    DEFAULT_STEPS, FRAME_SECONDS, RouteError, RoutePose, SmoothedRoute, Smoothing, WALK_SEED, Walk,
    WalkKind, smoothed_route_seeded,
};

const STEPS: usize = 60;
const PITCH: f32 = -0.125;
const BRISK: Smoothing = Smoothing {
    speed: 8.0,
    turn_cap_degrees: 270.0,
};
const STROLL: Smoothing = Smoothing {
    speed: 5.0,
    turn_cap_degrees: 120.0,
};

/// What a route measured, beside the properties `check_route` asserted.
#[derive(Debug)]
struct Measured {
    max_stride: f32,
    max_turn: f32,
    /// Consecutive non-cut pairs, and those moving a full stride.
    pairs: usize,
    full_stride: usize,
    /// Pairs on a straight run at a steady yaw, and those short of a stride.
    straight: usize,
    straight_short: usize,
}

fn wrap(angle: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    angle - TAU * ((angle + PI) / TAU).floor()
}

/// The walk's cells with consecutive repeats removed.
fn collapsed(cells: impl IntoIterator<Item = u32>) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for cell in cells {
        if out.last() != Some(&cell) {
            out.push(cell);
        }
    }
    out
}

/// Asserts AC 3 on `route` over `walk`: cells in walk order, every eye in the
/// current or next walk cell, cuts only where counted, and between
/// consecutive poses no more than a stride of travel or a cap of turn.
fn check_route(
    world: &LevelWorld,
    walk: &Walk,
    route: &SmoothedRoute,
    limits: Smoothing,
) -> Measured {
    let stride = limits.speed * FRAME_SECONDS as f32;
    let turn = limits.turn_cap_degrees.to_radians() * FRAME_SECONDS as f32;
    let poses = &route.poses;
    assert!(!poses.is_empty());
    assert!(!poses[0].cut, "the first pose is no cut");

    let mut located = Vec::with_capacity(poses.len());
    let mut last_step = 0;
    for (i, pose) in poses.iter().enumerate() {
        let (step, cell) = pose
            .walk
            .expect("every smoothed pose names its walk step and cell");
        let step = step as usize;
        assert!(step >= last_step, "pose {i}: steps never go back");
        assert_eq!(
            world.locate_cell(pose.eye) as u32,
            cell,
            "pose {i}: its cell is where its eye locates"
        );
        let current_or_next = [Some(walk.cells[step]), walk.cells.get(step + 1).copied()];
        if !pose.cut {
            assert!(
                current_or_next.contains(&Some(cell)),
                "pose {i}: cell {cell} is not step {step}'s current or next"
            );
        }
        if pose.cut && route.locate_cuts == 0 {
            assert!(
                (last_step + 1..=step).any(|s| walk.teleport[s]),
                "pose {i}: a cut where the walk does not teleport"
            );
        }
        assert_eq!(pose.pitch.to_bits(), poses[0].pitch.to_bits());
        located.push(cell);
        last_step = step;
    }
    let cuts = poses.iter().filter(|pose| pose.cut).count();
    assert_eq!(cuts, route.teleport_cuts + route.locate_cuts);
    if route.locate_cuts == 0 {
        // The walk's cells, in order, among the cells the eye passes through.
        let mut wanted = collapsed(walk.cells.iter().copied()).into_iter().peekable();
        for cell in collapsed(located) {
            if wanted.peek() == Some(&cell) {
                wanted.next();
            }
        }
        assert_eq!(
            wanted.next(),
            None,
            "the eye visits every walk cell in order"
        );
    }

    let scale = poses
        .iter()
        .map(|p| p.eye.abs().max_element())
        .fold(1.0f32, f32::max);
    let slack = 1e-4 + 8.0 * f32::EPSILON * scale;
    let mut measured = Measured {
        max_stride: 0.0,
        max_turn: 0.0,
        pairs: 0,
        full_stride: 0,
        straight: 0,
        straight_short: 0,
    };
    for i in 1..poses.len() {
        let (a, b) = (&poses[i - 1], &poses[i]);
        if b.cut {
            continue;
        }
        let moved = a.eye.distance(b.eye);
        let turned = wrap(b.yaw - a.yaw).abs();
        assert!(
            moved <= stride + slack,
            "poses {}–{i}: {moved} m over a {stride} m stride",
            i - 1
        );
        assert!(
            turned <= turn + 1e-5,
            "poses {}–{i}: {turned} rad over a {turn} rad cap",
            i - 1
        );
        measured.max_stride = measured.max_stride.max(moved);
        measured.max_turn = measured.max_turn.max(turned);
        measured.pairs += 1;
        measured.full_stride += usize::from(moved >= stride - slack);
        // Away from turns: from the pose before the pair to the pose after
        // it, the eye keeps one direction and one yaw. A corner at either
        // end, a vertical reversal included, is a turn.
        let (Some(prior), Some(next)) = (i.checked_sub(2).map(|k| &poses[k]), poses.get(i + 1))
        else {
            continue;
        };
        let travel = b.eye - a.eye;
        let parallel =
            |d: Vec3| d.length() > slack && d.normalize().dot(travel.normalize()) > 1.0 - 1e-6;
        let yaw = b.yaw.to_bits();
        if !a.cut
            && !next.cut
            && [prior.yaw, a.yaw, next.yaw].map(f32::to_bits) == [yaw; 3]
            && travel.length() > slack
            && parallel(a.eye - prior.eye)
            && parallel(next.eye - b.eye)
        {
            measured.straight += 1;
            measured.straight_short += usize::from((moved - stride).abs() > slack);
        }
    }
    measured
}

fn pose_bits(pose: &RoutePose) -> ([u32; 5], Option<(u32, u32)>, bool) {
    (
        [pose.eye.x, pose.eye.y, pose.eye.z, pose.yaw, pose.pitch].map(f32::to_bits),
        pose.walk,
        pose.cut,
    )
}

fn route(kind: WalkKind, steps: usize, limits: Smoothing) -> (LevelWorld, Walk, SmoothedRoute) {
    let world = smoothed_world();
    let walk = Walk::build(&world, kind, steps);
    let route =
        smoothed_route_seeded(&world, kind, steps, PITCH, WALK_SEED, limits).expect("route");
    (world, walk, route)
}

#[test]
fn smoothed_route_visits_the_walk_cells_in_order_within_its_current_and_next_cells() {
    for kind in WalkKind::ALL {
        for limits in [BRISK, STROLL] {
            let (world, walk, route) = route(kind, STEPS, limits);
            check_route(&world, &walk, &route, limits);
            assert_eq!(
                route.locate_cuts,
                0,
                "{}: every leg is proven",
                kind.label()
            );
            assert_eq!(
                route.teleport_cuts,
                walk.teleports(),
                "{}: a cut per teleport",
                kind.label()
            );
            assert_eq!(route.poses[0].pitch, PITCH);
        }
    }
}

#[test]
fn smoothed_route_detours_inside_a_non_convex_cell_and_cuts_at_teleports() {
    let (world, walk, route) = route(WalkKind::FarPointTour, STEPS, BRISK);
    assert!(walk.teleports() > 0, "the tour teleports to the 7–8 pair");
    assert!(route.teleport_cuts > 0);
    // Cell 5's straight line between its portals to cells 4 and 6 crosses
    // the solid crook (x 16–20, z 8–12), so reaching 6 with no cut and no
    // pose in the crook takes a detour inside 5.
    assert_eq!(route.locate_cuts, 0);
    for cell in [5, 6] {
        assert!(walk.cells.contains(&cell));
        assert!(
            route.poses.iter().any(|p| p.walk.unwrap().1 == cell),
            "a pose in cell {cell}"
        );
    }
    let crook = |eye: Vec3| (16.0..20.0).contains(&eye.x) && (8.0..12.0).contains(&eye.z);
    assert!(!route.poses.iter().any(|p| crook(p.eye)));
    check_route(&world, &walk, &route, BRISK);
}

#[test]
fn smoothed_route_runs_a_full_stride_on_straights_and_slows_only_to_turn() {
    for kind in WalkKind::ALL {
        for limits in [BRISK, STROLL] {
            let (world, walk, route) = route(kind, STEPS, limits);
            let measured = check_route(&world, &walk, &route, limits);
            let stride = limits.speed * FRAME_SECONDS as f32;
            let turn = limits.turn_cap_degrees.to_radians() * FRAME_SECONDS as f32;
            assert!(measured.straight > 50, "{}: {measured:?}", kind.label());
            assert_eq!(measured.straight_short, 0, "{}: {measured:?}", kind.label());
            assert!((measured.max_stride - stride).abs() < 1e-4, "{measured:?}");
            assert!(
                (measured.max_turn - turn).abs() < 1e-5,
                "turns reach the cap: {measured:?}"
            );
        }
    }
}

#[test]
fn smoothed_routes_at_two_limits_each_keep_their_own() {
    let brisk_stride = BRISK.speed * FRAME_SECONDS as f32;
    let stroll_stride = STROLL.speed * FRAME_SECONDS as f32;
    let (world, walk, brisk) = route(WalkKind::Random, STEPS, BRISK);
    let fast = check_route(&world, &walk, &brisk, BRISK);
    let (world, walk, stroll) = route(WalkKind::Random, STEPS, STROLL);
    let slow = check_route(&world, &walk, &stroll, STROLL);
    // Each reaches its own limits, and the brisk route breaks the stroll's.
    assert!((fast.max_stride - brisk_stride).abs() < 1e-4);
    assert!((slow.max_stride - stroll_stride).abs() < 1e-4);
    assert!(fast.max_turn > STROLL.turn_cap_degrees.to_radians() * FRAME_SECONDS as f32);
    assert!(
        stroll.poses.len() > brisk.poses.len(),
        "a slower route takes more poses"
    );
}

#[test]
fn smoothed_route_is_deterministic() {
    for kind in WalkKind::ALL {
        let (_, _, a) = route(kind, STEPS, BRISK);
        let (_, _, b) = route(kind, STEPS, BRISK);
        assert_eq!(a.poses.len(), b.poses.len());
        assert!(
            a.poses
                .iter()
                .zip(&b.poses)
                .all(|(x, y)| pose_bits(x) == pose_bits(y))
        );
        assert_eq!(
            (a.teleport_cuts, a.locate_cuts),
            (b.teleport_cuts, b.locate_cuts)
        );
    }
}

#[test]
fn smoothed_route_refuses_non_positive_limits_and_levels_without_a_camera_graph() {
    let world = smoothed_world();
    let generate = |world: &LevelWorld, steps, speed, turn_cap_degrees| {
        let limits = Smoothing {
            speed,
            turn_cap_degrees,
        };
        smoothed_route_seeded(world, WalkKind::Random, steps, 0.0, WALK_SEED, limits).map(|_| ())
    };
    for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(
            generate(&world, 10, bad, 270.0),
            Err(RouteError::NonPositiveSpeed)
        );
        assert_eq!(
            generate(&world, 10, 8.0, bad),
            Err(RouteError::NonPositiveTurnCap)
        );
    }
    assert_eq!(generate(&world, 0, 8.0, 270.0), Err(RouteError::ZeroSteps));
    assert_eq!(
        generate(&solid_world(), 10, 8.0, 270.0),
        Err(RouteError::NoCameraCells)
    );
    assert_eq!(
        generate(&portalless_world(), 10, 8.0, 270.0),
        Err(RouteError::NoPortalBetweenCameraCells)
    );
}

#[test]
#[ignore = "on demand: set POSTRETRO_CAMERA_ROUTE_PRL to a compiled level's absolute path"]
fn smoothed_routes_keep_their_limits_on_a_compiled_level() {
    let path = std::env::var("POSTRETRO_CAMERA_ROUTE_PRL")
        .expect("POSTRETRO_CAMERA_ROUTE_PRL names a .prl by absolute path");
    let world = postretro_level_loader::load_prl(&path).expect("load the level");
    for kind in [WalkKind::FarPointTour, WalkKind::Random] {
        let limits = Smoothing::default();
        let walk = Walk::build(&world, kind, DEFAULT_STEPS);
        let route = SmoothedRoute::of_walk(&world, &walk, 0.0, limits);
        let measured = check_route(&world, &walk, &route, limits);
        println!(
            "smoothed {}: poses={} walk_teleports={} teleport_cuts={} locate_cuts={} \
             max_stride={} m (limit {}) max_turn={} deg (limit {}) full_stride={}/{} ({:.1}%) \
             straight={} straight_short={}",
            kind.key(),
            route.poses.len(),
            walk.teleports(),
            route.teleport_cuts,
            route.locate_cuts,
            measured.max_stride,
            limits.speed * FRAME_SECONDS as f32,
            measured.max_turn.to_degrees(),
            limits.turn_cap_degrees * FRAME_SECONDS as f32,
            measured.full_stride,
            measured.pairs,
            100.0 * measured.full_stride as f64 / measured.pairs.max(1) as f64,
            measured.straight,
            measured.straight_short,
        );
        assert_eq!(
            route.locate_cuts,
            0,
            "{}: cuts only where the walk teleports",
            kind.key()
        );
        assert_eq!(route.teleport_cuts, walk.teleports());
        assert_eq!(
            measured.straight_short,
            0,
            "{}: run speed away from turns",
            kind.key()
        );
    }
}
