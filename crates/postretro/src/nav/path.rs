// One-shot pathfinding query over a NavGraph: A* across regions, then a Simple
// Stupid Funnel string-pull over the corridor's portal segments.
// See: context/lib/build_pipeline.md §Navigation bake

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::ops::Deref;

use glam::Vec3;
use postretro_level_format::navmesh::NavPortal;

use crate::collision::SKIN_DISTANCE;

use super::{NavGraph, distance_xz};

/// Shared slack for every clearance comparison in the endpoint-repair pass. The
/// violation test, the bevel "already clear" early-out, and the corner-identity
/// checks must all use ONE epsilon against ONE reference (the raw endpoint), or a
/// detected violation can fall into a band where no bevel is produced — dropping
/// a routable corridor to `None`. Keep them consistent by routing every such
/// comparison through this constant.
const CLEARANCE_EPS: f32 = 1e-5;

/// A funnelled path: the waypoint positions plus a parallel flag per waypoint.
///
/// Invariant: `points.len() == mandatory_waypoints.len()`. The two vectors are
/// built in lockstep and stay equal-length by construction; `mandatory_waypoints[i]`
/// marks `points[i]` as a clearance-mandated corner the consumer must not shortcut.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NavPath {
    points: Vec<Vec3>,
    mandatory_waypoints: Vec<bool>,
}

impl NavPath {
    fn direct(start: Vec3, goal: Vec3) -> Self {
        Self {
            points: vec![start, goal],
            mandatory_waypoints: vec![false, false],
        }
    }

    /// Split into the parallel `(points, mandatory_waypoints)` vectors. The two
    /// are always equal length (see the `NavPath` invariant); consumers index
    /// them together.
    pub(crate) fn into_parts(self) -> (Vec<Vec3>, Vec<bool>) {
        (self.points, self.mandatory_waypoints)
    }
}

impl Deref for NavPath {
    type Target = [Vec3];

    fn deref(&self) -> &Self::Target {
        &self.points
    }
}

/// One-shot path query: A* over portal crossings + funnel string-pull. Resolves
/// the regions containing `start` and `goal`, runs A* over directed portal
/// crossings anchored on the true `start`/`goal` (edge cost = XZ distance
/// between portal-segment midpoints, seeded/closed on the real endpoints,
/// heuristic = straight-line XZ distance to `goal`), reconstructs the exact
/// portal corridor A* chose, then funnels it to the tightest waypoint list
/// within the corridor.
///
/// Endpoint resolution tolerates the eroded wall margin: each endpoint resolves
/// via [`NavGraph::resolve_region_at`], so a capsule legitimately standing
/// against a wall — inside the conservatively-eroded band and therefore outside
/// every region — still routes from/to its nearest region instead of failing.
/// The emitted terminals normally keep the RAW `start`/`goal` positions; only the
/// region resolution snaps. This matters for pursuit: chase targets hug corners
/// and steered agents get pushed wall-ward, and a query that returned `None` for
/// those positions froze chasing agents in a permanent `blocked` state.
///
/// The one exception: a terminal snapped into the eroded band can land inside a
/// wide portal endpoint's clearance disk. Such a terminal is projected onto the
/// disk boundary, so the emitted first/last waypoint becomes a walkable STANDOFF
/// at the obstacle edge rather than the raw start/goal. The true entity target
/// stays on the agent's steering `destination` (independent of this path), which
/// is what engagement and arrival distance key off — the standoff is only where
/// the agent walks to.
///
/// Returns `None` when `start` or `goal` lies farther than the snap tolerance
/// from every region, when either endpoint is non-finite, or when no corridor
/// connects their regions. A reachable goal yields a path whose first/last
/// waypoints are `start`/`goal` (or their disk-boundary standoffs); a goal in the
/// start region is a trivial two-point `[start, goal]`.
pub(crate) fn find_path(graph: &NavGraph, start: Vec3, goal: Vec3) -> Option<NavPath> {
    // Finiteness guard: a NaN/inf endpoint makes every funnel area comparison
    // false, silently collapsing the result to a straight `[start, goal]` line
    // that may cross solid geometry. Reject it rather than emit a path through a
    // wall. (`region_at` would also fail on NaN, but inf could resolve a region.)
    debug_assert!(
        start.is_finite() && goal.is_finite(),
        "find_path called with non-finite start/goal"
    );
    if !start.is_finite() || !goal.is_finite() {
        return None;
    }

    let start_region = graph.resolve_region_at(start)?;
    let goal_region = graph.resolve_region_at(goal)?;

    if start_region == goal_region {
        // Same region: no portal to cross, the straight segment is the path.
        return Some(NavPath::direct(start, goal));
    }

    let corridor = astar_corridor(graph, start, goal, start_region, goal_region)?;
    let portals = oriented_portals(graph, &corridor);
    // The start/goal guard above does not cover the corridor's own geometry: a
    // corrupt baked portal endpoint (NaN/inf) makes the funnel's area tests false
    // and collapses the corridor to a straight `[start, goal]` line through walls,
    // exactly as a non-finite endpoint would. Bail rather than emit that path.
    if portals
        .iter()
        .any(|(l, r)| !l.is_finite() || !r.is_finite())
    {
        debug_assert!(
            false,
            "find_path: corridor has a non-finite portal endpoint"
        );
        return None;
    }
    // Funnel corners become capsule-center movement targets. Preserve the
    // collision sweep's skin across that seam so a radius-clear corner does
    // not still consume the first steering ticks in `collide_and_slide`.
    let corner_clearance_radius = graph.agent.radius.max(0.0) + SKIN_DISTANCE;
    let gates = inset_portals(&portals, corner_clearance_radius)?;
    let pulled_path = funnel(start, goal, &gates);
    ensure_endpoint_clearance(&pulled_path, &gates, corner_clearance_radius)
}

/// One hop of the region corridor: which portal A* crossed and which direction
/// (`from_region` is the region the agent leaves through this portal).
struct CorridorHop {
    portal_index: usize,
    from_region: usize,
}

/// Midpoint of a portal's segment as a world position.
fn portal_midpoint(portal: &NavPortal) -> Vec3 {
    let l = Vec3::from_array(portal.left);
    let r = Vec3::from_array(portal.right);
    (l + r) * 0.5
}

/// Priority-queue entry: min-heap on `f = g + h` via `Reverse`-style ordering.
/// `node` is a *directed portal crossing* (see [`astar_corridor`]).
struct Frontier {
    node: usize,
    f: f32,
}

impl PartialEq for Frontier {
    fn eq(&self, other: &Self) -> bool {
        self.f == other.f
    }
}
impl Eq for Frontier {}
impl PartialOrd for Frontier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Frontier {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse so `BinaryHeap` (a max-heap) pops the smallest `f` first.
        // NaN is not expected (finite world coords); fall back to Equal.
        other.f.partial_cmp(&self.f).unwrap_or(Ordering::Equal)
    }
}

/// A* over *directed portal crossings*, anchored on the true `start`/`goal`
/// positions (not region centroids). Each search node is one directed crossing
/// of a portal — encoded `2 * portal_index + dir`, `dir = 0` crossing
/// `region_a → region_b` (arriving in `region_b`), `dir = 1` the reverse. The
/// start expands to every portal of `start_region` at cost
/// `distance_xz(start, portal_mid)`; a crossing expands to the other portals of
/// the region it arrived in at portal-mid → portal-mid cost; a crossing arriving
/// in `goal_region` closes with the admissible straight-line heuristic
/// `distance_xz(portal_mid, goal)`, which is the exact remaining cost there, so
/// the first goal-region node popped is optimal.
///
/// Anchoring on the real endpoints (not centroids) is what lets two agents in
/// one large region pick *different* doorways toward the goal by where they
/// actually stand — the centroid metric charged both from the same region
/// center and mis-picked. Returns the ordered corridor of hops, each naming the
/// exact portal and the region it was left through (so a region pair joined by
/// two distinct portals resolves to the one A* costed). `None` when
/// disconnected.
fn astar_corridor(
    graph: &NavGraph,
    start: Vec3,
    goal: Vec3,
    start_region: usize,
    goal_region: usize,
) -> Option<Vec<CorridorHop>> {
    let portals = graph.portals();
    let mid = |portal_index: usize| portal_midpoint(&portals[portal_index]);
    // The region a directed node arrives in, and the region it was left through.
    let arrived_region = |node: usize| -> usize {
        let p = &portals[node / 2];
        if node % 2 == 0 {
            p.region_b as usize
        } else {
            p.region_a as usize
        }
    };
    let from_region = |node: usize| -> usize {
        let p = &portals[node / 2];
        if node % 2 == 0 {
            p.region_a as usize
        } else {
            p.region_b as usize
        }
    };
    // The directed node that leaves `region` through portal `portal_index`, or
    // `None` when the portal does not touch `region`.
    let directed_leaving = |portal_index: usize, region: usize| -> Option<usize> {
        let p = &portals[portal_index];
        if p.region_a as usize == region {
            Some(2 * portal_index)
        } else if p.region_b as usize == region {
            Some(2 * portal_index + 1)
        } else {
            None
        }
    };

    let node_count = portals.len() * 2;
    let mut g_score = vec![f32::INFINITY; node_count];
    // `came_from[node] = Some(previous_node)`, or `None` when reached from the
    // virtual start (the first crossing out of `start_region`).
    let mut came_from: Vec<Option<usize>> = vec![None; node_count];
    let mut open = BinaryHeap::new();

    // Seed: cross each portal of the start region, costed from the true start.
    for &portal_index in graph.region_portal_indices(start_region) {
        let Some(node) = directed_leaving(portal_index, start_region) else {
            continue;
        };
        let g = distance_xz(start, mid(portal_index));
        if g < g_score[node] {
            g_score[node] = g;
            came_from[node] = None;
            open.push(Frontier {
                node,
                f: g + distance_xz(mid(portal_index), goal),
            });
        }
    }

    while let Some(Frontier { node, .. }) = open.pop() {
        let region = arrived_region(node);
        if region == goal_region {
            return Some(reconstruct(&came_from, &from_region, node));
        }

        for &portal_index in graph.region_portal_indices(region) {
            if portal_index == node / 2 {
                continue; // don't immediately re-cross the portal just crossed
            }
            let Some(next) = directed_leaving(portal_index, region) else {
                continue;
            };
            let tentative = g_score[node] + distance_xz(mid(node / 2), mid(portal_index));
            if tentative < g_score[next] {
                g_score[next] = tentative;
                came_from[next] = Some(node);
                open.push(Frontier {
                    node: next,
                    f: tentative + distance_xz(mid(portal_index), goal),
                });
            }
        }
    }

    None
}

