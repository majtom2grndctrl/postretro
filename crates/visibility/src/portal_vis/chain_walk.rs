// Chain walk: per-chain DFS with polygon-vs-frustum clipping and narrowing.
// See: context/lib/rendering_pipeline.md §2

use std::fmt::Write as _;

use glam::Vec3;

use super::PortalTraversalStats;
use super::clip::{camera_on_polygon_plane, clip_polygon_to_frustum, narrow_frustum};
use crate::visibility::Frustum;
use postretro_level_loader::LevelWorld;

// Real maps run 5–10 deep, occasionally ~20. 256 is well above any realistic
// chain depth and well below stack-overflow territory. Tune upward only if a
// real map trips the guard; the visible set is conservative, not incorrect.
const MAX_PORTAL_CHAIN_DEPTH: usize = 256;

// `trace` is `Some(String)` only when capture is armed; event sites check this
// before every write so the hot path allocates nothing when diagnostics are off.
pub(super) struct DfsState<'a> {
    pub(super) world: &'a LevelWorld,
    pub(super) blocked_portals: &'a [bool],
    pub(super) camera_position: Vec3,
    pub(super) trace: Option<String>,
    pub(super) visible: Vec<bool>,
    pub(super) cell_count: usize,
    pub(super) stats: PortalTraversalStats,
    pub(super) step_limit: u32,
    pub(super) depth_limit_warned: bool,
    pub(super) camera_cell: usize,
}

// Recursive per-chain DFS. Mirrors id Tech 4's `FloodViewThroughArea_r`
// (Doom 3, `neo/renderer/RenderWorld_portals.cpp`).
pub(super) fn flood(
    state: &mut DfsState,
    cell: usize,
    frustum: &Frustum,
    path: &mut Vec<usize>,
    clip_scratch_a: &mut Vec<Vec3>,
    clip_scratch_b: &mut Vec<Vec3>,
) {
    // Every chain that reaches this cell contributes to the visible union.
    state.visible[cell] = true;

    if state.stats.step_limit_hit {
        return;
    }

    if state.stats.considered >= state.step_limit {
        state.stats.step_limit_hit = true;
        if let Some(buf) = state.trace.as_mut() {
            let _ = writeln!(buf, "  rej cell={} limit steps={}", cell, state.step_limit,);
        }
        return;
    }

    if path.len() >= MAX_PORTAL_CHAIN_DEPTH {
        state.stats.rejected_depth_limit += 1;
        if !state.depth_limit_warned {
            state.depth_limit_warned = true;
            // Keep this verbose-only: a problematic portal graph can hit the
            // limit every frame, and the caller already chooses a conservative
            // visibility fallback.
            log::debug!(
                target: "postretro::portal_trace",
                "[portal_trace] chain depth limit reached (MAX_PORTAL_CHAIN_DEPTH={}) \
                 camera_cell={} truncated_at_cell={} — visible set conservative \
                 past this point",
                MAX_PORTAL_CHAIN_DEPTH,
                state.camera_cell,
                cell,
            );
        }
        // The `log::debug!` fires once per walk; the trace line fires every
        // time the limit is hit so the event appears inline in the capture.
        if let Some(buf) = state.trace.as_mut() {
            let _ = writeln!(buf, "  rej cell={} depth", cell);
        }
        return;
    }

    let outbound_len = state.world.cell_portal_count(cell);

    // Index rather than iterate: re-borrowing `state.world` each step avoids
    // holding a long-lived borrow across the recursive call (`state` is `&mut`).
    for i in 0..outbound_len {
        if state.stats.considered >= state.step_limit {
            state.stats.step_limit_hit = true;
            if let Some(buf) = state.trace.as_mut() {
                let _ = writeln!(buf, "  rej cell={} limit steps={}", cell, state.step_limit,);
            }
            return;
        }

        let Some(portal_idx) = state.world.cell_portal_index(cell, i) else {
            state.stats.rejected_invalid += 1;
            continue;
        };
        let portal = &state.world.portals[portal_idx];

        let neighbor = if portal.front_cell == cell {
            portal.back_cell
        } else {
            portal.front_cell
        };

        state.stats.considered += 1;

        if state
            .blocked_portals
            .get(portal_idx)
            .copied()
            .unwrap_or(false)
        {
            state.stats.rejected_blocked_portal += 1;
            if let Some(buf) = state.trace.as_mut() {
                let _ = writeln!(
                    buf,
                    "  rej {}->{} v={} blocked",
                    cell,
                    neighbor,
                    portal.polygon.len(),
                );
            }
            continue;
        }

        if neighbor >= state.cell_count {
            state.stats.rejected_invalid += 1;
            continue;
        }

        // Linear scan beats HashSet hashing at typical chain depths (5–10).
        if path.contains(&portal_idx) {
            state.stats.rejected_path_cycle += 1;
            continue;
        }

        if state.world.cell_is_solid(neighbor) {
            state.stats.rejected_solid += 1;
            // For `solid` rejects the clip hasn't run yet, so the "clipped
            // verts" half of the v=c/p pair isn't meaningful. Print only the
            // portal vertex count.
            if let Some(buf) = state.trace.as_mut() {
                let _ = writeln!(
                    buf,
                    "  rej {}->{} v={} solid",
                    cell,
                    neighbor,
                    portal.polygon.len(),
                );
            }
            continue;
        }

        // When the camera sits on the portal's supporting plane, S-H crushes
        // the polygon to a degenerate line (the view cone's cross-section at
        // zero depth is a point). That's geometric truth, not a clipper bug:
        // bypass S-H and feed the full polygon to narrow_frustum directly.
        //
        // Inner scope: borrows of clip_scratch_a/b must end before the
        // recursive call below re-takes &mut of both scratch buffers.
        let (narrowed_opt, clipped_len) = {
            let apex_on_portal_plane =
                camera_on_polygon_plane(state.camera_position, &portal.polygon);
            if apex_on_portal_plane {
                let narrowed = narrow_frustum(state.camera_position, &portal.polygon, frustum);
                (narrowed, portal.polygon.len())
            } else {
                let clipped = clip_polygon_to_frustum(
                    &portal.polygon,
                    frustum,
                    clip_scratch_a,
                    clip_scratch_b,
                );
                let len = clipped.len();
                if len < 3 {
                    (None, len)
                } else {
                    let narrowed = narrow_frustum(state.camera_position, clipped, frustum);
                    (narrowed, len)
                }
            }
        };

        if clipped_len < 3 {
            state.stats.rejected_clipped += 1;
            if let Some(buf) = state.trace.as_mut() {
                let _ = writeln!(
                    buf,
                    "  rej {}->{} v={}/{} clip",
                    cell,
                    neighbor,
                    clipped_len,
                    portal.polygon.len(),
                );
            }
            continue;
        }

        let Some(narrowed) = narrowed_opt else {
            state.stats.rejected_narrow += 1;
            if let Some(buf) = state.trace.as_mut() {
                let _ = writeln!(
                    buf,
                    "  rej {}->{} v={}/{} narrow",
                    cell,
                    neighbor,
                    clipped_len,
                    portal.polygon.len(),
                );
            }
            continue;
        };

        state.stats.accepted += 1;
        if let Some(buf) = state.trace.as_mut() {
            let _ = writeln!(buf, "  acc {}->{} v={}", cell, neighbor, clipped_len);
        }

        // Push/pop so sibling branches at this depth see an unchanged path.
        path.push(portal_idx);
        flood(
            state,
            neighbor,
            &narrowed,
            path,
            clip_scratch_a,
            clip_scratch_b,
        );
        path.pop();
    }
}
