// Compiler extraction for the KinematicGeometry PRL section.
// See: context/lib/build_pipeline.md §PRL Compilation.

use postretro_level_format::kinematic_geometry::{
    KINEMATIC_GEOMETRY_VERSION, KinematicGeometrySection, KinematicMoverRecord,
    KinematicWaypointRecord,
};
use postretro_level_format::texture_names::TextureNamesSection;

use crate::geometry::extract_kinematic_mover_geometry;
use crate::map_data::{MapKinematicMover, MapKinematicWaypoint};

pub fn encode_kinematic_geometry_section(
    movers: &[MapKinematicMover],
    waypoints: &[MapKinematicWaypoint],
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
