// Bounded-region portal walk: one grow-only screen rect per cell.
// See: context/lib/rendering_pipeline.md §2

mod geometry;
mod rect;
mod scratch;

#[cfg(test)]
mod oracle;
#[cfg(test)]
mod test_fixtures;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_cases;
#[cfg(test)]
mod tests_seeded;

use glam::{Mat4, Vec3, Vec4};

use crate::portal_vis::clip_polygon_to_planes;
use crate::visibility::{
    Frustum, FrustumPlane, NEAR_PLANE_INDEX, extract_frustum_planes, is_aabb_outside_frustum,
};
use geometry::{distance_to_polygon, wholly_behind};
use postretro_level_loader::LevelWorld;
pub use scratch::PortalWalkScratch;

pub(crate) use rect::ScreenRect;

/// Eye-to-polygon distance within which a portal is crossed unclipped. Covers
/// float and locator disagreement at a shared plane, where projection
/// degenerates; a distant portal on the eye's plane is clipped normally.
pub(crate) const PORTAL_BYPASS_DISTANCE: f32 = 0.01;
/// A walk crosses a portal out of a cell only when the eye lies on that
/// cell's side of the portal plane by more than this. The oracle's
/// backward-crossing exception uses the same predicate.
const FACING_EPSILON: f32 = 1e-3;

/// The facing rule: `source_side` is the eye's signed distance to a portal
/// plane, positive on the side of the cell being left. NaN never crosses.
pub(crate) fn crosses_from_source_side(source_side: f32) -> bool {
    source_side > FACING_EPSILON
}

/// The view one walk projects through.
#[derive(Debug, Clone)]
pub(crate) struct WalkView {
    eye: Vec3,
    rows: [Vec4; 4],
    near: FrustumPlane,
    far: FrustumPlane,
    /// Seeded entry: a portal wholly behind the caller's near plane is
    /// rejected before the bypass is tested.
    near_gates_bypass: bool,
    /// The full view frustum with this view's near plane: the seeded entry's
    /// off-screen seed test.
    frustum: Frustum,
}

impl WalkView {
    /// Camera entry: the render near plane slides to the eye, so a portal the
    /// eye is pressed against still clips to a polygon.
    pub(crate) fn camera(eye: Vec3, view_proj: Mat4) -> Self {
        let mut frustum = extract_frustum_planes(view_proj);
        frustum.slide_near_plane_to(eye);
        Self::from_frustum(eye, view_proj, frustum, false)
    }

    /// Seeded entry: the caller's eye, view-projection and near plane, as
    /// given. Nothing slides.
    pub(crate) fn seeded(eye: Vec3, view_proj: Mat4, near: FrustumPlane) -> Self {
        let mut frustum = extract_frustum_planes(view_proj);
        frustum.planes[NEAR_PLANE_INDEX] = near;
        Self::from_frustum(eye, view_proj, frustum, true)
    }

    fn from_frustum(eye: Vec3, view_proj: Mat4, frustum: Frustum, near_gates_bypass: bool) -> Self {
        Self {
            eye,
            rows: [
                view_proj.row(0),
                view_proj.row(1),
                view_proj.row(2),
                view_proj.row(3),
            ],
            near: frustum.planes[NEAR_PLANE_INDEX],
            far: frustum.planes[NEAR_PLANE_INDEX + 1],
            near_gates_bypass,
            frustum,
        }
    }

    pub(crate) fn frustum(&self) -> &Frustum {
        &self.frustum
    }
}

/// Counters for one walk. `considered` counts every outbound portal looked
/// at, before any rejection, and is what the step cap bounds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RegionWalkStats {
    pub considered: u32,
    pub accepted: u32,
    pub rejected_blocked: u32,
    pub rejected_solid: u32,
    pub rejected_clipped: u32,
    pub rejected_invalid: u32,
    pub rejected_facing: u32,
    pub bypassed: u32,
    pub expansions: u32,
    pub step_limit_hit: bool,
}

/// The walk's world-side inputs.
#[derive(Clone, Copy)]
pub(crate) struct WalkWorld<'a> {
    pub world: &'a LevelWorld,
    pub blocked_portals: &'a [bool],
}

/// Walk from the camera cell with the full screen. The caller has already
/// located the camera and ruled out solid, exterior and out-of-range cells.
pub(crate) fn walk_from_camera(
    input: WalkWorld,
    view: &WalkView,
    camera_cell: usize,
    step_limit: u32,
    scratch: &mut PortalWalkScratch,
) -> RegionWalkStats {
    let mut stats = RegionWalkStats::default();
    scratch.begin(input.world.cell_count());
    scratch.offer(camera_cell, &ScreenRect::FULL, 0.0);
    drain(input, view, step_limit, scratch, &mut stats);
    stats
}

