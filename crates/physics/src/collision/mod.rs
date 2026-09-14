//! Collision world — world-space `parry3d` trimesh built from PRL static
//! geometry, plus combined static/mover query helpers used by movement.
//!
//! # Capsule axis convention
//!
//! The player capsule's axis is world **+Y**. Endpoints sit at
//! `origin ± half_height * Y`. parry3d's native capsule axis is also +Y, so
//! the engine's capsule definition maps directly to `parry3d::shape::Capsule`
//! without rotation.
//!
//! # Boundary
//!
//! `CollisionWorld` is Rust-only; not exposed to scripts. Parry and nalgebra
//! values stay private to this crate. Subsystem-boundary coordinates and query
//! results use engine-owned types built from `glam::Vec3`.
//!
//! Queries call `parry3d::query::*` free functions directly. There is no
//! `QueryPipeline` and no higher-level query API.
//!
//! See: `context/lib/entity_model.md` §7.

use parry3d::math::{Isometry, Point, Vector};
use parry3d::query::{
    Ray, RayCast, RayIntersection, ShapeCastHit, ShapeCastOptions, cast_shapes, intersection_test,
};
use parry3d::shape::{Ball, Capsule as ParryCapsule, TriMesh};

use postretro_level_loader::LevelWorld;

pub mod moving;

/// World-space static-geometry collider. Owns a single `parry3d::TriMesh`
/// built from the level's baked vertices and indices, plus a world-space
/// `Isometry3<f32>` (always identity — PRL geometry is already world-space).
#[derive(Debug)]
pub struct CollisionWorld {
    pub(crate) mesh: TriMesh,
    pub(crate) isometry: Isometry<f32>,
}

impl CollisionWorld {
    /// Build static geometry from engine-native triangle data for cross-crate
    /// harnesses. Production population remains [`Self::populate_from_level`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn from_triangles_for_test(vertices: Vec<glam::Vec3>, triangles: Vec<[u32; 3]>) -> Self {
        let points = vertices
            .into_iter()
            .map(|point| Point::new(point.x, point.y, point.z))
            .collect();
        Self {
            mesh: TriMesh::new(points, triangles),
            isometry: Isometry::identity(),
        }
    }

    /// Project a point onto the test world's static mesh without exposing
    /// Parry's point-query vocabulary to a dependent crate.
    #[cfg(any(test, feature = "test-support"))]
    pub fn project_point_for_test(&self, point: glam::Vec3, solid: bool) -> glam::Vec3 {
        use parry3d::query::PointQuery;

        let projection = self.mesh.project_point(
            &self.isometry,
            &Point::new(point.x, point.y, point.z),
            solid,
        );
        glam::Vec3::new(projection.point.x, projection.point.y, projection.point.z)
    }

    /// Initialize with a structurally valid 1-triangle mesh. `parry3d::TriMesh`
    /// requires at least one triangle; the placeholder is placed at `x = 1e6` —
    /// far outside any plausible game-space origin — so an unpopulated world
    /// reports no hits for ordinary gameplay queries.
    pub fn new() -> Self {
        // parry3d's TriMesh requires at least one triangle. Place the
        // placeholder at 1e6 on the X axis — far outside any plausible
        // game-space origin — so an unpopulated world reports no hits.
        let placeholder_points = vec![
            Point::new(1.0e6_f32, 0.0, 0.0),
            Point::new(1.0e6_f32, 1.0, 0.0),
            Point::new(1.0e6_f32, 0.0, 1.0),
        ];
        let placeholder_indices = vec![[0u32, 1, 2]];
        let mesh = TriMesh::new(placeholder_points, placeholder_indices);
        Self {
            mesh,
            isometry: Isometry::identity(),
        }
    }

    /// Rebuild the trimesh from PRL static geometry. All triangles are
    /// included — no material filter. The world-space isometry is reset
    /// to identity since PRL vertices are already in world space.
    pub fn populate_from_level(&mut self, world: &LevelWorld) {
        let points: Vec<Point<f32>> = world
            .vertices
            .iter()
            .map(|v| Point::new(v.position[0], v.position[1], v.position[2]))
            .collect();

        debug_assert_eq!(
            world.indices.len() % 3,
            0,
            "PRL indices must be a multiple of 3; got {}",
            world.indices.len()
        );

        let triangles: Vec<[u32; 3]> = world
            .indices
            .chunks_exact(3)
            .map(|chunk| [chunk[0], chunk[1], chunk[2]])
            .collect();

        if triangles.is_empty() {
            *self = Self::new();
            return;
        }

        // `TriMesh::new` returns the mesh directly (no Result). PRL geometry is
        // validated upstream by the level compiler, so we trust the
        // (points, triangles) pair here. `chunks_exact` silently drops a trailing
        // remainder — the debug_assert above guards against misaligned index buffers
        // reaching this point.
        self.mesh = TriMesh::new(points, triangles);
        self.isometry = Isometry::identity();
    }

    /// Reset to the empty placeholder state.
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn triangle_count(&self) -> usize {
        self.mesh.triangles().len()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn vertex_count(&self) -> usize {
        self.mesh.vertices().len()
    }
}

