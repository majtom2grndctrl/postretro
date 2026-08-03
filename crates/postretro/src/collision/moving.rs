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
const MAX_ROTATION_SWEEP_STEP_RAD: f32 = 5.0_f32.to_radians();
const MIN_ROTATION_SWEEP_STEP_RAD: f32 = 0.5_f32.to_radians();
const MAX_ROTATION_SWEEP_STEPS: usize = 128;

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MoverTranslationLeg {
    pub(crate) start: Vec3,
    pub(crate) end: Vec3,
}

pub(crate) trait MoverPoseSource {
    fn pose(&self, mover_id: u32) -> Option<MoverPose>;

    /// Ordered linear legs actually traversed during this tick. A mover can
    /// cross multiple waypoints and finish with a small or zero net delta, so
    /// policy collision must not infer the path from the final pose alone.
    fn translation_legs(&self, _mover_id: u32) -> Option<&[MoverTranslationLeg]> {
        None
    }

    /// Tick-local provenance for movers whose net pose delta is zero after
    /// crossing one or more path termini. Ordinary pose sources need no
    /// override; the fixed-tick mover table records these arrivals explicitly.
    fn had_endpoint_arrival(&self, _mover_id: u32) -> bool {
        false
    }
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
    local_radius: f32,
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
        let local_radius = vertices
            .iter()
            .map(|vertex| vertex.length())
            .fold(0.0_f32, f32::max);
        Some(Self {
            mover_id,
            local_mesh: TriMesh::new(points, triangles.to_vec()),
            local_radius,
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
        if !delta.is_finite() {
            continue;
        }
        let has_translation = delta.length_squared() > f32::EPSILON * f32::EPSILON;
        let has_rotation = tick_rotation_angle(pose) > f32::EPSILON;
        if !has_translation && !has_rotation {
            continue;
        }
        if !mover_swept_sphere_may_reach_capsule(mover, pose, pos, capsule) {
            continue;
        }
        sweep_mover_against_capsule(mover, pose, capsule, &capsule_iso, &mut deepest);
    }

    deepest
}

/// Conservative broad phase for the expensive sampled angular sweep. Rotation
/// keeps every mover point inside its origin-centered bounding sphere; expanding
/// the origin's translation segment by the capsule bounding sphere cannot reject
/// a real contact. Non-finite inputs fall through to the narrow phase.
fn mover_swept_sphere_may_reach_capsule(
    mover: &MoverCollider,
    pose: MoverPose,
    capsule_position: Point<f32>,
    capsule: &Capsule,
) -> bool {
    let scale = pose.transform.scale.abs().max_element();
    let mover_radius = mover.local_radius * scale;
    let capsule_center_local = capsule.center();
    let capsule_center = Vec3::new(
        capsule_position.x + capsule_center_local.x,
        capsule_position.y + capsule_center_local.y,
        capsule_position.z + capsule_center_local.z,
    );
    let capsule_radius = (capsule.segment.a - capsule_center_local)
        .norm()
        .max((capsule.segment.b - capsule_center_local).norm())
        + capsule.radius;
    let end = pose.transform.position;
    let start = end - pose.tick_delta;
    if !mover_radius.is_finite()
        || !capsule_radius.is_finite()
        || !capsule_center.is_finite()
        || !start.is_finite()
        || !end.is_finite()
    {
        return true;
    }

    let segment = end - start;
    let segment_len_sq = segment.length_squared();
    let closest = if segment_len_sq > f32::EPSILON {
        let t = ((capsule_center - start).dot(segment) / segment_len_sq).clamp(0.0, 1.0);
        start + segment * t
    } else {
        start
    };
    let reach = mover_radius + capsule_radius + SKIN_DISTANCE;
    capsule_center.distance_squared(closest) <= reach * reach
}

fn sweep_mover_against_capsule(
    mover: &MoverCollider,
    pose: MoverPose,
    capsule: &Capsule,
    capsule_iso: &Isometry<f32>,
    deepest: &mut Option<MoverPenetration>,
) {
    let steps = rotation_sweep_steps(mover, pose, capsule.radius);
    let final_iso = transform_isometry(pose.transform);
    let options = ShapeCastOptions {
        max_time_of_impact: 1.0,
        target_distance: SKIN_DISTANCE,
        stop_at_penetration: false,
        ..Default::default()
    };

    for step in 0..steps {
        let start_t = step as f32 / steps as f32;
        let end_t = (step + 1) as f32 / steps as f32;
        let start_transform = mover_sweep_transform(pose, start_t);
        let end_transform = mover_sweep_transform(pose, end_t);
        let start_iso = transform_isometry(start_transform);
        let segment_delta = end_transform.position - start_transform.position;

        if let Ok(Some(hit)) = cast_shapes(
            &start_iso,
            &Vector::new(segment_delta.x, segment_delta.y, segment_delta.z),
            &mover.local_mesh,
            capsule_iso,
            &Vector::zeros(),
            capsule,
            options,
        ) {
            if hit.time_of_impact.is_finite() && (0.0..=1.0).contains(&hit.time_of_impact) {
                let hit_t = start_t + (end_t - start_t) * hit.time_of_impact;
                let hit_transform = mover_sweep_transform(pose, hit_t);
                let remaining_motion = surface_motion_to_final(
                    transform_isometry(hit_transform),
                    final_iso,
                    hit.witness1,
                );
                let normal =
                    swept_push_normal(*hit.transform1_by(&start_iso).normal1, remaining_motion);
                let remaining = (1.0 - hit_t).max(0.0);
                let translation_fallback = pose.tick_delta.length() * remaining;
                record_swept_push(
                    deepest,
                    mover.mover_id,
                    normal,
                    remaining_motion,
                    translation_fallback,
                    0.0,
                );
            }
        }

        // A linear shape cast cannot express angular velocity. Sample each
        // reconstructed orientation and carry the contacted material point to
        // the final pose so a rotating face that crosses and ends clear still
        // produces the displace-only push.
        let Ok(Some(sample_contact)) = contact(
            capsule_iso,
            capsule,
            &start_iso,
            &mover.local_mesh,
            SKIN_DISTANCE,
        ) else {
            continue;
        };
        if sample_contact.dist > 0.0 {
            continue;
        }
        let remaining_motion = surface_motion_to_final(start_iso, final_iso, sample_contact.point2);
        let normal = swept_push_normal(-*sample_contact.normal1, remaining_motion);
        record_swept_push(
            deepest,
            mover.mover_id,
            normal,
            remaining_motion,
            pose.tick_delta.length() * (1.0 - start_t),
            -sample_contact.dist,
        );
    }
}

fn rotation_sweep_steps(mover: &MoverCollider, pose: MoverPose, capsule_radius: f32) -> usize {
    let angle = tick_rotation_angle(pose);
    if angle <= f32::EPSILON {
        return 1;
    }
    let arc_budget = (capsule_radius * 0.5).max(SKIN_DISTANCE);
    let step_angle = if mover.local_radius > f32::EPSILON {
        (arc_budget / mover.local_radius)
            .clamp(MIN_ROTATION_SWEEP_STEP_RAD, MAX_ROTATION_SWEEP_STEP_RAD)
    } else {
        MAX_ROTATION_SWEEP_STEP_RAD
    };
    ((angle / step_angle).ceil() as usize).clamp(1, MAX_ROTATION_SWEEP_STEPS)
}

fn mover_sweep_transform(pose: MoverPose, t: f32) -> Transform {
    let end_rotation = normalized_rotation(pose.transform.rotation);
    let rotation = if let Some((axis, angle)) = authored_tick_rotation(pose) {
        let start_rotation =
            normalized_rotation(Quat::from_axis_angle(axis, -angle) * end_rotation);
        normalized_rotation(Quat::from_axis_angle(axis, angle * t) * start_rotation)
    } else {
        let start_rotation = normalized_rotation(
            normalized_rotation(pose.tick_rotation_delta).conjugate() * end_rotation,
        );
        start_rotation.slerp(end_rotation, t)
    };
    Transform {
        position: pose.transform.position - pose.tick_delta * (1.0 - t),
        rotation,
        scale: pose.transform.scale,
    }
}

fn tick_rotation_angle(pose: MoverPose) -> f32 {
    authored_tick_rotation(pose).map_or_else(
        || {
            let rotation = normalized_rotation(pose.tick_rotation_delta);
            2.0 * rotation.w.abs().clamp(0.0, 1.0).acos()
        },
        |(_, angle)| angle,
    )
}

fn authored_tick_rotation(pose: MoverPose) -> Option<(Vec3, f32)> {
    if !pose.tick_dt.is_finite() || pose.tick_dt <= 0.0 || !pose.angular_velocity.is_finite() {
        return None;
    }
    let angular_speed = pose.angular_velocity.length();
    if !angular_speed.is_finite() || angular_speed <= f32::EPSILON {
        return None;
    }
    let angle = angular_speed * pose.tick_dt;
    if !angle.is_finite() || angle <= f32::EPSILON {
        return None;
    }
    Some((pose.angular_velocity / angular_speed, angle))
}

fn normalized_rotation(rotation: Quat) -> Quat {
    if rotation.is_finite() && rotation.length_squared() > 1.0e-12 {
        rotation.normalize()
    } else {
        Quat::IDENTITY
    }
}

fn surface_motion_to_final(
    sample_iso: Isometry<f32>,
    final_iso: Isometry<f32>,
    world_point: Point<f32>,
) -> Vec3 {
    let local_point = sample_iso.inverse_transform_point(&world_point);
    let final_point = final_iso.transform_point(&local_point);
    Vec3::new(
        final_point.x - world_point.x,
        final_point.y - world_point.y,
        final_point.z - world_point.z,
    )
}

fn record_swept_push(
    deepest: &mut Option<MoverPenetration>,
    mover_id: u32,
    normal: Vec3,
    remaining_motion: Vec3,
    translation_fallback: f32,
    penetration_depth: f32,
) {
    if !normal.is_finite() || normal.length_squared() <= 0.0 {
        return;
    }
    let projected_motion = remaining_motion.dot(normal).max(0.0);
    let motion_depth = if projected_motion > f32::EPSILON {
        projected_motion
    } else {
        remaining_motion.length().max(translation_fallback)
    };
    let depth = motion_depth + penetration_depth.max(0.0) + SKIN_DISTANCE;
    if !depth.is_finite() {
        return;
    }
    if deepest.as_ref().is_none_or(|current| depth > current.depth) {
        *deepest = Some(MoverPenetration {
            mover_id,
            normal,
            depth,
        });
    }
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
            self.insert_pose(
                mover_id,
                Transform {
                    position,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                velocity,
                delta,
                Quat::IDENTITY,
            );
        }

        fn insert_pose(
            &mut self,
            mover_id: u32,
            transform: Transform,
            velocity: Vec3,
            delta: Vec3,
            tick_rotation_delta: Quat,
        ) {
            self.poses.insert(
                mover_id,
                MoverPose {
                    transform,
                    linear_velocity: velocity,
                    tick_delta: delta,
                    angular_velocity: Vec3::ZERO,
                    tick_rotation_delta,
                    carry_yaw: false,
                    tick_dt: 0.1,
                },
            );
        }

        fn insert_rotating_pose(
            &mut self,
            mover_id: u32,
            transform: Transform,
            velocity: Vec3,
            delta: Vec3,
            angular_velocity: Vec3,
            tick_rotation_delta: Quat,
            tick_dt: f32,
        ) {
            self.poses.insert(
                mover_id,
                MoverPose {
                    transform,
                    linear_velocity: velocity,
                    tick_delta: delta,
                    angular_velocity,
                    tick_rotation_delta,
                    carry_yaw: false,
                    tick_dt,
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

    #[test]
    fn swept_contact_motion_tracks_mover_witness_through_rotation() {
        let sample_iso = Isometry::identity();
        let rotating_final_iso = transform_isometry(Transform {
            position: Vec3::new(0.5, 0.0, 0.0),
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            scale: Vec3::ONE,
        });
        let linear_final_iso = Isometry::translation(0.5, 0.0, 0.0);
        let capsule_center = Point::new(0.0, 1.0, 0.0);
        let mover_witness = Point::new(1.0, 1.0, 0.0);

        let witness_rotation_motion =
            surface_motion_to_final(sample_iso, rotating_final_iso, mover_witness);
        let center_rotation_motion =
            surface_motion_to_final(sample_iso, rotating_final_iso, capsule_center);
        assert!(
            (witness_rotation_motion - center_rotation_motion).length() > EPS,
            "rotation must use the mover contact witness, not the capsule center"
        );

        let witness_linear_motion =
            surface_motion_to_final(sample_iso, linear_final_iso, mover_witness);
        let center_linear_motion =
            surface_motion_to_final(sample_iso, linear_final_iso, capsule_center);
        assert!(
            (witness_linear_motion - center_linear_motion).length() < EPS,
            "pure translation must preserve the existing linear sweep motion"
        );
    }

    // Regression: the advancing sweep rebuilt the previous position with the
    // final orientation, so rotation-only crossings inside that advancing tick
    // disappeared when both endpoint poses were clear.
    #[test]
    fn advancing_rotating_sweep_detects_face_crossing_with_clear_endpoints() {
        let movers = [local_wall_collider(42)];
        let mut poses = TestPoseSource::default();
        let rotation_delta = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        poses.insert_rotating_pose(
            42,
            Transform {
                position: Vec3::new(0.02, 0.0, 0.0),
                rotation: rotation_delta,
                scale: Vec3::ONE,
            },
            Vec3::new(0.2, 0.0, 0.0),
            Vec3::new(0.02, 0.0, 0.0),
            Vec3::Y * std::f32::consts::FRAC_PI_2 / 0.1,
            rotation_delta,
            0.1,
        );
        let capsule = test_capsule();
        let capsule_position = Point::new(0.7, 1.0, 0.7);

        assert!(
            deepest_mover_penetration(&movers, &poses, capsule_position, &capsule).is_none(),
            "the final rotated face must be clear so the sweep is the only detector"
        );
        let penetration =
            deepest_mover_push_penetration(&movers, &poses, capsule_position, &capsule)
                .expect("the rotating face crosses the capsule between clear endpoints");
        assert_eq!(penetration.mover_id, 42);
        assert!(penetration.normal.is_finite());
        assert!(
            penetration.depth > SKIN_DISTANCE,
            "rotational sweep must produce a behaviorally meaningful displacement"
        );
    }

    // Regression: quaternion slerp chose the short arc for authored turns over
    // pi, so intermediate sweep samples followed the wrong rotation.
    #[test]
    fn rotating_sweep_transform_preserves_authored_winding_past_pi() {
        let angle = std::f32::consts::PI * 1.5;
        let pose = MoverPose {
            transform: Transform {
                position: Vec3::ZERO,
                rotation: Quat::from_rotation_y(angle),
                scale: Vec3::ONE,
            },
            linear_velocity: Vec3::ZERO,
            tick_delta: Vec3::ZERO,
            angular_velocity: Vec3::Y * angle,
            tick_rotation_delta: Quat::from_rotation_y(angle),
            carry_yaw: false,
            tick_dt: 1.0,
        };

        let one_third = mover_sweep_transform(pose, 1.0 / 3.0);
        assert!(
            one_third
                .rotation
                .abs_diff_eq(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), EPS),
            "the sample must follow the authored 270-degree turn, not the quaternion short arc"
        );

        let mover = local_wall_collider(42);
        let large_turn = MoverPose {
            angular_velocity: Vec3::Y * (std::f32::consts::TAU * 1_000.0),
            tick_rotation_delta: Quat::IDENTITY,
            ..pose
        };
        assert_eq!(
            rotation_sweep_steps(&mover, large_turn, test_capsule().radius),
            MAX_ROTATION_SWEEP_STEPS,
            "large authored turns must remain bounded"
        );
    }

    // Regression: a full-turn delta is quaternion-equivalent to identity, so
    // the rotating face crossed the capsule without any sweep samples.
    #[test]
    fn rotating_sweep_detects_full_turn_crossing_with_clear_endpoints() {
        let movers = [local_wall_collider(42)];
        let mut poses = TestPoseSource::default();
        poses.insert_rotating_pose(
            42,
            Transform::default(),
            Vec3::ZERO,
            Vec3::ZERO,
            Vec3::Y * std::f32::consts::TAU,
            Quat::from_rotation_y(std::f32::consts::TAU),
            1.0,
        );
        let capsule = test_capsule();
        let capsule_position = Point::new(0.7, 1.0, 0.7);

        assert!(
            deepest_mover_penetration(&movers, &poses, capsule_position, &capsule).is_none(),
            "the coincident full-turn endpoints must remain clear"
        );
        let penetration =
            deepest_mover_push_penetration(&movers, &poses, capsule_position, &capsule)
                .expect("the full-turn sweep should detect its intermediate crossing");
        assert_eq!(penetration.mover_id, 42);
        assert!(penetration.normal.is_finite());
        assert!(penetration.depth > SKIN_DISTANCE);
    }

    #[test]
    fn rotational_sweep_broad_phase_rejects_far_movers_without_rejecting_crossings() {
        let mover = local_wall_collider(42);
        let capsule = test_capsule();
        let pose = MoverPose {
            transform: Transform::default(),
            linear_velocity: Vec3::ZERO,
            tick_delta: Vec3::ZERO,
            angular_velocity: Vec3::Y * std::f32::consts::TAU,
            tick_rotation_delta: Quat::IDENTITY,
            carry_yaw: false,
            tick_dt: 1.0,
        };

        assert!(!mover_swept_sphere_may_reach_capsule(
            &mover,
            pose,
            Point::new(1_000.0, 1_000.0, 1_000.0),
            &capsule,
        ));
        assert!(mover_swept_sphere_may_reach_capsule(
            &mover,
            pose,
            Point::new(0.7, 1.0, 0.7),
            &capsule,
        ));
    }
}
