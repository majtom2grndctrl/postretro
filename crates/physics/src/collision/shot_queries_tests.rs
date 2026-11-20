// Combined shot and sight queries against an oracle: the same posed geometry
// baked into one static world.
use super::*;
use crate::collision::moving::{
    CollisionSource, MoverCollider, MoverPose, MoverPoseSource, cast_ray_combined,
};
use crate::collision::{CollisionWorld, cast_sphere_exact, line_of_sight};
use glam::Quat;
use postretro_entities::Transform;
use std::collections::HashMap;

const EPS: f32 = 1.0e-4;

#[derive(Default)]
struct Poses(HashMap<u32, Transform>);

impl Poses {
    fn place(mut self, mover_id: u32, position: Vec3, rotation: Quat) -> Self {
        self.0.insert(
            mover_id,
            Transform {
                position,
                rotation,
                scale: Vec3::ONE,
            },
        );
        self
    }
}

impl MoverPoseSource for Poses {
    fn pose(&self, mover_id: u32) -> Option<MoverPose> {
        self.0.get(&mover_id).map(|transform| MoverPose {
            transform: *transform,
            linear_velocity: Vec3::ZERO,
            tick_delta: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            tick_rotation_delta: Quat::IDENTITY,
            carry_yaw: false,
            tick_dt: 0.1,
        })
    }
}

/// A closed door slab: 0.2 thick along X, 2 tall from its origin, 2 wide.
fn slab() -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let (x, y0, y1, z) = (0.1, 0.0, 2.0, 1.0);
    let vertices = vec![
        Vec3::new(-x, y0, -z),
        Vec3::new(x, y0, -z),
        Vec3::new(x, y1, -z),
        Vec3::new(-x, y1, -z),
        Vec3::new(-x, y0, z),
        Vec3::new(x, y0, z),
        Vec3::new(x, y1, z),
        Vec3::new(-x, y1, z),
    ];
    let triangles = vec![
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [3, 6, 2],
        [3, 7, 6],
        [0, 4, 7],
        [0, 7, 3],
        [1, 2, 6],
        [1, 6, 5],
    ];
    (vertices, triangles)
}

fn slab_collider(mover_id: u32) -> MoverCollider {
    let (vertices, triangles) = slab();
    MoverCollider::from_local_triangles(mover_id, &vertices, &triangles).unwrap()
}

/// A back wall at x = 10, spanning the shot fixtures.
fn wall_triangles() -> (Vec<Vec3>, Vec<[u32; 3]>) {
    (
        vec![
            Vec3::new(10.0, -5.0, -5.0),
            Vec3::new(10.0, 5.0, -5.0),
            Vec3::new(10.0, 5.0, 5.0),
            Vec3::new(10.0, -5.0, 5.0),
        ],
        vec![[0, 1, 2], [0, 2, 3]],
    )
}

fn wall_world() -> CollisionWorld {
    let (vertices, triangles) = wall_triangles();
    CollisionWorld::from_triangles_for_test(vertices, triangles)
}

/// The wall plus every slab baked at its pose.
fn oracle(poses: &Poses, movers: &[MoverCollider]) -> CollisionWorld {
    let (mut vertices, mut triangles) = wall_triangles();
    let (slab_vertices, slab_triangles) = slab();
    for mover in movers {
        let Some(transform) = poses.0.get(&mover.mover_id()) else {
            continue;
        };
        let base = vertices.len() as u32;
        vertices.extend(
            slab_vertices
                .iter()
                .map(|vertex| transform.position + transform.rotation * *vertex),
        );
        triangles.extend(
            slab_triangles
                .iter()
                .map(|triangle| triangle.map(|index| index + base)),
        );
    }
    CollisionWorld::from_triangles_for_test(vertices, triangles)
}

struct Case {
    name: &'static str,
    poses: Poses,
    movers: Vec<MoverCollider>,
    eye: Vec3,
    aims: Vec<Vec3>,
}

