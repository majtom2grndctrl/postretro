// One-shot pathfinding query over a NavGraph: A* across regions, then a Simple
// Stupid Funnel string-pull over the corridor's portal segments.
// See: context/lib/build_pipeline.md §Navigation bake

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use glam::Vec3;
use postretro_level_format::navmesh::NavPortal;

use crate::collision::SKIN_DISTANCE;

use super::{NavGraph, distance_xz};

/// One-shot path query: A* over regions + funnel string-pull. Resolves the
/// regions containing `start` and `goal`, runs A* over the region graph (edge
/// cost = XZ distance between portal-segment midpoints, heuristic = XZ distance
/// between region centroids), reconstructs the exact portal corridor A* chose,
/// then funnels it to the tightest waypoint list within the corridor.
///
/// Returns `None` when `start` or `goal` lies outside every region, when either
/// endpoint is non-finite, or when no corridor connects their regions. A
/// reachable goal always yields a path whose first waypoint is `start` and last
/// is `goal`; a goal in the start region is a trivial two-point `[start, goal]`.
pub fn find_path(graph: &NavGraph, start: Vec3, goal: Vec3) -> Option<Vec<Vec3>> {
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

    let start_region = graph.region_at(start)?;
    let goal_region = graph.region_at(goal)?;

    if start_region == goal_region {
        // Same region: no portal to cross, the straight segment is the path.
        return Some(vec![start, goal]);
    }

    let corridor = astar_corridor(graph, start_region, goal_region)?;
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
    let gates = inset_portals(&portals, corner_clearance_radius);
    let pulled_path = funnel(start, goal, &gates);
    Some(bevel_clearance_corners(
        &pulled_path,
        corner_clearance_radius,
    ))
}

/// One hop of the region corridor: which portal A* crossed and which direction
/// (`from_region` is the region the agent leaves through this portal).
struct CorridorHop {
    portal_index: usize,
    from_region: usize,
}

/// Centroid of a region's XZ footprint as a world position (Y left at 0 — the
/// funnel and costs are XZ-only).
fn region_centroid(graph: &NavGraph, region: usize) -> Vec3 {
    let r = graph
        .region(region)
        .expect("region index from graph traversal is in range");
    Vec3::new(
        0.5 * (r.world_min_xz[0] + r.world_max_xz[0]),
        0.0,
        0.5 * (r.world_min_xz[1] + r.world_max_xz[1]),
    )
}

/// Midpoint of a portal's segment as a world position.
fn portal_midpoint(portal: &NavPortal) -> Vec3 {
    let l = Vec3::from_array(portal.left);
    let r = Vec3::from_array(portal.right);
    (l + r) * 0.5
}

