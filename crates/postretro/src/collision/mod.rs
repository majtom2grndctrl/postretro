//! Collision world — world-space `parry3d` trimesh built from PRL static
//! geometry, plus the free-function query surface used by player movement.
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
//! `CollisionWorld` is Rust-only; not exposed to scripts. nalgebra types
//! stay within the engine crate — `mesh` and `isometry` are `pub(crate)`.
//! Subsystem-boundary coordinates use `glam::Vec3`.
//!
//! Queries call `parry3d::query::*` free functions directly. There is no
//! `QueryPipeline` and no higher-level query API.
//!
//! See: `context/lib/entity_model.md` §7.

use parry3d::math::{Isometry, Point, Vector};
use parry3d::query::{Ray, RayCast, RayIntersection, ShapeCastHit, ShapeCastOptions, cast_shapes};
use parry3d::shape::{Capsule, TriMesh};

use postretro_level_loader::LevelWorld;

/// World-space static-geometry collider. Owns a single `parry3d::TriMesh`
/// built from the level's baked vertices and indices, plus a world-space
/// `Isometry3<f32>` (always identity — PRL geometry is already world-space).
#[derive(Debug)]
pub struct CollisionWorld {
    pub(crate) mesh: TriMesh,
    pub(crate) isometry: Isometry<f32>,
}

impl CollisionWorld {
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

    #[cfg(test)]
    pub(crate) fn triangle_count(&self) -> usize {
        self.mesh.triangles().len()
    }

    #[cfg(test)]
    pub(crate) fn vertex_count(&self) -> usize {
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
pub(crate) const SKIN_DISTANCE: f32 = 0.02;

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
pub(crate) fn cast_capsule(
    world: &CollisionWorld,
    pos: Point<f32>,
    capsule: &Capsule,
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

/// Cast a ray through the world trimesh, returning the first intersection
/// (with normal). `solid = true` so the ray exits a triangle hit on the back
/// face — matches the conventions used by the movement code's ground-stick
/// fallback.
pub(crate) fn cast_ray(
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

#[cfg(test)]
mod tests {
    use super::*;
    use parry3d::math::Vector;

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

    #[test]
    fn collision_world_ray_hits_floor_at_unit_distance() {
        let world = floor_world();

        let ray = Ray::new(Point::new(0.0, 1.0, 0.0), Vector::new(0.0, -1.0, 0.0));
        let max_toi = 10.0;
        let solid = true;

        let hit = world
            .mesh
            .cast_ray_and_get_normal(&world.isometry, &ray, max_toi, solid)
            .expect("ray pointing straight down should hit the floor");

        let eps = 1.0e-5;
        assert!(
            (hit.time_of_impact - 1.0).abs() < eps,
            "expected TOI ≈ 1.0, got {}",
            hit.time_of_impact
        );
        let normal_err = (hit.normal - parry3d::math::Vector::new(0.0, 1.0, 0.0)).norm();
        assert!(
            normal_err < eps,
            "expected contact normal ≈ (0, 1, 0), got ({}, {}, {})",
            hit.normal.x,
            hit.normal.y,
            hit.normal.z
        );
    }
}