impl Default for CollisionWorld {
    fn default() -> Self {
        Self::new()
    }
}

/// Capsule sweep skin distance, Quake's DIST_EPSILON analogue scaled for the
/// 0.4 m player radius. Parry returns hits when separation falls below this
/// value, so the swept capsule never actually touches geometry — it rests
/// `SKIN_DISTANCE` away. The slide loop in `movement::tick` relies on this
/// separation for clearance; do not duplicate the offset by pushing again.
pub const SKIN_DISTANCE: f32 = 0.02;

/// A surface counts as walkable when its contact normal points mostly upward.
/// Mirrors the small agent harness's floor test so placement queries and agent
/// movement agree about walls vs. floors. Exposed so the fixed-tick
/// foot ground-probe step applies the same floor-vs-wall threshold movement uses.
pub const COS_WALKABLE: f32 = postretro_foundation::WALKABLE_SURFACE_MIN_UP_DOT;

/// Upright engine-owned capsule description used by public collision queries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollisionCapsule {
    pub radius: f32,
    pub half_height: f32,
}

impl CollisionCapsule {
    pub fn new(radius: f32, half_height: f32) -> Self {
        Self {
            radius,
            half_height,
        }
    }

    pub(crate) fn parry(self) -> ParryCapsule {
        ParryCapsule::new(
            Point::new(0.0, -self.half_height, 0.0),
            Point::new(0.0, self.half_height, 0.0),
            self.radius,
        )
    }
}

/// Engine-owned hit returned by static collision queries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CastHit {
    pub time_of_impact: f32,
    pub normal: glam::Vec3,
}

/// Small lift margin used by placement ground probes. Matches the agent harness
/// so "within the step envelope" means the same thing for selection and motion.
const STEP_UP_LIFT_MARGIN: f32 = 0.05;

/// Capsule geometry for static placement checks. The capsule axis is world +Y,
/// matching [`cast_capsule`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CapsulePlacement {
    pub radius: f32,
    pub half_height: f32,
    pub step_height: f32,
}

impl CapsulePlacement {
    fn is_valid(self) -> bool {
        self.radius.is_finite()
            && self.radius > 0.0
            && self.half_height.is_finite()
            && self.half_height >= 0.0
            && self.step_height.is_finite()
            && self.step_height > 0.0
    }

    fn parry(self) -> ParryCapsule {
        ParryCapsule::new(
            Point::new(0.0, -self.half_height, 0.0),
            Point::new(0.0, self.half_height, 0.0),
            self.radius,
        )
    }

    pub fn rest_offset(self) -> f32 {
        self.half_height + self.radius + SKIN_DISTANCE
    }
}

/// Static-world-only capsule placement query. Returns the grounded capsule
/// center when a capsule near `center` can rest on walkable floor without
/// penetrating static geometry.
pub fn capsule_static_placement_center(
    world: &CollisionWorld,
    center: glam::Vec3,
    placement: CapsulePlacement,
) -> Option<glam::Vec3> {
    if !center.is_finite() || !placement.is_valid() {
        return None;
    }

    let capsule = placement.parry();
    let center = capsule_walkable_floor_center(world, center, &capsule, placement)?;
    let iso = Isometry::translation(center.x, center.y, center.z);
    match intersection_test(&iso, &capsule, &world.isometry, &world.mesh) {
        Ok(false) => Some(center),
        Ok(true) | Err(_) => None,
    }
}

