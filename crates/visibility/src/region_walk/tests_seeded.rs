// Seeded entry: the planar water mirror's seed cells under the caller's
// oblique near plane. See: context/lib/rendering_pipeline.md §2, §7.3a

use std::f32::consts::TAU;

use glam::{Mat4, Vec3};

use super::oracle::{oracle_from_camera, oracle_from_seeds};
use super::test_fixtures::{
    GridWorld, SplitMix64, axis_node, axis_rect, boxes_world, camera_view_proj, cyclic_rings,
    look_at_view_proj, open_lattice, portal, sample_cameras, stacked_slab_shaft,
};
use super::tests::{
    SWEEP_CAMERAS, SWEEP_SEED, WALK_STEP_LIMIT, fixtures, input, unclassified_misses,
};
use super::{
    PORTAL_BYPASS_DISTANCE, PortalWalkScratch, RegionWalkStats, WalkView, walk_from_camera,
    walk_from_seeds,
};
use crate::visibility::{
    FrustumPlane, NEAR_PLANE_INDEX, extract_frustum_planes, is_aabb_outside_frustum,
};
use postretro_level_loader::{CellLocatorChild, LevelWorld};

const MIRROR_SEED: u64 = 0x5EED_0A7E_2026;
const MIRROR_POSES: usize = 48;

/// One planar mirror's walk inputs: the mirrored eye and view, the water
/// height whose plane is the caller's near plane, and the in-view cells the
/// surface spans.
struct MirrorPose {
    eye: Vec3,
    view_proj: Mat4,
    water: f32,
    seeds: Vec<usize>,
}

impl MirrorPose {
    fn view(&self) -> WalkView {
        WalkView::seeded(self.eye, self.view_proj, water_plane(self.water))
    }
}

/// The water plane facing the visible, above-water side.
fn water_plane(h: f32) -> FrustumPlane {
    FrustumPlane {
        normal: Vec3::Y,
        dist: -h,
    }
}

/// Water at a grid plane, and how far above it the camera stands: more than
/// any cell below the water is tall, so the mirrored eye is in no seed.
fn mirror_fixtures() -> Vec<(&'static str, GridWorld, f32, f32)> {
    let slab = stacked_slab_shaft(3, 24);
    let slab_water = slab.ys[10];
    let lattice = open_lattice(6);
    let lattice_water = lattice.ys[3];
    vec![
        ("stacked-slab shaft", slab, slab_water, 1.5),
        ("open lattice", lattice, lattice_water, 2.5),
    ]
}

/// Cameras above the water looking down, mirrored across it. Poses that see
/// fewer than two seeds are redrawn.
fn mirror_poses(grid: &GridWorld, water: f32, clearance: f32, count: usize) -> Vec<MirrorPose> {
    let world = &grid.world;
    let last = |v: &[f32]| v[v.len() - 1];
    let lo = Vec3::new(grid.xs[0], water + clearance, grid.zs[0]);
    let hi = Vec3::new(last(&grid.xs), last(&grid.ys), last(&grid.zs));
    let mut rng = SplitMix64(MIRROR_SEED);
    let mut poses = Vec::new();
    let mut attempts = 0;
    while poses.len() < count {
        attempts += 1;
        assert!(attempts < count * 20, "too few mirror poses see two seeds");
        let fraction = Vec3::new(rng.unit(), rng.unit(), rng.unit()) * 0.9 + Vec3::splat(0.05);
        let camera = lo + (hi - lo) * fraction;
        let yaw = rng.unit() * TAU;
        let pitch = -(15.0 + rng.unit() * 65.0f32).to_radians();
        let eye = Vec3::new(camera.x, 2.0 * water - camera.y, camera.z);
        let view_proj = camera_view_proj(eye, yaw, -pitch);
        let view = WalkView::seeded(eye, view_proj, water_plane(water));
        let seeds: Vec<usize> = (0..world.cell_count())
            .filter(|&c| {
                let cell = &world.cells[c];
                !cell.is_solid
                    && cell.bounds_min.y <= water
                    && water <= cell.bounds_max.y
                    && !is_aabb_outside_frustum(cell.bounds_min, cell.bounds_max, view.frustum())
            })
            .collect();
        if seeds.len() >= 2 {
            poses.push(MirrorPose {
                eye,
                view_proj,
                water,
                seeds,
            });
        }
    }
    poses
}

