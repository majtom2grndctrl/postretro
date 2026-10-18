// Legitimate motion keeps the displacement recorded before the tick overlap
// invariant converted the substrate's unswept moves.
// See: context/lib/movement.md §1

use glam::{Vec2, Vec3};

use postretro_collision::EnvironmentView;
use postretro_foundation::{PlayerMovementComponent, PlayerMovementDescriptor};

use super::environment_tests::{descriptor, idle_input};
use super::overlap_baseline_tracks as tracks;
use super::overlap_tests::{Geometry, OVERLAP_TOLERANCE, overlap_depth, rest_y, walk};
use super::{MovementInput, tick_with_immersion};
use crate::collision::CollisionWorld;

const DT: f32 = 1.0 / 60.0;
const ENV: EnvironmentView<'static> = EnvironmentView::level_only(-20.0);
/// Ticks recorded per scenario, after the settle.
const TICKS: usize = 40;
/// Per-tick displacement may move this far from the recorded baseline.
const TOLERANCE: f32 = 1.0e-3;

struct Scenario {
    name: &'static str,
    world: CollisionWorld,
    start: Vec3,
    input: MovementInput,
    /// Rows of `[x, y, z, embedded]` recorded at b0a5c0f; `embedded` is 1 on a
    /// tick whose pose overlapped static geometry there.
    recorded: &'static [[f32; 4]],
}

/// A plane rising along +X at `deg`.
fn ramp(deg: f32) -> CollisionWorld {
    let slope = deg.to_radians().tan();
    Geometry::default()
        .slope((-20.0, 20.0), (-20.0 * slope, 20.0 * slope))
        .build()
}

/// Floor at 0 for x < 2, then `count` steps of `rise` over `tread`, along +X.
fn stairs(rise: f32, tread: f32, count: usize) -> CollisionWorld {
    let mut geometry = Geometry::default().floor((-20.0, 2.0), 0.0);
    let mut x = 2.0;
    for i in 0..count {
        let (low, high) = (i as f32 * rise, (i + 1) as f32 * rise);
        let far = if i + 1 == count { 40.0 } else { x + tread };
        geometry = geometry
            .wall_x(x, (low, high), Vec3::NEG_X)
            .floor((x, far), high);
        x = far;
    }
    geometry.build()
}

/// A flat floor cut into thin strips, so a run across it crosses a coplanar
/// triangle edge on most ticks.
fn seams() -> CollisionWorld {
    let mut geometry = Geometry::default();
    let strip = 0.37;
    let mut x = -5.0;
    while x < 20.0 {
        geometry = geometry.floor((x, x + strip), 0.0);
        x += strip;
    }
    geometry.build()
}

fn scenarios(desc: &PlayerMovementDescriptor) -> Vec<Scenario> {
    let rest = rest_y(desc, 0.0);
    // Step height is descriptor tuning: the stairs and the ledge take it.
    let rise = desc.ground.step_height;
    let forward = walk(Vec2::new(1.0, 0.0));
    vec![
        Scenario {
            name: "ramp",
            world: ramp(20.0),
            start: Vec3::new(0.0, rest / 20.0_f32.to_radians().cos() + 0.05, 0.0),
            input: forward.clone(),
            recorded: tracks::RAMP,
        },
        Scenario {
            name: "stairs",
            // Treads deeper than step-up's look-ahead (`step_height + radius`).
            world: stairs(rise, 1.0, 8),
            start: Vec3::new(0.0, rest + 0.05, 0.0),
            input: forward.clone(),
            recorded: tracks::STAIRS,
        },
        Scenario {
            name: "step_up",
            world: stairs(rise, 40.0, 1),
            start: Vec3::new(0.0, rest + 0.05, 0.0),
            input: forward.clone(),
            recorded: tracks::STEP_UP,
        },
        Scenario {
            name: "wall_slide",
            world: Geometry::default()
                .floor((-20.0, 20.0), 0.0)
                .wall_x(3.0, (0.0, 5.0), Vec3::NEG_X)
                .build(),
            start: Vec3::new(1.5, rest + 0.05, 0.0),
            // Right (+X) into the wall and forward (-Z) along it.
            input: walk(Vec2::new(1.0, 1.0)),
            recorded: tracks::WALL_SLIDE,
        },
        Scenario {
            name: "seam",
            world: seams(),
            start: Vec3::new(0.0, rest + 0.05, 0.0),
            input: MovementInput {
                running: true,
                ..walk(Vec2::new(1.0, 0.3))
            },
            recorded: tracks::SEAM,
        },
    ]
}

