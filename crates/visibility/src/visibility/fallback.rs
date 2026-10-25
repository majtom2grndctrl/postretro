// Frustum-culled cell sets for the paths that run no complete portal walk.
// See: context/lib/rendering_pipeline.md §2

use glam::Vec3;

use super::frustum::{Frustum, is_aabb_outside_frustum};
use crate::region_walk::PORTAL_BYPASS_DISTANCE;
use postretro_level_loader::CellData;

/// Write every drawable cell that passes AABB frustum culling into `out`,
/// ascending.
pub(super) fn visible_cells_frustum_all(cells: &[CellData], frustum: &Frustum, out: &mut Vec<u32>) {
    out.clear();
    out.extend(
        cells
            .iter()
            .enumerate()
            .filter(|(_, cell)| {
                cell.is_drawable
                    && !is_aabb_outside_frustum(cell.bounds_min, cell.bounds_max, frustum)
            })
            .map(|(i, _)| i as u32),
    );
}

/// Fog/light reach for the portal step-limit fallback: every non-solid,
/// non-exterior cell that passes `slid_frustum` or whose AABB lies within
/// `PORTAL_BYPASS_DISTANCE` of `eye`, plus the camera cell. Ascending.
///
/// It must hold every cell the walk could reach, so it bounds the walk from
/// above in two ways. `slid_frustum` is the camera frustum with its near
/// plane slid to the eye, the walk's own starting frustum: every region the
/// walk clips to lies inside it, while the unslid near cutoff (about 0.05
/// units) would drop a reachable cell in the near slab. The distance keep
/// covers the bypass: a portal within that distance of the eye hands its far
/// cell the source cell's region unclipped, so that cell can lie wholly
/// behind the eye or beside it outside a side plane. The frustum test culls
/// per plane, so widening the near plane alone would still drop the cell
/// beside the eye.
///
/// This is the drawing fallback's predicate minus `face_count > 0`,
/// mirroring how the walk's fog reach relates to its draw set. The camera
/// cell is forced in so the set is never empty (empty means "no portal
/// isolation" downstream) even when its AABB sits wholly behind the near
/// plane.
pub(super) fn fog_reachable_frustum_fallback(
    cells: &[CellData],
    slid_frustum: &Frustum,
    eye: Vec3,
    camera_cell: usize,
) -> Vec<u32> {
    cells
        .iter()
        .enumerate()
        .filter(|(i, cell)| {
            !cell.is_solid
                && !cell.is_exterior
                && (*i == camera_cell
                    || within_bypass_distance(eye, cell)
                    || !is_aabb_outside_frustum(cell.bounds_min, cell.bounds_max, slid_frustum))
        })
        .map(|(i, _)| i as u32)
        .collect()
}

/// Eye-to-box distance, without `clamp`'s ordered-bounds precondition.
fn within_bypass_distance(eye: Vec3, cell: &CellData) -> bool {
    let outside = (cell.bounds_min - eye)
        .max(eye - cell.bounds_max)
        .max(Vec3::ZERO);
    outside.length() <= PORTAL_BYPASS_DISTANCE
}
