// Compiler extraction for the KinematicGeometry PRL section.
// See: context/lib/build_pipeline.md §PRL Compilation.

use postretro_level_format::kinematic_geometry::{
    KINEMATIC_GEOMETRY_VERSION, KinematicGeometrySection, KinematicMoverRecord,
    KinematicWaypointRecord,
};
use postretro_level_format::texture_names::TextureNamesSection;

use crate::geometry::extract_kinematic_mover_geometry;
use crate::map_data::{BrushVolume, MapKinematicMover, MapKinematicWaypoint};
use crate::portals::Portal;

const PORTAL_SEAL_EPSILON: f64 = 1.0e-6;

pub fn encode_kinematic_geometry_section(
    movers: &[MapKinematicMover],
    waypoints: &[MapKinematicWaypoint],
    generated_portals: &[Portal],
    texture_names: &mut TextureNamesSection,
) -> Option<KinematicGeometrySection> {
    if movers.is_empty() {
        return None;
    }

    let mover_records = movers
        .iter()
        .map(|mover| {
            let sides: Vec<_> = mover
                .brush_volumes
                .iter()
                .flat_map(|brush| brush.sides.iter().cloned())
                .collect();
            let geometry = extract_kinematic_mover_geometry(&sides, texture_names, mover.origin);
            KinematicMoverRecord {
                mover_id: mover.mover_id,
                name: mover.name.clone(),
                tags: mover.tags.clone(),
                origin: [
                    mover.origin.x as f32,
                    mover.origin.y as f32,
                    mover.origin.z as f32,
                ],
                path: mover.path.clone(),
                speed: mover.speed,
                wait_ms: mover.wait_ms,
                spin_axis: mover.spin_axis,
                spin_speed_deg_s: mover.spin_speed_deg_s,
                spin_accel_deg_s2: mover.spin_accel_deg_s2,
                carry_yaw: mover.carry_yaw,
                block_policy: mover.block_policy.clone(),
                crush_damage: mover.crush_damage,
                crush_interval_ms: mover.crush_interval_ms,
                auto_close_ms: mover.auto_close_ms,
                open_event: mover.open_event.clone(),
                close_event: mover.close_event.clone(),
                blocked_event: mover.blocked_event.clone(),
                crush_event: mover.crush_event.clone(),
                sealed_portal_ids: sealed_portal_ids_for_mover(mover, generated_portals),
                move_mode: mover.move_mode.to_wire(),
                start_on_spawn: mover.start_on_spawn,
                vertices: geometry.vertices,
                indices: geometry.indices,
                face_meta: geometry.faces,
            }
        })
        .collect();

    let waypoint_records = waypoints
        .iter()
        .map(|waypoint| KinematicWaypointRecord {
            name: waypoint.name.clone(),
            next: waypoint.next.clone(),
            origin: [
                waypoint.origin.x as f32,
                waypoint.origin.y as f32,
                waypoint.origin.z as f32,
            ],
        })
        .collect();

    Some(KinematicGeometrySection {
        version: KINEMATIC_GEOMETRY_VERSION,
        movers: mover_records,
        waypoints: waypoint_records,
    })
}

/// Return the generated portal ids fully covered by a mover's closed-pose
/// geometry. This thin slice intentionally handles exactly one convex brush;
/// multi-brush movers stay non-occluding until the conservative union-coverage
/// carve replaces this helper.
fn sealed_portal_ids_for_mover(mover: &MapKinematicMover, portals: &[Portal]) -> Vec<u32> {
    let [brush] = mover.brush_volumes.as_slice() else {
        return Vec::new();
    };

    portals
        .iter()
        .enumerate()
        .filter_map(|(portal_id, portal)| {
            portal_fully_inside_brush(portal, brush)
                .then(|| u32::try_from(portal_id).ok())
                .flatten()
        })
        .collect()
}

/// A convex brush covers a portal only when every portal vertex is inside (or
/// on) every brush plane. Degenerate inputs deliberately remain uncovered.
fn portal_fully_inside_brush(portal: &Portal, brush: &BrushVolume) -> bool {
    portal.polygon.len() >= 3
        && !brush.planes.is_empty()
        && portal.polygon.iter().all(|vertex| {
            brush
                .planes
                .iter()
                .all(|plane| plane.normal.dot(*vertex) - plane.distance <= PORTAL_SEAL_EPSILON)
        })
}

#[cfg(test)]
mod tests {
    use glam::DVec3;

    use super::*;
    use crate::map_data::{BrushPlane, KinematicMoveMode};
    use crate::partition::Aabb;

    fn box_brush(min: DVec3, max: DVec3) -> BrushVolume {
        BrushVolume {
            planes: vec![
                BrushPlane {
                    normal: DVec3::X,
                    distance: max.x,
                },
                BrushPlane {
                    normal: DVec3::NEG_X,
                    distance: -min.x,
                },
                BrushPlane {
                    normal: DVec3::Y,
                    distance: max.y,
                },
                BrushPlane {
                    normal: DVec3::NEG_Y,
                    distance: -min.y,
                },
                BrushPlane {
                    normal: DVec3::Z,
                    distance: max.z,
                },
                BrushPlane {
                    normal: DVec3::NEG_Z,
                    distance: -min.z,
                },
            ],
            sides: Vec::new(),
            aabb: Aabb { min, max },
        }
    }

    fn mover(brush_volumes: Vec<BrushVolume>) -> MapKinematicMover {
        MapKinematicMover {
            mover_id: 7,
            name: "closet_door".to_string(),
            tags: Vec::new(),
            origin: DVec3::ZERO,
            authored_origin: None,
            path: "closed".to_string(),
            speed: 1.0,
            wait_ms: 0.0,
            spin_axis: [0.0; 3],
            spin_speed_deg_s: 0.0,
            spin_accel_deg_s2: 0.0,
            carry_yaw: false,
            block_policy: "displace".to_string(),
            crush_damage: 0.0,
            crush_interval_ms: 0.0,
            auto_close_ms: None,
            open_event: None,
            close_event: None,
            blocked_event: None,
            crush_event: None,
            move_mode: KinematicMoveMode::Once,
            start_on_spawn: false,
            brush_volumes,
        }
    }

    fn portal_at_x(x: f64) -> Portal {
        Portal {
            polygon: vec![
                DVec3::new(x, -1.0, -1.0),
                DVec3::new(x, 1.0, -1.0),
                DVec3::new(x, 1.0, 1.0),
                DVec3::new(x, -1.0, 1.0),
            ],
            front_leaf: 0,
            back_leaf: 1,
        }
    }

    #[test]
    fn single_brush_closed_door_marks_only_fully_covered_portals() {
        let door = mover(vec![box_brush(
            DVec3::new(-0.1, -2.0, -2.0),
            DVec3::new(0.1, 2.0, 2.0),
        )]);
        let portals = vec![portal_at_x(0.0), portal_at_x(2.0)];

        assert_eq!(sealed_portal_ids_for_mover(&door, &portals), vec![0]);
    }

    #[test]
    fn multi_brush_mover_stays_non_occluding_in_thin_slice() {
        let door = mover(vec![
            box_brush(DVec3::new(-0.1, -2.0, -2.0), DVec3::new(0.1, 0.0, 2.0)),
            box_brush(DVec3::new(-0.1, 0.0, -2.0), DVec3::new(0.1, 2.0, 2.0)),
        ]);

        assert!(sealed_portal_ids_for_mover(&door, &[portal_at_x(0.0)]).is_empty());
    }
}
