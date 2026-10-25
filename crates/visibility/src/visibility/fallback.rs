// Frustum-culled cell sets for the paths that run no complete portal walk.
// See: context/lib/rendering_pipeline.md §2

use glam::Vec3;

use super::frustum::{Frustum, FrustumPlane, is_aabb_outside_frustum, is_aabb_outside_planes_by};
use crate::portal_vis::CLIP_EPSILON;
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
/// non-exterior cell whose AABB passes `clip_planes` within `CLIP_EPSILON`,
/// or lies within `PORTAL_BYPASS_DISTANCE` of `eye`, plus the camera cell.
/// Ascending.
///
/// It must hold every cell the walk could reach, so it bounds the walk from
/// above in two ways. `clip_planes` is the walk's own outermost clip region
/// (`region_walk::screen_clip_planes`): the near plane slid to the eye, the
/// far plane, and the full screen's side planes pushed out by the clip pad.
/// The walk clips every portal against a rect's padded planes and keeps
/// vertices within `CLIP_EPSILON` of them, so a reached cell can lie wholly
/// in the sliver just outside a screen edge; the unpadded camera frustum
/// would drop it, and the unslid near cutoff (about 0.05 units) would drop a
/// reachable cell in the near slab. The distance keep covers the bypass: a
/// portal within that distance of the eye hands its far cell the source
/// cell's region unclipped, so that cell can lie wholly behind the eye or
/// beside it outside a side plane. The plane test culls per plane, so
/// widening the near plane alone would still drop the cell beside the eye.
///
/// The camera cell is forced in so the set is never empty (empty means "no
/// portal isolation" downstream) even when its AABB sits wholly behind the
/// near plane.
pub(super) fn fog_reachable_frustum_fallback(
    cells: &[CellData],
    clip_planes: &[FrustumPlane],
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
                    || !is_aabb_outside_planes_by(
                        cell.bounds_min,
                        cell.bounds_max,
                        clip_planes,
                        CLIP_EPSILON,
                    ))
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
