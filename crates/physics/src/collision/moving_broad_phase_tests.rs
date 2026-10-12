//! The mover broad phase must never change a query result: every culled query
//! is held to the exhaustive `BROAD_PHASE = false` loop, bit for bit.

use std::cell::Cell;
use std::collections::HashMap;

use glam::{Quat, Vec3};
use parry3d::shape::Capsule as ParryCapsule;
use postretro_entities::Transform;
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};

use super::*;

#[derive(Default)]
struct Poses(HashMap<u32, MoverPose>);

impl Poses {
    fn place(&mut self, mover_id: u32, position: Vec3, rotation: Quat) {
        self.place_moving(mover_id, position, rotation, Vec3::ZERO, Quat::IDENTITY);
    }

    fn place_moving(
        &mut self,
        mover_id: u32,
        position: Vec3,
        rotation: Quat,
        tick_delta: Vec3,
        tick_rotation_delta: Quat,
    ) {
        self.0.insert(
            mover_id,
            MoverPose {
                transform: Transform {
                    position,
                    rotation,
                    scale: Vec3::ONE,
                },
                linear_velocity: tick_delta * 60.0,
                tick_delta,
                angular_velocity: Vec3::ZERO,
                tick_rotation_delta,
                carry_yaw: false,
                tick_dt: 1.0 / 60.0,
            },
        );
    }
}

impl MoverPoseSource for Poses {
    fn pose(&self, mover_id: u32) -> Option<MoverPose> {
        self.0.get(&mover_id).copied()
    }
}

/// Axis-aligned box mesh, `offset` from the mover origin so the authored origin
/// can sit well away from the geometry.
fn box_collider(mover_id: u32, half: Vec3, offset: Vec3) -> MoverCollider {
    let corners: Vec<Vec3> = (0..8)
        .map(|i| {
            offset
                + Vec3::new(
                    if i & 1 == 0 { -half.x } else { half.x },
                    if i & 2 == 0 { -half.y } else { half.y },
                    if i & 4 == 0 { -half.z } else { half.z },
                )
        })
        .collect();
    let triangles = [
        [0, 2, 1],
        [1, 2, 3],
        [4, 5, 6],
        [5, 7, 6],
        [0, 1, 4],
        [1, 5, 4],
        [2, 6, 3],
        [3, 6, 7],
        [0, 4, 2],
        [2, 4, 6],
        [1, 3, 5],
        [3, 7, 5],
    ];
    MoverCollider::from_local_triangles(mover_id, &corners, &triangles).unwrap()
}

fn capsule() -> ParryCapsule {
    ParryCapsule::new_y(0.5, 0.25)
}

struct Query {
    pos: Vec3,
    dir: Vec3,
    max_toi: f32,
}

/// Every combined query, culled and exhaustive. Returns whether any query hit a
/// mover, so callers can confirm a case exercised the narrow phase.
fn assert_all_queries_match(
    world: &CollisionWorld,
    movers: &[MoverCollider],
    poses: &Poses,
    query: &Query,
) -> Result<bool, TestCaseError> {
    let capsule = capsule();
    let Query { pos, dir, max_toi } = *query;

    let culled =
        cast_capsule_combined_with::<true>(world, movers, poses, pos, &capsule, dir, max_toi);
    let full =
        cast_capsule_combined_with::<false>(world, movers, poses, pos, &capsule, dir, max_toi);
    prop_assert_eq!(culled, full, "capsule cast {:?} {:?} {}", pos, dir, max_toi);

    let culled_ray = cast_ray_combined_with::<true>(world, movers, poses, pos, dir, max_toi);
    let full_ray = cast_ray_combined_with::<false>(world, movers, poses, pos, dir, max_toi);
    prop_assert_eq!(culled_ray, full_ray, "ray {:?} {:?} {}", pos, dir, max_toi);

    let culled_pen = deepest_mover_penetration_with::<true>(movers, poses, pos, &capsule);
    let full_pen = deepest_mover_penetration_with::<false>(movers, poses, pos, &capsule);
    prop_assert_eq!(culled_pen, full_pen, "penetration at {:?}", pos);

    let culled_push =
        deepest_mover_push_penetration_inner::<true>(movers, poses, pos, &capsule, None);
    let full_push =
        deepest_mover_push_penetration_inner::<false>(movers, poses, pos, &capsule, None);
    prop_assert_eq!(culled_push, full_push, "push penetration at {:?}", pos);

    let mover_hit = |hit: &Option<CombinedCastHit>| {
        hit.is_some_and(|hit| matches!(hit.source, CollisionSource::Mover(_)))
    };
    Ok(mover_hit(&full) || mover_hit(&full_ray) || full_pen.is_some())
}