fn seeded(
    world: &LevelWorld,
    view: &WalkView,
    seeds: &[usize],
    cap: u32,
    scratch: &mut PortalWalkScratch,
) -> RegionWalkStats {
    walk_from_seeds(input(world), view, seeds, cap, scratch)
}

fn reached_set(scratch: &PortalWalkScratch) -> Vec<u32> {
    let mut cells = scratch.reached().to_vec();
    cells.sort_unstable();
    cells
}

fn shuffled(seeds: &[usize], rng: &mut SplitMix64) -> Vec<usize> {
    let mut out = seeds.to_vec();
    for i in (1..out.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        out.swap(i, j);
    }
    out
}

fn contains(world: &LevelWorld, cell: usize, point: Vec3) -> bool {
    let data = &world.cells[cell];
    point.cmpge(data.bounds_min).all() && point.cmple(data.bounds_max).all()
}

/// A copy of `world` with `cell` flagged exterior.
fn with_exterior(world: &LevelWorld, cell: usize) -> LevelWorld {
    let mut cells = world.cells.clone();
    cells[cell].is_exterior = true;
    LevelWorld::new_visibility_only(
        cells,
        world.cell_portal_refs.clone(),
        world.cell_locator_root,
        world.cell_locator_nodes.clone(),
        world.portals.clone(),
        world.has_portals,
    )
    .expect("flagging a cell exterior keeps the world valid")
}

// O27, O33: the seeded walk is a superset of the seeded oracle; no locator call.
#[test]
fn seeded_walk_reaches_every_cell_the_seeded_oracle_reaches_under_the_water_plane() {
    for (name, grid, water, clearance) in mirror_fixtures() {
        let world = &grid.world;
        let mut scratch = PortalWalkScratch::new();
        for pose in mirror_poses(&grid, water, clearance, MIRROR_POSES) {
            let view = pose.view();
            let own_near = extract_frustum_planes(pose.view_proj).planes[NEAR_PLANE_INDEX];
            assert!(
                own_near.normal.dot(Vec3::Y) < 0.999,
                "{name}: water plane is oblique"
            );
            assert!(
                pose.seeds.iter().all(|&s| !contains(world, s, pose.eye)),
                "{name}: no seed holds the mirrored eye"
            );

            let stats = seeded(world, &view, &pose.seeds, u32::MAX, &mut scratch);
            let exact = oracle_from_seeds(world, &[], &view, &pose.seeds);
            assert!(
                exact.complete(),
                "{name}: oracle finished at {:?}",
                pose.eye
            );
            assert!(!stats.step_limit_hit);
            let misses = unclassified_misses(&exact, &scratch);
            assert!(
                misses.is_empty(),
                "{name}: seeded walk misses cells the seeded oracle reaches cleanly: \
                 {misses:?} at eye {:?}",
                pose.eye
            );

            // The locator's answer for the mirrored eye lies below the water,
            // behind every portal out of the seeds: a walk that located the
            // eye would hold it.
            let located = world.locate_cell(pose.eye);
            assert!(!pose.seeds.contains(&located), "{name}");
            assert!(
                !scratch.is_reached(located),
                "{name}: located cell {located} stays hidden"
            );
            assert!(
                !exact.visible[located],
                "{name}: oracle hides located cell {located}"
            );
        }
    }
}

