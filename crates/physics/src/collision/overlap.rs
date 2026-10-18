// Static overlap query, its mapper-facing report, and the per-level-load state the
// tick overlap check keeps: which texture each triangle carries, the warn-once
// latch and the check's CPU timer.
// See: context/lib/movement.md §1 · context/lib/entity_model.md §7

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use glam::Vec3;
use parry3d::math::Pose;
use parry3d::query::intersection_test;
use parry3d::shape::{Capsule as ParryCapsule, Shape, Triangle};

use postretro_level_loader::LevelWorld;

use super::{CollisionCapsule, CollisionWorld, OVERLAP_TOLERANCE, capsule_triangle};

/// Metres per map unit in the idTech2 source format (1 unit = 1 inch). The
/// compiler applies it; the runtime inverts it only to report positions a
/// mapper can find (`build_pipeline.md` §Supported Map Formats).
pub const MAP_UNIT_METRES: f32 = 0.0254;

/// A body whose own movement ended a tick overlapping static geometry, as the
/// tick overlap check reports it. Callers resolve identity and log it once per
/// level load (`CollisionWorld::claim_overlap_warning`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StaticOverlap {
    /// Capsule centre where the breach was found, engine metres: the end of
    /// the tick, or for a capsule resize, the resized pose.
    pub position: Vec3,
    /// Texture of the deepest triangle overlapped, as an index for
    /// [`CollisionWorld::texture_name`]. `None` for an untextured face or a
    /// capsule wholly inside a solid cell.
    pub texture: Option<u16>,
}

impl StaticOverlap {
    /// [`Self::position`] in TrenchBroom map units: engine `(x, y, z)` is map
    /// `(-z, -x, y)` over [`MAP_UNIT_METRES`], inverting the compiler's axis
    /// swizzle.
    pub fn map_position(&self) -> Vec3 {
        let p = self.position;
        Vec3::new(-p.z, -p.x, p.y) / MAP_UNIT_METRES
    }
}

/// Texture per collision triangle, from the level's BVH leaves (each face has
/// one leaf with its index range, and its bucket id is the texture index).
#[derive(Debug, Default)]
pub(super) struct SurfaceTable {
    triangle_textures: Vec<u16>,
    texture_names: Vec<String>,
}

const NO_TEXTURE: u16 = u16::MAX;

/// What [`SurfaceTable`] reads from one BVH leaf.
struct LeafSurface {
    texture: u32,
    index_offset: u32,
    index_count: u32,
}

impl SurfaceTable {
    pub(super) fn from_level(world: &LevelWorld) -> Self {
        let leaves = world.bvh.leaves.iter().map(|leaf| LeafSurface {
            texture: leaf.material_bucket_id,
            index_offset: leaf.index_offset,
            index_count: leaf.index_count,
        });
        Self::from_leaves(world.indices.len() / 3, leaves, world.texture_names.clone())
    }