fn capsule_walkable_floor_center(
    world: &CollisionWorld,
    center: glam::Vec3,
    capsule: &ParryCapsule,
    placement: CapsulePlacement,
) -> Option<glam::Vec3> {
    let max_down = placement.step_height + STEP_UP_LIFT_MARGIN + SKIN_DISTANCE + 0.03;

    if let Some(h) = cast_capsule_parry(
        world,
        Point::new(center.x, center.y, center.z),
        capsule,
        Vector::new(0.0, -1.0, 0.0),
        max_down,
    ) {
        if h.normal2.y >= COS_WALKABLE {
            return Some(center - glam::Vec3::new(0.0, h.time_of_impact, 0.0));
        }
    }

    let ray_max = max_down + placement.half_height + placement.radius;
    let ray = cast_ray_parry(
        world,
        Point::new(center.x, center.y, center.z),
        Vector::new(0.0, -1.0, 0.0),
        ray_max,
    )?;

    if ray.normal.y < COS_WALKABLE {
        return None;
    }

    let target_gap = placement.rest_offset();
    let drop = ray.time_of_impact - target_gap;
    if drop > 0.0 && drop <= max_down {
        Some(center - glam::Vec3::new(0.0, drop, 0.0))
    } else {
        None
    }
}

/// Sweep a capsule through the world trimesh along `dir` up to `max_toi`
/// distance. The capsule's isometry sits at `pos` with identity rotation —
/// the capsule's `+Y` axis maps directly to world `+Y`, matching the
/// player-capsule convention documented at the top of this module.
///
/// `target_distance: SKIN_DISTANCE` keeps the capsule that far from surfaces.
/// Paired with `stop_at_penetration: false` so parry sweeps cleanly when the
/// capsule starts in contact — otherwise resting contact produces TOI=0 and
/// stalls the sweep-and-slide loop in `movement::tick`.
///
/// Returns `None` when no impact occurs within `max_toi`. `cast_shapes` also
/// returns `Err` for unsupported shape pairs, but that is impossible for
/// Capsule × TriMesh (always supported by parry3d). Returning `None` on `Err`
/// rather than panicking avoids a `Result` return type that would add noise at
/// every call site for a condition that cannot occur.
pub fn cast_capsule(
    world: &CollisionWorld,
    pos: glam::Vec3,
    capsule: CollisionCapsule,
    dir: glam::Vec3,
    max_toi: f32,
) -> Option<CastHit> {
    let capsule = capsule.parry();
    cast_capsule_parry(
        world,
        Point::new(pos.x, pos.y, pos.z),
        &capsule,
        Vector::new(dir.x, dir.y, dir.z),
        max_toi,
    )
    .map(shape_cast_hit)
}

pub(crate) fn cast_capsule_parry(
    world: &CollisionWorld,
    pos: Point<f32>,
    capsule: &ParryCapsule,
    dir: Vector<f32>,
    max_toi: f32,
) -> Option<ShapeCastHit> {
    let pos1 = Isometry::translation(pos.x, pos.y, pos.z);
    let vel2 = Vector::zeros();
    let options = ShapeCastOptions {
        max_time_of_impact: max_toi,
        target_distance: SKIN_DISTANCE,
        stop_at_penetration: false,
        ..Default::default()
    };
    cast_shapes(
        &pos1,
        &dir,
        capsule,
        &world.isometry,
        &vel2,
        &world.mesh,
        options,
    )
    .ok()
    .flatten()
}

/// Sweep a sphere using its exact authored radius. Unlike movement capsule
/// casts, projectile casts add no skin distance: proximity is not impact.
pub fn cast_sphere_exact(
    world: &CollisionWorld,
    pos: glam::Vec3,
    radius: f32,
    dir: glam::Vec3,
    max_toi: f32,
) -> Option<CastHit> {
    cast_sphere_exact_parry(
        world,
        Point::new(pos.x, pos.y, pos.z),
        radius,
        Vector::new(dir.x, dir.y, dir.z),
        max_toi,
    )
    .map(shape_cast_hit)
}

fn cast_sphere_exact_parry(
    world: &CollisionWorld,
    pos: Point<f32>,
    radius: f32,
    dir: Vector<f32>,
    max_toi: f32,
) -> Option<ShapeCastHit> {
    let pos1 = Isometry::translation(pos.x, pos.y, pos.z);
    let vel2 = Vector::zeros();
    let options = ShapeCastOptions {
        max_time_of_impact: max_toi,
        target_distance: 0.0,
        stop_at_penetration: false,
        ..Default::default()
    };
    cast_shapes(
        &pos1,
        &dir,
        &Ball::new(radius),
        &world.isometry,
        &vel2,
        &world.mesh,
        options,
    )
    .ok()
    .flatten()
}

