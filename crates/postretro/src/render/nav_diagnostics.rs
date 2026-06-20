// Navmesh diagnostic overlay: emits debug-line segments for the baked nav
// region rectangles and portal edges. Gated on `dev-tools`.
// See: context/lib/rendering_pipeline.md §12

use glam::Vec3;

use super::debug_lines::DebugLineRenderer;
use crate::nav::NavGraph;

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
pub(super) fn emit(graph: &NavGraph, lines: &mut DebugLineRenderer) {
    for region in graph.regions() {
        let y = region.floor_y_min;
        let min = region.world_min_xz;
        let max = region.world_max_xz;
        let c00 = Vec3::new(min[0], y, min[1]);
        let c10 = Vec3::new(max[0], y, min[1]);
        let c11 = Vec3::new(max[0], y, max[1]);
        let c01 = Vec3::new(min[0], y, max[1]);
        lines.push_line(c00, c10, COLOR_REGION);
        lines.push_line(c10, c11, COLOR_REGION);
        lines.push_line(c11, c01, COLOR_REGION);
        lines.push_line(c01, c00, COLOR_REGION);
    }

    for portal in graph.portals() {
        lines.push_line(
            Vec3::from(portal.left),
            Vec3::from(portal.right),
            COLOR_PORTAL,
        );
    }
}

/// Corridor segment from the agent toward each remaining waypoint.
const COLOR_AGENT_PATH: [u8; 4] = [120, 255, 120, 255];
/// Waypoint marker (a small cross) at each funnel waypoint.
const COLOR_AGENT_WAYPOINT: [u8; 4] = [255, 120, 220, 255];

/// Emit one frame of agent path/corridor diagnostic lines: the corridor from the
/// agent's current `position` through its remaining funnel waypoints (starting at
/// `cursor`), plus a small cross marker at each remaining waypoint. The marker
/// arm length scales with the agent `radius` so a fatter agent reads larger.
///
/// Like [`emit`], this is purely additive over the already-cleared debug-line
/// buffer and depth-tested (`push_line`), so the corridor reads as in-world.
/// No-op when the path is empty.
pub(super) fn emit_agent_path(
    position: Vec3,
    path: &[Vec3],
    cursor: usize,
    radius: f32,
    lines: &mut DebugLineRenderer,
) {
    let remaining = path.get(cursor..).unwrap_or(&[]);
    if remaining.is_empty() {
        return;
    }

    // Corridor: agent → first remaining waypoint → … → destination.
    let mut prev = position;
    for &waypoint in remaining {
        lines.push_line(prev, waypoint, COLOR_AGENT_PATH);
        prev = waypoint;
    }

    // Waypoint markers: an axis cross sized to the agent capsule radius so each
    // funnel corner is visible against the corridor line.
    let arm = radius.max(0.05);
    for &waypoint in remaining {
        lines.push_line(
            waypoint - Vec3::new(arm, 0.0, 0.0),
            waypoint + Vec3::new(arm, 0.0, 0.0),
            COLOR_AGENT_WAYPOINT,
        );
        lines.push_line(
            waypoint - Vec3::new(0.0, 0.0, arm),
            waypoint + Vec3::new(0.0, 0.0, arm),
            COLOR_AGENT_WAYPOINT,
        );
        lines.push_line(
            waypoint - Vec3::new(0.0, arm, 0.0),
            waypoint + Vec3::new(0.0, arm, 0.0),
            COLOR_AGENT_WAYPOINT,
        );
    }
}
