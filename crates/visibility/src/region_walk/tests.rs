// Region walk vs the exact oracle: superset, bounded cost, order independence.

use super::oracle::{ORACLE_STEP_LIMIT, OracleWalk, oracle_from_camera};
use super::test_fixtures::{
    CameraPose, GridWorld, camera_view_proj, cyclic_rings, open_lattice, sample_cameras,
    stacked_slab_shaft,
};
use super::{
    MAX_PORTAL_WALK_STEPS, PortalWalkScratch, RegionWalkStats, WalkView, WalkWorld,
    walk_from_camera,
};
use postretro_level_loader::LevelWorld;

/// Randomized superset sweep: fixed seed and count per fixture.
pub(crate) const SWEEP_SEED: u64 = 0x5EED_2026_1025;
pub(crate) const SWEEP_CAMERAS: usize = 600;
/// Portal tests per summed degree of reached cells: the measured worst over
/// the fixture sweep, rounded up, with headroom (see `plan.md`).
pub(super) const PORTAL_TESTS_PER_DEGREE: u32 = 2;
/// Expansions any one cell may take on a fixture.
pub(super) const MAX_EXPANSIONS_PER_CELL: u32 = 32;
/// The chain walk's cap before the bounded-region walk replaced it.
const PRE_REGION_WALK_STEP_LIMIT: u64 = 20_000;

pub(crate) fn fixtures() -> Vec<(&'static str, GridWorld)> {
    vec![
        ("stacked-slab shaft", stacked_slab_shaft(3, 24)),
        ("open lattice", open_lattice(6)),
        ("cyclic rings", cyclic_rings(5, 4)),
    ]
}

pub(super) fn input(world: &LevelWorld) -> WalkWorld<'_> {
    WalkWorld {
        world,
        blocked_portals: &[],
    }
}

pub(super) fn walk(
    world: &LevelWorld,
    pose: &CameraPose,
    scratch: &mut PortalWalkScratch,
) -> RegionWalkStats {
    let view = WalkView::camera(pose.eye, camera_view_proj(pose.eye, pose.yaw, pose.pitch));
    walk_from_camera(input(world), &view, pose.cell, u32::MAX, scratch)
}

pub(super) fn oracle(world: &LevelWorld, pose: &CameraPose) -> OracleWalk {
    let view = WalkView::camera(pose.eye, camera_view_proj(pose.eye, pose.yaw, pose.pitch));
    oracle_from_camera(world, &[], &view, pose.cell)
}

/// Cells the oracle reaches cleanly that `scratch` lacks: unclassified misses.
pub(super) fn unclassified_misses(oracle: &OracleWalk, scratch: &PortalWalkScratch) -> Vec<usize> {
    (0..oracle.visible.len())
        .filter(|&c| oracle.clean[c] && !scratch.is_reached(c))
        .collect()
}

fn summed_degree(world: &LevelWorld, scratch: &PortalWalkScratch) -> u32 {
    scratch
        .reached()
        .iter()
        .map(|&c| world.cell_portal_count(c as usize) as u32)
        .sum()
}

#[derive(Default)]
struct SweepReport {
    cameras: usize,
    oracle_incomplete: usize,
    classified_exceptions: usize,
    worst_tests: u32,
    worst_tests_per_degree: f32,
    worst_expansions: u32,
    worst_oracle_steps: u64,
    extra_cells_max: usize,
}

fn sweep(name: &str, grid: &GridWorld) -> SweepReport {
    let world = &grid.world;
    let mut scratch = PortalWalkScratch::new();
    let mut report = SweepReport::default();
    for pose in sample_cameras(grid, SWEEP_CAMERAS, SWEEP_SEED) {
        let stats = walk(world, &pose, &mut scratch);
        let exact = oracle(world, &pose);
        report.cameras += 1;
        if !exact.complete() {
            report.oracle_incomplete += 1;
        }
        report.worst_oracle_steps = report.worst_oracle_steps.max(exact.considered);

        assert!(!stats.step_limit_hit);
        assert!(
            scratch.is_reached(pose.cell),
            "{name}: camera cell always visible"
        );
        let misses = unclassified_misses(&exact, &scratch);
        assert!(
            misses.is_empty(),
            "{name}: walk misses cells the exact walk reaches cleanly: {misses:?} at {pose:?}"
        );
        report.classified_exceptions += (0..exact.visible.len())
            .filter(|&c| exact.visible[c] && !scratch.is_reached(c))
            .count();
        let exact_count = exact.visible.iter().filter(|&&v| v).count();
        report.extra_cells_max = report
            .extra_cells_max
            .max(scratch.reached().len().saturating_sub(exact_count));

        let degree = summed_degree(world, &scratch).max(1);
        report.worst_tests = report.worst_tests.max(stats.considered);
        report.worst_tests_per_degree = report
            .worst_tests_per_degree
            .max(stats.considered as f32 / degree as f32);
        let worst_cell = scratch
            .reached()
            .iter()
            .map(|&c| scratch.expansions(c as usize))
            .max()
            .unwrap_or(0);
        report.worst_expansions = report.worst_expansions.max(worst_cell);
    }
    eprintln!(
        "[region_walk sweep] {name}: cameras={} oracle_incomplete={} classified={} \
         worst_tests={} worst_tests_per_degree={:.2} worst_expansions={} \
         worst_oracle_steps={} extra_cells_max={}",
        report.cameras,
        report.oracle_incomplete,
        report.classified_exceptions,
        report.worst_tests,
        report.worst_tests_per_degree,
        report.worst_expansions,
        report.worst_oracle_steps,
        report.extra_cells_max,
    );
    report
}

#[test]
fn region_walk_reaches_every_cell_the_exact_walk_reaches_over_randomized_cameras() {
    let mut slab_beats_today = false;
    for (name, grid) in fixtures() {
        let report = sweep(name, &grid);
        assert_eq!(report.cameras, SWEEP_CAMERAS);
        assert!(report.worst_expansions <= MAX_EXPANSIONS_PER_CELL, "{name}");
        assert!(
            report.worst_tests_per_degree <= PORTAL_TESTS_PER_DEGREE as f32,
            "{name}: {} portal tests per degree",
            report.worst_tests_per_degree
        );
        assert!(
            MAX_PORTAL_WALK_STEPS >= 2 * report.worst_tests,
            "{name}: cap needs 2x headroom over {}",
            report.worst_tests
        );
        if name == "stacked-slab shaft" {
            slab_beats_today = report.worst_oracle_steps > PRE_REGION_WALK_STEP_LIMIT
                && report.worst_oracle_steps < ORACLE_STEP_LIMIT;
        }
    }
    assert!(
        slab_beats_today,
        "a stacked-slab camera's exact walk must exceed the pre-change cap and finish under the oracle's"
    );
}
