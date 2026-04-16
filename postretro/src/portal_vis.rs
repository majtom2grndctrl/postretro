// Runtime portal traversal: per-chain DFS with polygon-vs-frustum clipping + narrowing.
// See: context/lib/build_pipeline.md §Runtime visibility

use std::fmt::Write as _;

use glam::Vec3;

use crate::prl::LevelWorld;
use crate::visibility::{Frustum, FrustumPlane};

/// Half-space boundary epsilon for Sutherland-Hodgman.
///
/// Points within `CLIP_EPSILON` of a plane are treated as on the plane (kept as
/// inside). Over-inclusion at the boundary cannot violate the strict-subset
/// invariant — the next narrowing iteration will exclude any genuinely-outside
/// slop introduced here.
const CLIP_EPSILON: f32 = 1e-4;

/// Maximum portal-chain depth allowed during per-chain DFS traversal.
///
/// Real-map chains typically run 5–10 deep, occasionally ~20. 256 is well
/// above any realistic chain depth and well below stack-overflow territory.
/// When a chain hits this limit the helper logs a warning (under the
/// `postretro::portal_trace` target) and returns without recursing further —
/// the visible set becomes conservative at that branch but no crash occurs.
/// Tune upward only if a real map trips the guard.
const MAX_PORTAL_CHAIN_DEPTH: usize = 256;

/// Per-recursion state shared by every branch of the DFS.
///
/// Kept in a single struct so the recursive helper has one `&mut` argument
/// beyond the changing per-frame values (current leaf, current frustum, current
/// path). Counters accumulate across all chains; the visible bitset is the
/// union of every chain's reach.
///
/// `trace` holds the optional per-frame capture buffer. When the caller passes
/// `capture: true` it starts as `Some(String)` and event sites append terse
/// event lines into it; a single `log::info!` at the end of `portal_traverse`
/// emits the buffer as one multi-line message. `None` means capture is off,
/// and the event sites skip the writes entirely — no allocation on the hot
/// path when diagnostics are disabled.
struct DfsState<'a> {
    world: &'a LevelWorld,
    camera_position: Vec3,
    trace: Option<String>,
    visible: Vec<bool>,
    leaf_count: usize,
    considered: u32,
    accepted: u32,
    rejected_solid: u32,
    rejected_clipped: u32,
    rejected_narrow: u32,
    rejected_invalid: u32,
    rejected_path_cycle: u32,
    rejected_depth_limit: u32,
    depth_limit_warned: bool,
    camera_leaf: usize,
}

/// Perform per-chain depth-first portal traversal to determine which leaves
/// are visible from the camera's current leaf.
///
/// For each portal reached along a chain, the portal polygon is clipped
/// against every plane of the current frustum (Sutherland-Hodgman). An empty
/// clip output is the unified rejection signal — the portal is not visible
/// through the current sight cone. The clipped polygon then feeds frustum
/// narrowing, which builds a new cone strictly inside the current one.
///
/// Cycle prevention keys on portals crossed in the current chain, not on
/// leaves reached globally. Keying on leaves would drop any chain after the
/// first to arrive at a leaf — silently losing whichever carried the widest
/// sub-frustum. The visible bitset is the union across all chains.
///
/// By induction from the camera's initial frustum, every narrowed frustum
/// reachable through any portal chain is a strict subset of the camera
/// frustum. A per-leaf AABB cull is therefore unnecessary on this path.
///
/// **Recursion depth.** Bounded by `MAX_PORTAL_CHAIN_DEPTH` (currently 256).
/// Real maps run 5–10 deep. Beyond the bound, the current leaf is still
/// marked visible (conservative) and a warning is emitted once per top-level
/// walk. See [`MAX_PORTAL_CHAIN_DEPTH`].
///
/// **Per-path allocation.** The chain path is a single `Vec<usize>` of portal
/// indices, allocated once at the top of the walk and reused via push/pop
/// backtracking as the DFS descends and unwinds. The cycle-detection membership
/// test remains a linear scan — correct and fast at realistic chain depths
/// (5–20). Two `Vec<Vec3>` clip scratch buffers are allocated alongside the
/// path and threaded through `flood` into every `clip_polygon_to_frustum`
/// call, so the per-frame hot path performs no allocations.
///
/// When `capture` is true the walk emits one log line per portal touched
/// (accept/reject + reason) plus a per-frame summary, all under the
/// `postretro::portal_trace` target. Triggered by the `Alt+Shift+1`
/// diagnostic chord; see `context/lib/input.md` §7.
pub fn portal_traverse(
    camera_position: Vec3,
    camera_leaf: usize,
    frustum: &Frustum,
    world: &LevelWorld,
    capture: bool,
) -> Vec<bool> {
    let (visible, trace) =
        portal_traverse_inner(camera_position, camera_leaf, frustum, world, capture);
    // Single log emission: every event written during the walk lives inside
    // `trace`. One `log::info!` call means one timestamp/target prefix per
    // traced frame instead of one per event. See the file's doc comment on
    // `portal_traverse` for the target and the chord that arms capture.
    if let Some(buf) = trace {
        log::info!(target: "postretro::portal_trace", "[portal_trace]\n{}", buf);
    }
    visible
}

