// .map file parsing via shambler: brush classification and face extraction.
// See: context/lib/build_pipeline.md §PRL Compilation

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use glam::DVec3;
use shambler::GeoMap;
use shambler::brush::{BrushId, brush_hulls};
use shambler::entity::EntityId;
use shambler::face::face_planes;
use shambler::face::{FaceWinding, face_centers, face_indices, face_vertices};

use crate::map_data::{BrushPlane, BrushVolume, EntityInfo, Face, MapData, TextureProjection};
use crate::map_format::MapFormat;

/// Convert a shambler nalgebra Vector3 to glam DVec3.
///
/// This is the **input precision boundary**: shambler stores coordinates as f32
/// (parsed from the .map text), and we widen them to f64 here. All subsequent
/// compile-time geometry is computed in double precision.
fn shambler_to_dvec3(v: &shambler::Vector3) -> DVec3 {
    DVec3::new(v.x as f64, v.y as f64, v.z as f64)
}

/// Swizzle a direction vector from Quake coordinates (right-handed, Z-up) to
/// engine coordinates (right-handed, Y-up). For use on normals and other
/// direction vectors — does NOT apply unit scale.
///
/// Quake: +X forward, +Y left, +Z up
/// Engine: +X right, +Y up, -Z forward
///
/// engine_x = -quake_y, engine_y = quake_z, engine_z = -quake_x
///
/// For positions and plane distances, also multiply by `MapFormat::units_to_meters()`
/// after swizzling. Normals are direction vectors — scale must not be applied
/// to them (only the swizzle).
fn quake_to_engine(v: DVec3) -> DVec3 {
    DVec3::new(-v.y, v.z, -v.x)
}

/// Parse an origin string like "-192 25.6 167.736" into a DVec3.
///
/// Parses directly to f64 — no precision cast from f32.
fn parse_origin(s: &str) -> Option<DVec3> {
    let parts: Vec<f64> = s
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    if parts.len() == 3 {
        Some(DVec3::new(parts[0], parts[1], parts[2]))
    } else {
        None
    }
}

/// Look up a property value by key from shambler's entity properties.
fn get_property(geo_map: &GeoMap, entity_id: &EntityId, key: &str) -> Option<String> {
    let props = geo_map.entity_properties.get(entity_id)?;
    props.iter().find(|p| p.key == key).map(|p| p.value.clone())
}