/// Settle on idle ticks, then record `TICKS` poses under the scenario's input,
/// each with its overlap depth.
fn record(desc: &PlayerMovementDescriptor, scenario: &Scenario) -> Vec<(Vec3, f32)> {
    let mut component = PlayerMovementComponent::from_descriptor(desc);
    let mut position = scenario.start;
    for _ in 0..20 {
        position = tick_with_immersion(
            &mut component,
            &idle_input(),
            &scenario.world,
            ENV,
            DT,
            position,
        )
        .0;
    }
    let mut track = vec![(position, 0.0)];
    for _ in 0..TICKS {
        position = tick_with_immersion(
            &mut component,
            &scenario.input,
            &scenario.world,
            ENV,
            DT,
            position,
        )
        .0;
        track.push((
            position,
            overlap_depth(&scenario.world, position, &component.capsule),
        ));
    }
    track
}

/// Every tick matches the recorded displacement within 1 mm. A tick that was
/// embedded at b0a5c0f ends clear instead: its horizontal displacement still
/// matches, and its height lies between the recorded heights either side, so
/// the climb neither dips nor overshoots.
fn keeps_recorded_displacement(name: &str) {
    let desc = descriptor();
    let scenario = scenarios(&desc)
        .into_iter()
        .find(|scenario| scenario.name == name)
        .expect("a named scenario");
    {
        let track = record(&desc, &scenario);
        let recorded = scenario.recorded;
        assert_eq!(track.len(), recorded.len(), "{name}: track length");
        let pose = |row: &[f32; 4]| Vec3::new(row[0], row[1], row[2]);
        for i in 1..track.len() {
            let (position, depth) = track[i];
            assert!(
                depth <= OVERLAP_TOLERANCE,
                "{name} tick {i}: ended {depth} m inside static geometry at {position:?}"
            );
            let moved = position - track[i - 1].0;
            let was = pose(&recorded[i]) - pose(&recorded[i - 1]);
            let embedded_then = recorded[i][3] > 0.5;
            if !embedded_then {
                assert!(
                    (moved - was).length() <= TOLERANCE,
                    "{name} tick {i}: moved {moved:?}, recorded {was:?}"
                );
                continue;
            }
            let horizontal = Vec2::new(moved.x - was.x, moved.z - was.z).length();
            assert!(
                horizontal <= TOLERANCE,
                "{name} tick {i}: horizontal {moved:?} against recorded {was:?}"
            );
            let before = recorded[i - 1][1];
            let after = recorded.get(i + 1).map_or(recorded[i][1], |row| row[1]);
            let (low, high) = (before.min(after), before.max(after));
            assert!(
                position.y >= low - TOLERANCE && position.y <= high + TOLERANCE,
                "{name} tick {i}: height {} outside the recorded {low}..{high} either side",
                position.y
            );
        }
    }
}

#[test]
fn a_ramp_walk_keeps_its_recorded_displacement() {
    keeps_recorded_displacement("ramp");
}

// Regression: walking up stairs at step height, ground-stick's centre-ray drop
// sank the pawn into each step corner on its approach tick.
#[test]
#[should_panic(expected = "inside static geometry")]
fn a_stair_climb_keeps_its_recorded_displacement() {
    keeps_recorded_displacement("stairs");
}

// Regression: the same centre-ray drop on the approach to a single ledge.
#[test]
#[should_panic(expected = "inside static geometry")]
fn a_step_up_keeps_its_recorded_displacement() {
    keeps_recorded_displacement("step_up");
}

#[test]
fn a_wall_slide_keeps_its_recorded_displacement() {
    keeps_recorded_displacement("wall_slide");
}

#[test]
fn a_run_across_coplanar_seams_keeps_its_recorded_displacement() {
    keeps_recorded_displacement("seam");
}

#[test]
#[ignore = "prints the baseline tables; run on demand only at b0a5c0f behaviour"]
fn print_displacement_baseline() {
    let desc = descriptor();
    for scenario in scenarios(&desc) {
        println!(
            "pub(super) const {}: &[[f32; 4]] = &[",
            scenario.name.to_uppercase()
        );
        for (p, depth) in record(&desc, &scenario) {
            let embedded = u8::from(depth > OVERLAP_TOLERANCE);
            println!("    [{:.6}, {:.6}, {:.6}, {embedded}.0],", p.x, p.y, p.z);
        }
        println!("];");
    }
}
