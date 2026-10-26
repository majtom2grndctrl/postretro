// Bounded-region portal walk: one grow-only screen rect per cell.
// See: context/lib/rendering_pipeline.md §2

mod entries;
mod geometry;
mod rect;
mod scratch;
mod trace;

#[cfg(test)]
mod oracle;
#[cfg(test)]
mod real_map_mirror;
#[cfg(test)]
mod real_map_orientation;
#[cfg(test)]
mod real_map_sweep;
#[cfg(test)]
pub(crate) mod test_fixtures;
#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod tests_cases;
#[cfg(test)]
mod tests_seeded;
#[cfg(test)]
mod tests_traversal;

use glam::{Mat4, Vec3, Vec4};

use crate::portal_vis::clip_polygon_to_planes;
#[cfg(test)]
use crate::visibility::Frustum;
use crate::visibility::{
    FAR_PLANE_INDEX, FrustumPlane, NEAR_PLANE_INDEX, frustum_plane_array, is_aabb_outside_planes,
    slide_plane_to,
};
use geometry::{distance_to_polygon, wholly_behind};
use postretro_level_loader::LevelWorld;
use trace::Reject;

pub(crate) use entries::walk_seeded;
pub use entries::{PortalTraversal, SeededWalk, portal_traverse, portal_walk_seeded};
pub(crate) use rect::ScreenRect;
pub use scratch::PortalWalkScratch;
pub(crate) use trace::{WalkTrace, trace_out_of_range};

/// Per-walk cap on portal tests (`RegionWalkStats::considered`). A backstop,
/// not a path ordinary poses take: it keeps at least 2× headroom over the
/// worst walk measured on the fixtures and swept maps. A trip falls back to
/// bounded frustum-culled sets on the camera entry and returns to the caller
/// on the seeded entry.
pub(crate) const MAX_PORTAL_WALK_STEPS: u32 = 5_000;

/// Eye-to-polygon distance within which a portal is crossed unclipped. Covers
/// float and locator disagreement at a shared plane, where projection
/// degenerates; a distant portal on the eye's plane is clipped normally.
pub(crate) const PORTAL_BYPASS_DISTANCE: f32 = 0.01;
/// A walk crosses a portal out of a cell only when the eye lies on that
/// cell's side of the portal plane by more than this. It absorbs f32 error in
/// the signed distance at map-scale coordinates; it is not a slab, so an eye
/// just off a distant portal's plane still crosses it. An eye near the
/// polygon itself takes the bypass first. The oracle's backward-crossing
/// exception uses the same predicate.
const FACING_EPSILON: f32 = 1e-4;

/// The facing rule: `source_side` is the eye's signed distance to a portal
/// plane, positive on the side of the cell being left. NaN never crosses.
pub(crate) fn crosses_from_source_side(source_side: f32) -> bool {
    source_side > FACING_EPSILON
}

/// The view one walk projects through. Plain values, so building one per
/// frame allocates nothing.
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
    planes: [FrustumPlane; 6],
    #[cfg(test)]
    frustum: Frustum,
}

impl WalkView {
    /// Camera entry: the render near plane slides to the eye, so a portal the
    /// eye is pressed against still clips to a polygon.
    pub(crate) fn camera(eye: Vec3, view_proj: Mat4) -> Self {
        let mut planes = frustum_plane_array(view_proj);
        slide_plane_to(&mut planes[NEAR_PLANE_INDEX], eye);
        Self::from_planes(eye, view_proj, planes, false)
    }

    /// Seeded entry: the caller's eye, view-projection and near plane, as
    /// given. Nothing slides.
    pub(crate) fn seeded(eye: Vec3, view_proj: Mat4, near: FrustumPlane) -> Self {
        Self::from_planes(eye, view_proj, seeded_view_planes(view_proj, near), true)
    }