fn culled_mover_count(movers: &[MoverCollider], poses: &Poses, query: &Query) -> usize {
    let (center, radius) = capsule_bounding_sphere(&capsule());
    movers
        .iter()
        .filter(|mover| {
            let pose = transform_pose(poses.pose(mover.mover_id).unwrap().transform);
            !mover_bound_may_reach_segment(
                mover,
                &pose,
                query.pos + center,
                query.dir * query.max_toi,
                radius + SKIN_DISTANCE,
            )
        })
        .count()
}

#[test]
fn broad_phase_matches_exhaustive_queries_over_random_movers() {
    let config = Config {
        cases: 512,
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::deterministic_rng(RngAlgorithm::ChaCha));
    let coordinate = -4.5f32..4.5;
    let vector = || (coordinate.clone(), coordinate.clone(), coordinate.clone());
    let mover = (
        (0.05f32..2.5, 0.05f32..2.5, 0.05f32..2.5),
        (-3.0f32..3.0, -3.0f32..3.0, -3.0f32..3.0),
        vector(),
        (-1.0f32..1.0, -1.0f32..1.0, -1.0f32..1.0, -3.2f32..3.2),
        (-0.2f32..0.2, -0.2f32..0.2, -0.2f32..0.2, -0.3f32..0.3),
    );
    let strategy = (
        proptest::collection::vec(mover, 1..=6),
        (-3.0f32..3.0, -3.0f32..3.0, -3.0f32..3.0),
        (-1.0f32..1.0, -1.0f32..1.0, -1.0f32..1.0),
        prop_oneof![9 => 0.0f32..6.0, 1 => Just(0.0f32)],
        prop_oneof![9 => Just(true), 1 => Just(false)],
    );
    let world = CollisionWorld::new();
    let mover_hits = Cell::new(0usize);
    let culls = Cell::new(0usize);

    runner
        .run(&strategy, |(specs, pos, dir, max_toi, moving_dir)| {
            let mut poses = Poses::default();
            let movers: Vec<MoverCollider> = specs
                .iter()
                .enumerate()
                .map(|(index, (half, offset, position, axis_angle, sweep))| {
                    let id = index as u32;
                    let axis = Vec3::new(axis_angle.0, axis_angle.1, axis_angle.2)
                        .try_normalize()
                        .unwrap_or(Vec3::Y);
                    poses.place_moving(
                        id,
                        Vec3::new(position.0, position.1, position.2),
                        Quat::from_axis_angle(axis, axis_angle.3),
                        Vec3::new(sweep.0, sweep.1, sweep.2),
                        Quat::from_axis_angle(axis, sweep.3),
                    );
                    box_collider(
                        id,
                        Vec3::new(half.0, half.1, half.2),
                        Vec3::new(offset.0, offset.1, offset.2),
                    )
                })
                .collect();
            let dir = if moving_dir {
                Vec3::new(dir.0, dir.1, dir.2)
            } else {
                Vec3::ZERO
            };
            let query = Query {
                pos: Vec3::new(pos.0, pos.1, pos.2),
                dir,
                max_toi,
            };
            if assert_all_queries_match(&world, &movers, &poses, &query)? {
                mover_hits.set(mover_hits.get() + 1);
            }
            culls.set(culls.get() + culled_mover_count(&movers, &poses, &query));
            Ok(())
        })
        .unwrap();

    // Both sides of the predicate must have been exercised for the equality to
    // mean anything: queries the narrow phase answered, and movers it skipped.
    assert!(
        mover_hits.get() > 64,
        "only {} mover hits",
        mover_hits.get()
    );
    assert!(culls.get() > 64, "only {} culled movers", culls.get());
}