// O33: seed order changes neither the set, the portal-test count, nor the trip.
#[test]
fn seeded_walk_ignores_seed_order_with_and_without_a_cap_trip() {
    let mut rng = SplitMix64(MIRROR_SEED ^ 0xD1CE);
    for (name, grid, water, clearance) in mirror_fixtures() {
        let world = &grid.world;
        let mut scratch = PortalWalkScratch::new();
        for pose in mirror_poses(&grid, water, clearance, 16) {
            let view = pose.view();
            let base = seeded(world, &view, &pose.seeds, u32::MAX, &mut scratch);
            let base_set = reached_set(&scratch);
            let cap = (base.considered / 2).max(1);
            let capped = seeded(world, &view, &pose.seeds, cap, &mut scratch);
            let capped_set = reached_set(&scratch);
            assert!(capped.step_limit_hit, "{name}");

            let mut orders: Vec<Vec<usize>> =
                (0..4).map(|_| shuffled(&pose.seeds, &mut rng)).collect();
            orders.push(pose.seeds.iter().rev().copied().collect());
            for order in orders {
                let stats = seeded(world, &view, &order, u32::MAX, &mut scratch);
                assert_eq!(reached_set(&scratch), base_set, "{name}: order {order:?}");
                assert_eq!(stats.considered, base.considered, "{name}: order {order:?}");
                assert!(!stats.step_limit_hit);

                let stats = seeded(world, &view, &order, cap, &mut scratch);
                assert_eq!(
                    reached_set(&scratch),
                    capped_set,
                    "{name}: capped, order {order:?}"
                );
                assert_eq!(stats.considered, capped.considered, "{name}: capped");
                assert!(stats.step_limit_hit, "{name}: capped order still trips");
            }
        }
    }
}

// O31: on the seeded entry the near-plane test runs before the bypass.
#[test]
fn seeded_walk_hides_cells_behind_the_near_plane_even_within_bypass_distance() {
    // Cell 0 straddles the water at y = 0; cells 1 and 2 are stacked below it,
    // reachable only through floor portals wholly under the water.
    let boxes = [
        (Vec3::new(-2.0, -2.0, -2.0), Vec3::new(2.0, 2.0, 2.0)),
        (Vec3::new(-2.0, -4.0, -2.0), Vec3::new(2.0, -2.0, 2.0)),
        (Vec3::new(-2.0, -6.0, -2.0), Vec3::new(2.0, -4.0, 2.0)),
    ];
    let portals = vec![
        portal(axis_rect(1, -2.0, [-2.0, -2.0], [2.0, 2.0]), 0, 1),
        portal(axis_rect(1, -4.0, [-2.0, -2.0], [2.0, 2.0]), 1, 2),
    ];
    let nodes = vec![
        axis_node(
            1,
            -2.0,
            CellLocatorChild::Cell(0),
            CellLocatorChild::Node(1),
        ),
        axis_node(
            1,
            -4.0,
            CellLocatorChild::Cell(1),
            CellLocatorChild::Cell(2),
        ),
    ];
    let world = boxes_world(&boxes, portals, (CellLocatorChild::Node(0), nodes));

    // The eye sits in cell 0, 0.005 above the first floor portal, looking
    // down through it.
    let eye = Vec3::new(0.3, -1.995, 0.2);
    assert!((eye.y + 2.0).abs() <= PORTAL_BYPASS_DISTANCE);
    assert_eq!(world.locate_cell(eye), 0);
    let view_proj = look_at_view_proj(eye, Vec3::new(0.3, -5.0, -1.5));
    let mut scratch = PortalWalkScratch::new();

    // Camera entry, near plane slid to the eye: the bypass reaches cell 1.
    let camera = WalkView::camera(eye, view_proj);
    let stats = walk_from_camera(input(&world), &camera, 0, u32::MAX, &mut scratch);
    assert!(stats.bypassed >= 1);
    assert!(
        scratch.is_reached(1),
        "the bypass reaches the cell when the near plane allows"
    );
    assert!(oracle_from_camera(&world, &[], &camera, 0).visible[1]);

    // Seeded entry under the water plane: the same portal is rejected first.
    let mirror = WalkView::seeded(eye, view_proj, water_plane(0.0));
    let stats = seeded(&world, &mirror, &[0], u32::MAX, &mut scratch);
    assert!(scratch.is_reached(0));
    assert!(
        !scratch.is_reached(1),
        "cell behind the water plane stays hidden"
    );
    assert!(!scratch.is_reached(2));
    assert_eq!(stats.bypassed, 0);
    assert!(stats.rejected_clipped >= 1);
    let exact = oracle_from_seeds(&world, &[], &mirror, &[0]);
    assert!(exact.visible[0]);
    assert!(
        !exact.visible[1],
        "oracle hides the cell behind the water plane"
    );
    assert!(!exact.visible[2]);
}