/// Walk `came_from` back from the closing crossing to build the forward-ordered
/// corridor. Each directed node is one `CorridorHop` (its portal plus the region
/// it was left through).
fn reconstruct(
    came_from: &[Option<usize>],
    from_region: &impl Fn(usize) -> usize,
    end_node: usize,
) -> Vec<CorridorHop> {
    let mut hops = Vec::new();
    let mut current = Some(end_node);
    while let Some(node) = current {
        hops.push(CorridorHop {
            portal_index: node / 2,
            from_region: from_region(node),
        });
        current = came_from[node];
    }
    hops.reverse();
    hops
}

/// Resolve each corridor hop to a traversal-oriented `(left, right)` portal
/// segment. Stored `left`/`right` are bake-fixed relative to `region_a < region_b`;
/// when the agent crosses from `region_b` to `region_a` the handedness flips, so
/// swap them to keep "left" on the agent's left for the funnel.
fn oriented_portals(graph: &NavGraph, corridor: &[CorridorHop]) -> Vec<(Vec3, Vec3)> {
    let portals = graph.portals();
    corridor
        .iter()
        .map(|hop| {
            let portal = &portals[hop.portal_index];
            let left = Vec3::from_array(portal.left);
            let right = Vec3::from_array(portal.right);
            // Crossing region_a -> region_b keeps the bake orientation; crossing
            // region_b -> region_a reverses it.
            if hop.from_region == portal.region_a as usize {
                (left, right)
            } else {
                (right, left)
            }
        })
        .collect()
}

/// Twice the signed area of triangle (a, b, c) on the XZ plane. The sign encodes
/// turn handedness for the funnel's left/right tightening tests. In the XZ
/// projection (X east, Z north) with `left` placed on the agent's left of the
/// travel direction, the SSF tests expect `(b-a) x (c-a)` with Z taking the role
/// Y takes in the classic XY formulation — i.e. `abz*acx - abx*acz`.
fn triangle_area_xz(a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let abx = b.x - a.x;
    let abz = b.z - a.z;
    let acx = c.x - a.x;
    let acz = c.z - a.z;
    abz * acx - abx * acz
}

#[derive(Clone, Copy)]
struct FunnelEndpoint {
    point: Vec3,
    raw_endpoint: Option<Vec3>,
    portal_interior_xz: Option<Vec3>,
}

impl FunnelEndpoint {
    fn terminal(point: Vec3) -> Self {
        Self {
            point,
            raw_endpoint: None,
            portal_interior_xz: None,
        }
    }
}

#[derive(Clone, Copy)]
struct FunnelGate {
    left: FunnelEndpoint,
    right: FunnelEndpoint,
}

/// Shrink portal gates before string-pulling so every funnel comparison and
/// committed apex uses the same clearance-safe geometry. Portal width and
/// direction are horizontal; Y is interpolated along the original endpoint
/// pair so stepped or sloped portal height remains meaningful.
fn inset_portals(portals: &[(Vec3, Vec3)], clearance_radius: f32) -> Option<Vec<FunnelGate>> {
    let clearance_radius = clearance_radius.max(0.0);
    portals
        .iter()
        .map(|&(left, right)| {
            let delta_xz = Vec3::new(right.x - left.x, 0.0, right.z - left.z);
            let width_xz = delta_xz.length();
            if !delta_xz.is_finite() || !width_xz.is_finite() {
                return None;
            }
            if width_xz <= 2.0 * clearance_radius || width_xz <= f32::EPSILON {
                let midpoint = (left + right) * 0.5;
                if !midpoint.is_finite() {
                    return None;
                }
                return Some(FunnelGate {
                    left: FunnelEndpoint::terminal(midpoint),
                    right: FunnelEndpoint::terminal(midpoint),
                });
            }

            let inset_fraction = clearance_radius / width_xz;
            let left_interior = delta_xz / width_xz;
            let gate = FunnelGate {
                left: FunnelEndpoint {
                    point: left.lerp(right, inset_fraction),
                    raw_endpoint: Some(left),
                    portal_interior_xz: Some(left_interior),
                },
                right: FunnelEndpoint {
                    point: right.lerp(left, inset_fraction),
                    raw_endpoint: Some(right),
                    portal_interior_xz: Some(-left_interior),
                },
            };
            (gate.left.point.is_finite()
                && gate.right.point.is_finite()
                && left_interior.is_finite())
            .then_some(gate)
        })
        .collect()
}

fn segment_point_distance_xz(start: Vec3, end: Vec3, point: Vec3) -> f32 {
    let segment = Vec3::new(end.x - start.x, 0.0, end.z - start.z);
    let to_point = Vec3::new(point.x - start.x, 0.0, point.z - start.z);
    let length_squared = segment.length_squared();
    if length_squared <= f32::EPSILON {
        return to_point.length();
    }
    let t = (to_point.dot(segment) / length_squared).clamp(0.0, 1.0);
    (to_point - segment * t).length()
}

/// The offset point that pushes a chord approaching `corner.point` from `toward`
/// off the raw endpoint's clearance disk, tangent on the portal-side leg.
///
/// Geometry: `portal_interior` is the unit vector along the portal from the raw
/// endpoint into the corridor, so `left_perpendicular` (`portal_interior` rotated
/// 90° in XZ) is the portal-normal axis — the direction that moves off the wall
/// rather than along the gate. `toward_corner` points from the raw endpoint to the
/// incoming chord's far vertex; its dot with the normal picks the sign, so
/// `corridor_side` always faces the corridor interior the chord came from. At the
/// exact perpendicular tie (`dot == 0`) the `>= 0.0` branch picks the `+left`
/// side arbitrarily; the tie is measure-zero, so either side is clearance-safe.
/// The emitted point sits `clearance_radius` off `corner.point` (itself already
/// `clearance_radius` off the raw endpoint along the gate), so the `corner.point ->
/// point` leg runs tangent to the disk at `corner.point`.
fn bevel_point(corner: FunnelEndpoint, toward: Vec3, clearance_radius: f32) -> Option<Vec3> {
    let raw_endpoint = corner.raw_endpoint?;
    let portal_interior = corner.portal_interior_xz?;

    let left_perpendicular = Vec3::new(-portal_interior.z, 0.0, portal_interior.x);
    let toward_corner = Vec3::new(toward.x - raw_endpoint.x, 0.0, toward.z - raw_endpoint.z);
    let corridor_side = if left_perpendicular.dot(toward_corner) >= 0.0 {
        left_perpendicular
    } else {
        -left_perpendicular
    };
    Some(corner.point + corridor_side * clearance_radius)
}

/// `bevel_point`, but only when the chord from `toward` to `corner.point` actually
/// cuts the raw endpoint's clearance disk. Returns `None` when that leg already
/// clears — routed through `corner.point` directly, no bevel needed. Shares
/// `CLEARANCE_EPS` and the raw-endpoint reference with the caller's violation test
/// so a detected violation cannot fall into a "clear here" gap.
fn clearance_bevel(corner: FunnelEndpoint, toward: Vec3, clearance_radius: f32) -> Option<Vec3> {
    let raw_endpoint = corner.raw_endpoint?;
    if segment_point_distance_xz(toward, corner.point, raw_endpoint) + CLEARANCE_EPS
        >= clearance_radius
    {
        return None;
    }
    bevel_point(corner, toward, clearance_radius)
}