    /// A leaf naming a texture outside `texture_names`, or indices past the
    /// mesh, leaves its triangles untextured rather than failing the load.
    fn from_leaves(
        triangle_count: usize,
        leaves: impl IntoIterator<Item = LeafSurface>,
        texture_names: Vec<String>,
    ) -> Self {
        let mut triangle_textures = vec![NO_TEXTURE; triangle_count];
        for leaf in leaves {
            let texture = u16::try_from(leaf.texture)
                .ok()
                .filter(|&index| usize::from(index) < texture_names.len())
                .unwrap_or(NO_TEXTURE);
            let first = (leaf.index_offset / 3) as usize;
            let last = (leaf.index_offset.saturating_add(leaf.index_count) / 3) as usize;
            if let Some(range) = triangle_textures.get_mut(first..last.min(triangle_count)) {
                range.fill(texture);
            }
        }
        Self {
            triangle_textures,
            texture_names,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn for_test(triangle_textures: Vec<u16>, texture_names: Vec<String>) -> Self {
        Self {
            triangle_textures,
            texture_names,
        }
    }

    fn texture_of(&self, triangle: u32) -> Option<u16> {
        self.triangle_textures
            .get(triangle as usize)
            .copied()
            .filter(|&texture| texture != NO_TEXTURE)
    }
}

/// CPU time the tick overlap check spends, for its timing stage. The fixed
/// tick enables it per tick only while stage timing is on; disabled, a check
/// reads no clock.
#[derive(Debug, Default)]
pub struct OverlapCheckTimer {
    enabled: AtomicBool,
    calls: AtomicU64,
    nanos: AtomicU64,
}

impl OverlapCheckTimer {
    /// Start a timed span: zero the totals and time checks until [`Self::finish`].
    pub fn begin(&self, enabled: bool) {
        self.calls.store(0, Ordering::Relaxed);
        self.nanos.store(0, Ordering::Relaxed);
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// End the span: nanoseconds spent, or `None` when no check ran timed.
    pub fn finish(&self) -> Option<u64> {
        self.enabled.store(false, Ordering::Relaxed);
        (self.calls.load(Ordering::Relaxed) > 0).then(|| self.nanos.load(Ordering::Relaxed))
    }

    pub(crate) fn time<T>(&self, check: impl FnOnce() -> T) -> T {
        if !self.enabled.load(Ordering::Relaxed) {
            return check();
        }
        let start = Instant::now();
        let result = check();
        let nanos = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.nanos.fetch_add(nanos, Ordering::Relaxed);
        result
    }
}

impl CollisionWorld {
    /// Does a capsule at `position` intersect static geometry: overlap a
    /// surface by more than [`OVERLAP_TOLERANCE`], or have a core point (its
    /// centre or either end of its segment) inside a solid or exterior cell?
    /// Resting inside the skin band is contact, not overlap.
    pub fn overlaps_static(&self, position: Vec3, capsule: CollisionCapsule) -> bool {
        self.static_overlap(position, capsule).is_some()
    }

    /// The overlap [`Self::overlaps_static`] finds, with the texture of the
    /// triangle overlapped deepest. Depth comes from the engine's exact
    /// capsule–triangle closest points, as the skin cast's does: parry's GJK
    /// intersection test drifts by millimetres beside the edges of large
    /// triangles (`capsule_triangle`), which the 0.5 mm tolerance cannot absorb.
    pub fn static_overlap(
        &self,
        position: Vec3,
        capsule: CollisionCapsule,
    ) -> Option<StaticOverlap> {
        let pose = Pose::from_translation(position);
        let shape = ParryCapsule::new_y(capsule.half_height, capsule.radius);
        let mut deepest: Option<(f32, u32)> = None;
        for triangle in self.mesh.bvh().intersect_aabb(&shape.compute_aabb(&pose)) {
            let depth = triangle_depth(&pose, &shape, &self.mesh.triangle(triangle));
            if depth > OVERLAP_TOLERANCE && deepest.is_none_or(|(deepest, _)| depth > deepest) {
                deepest = Some((depth, triangle));
            }
        }
        if let Some((_, triangle)) = deepest {
            return Some(StaticOverlap {
                position,
                texture: self.surfaces.texture_of(triangle),
            });
        }
        self.core_in_solid(position, capsule)
            .then_some(StaticOverlap {
                position,
                texture: None,
            })
    }

    fn core_in_solid(&self, position: Vec3, capsule: CollisionCapsule) -> bool {
        [
            position,
            position + Vec3::Y * capsule.half_height,
            position - Vec3::Y * capsule.half_height,
        ]
        .iter()
        .any(|&point| self.solid_cells.contains(point))
    }

    /// The level's name for a [`StaticOverlap::texture`].
    pub fn texture_name(&self, texture: u16) -> Option<&str> {
        self.surfaces
            .texture_names
            .get(usize::from(texture))
            .map(String::as_str)
    }

    /// Claim this level load's one overlap warning: `true` for the first caller,
    /// `false` after. Every level load builds a new world, which re-arms it.
    pub fn claim_overlap_warning(&self) -> bool {
        !self.overlap_warned.swap(true, Ordering::Relaxed)
    }

    /// The overlap check's CPU timer, which the fixed tick brackets per tick.
    pub fn overlap_timer(&self) -> &OverlapCheckTimer {
        &self.overlap_timer
    }
}

/// How far a capsule at `pose` sinks into one triangle: the exact separation
/// negated, the radius when its core pierces or touches it, zero when clear.
pub(super) fn triangle_depth(pose: &Pose, shape: &ParryCapsule, triangle: &Triangle) -> f32 {
    if triangle.normal().is_none() {
        // Degenerate: no exact contact; parry's intersection test is all there is.
        let intersects = intersection_test(pose, shape, &Pose::IDENTITY, triangle)
            .is_ok_and(|hit| hit.intersecting);
        return if intersects { shape.radius } else { 0.0 };
    }
    match capsule_triangle::contact(&pose.inverse(), shape, triangle) {
        Some(contact) => (-contact.dist).max(0.0),
        None => shape.radius,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(texture: u32, first_triangle: u32, triangles: u32) -> LeafSurface {
        LeafSurface {
            texture,
            index_offset: first_triangle * 3,
            index_count: triangles * 3,
        }
    }

    #[test]
    fn each_leaf_textures_its_own_triangle_range() {
        let names = vec![
            String::from("metal/floor_grate"),
            String::from("concrete/wall"),
        ];
        let table = SurfaceTable::from_leaves(5, [leaf(1, 0, 2), leaf(0, 2, 2)], names);
        let textures: Vec<_> = (0..5).map(|triangle| table.texture_of(triangle)).collect();
        assert_eq!(textures, [Some(1), Some(1), Some(0), Some(0), None]);
    }

    #[test]
    fn a_leaf_past_the_names_or_the_mesh_leaves_its_triangles_untextured() {
        let names = vec![String::from("metal/floor_grate")];
        // Texture 3 has no name; the second leaf runs past the mesh's end.
        let table = SurfaceTable::from_leaves(3, [leaf(3, 0, 1), leaf(0, 2, 4)], names);
        let textures: Vec<_> = (0..4).map(|triangle| table.texture_of(triangle)).collect();
        assert_eq!(textures, [None, None, Some(0), None]);
    }
}
