// Clearance repair for funnel paths around portal endpoint disks.
// See: context/lib/build_pipeline.md §Navigation bake

use glam::Vec3;

use crate::nav::distance_xz;

use super::{FunnelEndpoint, FunnelGate, NavPath, segment_point_distance_xz};

// Keep all clearance predicates on the same side of floating-point boundaries.
const CLEARANCE_EPS: f32 = 1e-5;

// Keep the bevel on the adjacent corridor side. The portal normal is a
// constrained tangent surrogate, not a free turning direction.
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

// Add a fixed repair vertex only when this chord enters the endpoint disk.
fn clearance_bevel(corner: FunnelEndpoint, toward: Vec3, clearance_radius: f32) -> Option<Vec3> {
    let raw_endpoint = corner.raw_endpoint?;
    if segment_point_distance_xz(toward, corner.point, raw_endpoint) + CLEARANCE_EPS
        >= clearance_radius
    {
        return None;
    }
    bevel_point(corner, toward, clearance_radius)
}

// Preserve the vertex's portal-axis coordinate and slide along the normal to
// the disk boundary; changing that axis would violate its funnel constraint.
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
    let target_perp = remaining
        .sqrt()
        .copysign(if perp >= 0.0 { 1.0 } else { -1.0 });
    Some(Vec3::new(
        raw_endpoint.x + portal_interior.x * along + normal.x * target_perp,
        vertex.y,
        raw_endpoint.z + portal_interior.z * along + normal.z * target_perp,
    ))
}

// Treat near-zero XZ headings as directionless so terminal standoffs stay finite.
const MIN_XZ_LEN_SQ: f32 = 1e-8;