/// Slide a pinned vertex out of `obstacle`'s clearance disk along the portal
/// normal (the bevel axis), holding its along-gate coordinate fixed and moving it
/// perpendicular to the disk boundary. Used when a segment's immovable start
/// vertex lies inside a *foreign* endpoint's disk: no point appended after it can
/// pull that segment clear, so the vertex itself must move. Returns `None` when
/// the vertex has no perpendicular solution (its along-gate offset already exceeds
/// `clearance_radius`, i.e. it is not actually inside the disk).
fn route_out_of_disk(
    obstacle: FunnelEndpoint,
    vertex: Vec3,
    clearance_radius: f32,
) -> Option<Vec3> {
    let raw_endpoint = obstacle.raw_endpoint?;
    let portal_interior = obstacle.portal_interior_xz?;
    let normal = Vec3::new(-portal_interior.z, 0.0, portal_interior.x);

    let offset = Vec3::new(vertex.x - raw_endpoint.x, 0.0, vertex.z - raw_endpoint.z);
    let along = offset.dot(portal_interior);
    let perp = offset.dot(normal);
    let remaining = clearance_radius * clearance_radius - along * along;
    if remaining <= 0.0 {
        return None;
    }
    // Keep the along-gate coordinate; push perpendicular to the disk boundary on
    // the side the vertex already sits, preserving its Y.
    let target_perp = remaining
        .sqrt()
        .copysign(if perp >= 0.0 { 1.0 } else { -1.0 });
    Some(Vec3::new(
        raw_endpoint.x + portal_interior.x * along + normal.x * target_perp,
        vertex.y,
        raw_endpoint.z + portal_interior.z * along + normal.z * target_perp,
    ))
}

/// Squared XZ length below which a direction vector is treated as zero — no
/// stable direction can be normalized out of it. Matches the AI/steering guard.
const MIN_XZ_LEN_SQ: f32 = 1e-8;

/// Project an immovable terminal waypoint (the path's start `repaired[0]` or its
/// goal `repaired[last]`) radially onto `obstacle`'s clearance-disk boundary,
/// turning it into a walkable STANDOFF at the obstacle edge. The endpoint-snap
/// (`resolve_region_at`) admits a terminal sitting in the eroded wall band, which
/// can land inside a wide portal endpoint's disk; no point inserted around a
/// terminal can pull the chord's fixed far vertex clear, so the terminal itself
/// must move.
///
/// Unlike [`route_out_of_disk`] — which holds a vertex's along-gate coordinate to
/// preserve a corridor crossing — a terminal has no corridor constraint, so the
/// nearest boundary point (the radial projection `raw + normalize_xz(T - raw) *
/// clearance_radius`) is the minimal move. Y is preserved. `toward` is the
/// adjacent waypoint, used only to pick a stable direction when the terminal sits
/// on the disk center in XZ; if that too coincides, fall back to the portal normal
/// (the bevel axis). The true entity target still lives on the agent's steering
/// `destination`, independent of this emitted waypoint.
///
/// The `raw_endpoint`/`portal_interior_xz` `?`s and the final `None` are
/// type-safety guards, not reachable outcomes for a FILTERED obstacle:
/// `inset_portals` sets `raw_endpoint` and `portal_interior_xz` together, and a
/// wide portal's interior is a unit vector, so the portal-normal fallback always
/// yields a finite direction — `None` is unreachable here in practice.
///
/// Overlapping-disk churn is bounded, not prevented. Baked portal endpoints are
/// cell-lattice-aligned (distinct centers >= `cell_size` apart); two clearance disks
/// overlap when their centers are < `2 * clearance` apart. The production defaults
/// (`cell_size` 0.25 m, `agent_radius` 0.4 m => `2 * clearance` 0.84 m) do NOT
/// separate the disks, so a terminal projected onto one disk boundary CAN land inside
/// a distinct overlapping disk and be re-projected. `ensure_endpoint_clearance`'s
/// repair budget bounds that churn to a clean `None` — never a spin, panic, or
/// grazing path — surfacing as the far-side pinch-gap re-freeze tracked in
/// `context/plans/drafts/E10--pursuit-wraparound-blocked`. Ping-pong is structurally
/// impossible only where `cell_size > 2 * clearance`.
fn project_out_of_disk(
    obstacle: FunnelEndpoint,
    terminal: Vec3,
    toward: Vec3,
    clearance_radius: f32,
) -> Option<Vec3> {
    let raw_endpoint = obstacle.raw_endpoint?;
    let radial = Vec3::new(
        terminal.x - raw_endpoint.x,
        0.0,
        terminal.z - raw_endpoint.z,
    );
    let direction = if radial.length_squared() > MIN_XZ_LEN_SQ {
        radial.normalize()
    } else {
        // Terminal sits on the disk center in XZ: no radial direction. Push
        // toward the adjacent waypoint so the standoff faces the corridor; if
        // that too coincides, use the portal normal (unit for a wide portal).
        let toward_xz = Vec3::new(toward.x - raw_endpoint.x, 0.0, toward.z - raw_endpoint.z);
        if toward_xz.length_squared() > MIN_XZ_LEN_SQ {
            toward_xz.normalize()
        } else {
            // Terminal AND adjacent both coincide with the raw endpoint in XZ: no
            // radial and no `toward` direction survive, so `bevel_point`'s
            // toward-based side pick is unavailable. Both `+/-normal` clear the disk
            // equally, so the fixed `+normal` sign is arbitrary but clearance-safe;
            // this branch is only reachable at a measure-zero triple-coincidence.
            let portal_interior = obstacle.portal_interior_xz?;
            let normal = Vec3::new(-portal_interior.z, 0.0, portal_interior.x);
            if normal.length_squared() > MIN_XZ_LEN_SQ {
                normal.normalize()
            } else {
                return None;
            }
        }
    };
    Some(Vec3::new(
        raw_endpoint.x + direction.x * clearance_radius,
        terminal.y,
        raw_endpoint.z + direction.z * clearance_radius,
    ))
}

#[derive(Clone, Copy)]
struct PathPoint {
    point: Vec3,
    mandatory: bool,
}