fn cases() -> Vec<Case> {
    let eye = Vec3::new(0.0, 1.0, 0.0);
    let behind = |y: f32| Vec3::new(8.0, y, 0.0);
    vec![
        Case {
            name: "closed",
            poses: Poses::default().place(1, Vec3::new(5.0, 0.0, 0.0), Quat::IDENTITY),
            movers: vec![slab_collider(1)],
            eye,
            aims: vec![
                behind(1.0),
                Vec3::new(12.0, 1.0, 0.0),
                Vec3::new(4.0, 1.0, 0.0),
            ],
        },
        Case {
            name: "open gap",
            poses: Poses::default().place(1, Vec3::new(5.0, 3.0, 0.0), Quat::IDENTITY),
            movers: vec![slab_collider(1)],
            eye,
            aims: vec![behind(1.0), Vec3::new(12.0, 1.0, 0.0)],
        },
        Case {
            name: "partly open",
            poses: Poses::default().place(1, Vec3::new(5.0, 1.5, 0.0), Quat::IDENTITY),
            movers: vec![slab_collider(1)],
            eye: Vec3::new(0.0, 0.5, 0.0),
            aims: vec![behind(0.5), behind(2.5), Vec3::new(12.0, 0.5, 0.0)],
        },
        Case {
            name: "rotated",
            poses: Poses::default().place(
                1,
                Vec3::new(5.0, 0.0, 0.0),
                Quat::from_rotation_y(35.0_f32.to_radians()),
            ),
            movers: vec![slab_collider(1)],
            eye,
            aims: vec![
                behind(1.0),
                Vec3::new(8.0, 1.0, 3.0),
                Vec3::new(8.0, 1.0, -1.5),
            ],
        },
        Case {
            name: "two movers on one line",
            poses: Poses::default()
                .place(1, Vec3::new(7.0, 0.0, 0.0), Quat::IDENTITY)
                .place(2, Vec3::new(5.0, 0.0, 0.0), Quat::IDENTITY),
            movers: vec![slab_collider(1), slab_collider(2)],
            eye,
            aims: vec![behind(1.0), Vec3::new(6.0, 1.0, 0.0)],
        },
        Case {
            name: "origin inside a mover hull",
            poses: Poses::default().place(1, Vec3::new(0.0, 0.0, 0.0), Quat::IDENTITY),
            movers: vec![slab_collider(1)],
            eye,
            aims: vec![
                behind(1.0),
                Vec3::new(-8.0, 1.0, 0.0),
                Vec3::new(0.05, 1.0, 0.0),
            ],
        },
    ]
}

fn assert_hits_match(
    case: &str,
    combined: Option<CombinedCastHit>,
    oracle: Option<(f32, Vec3)>,
    what: &str,
) {
    match (combined, oracle) {
        (None, None) => {}
        (Some(hit), Some((toi, normal))) => {
            assert!(
                (hit.time_of_impact - toi).abs() < EPS,
                "{case}: {what} toi {} vs oracle {toi}",
                hit.time_of_impact
            );
            assert!(
                (hit.normal - normal).length() < 1.0e-3,
                "{case}: {what} normal {:?} vs oracle {normal:?}",
                hit.normal
            );
        }
        (combined, oracle) => panic!("{case}: {what} combined {combined:?} vs oracle {oracle:?}"),
    }
}

#[test]
fn combined_queries_return_the_posed_geometry_result() {
    let static_world = wall_world();
    for case in cases() {
        let baked = oracle(&case.poses, &case.movers);
        let world = CombinedCollisionWorld::new(&static_world, &case.movers, &case.poses);
        for aim in &case.aims {
            assert_eq!(
                line_of_sight_combined(case.eye, *aim, &world),
                line_of_sight(case.eye, *aim, &baked),
                "{}: line of sight to {aim:?}",
                case.name
            );
            let dir = (*aim - case.eye).normalize();
            for radius in [0.0, 0.05] {
                assert_hits_match(
                    case.name,
                    cast_sphere_exact_combined(&world, case.eye, radius, dir, 20.0),
                    cast_sphere_exact(&baked, case.eye, radius, dir, 20.0)
                        .map(|hit| (hit.time_of_impact, hit.normal)),
                    &format!("sphere r={radius} toward {aim:?}"),
                );
            }
        }
    }
}

#[test]
fn closed_mover_blocks_at_its_near_face_and_reports_a_mover_source() {
    let static_world = wall_world();
    let movers = [slab_collider(1)];
    let poses = Poses::default().place(1, Vec3::new(5.0, 0.0, 0.0), Quat::IDENTITY);
    let world = CombinedCollisionWorld::new(&static_world, &movers, &poses);
    let hit = cast_sphere_exact_combined(&world, Vec3::Y, 0.0, Vec3::X, 20.0).unwrap();
    assert!((hit.time_of_impact - 4.9).abs() < EPS);
    assert_eq!(hit.source(), CollisionSource::Mover(1));
    assert!(hit.is_mover());

    let open = Poses::default().place(1, Vec3::new(5.0, 3.0, 0.0), Quat::IDENTITY);
    let world = CombinedCollisionWorld::new(&static_world, &movers, &open);
    let hit = cast_sphere_exact_combined(&world, Vec3::Y, 0.0, Vec3::X, 20.0).unwrap();
    assert!((hit.time_of_impact - 10.0).abs() < EPS);
    assert_eq!(hit.source(), CollisionSource::Static);
}

