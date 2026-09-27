// Runtime portal traversal: per-chain DFS with polygon-vs-frustum clipping + narrowing.
// See: context/lib/build_pipeline.md §Runtime visibility

use std::fmt::Write as _;

use glam::Vec3;

use postretro_visibility::{Frustum, FrustumPlane};
use postretro_level_loader::LevelWorld;

// Half-space boundary epsilon for Sutherland-Hodgman. Over-inclusion at the
// boundary is safe: the next narrowing iteration will discard any slop, so the
// strict-subset invariant holds.
const CLIP_EPSILON: f32 = 1e-4;

// Real maps run 5–10 deep, occasionally ~20. 256 is well above any realistic
// chain depth and well below stack-overflow territory. Tune upward only if a
// real map trips the guard; the visible set is conservative, not incorrect.
const MAX_PORTAL_CHAIN_DEPTH: usize = 256;

// Dense detail-brush partitions can create highly cyclic portal graphs where
// the number of distinct portal chains is far larger than the number of cells.
// Keep the per-frame CPU walk bounded; the visibility layer falls back to
// per-cell AABB frustum culling when this trips.
const MAX_PORTAL_WALK_STEPS: u32 = 20_000;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PortalTraversalStats {
    pub considered: u32,
    pub accepted: u32,
    pub rejected_blocked_portal: u32,
    pub rejected_solid: u32,
    pub rejected_clipped: u32,
    pub rejected_narrow: u32,
    pub rejected_invalid: u32,
    pub rejected_path_cycle: u32,
    pub rejected_depth_limit: u32,
    pub step_limit_hit: bool,
}

#[derive(Debug)]
pub(crate) struct PortalTraversalResult {
    pub visible: Vec<bool>,
    pub stats: PortalTraversalStats,
}

// `trace` is `Some(String)` only when capture is armed; event sites check this
// before every write so the hot path allocates nothing when diagnostics are off.
struct DfsState<'a> {
    world: &'a LevelWorld,
    blocked_portals: &'a [bool],
    camera_position: Vec3,
    trace: Option<String>,
    visible: Vec<bool>,
    cell_count: usize,
    stats: PortalTraversalStats,
    step_limit: u32,
    depth_limit_warned: bool,
    camera_cell: usize,
}

/// Cycle prevention keys on *portals crossed in the current chain*, not on
/// cells reached globally — keying on cells would silently drop every chain
/// after the first to arrive at a cell, losing whichever carried the widest
/// sub-frustum. The visible set is the union across all chains.
///
/// By induction, every narrowed frustum is a strict subset of the camera
/// frustum, so a per-cell AABB cull is redundant and omitted.
///
/// `capture: true` emits per-portal events to the `postretro::portal_trace`
/// target as a single batched log message. Triggered by `Alt+Shift+1`; see
/// `context/lib/input.md` §7.
pub fn portal_traverse(
    camera_position: Vec3,
    camera_cell: usize,
    frustum: &Frustum,
    world: &LevelWorld,
    capture: bool,
) -> Vec<bool> {
    portal_traverse_detailed(camera_position, camera_cell, frustum, world, &[], capture).visible
}

pub(crate) fn portal_traverse_detailed(
    camera_position: Vec3,
    camera_cell: usize,
    frustum: &Frustum,
    world: &LevelWorld,
    blocked_portals: &[bool],
    capture: bool,
) -> PortalTraversalResult {
    portal_traverse_with_step_limit(
        camera_position,
        camera_cell,
        frustum,
        world,
        blocked_portals,
        capture,
        MAX_PORTAL_WALK_STEPS,
    )
}

pub(crate) fn portal_traverse_with_step_limit(
    camera_position: Vec3,
    camera_cell: usize,
    frustum: &Frustum,
    world: &LevelWorld,
    blocked_portals: &[bool],
    capture: bool,
    step_limit: u32,
) -> PortalTraversalResult {
    let (visible, trace) = portal_traverse_inner(
        camera_position,
        camera_cell,
        frustum,
        world,
        blocked_portals,
        capture,
        step_limit,
    );
    // One `log::info!` call: one timestamp/target prefix per traced frame
    // instead of one per event.
    if let Some(buf) = trace {
        log::info!(target: "postretro::portal_trace", "[portal_trace]\n{}", buf);
    }
    visible
}

