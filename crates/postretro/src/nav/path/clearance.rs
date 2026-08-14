use glam::Vec3;

use crate::nav::distance_xz;

use super::{FunnelEndpoint, FunnelGate, NavPath, segment_point_distance_xz};

const CLEARANCE_EPS: f32 = 1e-5;

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

fn clearance_bevel(corner: FunnelEndpoint, toward: Vec3, clearance_radius: f32) -> Option<Vec3> {
    let raw_endpoint = corner.raw_endpoint?;
    if segment_point_distance_xz(toward, corner.point, raw_endpoint) + CLEARANCE_EPS
        >= clearance_radius
    {
        return None;
    }
    bevel_point(corner, toward, clearance_radius)
}

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

const MIN_XZ_LEN_SQ: f32 = 1e-8;

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
        let toward_xz = Vec3::new(toward.x - raw_endpoint.x, 0.0, toward.z - raw_endpoint.z);
        if toward_xz.length_squared() > MIN_XZ_LEN_SQ {
            toward_xz.normalize()
        } else {
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
    use crate::nav::distance_xz;
    use glam::Vec3;

    const EPS: f32 = 1e-4;
    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() <= EPS
    }
    fn approx_xz(a: Vec3, b: Vec3) -> bool {
        approx_eq(a.x, b.x) && approx_eq(a.z, b.z)
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
}