#[test]
fn contacts_at_the_max_toi_boundary_survive_the_cull() {
    let world = CollisionWorld::new();
    let movers = [box_collider(1, Vec3::new(2.0, 0.1, 2.0), Vec3::ZERO)];
    let mut poses = Poses::default();
    poses.place(1, Vec3::new(0.0, -3.0, 0.0), Quat::IDENTITY);
    let capsule = capsule();
    let probe = cast_capsule_combined_with::<false>(
        &world,
        &movers,
        &poses,
        Vec3::ZERO,
        &capsule,
        Vec3::NEG_Y,
        10.0,
    )
    .expect("capsule lands on the mover");
    let toi = probe.time_of_impact;

    // Straddle the boundary: limits just short of, at, and just past the hit.
    let mut hits = 0;
    for max_toi in [
        toi - 1.0e-3,
        toi - 1.0e-6,
        toi,
        toi.next_up(),
        toi + 1.0e-6,
        toi + 1.0e-3,
    ] {
        let query = Query {
            pos: Vec3::ZERO,
            dir: Vec3::NEG_Y,
            max_toi,
        };
        assert_eq!(culled_mover_count(&movers, &poses, &query), 0);
        if assert_all_queries_match(&world, &movers, &poses, &query).unwrap() {
            hits += 1;
        }
    }
    assert!(hits > 0, "the sweep past the hit must report it");
}

#[test]
fn skin_band_contacts_survive_the_cull() {
    let world = CollisionWorld::new();
    let movers = [box_collider(1, Vec3::new(2.0, 0.1, 2.0), Vec3::ZERO)];
    let mut poses = Poses::default();
    poses.place(1, Vec3::ZERO, Quat::IDENTITY);
    let capsule = capsule();
    let bottom = capsule.segment.a.y.min(capsule.segment.b.y) - capsule.radius;
    let resting = 0.1 - bottom;

    // Hovering inside the skin band, sliding sideways and probing down.
    for (offset, dir, max_toi) in [
        (SKIN_DISTANCE * 0.5, Vec3::X, 1.0),
        (SKIN_DISTANCE * 0.5, Vec3::NEG_Y, 0.1),
        (SKIN_DISTANCE * 0.5, Vec3::X, 0.0),
    ] {
        let query = Query {
            pos: Vec3::new(0.0, resting + offset, 0.0),
            dir,
            max_toi,
        };
        assert_eq!(culled_mover_count(&movers, &poses, &query), 0);
        assert_all_queries_match(&world, &movers, &poses, &query).unwrap();
    }

    // Overlapping by half a skin: the penetration query must still see it.
    let pos = Vec3::new(0.0, resting - SKIN_DISTANCE * 0.5, 0.0);
    assert!(deepest_mover_penetration_with::<true>(&movers, &poses, pos, &capsule).is_some());
    assert_all_queries_match(
        &world,
        &movers,
        &poses,
        &Query {
            pos,
            dir: Vec3::ZERO,
            max_toi: 0.0,
        },
    )
    .unwrap();
}

#[test]
fn geometry_far_from_the_mover_origin_is_tested_where_it_is() {
    // The mesh sits 40 m from its authored origin; the origin is far from the
    // query, the geometry is under it.
    let world = CollisionWorld::new();
    let movers = [box_collider(
        1,
        Vec3::new(1.0, 0.1, 1.0),
        Vec3::new(40.0, 0.0, 0.0),
    )];
    let mut poses = Poses::default();
    poses.place(1, Vec3::new(-40.0, -2.0, 0.0), Quat::IDENTITY);
    let query = Query {
        pos: Vec3::ZERO,
        dir: Vec3::NEG_Y,
        max_toi: 5.0,
    };
    assert_eq!(culled_mover_count(&movers, &poses, &query), 0);
    let hit = cast_capsule_combined_with::<true>(
        &world,
        &movers,
        &poses,
        query.pos,
        &capsule(),
        query.dir,
        query.max_toi,
    );
    assert_eq!(hit.map(|hit| hit.source), Some(CollisionSource::Mover(1)));
    assert_all_queries_match(&world, &movers, &poses, &query).unwrap();
}