// Split from `portal_traverse` so tests can inspect the formatted trace string
// directly without wiring a test logger.
pub(crate) fn portal_traverse_inner(
    camera_position: Vec3,
    camera_cell: usize,
    frustum: &Frustum,
    world: &LevelWorld,
    blocked_portals: &[bool],
    capture: bool,
    step_limit: u32,
) -> (PortalTraversalResult, Option<String>) {
    let cell_count = world.cell_count();
    let visible = vec![false; cell_count];
    let stats = PortalTraversalStats::default();

    let mut trace = if capture {
        Some(String::with_capacity(512))
    } else {
        None
    };

    // Out-of-range camera cell: emit a single `cell_oor` line into the buffer
    // and bail before any header write that reads the cell.
    if camera_cell >= cell_count {
        if let Some(buf) = trace.as_mut() {
            let _ = writeln!(
                buf,
                "abort cell_oor cam=({:.2},{:.2},{:.2}) cell={} cells={}",
                camera_position.x, camera_position.y, camera_position.z, camera_cell, cell_count,
            );
        }
        return (PortalTraversalResult { visible, stats }, trace);
    }

    // `solid` is omitted from the header — solid cells short-circuit in
    // `determine_visible_cells` before reaching `portal_traverse`.
    if let Some(buf) = trace.as_mut() {
        let (bounds_min, bounds_max) = world
            .cell_bounds(camera_cell)
            .unwrap_or((Vec3::ZERO, Vec3::ZERO));
        let _ = writeln!(
            buf,
            "cam=({:.2},{:.2},{:.2}) cell={} faces={} bnds=({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2}) cells={}",
            camera_position.x,
            camera_position.y,
            camera_position.z,
            camera_cell,
            world.cell_face_count(camera_cell),
            bounds_min.x,
            bounds_min.y,
            bounds_min.z,
            bounds_max.x,
            bounds_max.y,
            bounds_max.z,
            cell_count,
        );
    }

    let mut state = DfsState {
        world,
        blocked_portals,
        camera_position,
        trace,
        visible,
        cell_count,
        stats,
        step_limit,
        depth_limit_warned: false,
        camera_cell,
    };

    // The render pipeline uses a 0.1-unit near clip for depth-buffer
    // precision, but visibility has no such need. Slide the near plane up
    // to the camera apex so portals the player is pressed against aren't
    // clipped to empty at the Near step. See
    // `Frustum::slide_near_plane_to` for the full rationale and
    // `portal_traverse_reaches_neighbor_when_camera_is_close_to_portal_wall`
    // for the regression probe. Only applied to the top-level camera
    // frustum — narrowed sub-frustums produced by `narrow_frustum` already
    // build all edge planes through the camera apex, so the relaxation is
    // redundant (and would be incorrect) inside the DFS.
    let mut visibility_frustum = frustum.clone();
    { let n = &mut visibility_frustum.planes[4]; n.dist = -n.normal.dot(camera_position); }

    let mut path: Vec<usize> = Vec::new();
    let mut clip_scratch_a: Vec<Vec3> = Vec::new();
    let mut clip_scratch_b: Vec<Vec3> = Vec::new();
    flood(
        &mut state,
        camera_cell,
        &visibility_frustum,
        &mut path,
        &mut clip_scratch_a,
        &mut clip_scratch_b,
    );

    // Summary: reach count + the considered/accepted totals, plus a compact
    // rej[...] bracket that elides zero counters. An all-clean frame still
    // prints `rej[]` so the shape of every summary is visually identical.
    if let Some(buf) = state.trace.as_mut() {
        let reach_count = state.visible.iter().filter(|&&v| v).count();
        let _ = write!(
            buf,
            "  = reach={} cons={} acc={} rej[",
            reach_count, state.stats.considered, state.stats.accepted,
        );
        let mut first = true;
        let mut emit = |buf: &mut String, name: &str, count: u32| {
            if count == 0 {
                return;
            }
            if !first {
                buf.push(' ');
            }
            let _ = write!(buf, "{}={}", name, count);
            first = false;
        };
        // Same order as the event-site reason codes: blocked, clip, narrow,
        // solid, cycle, depth, invalid.
        emit(buf, "blocked", state.stats.rejected_blocked_portal);
        emit(buf, "clip", state.stats.rejected_clipped);
        emit(buf, "narrow", state.stats.rejected_narrow);
        emit(buf, "solid", state.stats.rejected_solid);
        emit(buf, "cycle", state.stats.rejected_path_cycle);
        emit(buf, "depth", state.stats.rejected_depth_limit);
        emit(buf, "limit", u32::from(state.stats.step_limit_hit));
        emit(buf, "invalid", state.stats.rejected_invalid);
        let _ = writeln!(buf, "]");
    }

    (
        PortalTraversalResult {
            visible: state.visible,
            stats: state.stats,
        },
        state.trace,
    )
}