/// Repair every emitted chord against every wide-portal endpoint disk. The
/// funnel may cross a gate near an endpoint without emitting that endpoint, so
/// checking only committed funnel corners is insufficient. A repair routes via
/// the inset point and whichever square bevels its adjacent chords need. The
/// portal-side (`corner.point -> bevel`) leg is tangent to the disk by
/// construction; the incoming leg's clearance is not guaranteed here — it is
/// re-checked on the next loop iteration and beveled again if it still cuts.
fn ensure_endpoint_clearance(
    path: &[FunnelEndpoint],
    gates: &[FunnelGate],
    clearance_radius: f32,
) -> Option<NavPath> {
    let clearance_radius = clearance_radius.max(0.0);
    let obstacles: Vec<FunnelEndpoint> = gates
        .iter()
        .flat_map(|gate| [gate.left, gate.right])
        .filter(|endpoint| endpoint.raw_endpoint.is_some())
        .collect();
    let mut repaired: Vec<PathPoint> = path
        .iter()
        .map(|waypoint| PathPoint {
            point: waypoint.point,
            mandatory: false,
        })
        .collect();

    if clearance_radius > f32::EPSILON {
        let mut segment_index = 0;
        // Safety budget: each splice or terminal projection re-checks the same
        // segment without advancing `segment_index`, so a repair that never
        // converges would spin forever. A converging interior repair touches a given
        // obstacle a bounded number of times (route-out plus at most the
        // start/corner/end inserts). The two terminal-projection branches (start
        // `repaired[0]`, goal `repaired[last]`) cost one projection per terminal where
        // clearance disks do not overlap; under the production defaults they DO overlap
        // (see `project_out_of_disk`), so a terminal can ping-pong between disks — this
        // budget hard-bounds that case too. `4` per obstacle stays comfortable
        // headroom; exhausting it means the geometry is genuinely unroutable (a
        // sub-`2 * clearance` pinch or overlapping disks) and we bail cleanly to `None`.
        let mut repairs_remaining = obstacles.len().saturating_mul(4).max(1);
        while segment_index + 1 < repaired.len() {
            let start = repaired[segment_index].point;
            let end = repaired[segment_index + 1].point;
            let violated = obstacles.iter().copied().find(|obstacle| {
                let raw = obstacle.raw_endpoint.expect("filtered endpoint");
                segment_point_distance_xz(start, end, raw) + CLEARANCE_EPS < clearance_radius
            });
            let Some(obstacle) = violated else {
                segment_index += 1;
                continue;
            };
            if repairs_remaining == 0 {
                return None;
            }
            repairs_remaining -= 1;

            let raw = obstacle.raw_endpoint.expect("filtered endpoint");
            let last_index = repaired.len() - 1;

            // A vertex is immovable by appending: no point inserted after it can
            // pull the fixed near end of its chord out of `raw`'s disk, so a
            // vertex sitting inside the disk must itself move.
            //
            // Interior vertices (a zig-zag where one gate's inset corner lands
            // within clearance of a *distinct* neighbor endpoint) slide out along
            // the bevel axis, holding their along-gate coordinate to keep the
            // corridor crossing, then re-validate the feeding segment.
            //
            // Terminals cannot slide that way — they have no corridor crossing to
            // preserve — and are the agent's own start (`repaired[0]`) or the goal
            // (`repaired[last]`). The endpoint-snap admits a terminal in the eroded
            // wall band that can land inside a WIDE portal endpoint's disk; project
            // it radially onto the disk boundary, making the emitted terminal a
            // walkable STANDOFF at the obstacle edge. The true entity target still
            // lives on the agent's steering `destination`, so the first/last
            // waypoint no longer necessarily equals the raw start/goal.
            if distance_xz(start, raw) + CLEARANCE_EPS < clearance_radius {
                if segment_index == 0 {
                    let projected = project_out_of_disk(obstacle, start, end, clearance_radius)?;
                    repaired[0].point = projected;
                    repaired[0].mandatory = true;
                    continue;
                }
                match route_out_of_disk(obstacle, start, clearance_radius) {
                    Some(routed) => {
                        repaired[segment_index].point = routed;
                        repaired[segment_index].mandatory = true;
                        segment_index -= 1;
                        continue;
                    }
                    None => return None,
                }
            }
            // The goal terminal is the immovable FAR end of the final segment;
            // project it out the same way when it is snapped inside the disk.
            if segment_index + 1 == last_index
                && distance_xz(end, raw) + CLEARANCE_EPS < clearance_radius
            {
                let projected = project_out_of_disk(obstacle, end, start, clearance_radius)?;
                repaired[last_index].point = projected;
                repaired[last_index].mandatory = true;
                continue;
            }

            let corner = obstacle.point;
            let start_is_corner = start.abs_diff_eq(corner, CLEARANCE_EPS);
            let end_is_corner = end.abs_diff_eq(corner, CLEARANCE_EPS);
            let mut inserts = Vec::with_capacity(3);
            if start_is_corner {
                repaired[segment_index].mandatory = true;
            } else if let Some(bevel) = clearance_bevel(obstacle, start, clearance_radius) {
                inserts.push(PathPoint {
                    point: bevel,
                    mandatory: true,
                });
            }
            if !start_is_corner && !end_is_corner {
                inserts.push(PathPoint {
                    point: corner,
                    mandatory: true,
                });
            }
            if end_is_corner {
                repaired[segment_index + 1].mandatory = true;
            } else if let Some(bevel) = clearance_bevel(obstacle, end, clearance_radius) {
                inserts.push(PathPoint {
                    point: bevel,
                    mandatory: true,
                });
            }

            if inserts.is_empty() {
                // The violation is real, but both endpoints' bevel tests judged
                // their leg already clear (an endpoint sits exactly `corner` while
                // the opposite leg is a hair over clearance). The incursion still
                // needs routing, so force the movable endpoint's bevel rather than
                // dropping a routable corridor to `None`. `bevel_point` yields
                // `Some` for a filtered obstacle (both endpoint fields present).
                let toward = if start_is_corner { end } else { start };
                let bevel = bevel_point(obstacle, toward, clearance_radius)?;
                inserts.push(PathPoint {
                    point: bevel,
                    mandatory: true,
                });
            }
            repaired.splice(segment_index + 1..segment_index + 1, inserts);
        }
    }

    Some(NavPath {
        points: repaired.iter().map(|waypoint| waypoint.point).collect(),
        mandatory_waypoints: repaired.iter().map(|waypoint| waypoint.mandatory).collect(),
    })
}

