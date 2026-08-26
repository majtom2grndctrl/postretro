// Compiler extraction for the KinematicGeometry PRL section.
// See: context/lib/build_pipeline.md §PRL Compilation.

use std::collections::HashMap;

use postretro_level_format::kinematic_geometry::{
    KINEMATIC_GEOMETRY_VERSION, KinematicGeometrySection, KinematicMoverRecord,
    KinematicWaypointRecord, MemberLight,
};
use postretro_level_format::texture_names::TextureNamesSection;

use crate::geometry::extract_kinematic_mover_geometry;
use crate::geometry_utils::clip_winding_to_half_spaces;
use crate::light_namespaces::AlphaLightsNs;
use crate::map_data::{BrushVolume, CarriedLightLink, MapKinematicMover, MapKinematicWaypoint};
use crate::partition::Aabb;
use crate::portals::{PORTAL_EPSILON, Portal};

/// Ignore only numerical clipping dust. Any real uncovered portal area must
/// survive the carve so the camera flood stays conservative toward drawing.
const MIN_UNCOVERED_PORTAL_AREA_M2: f64 = 1.0e-12;
const MIN_BRUSH_AABB_EXTENT: f64 = 1.0e-12;

pub fn encode_kinematic_geometry_section(
    movers: &[MapKinematicMover],
    waypoints: &[MapKinematicWaypoint],
    carried_lights_by_mover: &[Vec<MemberLight>],
    generated_portals: &[Portal],
    texture_names: &mut TextureNamesSection,
) -> Option<KinematicGeometrySection> {
    if movers.is_empty() {
        return None;
    }

    let mover_records = movers
        .iter()
        .enumerate()
        .map(|(mover_index, mover)| {
            let sides: Vec<_> = mover
                .brush_volumes
                .iter()
                .flat_map(|brush| brush.sides.iter().cloned())
                .collect();
            let geometry = extract_kinematic_mover_geometry(&sides, texture_names, mover.origin);
            let sealed_portal_ids = sealed_portal_ids_for_mover(mover, generated_portals);
            for &portal_id in &sealed_portal_ids {
                log::info!(
                    "[Compiler] kinematic mover {} seals generated portal {}",
                    mover.mover_id,
                    portal_id
                );
            }

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
                sealed_portal_ids,
                carried_lights: carried_lights_by_mover
                    .get(mover_index)
                    .cloned()
                    .unwrap_or_default(),
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

/// Translate compiler-source light indices into the positional AlphaLights
/// namespace at the one seam where both index spaces are available.
pub fn member_lights_by_mover(
    movers: &[MapKinematicMover],
    links: &[CarriedLightLink],
    alpha_lights: &AlphaLightsNs<'_>,
) -> Vec<Vec<MemberLight>> {
    let alpha_index_by_source: HashMap<usize, u32> = alpha_lights
        .entries()
        .iter()
        .enumerate()
        .map(|(alpha_index, entry)| (entry.source_index, alpha_index as u32))
        .collect();
    let mover_index_by_id: HashMap<u32, usize> = movers
        .iter()
        .enumerate()
        .map(|(mover_index, mover)| (mover.mover_id, mover_index))
        .collect();
    let mut members = vec![Vec::new(); movers.len()];

    for link in links {
        let Some(&alpha_light_index) = alpha_index_by_source.get(&link.source_light_index) else {
            continue;
        };
        let Some(&mover_index) = mover_index_by_id.get(&link.mover_id) else {
            continue;
        };
        members[mover_index].push(MemberLight {
            alpha_light_index,
            local_offset: link.local_offset,
        });
    }

    members
}

/// Return generated portal ids covered by the union of a mover's closed-pose
/// convex brush volumes. Portal enumeration supplies the IDs directly, so the
/// emitted list is deterministic, ascending, and duplicate-free.
fn sealed_portal_ids_for_mover(mover: &MapKinematicMover, portals: &[Portal]) -> Vec<u32> {
    let Some(mover_brush_bounds) = mover_brush_bounds(&mover.brush_volumes) else {
        return Vec::new();
    };

    portals
        .iter()
        .enumerate()
        .filter_map(|(portal_id, portal)| {
            let portal_bounds = portal_bounds(portal)?;
            mover_brush_bounds
                .intersects(&portal_bounds)
                .then_some(())?;
            portal_fully_covered_by_brush_union(portal, &mover.brush_volumes)
                .then(|| u32::try_from(portal_id).ok())
                .flatten()
        })
        .collect()
}

fn mover_brush_bounds(brushes: &[BrushVolume]) -> Option<Aabb> {
    if brushes.is_empty() || brushes.iter().any(|brush| !brush_is_valid(brush)) {
        return None;
    }

    let mut bounds = Aabb::empty();
    for brush in brushes {
        bounds.expand_aabb(&brush.aabb);
    }
    bounds.is_valid().then_some(bounds)
}

/// A brush must describe a finite, non-zero-volume convex solid before it can
/// remove any portal area. Rejecting malformed input is deliberately safer
/// than guessing that it covers a sightline.
fn brush_is_valid(brush: &BrushVolume) -> bool {
    brush.planes.len() >= 4
        && brush.aabb.is_valid()
        && aabb_has_volume(&brush.aabb)
        && brush.planes.iter().all(|plane| {
            plane.normal.is_finite()
                && plane.distance.is_finite()
                && plane.normal.length_squared() > f64::EPSILON
        })
}

fn aabb_has_volume(bounds: &Aabb) -> bool {
    (bounds.max.x - bounds.min.x) > MIN_BRUSH_AABB_EXTENT
        && (bounds.max.y - bounds.min.y) > MIN_BRUSH_AABB_EXTENT
        && (bounds.max.z - bounds.min.z) > MIN_BRUSH_AABB_EXTENT
}

fn portal_bounds(portal: &Portal) -> Option<Aabb> {
    (portal.polygon.len() >= 3
        && portal.polygon.iter().all(|vertex| vertex.is_finite())
        && polygon_area(&portal.polygon) > MIN_UNCOVERED_PORTAL_AREA_M2)
        .then(|| portal_polygon_is_valid(&portal.polygon))
        .filter(|is_valid| *is_valid)
        .map(|_| Aabb::from_points(&portal.polygon))
        .filter(Aabb::is_valid)
}

/// The generated-portal contract promises a finite convex winding. Preserve
/// the conservative failure mode if a malformed caller violates that contract:
/// do not try to carve an arbitrary non-convex or non-planar polygon.
fn portal_polygon_is_valid(vertices: &[glam::DVec3]) -> bool {
    let mut unnormalized_normal = glam::DVec3::ZERO;
    for index in 0..vertices.len() {
        unnormalized_normal += vertices[index].cross(vertices[(index + 1) % vertices.len()]);
    }
    if unnormalized_normal.length_squared() <= f64::EPSILON {
        return false;
    }

    let normal = unnormalized_normal.normalize();
    let plane_distance = normal.dot(vertices[0]);
    if vertices
        .iter()
        .any(|vertex| (normal.dot(*vertex) - plane_distance).abs() > PORTAL_EPSILON)
    {
        return false;
    }

    let mut winding_sign = 0.0_f64;
    for index in 0..vertices.len() {
        let current = vertices[index];
        let next = vertices[(index + 1) % vertices.len()];
        let after_next = vertices[(index + 2) % vertices.len()];
        let edge = next - current;
        if edge.length_squared() <= f64::EPSILON {
            return false;
        }

        let turn = edge.cross(after_next - next).dot(normal);
        if turn.abs() <= f64::EPSILON {
            continue;
        }
        if winding_sign != 0.0 && turn.signum() != winding_sign {
            return false;
        }
        winding_sign = turn.signum();
    }

    winding_sign != 0.0
}

/// Carve each convex brush from the still-uncovered portal fragments. The
/// first pass uses the exact portal-generation tolerance. A second zero-
/// tolerance certification prevents that intentionally forgiving clip margin
/// from swallowing a physically open, sub-epsilon seam.
fn portal_fully_covered_by_brush_union(portal: &Portal, brushes: &[BrushVolume]) -> bool {
    portal_is_covered_with_clip_epsilon(portal, brushes, PORTAL_EPSILON)
        && portal_is_covered_with_clip_epsilon(portal, brushes, 0.0)
}

fn portal_is_covered_with_clip_epsilon(
    portal: &Portal,
    brushes: &[BrushVolume],
    clip_epsilon: f64,
) -> bool {
    let mut uncovered = vec![portal.polygon.clone()];

    for brush in brushes {
        let mut next_uncovered = Vec::new();
        for fragment in uncovered {
            next_uncovered.extend(carve_fragment_outside_brush(fragment, brush, clip_epsilon));
        }
        uncovered = next_uncovered;

        if !uncovered
            .iter()
            .any(|fragment| polygon_area(fragment) > MIN_UNCOVERED_PORTAL_AREA_M2)
        {
            return true;
        }
    }

    !uncovered
        .iter()
        .any(|fragment| polygon_area(fragment) > MIN_UNCOVERED_PORTAL_AREA_M2)
}

/// Split one convex uncovered fragment against every brush plane. At each
/// plane the portion outside that plane stays uncovered; only the portion
/// satisfying every brush half-space is discarded as covered.
fn carve_fragment_outside_brush(
    fragment: Vec<glam::DVec3>,
    brush: &BrushVolume,
    clip_epsilon: f64,
) -> Vec<Vec<glam::DVec3>> {
    let mut candidate_inside = fragment;
    let mut outside_fragments = Vec::new();

    for plane in &brush.planes {
        let has_outside_area = candidate_inside
            .iter()
            .any(|vertex| plane.normal.dot(*vertex) - plane.distance > clip_epsilon);
        if has_outside_area {
            if let Some(outside) = clip_winding_to_half_spaces(
                candidate_inside.clone(),
                &[(plane.normal, plane.distance)],
                clip_epsilon,
            ) {
                outside_fragments.push(outside);
            }
        }

        let Some(inside) = clip_winding_to_half_spaces(
            candidate_inside,
            &[(-plane.normal, -plane.distance)],
            clip_epsilon,
        ) else {
            return outside_fragments;
        };
        candidate_inside = inside;
    }

    // The remaining candidate is inside every brush plane, hence covered.
    outside_fragments
}

fn polygon_area(vertices: &[glam::DVec3]) -> f64 {
    if vertices.len() < 3 {
        return 0.0;
    }

    let mut area = glam::DVec3::ZERO;
    let first = vertices[0];
    for index in 1..vertices.len() - 1 {
        area += (vertices[index] - first).cross(vertices[index + 1] - first);
    }
    area.length() * 0.5
}

#[cfg(test)]
mod tests {
    use glam::DVec3;

    use super::*;
    use crate::map_data::{
        BrushPlane, CarriedLightLink, FalloffModel, KinematicMoveMode, LightType, MapLight,
        ShadowType,
    };
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

    fn point_light(bake_only: bool) -> MapLight {
        MapLight {
            origin: DVec3::ZERO,
            light_type: LightType::Point,
            carrier: String::new(),
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 10.0,
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            bake_only,
            is_dynamic: !bake_only,
            casts_entity_shadows: false,
            is_animated: false,
            tags: Vec::new(),
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    fn portal_at_x(x: f64) -> Portal {
        portal_at_x_with_y_range(x, -1.0, 1.0)
    }

    fn portal_at_x_with_y_range(x: f64, min_y: f64, max_y: f64) -> Portal {
        Portal {
            polygon: vec![
                DVec3::new(x, min_y, -1.0),
                DVec3::new(x, max_y, -1.0),
                DVec3::new(x, max_y, 1.0),
                DVec3::new(x, min_y, 1.0),
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
    fn flush_two_brush_door_union_covers_portal() {
        let door = mover(vec![
            box_brush(DVec3::new(-0.1, -2.0, -2.0), DVec3::new(0.1, 0.0, 2.0)),
            box_brush(DVec3::new(-0.1, 0.0, -2.0), DVec3::new(0.1, 2.0, 2.0)),
        ]);

        assert_eq!(
            sealed_portal_ids_for_mover(&door, &[portal_at_x(0.0)]),
            vec![0]
        );
    }

    #[test]
    fn two_brush_door_with_real_seam_gap_does_not_cover_portal() {
        let door = mover(vec![
            box_brush(DVec3::new(-0.1, -2.0, -2.0), DVec3::new(0.1, -0.1, 2.0)),
            box_brush(DVec3::new(-0.1, 0.1, -2.0), DVec3::new(0.1, 2.0, 2.0)),
        ]);

        assert!(sealed_portal_ids_for_mover(&door, &[portal_at_x(0.0)]).is_empty());
    }

    #[test]
    fn partial_single_brush_overlap_does_not_cover_portal() {
        let door = mover(vec![box_brush(
            DVec3::new(-0.1, -2.0, -2.0),
            DVec3::new(0.1, 0.0, 2.0),
        )]);

        assert!(sealed_portal_ids_for_mover(&door, &[portal_at_x(0.0)]).is_empty());
    }

    #[test]
    fn mover_records_every_portal_covered_by_its_brush_union() {
        let door = mover(vec![box_brush(
            DVec3::new(-0.1, -4.0, -2.0),
            DVec3::new(0.1, 4.0, 2.0),
        )]);
        let portals = vec![
            portal_at_x_with_y_range(0.0, -3.0, -1.0),
            portal_at_x(2.0),
            portal_at_x_with_y_range(0.0, 1.0, 3.0),
        ];

        assert_eq!(sealed_portal_ids_for_mover(&door, &portals), vec![0, 2]);
    }

    #[test]
    fn invalid_portal_or_brush_does_not_cover() {
        let valid_portal = portal_at_x(0.0);
        let mut flat_brush = box_brush(DVec3::new(-0.1, -2.0, -2.0), DVec3::new(0.1, 2.0, 2.0));
        flat_brush.aabb.max.z = flat_brush.aabb.min.z;

        assert!(sealed_portal_ids_for_mover(&mover(vec![flat_brush]), &[valid_portal]).is_empty());

        let invalid_portal = Portal {
            polygon: vec![
                DVec3::new(0.0, -1.0, 0.0),
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(0.0, 1.0, 0.0),
            ],
            front_leaf: 0,
            back_leaf: 1,
        };
        let valid_door = mover(vec![box_brush(
            DVec3::new(-0.1, -2.0, -2.0),
            DVec3::new(0.1, 2.0, 2.0),
        )]);

        assert!(sealed_portal_ids_for_mover(&valid_door, &[invalid_portal]).is_empty());
    }

    #[test]
    fn v5_encoding_is_deterministic_and_portal_ids_are_ascending_unique() {
        let door = mover(vec![box_brush(
            DVec3::new(-0.1, -4.0, -2.0),
            DVec3::new(0.1, 4.0, 2.0),
        )]);
        let portals = vec![
            portal_at_x_with_y_range(0.0, -3.0, -1.0),
            portal_at_x(2.0),
            portal_at_x_with_y_range(0.0, 1.0, 3.0),
        ];
        let mut first_textures = TextureNamesSection { names: Vec::new() };
        let first = encode_kinematic_geometry_section(
            &[door.clone()],
            &[],
            &[],
            &portals,
            &mut first_textures,
        )
        .expect("mover must produce a kinematic section");
        let mut second_textures = TextureNamesSection { names: Vec::new() };
        let second =
            encode_kinematic_geometry_section(&[door], &[], &[], &portals, &mut second_textures)
                .expect("mover must produce a kinematic section");

        let sealed_ids = &first.movers[0].sealed_portal_ids;
        assert_eq!(sealed_ids, &[0, 2]);
        assert!(sealed_ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(first.to_bytes(), second.to_bytes());
    }

    #[test]
    fn member_lights_use_positional_alpha_light_indices() {
        let mover = mover(Vec::new());
        let lights = vec![point_light(true), point_light(false), point_light(false)];
        let alpha_lights = AlphaLightsNs::from_lights(&lights);
        let members = member_lights_by_mover(
            &[mover],
            &[
                CarriedLightLink {
                    source_light_index: 1,
                    mover_id: 7,
                    local_offset: [1.0, 2.0, 3.0],
                },
                CarriedLightLink {
                    source_light_index: 2,
                    mover_id: 7,
                    local_offset: [-1.0, 0.0, 0.5],
                },
            ],
            &alpha_lights,
        );

        assert_eq!(
            members,
            vec![vec![
                MemberLight {
                    alpha_light_index: 0,
                    local_offset: [1.0, 2.0, 3.0],
                },
                MemberLight {
                    alpha_light_index: 1,
                    local_offset: [-1.0, 0.0, 0.5],
                },
            ]]
        );
    }
}