/// Core walk shared by `portal_traverse` and the `#[cfg(test)]` helper that
/// needs to inspect the formatted trace string directly. Returns the visible
/// bitset and, when capture is on, the fully-formatted trace buffer (header +
/// event lines + summary). The public entry point logs the buffer; tests read
/// it as a string so they can assert format shape without wiring a test
/// logger.
fn portal_traverse_inner(
    camera_position: Vec3,
    camera_leaf: usize,
    frustum: &Frustum,
    world: &LevelWorld,
    capture: bool,
) -> (Vec<bool>, Option<String>) {
    let leaf_count = world.leaves.len();
    let visible = vec![false; leaf_count];

    // Allocate the trace buffer only when capture is armed — the disabled path
    // must remain allocation-free.
    let mut trace = if capture {
        Some(String::with_capacity(512))
    } else {
        None
    };

    // Out-of-range camera leaf: emit a single `leaf_oor` line into the buffer
    // and bail. `world.leaves[camera_leaf]` would panic, so this path must run
    // before any header write that reads the leaf.
    if camera_leaf >= leaf_count {
        if let Some(buf) = trace.as_mut() {
            let _ = writeln!(
                buf,
                "abort leaf_oor cam=({:.2},{:.2},{:.2}) leaf={} leaves={}",
                camera_position.x, camera_position.y, camera_position.z, camera_leaf, leaf_count,
            );
        }
        return (visible, trace);
    }

    // Header line: the camera-leaf diagnostic fields we need for the current
    // flicker bug hunt. `solid` is intentionally omitted — solid leaves
    // short-circuit in `determine_visible_cells` before they ever reach
    // `portal_traverse`, so any leaf here is already known non-solid.
    if let Some(buf) = trace.as_mut() {
        let leaf = &world.leaves[camera_leaf];
        let _ = writeln!(
            buf,
            "cam=({:.2},{:.2},{:.2}) leaf={} faces={} bnds=({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2}) leaves={}",
            camera_position.x,
            camera_position.y,
            camera_position.z,
            camera_leaf,
            leaf.face_count,
            leaf.bounds_min.x,
            leaf.bounds_min.y,
            leaf.bounds_min.z,
            leaf.bounds_max.x,
            leaf.bounds_max.y,
            leaf.bounds_max.z,
            leaf_count,
        );
    }

    let mut state = DfsState {
        world,
        camera_position,
        trace,
        visible,
        leaf_count,
        considered: 0,
        accepted: 0,
        rejected_solid: 0,
        rejected_clipped: 0,
        rejected_narrow: 0,
        rejected_invalid: 0,
        rejected_path_cycle: 0,
        rejected_depth_limit: 0,
        depth_limit_warned: false,
        camera_leaf,
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
    visibility_frustum.slide_near_plane_to(camera_position);

    // Allocate the path and clip scratch buffers once and reuse them for the
    // entire traversal. `flood` pushes/pops the path across recursive calls
    // and passes the scratch buffers into each `clip_polygon_to_frustum`
    // call, so per-frame portal walks allocate nothing on the hot path.
    let mut path: Vec<usize> = Vec::new();
    let mut clip_scratch_a: Vec<Vec3> = Vec::new();
    let mut clip_scratch_b: Vec<Vec3> = Vec::new();
    flood(
        &mut state,
        camera_leaf,
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
            reach_count, state.considered, state.accepted,
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
        // Same order as the event-site reason codes: clip, narrow, solid,
        // cycle, depth, invalid.
        emit(buf, "clip", state.rejected_clipped);
        emit(buf, "narrow", state.rejected_narrow);
        emit(buf, "solid", state.rejected_solid);
        emit(buf, "cycle", state.rejected_path_cycle);
        emit(buf, "depth", state.rejected_depth_limit);
        emit(buf, "invalid", state.rejected_invalid);
        let _ = writeln!(buf, "]");
    }

    (state.visible, state.trace)
}

/// Recursive per-chain flood-fill.
///
/// Marks the current leaf visible, then tries every outbound portal. A portal
/// already on the current chain's path is skipped (would loop). Each surviving
/// portal produces a narrowed sub-frustum and a recursive descent. `path`,
/// `clip_scratch_a`, and `clip_scratch_b` are owned by `portal_traverse` and
/// reused across every recursive call — the path tracks the current chain via
/// push/pop backtracking, and the scratch Vecs are cleared and refilled inside
/// `clip_polygon_to_frustum` on each call.
///
/// The algorithmic shape mirrors id Tech 4's `FloodViewThroughArea_r`
/// (Doom 3, `neo/renderer/RenderWorld_portals.cpp`).
fn flood(
    state: &mut DfsState,
    leaf: usize,
    frustum: &Frustum,
    path: &mut Vec<usize>,
    clip_scratch_a: &mut Vec<Vec3>,
    clip_scratch_b: &mut Vec<Vec3>,
) {
    // Mark the current leaf visible on entry. Every chain that reaches a leaf
    // contributes to the visible union regardless of which portals it crossed.
    state.visible[leaf] = true;

    if path.len() >= MAX_PORTAL_CHAIN_DEPTH {
        state.rejected_depth_limit += 1;
        if !state.depth_limit_warned {
            state.depth_limit_warned = true;
            // Real warning, independent of capture: visible-set conservatism
            // past this point is a correctness signal worth seeing even when
            // the diagnostic chord is off. Stays a separate emission.
            log::warn!(
                target: "postretro::portal_trace",
                "[portal_trace] chain depth limit reached (MAX_PORTAL_CHAIN_DEPTH={}) \
                 camera_leaf={} truncated_at_leaf={} — visible set conservative \
                 past this point",
                MAX_PORTAL_CHAIN_DEPTH,
                state.camera_leaf,
                leaf,
            );
        }
        // Additionally append one compact event line to the trace buffer so
        // the depth-limit hit appears inline with the rest of the frame's
        // events (the `log::warn!` above fires once per walk; this fires
        // every time the limit is hit during capture).
        if let Some(buf) = state.trace.as_mut() {
            let _ = writeln!(buf, "  rej leaf={} depth", leaf);
        }
        return;
    }

    let outbound_len = state.world.leaf_portals[leaf].len();

    // Iterate by index into `world.leaf_portals[leaf]`. `leaf` is the caller's
    // already-validated leaf index, so direct indexing is safe. Re-borrowing
    // `state.world` per step avoids holding a long-lived borrow across the
    // recursive call, which would conflict with `state` being `&mut`.
    for i in 0..outbound_len {
        let portal_idx = state.world.leaf_portals[leaf][i];
        let portal = &state.world.portals[portal_idx];

        // Determine the neighbor leaf (the portal's other side).
        let neighbor = if portal.front_leaf == leaf {
            portal.back_leaf
        } else {
            portal.front_leaf
        };

        state.considered += 1;

        if neighbor >= state.leaf_count {
            state.rejected_invalid += 1;
            continue;
        }

        // Per-chain cycle check: if this portal is already on the current
        // chain's path, taking it would form a loop. Linear scan over a small
        // Vec beats HashSet hashing for typical chain depths (5–10).
        if path.contains(&portal_idx) {
            state.rejected_path_cycle += 1;
            continue;
        }

        // Skip solid leaves.
        if state.world.leaves[neighbor].is_solid {
            state.rejected_solid += 1;
            // For `solid` rejects the clip hasn't run yet, so the "clipped
            // verts" half of the v=c/p pair isn't meaningful. Print only the
            // portal vertex count.
            if let Some(buf) = state.trace.as_mut() {
                let _ = writeln!(
                    buf,
                    "  rej {}->{} v={} solid",
                    leaf,
                    neighbor,
                    portal.polygon.len(),
                );
            }
            continue;
        }

        // Clip the portal polygon against the current frustum. An empty
        // result unifies "portal entirely outside cone" and "portal
        // degenerate after clipping" into one rejection path.
        //
        // `narrowed` is computed inside an inner scope so that the borrows of
        // `clip_scratch_a`/`clip_scratch_b` end before the recursive call
        // below, which needs mutable access to both scratch buffers again.
        // `clipped_len` is captured ahead of the scope so trace logging and
        // path updates downstream can still read it.
        let (narrowed_opt, clipped_len) = {
            let clipped =
                clip_polygon_to_frustum(&portal.polygon, frustum, clip_scratch_a, clip_scratch_b);
            let len = clipped.len();
            if len < 3 {
                (None, len)
            } else {
                let narrowed = narrow_frustum(state.camera_position, clipped, frustum);
                (narrowed, len)
            }
        };

        if clipped_len < 3 {
            state.rejected_clipped += 1;
            if let Some(buf) = state.trace.as_mut() {
                let _ = writeln!(
                    buf,
                    "  rej {}->{} v={}/{} clip",
                    leaf,
                    neighbor,
                    clipped_len,
                    portal.polygon.len(),
                );
            }
            continue;
        }

        // Narrow the frustum through the clipped polygon. The clipped
        // polygon lies entirely inside the current frustum, so the edge
        // planes it produces form a cone strictly inside the current one.
        let Some(narrowed) = narrowed_opt else {
            state.rejected_narrow += 1;
            if let Some(buf) = state.trace.as_mut() {
                let _ = writeln!(
                    buf,
                    "  rej {}->{} v={}/{} narrow",
                    leaf,
                    neighbor,
                    clipped_len,
                    portal.polygon.len(),
                );
            }
            continue;
        };

        state.accepted += 1;
        if let Some(buf) = state.trace.as_mut() {
            let _ = writeln!(buf, "  acc {}->{} v={}", leaf, neighbor, clipped_len);
        }

        // Descend into the neighbor, reusing the shared path via push/pop
        // backtracking. After the recursive call returns, `path` is restored
        // to its pre-push state so sibling branches at this depth see an
        // unchanged chain history.
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

/// Clip a convex polygon against every plane of a frustum (Sutherland-Hodgman).
///
/// Writes the clipped polygon into one of the caller-provided scratch buffers
/// and returns a slice borrowing from it. A result with fewer than 3 vertices
/// means the polygon is entirely outside the frustum (or clipped down to a
/// degenerate edge/point at a boundary).
///
/// `scratch_a` and `scratch_b` are ping-pong buffers: each Sutherland-Hodgman
/// step reads from one and writes to the other, swapping roles between planes.
/// Both buffers are cleared on entry and their pre-call contents are not
/// preserved. The returned slice borrows from whichever buffer most-recently
/// acted as the output pass; both buffers share the same `'a` lifetime in the
/// signature, so the borrow checker will prevent the caller from reusing
/// *either* scratch buffer until the returned slice is dropped. Callers that
/// need to recurse with the same scratches (e.g., `flood`) must confine the
/// returned slice to an inner scope so the borrows end before the recursive
/// call re-takes `&mut` of the scratches.
///
/// Each frustum plane is in Hessian normal form pointing inward: a vertex `v`
/// is inside when `plane.normal · v + plane.dist >= -CLIP_EPSILON`. The
/// epsilon tilts boundary cases toward "inside". This cannot violate the
/// strict-subset invariant — any slop kept at one hop becomes outside the
/// next narrowing's edge planes and is discarded there.
///
/// Because every clipped vertex is either an original polygon vertex or an
/// intersection of a polygon edge with a frustum plane (both on the polygon
/// plane), the clipped polygon remains planar. This is required for
/// `narrow_frustum` to produce meaningful edge planes.
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

    // Ping-pong between `scratch_a` (current input) and `scratch_b` (current
    // output). After each plane, swap roles by flipping `input_is_a`. The
    // final result lives in whichever buffer most-recently acted as output.
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

    // Direction-symmetric: always lerp FRONT → BACK.
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

/// Narrow a frustum by constructing clip planes through the camera and the
/// portal polygon edges.
///
/// For a portal polygon with N vertices, constructs N edge planes (each through
/// the camera position and one edge of the portal) plus the portal's own plane
/// as a near clip. The far plane is retained from the original frustum.
///
/// Returns None if the portal is behind the camera or degenerate.
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

    // Orient the portal normal to face away from the camera.
    // The near plane should clip away the side of the portal the camera is on.
    let camera_side = portal_normal.dot(camera_position - centroid);
    let oriented_normal = if camera_side > 0.0 {
        -portal_normal
    } else {
        portal_normal
    };
    let portal_dist = -oriented_normal.dot(centroid);

    let mut planes = Vec::with_capacity(n + 2);

    // Portal plane as near clip.
    planes.push(crate::visibility::FrustumPlane {
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

        planes.push(crate::visibility::FrustumPlane {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prl::{BspChild, LeafData, LevelWorld, NodeData, PortalData};
    use crate::visibility::{FrustumPlane, is_aabb_outside_frustum};
    use glam::Mat4;

    /// Extract frustum from a view-projection matrix (reuse from visibility module).
    fn extract_test_frustum(view_proj: Mat4) -> Frustum {
        use glam::Vec4;

        let row = |n: usize| -> Vec4 {
            Vec4::new(
                view_proj.col(0)[n],
                view_proj.col(1)[n],
                view_proj.col(2)[n],
                view_proj.col(3)[n],
            )
        };

        let r0 = row(0);
        let r1 = row(1);
        let r2 = row(2);
        let r3 = row(3);

        let raw_planes = [
            r3 + r0, // Left
            r3 - r0, // Right
            r3 + r1, // Bottom
            r3 - r1, // Top
            r3 + r2, // Near
            r3 - r2, // Far
        ];

        let mut planes = Vec::with_capacity(6);

        for raw in &raw_planes {
            let normal = Vec3::new(raw.x, raw.y, raw.z);
            let length = normal.length();
            if length > 0.0 {
                let inv_len = 1.0 / length;
                planes.push(FrustumPlane {
                    normal: normal * inv_len,
                    dist: raw.w * inv_len,
                });
            } else {
                planes.push(FrustumPlane {
                    normal: Vec3::ZERO,
                    dist: 0.0,
                });
            }
        }

        Frustum { planes }
    }

    fn make_camera_frustum(position: Vec3, look_dir: Vec3) -> Frustum {
        let target = position + look_dir;
        let view = Mat4::look_at_rh(position, target, Vec3::Y);
        let aspect = 16.0 / 9.0;
        let hfov = 100.0_f32.to_radians();
        let vfov = 2.0 * ((hfov / 2.0).tan() / aspect).atan();
        let proj = Mat4::perspective_rh(vfov, aspect, 0.1, 4096.0);
        extract_test_frustum(proj * view)
    }

    /// Build a three-leaf chain: A (leaf 0) -- portal 0 -- B (leaf 1) -- portal 1 -- C (leaf 2)
    /// arranged along the X axis.
    fn three_leaf_chain() -> LevelWorld {
        let portal_0 = PortalData {
            polygon: vec![
                Vec3::new(32.0, 0.0, 0.0),
                Vec3::new(32.0, 64.0, 0.0),
                Vec3::new(32.0, 64.0, 64.0),
                Vec3::new(32.0, 0.0, 64.0),
            ],
            front_leaf: 0,
            back_leaf: 1,
        };
        let portal_1 = PortalData {
            polygon: vec![
                Vec3::new(64.0, 0.0, 0.0),
                Vec3::new(64.0, 64.0, 0.0),
                Vec3::new(64.0, 64.0, 64.0),
                Vec3::new(64.0, 0.0, 64.0),
            ],
            front_leaf: 1,
            back_leaf: 2,
        };

        LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![
                NodeData {
                    plane_normal: Vec3::X,
                    plane_distance: 32.0,
                    front: BspChild::Node(1),
                    back: BspChild::Leaf(0),
                },
                NodeData {
                    plane_normal: Vec3::X,
                    plane_distance: 64.0,
                    front: BspChild::Leaf(2),
                    back: BspChild::Leaf(1),
                },
            ],
            leaves: vec![
                LeafData {
                    bounds_min: Vec3::new(0.0, 0.0, 0.0),
                    bounds_max: Vec3::new(32.0, 64.0, 64.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
                LeafData {
                    bounds_min: Vec3::new(32.0, 0.0, 0.0),
                    bounds_max: Vec3::new(64.0, 64.0, 64.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
                LeafData {
                    bounds_min: Vec3::new(64.0, 0.0, 0.0),
                    bounds_max: Vec3::new(96.0, 64.0, 64.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
            ],
            root: BspChild::Node(0),
            has_pvs: false,
            portals: vec![portal_0, portal_1],
            leaf_portals: vec![
                vec![0],    // leaf 0 touches portal 0
                vec![0, 1], // leaf 1 touches portals 0 and 1
                vec![1],    // leaf 2 touches portal 1
            ],
            has_portals: true,
            texture_names: vec![],
            bvh: crate::geometry::BvhTree {
                nodes: vec![],
                leaves: vec![],
                root_node_index: 0,
            },
            lights: vec![],
            light_influences: vec![],
        }
    }

    #[test]
    fn portal_traverse_camera_leaf_always_visible() {
        let world = three_leaf_chain();
        // Camera in leaf 0, looking away from all portals.
        let frustum = make_camera_frustum(Vec3::new(16.0, 32.0, 32.0), Vec3::NEG_X);
        let visible = portal_traverse(Vec3::new(16.0, 32.0, 32.0), 0, &frustum, &world, false);
        assert!(visible[0], "camera leaf should always be visible");
    }

    #[test]
    fn portal_traverse_straight_corridor_sees_all_three() {
        let world = three_leaf_chain();
        // Camera in leaf 0, looking through portals toward +X.
        let camera_pos = Vec3::new(16.0, 32.0, 32.0);
        let frustum = make_camera_frustum(camera_pos, Vec3::X);
        let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
        assert!(visible[0], "camera leaf A should be visible");
        assert!(visible[1], "leaf B should be visible through portal 0");
        assert!(visible[2], "leaf C should be visible through portals 0+1");
    }

    #[test]
    fn portal_traverse_looking_away_hides_distant_leaves() {
        let world = three_leaf_chain();
        // Camera in leaf 0, looking away from the portals (toward -X).
        let camera_pos = Vec3::new(16.0, 32.0, 32.0);
        let frustum = make_camera_frustum(camera_pos, Vec3::NEG_X);
        let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
        assert!(visible[0], "camera leaf should be visible");
        // Portals are at X=32 and X=64, camera looks toward -X, so they're behind.
        assert!(
            !visible[1],
            "leaf B should not be visible when looking away"
        );
        assert!(
            !visible[2],
            "leaf C should not be visible when looking away"
        );
    }

    #[test]
    fn portal_traverse_skips_solid_neighbors() {
        let mut world = three_leaf_chain();
        world.leaves[1].is_solid = true;

        let camera_pos = Vec3::new(16.0, 32.0, 32.0);
        let frustum = make_camera_frustum(camera_pos, Vec3::X);
        let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
        assert!(visible[0], "camera leaf should be visible");
        assert!(!visible[1], "solid leaf should not be visible");
        // Leaf 2 is behind solid leaf 1, so it can't be reached.
        assert!(!visible[2], "leaf behind solid should not be visible");
    }

    #[test]
    fn portal_traverse_empty_world() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![],
            leaves: vec![],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![],
            leaf_portals: vec![],
            has_portals: false,
            texture_names: vec![],
            bvh: crate::geometry::BvhTree {
                nodes: vec![],
                leaves: vec![],
                root_node_index: 0,
            },
            lights: vec![],
            light_influences: vec![],
        };

        let frustum = make_camera_frustum(Vec3::ZERO, Vec3::NEG_Z);
        let visible = portal_traverse(Vec3::ZERO, 0, &frustum, &world, false);
        assert!(visible.is_empty());
    }

    #[test]
    fn portal_traverse_l_shaped_corridor_hides_c() {
        // L-shaped corridor: A -- portal 0 (at X=32 in YZ plane) -- B -- portal 1 (at Z=64 in XY plane) -- C
        // Camera in A looking along +X sees B through portal 0,
        // but portal 1 is perpendicular (in the Z direction), so C is not visible
        // through the narrow frustum left after passing through portal 0.
        let portal_0 = PortalData {
            polygon: vec![
                Vec3::new(32.0, 0.0, 0.0),
                Vec3::new(32.0, 64.0, 0.0),
                Vec3::new(32.0, 64.0, 64.0),
                Vec3::new(32.0, 0.0, 64.0),
            ],
            front_leaf: 0,
            back_leaf: 1,
        };
        // Portal 1 is on the Z=64 plane — perpendicular to the camera's line of sight.
        // Positioned far to the +Z side of the corridor.
        let portal_1 = PortalData {
            polygon: vec![
                Vec3::new(32.0, 0.0, 200.0),
                Vec3::new(64.0, 0.0, 200.0),
                Vec3::new(64.0, 64.0, 200.0),
                Vec3::new(32.0, 64.0, 200.0),
            ],
            front_leaf: 1,
            back_leaf: 2,
        };

        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![],
            leaves: vec![
                LeafData {
                    bounds_min: Vec3::new(0.0, 0.0, 0.0),
                    bounds_max: Vec3::new(32.0, 64.0, 64.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
                LeafData {
                    bounds_min: Vec3::new(32.0, 0.0, 0.0),
                    bounds_max: Vec3::new(64.0, 64.0, 200.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
                LeafData {
                    bounds_min: Vec3::new(32.0, 0.0, 200.0),
                    bounds_max: Vec3::new(64.0, 64.0, 264.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
            ],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![portal_0, portal_1],
            leaf_portals: vec![vec![0], vec![0, 1], vec![1]],
            has_portals: true,
            texture_names: vec![],
            bvh: crate::geometry::BvhTree {
                nodes: vec![],
                leaves: vec![],
                root_node_index: 0,
            },
            lights: vec![],
            light_influences: vec![],
        };

        // Camera in leaf A, looking straight along +X toward portal 0.
        let camera_pos = Vec3::new(16.0, 32.0, 32.0);
        let frustum = make_camera_frustum(camera_pos, Vec3::X);
        let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
        assert!(visible[0], "camera leaf A should be visible");
        assert!(visible[1], "leaf B should be visible through portal 0");
        assert!(
            !visible[2],
            "leaf C should not be visible — portal 1 is around the corner at Z=200"
        );
    }

    #[test]
    fn narrow_frustum_produces_tighter_frustum() {
        // Camera at origin looking along +X.
        let camera_pos = Vec3::ZERO;
        let frustum = make_camera_frustum(camera_pos, Vec3::X);

        // A small portal at X=10 centered at Y=5,Z=5, 2x2 units.
        let portal = vec![
            Vec3::new(10.0, 4.0, 4.0),
            Vec3::new(10.0, 6.0, 4.0),
            Vec3::new(10.0, 6.0, 6.0),
            Vec3::new(10.0, 4.0, 6.0),
        ];

        let narrowed = narrow_frustum(camera_pos, &portal, &frustum);
        assert!(
            narrowed.is_some(),
            "narrow_frustum should succeed for a visible portal"
        );

        let narrowed = narrowed.unwrap();

        // The narrowed frustum should be tighter: a point far from the portal
        // line of sight should be outside the narrowed frustum but might be
        // inside the original.
        let far_off_point_mins = Vec3::new(20.0, 50.0, 50.0);
        let far_off_point_maxs = Vec3::new(21.0, 51.0, 51.0);

        let narrowed_rejects =
            is_aabb_outside_frustum(far_off_point_mins, far_off_point_maxs, &narrowed);
        assert!(
            narrowed_rejects,
            "narrowed frustum should reject a point far off the portal's line of sight"
        );
    }

    #[test]
    fn narrow_frustum_rejects_degenerate_portal() {
        let camera_pos = Vec3::ZERO;
        let frustum = make_camera_frustum(camera_pos, Vec3::X);

        // Degenerate: less than 3 vertices.
        assert!(narrow_frustum(camera_pos, &[Vec3::X, Vec3::Y], &frustum).is_none());
        assert!(narrow_frustum(camera_pos, &[], &frustum).is_none());
    }

    /// Regression gate for the `test-2.prl` S-maze missing-panels bug.
    ///
    /// The old two-state Sutherland-Hodgman clipper produced clipped
    /// polygons whose first two vertices were near-duplicates whenever a
    /// polygon vertex lay within `CLIP_EPSILON` of a clip plane: for a
    /// quad `[A, B, C, D]` with `A` inside-by-epsilon and `D` outside, the
    /// clipper emitted `intersect(D,A), A, B, C, intersect(C,D)` — and
    /// `intersect(D,A)` sat within epsilon of `A` itself. The first two
    /// output vertices were then effectively coincident, and
    /// `narrow_frustum`'s leading-triple cross-product normal collapsed
    /// below its `1e-12` early-out, silently dropping the portal as
    /// `rej A->B v=5/4 narrow`.
    ///
    /// In the broken `test-2.prl` trace, this was the failure along chain
    /// `41 → 43 → 38 → 37 → 31 → 30`: `rej 43->38 v=5/4 narrow` broke the
    /// only chain that reached leaf 30, the leaf holding the missing wall
    /// and ceiling panels.
    ///
    /// The fix is the three-state `FRONT`/`BACK`/`ON` classifier from
    /// Doom 3's `idWinding::Split` (same lineage as Quake's 1999
    /// `qbsp/winding.c` and ericw-tools' `polylib::winding_base_t::clip`):
    /// `ON` vertices are emitted verbatim, and the "skip split point if
    /// next is ON or same-side" predicate prevents emitting a lerped
    /// intersection adjacent to an already-emitted on-plane vertex.
    ///
    /// This probe runs the production clipper on exactly the quad shape
    /// that used to trigger the bug (one vertex 5e-8 units inside a clip
    /// plane — cumulative Sutherland-Hodgman imprecision after a handful
    /// of portal-chain hops routinely lands vertices this close to clip
    /// boundaries). The clipped polygon now has no degenerate leading
    /// vertices and `narrow_frustum` accepts it.
    #[test]
    fn narrow_frustum_accepts_sutherland_hodgman_near_duplicate_leading_vertices() {
        // Quad that yields the pathological clip: A inside-by-epsilon, D
        // outside. Epsilon of 5e-8 puts cross^2 firmly below the 1e-12
        // narrow-rejection threshold after clipping (verified by sweep).
        let polygon = vec![
            Vec3::new(10.0, 5e-8, 0.0),
            Vec3::new(10.0, 5.0, 0.0),
            Vec3::new(10.0, 5.0, 5.0),
            Vec3::new(10.0, -5.0, 5.0),
        ];
        let clip_plane = crate::visibility::FrustumPlane {
            normal: Vec3::new(0.0, 1.0, 0.0),
            dist: 0.0,
        };
        let front_frustum = Frustum {
            planes: vec![clip_plane],
        };

        let mut scratch_a: Vec<Vec3> = Vec::new();
        let mut scratch_b: Vec<Vec3> = Vec::new();
        let clipped =
            clip_polygon_to_frustum(&polygon, &front_frustum, &mut scratch_a, &mut scratch_b)
                .to_vec();

        // Sanity: the clipper survived as a polygon the DFS will hand to
        // narrow_frustum (>= 3 verts, so `clipped_len < 3` doesn't early-
        // reject the portal). Localizes the failure site if the clipper
        // itself has regressed (as opposed to narrow_frustum).
        assert!(
            clipped.len() >= 3,
            "Sutherland-Hodgman must emit >= 3 vertices for this input \
             so the DFS hands the polygon to narrow_frustum; got {}",
            clipped.len()
        );

        let camera_pos = Vec3::ZERO;
        let camera_frustum = make_camera_frustum(camera_pos, Vec3::X);
        let narrowed = narrow_frustum(camera_pos, &clipped, &camera_frustum);

        assert!(
            narrowed.is_some(),
            "narrow_frustum rejected a geometrically valid portal polygon — \
             the three-state clipper should have prevented near-duplicate \
             leading vertices from reaching narrow_frustum in the first \
             place. If this fails, the SIDE_ON dedupe predicate in \
             clip_polygon_to_plane has regressed."
        );
    }

    /// Defense-in-depth gate for the `narrow_frustum` leading-triple
    /// fragility flagged by edge-case review on commit 1535c92.
    ///
    /// The S-maze fix hardened the clipper so it no longer emits the
    /// specific "near-duplicate leading pair from inside-by-epsilon
    /// vertex" shape, but the old `narrow_frustum` still trusted
    /// `(v1-v0) × (v2-v0)` and would collapse on any polygon whose
    /// first three vertices happened to be colinear — a shape that can
    /// still arise from BSP fragmentation or from polygons that were
    /// colinear at the source before any clipping occurred.
    ///
    /// Newell's method (Graphics Gems III, 1992) sums edge cross
    /// products across the entire polygon, so a single degenerate
    /// triple contributes zero while the rest of the polygon still
    /// supplies a correct normal. This probe feeds a pentagon whose
    /// leading three vertices are exactly colinear and asserts
    /// `narrow_frustum` still returns a valid frustum.
    #[test]
    fn narrow_frustum_accepts_colinear_leading_triple() {
        let camera_pos = Vec3::ZERO;
        let frustum = make_camera_frustum(camera_pos, Vec3::X);

        // Pentagon on the plane X=10: the 2×2 quad used in
        // `narrow_frustum_produces_tighter_frustum`, plus a midpoint
        // inserted on the bottom edge so v0, v1, v2 are colinear along
        // Z=4. (v1-v0)×(v2-v0) is exactly zero; Newell's sum of all
        // five edges still resolves to (±8, 0, 0).
        let portal = vec![
            Vec3::new(10.0, 4.0, 4.0),
            Vec3::new(10.0, 5.0, 4.0),
            Vec3::new(10.0, 6.0, 4.0),
            Vec3::new(10.0, 6.0, 6.0),
            Vec3::new(10.0, 4.0, 6.0),
        ];

        let narrowed = narrow_frustum(camera_pos, &portal, &frustum);
        assert!(
            narrowed.is_some(),
            "narrow_frustum rejected a pentagon with colinear leading \
             vertices — Newell's method should have derived a valid \
             normal from the remaining non-degenerate edges. If this \
             fails, the leading-triple cross product has regressed \
             into the polygon-normal computation."
        );
    }

    #[test]
    fn portal_traversal_sees_room_through_both_sides_of_pillar() {
        // Room layout with NARROW portals (2 units wide) matching the pillar
        // gap dimensions that cause issues in portal generation:
        //
        // Leaf A (camera room, X=0..120) --[portal 0 at X=120, Z=62..64]--> Leaf B (left gap)
        //                                --[portal 1 at X=120, Z=66..68]--> Leaf C (right gap)
        // Leaf B --[portal 2 at X=136, Z=62..64]--> Leaf D (far room, X=136..256)
        // Leaf C --[portal 3 at X=136, Z=66..68]--> Leaf D
        //
        // The portals are only 2 units wide (matching a narrow doorway gap).
        let portal_a_b = PortalData {
            polygon: vec![
                Vec3::new(120.0, 16.0, 62.0),
                Vec3::new(120.0, 112.0, 62.0),
                Vec3::new(120.0, 112.0, 64.0),
                Vec3::new(120.0, 16.0, 64.0),
            ],
            front_leaf: 0,
            back_leaf: 1,
        };
        let portal_a_c = PortalData {
            polygon: vec![
                Vec3::new(120.0, 16.0, 66.0),
                Vec3::new(120.0, 112.0, 66.0),
                Vec3::new(120.0, 112.0, 68.0),
                Vec3::new(120.0, 16.0, 68.0),
            ],
            front_leaf: 0,
            back_leaf: 2,
        };
        let portal_b_d = PortalData {
            polygon: vec![
                Vec3::new(136.0, 16.0, 62.0),
                Vec3::new(136.0, 112.0, 62.0),
                Vec3::new(136.0, 112.0, 64.0),
                Vec3::new(136.0, 16.0, 64.0),
            ],
            front_leaf: 1,
            back_leaf: 3,
        };
        let portal_c_d = PortalData {
            polygon: vec![
                Vec3::new(136.0, 16.0, 66.0),
                Vec3::new(136.0, 112.0, 66.0),
                Vec3::new(136.0, 112.0, 68.0),
                Vec3::new(136.0, 16.0, 68.0),
            ],
            front_leaf: 2,
            back_leaf: 3,
        };

        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![
                // Root splits at X=120
                NodeData {
                    plane_normal: Vec3::X,
                    plane_distance: 120.0,
                    front: BspChild::Node(1),
                    back: BspChild::Leaf(0),
                },
                // Split at X=136
                NodeData {
                    plane_normal: Vec3::X,
                    plane_distance: 136.0,
                    front: BspChild::Leaf(3),
                    back: BspChild::Node(2),
                },
                // Split at Z=65 (between the two gaps) to separate B and C
                NodeData {
                    plane_normal: Vec3::Z,
                    plane_distance: 65.0,
                    front: BspChild::Leaf(2),
                    back: BspChild::Leaf(1),
                },
            ],
            leaves: vec![
                // Leaf 0: camera room (A), X=0..120
                LeafData {
                    bounds_min: Vec3::new(0.0, 0.0, 0.0),
                    bounds_max: Vec3::new(120.0, 128.0, 128.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
                // Leaf 1: left gap passage (B), Z=62..64
                LeafData {
                    bounds_min: Vec3::new(120.0, 16.0, 62.0),
                    bounds_max: Vec3::new(136.0, 112.0, 64.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
                // Leaf 2: right gap passage (C), Z=66..68
                LeafData {
                    bounds_min: Vec3::new(120.0, 16.0, 66.0),
                    bounds_max: Vec3::new(136.0, 112.0, 68.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
                // Leaf 3: far room (D), X=136..256
                LeafData {
                    bounds_min: Vec3::new(136.0, 0.0, 0.0),
                    bounds_max: Vec3::new(256.0, 128.0, 128.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
            ],
            root: BspChild::Node(0),
            has_pvs: false,
            portals: vec![portal_a_b, portal_a_c, portal_b_d, portal_c_d],
            leaf_portals: vec![
                vec![0, 1], // leaf A touches portal 0 (A-B) and portal 1 (A-C)
                vec![0, 2], // leaf B touches portal 0 (A-B) and portal 2 (B-D)
                vec![1, 3], // leaf C touches portal 1 (A-C) and portal 3 (C-D)
                vec![2, 3], // leaf D touches portal 2 (B-D) and portal 3 (C-D)
            ],
            has_portals: true,
            texture_names: vec![],
            bvh: crate::geometry::BvhTree {
                nodes: vec![],
                leaves: vec![],
                root_node_index: 0,
            },
            lights: vec![],
            light_influences: vec![],
        };

        // Camera looking through the LEFT passage (Z=63, center of Z=62..64 gap).
        // Camera is in leaf A, looking toward +X.
        {
            let camera_pos = Vec3::new(16.0, 64.0, 63.0);
            let frustum = make_camera_frustum(camera_pos, Vec3::X);
            let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
            assert!(visible[0], "camera leaf A should be visible");
            assert!(
                visible[1],
                "leaf B (left gap) should be visible when looking through left doorway"
            );
            assert!(
                visible[3],
                "leaf D (far room) should be visible through left passage (A->B->D). \
                 If not, the narrow frustum through the 2-unit-wide portal A-B may be \
                 rejecting the 2-unit-wide portal B-D."
            );
        }

        // Camera looking through the RIGHT passage (Z=67, center of Z=66..68 gap).
        {
            let camera_pos = Vec3::new(16.0, 64.0, 67.0);
            let frustum = make_camera_frustum(camera_pos, Vec3::X);
            let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
            assert!(visible[0], "camera leaf A should be visible");
            assert!(
                visible[2],
                "leaf C (right gap) should be visible when looking through right doorway"
            );
            assert!(
                visible[3],
                "leaf D (far room) should be visible through right passage (A->C->D). \
                 If not, the narrow frustum through the 2-unit-wide portal A-C may be \
                 rejecting the 2-unit-wide portal C-D."
            );
        }
    }

    // --- Polygon-vs-frustum clipping tests ---

    /// Classify a polygon vertex as strictly inside every plane of a frustum
    /// (within the clip epsilon).
    fn point_inside_frustum(point: Vec3, frustum: &Frustum) -> bool {
        frustum
            .planes
            .iter()
            .all(|p| p.normal.dot(point) + p.dist >= -CLIP_EPSILON)
    }

    #[test]
    fn clip_polygon_fully_inside_is_unchanged() {
        let camera_pos = Vec3::ZERO;
        let frustum = make_camera_frustum(camera_pos, Vec3::X);

        // Small polygon centered on the line of sight, well inside the cone.
        let polygon = vec![
            Vec3::new(10.0, -0.5, -0.5),
            Vec3::new(10.0, 0.5, -0.5),
            Vec3::new(10.0, 0.5, 0.5),
            Vec3::new(10.0, -0.5, 0.5),
        ];

        let mut scratch_a: Vec<Vec3> = Vec::new();
        let mut scratch_b: Vec<Vec3> = Vec::new();
        let clipped = clip_polygon_to_frustum(&polygon, &frustum, &mut scratch_a, &mut scratch_b);
        assert_eq!(
            clipped.len(),
            4,
            "polygon fully inside frustum should retain all 4 vertices"
        );
        for (i, v) in clipped.iter().enumerate() {
            assert!(
                point_inside_frustum(*v, &frustum),
                "clipped vertex {i} should be inside the frustum"
            );
        }
    }

    #[test]
    fn clip_polygon_fully_outside_yields_empty() {
        let camera_pos = Vec3::ZERO;
        let frustum = make_camera_frustum(camera_pos, Vec3::X);

        // Polygon entirely behind the camera (on -X side, past the near plane).
        let polygon = vec![
            Vec3::new(-10.0, -1.0, -1.0),
            Vec3::new(-10.0, 1.0, -1.0),
            Vec3::new(-10.0, 1.0, 1.0),
            Vec3::new(-10.0, -1.0, 1.0),
        ];

        let mut scratch_a: Vec<Vec3> = Vec::new();
        let mut scratch_b: Vec<Vec3> = Vec::new();
        let clipped = clip_polygon_to_frustum(&polygon, &frustum, &mut scratch_a, &mut scratch_b);
        assert!(
            clipped.len() < 3,
            "polygon fully outside frustum should clip to empty (got {} verts)",
            clipped.len()
        );
    }

    #[test]
    fn clip_polygon_partial_stays_inside_frustum() {
        let camera_pos = Vec3::ZERO;
        let frustum = make_camera_frustum(camera_pos, Vec3::X);

        // Large polygon straddling the camera cone — extends from deep inside
        // the cone well past the left/right frustum planes.
        let polygon = vec![
            Vec3::new(10.0, -500.0, -1.0),
            Vec3::new(10.0, 500.0, -1.0),
            Vec3::new(10.0, 500.0, 1.0),
            Vec3::new(10.0, -500.0, 1.0),
        ];

        let mut scratch_a: Vec<Vec3> = Vec::new();
        let mut scratch_b: Vec<Vec3> = Vec::new();
        let clipped = clip_polygon_to_frustum(&polygon, &frustum, &mut scratch_a, &mut scratch_b);
        assert!(
            clipped.len() >= 3,
            "a polygon that straddles the frustum should clip to a non-empty polygon"
        );
        for (i, v) in clipped.iter().enumerate() {
            assert!(
                point_inside_frustum(*v, &frustum),
                "clipped vertex {i} at {v:?} should be inside the frustum"
            );
        }
    }

    #[test]
    fn clip_polygon_degenerate_input_yields_empty() {
        let frustum = make_camera_frustum(Vec3::ZERO, Vec3::X);
        let mut scratch_a: Vec<Vec3> = Vec::new();
        let mut scratch_b: Vec<Vec3> = Vec::new();
        assert!(clip_polygon_to_frustum(&[], &frustum, &mut scratch_a, &mut scratch_b).is_empty());
        assert!(
            clip_polygon_to_frustum(
                &[Vec3::X, Vec3::Y],
                &frustum,
                &mut scratch_a,
                &mut scratch_b
            )
            .is_empty()
        );
    }

    /// Test that a clipped polygon feeds a narrowed frustum whose vertices all
    /// lie inside the parent frustum. This is the strict-subset invariant at
    /// one hop.
    #[test]
    fn narrowed_frustum_from_clipped_polygon_is_subset_of_parent() {
        let camera_pos = Vec3::ZERO;
        let parent = make_camera_frustum(camera_pos, Vec3::X);

        // Portal that straddles the frustum boundary (large in Y).
        let portal = vec![
            Vec3::new(10.0, -500.0, -1.0),
            Vec3::new(10.0, 500.0, -1.0),
            Vec3::new(10.0, 500.0, 1.0),
            Vec3::new(10.0, -500.0, 1.0),
        ];

        let mut scratch_a: Vec<Vec3> = Vec::new();
        let mut scratch_b: Vec<Vec3> = Vec::new();
        let clipped: Vec<Vec3> =
            clip_polygon_to_frustum(&portal, &parent, &mut scratch_a, &mut scratch_b).to_vec();
        assert!(clipped.len() >= 3, "clipped polygon should be non-empty");

        // All clipped vertices lie inside the parent frustum by construction.
        for v in &clipped {
            assert!(
                point_inside_frustum(*v, &parent),
                "clipped polygon vertex {v:?} must lie inside parent frustum"
            );
        }

        // The narrowed frustum produced from the clipped polygon should accept
        // points that are clearly inside the narrowed cone and also inside the
        // parent — and should not accept points outside the parent frustum.
        let narrowed = narrow_frustum(camera_pos, &clipped, &parent)
            .expect("narrow_frustum should succeed for a clipped, visible polygon");

        // A sample point far outside the parent's side plane must also be
        // rejected by the narrowed frustum (strict subset means: outside
        // parent implies outside narrowed).
        let outside_parent = Vec3::new(20.0, 500.0, 0.0);
        assert!(
            !point_inside_frustum(outside_parent, &parent),
            "sanity: test point should be outside parent"
        );
        assert!(
            !point_inside_frustum(outside_parent, &narrowed),
            "point outside parent must be outside the narrowed (subset) frustum"
        );
    }

    #[test]
    fn multi_hop_narrowed_frustums_preserve_strict_subset_invariant() {
        // Three collinear portals along +X. After clipping+narrowing at each
        // hop, every leaf visible in the narrowed frustum must also be inside
        // the original camera frustum.
        let camera_pos = Vec3::new(0.0, 0.0, 0.0);
        let parent = make_camera_frustum(camera_pos, Vec3::X);

        let portal_a = vec![
            Vec3::new(10.0, -2.0, -2.0),
            Vec3::new(10.0, 2.0, -2.0),
            Vec3::new(10.0, 2.0, 2.0),
            Vec3::new(10.0, -2.0, 2.0),
        ];
        let portal_b = vec![
            Vec3::new(20.0, -2.0, -2.0),
            Vec3::new(20.0, 2.0, -2.0),
            Vec3::new(20.0, 2.0, 2.0),
            Vec3::new(20.0, -2.0, 2.0),
        ];
        let portal_c = vec![
            Vec3::new(30.0, -2.0, -2.0),
            Vec3::new(30.0, 2.0, -2.0),
            Vec3::new(30.0, 2.0, 2.0),
            Vec3::new(30.0, -2.0, 2.0),
        ];

        // Tests construct scratch buffers locally; they are off the per-frame
        // hot path. Clipped slices are copied into owned Vecs so all three
        // hops' results can be compared simultaneously below.
        let mut scratch_a: Vec<Vec3> = Vec::new();
        let mut scratch_b: Vec<Vec3> = Vec::new();

        // Hop 1.
        let clipped_a: Vec<Vec3> =
            clip_polygon_to_frustum(&portal_a, &parent, &mut scratch_a, &mut scratch_b).to_vec();
        assert!(clipped_a.len() >= 3);
        let narrowed_1 = narrow_frustum(camera_pos, &clipped_a, &parent).expect("hop 1");

        // Hop 2: clip next portal against hop-1 frustum.
        let clipped_b: Vec<Vec3> =
            clip_polygon_to_frustum(&portal_b, &narrowed_1, &mut scratch_a, &mut scratch_b)
                .to_vec();
        assert!(clipped_b.len() >= 3);
        let narrowed_2 = narrow_frustum(camera_pos, &clipped_b, &narrowed_1).expect("hop 2");

        // Hop 3.
        let clipped_c: Vec<Vec3> =
            clip_polygon_to_frustum(&portal_c, &narrowed_2, &mut scratch_a, &mut scratch_b)
                .to_vec();
        assert!(clipped_c.len() >= 3);
        let narrowed_3 = narrow_frustum(camera_pos, &clipped_c, &narrowed_2).expect("hop 3");

        // Hop-wise strict-subset check: each clipped polygon must lie fully
        // inside the frustum it was clipped against. This is the inductive
        // step that guarantees every narrowed frustum is a subset of its
        // immediate predecessor — from which "subset of the original camera
        // frustum" follows by induction.
        for v in &clipped_a {
            assert!(
                point_inside_frustum(*v, &parent),
                "hop 1 clipped vertex {v:?} must lie inside the parent frustum"
            );
        }
        for v in &clipped_b {
            assert!(
                point_inside_frustum(*v, &narrowed_1),
                "hop 2 clipped vertex {v:?} must lie inside the hop-1 narrowed frustum"
            );
        }
        for v in &clipped_c {
            assert!(
                point_inside_frustum(*v, &narrowed_2),
                "hop 3 clipped vertex {v:?} must lie inside the hop-2 narrowed frustum"
            );
        }

        // Transitively, every clipped vertex at any hop lies inside the
        // original parent frustum — the induction target.
        for v in clipped_a
            .iter()
            .chain(clipped_b.iter())
            .chain(clipped_c.iter())
        {
            assert!(
                point_inside_frustum(*v, &parent),
                "clipped vertex {v:?} must lie inside the original camera frustum"
            );
        }

        // Subset-at-each-hop sampled check: a sample of points that lie inside
        // the narrowed frustum at hop N must also lie inside the frustum at
        // hop N-1. Sample the clipped polygon vertices themselves plus the
        // polygon centroid at each hop — both are guaranteed inside the
        // narrowed frustum (they lie on the near plane and are bounded by the
        // edge planes).
        let centroid = |poly: &[Vec3]| poly.iter().copied().sum::<Vec3>() / poly.len() as f32;

        // Hop 1 → parent.
        for v in &clipped_a {
            assert!(point_inside_frustum(*v, &parent));
        }
        assert!(point_inside_frustum(centroid(&clipped_a), &parent));
        // Hop 2 → hop 1.
        for v in &clipped_b {
            assert!(point_inside_frustum(*v, &narrowed_1));
        }
        assert!(point_inside_frustum(centroid(&clipped_b), &narrowed_1));
        // Hop 3 → hop 2.
        for v in &clipped_c {
            assert!(point_inside_frustum(*v, &narrowed_2));
        }
        assert!(point_inside_frustum(centroid(&clipped_c), &narrowed_2));

        // And points clearly outside the parent must be outside every
        // narrowed frustum, at every hop.
        let way_off = Vec3::new(15.0, 500.0, 0.0);
        assert!(!point_inside_frustum(way_off, &parent));
        assert!(
            !point_inside_frustum(way_off, &narrowed_1),
            "hop 1 must reject points outside the parent"
        );
        assert!(
            !point_inside_frustum(way_off, &narrowed_2),
            "hop 2 must reject points outside the parent"
        );
        assert!(
            !point_inside_frustum(way_off, &narrowed_3),
            "hop 3 must reject points outside the parent"
        );
    }

    #[test]
    fn portal_traverse_straddling_portal_hides_unreachable_side_branch() {
        // Straight-through layout: camera in leaf 0 looking +X.
        // Portal 0 (A -> B) straddles the camera's side plane — it extends
        // far beyond the frustum to the +Y direction. Without polygon
        // clipping, frustum narrowing through the un-clipped portal could
        // produce a cone that extends into -Y regions the camera cannot see
        // and incorrectly admit off-axis neighbors.
        //
        // This test asserts that with clipping in place, leaf B is still
        // visible (the portal is in view) and a far off-axis leaf C reached
        // through an orthogonal portal at leaf B is correctly hidden.
        let portal_a_b = PortalData {
            polygon: vec![
                // 1000-unit-tall portal at X=10, centered on Z=0.
                Vec3::new(10.0, -500.0, -1.0),
                Vec3::new(10.0, 500.0, -1.0),
                Vec3::new(10.0, 500.0, 1.0),
                Vec3::new(10.0, -500.0, 1.0),
            ],
            front_leaf: 0,
            back_leaf: 1,
        };
        // Portal 1 (B -> C) is far out in +Y, well outside the camera's
        // actual view cone even though leaf B is reachable.
        let portal_b_c = PortalData {
            polygon: vec![
                Vec3::new(15.0, 400.0, -1.0),
                Vec3::new(20.0, 400.0, -1.0),
                Vec3::new(20.0, 400.0, 1.0),
                Vec3::new(15.0, 400.0, 1.0),
            ],
            front_leaf: 1,
            back_leaf: 2,
        };

        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![],
            leaves: vec![
                LeafData {
                    bounds_min: Vec3::new(0.0, -500.0, -500.0),
                    bounds_max: Vec3::new(10.0, 500.0, 500.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
                LeafData {
                    bounds_min: Vec3::new(10.0, -500.0, -500.0),
                    bounds_max: Vec3::new(25.0, 500.0, 500.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
                LeafData {
                    bounds_min: Vec3::new(15.0, 400.0, -500.0),
                    bounds_max: Vec3::new(25.0, 600.0, 500.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
            ],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![portal_a_b, portal_b_c],
            leaf_portals: vec![vec![0], vec![0, 1], vec![1]],
            has_portals: true,
            texture_names: vec![],
            bvh: crate::geometry::BvhTree {
                nodes: vec![],
                leaves: vec![],
                root_node_index: 0,
            },
            lights: vec![],
            light_influences: vec![],
        };

        let camera_pos = Vec3::new(1.0, 0.0, 0.0);
        let frustum = make_camera_frustum(camera_pos, Vec3::X);
        let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);

        assert!(visible[0], "camera leaf should always be visible");
        assert!(
            visible[1],
            "leaf B should be visible through the straddling portal"
        );
        assert!(
            !visible[2],
            "leaf C should be hidden: portal 1 is far off-axis and \
             unreachable through the clipped sight cone"
        );
    }

    /// Regression test for the "two paths to the same leaf, narrower path
    /// wins, downstream reach is lost" topology fixed by per-chain DFS.
    ///
    /// Topology (abstract; bounding boxes are not used by portal_traverse):
    ///
    ///   A (camera) -- portal 0 (NARROW 0.1x0.1 at X=10) --> B
    ///   A          -- portal 1 (WIDE   4.0x4.0 at X=10) --> C
    ///   B          -- portal 2 (2.0x2.0 at X=20) --------> X
    ///   C          -- portal 3 (2.0x2.0 at X=20) --------> X
    ///   X          -- portal 4 (1.0x1.0 at X=30, offset  -> Y
    ///                           to Y=1..2, Z=-0.5..0.5)
    ///
    /// All portals lie in YZ planes perpendicular to +X. Portal 0 and portal 1
    /// share the same spatial position (both at X=10 centered on the origin)
    /// — portal_traverse cares about topology and polygon shape, not physical
    /// room layout, so this is a legal test fixture. Likewise portals 2 and 3
    /// overlap at X=20.
    ///
    /// The load-bearing geometry:
    ///
    /// - Camera frustum (100° hfov) easily contains both A-outbound portals.
    /// - The B-path's frustum narrows severely through portal 0's 0.1x0.1 slit.
    ///   By X=30 that cone has a radius of roughly 0.15 units from the X axis.
    /// - The C-path's frustum narrows gently through portal 1's 4.0x4.0 aperture.
    ///   By X=30 that cone has a radius of several units.
    /// - Portal 4 is offset to Y=1..2 so it lies **outside** the narrow B-path
    ///   cone at X=30 (clips to empty) and **inside** the wide C-path cone
    ///   (passes through cleanly).
    ///
    /// Under BFS-keyed-on-leaves (the former implementation):
    ///   A's outbound iteration order is [0, 1] → A→B runs first → X marked
    ///   visible with the narrow frustum planted by the B-path → A→C→X is
    ///   then rejected by the already-visited early-skip → X's outbound
    ///   portal 4 is evaluated against the narrow B-frustum → Y missed.
    ///
    /// Under DFS-with-per-path-tracking (the current implementation):
    ///   Both A→B→X and A→C→X chains run independently. The C-path produces
    ///   a wide frustum at X that does not clip X→Y to empty → Y visible.
    ///
    /// The test asserts visibility of all five leaves. The BFS topology fails
    /// on `visible[4]` (Y) and DFS passes.
    #[test]
    fn portal_traverse_two_paths_to_same_leaf_uses_widest_frustum() {
        // Portal 0: A→B, NARROW slit at X=10.
        let portal_a_b = PortalData {
            polygon: vec![
                Vec3::new(10.0, -0.05, -0.05),
                Vec3::new(10.0, 0.05, -0.05),
                Vec3::new(10.0, 0.05, 0.05),
                Vec3::new(10.0, -0.05, 0.05),
            ],
            front_leaf: 0, // A
            back_leaf: 1,  // B
        };

        // Portal 1: A→C, WIDE aperture at X=10 (same spatial position as
        // portal 0; they serve different topology roles in this abstract
        // fixture).
        let portal_a_c = PortalData {
            polygon: vec![
                Vec3::new(10.0, -2.0, -2.0),
                Vec3::new(10.0, 2.0, -2.0),
                Vec3::new(10.0, 2.0, 2.0),
                Vec3::new(10.0, -2.0, 2.0),
            ],
            front_leaf: 0, // A
            back_leaf: 2,  // C
        };

        // Portal 2: B→X at X=20.
        let portal_b_x = PortalData {
            polygon: vec![
                Vec3::new(20.0, -1.0, -1.0),
                Vec3::new(20.0, 1.0, -1.0),
                Vec3::new(20.0, 1.0, 1.0),
                Vec3::new(20.0, -1.0, 1.0),
            ],
            front_leaf: 1, // B
            back_leaf: 3,  // X
        };

        // Portal 3: C→X at X=20 (same spatial position as portal 2).
        let portal_c_x = PortalData {
            polygon: vec![
                Vec3::new(20.0, -1.0, -1.0),
                Vec3::new(20.0, 1.0, -1.0),
                Vec3::new(20.0, 1.0, 1.0),
                Vec3::new(20.0, -1.0, 1.0),
            ],
            front_leaf: 2, // C
            back_leaf: 3,  // X
        };

        // Portal 4: X→Y at X=30, offset to Y=1..2 so it sits outside the
        // narrow B-path cone and inside the wide C-path cone.
        let portal_x_y = PortalData {
            polygon: vec![
                Vec3::new(30.0, 1.0, -0.5),
                Vec3::new(30.0, 2.0, -0.5),
                Vec3::new(30.0, 2.0, 0.5),
                Vec3::new(30.0, 1.0, 0.5),
            ],
            front_leaf: 3, // X
            back_leaf: 4,  // Y
        };

        let leaf_template = || LeafData {
            bounds_min: Vec3::new(-1000.0, -1000.0, -1000.0),
            bounds_max: Vec3::new(1000.0, 1000.0, 1000.0),
            face_start: 0,
            face_count: 0,
            pvs: vec![],
            is_solid: false,
        };

        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![],
            leaves: vec![
                leaf_template(), // 0: A
                leaf_template(), // 1: B
                leaf_template(), // 2: C
                leaf_template(), // 3: X
                leaf_template(), // 4: Y
            ],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![portal_a_b, portal_a_c, portal_b_x, portal_c_x, portal_x_y],
            // Iteration order matters: A lists portal 0 (narrow) BEFORE
            // portal 1 (wide) so BFS would deterministically plant the
            // narrow frustum at X first.
            leaf_portals: vec![
                vec![0, 1],    // A touches portals 0, 1
                vec![0, 2],    // B touches portals 0, 2
                vec![1, 3],    // C touches portals 1, 3
                vec![2, 3, 4], // X touches portals 2, 3, 4
                vec![4],       // Y touches portal 4
            ],
            has_portals: true,
            texture_names: vec![],
            bvh: crate::geometry::BvhTree {
                nodes: vec![],
                leaves: vec![],
                root_node_index: 0,
            },
            lights: vec![],
            light_influences: vec![],
        };

        // Camera at origin looking +X. The camera frustum is wide enough that
        // both A-outbound portals are accepted on the first hop.
        let camera_pos = Vec3::new(0.0, 0.0, 0.0);
        let frustum = make_camera_frustum(camera_pos, Vec3::X);
        let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);

        assert!(visible[0], "leaf A (camera) must be visible");
        assert!(visible[1], "leaf B must be visible (A→B direct)");
        assert!(visible[2], "leaf C must be visible (A→C direct)");
        assert!(
            visible[3],
            "leaf X must be visible (reachable via either path)"
        );
        assert!(
            visible[4],
            "leaf Y must be visible via the A→C→X→Y chain. Under the \
             previous BFS-keyed-on-leaves implementation, the A→B→X chain \
             would plant a narrow frustum at X that clips X→Y to empty, \
             and the wider A→C→X chain would be dropped by the \
             visible[X] early-skip before it ever reached X→Y."
        );
    }

    /// Regression probe: camera sits 0.03 units from a vertical portal
    /// wall, reproducing the blank-frame scenario captured from
    /// `test-3.prl` at 2026-04-11T22:52:11Z. Camera at `(4.91, 0.92,
    /// -14.67)` inside leaf 99 whose -X wall is on `x = 4.88`.
    ///
    /// **Root cause (confirmed by diagnostic trace on 2026-04-11):** the
    /// render-pipeline near clip (`camera::NEAR = 0.1`) is baked into the
    /// view-projection matrix used to build the visibility frustum. When
    /// the camera sits within 0.1 units of a portal plane, the entire
    /// portal polygon lies between the camera apex and the near plane, so
    /// every vertex fails the near-plane test and Sutherland-Hodgman
    /// clips the polygon to empty. Side-plane clipping is not the
    /// culprit: pushing only the near plane up to the camera apex (near
    /// ≈ 0) makes the portal reach its neighbor on this exact fixture.
    ///
    /// The test geometry is copied from the live trace verbatim — same
    /// camera position, same leaf bounds, same 6.5×4.88 portal rectangle.
    /// Leaf A is the camera leaf (+X side), leaf B is the -X neighbor.
    ///
    /// Routes through `visibility::extract_frustum_planes` directly
    /// rather than the module-private `extract_test_frustum` copy to
    /// exercise the real production path end-to-end.
    #[test]
    fn portal_traverse_reaches_neighbor_when_camera_is_close_to_portal_wall() {
        use crate::camera;
        use crate::visibility::extract_frustum_planes;

        let portal = PortalData {
            polygon: vec![
                Vec3::new(4.88, 0.00, -17.88),
                Vec3::new(4.88, 0.00, -11.38),
                Vec3::new(4.88, 4.88, -11.38),
                Vec3::new(4.88, 4.88, -17.88),
            ],
            front_leaf: 0,
            back_leaf: 1,
        };

        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![],
            leaves: vec![
                LeafData {
                    bounds_min: Vec3::new(4.88, 0.00, -17.88),
                    bounds_max: Vec3::new(11.38, 4.88, -11.38),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
                LeafData {
                    bounds_min: Vec3::new(-50.00, 0.00, -17.88),
                    bounds_max: Vec3::new(4.88, 4.88, -11.38),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
            ],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![portal],
            leaf_portals: vec![vec![0], vec![0]],
            has_portals: true,
            texture_names: vec![],
            bvh: crate::geometry::BvhTree {
                nodes: vec![],
                leaves: vec![],
                root_node_index: 0,
            },
            lights: vec![],
            light_influences: vec![],
        };

        // Camera pose from the captured blank-frame trace. The live trace
        // did not record view direction; -X stares straight at the portal
        // wall, which is the natural "camera pressed against a wall"
        // orientation and reproduces the same clip-to-empty symptom.
        let camera_pos = Vec3::new(4.91, 0.92, -14.67);
        let look_dir = Vec3::NEG_X;

        let aspect = 16.0 / 9.0;
        let vfov = 2.0 * ((camera::HFOV / 2.0).tan() / aspect).atan();
        let view = Mat4::look_at_rh(camera_pos, camera_pos + look_dir, Vec3::Y);
        let proj = Mat4::perspective_rh(vfov, aspect, camera::NEAR, camera::FAR);
        let frustum = extract_frustum_planes(proj * view);

        // Precondition: at least one portal vertex must lie outside at
        // least one frustum plane, i.e. the clip step has real work to do.
        // A test that passes because every vertex is trivially inside
        // every plane proves nothing about the clip behaviour.
        let any_vertex_outside = world.portals[0].polygon.iter().any(|&v| {
            frustum
                .planes
                .iter()
                .any(|p| p.normal.dot(v) + p.dist < 0.0)
        });
        assert!(
            any_vertex_outside,
            "precondition failed: every portal vertex is inside every \
             frustum plane, so the clip step is a no-op and this test \
             does not exercise the blank-frame scenario."
        );

        let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);

        assert!(visible[0], "camera leaf must always be visible");
        assert!(
            visible[1],
            "leaf B must be reachable through the portal even when the \
             camera sits 0.03 units from the portal plane. Failure here \
             means the visibility frustum's near plane (inherited from \
             the render pipeline's 0.1-unit near clip) is clipping the \
             entire portal polygon to empty — the blank-frame bug in \
             tight corridors."
        );
    }

    /// Shape-check the compact trace format: header fields, at least one
    /// event line, and the new-format summary. Uses the module-private
    /// `portal_traverse_inner` to read the formatted buffer directly —
    /// asserting on `log::info!` output would need a custom test logger and
    /// would buy nothing over reading the source-of-truth string.
    #[test]
    fn portal_traverse_capture_emits_compact_header_fields() {
        let world = three_leaf_chain();
        let camera_pos = Vec3::new(16.0, 32.0, 32.0);
        let frustum = make_camera_frustum(camera_pos, Vec3::X);
        let (_visible, trace) = portal_traverse_inner(camera_pos, 0, &frustum, &world, true);
        let buf = trace.expect("capture: true should produce a trace buffer");

        // Header fields — these are the new per-frame camera-leaf diagnostics
        // added for the flicker bug hunt.
        assert!(buf.contains("cam=("), "header missing cam=(: {buf}");
        assert!(buf.contains("leaf="), "header missing leaf=: {buf}");
        assert!(buf.contains("faces="), "header missing faces=: {buf}");
        assert!(buf.contains("bnds=("), "header missing bnds=(: {buf}");
        assert!(buf.contains("leaves="), "header missing leaves=: {buf}");

        // At least one accepted/rejected event line under the header. The
        // straight corridor walks into leaf B and leaf C, so there's at
        // least one `  acc ` line.
        let has_event = buf
            .lines()
            .any(|line| line.starts_with("  acc ") || line.starts_with("  rej "));
        assert!(has_event, "expected at least one event line: {buf}");

        // Summary: starts with `  = reach=` and contains `rej[`.
        let summary = buf
            .lines()
            .find(|line| line.starts_with("  = reach="))
            .expect("expected a summary line starting with `  = reach=`");
        assert!(
            summary.contains("rej["),
            "summary missing rej[ bracket: {summary}"
        );
    }
}
