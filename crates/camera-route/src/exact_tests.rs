// Exact routes reproduce the walk harness's frames draw for draw.
// See: context/lib/testing_guide.md

use glam::Vec3;
use postretro_level_loader::LevelWorld;

use crate::test_fixtures::{grid_world, portalless_world, solid_world};
use crate::{
    DEFAULT_ASPECT, DEFAULT_HFOV_DEGREES, EYE_TRIES, ExactRoute, RouteError, SplitMix64, WALK_SEED,
    Walk, WalkKind, exact_route, exact_route_seeded, pose_fingerprint,
};

// Past the random walk's 1,000-step stall, so it teleports to the portalless cell.
const STEPS: usize = 1_500;
const PROJECTION: (f32, f32) = (DEFAULT_HFOV_DEGREES, DEFAULT_ASPECT);

/// The harness's eye draw as it stood before the move, kept here verbatim
/// so a reordered draw inside the crate shows up as a mismatch.
fn oracle_eye_point(world: &LevelWorld, cell: u32, rng: &mut SplitMix64) -> Vec3 {
    let data = &world.cells[cell as usize];
    let extent = data.bounds_max - data.bounds_min;
    for _ in 0..EYE_TRIES {
        let fraction = Vec3::new(rng.unit(), rng.unit(), rng.unit()) * 0.8 + Vec3::splat(0.1);
        let point = data.bounds_min + extent * fraction;
        if world.locate_cell(point) == cell as usize {
            return point;
        }
    }
    (data.bounds_min + data.bounds_max) * 0.5
}

/// The harness `run` loop's eye stream and headings before the move: the
/// spawn eye, then each frame's pose fingerprint.
fn oracle(world: &LevelWorld, walk: &Walk, pitch: f32) -> (Vec3, Vec<u64>) {
    let mut rng = SplitMix64(WALK_SEED ^ 0xE7E5);
    let spawn_eye = oracle_eye_point(world, walk.cells[0], &mut rng);
    let mut prints = Vec::new();
    for (step, &cell) in walk.cells.iter().enumerate() {
        for frame in 0..walk.dwell[step] {
            let eye = oracle_eye_point(world, cell, &mut rng);
            let yaw = walk.travel_yaw[step] + frame as f32 * std::f32::consts::FRAC_PI_2;
            prints.push(pose_fingerprint(step, cell, eye, yaw, pitch, PROJECTION));
        }
    }
    (spawn_eye, prints)
}

fn fingerprints(route: &ExactRoute) -> Vec<u64> {
    route
        .poses
        .iter()
        .map(|pose| {
            let (step, cell) = pose.walk.expect("walk-made pose");
            pose_fingerprint(
                step as usize,
                cell,
                pose.eye,
                pose.yaw,
                pose.pitch,
                PROJECTION,
            )
        })
        .collect()
}

fn bits(v: Vec3) -> [u32; 3] {
    v.to_array().map(f32::to_bits)
}

#[test]
fn walk_build_is_deterministic_and_teleports_to_a_portalless_cell() {
    let world = grid_world();
    for kind in WalkKind::ALL {
        let a = Walk::build(&world, kind, STEPS);
        let b = Walk::build(&world, kind, STEPS);
        assert_eq!(a.cells, b.cells, "{}", kind.label());
        assert_eq!(a.dwell, b.dwell);
        assert_eq!(a.teleport, b.teleport);
        assert_eq!(
            a.travel_yaw.iter().map(|y| y.to_bits()).collect::<Vec<_>>(),
            b.travel_yaw.iter().map(|y| y.to_bits()).collect::<Vec<_>>()
        );
        assert_eq!(a.cells.len(), STEPS);
        assert!(
            a.cells.contains(&8),
            "{} reaches the portalless cell",
            kind.label()
        );
        assert!(a.teleports() > 0, "{} teleports to cell 8", kind.label());
        assert!(!a.cells.contains(&4), "a solid cell is never a camera cell");
        assert!(
            !a.cells.contains(&9),
            "an exterior cell is never a camera cell"
        );
    }
}

#[test]
fn exact_route_draws_eyes_in_the_harness_order_with_bit_equal_fingerprints() {
    let world = grid_world();
    for kind in WalkKind::ALL {
        for pitch in [0.0f32, (-12.5f32).to_radians()] {
            let walk = Walk::build(&world, kind, STEPS);
            let (spawn, expected) = oracle(&world, &walk, pitch);
            let route = exact_route(&world, kind, STEPS, pitch).expect("route");
            assert_eq!(bits(route.spawn), bits(spawn), "{} spawn", kind.label());
            assert_eq!(route.poses.len() as u64, walk.frames());
            let got = fingerprints(&route);
            if let Some(i) = (0..expected.len()).find(|&i| got[i] != expected[i]) {
                panic!(
                    "{} pitch {pitch}: fingerprints differ from frame {i}",
                    kind.label()
                );
            }
            assert!(
                route
                    .poses
                    .iter()
                    .all(|p| p.pitch.to_bits() == pitch.to_bits() && !p.cut)
            );
        }
    }
}

#[test]
fn exact_route_eyes_locate_in_their_walk_cell_or_fall_back_to_its_centre() {
    let world = grid_world();
    let route = exact_route(&world, WalkKind::Random, STEPS, 0.0).expect("route");
    let mut fallbacks = 0;
    for pose in &route.poses {
        let (_, cell) = pose.walk.expect("walk-made pose");
        let data = &world.cells[cell as usize];
        if cell == 10 {
            assert_eq!(
                bits(pose.eye),
                bits((data.bounds_min + data.bounds_max) * 0.5)
            );
            fallbacks += 1;
        } else {
            assert_eq!(world.locate_cell(pose.eye), cell as usize);
        }
    }
    assert!(fallbacks > 0, "the walk visits the locator-less cell");
}

#[test]
fn exact_route_seeded_at_the_walk_seed_is_the_walk_and_another_seed_is_not() {
    let world = grid_world();
    let walk = exact_route(&world, WalkKind::FarPointTour, STEPS, 0.0).expect("route");
    let same =
        exact_route_seeded(&world, WalkKind::FarPointTour, STEPS, 0.0, WALK_SEED).expect("route");
    assert_eq!(fingerprints(&walk), fingerprints(&same));
    let other = exact_route_seeded(&world, WalkKind::FarPointTour, STEPS, 0.0, 7).expect("route");
    assert_ne!(fingerprints(&walk), fingerprints(&other));
}

#[test]
fn exact_route_refuses_zero_steps_and_levels_without_a_camera_graph() {
    let grid = grid_world();
    assert_eq!(
        exact_route(&grid, WalkKind::Random, 0, 0.0),
        Err(RouteError::ZeroSteps)
    );
    assert_eq!(
        exact_route(&solid_world(), WalkKind::Random, 10, 0.0),
        Err(RouteError::NoCameraCells)
    );
    assert_eq!(
        exact_route(&portalless_world(), WalkKind::FarPointTour, 10, 0.0),
        Err(RouteError::NoPortalBetweenCameraCells)
    );
}