    fn from_planes(
        eye: Vec3,
        view_proj: Mat4,
        planes: [FrustumPlane; 6],
        near_gates_bypass: bool,
    ) -> Self {
        Self {
            eye,
            rows: [
                view_proj.row(0),
                view_proj.row(1),
                view_proj.row(2),
                view_proj.row(3),
            ],
            near: planes[NEAR_PLANE_INDEX],
            far: planes[FAR_PLANE_INDEX],
            near_gates_bypass,
            planes,
            #[cfg(test)]
            frustum: Frustum {
                planes: planes.to_vec(),
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn frustum(&self) -> &Frustum {
        &self.frustum
    }

    /// The planes a cell holding `rect` clips its portals with: this view's
    /// near plane, the rect's padded side planes, and its far plane.
    fn load_clip_planes(&self, rect: &ScreenRect, out: &mut Vec<FrustumPlane>) {
        out.clear();
        out.push(self.near);
        rect.push_planes(&self.rows, out);
        out.push(self.far);
    }
}

/// The seeded entry's view frustum: `view_proj`'s planes with `near` in
/// place of its near plane. Its off-screen seed test, and the mirror's cull.
pub(crate) fn seeded_view_planes(view_proj: Mat4, near: FrustumPlane) -> [FrustumPlane; 6] {
    let mut planes = frustum_plane_array(view_proj);
    planes[NEAR_PLANE_INDEX] = near;
    planes
}

/// Whether the seeded entry walks from `seed` under `planes`
/// ([`seeded_view_planes`]): in range, not solid, not exterior, and not wholly
/// outside the frustum.
pub(crate) fn seed_is_usable(world: &LevelWorld, planes: &[FrustumPlane], seed: usize) -> bool {
    world.cells.get(seed).is_some_and(|cell| {
        !cell.is_solid
            && !cell.is_exterior
            && !is_aabb_outside_planes(cell.bounds_min, cell.bounds_max, planes)
    })
}

/// The walk's outermost clip region for `view`, in `scratch`'s plane buffer:
/// the full screen's clip planes, padded as every rect's are. Every polygon
/// the walk keeps lies inside it within `CLIP_EPSILON`, so every cell it
/// reaches through a clipped portal touches it; the step-limit fallback's
/// fog test reads it.
pub(crate) fn screen_clip_planes<'a>(
    view: &WalkView,
    scratch: &'a mut PortalWalkScratch,
) -> &'a [FrustumPlane] {
    view.load_clip_planes(&ScreenRect::FULL, &mut scratch.planes);
    &scratch.planes
}

/// Counters for one walk. `considered` counts every outbound portal looked
/// at, before any rejection, and is what the step cap bounds. A rejection
/// behind the seeded entry's near plane counts as clipped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RegionWalkStats {
    pub considered: u32,
    pub accepted: u32,
    pub rejected_blocked: u32,
    pub rejected_solid: u32,
    pub rejected_clipped: u32,
    pub rejected_invalid: u32,
    #[cfg(test)]
    pub rejected_facing: u32,
    #[cfg(test)]
    pub bypassed: u32,
    #[cfg(test)]
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
    walk_from_camera_traced(input, view, camera_cell, step_limit, scratch, None)
}

/// [`walk_from_camera`], writing one trace line per portal test into `trace`
/// when the capture is armed.
pub(crate) fn walk_from_camera_traced(
    input: WalkWorld,
    view: &WalkView,
    camera_cell: usize,
    step_limit: u32,
    scratch: &mut PortalWalkScratch,
    trace: Option<&mut WalkTrace>,
) -> RegionWalkStats {
    let mut stats = RegionWalkStats::default();
    scratch.begin(input.world.cell_count());
    scratch.offer(camera_cell, &ScreenRect::FULL, 0.0);
    drain(input, view, step_limit, scratch, &mut stats, trace);
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
        if !seed_is_usable(world, &view.planes, seed) {
            continue;
        }
        let cell = &world.cells[seed];
        let nearest = view.eye.clamp(cell.bounds_min, cell.bounds_max);
        scratch.offer(seed, &ScreenRect::FULL, nearest.distance(view.eye));
    }
    drain(input, view, step_limit, scratch, &mut stats, None);
    stats
}

fn drain(
    input: WalkWorld,
    view: &WalkView,
    step_limit: u32,
    scratch: &mut PortalWalkScratch,
    stats: &mut RegionWalkStats,
    mut trace: Option<&mut WalkTrace>,
) {
    while let Some((cell, rect)) = scratch.pop() {
        #[cfg(test)]
        {
            stats.expansions += 1;
        }
        if !expand_cell(
            input,
            view,
            cell,
            rect,
            step_limit,
            scratch,
            stats,
            trace.as_deref_mut(),
        ) {
            stats.step_limit_hit = true;
            if let Some(trace) = trace {
                trace.limit(cell, step_limit);
            }
            return;
        }
    }
}