/// Read and parse a .map file, classify brushes, and extract face geometry.
///
/// The `format` parameter identifies the source map format. Its `units_to_meters()`
/// scale is applied at this boundary alongside the axis swizzle: vertex positions,
/// entity origins, and plane distances are all converted to engine meters here.
/// All downstream stages receive engine-native coordinates and meters.
pub fn parse_map_file(path: &Path, format: MapFormat) -> Result<MapData> {
    let scale = format.units_to_meters();
    let map_text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read map file: {}", path.display()))?;

    let shalrath_map: shambler::shalrath::repr::Map = map_text
        .parse()
        .map_err(|e| anyhow::anyhow!("failed to parse .map syntax: {e}"))?;

    let geo_map = GeoMap::new(shalrath_map);

    // Identify worldspawn entity
    let worldspawn_id = geo_map
        .entities
        .iter()
        .find(|id| get_property(&geo_map, id, "classname").as_deref() == Some("worldspawn"))
        .copied()
        .context("no worldspawn entity found in .map file")?;

    // Classify brushes: world vs entity
    let world_brush_ids: Vec<BrushId> = geo_map
        .entity_brushes
        .get(&worldspawn_id)
        .cloned()
        .unwrap_or_default();

    // Collect entity info and entity brush counts
    let mut entities = Vec::new();
    let mut entity_brushes_summary = Vec::new();
    let mut entity_classnames: Vec<String> = Vec::new();

    for entity_id in geo_map.entities.iter() {
        let classname =
            get_property(&geo_map, entity_id, "classname").unwrap_or_else(|| "unknown".to_string());
        // Swizzle axes then apply unit scale: origin is a position, not a direction.
        let origin = get_property(&geo_map, entity_id, "origin")
            .and_then(|s| parse_origin(&s))
            .map(|v| quake_to_engine(v) * scale);

        entities.push(EntityInfo {
            classname: classname.clone(),
            origin,
        });

        if !entity_classnames.contains(&classname) {
            entity_classnames.push(classname.clone());
        }

        if *entity_id != worldspawn_id {
            let brush_count = geo_map
                .entity_brushes
                .get(entity_id)
                .map(|v| v.len())
                .unwrap_or(0);
            entity_brushes_summary.push((classname, brush_count));
        }
    }

    // Compute geometry for world brushes only
    let geo_planes = face_planes(&geo_map.face_planes);

    // Build brush_faces subset for world brushes only
    let world_brush_faces: BTreeMap<BrushId, Vec<shambler::face::FaceId>> = world_brush_ids
        .iter()
        .filter_map(|bid| {
            geo_map
                .brush_faces
                .get(bid)
                .map(|faces| (*bid, faces.clone()))
        })
        .collect();

    let brush_hulls = brush_hulls(&world_brush_faces, &geo_planes);
    let (face_verts, _face_vert_planes) =
        face_vertices(&world_brush_faces, &geo_planes, &brush_hulls);
    let face_ctrs = face_centers(&face_verts);
    let face_idx = face_indices(
        &geo_map.face_planes,
        &geo_planes,
        &face_verts,
        &face_ctrs,
        // Shambler's FaceWinding naming is relative to the solid interior of the brush.
        // FaceWinding::Clockwise produces ascending-angle (CCW-from-front) vertex order,
        // which is what wgpu FrontFace::Ccw requires.
        FaceWinding::Clockwise,
    );

    // Extract brush volumes and faces together so every face can carry the
    // index of its source brush in `brush_volumes`. Face ownership is how
    // downstream leaf solidity classification knows which side of the brush
    // a leaf lies on (face normals point outward from the source brush).
    let mut brush_volumes: Vec<BrushVolume> = Vec::new();
    let mut world_faces: Vec<Face> = Vec::new();
    let mut total_vertex_count: usize = 0;

    for brush_id in &world_brush_ids {
        let face_ids = match geo_map.brush_faces.get(brush_id) {
            Some(ids) => ids,
            None => continue,
        };

        let planes: Vec<BrushPlane> = face_ids
            .iter()
            .filter_map(|fid| {
                let plane = geo_planes.get(fid)?;
                // Normal: swizzle only — normals are direction vectors, scale must not be applied.
                // Distance: scaled explicitly. A plane n·x = d in Quake units becomes
                // n·x' = d * scale where x' is in meters. Scale and swizzle are independent
                // for this scalar; the swizzle is already embedded in `normal`.
                Some(BrushPlane {
                    normal: quake_to_engine(shambler_to_dvec3(plane.normal())),
                    distance: plane.distance() as f64 * scale,
                })
            })
            .collect();

        if planes.is_empty() {
            // No valid volume for this brush — skip both the volume and its
            // faces. Faces without a brush volume have no owner, and the
            // solidity classifier can't reason about them.
            continue;
        }

        // Compute AABB from this brush's face vertices (already in engine space).
        let mut aabb = crate::partition::Aabb::empty();
        for fid in face_ids {
            if let Some(verts) = face_verts.get(fid) {
                for v in verts {
                    aabb.expand_point(quake_to_engine(shambler_to_dvec3(v)) * scale);
                }
            }
        }

        let brush_index = brush_volumes.len();
        brush_volumes.push(BrushVolume { planes, aabb });

        for face_id in face_ids {
            let vertices_raw = match face_verts.get(face_id) {
                Some(v) => v,
                None => continue,
            };

            // Skip degenerate faces
            if vertices_raw.len() < 3 {
                continue;
            }

            let indices = match face_idx.get(face_id) {
                Some(i) => i,
                None => continue,
            };

            // Reorder vertices by winding indices; swizzle axes and apply unit scale.
            // Vertices are positions — both the axis swizzle and the meter scale apply.
            let vertices: Vec<DVec3> = indices
                .iter()
                .map(|&i| quake_to_engine(shambler_to_dvec3(&vertices_raw[i])) * scale)
                .collect();

            let plane = &geo_planes[face_id];
            // Normal: swizzle only — direction vector, no unit scale.
            let normal = quake_to_engine(shambler_to_dvec3(plane.normal()));
            // Distance: scaled explicitly. A plane n·x = d in Quake units becomes
            // n·x' = d * scale where x' is in meters.
            let distance = plane.distance() as f64 * scale;

            // Look up texture name
            let texture = geo_map
                .face_textures
                .get(face_id)
                .and_then(|tex_id| geo_map.textures.get(tex_id))
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());

            // Extract texture projection data (Quake space).
            let face_offset = geo_map.face_offsets.get(face_id).copied();
            let face_angle = geo_map.face_angles.get(face_id).copied().unwrap_or(0.0) as f64;
            let face_scale = geo_map.face_scales.get(face_id);

            let (scale_u, scale_v) = face_scale
                .map(|s| (s.x as f64, s.y as f64))
                .unwrap_or((1.0, 1.0));

            let tex_projection = match face_offset {
                Some(shambler::shalrath::repr::TextureOffset::Valve { u, v }) => {
                    TextureProjection::Valve {
                        u_axis: DVec3::new(u.x as f64, u.y as f64, u.z as f64),
                        u_offset: u.d as f64,
                        v_axis: DVec3::new(v.x as f64, v.y as f64, v.z as f64),
                        v_offset: v.d as f64,
                        scale_u,
                        scale_v,
                    }
                }
                Some(shambler::shalrath::repr::TextureOffset::Standard { u, v }) => {
                    TextureProjection::Standard {
                        u_offset: u as f64,
                        v_offset: v as f64,
                        angle: face_angle,
                        scale_u,
                        scale_v,
                    }
                }
                None => TextureProjection::Standard {
                    u_offset: 0.0,
                    v_offset: 0.0,
                    angle: 0.0,
                    scale_u: 1.0,
                    scale_v: 1.0,
                },
            };

            total_vertex_count += vertices.len();

            world_faces.push(Face {
                vertices,
                normal,
                distance,
                texture,
                tex_projection,
                brush_index,
            });
        }
    }

    // Stat logging
    let total_brushes = geo_map.brushes.len();
    let world_brush_count = world_brush_ids.len();
    let entity_brush_count = total_brushes - world_brush_count;

    log::info!("[Compiler] Total brushes: {total_brushes}");
    log::info!("[Compiler] World brushes: {world_brush_count}");
    log::info!("[Compiler] Entity brushes: {entity_brush_count}");
    log::info!("[Compiler] World faces: {}", world_faces.len());
    log::info!("[Compiler] Total vertices: {total_vertex_count}");
    log::info!(
        "[Compiler] Entity classnames: {}",
        entity_classnames.join(", ")
    );

    Ok(MapData {
        world_faces,
        brush_volumes,
        entity_brushes: entity_brushes_summary,
        entities,
    })
}