/// Return whether the exact segment from `eye` to `aim` is unobstructed by
/// static world geometry. Dynamic movers intentionally do not participate:
/// callers that need mover-aware collision use the combined query surface
/// explicitly instead.
pub fn line_of_sight(eye: glam::Vec3, aim: glam::Vec3, world: &CollisionWorld) -> bool {
    let to_aim = aim - eye;
    let distance = to_aim.length();
    if !distance.is_finite() || distance <= 1.0e-5 {
        return false;
    }

    let direction = to_aim / distance;
    let hit = cast_ray(world, eye, direction, distance);
    !matches!(hit, Some(hit) if hit.time_of_impact < distance - 1.0e-4)
}

/// Cast a ray through the world trimesh, returning the first intersection
/// (with normal). `solid = true` so the ray exits a triangle hit on the back
/// face — matches the conventions used by the movement code's ground-stick
/// fallback.
pub fn cast_ray(
    world: &CollisionWorld,
    origin: glam::Vec3,
    dir: glam::Vec3,
    max_toi: f32,
) -> Option<CastHit> {
    cast_ray_parry(
        world,
        Point::new(origin.x, origin.y, origin.z),
        Vector::new(dir.x, dir.y, dir.z),
        max_toi,
    )
    .map(ray_hit)
}

pub(crate) fn cast_ray_parry(
    world: &CollisionWorld,
    origin: Point<f32>,
    dir: Vector<f32>,
    max_toi: f32,
) -> Option<RayIntersection> {
    let ray = Ray::new(origin, dir);
    world
        .mesh
        .cast_ray_and_get_normal(&world.isometry, &ray, max_toi, true)
}

fn shape_cast_hit(hit: ShapeCastHit) -> CastHit {
    CastHit {
        time_of_impact: hit.time_of_impact,
        normal: glam::Vec3::new(hit.normal2.x, hit.normal2.y, hit.normal2.z),
    }
}

fn ray_hit(hit: RayIntersection) -> CastHit {
    CastHit {
        time_of_impact: hit.time_of_impact,
        normal: glam::Vec3::new(hit.normal.x, hit.normal.y, hit.normal.z),
    }
}

/// Ray-test a world-space segment capsule, or a sphere when `b` is absent or
/// degenerate. This keeps entity hit-zone collision on the same engine-owned
/// glam boundary as static-world queries.
pub fn cast_ray_against_segment(
    origin: glam::Vec3,
    direction: glam::Vec3,
    a: glam::Vec3,
    b: Option<glam::Vec3>,
    radius: f32,
    range: f32,
) -> Option<CastHit> {
    let ray = Ray::new(
        Point::new(origin.x, origin.y, origin.z),
        Vector::new(direction.x, direction.y, direction.z),
    );
    let intersection = match b {
        Some(b) if (b - a).length() > 1.0e-6 => {
            let capsule =
                ParryCapsule::new(Point::new(a.x, a.y, a.z), Point::new(b.x, b.y, b.z), radius);
            capsule.cast_ray_and_get_normal(&Isometry::identity(), &ray, range, true)
        }
        _ => Ball::new(radius).cast_ray_and_get_normal(
            &Isometry::translation(a.x, a.y, a.z),
            &ray,
            range,
            true,
        ),
    }?;
    (intersection.time_of_impact <= range).then(|| ray_hit(intersection))
}