/// Walk from caller-given seed cells, each with the full screen. Seeds share
/// one region state and one cap; every usable seed is queued before any cell
/// expands, so seed order changes neither the set nor the count. Seeds that
/// are out of range, solid, exterior or wholly off screen are skipped. A trip
/// is the caller's to handle: the stats say so and the walk picks no
/// fallback.
pub(crate) fn walk_from_seeds(
    input: WalkWorld,
    view: &WalkView,
    seeds: &[usize],
    step_limit: u32,
    scratch: &mut PortalWalkScratch,
) -> RegionWalkStats {
    let mut stats = RegionWalkStats::default();
    let world = input.world;
    scratch.begin(world.cell_count());
    for &seed in seeds {
        let Some(cell) = world.cells.get(seed) else {
            continue;
        };
        if cell.is_solid
            || cell.is_exterior
            || is_aabb_outside_frustum(cell.bounds_min, cell.bounds_max, &view.frustum)
        {
            continue;
        }
        let nearest = view.eye.clamp(cell.bounds_min, cell.bounds_max);
        scratch.offer(seed, &ScreenRect::FULL, nearest.distance(view.eye));
    }
    drain(input, view, step_limit, scratch, &mut stats);
    stats
}

fn drain(
    input: WalkWorld,
    view: &WalkView,
    step_limit: u32,
    scratch: &mut PortalWalkScratch,
    stats: &mut RegionWalkStats,
) {
    while let Some((cell, rect)) = scratch.pop() {
        stats.expansions += 1;
        if !expand_cell(input, view, cell, rect, step_limit, scratch, stats) {
            stats.step_limit_hit = true;
            return;
        }
    }
}

/// Test each outbound portal of `cell` against its rect and offer the
/// neighbour the clipped opening. Returns false when the cap stops it; the
/// cap is checked before each portal test, so a walk needing exactly
/// `step_limit` tests completes.
fn expand_cell(
    input: WalkWorld,
    view: &WalkView,
    cell: usize,
    rect: ScreenRect,
    step_limit: u32,
    scratch: &mut PortalWalkScratch,
    stats: &mut RegionWalkStats,
) -> bool {
    let world = input.world;
    let planes_by_portal = world.portal_planes();
    scratch.planes.clear();
    scratch.planes.push(view.near);
    rect.push_planes(&view.rows, &mut scratch.planes);
    scratch.planes.push(view.far);

    for i in 0..world.cell_portal_count(cell) {
        if stats.considered >= step_limit {
            return false;
        }
        stats.considered += 1;

        let Some(portal_idx) = world.cell_portal_index(cell, i) else {
            stats.rejected_invalid += 1;
            continue;
        };
        let portal = &world.portals[portal_idx];
        let neighbor = if portal.front_cell == cell {
            portal.back_cell
        } else {
            portal.front_cell
        };
        // Blocked first: a closed door stays shut even against the eye.
        if input
            .blocked_portals
            .get(portal_idx)
            .copied()
            .unwrap_or(false)
        {
            stats.rejected_blocked += 1;
            continue;
        }
        if neighbor >= world.cell_count() {
            stats.rejected_invalid += 1;
            continue;
        }
        if world.cell_is_solid(neighbor) {
            stats.rejected_solid += 1;
            continue;
        }
        if view.near_gates_bypass && wholly_behind(&view.near, &portal.polygon) {
            stats.rejected_clipped += 1;
            continue;
        }

        let plane = planes_by_portal[portal_idx];
        let (contribution, key) =
            if distance_to_polygon(view.eye, &portal.polygon, &plane) <= PORTAL_BYPASS_DISTANCE {
                // Projection degenerates this close; the neighbour sees what
                // this cell sees, never more.
                stats.bypassed += 1;
                (rect, 0.0)
            } else {
                if plane.oriented {
                    let distance = plane.signed_distance(view.eye);
                    let source_side = if portal.front_cell == cell {
                        distance
                    } else {
                        -distance
                    };
                    if !crosses_from_source_side(source_side) {
                        stats.rejected_facing += 1;
                        continue;
                    }
                }
                let clipped = clip_polygon_to_planes(
                    &portal.polygon,
                    &scratch.planes,
                    &mut scratch.clip_a,
                    &mut scratch.clip_b,
                );
                if clipped.len() < 3 {
                    stats.rejected_clipped += 1;
                    continue;
                }
                let key = clipped
                    .iter()
                    .map(|v| v.distance_squared(view.eye))
                    .fold(f32::MAX, f32::min)
                    .sqrt();
                let contribution = match ScreenRect::bound_points(clipped, &view.rows) {
                    // Touches the eye plane: hand on the source rect.
                    None => rect,
                    Some(bounds) => bounds
                        .intersect(&rect)
                        .unwrap_or_else(|| bounds.clamp_into(&rect)),
                };
                (contribution, key)
            };
        stats.accepted += 1;
        scratch.offer(neighbor, &contribution, key);
    }
    true
}
