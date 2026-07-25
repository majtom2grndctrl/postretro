//! Combined static-world plus kinematic-mover collision queries.
//!
//! This is deliberately hand-rolled over parry free functions. There is no
//! scene/query pipeline here: callers provide the static world, active mover
//! colliders, and a pose source for the mover transform at the query tick.

use glam::{Quat, Vec3};
use parry3d::math::{Isometry, Point, Vector};
use parry3d::na::{Quaternion, Translation3, UnitQuaternion};
use parry3d::query::{
    Ray, RayCast, RayIntersection, ShapeCastHit, ShapeCastOptions, cast_shapes, contact,
};
use parry3d::shape::{Capsule, TriMesh};

use super::{COS_WALKABLE, CollisionWorld, SKIN_DISTANCE, cast_capsule, cast_ray};
use postretro_entities::Transform;

const HIT_TOI_TIE_EPSILON: f32 = 1.0e-5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CollisionSource {
    Static,
    Mover(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContactClassification {
    Floor,
    Wall,
    Ceiling,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CombinedCastHit {
    pub(crate) time_of_impact: f32,
    pub(crate) normal: Vec3,
    pub(crate) source: CollisionSource,
    pub(crate) mover_id: Option<u32>,
    pub(crate) mover_linear_velocity: Vec3,
    pub(crate) mover_tick_delta: Vec3,
    pub(crate) mover_tick_dt: f32,
    pub(crate) classification: ContactClassification,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MoverPose {
    pub(crate) transform: Transform,
    pub(crate) linear_velocity: Vec3,
    pub(crate) tick_delta: Vec3,
    pub(crate) angular_velocity: Vec3,
    pub(crate) tick_rotation_delta: Quat,
    pub(crate) carry_yaw: bool,
    pub(crate) tick_dt: f32,
}

pub(crate) trait MoverPoseSource {
    fn pose(&self, mover_id: u32) -> Option<MoverPose>;
}

#[derive(Debug, Default)]
pub(crate) struct EmptyMoverPoseSource;

impl MoverPoseSource for EmptyMoverPoseSource {
    fn pose(&self, _mover_id: u32) -> Option<MoverPose> {
        None
    }
}

pub(crate) static EMPTY_MOVER_POSES: EmptyMoverPoseSource = EmptyMoverPoseSource;

#[derive(Clone, Copy)]
pub(crate) struct CombinedCollisionWorld<'a> {
    pub(crate) static_world: &'a CollisionWorld,
    pub(crate) movers: &'a [MoverCollider],
    pub(crate) poses: &'a dyn MoverPoseSource,
}

impl<'a> CombinedCollisionWorld<'a> {
    pub(crate) fn new(
        static_world: &'a CollisionWorld,
        movers: &'a [MoverCollider],
        poses: &'a dyn MoverPoseSource,
    ) -> Self {
        Self {
            static_world,
            movers,
            poses,
        }
    }

    pub(crate) fn static_only(static_world: &'a CollisionWorld) -> Self {
        Self::new(static_world, &[], &EMPTY_MOVER_POSES)
    }
}

#[derive(Debug)]
pub(crate) struct MoverCollider {
    pub(crate) mover_id: u32,
    local_mesh: TriMesh,
}

impl MoverCollider {
    pub(crate) fn from_local_triangles(
        mover_id: u32,
        vertices: &[Vec3],
        triangles: &[[u32; 3]],
    ) -> Option<Self> {
        if vertices.is_empty() || triangles.is_empty() {
            return None;
        }
        let points = vertices.iter().map(|v| Point::new(v.x, v.y, v.z)).collect();
        Some(Self {
            mover_id,
            local_mesh: TriMesh::new(points, triangles.to_vec()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MoverPenetration {
    pub(crate) mover_id: u32,
    pub(crate) normal: Vec3,
    pub(crate) depth: f32,
}

pub(crate) fn cast_capsule_combined(
    static_world: &CollisionWorld,
    movers: &[MoverCollider],
    poses: &(impl MoverPoseSource + ?Sized),
    pos: Point<f32>,
    capsule: &Capsule,
    dir: Vector<f32>,
    max_toi: f32,
) -> Option<CombinedCastHit> {
    let mut nearest = cast_capsule(static_world, pos, capsule, dir, max_toi).map(static_shape_hit);

    for mover in movers {
        let Some(pose) = poses.pose(mover.mover_id) else {
            continue;
        };
        let mover_iso = transform_isometry(pose.transform);
        let options = ShapeCastOptions {
            max_time_of_impact: max_toi,
            target_distance: SKIN_DISTANCE,
            stop_at_penetration: false,
            ..Default::default()
        };
        let hit = cast_shapes(
            &Isometry::translation(pos.x, pos.y, pos.z),
            &dir,
            capsule,
            &mover_iso,
            &Vector::zeros(),
            &mover.local_mesh,
            options,
        )
        .ok()
        .flatten()
        .map(|hit| mover_shape_hit(hit, mover.mover_id, pose));
        choose_nearest(&mut nearest, hit);
    }

    nearest
}

pub(crate) fn cast_ray_combined(
    static_world: &CollisionWorld,
    movers: &[MoverCollider],
    poses: &(impl MoverPoseSource + ?Sized),
    origin: Point<f32>,
    dir: Vector<f32>,
    max_toi: f32,
) -> Option<CombinedCastHit> {
    let mut nearest = cast_ray(static_world, origin, dir, max_toi).map(static_ray_hit);
    let ray = Ray::new(origin, dir);

    for mover in movers {
        let Some(pose) = poses.pose(mover.mover_id) else {
            continue;
        };
        let mover_iso = transform_isometry(pose.transform);
        let hit = mover
            .local_mesh
            .cast_ray_and_get_normal(&mover_iso, &ray, max_toi, true)
            .map(|hit| mover_ray_hit(hit, mover.mover_id, pose));
        choose_nearest(&mut nearest, hit);
    }

    nearest
}

pub(crate) fn deepest_mover_penetration(
    movers: &[MoverCollider],
    poses: &(impl MoverPoseSource + ?Sized),
    pos: Point<f32>,
    capsule: &Capsule,
) -> Option<MoverPenetration> {
    let capsule_iso = Isometry::translation(pos.x, pos.y, pos.z);
    let mut deepest: Option<MoverPenetration> = None;

    for mover in movers {
        let Some(pose) = poses.pose(mover.mover_id) else {
            continue;
        };
        let mover_iso = transform_isometry(pose.transform);
        let Ok(Some(contact)) = contact(
            &capsule_iso,
            capsule,
            &mover_iso,
            &mover.local_mesh,
            SKIN_DISTANCE,
        ) else {
            continue;
        };
        if contact.dist > 0.0 {
            continue;
        }
        let normal = Vec3::new(-contact.normal1.x, -contact.normal1.y, -contact.normal1.z);
        let depth = -contact.dist + SKIN_DISTANCE;
        if deepest.as_ref().is_none_or(|current| depth > current.depth) {
            deepest = Some(MoverPenetration {
                mover_id: mover.mover_id,
                normal,
                depth,
            });
        }
    }

    deepest
}

pub(crate) fn deepest_mover_push_penetration(
    movers: &[MoverCollider],
    poses: &(impl MoverPoseSource + ?Sized),
    pos: Point<f32>,
    capsule: &Capsule,
) -> Option<MoverPenetration> {
    deepest_mover_push_penetration_inner(movers, poses, pos, capsule, None)
}

pub(crate) fn deepest_mover_push_penetration_excluding_swept(
    movers: &[MoverCollider],
    poses: &(impl MoverPoseSource + ?Sized),
    pos: Point<f32>,
    capsule: &Capsule,
    excluded_mover_id: u32,
) -> Option<MoverPenetration> {
    deepest_mover_push_penetration_inner(movers, poses, pos, capsule, Some(excluded_mover_id))
}

fn deepest_mover_push_penetration_inner(
    movers: &[MoverCollider],
    poses: &(impl MoverPoseSource + ?Sized),
    pos: Point<f32>,
    capsule: &Capsule,
    excluded_swept_mover_id: Option<u32>,
) -> Option<MoverPenetration> {
    let capsule_iso = Isometry::translation(pos.x, pos.y, pos.z);
    let mut deepest = deepest_mover_penetration(movers, poses, pos, capsule);

    for mover in movers {
        if excluded_swept_mover_id == Some(mover.mover_id) {
            continue;
        }
        let Some(pose) = poses.pose(mover.mover_id) else {
            continue;
        };
        let delta = pose.tick_delta;
        if !delta.is_finite() || delta.length_squared() <= f32::EPSILON * f32::EPSILON {
            continue;
        }
        let mut previous_transform = pose.transform;
        previous_transform.position -= delta;
        let previous_iso = transform_isometry(previous_transform);
        let options = ShapeCastOptions {
            max_time_of_impact: 1.0,
            target_distance: SKIN_DISTANCE,
            stop_at_penetration: false,
            ..Default::default()
        };
        let Ok(Some(hit)) = cast_shapes(
            &previous_iso,
            &Vector::new(delta.x, delta.y, delta.z),
            &mover.local_mesh,
            &capsule_iso,
            &Vector::zeros(),
            capsule,
            options,
        ) else {
            continue;
        };
        if !hit.time_of_impact.is_finite() || hit.time_of_impact < 0.0 || hit.time_of_impact > 1.0 {
            continue;
        }
        let normal = swept_push_normal(*hit.transform1_by(&previous_iso).normal1, delta);
        let remaining = (1.0 - hit.time_of_impact).max(0.0);
        let mut depth = delta.dot(normal).max(0.0) * remaining + SKIN_DISTANCE;
        if depth <= SKIN_DISTANCE {
            depth = delta.length() * remaining + SKIN_DISTANCE;
        }
        if !normal.is_finite() || normal.length_squared() <= 0.0 || !depth.is_finite() {
            continue;
        }
        if deepest.as_ref().is_none_or(|current| depth > current.depth) {
            deepest = Some(MoverPenetration {
                mover_id: mover.mover_id,
                normal,
                depth,
            });
        }
    }

    deepest
}

fn choose_nearest(nearest: &mut Option<CombinedCastHit>, candidate: Option<CombinedCastHit>) {
    let Some(candidate) = candidate else {
        return;
    };
    if nearest
        .as_ref()
        .is_none_or(|current| should_replace_hit(current, &candidate))
    {
        *nearest = Some(candidate);
    }
}

fn should_replace_hit(current: &CombinedCastHit, candidate: &CombinedCastHit) -> bool {
    if candidate.time_of_impact < current.time_of_impact - HIT_TOI_TIE_EPSILON {
        return true;
    }
    if (candidate.time_of_impact - current.time_of_impact).abs() <= HIT_TOI_TIE_EPSILON {
        return hit_tie_prefers(candidate, current);
    }
    false
}

fn hit_tie_prefers(candidate: &CombinedCastHit, current: &CombinedCastHit) -> bool {
    let candidate_mover_floor = matches!(candidate.source, CollisionSource::Mover(_))
        && candidate.classification == ContactClassification::Floor;
    let current_mover_floor = matches!(current.source, CollisionSource::Mover(_))
        && current.classification == ContactClassification::Floor;
    candidate_mover_floor && !current_mover_floor
}

fn static_shape_hit(hit: ShapeCastHit) -> CombinedCastHit {
    hit_from_parts(
        hit.time_of_impact,
        *hit.normal2,
        CollisionSource::Static,
        None,
    )
}

fn mover_shape_hit(hit: ShapeCastHit, mover_id: u32, pose: MoverPose) -> CombinedCastHit {
    hit_from_parts(
        hit.time_of_impact,
        *hit.normal2,
        CollisionSource::Mover(mover_id),
        Some(pose),
    )
}

fn static_ray_hit(hit: RayIntersection) -> CombinedCastHit {
    hit_from_parts(
        hit.time_of_impact,
        hit.normal,
        CollisionSource::Static,
        None,
    )
}

fn mover_ray_hit(hit: RayIntersection, mover_id: u32, pose: MoverPose) -> CombinedCastHit {
    hit_from_parts(
        hit.time_of_impact,
        hit.normal,
        CollisionSource::Mover(mover_id),
        Some(pose),
    )
}

fn hit_from_parts(
    time_of_impact: f32,
    normal: Vector<f32>,
    source: CollisionSource,
    pose: Option<MoverPose>,
) -> CombinedCastHit {
    let normal = Vec3::new(normal.x, normal.y, normal.z);
    let mover_id = match source {
        CollisionSource::Static => None,
        CollisionSource::Mover(id) => Some(id),
    };
    CombinedCastHit {
        time_of_impact,
        normal,
        source,
        mover_id,
        mover_linear_velocity: pose.map_or(Vec3::ZERO, |pose| pose.linear_velocity),
        mover_tick_delta: pose.map_or(Vec3::ZERO, |pose| pose.tick_delta),
        mover_tick_dt: pose.map_or(0.0, |pose| pose.tick_dt),
        classification: classify_contact(normal),
    }
}

fn classify_contact(normal: Vec3) -> ContactClassification {
    if normal.y >= COS_WALKABLE {
        ContactClassification::Floor
    } else if normal.y <= -COS_WALKABLE {
        ContactClassification::Ceiling
    } else {
        ContactClassification::Wall
    }
}

fn transform_isometry(transform: Transform) -> Isometry<f32> {
    debug_assert!(
        (transform.scale - Vec3::ONE).length_squared() < 1.0e-6,
        "kinematic mover collision ignores non-unit Transform.scale"
    );
    let q = if transform.rotation.is_finite() && transform.rotation.length_squared() > 1.0e-12 {
        transform.rotation.normalize()
    } else {
        glam::Quat::IDENTITY
    };
    Isometry::from_parts(
        Translation3::new(
            transform.position.x,
            transform.position.y,
            transform.position.z,
        ),
        UnitQuaternion::from_quaternion(Quaternion::new(q.w, q.x, q.y, q.z)),
    )
}

fn swept_push_normal(normal: Vector<f32>, delta: Vec3) -> Vec3 {
    let mut normal = Vec3::new(normal.x, normal.y, normal.z);
    if !normal.is_finite() || normal.length_squared() <= 1.0e-8 {
        normal = delta.normalize_or_zero();
    }
    if normal.dot(delta) < 0.0 {
        normal = -normal;
    }
    normal.normalize_or_zero()
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;

    const EPS: f32 = 1.0e-5;

    #[derive(Default)]
    struct TestPoseSource {
        poses: std::collections::HashMap<u32, MoverPose>,
    }

    impl TestPoseSource {
        fn insert(&mut self, mover_id: u32, position: Vec3, velocity: Vec3, delta: Vec3) {
            self.poses.insert(
                mover_id,
                MoverPose {
                    transform: Transform {
                        position,
                        rotation: Quat::IDENTITY,
                        scale: Vec3::ONE,
                    },
                    linear_velocity: velocity,
                    tick_delta: delta,
                    angular_velocity: Vec3::ZERO,
                    tick_rotation_delta: Quat::IDENTITY,
                    carry_yaw: false,
                    tick_dt: 0.1,
                },
            );
        }
    }

    impl MoverPoseSource for TestPoseSource {
        fn pose(&self, mover_id: u32) -> Option<MoverPose> {
            self.poses.get(&mover_id).copied()
        }
    }

    fn floor_world(y: f32) -> CollisionWorld {
        let points = vec![
            Point::new(-5.0, y, -5.0),
            Point::new(5.0, y, -5.0),
            Point::new(5.0, y, 5.0),
            Point::new(-5.0, y, 5.0),
        ];
        let triangles = vec![[0u32, 1, 2], [0, 2, 3]];
        CollisionWorld {
            mesh: TriMesh::new(points, triangles),
            isometry: Isometry::identity(),
        }
    }

    fn local_floor_collider(mover_id: u32) -> MoverCollider {
        MoverCollider::from_local_triangles(
            mover_id,
            &[
                Vec3::new(-1.0, 0.0, -1.0),
                Vec3::new(1.0, 0.0, -1.0),
                Vec3::new(1.0, 0.0, 1.0),
                Vec3::new(-1.0, 0.0, 1.0),
            ],
            &[[0, 1, 2], [0, 2, 3]],
        )
        .unwrap()
    }

    fn local_wall_collider(mover_id: u32) -> MoverCollider {
        MoverCollider::from_local_triangles(
            mover_id,
            &[
                Vec3::new(0.0, 0.0, -1.0),
                Vec3::new(0.0, 2.0, -1.0),
                Vec3::new(0.0, 2.0, 1.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
            &[[0, 1, 2], [0, 2, 3]],
        )
        .unwrap()
    }

    fn test_capsule() -> Capsule {
        Capsule::new(Point::new(0.0, -0.5, 0.0), Point::new(0.0, 0.5, 0.0), 0.25)
    }

    #[test]
    fn combined_ray_without_movers_matches_static_ray() {
        let world = floor_world(0.0);
        let poses = TestPoseSource::default();
        let origin = Point::new(0.0, 2.0, 0.0);
        let dir = Vector::new(0.0, -1.0, 0.0);

        let direct = cast_ray(&world, origin, dir, 10.0).unwrap();
        let combined = cast_ray_combined(&world, &[], &poses, origin, dir, 10.0).unwrap();

        assert_eq!(combined.source, CollisionSource::Static);
        assert!((combined.time_of_impact - direct.time_of_impact).abs() < EPS);
        assert!(
            (combined.normal - Vec3::new(direct.normal.x, direct.normal.y, direct.normal.z))
                .length()
                < EPS
        );
    }

    #[test]
    fn combined_capsule_without_movers_matches_static_capsule() {
        let world = floor_world(0.0);
        let poses = TestPoseSource::default();
        let capsule = test_capsule();
        let origin = Point::new(0.0, 2.0, 0.0);
        let dir = Vector::new(0.0, -1.0, 0.0);

        let direct = cast_capsule(&world, origin, &capsule, dir, 10.0).unwrap();
        let combined =
            cast_capsule_combined(&world, &[], &poses, origin, &capsule, dir, 10.0).unwrap();

        assert_eq!(combined.source, CollisionSource::Static);
        assert!((combined.time_of_impact - direct.time_of_impact).abs() < EPS);
        assert!(
            (combined.normal - Vec3::new(direct.normal2.x, direct.normal2.y, direct.normal2.z))
                .length()
                < EPS
        );
    }

    #[test]
    fn combined_query_returns_nearest_mover_over_farther_static() {
        let world = floor_world(0.0);
        let movers = [local_floor_collider(42)];
        let mut poses = TestPoseSource::default();
        poses.insert(
            42,
            Vec3::new(0.0, 3.0, 0.0),
            Vec3::new(0.0, 1.5, 0.0),
            Vec3::new(0.0, 0.15, 0.0),
        );

        let hit = cast_ray_combined(
            &world,
            &movers,
            &poses,
            Point::new(0.0, 5.0, 0.0),
            Vector::new(0.0, -1.0, 0.0),
            10.0,
        )
        .unwrap();

        assert_eq!(hit.source, CollisionSource::Mover(42));
        assert_eq!(hit.mover_id, Some(42));
        assert_eq!(hit.classification, ContactClassification::Floor);
        assert!((hit.time_of_impact - 2.0).abs() < EPS);
        assert!((hit.normal - Vec3::Y).length() < EPS);
        assert_eq!(hit.mover_linear_velocity, Vec3::new(0.0, 1.5, 0.0));
        assert_eq!(hit.mover_tick_delta, Vec3::new(0.0, 0.15, 0.0));
    }

    #[test]
    fn combined_query_prefers_equal_toi_mover_floor_over_static_floor() {
        let world = floor_world(3.0);
        let movers = [local_floor_collider(42)];
        let mut poses = TestPoseSource::default();
        poses.insert(42, Vec3::new(0.0, 3.0, 0.0), Vec3::ZERO, Vec3::ZERO);
        let capsule = test_capsule();

        let hit = cast_capsule_combined(
            &world,
            &movers,
            &poses,
            Point::new(0.0, 5.0, 0.0),
            &capsule,
            Vector::new(0.0, -1.0, 0.0),
            10.0,
        )
        .unwrap();

        assert_eq!(hit.source, CollisionSource::Mover(42));
        assert_eq!(hit.classification, ContactClassification::Floor);
    }

    #[test]
    fn combined_query_keeps_static_when_static_is_nearest() {
        let world = floor_world(0.0);
        let movers = [local_floor_collider(42)];
        let mut poses = TestPoseSource::default();
        poses.insert(42, Vec3::new(0.0, -3.0, 0.0), Vec3::Y, Vec3::Y * 0.1);

        let hit = cast_ray_combined(
            &world,
            &movers,
            &poses,
            Point::new(0.0, 5.0, 0.0),
            Vector::new(0.0, -1.0, 0.0),
            10.0,
        )
        .unwrap();

        assert_eq!(hit.source, CollisionSource::Static);
        assert_eq!(hit.mover_id, None);
        assert_eq!(hit.mover_linear_velocity, Vec3::ZERO);
        assert_eq!(hit.mover_tick_delta, Vec3::ZERO);
    }

    #[test]
    fn swept_mover_push_detects_thin_mover_crossing_capsule() {
        let movers = [local_wall_collider(42)];
        let mut poses = TestPoseSource::default();
        poses.insert(
            42,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(20.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
        );
        let capsule = test_capsule();

        let penetration =
            deepest_mover_push_penetration(&movers, &poses, Point::new(0.0, 1.0, 0.0), &capsule)
                .expect("swept mover should detect crossing");

        assert_eq!(penetration.mover_id, 42);
        assert!(
            penetration.normal.x > 0.9,
            "push normal should follow mover travel, got {:?}",
            penetration.normal
        );
        assert!(
            penetration.depth > 1.0,
            "crossing mover should push by the remaining sweep, got {}",
            penetration.depth
        );
    }
}
