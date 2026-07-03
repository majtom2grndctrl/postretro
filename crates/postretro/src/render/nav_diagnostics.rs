// Navmesh diagnostic overlay: emits debug-line segments for the baked nav
// region rectangles and portal edges. Gated on `dev-tools`.
// See: context/lib/rendering_pipeline.md §12

use glam::Vec3;

use crate::nav::NavGraph;
use crate::render::Renderer;

/// Region footprint outline, drawn on the floor band so it visibly hugs floors.
const COLOR_REGION: [u8; 4] = [60, 200, 255, 220];
/// Portal traversable edge between two regions.
const COLOR_PORTAL: [u8; 4] = [255, 200, 60, 255];

/// Emit one frame of navmesh diagnostic line segments from the runtime graph.
///
/// Region rectangles are drawn as the four edges of their world-space XZ
/// footprint, placed at the region's `floor_y_min` so the outline sits on the
/// floor (and stops at walls, since the bake only emits walkable cells). Portal
/// segments are drawn from their stored world-space endpoints.
///
/// Depth-tested (`push_line`, not the overlay pipeline) so the overlay reads as
/// in-world geometry occluded by walls rather than x-ray.
///
/// The frame loop clears the debug-line buffer before this call, so this
/// function is purely additive and never owns the buffer lifecycle.
///
/// Gated once up front on the navmesh overlay toggle: when it is off, skip the
/// whole region/portal walk rather than building segments the renderer would
/// buffer unseen. `push_debug_line` is an unconditional primitive, so this
/// early-out is the sole gate.
pub(crate) fn emit(renderer: &mut Renderer, graph: &NavGraph) {
    if !renderer.nav_overlay_enabled() {
        return;
    }

    for region in graph.regions() {
        let y = region.floor_y_min;
        let min = region.world_min_xz;
        let max = region.world_max_xz;
        let c00 = Vec3::new(min[0], y, min[1]);
        let c10 = Vec3::new(max[0], y, min[1]);
        let c11 = Vec3::new(max[0], y, max[1]);
        let c01 = Vec3::new(min[0], y, max[1]);
        renderer.push_debug_line(c00, c10, COLOR_REGION);
        renderer.push_debug_line(c10, c11, COLOR_REGION);
        renderer.push_debug_line(c11, c01, COLOR_REGION);
        renderer.push_debug_line(c01, c00, COLOR_REGION);
    }

    for portal in graph.portals() {
        renderer.push_debug_line(
            Vec3::from(portal.left),
            Vec3::from(portal.right),
            COLOR_PORTAL,
        );
    }
}