/// Re-export `quake_to_engine` for cross-module tests (geometry round-trip).
#[cfg(test)]
pub fn quake_to_engine_for_test(v: DVec3) -> DVec3 {
    quake_to_engine(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // -- Coordinate transform (axis swizzle only) --
    // These tests verify the swizzle in isolation; they do not include the unit
    // scale because `quake_to_engine` is a direction-vector transform used for
    // normals as well as positions. Positions are scaled separately via
    // `MapFormat::units_to_meters()`.

    #[test]
    fn quake_to_engine_z_up_maps_to_y_up() {
        // Quake Z-up → engine Y-up (swizzle only)
        let result = quake_to_engine(DVec3::new(0.0, 0.0, 1.0));
        assert_eq!(result, DVec3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn quake_to_engine_x_forward_maps_to_negative_z_forward() {
        // Quake +X forward → engine -Z forward (swizzle only)
        let result = quake_to_engine(DVec3::new(1.0, 0.0, 0.0));
        assert_eq!(result, DVec3::new(0.0, 0.0, -1.0));
    }

    #[test]
    fn quake_to_engine_y_left_maps_to_negative_x() {
        // Quake +Y left → engine -X (swizzle only)
        let result = quake_to_engine(DVec3::new(0.0, 1.0, 0.0));
        assert_eq!(result, DVec3::new(-1.0, 0.0, 0.0));
    }

    // -- Unit scale (position transform = swizzle + scale) --

    #[test]
    fn position_transform_z_up_scales_to_meters() {
        // A point at Quake Z=1 (1 inch up) → engine Y = 0.0254 m
        let scale = MapFormat::IdTech2.units_to_meters();
        let result = quake_to_engine(DVec3::new(0.0, 0.0, 1.0)) * scale;
        assert!(
            (result.y - 0.0254).abs() < 1e-6,
            "expected y=0.0254, got {}",
            result.y
        );
        assert!(result.x.abs() < 1e-6);
        assert!(result.z.abs() < 1e-6);
    }

    #[test]
    fn plane_distance_scales_to_meters() {
        // A face plane with Quake distance 64.0 → engine distance 1.6256 m (64 × 0.0254)
        let scale = MapFormat::IdTech2.units_to_meters();
        let quake_distance: f64 = 64.0;
        let engine_distance = quake_distance * scale;
        assert!(
            (engine_distance - 1.6256).abs() < 1e-5,
            "expected 1.6256, got {engine_distance}"
        );
    }

    // -- Map parsing --

    fn test_map_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("assets/maps/test.map")
    }

    #[test]
    fn parses_test_map() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("test.map should parse without error");

        // The test map has 10 world brushes
        assert!(!map_data.world_faces.is_empty(), "should have world faces");
    }

    #[test]
    fn classifies_brushes_correctly() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("test.map should parse without error");

        // info_player_start has 0 brushes
        assert!(
            map_data.entity_brushes.iter().all(|(_, count)| *count == 0),
            "info_player_start should have 0 brushes"
        );

        // Should have worldspawn + info_player_start
        let classnames: Vec<&str> = map_data
            .entities
            .iter()
            .map(|e| e.classname.as_str())
            .collect();
        assert!(classnames.contains(&"worldspawn"));
        assert!(classnames.contains(&"info_player_start"));
    }

    #[test]
    fn faces_have_valid_vertices() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("test.map should parse without error");

        for face in &map_data.world_faces {
            assert!(
                face.vertices.len() >= 3,
                "face should have at least 3 vertices, got {}",
                face.vertices.len()
            );
        }
    }

    #[test]
    fn faces_have_unit_normals() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("test.map should parse without error");

        for face in &map_data.world_faces {
            let len = face.normal.length();
            assert!(
                (len - 1.0).abs() < 0.01,
                "normal should be unit length, got {len}"
            );
        }
    }

    #[test]
    fn extracts_player_start_origin() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("test.map should parse without error");

        let player_start = map_data
            .entities
            .iter()
            .find(|e| e.classname == "info_player_start")
            .expect("should have info_player_start");

        let origin = player_start
            .origin
            .expect("info_player_start should have origin");
        assert!(origin.x.is_finite(), "origin x should be finite");
        assert!(origin.y.is_finite(), "origin y should be finite");
        assert!(origin.z.is_finite(), "origin z should be finite");
    }

    #[test]
    fn missing_file_returns_error() {
        let result = parse_map_file(Path::new("nonexistent.map"), MapFormat::IdTech2);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("failed to read"),
            "error should mention file reading, got: {msg}"
        );
    }

    /// Vertex winding: for every parsed face, the first triangle's geometric normal
    /// (cross product of the first two edges) must align with the stored face normal.
    ///
    /// This locks in the FaceWinding convention: vertices should appear CCW when
    /// viewed from the front (from the direction of the face's outward normal), which
    /// is what wgpu FrontFace::Ccw requires.
    #[test]
    fn face_vertex_winding_aligns_with_face_normal() {
        let map_data = parse_map_file(&test_map_path(), MapFormat::IdTech2)
            .expect("test.map should parse without error");

        let mut checked = 0usize;
        for (i, face) in map_data.world_faces.iter().enumerate() {
            if face.vertices.len() < 3 {
                continue;
            }
            let v0 = face.vertices[0];
            let v1 = face.vertices[1];
            let v2 = face.vertices[2];
            let edge1 = v1 - v0;
            let edge2 = v2 - v0;
            let geometric_normal = edge1.cross(edge2);

            // Skip degenerate triangles (collinear vertices).
            if geometric_normal.length_squared() < 1e-10 {
                continue;
            }

            let dot = geometric_normal.dot(face.normal);
            assert!(
                dot > 0.0,
                "face {i}: geometric normal {geometric_normal:?} is opposite to face normal {:?} \
                 (dot={dot:.4}); vertex winding is backwards",
                face.normal
            );
            checked += 1;
        }

        assert!(checked > 0, "no faces were checked — test is vacuous");
    }
}