#[test]
fn rotated_mover_geometry_swings_into_the_query() {
    // A long arm offset along +X, rotated a quarter turn so it lies along -Z
    // under the query. Unrotated, the same mover would be culled.
    let world = CollisionWorld::new();
    let movers = [box_collider(
        1,
        Vec3::new(3.0, 0.1, 0.5),
        Vec3::new(4.0, 0.0, 0.0),
    )];
    let mut poses = Poses::default();
    let query = Query {
        pos: Vec3::new(0.0, 0.0, -4.0),
        dir: Vec3::NEG_Y,
        max_toi: 3.0,
    };

    poses.place(1, Vec3::new(0.0, -2.0, 0.0), Quat::IDENTITY);
    assert_eq!(culled_mover_count(&movers, &poses, &query), 1);
    assert_all_queries_match(&world, &movers, &poses, &query).unwrap();

    poses.place(
        1,
        Vec3::new(0.0, -2.0, 0.0),
        Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
    );
    assert_eq!(culled_mover_count(&movers, &poses, &query), 0);
    let hit = cast_capsule_combined_with::<true>(
        &world,
        &movers,
        &poses,
        query.pos,
        &capsule(),
        query.dir,
        query.max_toi,
    );
    assert_eq!(hit.map(|hit| hit.source), Some(CollisionSource::Mover(1)));
    assert_all_queries_match(&world, &movers, &poses, &query).unwrap();
}

#[test]
fn bound_test_rejects_just_outside_reach_and_admits_just_inside() {
    let mover = box_collider(1, Vec3::splat(1.0), Vec3::ZERO);
    let pose = Pose::from_translation(Vec3::ZERO);
    let query_radius = 0.5;
    let slack = BROAD_PHASE_MARGIN + 10.0 * BROAD_PHASE_RELATIVE_MARGIN;
    let reach = mover.bound_radius + query_radius + slack;
    // The sweep passes the sphere sideways, offset along +Z.
    let start = Vec3::new(-10.0, 0.0, 0.0);
    let motion = Vec3::new(20.0, 0.0, 0.0);

    let inside = Vec3::new(0.0, 0.0, reach - 1.0e-3);
    let outside = Vec3::new(0.0, 0.0, reach + 1.0e-2);
    assert!(mover_bound_may_reach_segment(
        &mover,
        &pose,
        start + inside,
        motion,
        query_radius
    ));
    assert!(!mover_bound_may_reach_segment(
        &mover,
        &pose,
        start + outside,
        motion,
        query_radius
    ));

    // Segment ends short of the sphere: rejected; extended to reach it: admitted.
    let toward = Vec3::new(1.0, 0.0, 0.0);
    let near_start = Vec3::new(-(reach + 2.0), 0.0, 0.0);
    assert!(!mover_bound_may_reach_segment(
        &mover,
        &pose,
        near_start,
        toward * 1.9,
        query_radius
    ));
    assert!(mover_bound_may_reach_segment(
        &mover,
        &pose,
        near_start,
        toward * 2.1,
        query_radius
    ));

    // A zero-length query is a point test.
    assert!(mover_bound_may_reach_segment(
        &mover,
        &pose,
        inside,
        Vec3::ZERO,
        query_radius
    ));
    assert!(!mover_bound_may_reach_segment(
        &mover,
        &pose,
        outside,
        Vec3::ZERO,
        query_radius
    ));
}

#[test]
fn non_finite_queries_fall_through_to_the_narrow_phase() {
    let mover = box_collider(1, Vec3::splat(1.0), Vec3::ZERO);
    let pose = Pose::from_translation(Vec3::new(100.0, 0.0, 0.0));
    let far = Vec3::new(-100.0, 0.0, 0.0);
    assert!(mover_bound_may_reach_segment(
        &mover,
        &pose,
        Vec3::NAN,
        Vec3::X,
        0.5
    ));
    assert!(mover_bound_may_reach_segment(
        &mover,
        &pose,
        far,
        Vec3::splat(f32::INFINITY),
        0.5
    ));
    assert!(mover_bound_may_reach_segment(
        &mover,
        &pose,
        far,
        Vec3::X,
        f32::NAN
    ));

    let world = CollisionWorld::new();
    let movers = [mover];
    let mut poses = Poses::default();
    poses.place(1, Vec3::new(100.0, 0.0, 0.0), Quat::IDENTITY);
    for query in [
        Query {
            pos: Vec3::NAN,
            dir: Vec3::X,
            max_toi: 1.0,
        },
        Query {
            pos: far,
            dir: Vec3::X,
            max_toi: f32::INFINITY,
        },
    ] {
        assert_all_queries_match(&world, &movers, &poses, &query).unwrap();
    }
}