/// Priority-queue entry: min-heap on `f = g + h` via `Reverse`-style ordering.
struct Frontier {
    region: usize,
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

/// A* over the region graph. Returns the ordered corridor of hops from
/// `start_region` to `goal_region`, each hop naming the exact portal crossed —
/// carried through `came_from` so a region pair joined by two distinct portals
/// uses the one A* costed, not the first match. `None` when disconnected.
fn astar_corridor(
    graph: &NavGraph,
    start_region: usize,
    goal_region: usize,
) -> Option<Vec<CorridorHop>> {
    let goal_centroid = region_centroid(graph, goal_region);
    let heuristic = |region: usize| distance_xz(region_centroid(graph, region), goal_centroid);

    let region_count = graph.region_count();
    let mut g_score = vec![f32::INFINITY; region_count];
    // `came_from[r] = (previous_region, portal_index_crossed)`.
    let mut came_from: Vec<Option<(usize, usize)>> = vec![None; region_count];

    g_score[start_region] = 0.0;
    let mut open = BinaryHeap::new();
    open.push(Frontier {
        region: start_region,
        f: heuristic(start_region),
    });

    // The per-region adjacency yields portal indices touching `region`, so we
    // both restrict the scan to real neighbors and record the exact portal A*
    // relaxed an edge through (Fix A: two portals may join the same region pair).
    let portals = graph.portals();

    while let Some(Frontier { region, .. }) = open.pop() {
        if region == goal_region {
            return Some(reconstruct(&came_from, start_region, goal_region));
        }

        for &portal_index in graph.region_portal_indices(region) {
            let portal = &portals[portal_index];
            let neighbor = if portal.region_a as usize == region {
                portal.region_b as usize
            } else if portal.region_b as usize == region {
                portal.region_a as usize
            } else {
                continue;
            };
            if neighbor >= region_count {
                continue;
            }

            // Edge cost: XZ distance from this region's centroid to the portal
            // midpoint plus the portal midpoint to the neighbor's centroid — a
            // stable per-edge cost anchored on the portal A* would cross.
            let mid = portal_midpoint(portal);
            let step = distance_xz(region_centroid(graph, region), mid)
                + distance_xz(mid, region_centroid(graph, neighbor));
            let tentative = g_score[region] + step;
            if tentative < g_score[neighbor] {
                g_score[neighbor] = tentative;
                came_from[neighbor] = Some((region, portal_index));
                open.push(Frontier {
                    region: neighbor,
                    f: tentative + heuristic(neighbor),
                });
            }
        }
    }

    None
}

/// Walk `came_from` back from the goal to build the forward-ordered corridor.
fn reconstruct(
    came_from: &[Option<(usize, usize)>],
    start_region: usize,
    goal_region: usize,
) -> Vec<CorridorHop> {
    let mut hops = Vec::new();
    let mut current = goal_region;
    while current != start_region {
        let (prev, portal_index) =
            came_from[current].expect("every region between start and goal has a predecessor");
        hops.push(CorridorHop {
            portal_index,
            from_region: prev,
        });
        current = prev;
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
fn inset_portals(portals: &[(Vec3, Vec3)], clearance_radius: f32) -> Vec<FunnelGate> {
    let clearance_radius = clearance_radius.max(0.0);
    portals
        .iter()
        .map(|&(left, right)| {
            let delta_xz = Vec3::new(right.x - left.x, 0.0, right.z - left.z);
            let width_xz = delta_xz.length();
            if width_xz <= 2.0 * clearance_radius || width_xz <= f32::EPSILON {
                let midpoint = (left + right) * 0.5;
                return FunnelGate {
                    left: FunnelEndpoint::terminal(midpoint),
                    right: FunnelEndpoint::terminal(midpoint),
                };
            }

            let inset_fraction = clearance_radius / width_xz;
            let left_interior = delta_xz / width_xz;
            FunnelGate {
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
            }
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

fn clearance_bevel(corner: FunnelEndpoint, toward: Vec3, clearance_radius: f32) -> Option<Vec3> {
    let raw_endpoint = corner.raw_endpoint?;
    let portal_interior = corner.portal_interior_xz?;
    if segment_point_distance_xz(toward, corner.point, raw_endpoint) + f32::EPSILON
        >= clearance_radius
    {
        return None;
    }

    let left_perpendicular = Vec3::new(-portal_interior.z, 0.0, portal_interior.x);
    let toward_corner = Vec3::new(toward.x - raw_endpoint.x, 0.0, toward.z - raw_endpoint.z);
    let corridor_side = if left_perpendicular.dot(toward_corner) >= 0.0 {
        left_perpendicular
    } else {
        -left_perpendicular
    };
    Some(corner.point + corridor_side * clearance_radius)
}

/// Radius-offset portal endpoints are safe points, but a chord incident to one
/// can still cut the endpoint's clearance disk. Add a portal-side bevel only
/// for those incident segments. The two bevel legs are tangent to the disk and
/// stay on the same corridor side as the adjacent path point.
fn bevel_clearance_corners(path: &[FunnelEndpoint], clearance_radius: f32) -> Vec<Vec3> {
    let clearance_radius = clearance_radius.max(0.0);
    if path.len() <= 2 || clearance_radius <= f32::EPSILON {
        return path.iter().map(|waypoint| waypoint.point).collect();
    }

    let mut beveled = Vec::with_capacity(path.len() * 2);
    beveled.push(path[0].point);
    for index in 1..path.len() - 1 {
        let corner = path[index];
        let previous = *beveled.last().expect("path starts with its first point");
        if let Some(incoming_bevel) = clearance_bevel(corner, previous, clearance_radius) {
            beveled.push(incoming_bevel);
        }
        beveled.push(corner.point);

        let next = path[index + 1].point;
        if let Some(outgoing_bevel) = clearance_bevel(corner, next, clearance_radius) {
            beveled.push(outgoing_bevel);
        }
    }
    beveled.push(path[path.len() - 1].point);
    beveled
}

/// Simple Stupid Funnel string-pull over an ordered list of traversal-oriented
/// `(left, right)` portal segments. Emits the tightest waypoint list from
/// `start` to `goal` that stays within the corridor. The first waypoint is
/// `start`, the last is `goal`; a straight corridor collapses to `[start, goal]`.
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
    fn find_path_bends_l_corridor_at_inset_inner_corner_portal_endpoint() {
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
        // The funnel must bend at the inner-corner portal endpoint inset by the
        // canonical agent radius plus collision skin, rather than steering the
        // capsule into the raw wall corner.
        let inset_corner = Vec3::new(4.0, 0.0, 4.0 + graph.agent.radius + SKIN_DISTANCE);
        let bends_at_inset_corner = path[1..path.len() - 1]
            .iter()
            .any(|w| approx_xz(*w, inset_corner));
        assert!(
            bends_at_inset_corner,
            "expected a bend inset from the inner corner {inner_corner:?}, got {path:?}"
        );
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

        let gate = inset_portals(&[(endpoint, opposite_endpoint)], 0.37)[0];

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

        let gate = inset_portals(&[(left, right)], 0.4)[0];

        assert!(gate.left.point.abs_diff_eq(Vec3::new(0.4, 1.4, 4.0), EPS));
        assert!(gate.right.point.abs_diff_eq(Vec3::new(1.6, 2.6, 4.0), EPS));
    }

    #[test]
    fn funnel_bevels_segments_around_inner_corner_clearance_disk() {
        let graph = NavGraph::from_section(&l_corridor_section());
        let start = Vec3::new(1.0, 0.0, 1.0);
        let goal = Vec3::new(7.0, 0.0, 5.0);
        let path = find_path(&graph, start, goal).expect("L corridor connects");
        let corner = Vec3::new(4.0, 0.0, 4.0);
        let effective_clearance = graph.agent.radius + SKIN_DISTANCE;

        // Regression: radius-clear inset points were joined by chords that cut
        // inside the same corner's clearance disk.
        for segment in path.windows(2) {
            let clearance = segment_point_distance_xz(segment[0], segment[1], corner);
            assert!(
                clearance + EPS >= effective_clearance,
                "segment {:?} cuts the corner disk: clearance={clearance}, path={path:?}",
                segment,
            );
        }
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
}