// O27: one seed at the camera cell under the slid frustum is the camera entry.
#[test]
fn seeded_walk_from_the_camera_cell_under_the_slid_frustum_equals_the_camera_entry() {
    for (name, grid) in fixtures() {
        let world = &grid.world;
        let mut scratch = PortalWalkScratch::new();
        let mut compared = 0;
        for pose in sample_cameras(&grid, SWEEP_CAMERAS, SWEEP_SEED) {
            // A sampled eye pushed past a wall can sit in a solid cell, or
            // outside the grid where the fixture's locator clamps it to a
            // cell that does not hold it. The camera entry walks only a
            // non-solid cell holding the eye; elsewhere the caller falls back.
            if world.cells[pose.cell].is_solid || !contains(world, pose.cell, pose.eye) {
                continue;
            }
            compared += 1;
            let view_proj = camera_view_proj(pose.eye, pose.yaw, pose.pitch);
            let camera = WalkView::camera(pose.eye, view_proj);
            let slid_near = camera.frustum().planes[NEAR_PLANE_INDEX];
            let seeded_view = WalkView::seeded(pose.eye, view_proj, slid_near);

            let expected =
                walk_from_camera(input(world), &camera, pose.cell, u32::MAX, &mut scratch);
            let expected_set = reached_set(&scratch);
            let stats = seeded(world, &seeded_view, &[pose.cell], u32::MAX, &mut scratch);
            assert_eq!(reached_set(&scratch), expected_set, "{name}: {pose:?}");
            assert_eq!(stats.considered, expected.considered, "{name}: {pose:?}");
        }
        assert!(
            compared > SWEEP_CAMERAS * 9 / 10,
            "{name}: {compared} cameras compared"
        );
    }
}

// O32: no usable seed walks nothing; each unusable kind is skipped on its own.
#[test]
fn seeded_walk_without_usable_seeds_returns_an_empty_set_without_a_trip() {
    let grid = cyclic_rings(5, 4);
    let usable = grid.cell_at(0, 1, 2);
    let solid = grid.cell_at(2, 1, 2);
    let exterior = grid.cell_at(0, 1, 4);
    let off_screen = grid.cell_at(0, 1, 0);
    let world = with_exterior(&grid.world, exterior);
    assert!(world.cells[solid].is_solid);

    let eye = grid.centre(usable);
    let view_proj = look_at_view_proj(eye, eye + Vec3::Z);
    let view = WalkView::seeded(
        eye,
        view_proj,
        WalkView::camera(eye, view_proj).frustum().planes[NEAR_PLANE_INDEX],
    );
    let cell = |c: usize| &world.cells[c];
    assert!(is_aabb_outside_frustum(
        cell(off_screen).bounds_min,
        cell(off_screen).bounds_max,
        view.frustum()
    ));
    assert!(!is_aabb_outside_frustum(
        cell(exterior).bounds_min,
        cell(exterior).bounds_max,
        view.frustum()
    ));

    let out_of_range = [world.cell_count(), usize::MAX];
    let unusable: [&[usize]; 5] = [&out_of_range, &[solid], &[exterior], &[off_screen], &[]];
    let mut scratch = PortalWalkScratch::new();
    let all: Vec<usize> = unusable.iter().flat_map(|s| s.iter().copied()).collect();
    for seeds in unusable.iter().copied().chain([all.as_slice()]) {
        let stats = seeded(&world, &view, seeds, u32::MAX, &mut scratch);
        assert_eq!(stats, RegionWalkStats::default(), "seeds {seeds:?}");
        assert!(scratch.reached().is_empty(), "seeds {seeds:?}");
    }

    // Beside a usable seed, each unusable kind changes nothing.
    let base = seeded(&world, &view, &[usable], u32::MAX, &mut scratch);
    let base_set = reached_set(&scratch);
    assert!(base.considered > 0);
    for kind in unusable {
        for seeds in [
            [&[usable][..], kind].concat(),
            [kind, &[usable][..]].concat(),
        ] {
            let stats = seeded(&world, &view, &seeds, u32::MAX, &mut scratch);
            assert_eq!(reached_set(&scratch), base_set, "seeds {seeds:?}");
            assert_eq!(stats.considered, base.considered, "seeds {seeds:?}");
            assert!(!stats.step_limit_hit);
        }
    }
}