// Recursive per-chain DFS. Mirrors id Tech 4's `FloodViewThroughArea_r`
// (Doom 3, `neo/renderer/RenderWorld_portals.cpp`).
fn flood(
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
                let mut len = clipped.len();
                if len >= 3 && sliver_reject(state.camera_position, clipped) {
                    len = 0;
                }
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

/// Clip a convex polygon against every frustum plane (Sutherland-Hodgman).
///
/// Returns a slice into whichever scratch buffer held the final output; the
/// `'a` lifetime ties both scratch buffers to the return value so the borrow
/// checker prevents reuse of either until the slice is dropped. Callers that
/// recurse with the same scratches (e.g. `flood`) must confine this slice to
/// an inner scope.
///
/// Planes use Hessian normal form pointing inward; `CLIP_EPSILON` tilts
/// boundary cases toward "inside" without violating the strict-subset
/// invariant — slop kept here is outside the next narrowing's edge planes.
pub(crate) fn clip_polygon_to_frustum<'a>(
    polygon: &[Vec3],
    frustum: &Frustum,
    scratch_a: &'a mut Vec<Vec3>,
    scratch_b: &'a mut Vec<Vec3>,
) -> &'a [Vec3] {
    scratch_a.clear();
    scratch_b.clear();

    if polygon.len() < 3 {
        return &scratch_a[..];
    }

    scratch_a.extend_from_slice(polygon);

    let mut input_is_a = true;
    for plane in &frustum.planes {
        let (input, output) = if input_is_a {
            (&*scratch_a, &mut *scratch_b)
        } else {
            (&*scratch_b, &mut *scratch_a)
        };
        if input.is_empty() {
            break;
        }
        output.clear();
        clip_polygon_to_plane(input, plane, output);
        input_is_a = !input_is_a;
    }

    if input_is_a {
        &scratch_a[..]
    } else {
        &scratch_b[..]
    }
}

/// Clip a convex polygon against a single half-space (one Sutherland-Hodgman
/// step), using the three-state classifier from Doom 3's `idWinding::Split`
/// (RBDOOM-3-BFG `neo/idlib/geometry/Winding.cpp` L115-200). The same
/// algorithm ships in id's 1999 Quake `qbsp/winding.c` and in ericw-tools'
/// `polylib::winding_base_t::clip` today — a ~30-year-battle-tested lineage.
///
/// Each vertex is classified `FRONT`, `BACK`, or `ON` relative to the plane,
/// using `CLIP_EPSILON` as the on-plane tolerance. Both `FRONT` and `ON`
/// vertices are emitted to the output. The crucial predicate is the split-
/// point skip: when the *next* vertex is `ON` or on the same side as the
/// current one, no intersection vertex is generated — otherwise a vertex
/// within `CLIP_EPSILON` of the plane would get both emitted directly (as an
/// `ON` vertex) *and* have an intersection vertex generated adjacent to it
/// from the bracketing edge, producing the near-duplicate leading pair that
/// makes `narrow_frustum`'s cross-product normal collapse. That is the
/// mechanism behind the `test-2.prl` S-maze missing-panels bug; see the
/// regression probe below.
///
/// Writes the clipped vertices into `output` (which is cleared on entry by
/// the caller). The input polygon must be closed in winding order; vertex
/// order is preserved in the output.
fn clip_polygon_to_plane(input: &[Vec3], plane: &FrustumPlane, output: &mut Vec<Vec3>) {
    let n = input.len();
    if n < 3 {
        return;
    }

    let classify = |d: f32| -> i8 {
        if d > CLIP_EPSILON {
            1 // FRONT — strictly inside the half-space
        } else if d < -CLIP_EPSILON {
            -1 // BACK — strictly outside
        } else {
            0 // ON — within epsilon of the plane
        }
    };

    for i in 0..n {
        let p1 = input[i];
        let d1 = plane.normal.dot(p1) + plane.dist;
        let s1 = classify(d1);

        // Emit `p1` if it is FRONT or ON. ON vertices are emitted to both
        // sides in a full front-and-back split; since we only keep the
        // front side here, ON still belongs in the output.
        if s1 >= 0 {
            output.push(p1);
        }

        // If `p1` is ON, do not generate a split point for the outgoing
        // edge: the ON vertex itself is already the geometric split point,
        // so emitting another one adjacent to it would produce a near-
        // duplicate. The next vertex is handled by its own iteration.
        if s1 == 0 {
            continue;
        }

        let next_idx = (i + 1) % n;
        let p2 = input[next_idx];
        let d2 = plane.normal.dot(p2) + plane.dist;
        let s2 = classify(d2);

        // Skip the split point when:
        //   - `p2` is ON: it will be emitted verbatim in the next
        //     iteration as the geometric split point (Doom 3/Quake rule).
        //   - `p2` is on the same side as `p1`: the edge does not cross
        //     the plane, so there is no split point to generate.
        if s2 == 0 || s2 == s1 {
            continue;
        }

        output.push(compute_split_point_on_plane(p1, p2, d1, d2, plane));
    }
}