#[test]
fn unbounded_finite_queries_fall_through_instead_of_overflowing() {
    // `f32::MAX` is the usual "no limit": the swept length overflows inside
    // the bound test, which must admit the mover rather than reject on NaN.
    let mover = box_collider(1, Vec3::splat(1.0), Vec3::ZERO);
    let pose = Pose::from_translation(Vec3::new(100.0, 0.0, 0.0));
    let far = Vec3::new(-100.0, 0.0, 0.0);
    for max_toi in [f32::MAX, 1.0e37, 1.0e20] {
        assert!(mover_bound_may_reach_segment(
            &mover,
            &pose,
            far,
            Vec3::X * max_toi,
            0.5
        ));
    }

    let world = CollisionWorld::new();
    let movers = [mover];
    let mut poses = Poses::default();
    poses.place(1, Vec3::new(100.0, 0.0, 0.0), Quat::IDENTITY);
    for max_toi in [f32::MAX, 1.0e37] {
        let query = Query {
            pos: far,
            dir: Vec3::X,
            max_toi,
        };
        assert_all_queries_match(&world, &movers, &poses, &query).unwrap();
        let ray = cast_ray_combined_with::<true>(&world, &movers, &poses, far, Vec3::X, max_toi);
        assert_eq!(ray.map(|hit| hit.source), Some(CollisionSource::Mover(1)));
    }
}

/// A 2 mm triangle: its bounding sphere is nearly a point, so the cull's reach
/// is set by the capsule sphere and the slack alone.
fn speck_collider(mover_id: u32) -> MoverCollider {
    MoverCollider::from_local_triangles(
        mover_id,
        &[
            Vec3::new(-0.001, 0.0, -0.001),
            Vec3::new(0.001, 0.0, -0.001),
            Vec3::new(0.0, 0.0, 0.001),
        ],
        &[[0, 2, 1]],
    )
    .unwrap()
}

#[test]
fn sweep_ending_inside_the_skin_band_is_never_culled() {
    // The capsule's bottom pole sits on its bounding sphere, straight above the
    // speck, so the two spheres meet where the shapes do. The sweep ends three
    // quarters of a skin above the speck: the narrow phase reports the band
    // hit, and the cull must reach that far. Its absolute margin alone is
    // smaller than this gap, so dropping the skin term from the reach fails.
    let world = CollisionWorld::new();
    let movers = [speck_collider(1)];
    let mut poses = Poses::default();
    poses.place(1, Vec3::ZERO, Quat::IDENTITY);
    let capsule = capsule();
    let (_, capsule_radius) = capsule_bounding_sphere(&capsule);
    let end_gap = SKIN_DISTANCE * 0.75;
    assert!(end_gap > BROAD_PHASE_MARGIN);
    let start_height = capsule_radius + 0.5;
    let query = Query {
        pos: Vec3::new(0.0, start_height, 0.0),
        dir: Vec3::NEG_Y,
        max_toi: start_height - capsule_radius - end_gap,
    };

    assert_eq!(culled_mover_count(&movers, &poses, &query), 0);
    let hit = cast_capsule_combined_with::<true>(
        &world,
        &movers,
        &poses,
        query.pos,
        &capsule,
        query.dir,
        query.max_toi,
    );
    assert_eq!(hit.map(|hit| hit.source), Some(CollisionSource::Mover(1)));
    assert_all_queries_match(&world, &movers, &poses, &query).unwrap();

    // Past the skin plus slack, the same speck is culled.
    let short = Query {
        max_toi: start_height - capsule_radius - SKIN_DISTANCE - 2.0 * BROAD_PHASE_MARGIN,
        ..query
    };
    assert_eq!(culled_mover_count(&movers, &poses, &short), 1);
    assert_all_queries_match(&world, &movers, &poses, &short).unwrap();
}