/// Moves an in-disk terminal to the corridor-facing standoff so its onward
/// chord does not re-enter the endpoint disk.
///
/// The adjacent corridor waypoint is primary; degenerate XZ direction falls
/// back to the terminal's radial direction, then the portal normal.
fn project_out_of_disk(
    obstacle: FunnelEndpoint,
    terminal: Vec3,
    toward: Vec3,
    clearance_radius: f32,
) -> Option<Vec3> {
    let raw_endpoint = obstacle.raw_endpoint?;
    let toward_xz = Vec3::new(toward.x - raw_endpoint.x, 0.0, toward.z - raw_endpoint.z);
    let radial = Vec3::new(
        terminal.x - raw_endpoint.x,
        0.0,
        terminal.z - raw_endpoint.z,
    );
    let direction = if toward_xz.length_squared() > MIN_XZ_LEN_SQ {
        toward_xz.normalize()
    } else if radial.length_squared() > MIN_XZ_LEN_SQ {
        radial.normalize()
    } else {
        let portal_interior = obstacle.portal_interior_xz?;
        let normal = Vec3::new(-portal_interior.z, 0.0, portal_interior.x);
        if normal.length_squared() > MIN_XZ_LEN_SQ {
            normal.normalize()
        } else {
            return None;
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

/// Repairs endpoint-disk incursions while retaining each repair turn as a fixed,
/// mandatory vertex, so later smoothing cannot shortcut its clearance.
pub(super) fn ensure_endpoint_clearance(
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
        // Overlapping endpoint disks can revisit prior segments; bound repair churn
        // so non-converging or unrepresentable clearance repair returns `None`
        // rather than spinning.
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
            if distance_xz(start, raw) + CLEARANCE_EPS < clearance_radius {
                if segment_index == 0 {
                    let projected = project_out_of_disk(obstacle, start, end, clearance_radius)?;
                    repaired[0].point = projected;
                    repaired[0].mandatory = true;
                    continue;
                }
                let routed = route_out_of_disk(obstacle, start, clearance_radius)?;
                repaired[segment_index].point = routed;
                repaired[segment_index].mandatory = true;
                segment_index -= 1;
                continue;
            }
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

#[cfg(test)]
mod tests {
    use super::{project_out_of_disk, route_out_of_disk};
    use crate::nav::{NavGraph, distance_xz, find_path};
    use glam::Vec3;
    use postretro_level_format::navmesh::{NAVMESH_VERSION, NavMeshSection, NavPortal, NavRegion};

    const EPS: f32 = 1e-4;
    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() <= EPS
    }
    fn approx_xz(a: Vec3, b: Vec3) -> bool {
        approx_eq(a.x, b.x) && approx_eq(a.z, b.z)
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

    fn wall_wraparound_section() -> NavMeshSection {
        NavMeshSection {
            version: NAVMESH_VERSION,
            origin: [0.0, 0.0, 0.0],
            cell_size: 0.1,
            dim_x: 160,
            dim_z: 160,
            agent_radius: 0.3,
            agent_height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
            // Four walkable regions wrap the left end of a freestanding wall:
            // near side -> wall end -> far-side relay -> far side.
            regions: vec![
                region(0, 0, 80, 20),
                region(0, 20, 30, 70),
                region(0, 70, 25, 73),
                region(0, 75, 80, 110),
            ],
            portals: vec![
                NavPortal {
                    region_a: 0,
                    region_b: 1,
                    left: [0.0, 0.0, 2.0],
                    right: [3.0, 0.0, 2.0],
                },
                NavPortal {
                    region_a: 1,
                    region_b: 2,
                    left: [0.0, 0.0, 7.0],
                    right: [3.0, 0.0, 7.0],
                },
                NavPortal {
                    region_a: 2,
                    region_b: 3,
                    left: [0.0, 0.0, 7.3],
                    right: [3.0, 0.0, 7.3],
                },
            ],
        }
    }

    fn wall_end_pinch_section() -> NavMeshSection {
        NavMeshSection {
            version: NAVMESH_VERSION,
            origin: [0.0, 0.0, 0.0],
            cell_size: 0.1,
            dim_x: 80,
            dim_z: 80,
            agent_radius: 0.3,
            agent_height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
            // The only route from the near side to the far side turns around a
            // wall end through this deliberately narrow middle throat.
            regions: vec![
                region(0, 0, 70, 20),
                region(0, 20, 27, 25),
                region(0, 25, 70, 70),
            ],
            portals: vec![
                NavPortal {
                    region_a: 0,
                    region_b: 1,
                    left: [0.0, 0.0, 2.0],
                    right: [2.7, 0.0, 2.0],
                },
                NavPortal {
                    region_a: 1,
                    region_b: 2,
                    left: [2.7, 0.0, 2.45],
                    right: [7.0, 0.0, 2.45],
                },
            ],
        }
    }

    #[test]
    fn route_out_of_disk_slides_vertex_to_boundary_along_portal_normal() {
        let gates = super::super::inset_portals(
            &[(Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 0.0, 0.0))],
            0.5,
        )
        .expect("wide portal insets");
        let obstacle = gates[0].left;
        assert!(obstacle.raw_endpoint.is_some());
        let vertex = Vec3::new(0.3, 0.0, 0.2);
        assert!(distance_xz(vertex, Vec3::new(0.0, 0.0, 0.0)) < 0.5);
        let routed =
            route_out_of_disk(obstacle, vertex, 0.5).expect("vertex has perpendicular room");
        assert!(routed.abs_diff_eq(Vec3::new(0.3, 0.0, 0.4), EPS));
        assert!(approx_eq(
            distance_xz(routed, Vec3::new(0.0, 0.0, 0.0)),
            0.5
        ));
        assert!(route_out_of_disk(obstacle, Vec3::new(0.9, 0.0, 0.0), 0.5).is_none());
    }

    #[test]
    fn project_out_of_disk_uses_portal_normal_for_fully_coincident_terminal() {
        let gates = super::super::inset_portals(
            &[(Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 0.0, 0.0))],
            0.5,
        )
        .expect("wide portal insets");
        let obstacle = gates[0].left;
        let raw = obstacle
            .raw_endpoint
            .expect("wide endpoint carries its raw position");
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

    // Regression: far-side endpoint standoff projection exhausted clearance repair around a wall.
    #[test]
    fn find_path_routes_wall_wraparound_from_far_side_eroded_goal() {
        let graph = NavGraph::from_section(&wall_wraparound_section());
        let clearance = graph.agent_params().radius + crate::collision::SKIN_DISTANCE;
        let wall_end = Vec3::new(3.0, 0.0, 7.0);
        let neighboring_endpoint = Vec3::new(3.0, 0.0, 7.3);
        let start = Vec3::new(7.0, 0.0, 1.0);
        let goal = Vec3::new(3.1, 0.0, 7.25);

        // The overlapping endpoints are on the wall side; each around-end gate
        // remains 3 m wide, leaving a generous channel on the other side.
        assert!(
            3.0 > 2.0 * clearance + 1.0,
            "wall-end throat must be comfortably wider than two clearances"
        );
        assert!(
            graph.region_at(goal).is_none() && graph.resolve_region_at(goal) == Some(3),
            "far-side goal must exercise snapped eroded-band resolution"
        );
        assert!(
            distance_xz(goal, wall_end) < clearance,
            "far-side goal must sit inside the wall-end endpoint disk"
        );
        let radial_standoff = wall_end
            + Vec3::new(goal.x - wall_end.x, 0.0, goal.z - wall_end.z).normalize() * clearance;
        assert!(
            distance_xz(radial_standoff, neighboring_endpoint) < clearance,
            "radial standoff must land inside the overlapping neighbor disk"
        );

        let path = find_path(&graph, start, goal)
            .expect("corridor-facing far-side standoff must preserve the wraparound route");
        assert!(
            path.len() >= 3,
            "wall wraparound must remain a multi-waypoint route: {path:?}"
        );
        assert!(
            !approx_xz(*path.last().expect("path has a goal standoff"), goal),
            "in-disk far-side goal must project to a standoff: {path:?}"
        );
        assert!(
            approx_eq(
                distance_xz(
                    *path.last().expect("path has a goal standoff"),
                    neighboring_endpoint
                ),
                clearance
            ),
            "goal standoff must remain exactly on the neighbor disk boundary: {path:?}"
        );

        let raw_endpoints = [
            Vec3::new(0.0, 0.0, 2.0),
            Vec3::new(3.0, 0.0, 2.0),
            Vec3::new(0.0, 0.0, 7.0),
            wall_end,
            Vec3::new(0.0, 0.0, 7.3),
            neighboring_endpoint,
        ];
        for segment in path.windows(2) {
            for raw_endpoint in raw_endpoints {
                let segment_clearance =
                    super::super::segment_point_distance_xz(segment[0], segment[1], raw_endpoint);
                assert!(
                    segment_clearance + EPS >= clearance,
                    "segment {segment:?} cuts endpoint {raw_endpoint:?}; path={path:?}"
                );
            }
        }
    }

    // Regression: corridor-biased terminal projection must not thread a sub-clearance wall-end pinch.
    #[test]
    fn wall_end_pinch_returns_none_below_clearance_width() {
        let graph = NavGraph::from_section(&wall_end_pinch_section());
        let clearance = graph.agent_params().radius + crate::collision::SKIN_DISTANCE;
        let near_corner = Vec3::new(2.7, 0.0, 2.0);
        let far_corner = Vec3::new(2.7, 0.0, 2.45);

        assert!(
            distance_xz(near_corner, far_corner) + EPS < 2.0 * clearance,
            "fixture throat must remain strictly narrower than two clearances"
        );

        let start = Vec3::new(5.0, 0.0, 1.0);
        let goal = Vec3::new(5.0, 0.0, 4.0);
        assert_eq!(graph.region_at(start), Some(0));
        assert_eq!(graph.region_at(goal), Some(2));
        assert!(
            find_path(&graph, start, goal).is_none(),
            "a sub-2*clearance wall-end throat must remain unroutable"
        );
    }
}