#[test]
fn combined_queries_match_static_queries_exactly_without_movers() {
    let static_world = wall_world();
    let world = CombinedCollisionWorld::static_only(&static_world);
    let eye = Vec3::new(0.0, 1.0, 0.0);
    // A hit at the aim point is clear; a degenerate segment is blocked.
    for aim in [
        Vec3::new(10.0, 1.0, 0.0),
        Vec3::new(12.0, 1.0, 0.0),
        Vec3::new(6.0, 2.0, 1.0),
        eye,
        eye + Vec3::splat(1.0e-7),
    ] {
        assert_eq!(
            line_of_sight_combined(eye, aim, &world),
            line_of_sight(eye, aim, &static_world),
            "aim {aim:?}"
        );
    }
    assert!(line_of_sight_combined(
        eye,
        Vec3::new(10.0, 1.0, 0.0),
        &world
    ));
    assert!(!line_of_sight_combined(eye, eye, &world));

    for (origin, radius) in [
        (eye, 0.0),
        (eye, 0.25),
        // On the wall face: the static sweep passes its start surface.
        (Vec3::new(10.0, 1.0, 0.0), 0.0),
    ] {
        let combined = cast_sphere_exact_combined(&world, origin, radius, Vec3::X, 20.0);
        let direct = cast_sphere_exact(&static_world, origin, radius, Vec3::X, 20.0);
        assert_eq!(
            combined.map(|hit| (
                hit.time_of_impact.to_bits(),
                hit.normal.to_array().map(f32::to_bits)
            )),
            direct.map(|hit| (
                hit.time_of_impact.to_bits(),
                hit.normal.to_array().map(f32::to_bits)
            )),
            "origin {origin:?} radius {radius}"
        );
    }
}

#[test]
fn zero_radius_combined_sphere_cast_returns_the_combined_ray_hit() {
    let static_world = wall_world();
    for case in cases() {
        let world = CombinedCollisionWorld::new(&static_world, &case.movers, &case.poses);
        for aim in &case.aims {
            let dir = (*aim - case.eye).normalize();
            let ray = cast_ray_combined(
                &static_world,
                &case.movers,
                &case.poses,
                case.eye,
                dir,
                20.0,
            );
            assert_hits_match(
                case.name,
                cast_sphere_exact_combined(&world, case.eye, 0.0, dir, 20.0),
                ray.map(|hit| (hit.time_of_impact, hit.normal)),
                &format!("zero-radius sphere vs ray toward {aim:?}"),
            );
        }
    }
}

#[test]
fn mover_at_sphere_reports_enclosure_and_contact_but_not_clearance() {
    let static_world = wall_world();
    let movers = [slab_collider(7)];
    let poses = Poses::default().place(7, Vec3::new(5.0, 0.0, 0.0), Quat::IDENTITY);
    let world = CombinedCollisionWorld::new(&static_world, &movers, &poses);
    assert_eq!(
        mover_at_sphere(&world, Vec3::new(5.0, 1.0, 0.0), 0.0),
        Some(7)
    );
    assert_eq!(
        mover_at_sphere(&world, Vec3::new(5.0, 1.0, 0.0), 0.05),
        Some(7)
    );
    assert_eq!(
        mover_at_sphere(&world, Vec3::new(4.85, 1.0, 0.0), 0.1),
        Some(7)
    );
    assert_eq!(mover_at_sphere(&world, Vec3::new(4.5, 1.0, 0.0), 0.1), None);
    assert_eq!(mover_at_sphere(&world, Vec3::new(5.0, 3.0, 0.0), 0.0), None);
}

#[test]
fn mover_surface_distance_finds_the_exit_from_inside() {
    let static_world = wall_world();
    let movers = [slab_collider(7)];
    let poses = Poses::default().place(7, Vec3::new(5.0, 0.0, 0.0), Quat::IDENTITY);
    let world = CombinedCollisionWorld::new(&static_world, &movers, &poses);
    let back = mover_surface_distance(&world, 7, Vec3::new(5.02, 1.0, 0.0), -Vec3::X, 5.0).unwrap();
    assert!((back - 0.12).abs() < EPS, "{back}");
    assert_eq!(
        mover_surface_distance(&world, 8, Vec3::new(5.0, 1.0, 0.0), -Vec3::X, 5.0),
        None
    );
}

#[test]
fn combined_queries_do_not_allocate() {
    use postretro_sim::alloc_probe::AllocSnapshot;

    let static_world = wall_world();
    let movers = [slab_collider(1), slab_collider(2)];
    let poses = Poses::default()
        .place(1, Vec3::new(5.0, 0.0, 0.0), Quat::IDENTITY)
        .place(2, Vec3::new(7.0, 0.0, 0.0), Quat::from_rotation_y(0.4));
    let world = CombinedCollisionWorld::new(&static_world, &movers, &poses);
    let eye = Vec3::new(0.0, 1.0, 0.0);
    // Warm any lazily built parry state before arming.
    let _ = cast_sphere_exact_combined(&world, eye, 0.1, Vec3::X, 20.0);
    let _ = line_of_sight_combined(eye, Vec3::new(8.0, 1.0, 0.0), &world);

    let snapshot = AllocSnapshot::arm();
    let mut blocked = 0;
    for step in 0..32 {
        let aim = Vec3::new(8.0, 0.1 * step as f32, 0.0);
        blocked += usize::from(!line_of_sight_combined(eye, aim, &world));
        blocked += usize::from(
            cast_sphere_exact_combined(&world, eye, 0.05, (aim - eye).normalize(), 20.0).is_some(),
        );
        blocked += usize::from(mover_at_sphere(&world, aim, 0.05).is_some());
    }
    let allocs = snapshot.allocs_since();
    std::hint::black_box(blocked);
    assert_eq!(allocs, 0, "combined shot queries allocated {allocs} times");
}