// O32: a repeated seed is one seed.
#[test]
fn seeded_walk_counts_a_repeated_seed_once() {
    for (name, grid, water, clearance) in mirror_fixtures() {
        let world = &grid.world;
        let mut scratch = PortalWalkScratch::new();
        for pose in mirror_poses(&grid, water, clearance, 16) {
            let view = pose.view();
            let base = seeded(world, &view, &pose.seeds, u32::MAX, &mut scratch);
            let base_set = reached_set(&scratch);
            let mut repeated: Vec<usize> = pose.seeds.iter().flat_map(|&s| [s, s]).collect();
            repeated.push(pose.seeds[0]);
            let stats = seeded(world, &view, &repeated, u32::MAX, &mut scratch);
            assert_eq!(reached_set(&scratch), base_set, "{name}");
            assert_eq!(stats.considered, base.considered, "{name}");

            let single = seeded(world, &view, &pose.seeds[..1], u32::MAX, &mut scratch);
            let single_set = reached_set(&scratch);
            let stats = seeded(world, &view, &[pose.seeds[0]; 3], u32::MAX, &mut scratch);
            assert_eq!(reached_set(&scratch), single_set, "{name}");
            assert_eq!(stats.considered, single.considered, "{name}");
        }
    }
}

// O34: a trip goes back to the caller; what it reached, the uncapped walk reaches.
#[test]
fn seeded_walk_past_its_cap_reports_the_trip_and_keeps_to_uncapped_cells() {
    let (_, grid, water, clearance) = mirror_fixtures().swap_remove(0);
    let world = &grid.world;
    let mut scratch = PortalWalkScratch::new();
    for pose in mirror_poses(&grid, water, clearance, 12) {
        let view = pose.view();

        let empty = seeded(world, &view, &[], 0, &mut scratch);
        assert!(!empty.step_limit_hit && empty.considered == 0 && scratch.reached().is_empty());

        let full = seeded(world, &view, &pose.seeds, u32::MAX, &mut scratch);
        let full_set = reached_set(&scratch);
        assert!(!full.step_limit_hit && full.considered > 4);

        // Exactly the needed tests completes; the caller sees no trip.
        let exact_cap = seeded(world, &view, &pose.seeds, full.considered, &mut scratch);
        assert!(!exact_cap.step_limit_hit);
        assert_eq!(reached_set(&scratch), full_set);

        for cap in [
            1,
            full.considered / 4,
            full.considered / 2,
            full.considered - 1,
        ] {
            let stats = seeded(world, &view, &pose.seeds, cap, &mut scratch);
            assert!(stats.step_limit_hit, "cap {cap} of {}", full.considered);
            assert_eq!(stats.considered, cap);
            assert!(
                !scratch.reached().is_empty(),
                "a tripped walk is not an empty one"
            );
            let stray: Vec<u32> = scratch
                .reached()
                .iter()
                .copied()
                .filter(|c| full_set.binary_search(c).is_err())
                .collect();
            assert!(
                stray.is_empty(),
                "capped walk reached {stray:?} the uncapped one does not"
            );
        }
    }
}

