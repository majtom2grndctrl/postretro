// One-shot pathfinding query over a NavGraph: A* across regions, then a Simple
// Stupid Funnel string-pull over the corridor's portal segments.
// See: context/lib/build_pipeline.md §Navigation bake

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use glam::Vec3;
use postretro_level_format::navmesh::NavPortal;

use super::{NavGraph, distance_xz};

/// One-shot path query: A* over regions + funnel string-pull. Resolves the
/// regions containing `start` and `goal`, runs A* over the region graph (edge
/// cost = XZ distance between portal-segment midpoints, heuristic = XZ distance
/// between region centroids), reconstructs the exact portal corridor A* chose,
/// then funnels it to the tightest waypoint list within the corridor.
///
/// Returns `None` when `start` or `goal` lies outside every region, or when no
/// corridor connects their regions. A reachable goal always yields a path whose
/// first waypoint is `start` and last is `goal`; a goal in the start region is a
/// trivial two-point `[start, goal]`.
#[allow(dead_code)]
pub fn find_path(graph: &NavGraph, start: Vec3, goal: Vec3) -> Option<Vec<Vec3>> {
    let start_region = graph.region_at(start)?;
    let goal_region = graph.region_at(goal)?;

    if start_region == goal_region {
        // Same region: no portal to cross, the straight segment is the path.
        return Some(vec![start, goal]);
    }

    let corridor = astar_corridor(graph, start_region, goal_region)?;
    let portals = oriented_portals(graph, &corridor);
    Some(funnel(start, goal, &portals))
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

/// Simple Stupid Funnel string-pull over an ordered list of traversal-oriented
/// `(left, right)` portal segments. Emits the tightest waypoint list from
/// `start` to `goal` that stays within the corridor. The first waypoint is
/// `start`, the last is `goal`; a straight corridor collapses to `[start, goal]`.
fn funnel(start: Vec3, goal: Vec3, portals: &[(Vec3, Vec3)]) -> Vec<Vec3> {
    let mut path = vec![start];

    let mut apex = start;
    let mut left = start;
    let mut right = start;
    let mut left_index = 0usize;
    let mut right_index = 0usize;

    // Append the goal as a degenerate final portal so the funnel pulls all the
    // way to it with the same logic as any interior gate.
    let mut gates: Vec<(Vec3, Vec3)> = Vec::with_capacity(portals.len() + 1);
    gates.extend_from_slice(portals);
    gates.push((goal, goal));

    let mut i = 0;
    while i < gates.len() {
        let (gate_left, gate_right) = gates[i];

        // Tighten the right side.
        if triangle_area_xz(apex, right, gate_right) <= 0.0 {
            if apex == right || triangle_area_xz(apex, left, gate_right) > 0.0 {
                // Still inside the funnel — narrow the right edge.
                right = gate_right;
                right_index = i;
            } else {
                // Right over left: the left vertex becomes a new apex/corner.
                path.push(left);
                apex = left;
                // Restart the funnel from the vertex after the new apex.
                left_index += 1;
                right_index = left_index;
                left = apex;
                right = apex;
                i = left_index;
                continue;
            }
        }

        // Tighten the left side.
        if triangle_area_xz(apex, left, gate_left) >= 0.0 {
            if apex == left || triangle_area_xz(apex, right, gate_left) < 0.0 {
                left = gate_left;
                left_index = i;
            } else {
                path.push(right);
                apex = right;
                right_index += 1;
                left_index = right_index;
                left = apex;
                right = apex;
                i = right_index;
                continue;
            }
        }

        i += 1;
    }

    // Always end on the goal (the degenerate last gate guarantees reachability,
    // but the apex may already sit at goal if the corridor pulled straight).
    if *path.last().expect("path starts with `start`") != goal {
        path.push(goal);
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
    fn find_path_bends_l_corridor_at_inner_corner_portal_endpoint() {
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
        // A correct funnel bends exactly at the inner-corner portal endpoint; a
        // broken-handedness straight-collapse would not place a vertex there.
        let bends_at_corner = path[1..path.len() - 1]
            .iter()
            .any(|w| approx_xz(*w, inner_corner));
        assert!(
            bends_at_corner,
            "expected a bend at the inner corner {inner_corner:?}, got {path:?}"
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

        let inner_corner = Vec3::new(4.0, 0.0, 4.0);
        assert!(approx_xz(path[0], start));
        assert!(approx_xz(*path.last().unwrap(), goal));
        let bends_at_corner = path[1..path.len() - 1]
            .iter()
            .any(|w| approx_xz(*w, inner_corner));
        assert!(
            bends_at_corner,
            "reversed L corridor must still bend at inner corner {inner_corner:?}, got {path:?}"
        );
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