/// Test each outbound portal of `cell` against its rect and offer the
/// neighbour the clipped opening. Returns false when the cap stops it; the
/// cap is checked before each portal test, so a walk needing exactly
/// `step_limit` tests completes.
#[allow(clippy::too_many_arguments)]
fn expand_cell(
    input: WalkWorld,
    view: &WalkView,
    cell: usize,
    rect: ScreenRect,
    step_limit: u32,
    scratch: &mut PortalWalkScratch,
    stats: &mut RegionWalkStats,
    mut trace: Option<&mut WalkTrace>,
) -> bool {
    let world = input.world;
    let planes_by_portal = world.portal_planes();
    view.load_clip_planes(&rect, &mut scratch.planes);

    for i in 0..world.cell_portal_count(cell) {
        if stats.considered >= step_limit {
            return false;
        }
        stats.considered += 1;

        // A ref past the portal list counts as invalid, like a missing ref.
        let found = world
            .cell_portal_index(cell, i)
            .and_then(|idx| Some((idx, world.portals.get(idx)?, *planes_by_portal.get(idx)?)));
        let Some((portal_idx, portal, plane)) = found else {
            stats.rejected_invalid += 1;
            if let Some(trace) = trace.as_deref_mut() {
                trace.reject(cell, None, 0, Reject::Invalid);
            }
            continue;
        };
        let verts = portal.polygon.len();
        let neighbor = if portal.front_cell == cell {
            portal.back_cell
        } else {
            portal.front_cell
        };
        // Blocked first: a closed door stays shut even against the eye.
        let reject = if input
            .blocked_portals
            .get(portal_idx)
            .copied()
            .unwrap_or(false)
        {
            stats.rejected_blocked += 1;
            Some(Reject::Blocked)
        } else if neighbor >= world.cell_count() {
            stats.rejected_invalid += 1;
            Some(Reject::Invalid)
        } else if world.cell_is_solid(neighbor) {
            stats.rejected_solid += 1;
            Some(Reject::Solid)
        } else if view.near_gates_bypass && wholly_behind(&view.near, &portal.polygon) {
            stats.rejected_clipped += 1;
            Some(Reject::Near)
        } else {
            None
        };
        if let Some(reason) = reject {
            if let Some(trace) = trace.as_deref_mut() {
                trace.reject(cell, Some(neighbor), verts, reason);
            }
            continue;
        }

        let bypass =
            distance_to_polygon(view.eye, &portal.polygon, &plane) <= PORTAL_BYPASS_DISTANCE;
        let (contribution, key, clipped_len) = if bypass {
            // Projection degenerates this close; the neighbour sees what
            // this cell sees, never more.
            #[cfg(test)]
            {
                stats.bypassed += 1;
            }
            (rect, 0.0, verts)
        } else {
            if plane.oriented {
                let distance = plane.signed_distance(view.eye);
                let source_side = if portal.front_cell == cell {
                    distance
                } else {
                    -distance
                };
                if !crosses_from_source_side(source_side) {
                    #[cfg(test)]
                    {
                        stats.rejected_facing += 1;
                    }
                    if let Some(trace) = trace.as_deref_mut() {
                        trace.reject(cell, Some(neighbor), verts, Reject::Facing);
                    }
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
                if let Some(trace) = trace.as_deref_mut() {
                    trace.reject(cell, Some(neighbor), verts, Reject::Clip(clipped.len()));
                }
                continue;
            }
            let key = clipped
                .iter()
                .map(|v| v.distance_squared(view.eye))
                .fold(f32::MAX, f32::min)
                .sqrt();
            // No usable bounds (the polygon touches the eye plane, or the
            // projection is degenerate), or bounds that miss the rect (rare
            // near-eye boundary cases): hand on the whole source rect. That
            // only over-includes, and is not monotone in the source rect,
            // but the walk still terminates: the contribution never leaves
            // the source rect.
            let contribution = ScreenRect::bound_points(clipped, &view.rows)
                .and_then(|bounds| bounds.intersect(&rect))
                .unwrap_or(rect);
            (contribution, key, clipped.len())
        };
        stats.accepted += 1;
        if let Some(trace) = trace.as_deref_mut() {
            trace.accept(cell, neighbor, clipped_len, bypass);
        }
        scratch.offer(neighbor, &contribution, key);
    }
    true
}