/// Compute the split point where a line segment crosses a plane, with two
/// numerical-robustness tweaks borrowed from Doom 3's `idWinding::Split`
/// (RBDOOM-3-BFG `neo/idlib/geometry/Winding.cpp` L205-224):
///
/// 1. **Direction-symmetric lerp.** Always interpolate from the FRONT
///    vertex toward the BACK vertex. This guarantees that processing edge
///    `A→B` and edge `B→A` yields bitwise-identical split points, which
///    matters when the same edge is walked from opposite directions by
///    adjacent clip steps.
/// 2. **Axis-aligned plane snap.** If the clip plane's normal is exactly a
///    unit axis (e.g. `(±1, 0, 0)`), force the split point's coordinate on
///    that axis to lie exactly on the plane instead of accepting lerp
///    drift. Frees a split-point vertex from later misclassification by
///    adjacent planes.
///
/// Caller guarantees `d1` and `d2` have opposite signs and neither is ON,
/// so the denominator is non-zero.
fn compute_split_point_on_plane(
    p1: Vec3,
    p2: Vec3,
    d1: f32,
    d2: f32,
    plane: &FrustumPlane,
) -> Vec3 {
    debug_assert!(
        d1.abs() > CLIP_EPSILON && d2.abs() > CLIP_EPSILON && d1.signum() != d2.signum(),
        "compute_split_point_on_plane requires d1/d2 to be strictly \
         opposite-sign and neither within CLIP_EPSILON — the SIDE_ON \
         filter in clip_polygon_to_plane must guarantee this"
    );

    let (front, back, d_front, d_back) = if d1 >= 0.0 {
        (p1, p2, d1, d2)
    } else {
        (p2, p1, d2, d1)
    };
    let t = d_front / (d_front - d_back);
    let mut mid = front + (back - front) * t;

    // Axis-aligned snap. Our Hessian convention is `n·v + d = 0`, so for
    // `n = +unit_j` the plane is `v[j] = -plane.dist`, and for
    // `n = -unit_j` it is `v[j] = plane.dist`.
    for j in 0..3 {
        let n_j = plane.normal[j];
        if n_j == 1.0 {
            mid[j] = -plane.dist;
        } else if n_j == -1.0 {
            mid[j] = plane.dist;
        }
    }

    mid
}

/// Tolerance for "camera lies on the portal's supporting plane".
///
/// Signed distance from the apex to the polygon's plane, measured in world
/// units. Looser than `CLIP_EPSILON` because the camera only needs to be
/// *near enough* that the view-frustum cross-section at the portal collapses
/// to something Sutherland-Hodgman can't keep convex — a sub-millimeter
/// margin around the plane is plenty to catch the "player on the portal
/// boundary" case without triggering on genuinely-frontal portals.
const APEX_ON_PORTAL_PLANE_EPSILON: f32 = 1e-3;