// O35: a seeded walk between two camera walks on one scratch changes neither.
#[test]
fn seeded_walk_between_camera_walks_leaves_the_camera_result_unchanged() {
    for (name, grid, water, clearance) in mirror_fixtures() {
        let world = &grid.world;
        let mirrors = mirror_poses(&grid, water, clearance, 8);
        let cameras = sample_cameras(&grid, mirrors.len(), SWEEP_SEED);
        let mut shared = PortalWalkScratch::new();
        for (pose, mirror) in cameras.iter().zip(&mirrors) {
            let camera =
                WalkView::camera(pose.eye, camera_view_proj(pose.eye, pose.yaw, pose.pitch));
            let mirror_view = mirror.view();

            let mut fresh = PortalWalkScratch::new();
            let fresh_camera =
                walk_from_camera(input(world), &camera, pose.cell, u32::MAX, &mut fresh);
            let fresh_camera_set = reached_set(&fresh);
            let mut fresh = PortalWalkScratch::new();
            let fresh_seeded = seeded(world, &mirror_view, &mirror.seeds, u32::MAX, &mut fresh);
            let fresh_seeded_set = reached_set(&fresh);

            let first = walk_from_camera(input(world), &camera, pose.cell, u32::MAX, &mut shared);
            let first_set = reached_set(&shared);
            let between = seeded(world, &mirror_view, &mirror.seeds, u32::MAX, &mut shared);
            let between_set = reached_set(&shared);
            let second = walk_from_camera(input(world), &camera, pose.cell, u32::MAX, &mut shared);

            assert_eq!(first, fresh_camera, "{name}");
            assert_eq!(first_set, fresh_camera_set, "{name}");
            assert_eq!(between, fresh_seeded, "{name}: seeded counters are its own");
            assert_eq!(between_set, fresh_seeded_set, "{name}");
            assert_eq!(
                second, first,
                "{name}: camera counters unchanged by the seeded walk"
            );
            assert_eq!(reached_set(&shared), first_set, "{name}");
        }
    }
}

// Row 33 (seeded part): the cap keeps 2x headroom over every seeded sweep.
#[test]
fn seeded_walk_cap_keeps_double_headroom_over_seeded_sweeps() {
    let mut scratch = PortalWalkScratch::new();
    for (name, grid, water, clearance) in mirror_fixtures() {
        let world = &grid.world;
        let mut worst = 0;
        for pose in mirror_poses(&grid, water, clearance, 4 * MIRROR_POSES) {
            let stats = seeded(world, &pose.view(), &pose.seeds, u32::MAX, &mut scratch);
            assert!(!stats.step_limit_hit);
            worst = worst.max(stats.considered);
        }
        eprintln!("[region_walk seeded sweep] mirror {name}: worst_tests={worst}");
        assert!(
            WALK_STEP_LIMIT >= 2 * worst,
            "{name}: cap needs 2x headroom over {worst}"
        );
    }
    for (name, grid) in fixtures() {
        let world = &grid.world;
        let mut worst = 0;
        for pose in sample_cameras(&grid, SWEEP_CAMERAS, SWEEP_SEED) {
            let view_proj = camera_view_proj(pose.eye, pose.yaw, pose.pitch);
            let near = WalkView::camera(pose.eye, view_proj).frustum().planes[NEAR_PLANE_INDEX];
            let view = WalkView::seeded(pose.eye, view_proj, near);
            let stats = seeded(world, &view, &[pose.cell], u32::MAX, &mut scratch);
            worst = worst.max(stats.considered);
        }
        eprintln!("[region_walk seeded sweep] camera cell {name}: worst_tests={worst}");
        assert!(
            WALK_STEP_LIMIT >= 2 * worst,
            "{name}: cap needs 2x headroom over {worst}"
        );
    }
}