/// Manual measurement, not a gate: per-call cost of each combined query with
/// 12 far movers (512-triangle grids), culled vs exhaustive.
/// `cargo test -p postretro-physics --release --lib mover_query_cost -- --ignored --nocapture --test-threads=1`
#[test]
#[ignore]
fn mover_query_cost() {
    use std::hint::black_box;
    use std::time::Instant;

    fn grid(mover_id: u32, n: u32) -> MoverCollider {
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        for z in 0..=n {
            for x in 0..=n {
                vertices.push(Vec3::new(
                    x as f32 * 0.25 - 1.0,
                    (x + z) as f32 * 0.01,
                    z as f32 * 0.25 - 1.0,
                ));
            }
        }
        for z in 0..n {
            for x in 0..n {
                let i = z * (n + 1) + x;
                triangles.push([i, i + 1, i + n + 2]);
                triangles.push([i, i + n + 2, i + n + 1]);
            }
        }
        MoverCollider::from_local_triangles(mover_id, &vertices, &triangles).unwrap()
    }

    let world = CollisionWorld::new();
    let capsule = capsule();
    let movers: Vec<MoverCollider> = (0..12).map(|id| grid(id, 16)).collect();
    let mut poses = Poses::default();
    for id in 0..12u32 {
        poses.place(
            id,
            Vec3::new(100.0 + id as f32 * 20.0, 3.0, 50.0),
            Quat::IDENTITY,
        );
    }
    let pos = Vec3::new(0.0, 1.5, 0.0);
    let dir = Vec3::new(0.3, -1.0, 0.1).normalize();
    const N: u32 = 200_000;
    let time = |label: &str, query: &dyn Fn() -> bool| {
        let mut found = 0usize;
        for _ in 0..N / 10 {
            found += query() as usize;
        }
        let start = Instant::now();
        for _ in 0..N {
            found += query() as usize;
        }
        let ns = start.elapsed().as_nanos() as f64 / f64::from(N);
        println!("{label:<32} {ns:8.1} ns/call ({found})");
    };
    time("capsule, no movers", &|| {
        black_box(cast_capsule_combined_with::<true>(
            &world,
            &[],
            &poses,
            pos,
            &capsule,
            dir,
            0.5,
        ))
        .is_some()
    });
    time("capsule, 12 far, exhaustive", &|| {
        black_box(cast_capsule_combined_with::<false>(
            &world, &movers, &poses, pos, &capsule, dir, 0.5,
        ))
        .is_some()
    });
    time("capsule, 12 far, culled", &|| {
        black_box(cast_capsule_combined_with::<true>(
            &world, &movers, &poses, pos, &capsule, dir, 0.5,
        ))
        .is_some()
    });
    time("ray, 12 far, exhaustive", &|| {
        black_box(cast_ray_combined_with::<false>(
            &world, &movers, &poses, pos, dir, 2.0,
        ))
        .is_some()
    });
    time("ray, 12 far, culled", &|| {
        black_box(cast_ray_combined_with::<true>(
            &world, &movers, &poses, pos, dir, 2.0,
        ))
        .is_some()
    });
    time("penetration, 12 far, exhaustive", &|| {
        black_box(deepest_mover_penetration_with::<false>(
            &movers, &poses, pos, &capsule,
        ))
        .is_some()
    });
    time("penetration, 12 far, culled", &|| {
        black_box(deepest_mover_penetration_with::<true>(
            &movers, &poses, pos, &capsule,
        ))
        .is_some()
    });
    time("push pen., 12 far, exhaustive", &|| {
        black_box(deepest_mover_push_penetration_inner::<false>(
            &movers, &poses, pos, &capsule, None,
        ))
        .is_some()
    });
    time("push pen., 12 far, culled", &|| {
        black_box(deepest_mover_push_penetration_inner::<true>(
            &movers, &poses, pos, &capsule, None,
        ))
        .is_some()
    });
}