// Newell's method for the normal: robust against near-colinear leading
// vertices that collapse a simple (v1-v0)×(v2-v0) cross product.
pub(crate) fn camera_on_polygon_plane(apex: Vec3, polygon: &[Vec3]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let n = polygon.len();
    let centroid = polygon.iter().copied().sum::<Vec3>() / n as f32;
    let mut normal = Vec3::ZERO;
    for i in 0..n {
        let cur = polygon[i];
        let nxt = polygon[(i + 1) % n];
        normal.x += (cur.y - nxt.y) * (cur.z + nxt.z);
        normal.y += (cur.z - nxt.z) * (cur.x + nxt.x);
        normal.z += (cur.x - nxt.x) * (cur.y + nxt.y);
    }
    if normal.length_squared() < 1e-12 {
        return false;
    }
    let normal = normal.normalize();
    normal.dot(apex - centroid).abs() < APEX_ON_PORTAL_PLANE_EPSILON
}

/// Returns None if the portal is degenerate or the normal collapses.
pub fn narrow_frustum(
    camera_position: Vec3,
    portal_polygon: &[Vec3],
    original_frustum: &Frustum,
) -> Option<Frustum> {
    if portal_polygon.len() < 3 {
        return None;
    }

    let n = portal_polygon.len();
    let centroid = portal_polygon.iter().copied().sum::<Vec3>() / n as f32;

    // Newell's method: robust against colinear/near-duplicate vertices that would collapse a single (v1-v0)×(v2-v0) cross product.
    let mut portal_normal = Vec3::ZERO;
    for i in 0..n {
        let cur = portal_polygon[i];
        let nxt = portal_polygon[(i + 1) % n];
        portal_normal.x += (cur.y - nxt.y) * (cur.z + nxt.z);
        portal_normal.y += (cur.z - nxt.z) * (cur.x + nxt.x);
        portal_normal.z += (cur.x - nxt.x) * (cur.y + nxt.y);
    }
    if portal_normal.length_squared() < 1e-12 {
        return None;
    }
    let portal_normal = portal_normal.normalize();

    // Orient normal away from the camera so the near plane clips the camera-side.
    let camera_side = portal_normal.dot(camera_position - centroid);
    let oriented_normal = if camera_side > 0.0 {
        -portal_normal
    } else {
        portal_normal
    };
    let portal_dist = -oriented_normal.dot(centroid);

    let mut planes = Vec::with_capacity(n + 2);

    // Portal plane as near clip.
    planes.push(FrustumPlane {
        normal: oriented_normal,
        dist: portal_dist,
    });

    // Edge planes: for each portal edge, the clip plane passes through the
    // camera and the edge, oriented to face the portal centroid. This is the
    // exact visibility cone from a point camera through the portal.
    for i in 0..n {
        let edge_a = portal_polygon[i];
        let edge_b = portal_polygon[(i + 1) % n];
        let edge_dir = edge_b - edge_a;
        let to_camera = camera_position - edge_a;

        let mut edge_normal = edge_dir.cross(to_camera);
        if edge_normal.length_squared() < 1e-12 {
            continue;
        }
        edge_normal = edge_normal.normalize();
        if edge_normal.dot(centroid - edge_a) < 0.0 {
            edge_normal = -edge_normal;
        }
        let dist = -edge_normal.dot(edge_a);

        planes.push(FrustumPlane {
            normal: edge_normal,
            dist,
        });
    }

    // Keep the far plane from the original frustum (always the last plane).
    if let Some(&far_plane) = original_frustum.planes.last() {
        planes.push(far_plane);
    }

    Some(Frustum { planes })
}


static SLIVER: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
fn sliver_reject(cam: Vec3, poly: &[Vec3]) -> bool {
    let t = *SLIVER.get_or_init(|| std::env::var("SLIVER_SR").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0));
    if t <= 0.0 { return false; }
    // approximate solid angle: area / dist^2
    let c = poly.iter().copied().sum::<Vec3>() / poly.len() as f32;
    let mut a = Vec3::ZERO;
    for i in 0..poly.len() { a += poly[i].cross(poly[(i + 1) % poly.len()]); }
    let d2 = (c - cam).length_squared().max(1e-6);
    let dir = (c - cam) / d2.sqrt();
    let proj_area = 0.5 * a.dot(dir).abs();
    proj_area / d2 < t
}