/// Simple Stupid Funnel string-pull over an ordered list of traversal-oriented
/// `FunnelGate`s (each carrying its clearance-inset `left`/`right`
/// `FunnelEndpoint`). Emits the tightest `FunnelEndpoint` list from `start` to
/// `goal` that stays within the corridor. The first waypoint is `start`, the last
/// is `goal`; a straight corridor collapses to `[start, goal]`.
///
/// Only the `point` of each emitted endpoint is load-bearing downstream: the
/// clearance repair rebuilds its obstacle disks from `gates`, not from the
/// raw-endpoint/interior metadata that rides along on committed apexes.
fn funnel(start: Vec3, goal: Vec3, portals: &[FunnelGate]) -> Vec<FunnelEndpoint> {
    let start_endpoint = FunnelEndpoint::terminal(start);
    let mut path = vec![start_endpoint];

    let mut apex = start;
    let mut left = start_endpoint;
    let mut right = start_endpoint;
    let mut left_index = 0usize;
    let mut right_index = 0usize;

    // Append the goal as a degenerate final portal so the funnel pulls all the
    // way to it with the same logic as any interior gate.
    let mut gates: Vec<FunnelGate> = Vec::with_capacity(portals.len() + 1);
    gates.extend_from_slice(portals);
    let goal_endpoint = FunnelEndpoint::terminal(goal);
    gates.push(FunnelGate {
        left: goal_endpoint,
        right: goal_endpoint,
    });

    let mut i = 0;
    while i < gates.len() {
        let gate_left = gates[i].left;
        let gate_right = gates[i].right;

        // Tighten the right side.
        if triangle_area_xz(apex, right.point, gate_right.point) <= 0.0 {
            if apex == right.point || triangle_area_xz(apex, left.point, gate_right.point) > 0.0 {
                // Still inside the funnel — narrow the right edge.
                right = gate_right;
                right_index = i;
            } else {
                // Right over left: the left vertex becomes a new apex/corner.
                let new_apex = left;
                path.push(new_apex);
                apex = new_apex.point;
                // Restart the funnel from the vertex after the new apex.
                left_index += 1;
                right_index = left_index;
                left = new_apex;
                right = new_apex;
                i = left_index;
                continue;
            }
        }

        // Tighten the left side.
        if triangle_area_xz(apex, left.point, gate_left.point) >= 0.0 {
            if apex == left.point || triangle_area_xz(apex, right.point, gate_left.point) < 0.0 {
                left = gate_left;
                left_index = i;
            } else {
                let new_apex = right;
                path.push(new_apex);
                apex = new_apex.point;
                right_index += 1;
                left_index = right_index;
                left = new_apex;
                right = new_apex;
                i = right_index;
                continue;
            }
        }

        i += 1;
    }

    // Always end on the goal (the degenerate last gate guarantees reachability,
    // but the apex may already sit at goal if the corridor pulled straight).
    if path.last().expect("path starts with `start`").point != goal {
        path.push(goal_endpoint);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::navmesh::{NAVMESH_VERSION, NavMeshSection, NavPortal, NavRegion};

    const EPS: f32 = 1e-4;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() <= EPS
    }

    fn approx_xz(a: Vec3, b: Vec3) -> bool {
        approx_eq(a.x, b.x) && approx_eq(a.z, b.z)
    }

    /// Base section header shared by the hand-built fixtures (origin at world
    /// zero, unit cells), with caller-supplied regions and portals.
    fn section(regions: Vec<NavRegion>, portals: Vec<NavPortal>) -> NavMeshSection {
        NavMeshSection {
            version: NAVMESH_VERSION,
            origin: [0.0, 0.0, 0.0],
            cell_size: 1.0,
            dim_x: 64,
            dim_z: 64,
            agent_radius: 0.3,
            agent_height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
            regions,
            portals,
        }
    }

    fn region(x0: u32, z0: u32, x1: u32, z1: u32) -> NavRegion {
        NavRegion {
            x0,
            z0,
            x1,
            z1,
            floor_y_min: 0.0,
            floor_y_max: 0.5,
        }
    }

    /// Three regions stacked along +Z, each [0,4) wide, joined end to end by two
    /// full-width portals. A straight corridor.
    fn straight_corridor_section() -> NavMeshSection {
        section(
            vec![region(0, 0, 4, 4), region(0, 4, 4, 8), region(0, 8, 4, 12)],
            vec![
                NavPortal {
                    region_a: 0,
                    region_b: 1,
                    left: [0.0, 0.0, 4.0],
                    right: [4.0, 0.0, 4.0],
                },
                NavPortal {
                    region_a: 1,
                    region_b: 2,
                    left: [0.0, 0.0, 8.0],
                    right: [4.0, 0.0, 8.0],
                },
            ],
        )
    }

    #[test]
    fn find_path_returns_none_when_start_outside_all_regions() {
        let graph = NavGraph::from_section(&straight_corridor_section());
        let path = find_path(
            &graph,
            Vec3::new(100.0, 0.0, 100.0),
            Vec3::new(2.0, 0.0, 2.0),
        );
        assert!(path.is_none());
    }

    #[test]
    fn find_path_returns_none_when_goal_outside_all_regions() {
        let graph = NavGraph::from_section(&straight_corridor_section());
        let path = find_path(
            &graph,
            Vec3::new(2.0, 0.0, 2.0),
            Vec3::new(100.0, 0.0, 100.0),
        );
        assert!(path.is_none());
    }

    #[test]
    fn find_path_snaps_eroded_band_endpoints_onto_the_graph() {
        // Both endpoints sit just OUTSIDE every region (the eroded wall margin a
        // capsule can legitimately occupy) but within the snap tolerance
        // (radius 0.3 + 1.5 * cell 1.0 = 1.8). The query must resolve each to
        // its nearest region and route, keeping the RAW endpoint positions.
        let graph = NavGraph::from_section(&straight_corridor_section());
        let start = Vec3::new(2.0, 0.0, -0.4); // 0.4 below region 0
        let goal = Vec3::new(2.0, 0.0, 12.4); // 0.4 past region 2
        assert!(graph.region_at(start).is_none() && graph.region_at(goal).is_none());

        let path = find_path(&graph, start, goal).expect("eroded-band endpoints must route");
        assert!(approx_xz(path[0], start), "raw start preserved: {path:?}");
        assert!(
            approx_xz(*path.last().unwrap(), goal),
            "raw goal preserved: {path:?}"
        );
    }

    #[test]
    fn find_path_still_rejects_endpoints_far_off_the_mesh() {
        let graph = NavGraph::from_section(&straight_corridor_section());
        // 3.0 beyond the last region: past the 1.8 snap tolerance.
        assert!(find_path(&graph, Vec3::new(2.0, 0.0, 1.0), Vec3::new(2.0, 0.0, 15.0)).is_none());
        assert!(find_path(&graph, Vec3::new(2.0, 0.0, -3.0), Vec3::new(2.0, 0.0, 1.0)).is_none());
    }

    #[test]
    fn find_path_returns_direct_two_points_when_goal_in_start_region() {
        let graph = NavGraph::from_section(&straight_corridor_section());
        let start = Vec3::new(1.0, 0.0, 1.0);
        let goal = Vec3::new(3.0, 0.0, 3.0);
        let path = find_path(&graph, start, goal).expect("same-region path exists");
        assert_eq!(path.len(), 2);
        assert!(approx_xz(path[0], start));
        assert!(approx_xz(path[1], goal));
    }

    #[test]
    fn find_path_returns_none_when_goal_region_unreachable() {
        // Two regions with NO portal between them: disconnected graph.
        let graph = NavGraph::from_section(&section(
            vec![region(0, 0, 4, 4), region(0, 8, 4, 12)],
            vec![],
        ));
        let path = find_path(&graph, Vec3::new(2.0, 0.0, 2.0), Vec3::new(2.0, 0.0, 10.0));
        assert!(path.is_none());
    }

    #[test]
    fn find_path_collapses_straight_corridor_to_two_points() {
        let graph = NavGraph::from_section(&straight_corridor_section());
        let start = Vec3::new(2.0, 0.0, 1.0);
        let goal = Vec3::new(2.0, 0.0, 11.0);
        let path = find_path(&graph, start, goal).expect("connected corridor");
        // Start and goal share an X; the funnel pulls a straight line.
        assert_eq!(path.len(), 2, "straight corridor should not bend: {path:?}");
        assert!(approx_xz(path[0], start));
        assert!(approx_xz(path[1], goal));
    }

    /// L-shaped corridor: region 0 at the bottom, region 1 above it, region 2 to
    /// the +X side of region 1. The inner corner sits where the two portals meet.
    ///
    ///   region 1 [0,4) x [4,8)   ── portal 1-2 at x=4 ──  region 2 [4,8) x [4,8)
    ///        │
    ///   portal 0-1 at z=4
    ///        │
    ///   region 0 [0,4) x [0,4)
    fn l_corridor_section() -> NavMeshSection {
        section(
            vec![region(0, 0, 4, 4), region(0, 4, 4, 8), region(4, 4, 8, 8)],
            vec![
                // Portal 0<->1 spans z=4, x in [0,4].
                NavPortal {
                    region_a: 0,
                    region_b: 1,
                    left: [0.0, 0.0, 4.0],
                    right: [4.0, 0.0, 4.0],
                },
                // Portal 1<->2 spans x=4, z in [4,8].
                NavPortal {
                    region_a: 1,
                    region_b: 2,
                    left: [4.0, 0.0, 8.0],
                    right: [4.0, 0.0, 4.0],
                },
            ],
        )
    }

    #[test]
    fn find_path_bends_l_corridor_at_inset_corner_clearing_every_segment() {
        let graph = NavGraph::from_section(&l_corridor_section());
        // Start low in region 0, goal in region 2 (+X side). Start and goal are
        // chosen so the straight segment would exit the corridor at the z=4
        // portal (x would reach 5.5 > 4), forcing the funnel to bend.
        let start = Vec3::new(1.0, 0.0, 1.0);
        let goal = Vec3::new(7.0, 0.0, 5.0);
        let path = find_path(&graph, start, goal).expect("L corridor connects");

        // The inner corner is the shared endpoint of the two portals at (4,*,4).
        let inner_corner = Vec3::new(4.0, 0.0, 4.0);
        assert!(
            path.len() >= 3,
            "an L-bend must introduce at least one interior waypoint: {path:?}"
        );
        assert!(approx_xz(path[0], start));
        assert!(approx_xz(*path.last().unwrap(), goal));

        // Two independent, complementary guarantees on the same path. First: the
        // bend lands at the exact inner-corner endpoint inset by the canonical
        // agent radius plus collision skin, not the raw wall corner — a specific
        // location the general clearance test below cannot pin (a wide detour
        // would also clear the disk). Second: no segment cuts the corner's
        // clearance disk anywhere along its length — a universal property the
        // single exact-bend waypoint cannot certify (a segment can touch the disk
        // boundary at the bend yet dip closer elsewhere).
        let inset_corner = Vec3::new(4.0, 0.0, 4.0 + graph.agent.radius + SKIN_DISTANCE);
        let bends_at_inset_corner = path[1..path.len() - 1]
            .iter()
            .any(|w| approx_xz(*w, inset_corner));
        assert!(
            bends_at_inset_corner,
            "expected a bend inset from the inner corner {inner_corner:?}, got {path:?}"
        );

        let effective_clearance = graph.agent.radius + SKIN_DISTANCE;
        for segment in path.windows(2) {
            let clearance = segment_point_distance_xz(segment[0], segment[1], inner_corner);
            assert!(
                clearance + EPS >= effective_clearance,
                "segment {segment:?} cuts the corner disk: clearance={clearance}, path={path:?}"
            );
        }
    }

    // Raw wide-portal endpoints of `l_corridor_section`: the two ends of the z=4
    // portal and the far end of the x=4 portal. `(4,*,4)` is shared by both.
    const L_CORRIDOR_ENDPOINTS: [Vec3; 3] = [
        Vec3::new(0.0, 0.0, 4.0),
        Vec3::new(4.0, 0.0, 4.0),
        Vec3::new(4.0, 0.0, 8.0),
    ];

    #[test]
    fn find_path_projects_start_inside_wide_endpoint_disk_to_a_standoff() {
        // R1: the endpoint-snap admits a terminal inside a WIDE portal endpoint's
        // clearance disk (a start hugging a doorway jamb). The agent's own start
        // (`repaired[0]`) cannot be pulled clear by appending points — it is the
        // chord's fixed near end — so the pre-fix repair churned its budget and
        // dropped a routable corridor to `None`, re-freezing the chaser. The fix
        // projects the start onto the disk boundary — a walkable standoff — and
        // still routes.
        let graph = NavGraph::from_section(&l_corridor_section());
        let clearance = graph.agent.radius + SKIN_DISTANCE;
        // `start` hugs the (0,*,4) jamb from the region-0 side, 0.224 m off it —
        // inside the 0.32 m disk. Its offset points into the wide opening, so the
        // funnel crosses both gates interior (no bend) and the standoff routes
        // straight on. (A pre-fix run churns this exact fixture to `None`.)
        let jamb = Vec3::new(0.0, 0.0, 4.0);
        let start = Vec3::new(0.2, 0.0, 3.9);
        let goal = Vec3::new(7.0, 0.0, 5.0);
        assert_eq!(graph.region_at(start), Some(0), "start is in region 0");
        assert!(
            distance_xz(start, jamb) < clearance,
            "fixture must place the start inside the endpoint disk"
        );

        let path =
            find_path(&graph, start, goal).expect("start-in-disk corridor must not drop to None");

        // The emitted first waypoint is a projected standoff, no longer the raw
        // start, and it clears the endpoint disk by the effective clearance.
        assert!(
            !approx_xz(path[0], start),
            "start terminal must be projected off the raw position: {path:?}"
        );
        assert!(
            distance_xz(path[0], jamb) + EPS >= clearance,
            "projected start must clear the endpoint disk: {path:?}"
        );
        assert!(
            approx_xz(*path.last().unwrap(), goal),
            "clean goal preserved: {path:?}"
        );

        // Interior clearance still holds: no segment cuts any wide-portal endpoint.
        for segment in path.windows(2) {
            for raw in L_CORRIDOR_ENDPOINTS {
                assert!(
                    segment_point_distance_xz(segment[0], segment[1], raw) + EPS >= clearance,
                    "segment {segment:?} cuts endpoint {raw:?}; path={path:?}"
                );
            }
        }
    }

    #[test]
    fn find_path_projects_goal_inside_wide_endpoint_disk_to_a_standoff() {
        // R1, goal side: the goal terminal is the immovable FAR end of the final
        // segment, which the pre-fix repair also could not move — a goal inside a
        // wide endpoint disk churned to `None` too. The fix projects it to a
        // boundary standoff.
        let graph = NavGraph::from_section(&l_corridor_section());
        let clearance = graph.agent.radius + SKIN_DISTANCE;
        // `goal` sits 0.224 m off the (4,*,8) endpoint inside region 2, so the
        // straight corridor crosses both gates interior and the final segment
        // approaches the endpoint from one side — the standoff clears without a
        // disk-cutting bend.
        let endpoint = Vec3::new(4.0, 0.0, 8.0);
        let start = Vec3::new(1.0, 0.0, 1.0);
        let goal = Vec3::new(4.1, 0.0, 7.8);
        assert_eq!(graph.region_at(goal), Some(2), "goal is in region 2");
        assert!(
            distance_xz(goal, endpoint) < clearance,
            "fixture must place the goal inside the endpoint disk"
        );

        let path =
            find_path(&graph, start, goal).expect("goal-in-disk corridor must not drop to None");

        assert!(approx_xz(path[0], start), "clean start preserved: {path:?}");
        assert!(
            !approx_xz(*path.last().unwrap(), goal),
            "goal terminal must be projected off the raw position: {path:?}"
        );
        assert!(
            distance_xz(*path.last().unwrap(), endpoint) + EPS >= clearance,
            "projected goal must clear the endpoint disk: {path:?}"
        );

        for segment in path.windows(2) {
            for raw in L_CORRIDOR_ENDPOINTS {
                assert!(
                    segment_point_distance_xz(segment[0], segment[1], raw) + EPS >= clearance,
                    "segment {segment:?} cuts endpoint {raw:?}; path={path:?}"
                );
            }
        }
    }

    #[test]
    fn find_path_single_region_routes_start_to_goal_directly() {
        let graph = NavGraph::from_section(&section(vec![region(0, 0, 8, 8)], vec![]));
        let start = Vec3::new(1.0, 0.0, 1.0);
        let goal = Vec3::new(6.0, 0.0, 6.0);
        let path = find_path(&graph, start, goal).expect("single region path");
        assert_eq!(path.len(), 2);
        assert!(approx_xz(path[0], start));
        assert!(approx_xz(path[1], goal));
    }

    #[test]
    fn find_path_handles_reversed_portal_traversal_via_left_right_swap() {
        // Corridor whose region indices descend along the path of travel, so at
        // least one portal is crossed region_b -> region_a (reversed). Regions
        // are laid out so the natural route is region 2 -> region 1 -> region 0
        // along -Z, but we still travel from the higher-index region to the
        // lower. Build an L so handedness matters: a wrong swap would fail to
        // bend at the inner corner.
        //
        //   region 0 [4,8) x [4,8)  ── portal 0-1 at x=4 ── region 1 [0,4) x [4,8)
        //                                                          │
        //                                                  portal 1-2 at z=4
        //                                                          │
        //                                                   region 2 [0,4) x [0,4)
        let graph = NavGraph::from_section(&section(
            vec![region(4, 4, 8, 8), region(0, 4, 4, 8), region(0, 0, 4, 4)],
            vec![
                // Portal 0<->1 spans x=4, z in [4,8].
                NavPortal {
                    region_a: 0,
                    region_b: 1,
                    left: [4.0, 0.0, 4.0],
                    right: [4.0, 0.0, 8.0],
                },
                // Portal 1<->2 spans z=4, x in [0,4].
                NavPortal {
                    region_a: 1,
                    region_b: 2,
                    left: [4.0, 0.0, 4.0],
                    right: [0.0, 0.0, 4.0],
                },
            ],
        ));
        // Travel from region 2 (low) up to region 0 (+X), crossing both portals
        // in the region_b -> region_a direction. Goal at z=5 forces a bend (the
        // straight segment would exit the corridor at the z=4 portal).
        let start = Vec3::new(1.0, 0.0, 1.0); // region 2
        let goal = Vec3::new(7.0, 0.0, 5.0); // region 0
        let path = find_path(&graph, start, goal).expect("reversed corridor connects");

        assert!(approx_xz(path[0], start));
        assert!(approx_xz(*path.last().unwrap(), goal));
        let inset_corner = Vec3::new(4.0, 0.0, 4.0 + graph.agent.radius + SKIN_DISTANCE);
        let bends_at_inset_corner = path[1..path.len() - 1]
            .iter()
            .any(|w| approx_xz(*w, inset_corner));
        assert!(
            bends_at_inset_corner,
            "reversed L corridor must still bend inset from its inner corner {inset_corner:?}, got {path:?}"
        );
    }

    #[test]
    fn funnel_uses_midpoint_for_equal_effective_diameter_with_unequal_endpoint_y() {
        let endpoint = Vec3::new(4.0, 1.0, 4.0);
        let opposite_endpoint = Vec3::new(4.74, 3.0, 4.0);

        let gates = inset_portals(&[(endpoint, opposite_endpoint)], 0.37).unwrap();
        let gate = gates[0];

        // Regression: 3D width treated this stepped portal as wide even though
        // its horizontal width is exactly the inclusive midpoint threshold.
        let midpoint = Vec3::new(4.37, 2.0, 4.0);
        assert!(gate.left.point.is_finite() && gate.right.point.is_finite());
        assert!(gate.left.point.abs_diff_eq(midpoint, EPS));
        assert!(gate.right.point.abs_diff_eq(midpoint, EPS));
    }

    #[test]
    fn funnel_insets_wide_unequal_y_portal_by_horizontal_fraction() {
        let left = Vec3::new(0.0, 1.0, 4.0);
        let right = Vec3::new(2.0, 3.0, 4.0);

        let gates = inset_portals(&[(left, right)], 0.4).unwrap();
        let gate = gates[0];

        assert!(gate.left.point.abs_diff_eq(Vec3::new(0.4, 1.4, 4.0), EPS));
        assert!(gate.right.point.abs_diff_eq(Vec3::new(1.6, 2.6, 4.0), EPS));
    }

    #[test]
    fn find_path_repairs_oblique_straight_chord_near_raw_portal_endpoint() {
        let mut navmesh = section(
            vec![region(0, 0, 4, 4), region(0, 4, 4, 8)],
            vec![NavPortal {
                region_a: 0,
                region_b: 1,
                left: [0.0, 0.0, 4.0],
                right: [4.0, 0.0, 4.0],
            }],
        );
        navmesh.agent_radius = 0.35;
        let graph = NavGraph::from_section(&navmesh);
        let raw_endpoint = Vec3::new(0.0, 0.0, 4.0);
        let clearance = graph.agent.radius + SKIN_DISTANCE;
        let start = Vec3::new(0.0, 0.0, 3.0);
        let goal = Vec3::new(0.74, 0.0, 5.0);

        assert!(
            segment_point_distance_xz(start, goal, raw_endpoint) < clearance,
            "fixture must reproduce the oblique straight-chord failure"
        );
        let path = find_path(&graph, start, goal).expect("connected corridor");
        assert!(
            path.len() > 2,
            "unsafe straight chord must be repaired: {path:?}"
        );
        assert!(path.mandatory_waypoints.iter().any(|mandatory| *mandatory));
        for segment in path.windows(2) {
            assert!(
                segment_point_distance_xz(segment[0], segment[1], raw_endpoint) + EPS >= clearance,
                "repaired segment {segment:?} cuts endpoint disk; path={path:?}"
            );
        }
    }

    #[test]
    fn find_path_rejects_finite_portal_endpoints_whose_delta_overflows() {
        let graph = NavGraph::from_section(&section(
            vec![region(0, 0, 4, 4), region(0, 4, 4, 8)],
            vec![NavPortal {
                region_a: 0,
                region_b: 1,
                left: [f32::MAX, 0.0, 4.0],
                right: [-f32::MAX, 0.0, 4.0],
            }],
        ));

        // Regression: finite endpoint subtraction overflowed while insetting,
        // and NaN gates collapsed the funnel to a bad direct route.
        let path = find_path(&graph, Vec3::new(1.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 6.0));
        assert!(path.is_none());
    }

    #[test]
    fn find_path_keeps_every_segment_crossing_inside_preinset_portals() {
        let mut navmesh = section(
            vec![
                region(7, 0, 9, 1),
                region(0, 1, 8, 2),
                region(0, 2, 6, 3),
                region(3, 3, 5, 5),
            ],
            vec![
                NavPortal {
                    region_a: 0,
                    region_b: 1,
                    left: [7.0, 0.0, 1.0],
                    right: [8.0, 0.0, 1.0],
                },
                NavPortal {
                    region_a: 1,
                    region_b: 2,
                    left: [0.0, 0.0, 2.0],
                    right: [6.0, 0.0, 2.0],
                },
                NavPortal {
                    region_a: 2,
                    region_b: 3,
                    left: [3.0, 0.0, 3.0],
                    right: [5.0, 0.0, 3.0],
                },
            ],
        );
        navmesh.agent_radius = 0.35;
        let graph = NavGraph::from_section(&navmesh);
        let start = Vec3::new(8.0, 0.0, 0.2);
        let goal = Vec3::new(4.0, 0.0, 4.0);
        let path = find_path(&graph, start, goal).expect("four-region corridor connects");
        let inset = graph.agent.radius + SKIN_DISTANCE;

        // Regression: the returned corner was inset while the funnel apex stayed
        // raw, letting a later segment cross the first portal outside its gate.
        for (portal_z, raw_min_x, raw_max_x) in [(1.0, 7.0, 8.0), (2.0, 0.0, 6.0), (3.0, 3.0, 5.0)]
        {
            let crossing = path.windows(2).find_map(|segment| {
                let dz = segment[1].z - segment[0].z;
                if dz.abs() <= EPS {
                    return None;
                }
                let t = (portal_z - segment[0].z) / dz;
                (t >= -EPS && t <= 1.0 + EPS)
                    .then_some(segment[0].x + t * (segment[1].x - segment[0].x))
            });
            let crossing_x = crossing.expect("path must cross each corridor portal");
            assert!(
                crossing_x + EPS >= raw_min_x + inset && crossing_x - EPS <= raw_max_x - inset,
                "portal z={portal_z} crossing x={crossing_x} leaves inset range [{}, {}], path={path:?}",
                raw_min_x + inset,
                raw_max_x - inset,
            );
        }
    }

    #[test]
    fn find_path_follows_cheaper_of_two_doorways_between_same_region_pair() {
        // Region 0 and region 1 are joined by TWO distinct portals at different X
        // offsets. A* must select the cheaper doorway (by centroid/midpoint
        // metric) and the funnel must pull through THAT doorway — the one A*
        // costed — not whichever appears first in the portal array.
        //
        // region 0 [4,8) x [0,4) → centroid (6,2); region 1 [0,8) x [4,8) →
        // centroid (4,6). The near doorway [6,8] (mid (7,4)) costs ~5.8 by the
        // centroid→mid→centroid metric; the far doorway [0,2] (mid (1,4)) ~9.0.
        // Region 0's centroid sits near the near doorway, so A* picks it.
        let graph = NavGraph::from_section(&section(
            vec![region(4, 0, 8, 4), region(0, 4, 8, 8)],
            vec![
                // Doorway near x=1 (the FAR / costlier one).
                NavPortal {
                    region_a: 0,
                    region_b: 1,
                    left: [0.0, 0.0, 4.0],
                    right: [2.0, 0.0, 4.0],
                },
                // Doorway near x=7 (the CHEAP one A* should select).
                NavPortal {
                    region_a: 0,
                    region_b: 1,
                    left: [6.0, 0.0, 4.0],
                    right: [8.0, 0.0, 4.0],
                },
            ],
        ));
        let start = Vec3::new(6.0, 0.0, 1.0);
        let goal = Vec3::new(6.0, 0.0, 7.0);
        let path = find_path(&graph, start, goal).expect("two-doorway corridor connects");

        // Both start and goal sit at x=6. Routing through the far doorway [0,2]
        // would force an interior waypoint at x <= 2; the cheap doorway [6,8]
        // lets the funnel stay near x=6. A first-match portal pick (which would
        // grab the far doorway, index 0) would string-pull through x<=2.
        let detours_through_far_door = path.iter().any(|w| w.x <= 2.0 + EPS);
        assert!(
            !detours_through_far_door,
            "funnel must follow the cheaper doorway A* selected (near x=6), not the far one: {path:?}"
        );
        assert!(approx_xz(path[0], start));
        assert!(approx_xz(*path.last().unwrap(), goal));
    }

    #[test]
    fn find_path_exits_the_doorway_nearest_the_start_from_one_large_region() {
        // AC6: one large region offers TWO doorways toward the goal region. Each
        // agent must leave through the doorway beside where it actually stands —
        // the true-start-anchored A*'s discriminating case. The old
        // centroid-anchored cost charged both starts from the same region center,
        // so it would send one of them across the room to the wrong doorway.
        //
        // region 0 (large room) [0,8) x [0,4); region 1 (goal room) [0,8) x [4,8).
        // Two doorways at z=4: door A over x in [0,2], door B over x in [6,8].
        let make_graph = || {
            NavGraph::from_section(&section(
                vec![region(0, 0, 8, 4), region(0, 4, 8, 8)],
                vec![
                    // Door A (near x=1).
                    NavPortal {
                        region_a: 0,
                        region_b: 1,
                        left: [0.0, 0.0, 4.0],
                        right: [2.0, 0.0, 4.0],
                    },
                    // Door B (near x=7).
                    NavPortal {
                        region_a: 0,
                        region_b: 1,
                        left: [6.0, 0.0, 4.0],
                        right: [8.0, 0.0, 4.0],
                    },
                ],
            ))
        };
        let goal = Vec3::new(4.0, 0.0, 6.0); // centered in the goal room

        // Where does the path cross the z=4 doorway line?
        let crossing_x = |path: &NavPath| -> f32 {
            path.windows(2)
                .find_map(|seg| {
                    let dz = seg[1].z - seg[0].z;
                    if dz.abs() <= EPS {
                        return None;
                    }
                    let t = (4.0 - seg[0].z) / dz;
                    (t >= -EPS && t <= 1.0 + EPS)
                        .then_some(seg[0].x + t * (seg[1].x - seg[0].x))
                })
                .expect("path must cross the z=4 doorway line")
        };

        // Start beside door A → exit through door A (x in [0,2] band).
        let graph = make_graph();
        let path_a = find_path(&graph, Vec3::new(1.0, 0.0, 2.0), goal).expect("routes via door A");
        let xa = crossing_x(&path_a);
        assert!(
            xa <= 2.0 + EPS,
            "agent beside door A must exit through it (x<=2), crossed at x={xa}: {path_a:?}"
        );

        // Start beside door B → exit through door B (x in [6,8] band). The
        // centroid metric would send this one through the same door as start A.
        let path_b = find_path(&graph, Vec3::new(7.0, 0.0, 2.0), goal).expect("routes via door B");
        let xb = crossing_x(&path_b);
        assert!(
            xb >= 6.0 - EPS,
            "agent beside door B must exit through it (x>=6), crossed at x={xb}: {path_b:?}"
        );
    }

    #[test]
    fn find_path_routes_bent_twin_portal_chicane_without_dropping_to_none() {
        // Guards `ensure_endpoint_clearance` against dropping a routable bent
        // twin-portal chicane to `None`. The funnel's naive chord grazes a raw
        // endpoint disk; the repair must bevel it clear and still return `Some`
        // with every segment clearing every wide-portal raw endpoint.
        //
        // Fixture gap size is load-bearing. The repair offsets bevels along the
        // portal normal (axis-aligned), so it can only thread a pinch gap that is
        // comfortably wider than `2 * clearance` — an axis-aligned route exists
        // there with margin. The two convex corners are set ~0.94 apart against a
        // 0.64 (`2 * clearance`) minimum, a ~0.30 m safe gap. That is wide enough
        // for a clean bevel route yet tight enough that the naive chord still cuts
        // corner_b's disk (~0.077 m of incursion), so the repair genuinely fires
        // rather than the corridor collapsing to a trivial straight `[start,
        // goal]`. A gap only marginally above `2 * clearance` (e.g. ~0.05 m) has
        // no axis-aligned threading — the sole clear route is a segment tilted off
        // the normal axis, which this repair does not emit — so shrinking the gap
        // would make the drop a genuine geometric limit, not a regression.
        //
        // Regions are integer cells for `region_at`; the portal geometry is float.
        let mut navmesh = section(
            vec![region(0, 0, 6, 2), region(0, 2, 6, 3), region(0, 3, 6, 6)],
            vec![
                NavPortal {
                    region_a: 0,
                    region_b: 1,
                    left: [0.0, 0.0, 2.0],
                    right: [3.2, 0.0, 2.0],
                },
                NavPortal {
                    region_a: 1,
                    region_b: 2,
                    left: [2.4, 0.0, 2.5],
                    right: [6.0, 0.0, 2.5],
                },
            ],
        );
        navmesh.agent_radius = 0.3;
        let graph = NavGraph::from_section(&navmesh);
        let clearance = graph.agent.radius + SKIN_DISTANCE;

        // The two convex pinch corners are distinct and comfortably more than
        // 2*clearance apart — a real safe gap the axis-aligned repair can thread.
        let corner_a = Vec3::new(3.2, 0.0, 2.0);
        let corner_b = Vec3::new(2.4, 0.0, 2.5);
        assert!(distance_xz(corner_a, corner_b) > 2.0 * clearance + 0.25);

        let start = Vec3::new(1.0, 0.0, 0.5);
        let goal = Vec3::new(4.0, 0.0, 4.0);
        let path =
            find_path(&graph, start, goal).expect("routable twin-portal chicane must not drop");

        assert!(approx_xz(path[0], start));
        assert!(approx_xz(*path.last().unwrap(), goal));
        // The repair must genuinely fire: the naive funnel chord cuts corner_b's
        // disk, so the returned path bends at a clearance-mandated waypoint rather
        // than collapsing to a straight `[start, goal]`.
        assert!(
            path.len() > 2 && path.mandatory_waypoints.iter().any(|mandatory| *mandatory),
            "endpoint-clearance repair must insert a mandatory bend: {path:?}"
        );

        // Guarantee: every returned segment clears every wide-portal raw endpoint
        // by the effective clearance, within test epsilon.
        let raw_endpoints = [
            Vec3::new(0.0, 0.0, 2.0),
            corner_a,
            corner_b,
            Vec3::new(6.0, 0.0, 2.5),
        ];
        for segment in path.windows(2) {
            for raw in raw_endpoints {
                assert!(
                    segment_point_distance_xz(segment[0], segment[1], raw) + EPS >= clearance,
                    "segment {segment:?} cuts raw endpoint {raw:?}; path={path:?}"
                );
            }
        }
    }

    #[test]
    fn route_out_of_disk_slides_vertex_to_boundary_along_portal_normal() {
        // A vertex inside an endpoint's clearance disk is moved to exactly
        // `clearance` from the raw endpoint along the portal normal, holding its
        // along-gate coordinate and Y fixed.
        let gates = inset_portals(&[(Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 0.0, 0.0))], 0.5)
            .expect("wide portal insets");
        let obstacle = gates[0].left; // raw (0,0,0), interior +x, normal +z
        assert!(obstacle.raw_endpoint.is_some());

        let vertex = Vec3::new(0.3, 0.0, 0.2); // inside the 0.5 disk (dist ~0.36)
        assert!(distance_xz(vertex, Vec3::new(0.0, 0.0, 0.0)) < 0.5);

        let routed =
            route_out_of_disk(obstacle, vertex, 0.5).expect("vertex has perpendicular room");
        // along-gate coordinate (x) preserved; pushed out on +z to the boundary.
        assert!(routed.abs_diff_eq(Vec3::new(0.3, 0.0, 0.4), EPS));
        assert!(approx_eq(
            distance_xz(routed, Vec3::new(0.0, 0.0, 0.0)),
            0.5
        ));

        // A vertex already clear (along-gate offset beyond the radius) has no
        // perpendicular solution.
        assert!(route_out_of_disk(obstacle, Vec3::new(0.9, 0.0, 0.0), 0.5).is_none());
    }

    #[test]
    fn project_out_of_disk_uses_portal_normal_for_fully_coincident_terminal() {
        // Guards the degenerate triple-coincidence fallback: the terminal AND its
        // adjacent waypoint both sit on the raw endpoint in XZ, so neither the
        // radial nor the `toward` direction survives and `project_out_of_disk` must
        // fall back to the portal normal (Finding 3's arbitrary-but-clearance-safe
        // branch). The fallback must still emit a finite standoff, moved off the raw
        // endpoint, that clears the disk by the effective clearance — no NaN/inf, no
        // zero-length move.
        let gates = inset_portals(&[(Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 0.0, 0.0))], 0.5)
            .expect("wide portal insets");
        let obstacle = gates[0].left; // raw (0,0,0), interior +x, normal +z
        let raw = obstacle
            .raw_endpoint
            .expect("wide endpoint carries its raw position");

        // Terminal and toward both coincide with raw in XZ (distinct Y is fine),
        // forcing the portal-normal fallback.
        let terminal = Vec3::new(0.0, 0.7, 0.0);
        let toward = Vec3::new(0.0, 1.0, 0.0);
        let projected = project_out_of_disk(obstacle, terminal, toward, 0.5)
            .expect("portal-normal fallback yields a finite direction for a filtered obstacle");

        assert!(
            projected.is_finite(),
            "standoff must be finite: {projected:?}"
        );
        assert!(
            !approx_xz(projected, raw),
            "standoff must move off the raw endpoint: {projected:?}"
        );
        assert!(
            approx_eq(distance_xz(projected, raw), 0.5),
            "standoff must sit on the disk boundary (clearance 0.5): {projected:?}"
        );
        assert!(
            approx_eq(projected.y, terminal.y),
            "terminal Y is preserved: {projected:?}"
        );
    }

    #[test]
    fn find_path_projects_both_terminals_in_wide_endpoint_disks_to_standoffs() {
        // Both endpoints snapped into a disk at once on one funnelled L route: the
        // start hugs the (0,*,4) jamb and the goal hugs the (4,*,8) endpoint, each
        // inside its wide-portal clearance disk. The start branch (`repaired[0]`)
        // and the goal branch (`repaired[last]`) must BOTH project to boundary
        // standoffs and the corridor must still route. Combines the two single-side
        // R1 fixtures to pin their interaction.
        let graph = NavGraph::from_section(&l_corridor_section());
        let clearance = graph.agent.radius + SKIN_DISTANCE;
        let start_jamb = Vec3::new(0.0, 0.0, 4.0);
        let goal_endpoint = Vec3::new(4.0, 0.0, 8.0);
        let start = Vec3::new(0.2, 0.0, 3.9);
        let goal = Vec3::new(4.1, 0.0, 7.8);
        assert_eq!(graph.region_at(start), Some(0), "start is in region 0");
        assert_eq!(graph.region_at(goal), Some(2), "goal is in region 2");
        assert!(
            distance_xz(start, start_jamb) < clearance
                && distance_xz(goal, goal_endpoint) < clearance,
            "fixture must place BOTH terminals inside their endpoint disks"
        );

        let path = find_path(&graph, start, goal)
            .expect("both-terminals-in-disk corridor must not drop to None");

        assert!(
            !approx_xz(path[0], start),
            "start terminal must be projected off its raw position: {path:?}"
        );
        assert!(
            distance_xz(path[0], start_jamb) + EPS >= clearance,
            "projected start must clear its endpoint disk: {path:?}"
        );
        assert!(
            !approx_xz(*path.last().unwrap(), goal),
            "goal terminal must be projected off its raw position: {path:?}"
        );
        assert!(
            distance_xz(*path.last().unwrap(), goal_endpoint) + EPS >= clearance,
            "projected goal must clear its endpoint disk: {path:?}"
        );

        // Interior clearance still holds: no segment cuts any wide-portal endpoint.
        for segment in path.windows(2) {
            for raw in L_CORRIDOR_ENDPOINTS {
                assert!(
                    segment_point_distance_xz(segment[0], segment[1], raw) + EPS >= clearance,
                    "segment {segment:?} cuts endpoint {raw:?}; path={path:?}"
                );
            }
        }
    }

    #[test]
    fn find_path_returns_none_for_unthreadable_pinch_gap_limit() {
        // Characterizes the KNOWN pinch-gap limit (see the twin-portal chicane
        // test): the endpoint-clearance repair offsets bevels along the portal
        // normal (axis-aligned), so it cannot thread a pinch narrower than
        // `2 * clearance`. Here the two convex corners are 0.5 apart — BELOW
        // `2 * clearance` (0.64) — so their clearance disks OVERLAP and no route
        // crosses the throat clear (portal 0-1 ends at x=2.7, portal 1-2 starts at
        // x=2.7, so any crossing passes the overlapping disks). The funnel's tight
        // route through the throat cannot be repaired clear, so `find_path` bails to
        // a clean `None` (the repair budget bounds the churn). This is the documented
        // residual, NOT a bug: a `None` here is expected, and the test guards that it
        // stays graceful — no panic, no NaN, no emitted path that grazes the disks.
        // The start is a snapped eroded-band terminal (region_at `None`) that reaches
        // this limit, so the `None` comes from the repair, not from endpoint resolve.
        let mut navmesh = section(
            vec![region(0, 0, 6, 2), region(0, 2, 6, 3), region(0, 3, 6, 6)],
            vec![
                NavPortal {
                    region_a: 0,
                    region_b: 1,
                    left: [0.0, 0.0, 2.0],
                    right: [2.7, 0.0, 2.0],
                },
                NavPortal {
                    region_a: 1,
                    region_b: 2,
                    left: [2.7, 0.0, 2.5],
                    right: [6.0, 0.0, 2.5],
                },
            ],
        );
        navmesh.agent_radius = 0.3;
        let graph = NavGraph::from_section(&navmesh);
        let clearance = graph.agent.radius + SKIN_DISTANCE;

        // The two convex pinch corners overlap: distance < 2 * clearance, so the
        // throat between them has no clearance-safe crossing.
        let corner_a = Vec3::new(2.7, 0.0, 2.0);
        let corner_b = Vec3::new(2.7, 0.0, 2.5);
        assert!(
            distance_xz(corner_a, corner_b) < 2.0 * clearance,
            "fixture must overlap the disks so the throat is genuinely unroutable"
        );

        // Start snapped from the eroded band (region_at `None`) just below region 0;
        // it must still resolve so the `None` is attributable to the pinch repair.
        let start = Vec3::new(1.0, 0.0, -0.2);
        let goal = Vec3::new(4.0, 0.0, 4.0);
        assert!(
            graph.region_at(start).is_none() && graph.resolve_region_at(start).is_some(),
            "start must be off-mesh yet snap onto the graph"
        );
        assert_eq!(graph.region_at(goal), Some(2), "goal is in region 2");

        assert!(
            find_path(&graph, start, goal).is_none(),
            "an unthreadable sub-2*clearance pinch must drop to a clean None"
        );
    }
}