/// Return whether an exact sphere is clear of static world geometry.
pub fn sphere_fits_world(world: &CollisionWorld, position: glam::Vec3, radius: f32) -> bool {
    if !position.is_finite() || !radius.is_finite() || radius <= 0.0 {
        return false;
    }

    let sphere = Ball::new(radius);
    let sphere_isometry = Isometry::translation(position.x, position.y, position.z);
    let options = ShapeCastOptions {
        max_time_of_impact: 0.0,
        target_distance: 0.0,
        stop_at_penetration: true,
        ..Default::default()
    };
    let cast_hits = cast_shapes(
        &sphere_isometry,
        &Vector::zeros(),
        &sphere,
        &world.isometry,
        &Vector::zeros(),
        &world.mesh,
        options,
    )
    .is_ok_and(|hit| hit.is_some());
    !cast_hits
        && intersection_test(&sphere_isometry, &sphere, &world.isometry, &world.mesh)
            .is_ok_and(|intersects| !intersects)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collision_world_is_thread_shareable() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CollisionWorld>();
    }
    /// Two-triangle floor at y=0 spanning the XZ plane from (-1,-1) to (1,1).
    /// Used as a fixture for ray-cast verification independent of PRL plumbing.
    fn floor_world() -> CollisionWorld {
        let points = vec![
            Point::new(-1.0, 0.0, -1.0),
            Point::new(1.0, 0.0, -1.0),
            Point::new(1.0, 0.0, 1.0),
            Point::new(-1.0, 0.0, 1.0),
        ];
        let triangles = vec![[0u32, 1, 2], [0, 2, 3]];
        let mesh = TriMesh::new(points, triangles);
        CollisionWorld {
            mesh,
            isometry: Isometry::identity(),
        }
    }

    fn wall_world(x: f32) -> CollisionWorld {
        let points = vec![
            Point::new(x, -1.0, -1.0),
            Point::new(x, 1.0, -1.0),
            Point::new(x, 1.0, 1.0),
            Point::new(x, -1.0, 1.0),
        ];
        let triangles = vec![[0u32, 1, 2], [0, 2, 3]];
        CollisionWorld {
            mesh: TriMesh::new(points, triangles),
            isometry: Isometry::identity(),
        }
    }

    #[test]
    fn collision_world_ray_hits_floor_at_unit_distance() {
        let world = floor_world();

        let hit = cast_ray(&world, glam::Vec3::Y, glam::Vec3::NEG_Y, 10.0)
            .expect("ray pointing straight down should hit the floor");

        let eps = 1.0e-5;
        assert!(
            (hit.time_of_impact - 1.0).abs() < eps,
            "expected TOI ≈ 1.0, got {}",
            hit.time_of_impact
        );
        let normal_err = (hit.normal - glam::Vec3::Y).length();
        assert!(
            normal_err < eps,
            "expected contact normal ≈ (0, 1, 0), got ({}, {}, {})",
            hit.normal.x,
            hit.normal.y,
            hit.normal.z
        );
    }

    #[test]
    fn glam_shape_adapters_preserve_toi_and_normal() {
        let floor = floor_world();
        let capsule = CollisionCapsule::new(0.25, 0.5);
        let capsule_hit = cast_capsule(
            &floor,
            glam::Vec3::new(0.0, 2.0, 0.0),
            capsule,
            glam::Vec3::NEG_Y,
            10.0,
        )
        .expect("engine-native capsule should hit the floor");
        // The capsule begins 2 m above the floor, with its lowest point
        // `half_height + radius` below its origin. The sweep must stop one
        // skin distance before contact. Parry's support-map cast terminates at
        // a relative GJK tolerance of sqrt(10 * f32::EPSILON), so use that
        // solver tolerance around the independently derived geometric TOI.
        let expected_capsule_toi = 2.0 - capsule.half_height - capsule.radius - SKIN_DISTANCE;
        let gjk_tolerance = (10.0 * f32::EPSILON).sqrt() * expected_capsule_toi;
        assert!(
            (capsule_hit.time_of_impact - expected_capsule_toi).abs() <= gjk_tolerance,
            "expected capsule TOI ≈ {expected_capsule_toi}, got {}",
            capsule_hit.time_of_impact
        );
        let capsule_normal_length = capsule_hit.normal.length();
        assert!(
            capsule_hit.normal.is_finite()
                && (capsule_normal_length - 1.0).abs() <= gjk_tolerance
                && capsule_hit.normal.y >= COS_WALKABLE,
            "expected a finite, unit-length walkable floor normal, got ({}, {}, {}) with length {}",
            capsule_hit.normal.x,
            capsule_hit.normal.y,
            capsule_hit.normal.z,
            capsule_normal_length
        );

        let sphere_hit =
            cast_sphere_exact(&wall_world(1.0), glam::Vec3::ZERO, 0.1, glam::Vec3::X, 10.0)
                .expect("engine-native sphere should hit the wall");
        assert!((sphere_hit.time_of_impact - 0.9).abs() < 1.0e-5);
        assert!((sphere_hit.normal.abs() - glam::Vec3::X).length() < 1.0e-5);
    }

    #[test]
    fn line_of_sight_blocks_only_static_world_hits_before_the_aim_point() {
        let wall = wall_world(1.0);
        let eye = glam::Vec3::new(0.0, 0.0, 0.0);
        let aim = glam::Vec3::new(2.0, 0.0, 0.0);

        assert!(
            !line_of_sight(eye, aim, &wall),
            "a static wall before the aim point blocks sight"
        );
        assert!(
            line_of_sight(eye, aim, &CollisionWorld::new()),
            "the empty static world leaves sight clear"
        );
        assert!(
            !line_of_sight(eye, eye, &wall),
            "a zero-length sightline is never considered clear"
        );
    }
}
