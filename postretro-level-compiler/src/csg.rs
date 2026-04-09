// CSG face clipping: remove faces inside solid brush volumes.
// See: context/reference/csg-face-clipping.md

use glam::Vec3;

use crate::geometry_utils::split_polygon;
use crate::map_data::{BrushVolume, Face};
use crate::partition::Aabb;

/// Epsilon for polygon splitting during CSG clipping. Small positive value
/// consistent with the Sutherland-Hodgman implementation in geometry_utils.
const CSG_SPLIT_EPSILON: f32 = 0.01;

/// Epsilon for classifying a point as "on a brush plane" during the interior
/// test. A point within this distance of a plane is considered to be on the
/// plane surface, not behind it.
const ON_PLANE_EPSILON: f32 = 0.02;

/// Clip faces against brush volumes, removing geometry inside solid space.
///
/// For each face, tests overlap against every brush (AABB pre-filter), then
/// clips the face polygon against the brush's half-planes. Faces fully inside
/// a brush are discarded. Faces partially inside are clipped to the outside
/// portion.
///
/// Returns a new face list with clipped/discarded faces applied.
pub fn csg_clip_faces(faces: &[Face], brush_volumes: &[BrushVolume]) -> Vec<Face> {
    if brush_volumes.is_empty() {
        return faces.to_vec();
    }

    let mut result = Vec::with_capacity(faces.len());
    let mut clipped_count = 0usize;
    let mut discarded_count = 0usize;

    for face in faces {
        let face_aabb = Aabb::from_points(&face.vertices);

        // Start with a single polygon representing the face.
        let mut surviving_fragments: Vec<Vec<Vec3>> = vec![face.vertices.clone()];

        for brush in brush_volumes {
            if !face_aabb.intersects(&brush.aabb) {
                continue;
            }

            // Clip each surviving fragment against this brush.
            let mut next_surviving = Vec::new();

            for fragment in surviving_fragments {
                let outside_pieces = clip_polygon_outside_brush(&fragment, brush);
                next_surviving.extend(outside_pieces);
            }

            surviving_fragments = next_surviving;

            if surviving_fragments.is_empty() {
                break;
            }
        }

        if surviving_fragments.is_empty() {
            discarded_count += 1;
        } else {
            for fragment in surviving_fragments {
                if fragment.len() < 3 {
                    continue;
                }
                let changed = fragment.len() != face.vertices.len()
                    || fragment
                        .iter()
                        .zip(face.vertices.iter())
                        .any(|(a, b)| (*a - *b).length_squared() > 1e-8);
                if changed {
                    clipped_count += 1;
                }
                result.push(Face {
                    vertices: fragment,
                    normal: face.normal,
                    distance: face.distance,
                    texture: face.texture.clone(),
                });
            }
        }
    }

    log::info!(
        "[Compiler] CSG clip: {} faces in, {} out ({} discarded, {} clipped)",
        faces.len(),
        result.len(),
        discarded_count,
        clipped_count,
    );

    result
}

/// Test if a polygon intersects the brush interior.
///
/// A polygon intersects the brush interior if any portion of it is strictly
/// behind all brush planes (i.e., inside the brush volume).
///
/// First checks if the polygon is entirely on the front side (or on the
/// surface) of any single brush plane. If so, it can't be inside the brush.
/// Then checks if any vertex is strictly inside the brush, and finally does
/// a geometric clip to catch edge-crossing cases.
fn polygon_intersects_brush_interior(vertices: &[Vec3], brush: &BrushVolume) -> bool {
    // Quick check: if all vertices are on the front side of (or on the surface
    // of) any single brush plane, the polygon can't be inside the brush.
    for plane in &brush.planes {
        let all_front_or_on = vertices.iter().all(|v| {
            let d = v.dot(plane.normal) - plane.distance;
            d >= -ON_PLANE_EPSILON
        });
        if all_front_or_on {
            return false;
        }
    }

    // Check if any vertex is strictly inside the brush (behind all planes
    // by more than the on-plane tolerance).
    let any_vertex_inside = vertices.iter().any(|v| {
        brush.planes.iter().all(|plane| {
            let d = v.dot(plane.normal) - plane.distance;
            d < -ON_PLANE_EPSILON
        })
    });

    if any_vertex_inside {
        return true;
    }

    // No vertex is strictly inside, but the polygon might cross through the
    // brush (a large polygon spanning a small brush). Clip to the brush
    // interior to detect this.
    let mut polygon = vertices.to_vec();
    for plane in &brush.planes {
        // Clip to keep the back side (inside the brush). We clip to the front
        // of the negated plane.
        let (front, _) = split_polygon(
            &polygon,
            -plane.normal,
            -plane.distance,
            ON_PLANE_EPSILON,
        );

        match front {
            Some(verts) if verts.len() >= 3 => polygon = verts,
            _ => return false,
        }
    }

    // Check that the remaining polygon has meaningful area (not just
    // degenerate on-plane vertices).
    polygon.len() >= 3
}

/// Clip a polygon against a brush, returning only the portions OUTSIDE the brush.
///
/// First checks if the polygon intersects the brush interior at all. If not,
/// returns the polygon unchanged (avoids unnecessary fragmentation).
///
/// When intersection exists, uses the CSG subtraction algorithm: for each brush
/// plane, split the polygon into front (outside this plane) and back (inside
/// this plane's half-space). The front fragment is saved as an outside piece.
/// The back fragment continues to be tested against remaining planes. After all
/// planes, any remaining back fragment was inside the brush and is discarded.
fn clip_polygon_outside_brush(vertices: &[Vec3], brush: &BrushVolume) -> Vec<Vec<Vec3>> {
    // Quick rejection: if the polygon doesn't intersect the brush interior,
    // return it unchanged.
    if !polygon_intersects_brush_interior(vertices, brush) {
        return vec![vertices.to_vec()];
    }

    let mut outside_pieces = Vec::new();
    let mut inside_candidate = vertices.to_vec();

    for plane in &brush.planes {
        if inside_candidate.len() < 3 {
            return outside_pieces;
        }

        let (front, back) = split_polygon(
            &inside_candidate,
            plane.normal,
            plane.distance,
            CSG_SPLIT_EPSILON,
        );

        match (front, back) {
            (Some(front_verts), Some(back_verts)) => {
                // Polygon spans this plane. The front part is outside this
                // plane's half-space, so it's outside the brush. Save it.
                // The back part might be inside the brush -- continue.
                if front_verts.len() >= 3 {
                    outside_pieces.push(front_verts);
                }
                inside_candidate = back_verts;
            }
            (Some(_), None) => {
                // Polygon is entirely in front of this plane. Can't be inside
                // the brush. Return unchanged.
                return vec![inside_candidate];
            }
            (None, Some(back_verts)) => {
                // Polygon is entirely behind this plane. Continue testing.
                inside_candidate = back_verts;
            }
            (None, None) => {
                // Degenerate split.
                return outside_pieces;
            }
        }
    }

    // Whatever remains in inside_candidate was behind all planes = inside brush.
    // Discard it (don't add to outside_pieces).
    outside_pieces
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_data::{BrushPlane, BrushVolume};

    fn box_brush(min: Vec3, max: Vec3) -> BrushVolume {
        BrushVolume {
            planes: vec![
                BrushPlane {
                    normal: Vec3::X,
                    distance: max.x,
                },
                BrushPlane {
                    normal: Vec3::NEG_X,
                    distance: -min.x,
                },
                BrushPlane {
                    normal: Vec3::Y,
                    distance: max.y,
                },
                BrushPlane {
                    normal: Vec3::NEG_Y,
                    distance: -min.y,
                },
                BrushPlane {
                    normal: Vec3::Z,
                    distance: max.z,
                },
                BrushPlane {
                    normal: Vec3::NEG_Z,
                    distance: -min.z,
                },
            ],
            aabb: Aabb { min, max },
        }
    }

    fn make_face(vertices: Vec<Vec3>, normal: Vec3, distance: f32) -> Face {
        Face {
            vertices,
            normal,
            distance,
            texture: "test".to_string(),
        }
    }

    // -- Basic behavior tests --

    #[test]
    fn face_outside_brush_is_unchanged() {
        let face = make_face(
            vec![
                Vec3::new(10.0, 0.0, 0.0),
                Vec3::new(11.0, 0.0, 0.0),
                Vec3::new(11.0, 1.0, 0.0),
                Vec3::new(10.0, 1.0, 0.0),
            ],
            Vec3::NEG_Z,
            0.0,
        );
        let brush = box_brush(Vec3::ZERO, Vec3::new(5.0, 5.0, 5.0));

        let result = csg_clip_faces(&[face.clone()], &[brush]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].vertices.len(), face.vertices.len());
    }

    #[test]
    fn face_fully_inside_brush_is_discarded() {
        // Face is entirely inside the brush (behind all planes)
        let face = make_face(
            vec![
                Vec3::new(1.0, 1.0, 2.5),
                Vec3::new(4.0, 1.0, 2.5),
                Vec3::new(4.0, 4.0, 2.5),
                Vec3::new(1.0, 4.0, 2.5),
            ],
            Vec3::Z,
            2.5,
        );
        let brush = box_brush(Vec3::ZERO, Vec3::new(5.0, 5.0, 5.0));

        let result = csg_clip_faces(&[face], &[brush]);
        assert_eq!(result.len(), 0, "face inside brush should be discarded");
    }

    #[test]
    fn face_on_brush_boundary_survives() {
        // Face sits exactly on a brush boundary plane (x=5). The face vertices
        // are on the plane, not behind it, so the face is not inside the brush.
        let face = make_face(
            vec![
                Vec3::new(5.0, 0.0, 0.0),
                Vec3::new(5.0, 5.0, 0.0),
                Vec3::new(5.0, 5.0, 5.0),
                Vec3::new(5.0, 0.0, 5.0),
            ],
            Vec3::X,
            5.0,
        );
        let brush = box_brush(Vec3::ZERO, Vec3::new(5.0, 5.0, 5.0));

        let result = csg_clip_faces(&[face], &[brush]);
        assert_eq!(
            result.len(),
            1,
            "face on brush boundary should survive clipping"
        );
    }

    #[test]
    fn face_partially_inside_brush_is_clipped() {
        // Face spans from inside to outside the brush along X.
        // Brush spans [0,5] in X. Face spans [3,7] in X at y=2.5, z=2.5.
        let face = make_face(
            vec![
                Vec3::new(3.0, 1.0, 2.5),
                Vec3::new(7.0, 1.0, 2.5),
                Vec3::new(7.0, 4.0, 2.5),
                Vec3::new(3.0, 4.0, 2.5),
            ],
            Vec3::Z,
            2.5,
        );
        let brush = box_brush(Vec3::ZERO, Vec3::new(5.0, 5.0, 5.0));

        let result = csg_clip_faces(&[face], &[brush]);

        assert!(
            !result.is_empty(),
            "partially inside face should have surviving fragments"
        );

        // The surviving portion should be the part outside the brush
        let total_area: f32 = result.iter().map(|f| polygon_area(&f.vertices)).sum();
        // Original outside area: x=[5,7], y=[1,4] -> 2 * 3 = 6
        assert!(
            (total_area - 6.0).abs() < 0.5,
            "outside area should be ~6.0, got {total_area}"
        );
    }

    #[test]
    fn non_overlapping_brushes_produce_no_change() {
        let face = make_face(
            vec![
                Vec3::new(100.0, 0.0, 0.0),
                Vec3::new(101.0, 0.0, 0.0),
                Vec3::new(101.0, 1.0, 0.0),
            ],
            Vec3::NEG_Z,
            0.0,
        );
        let brush = box_brush(Vec3::ZERO, Vec3::new(5.0, 5.0, 5.0));

        let result = csg_clip_faces(&[face.clone()], &[brush]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].vertices.len(), face.vertices.len());
    }

    #[test]
    fn empty_brush_list_is_noop() {
        let face = make_face(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
            ],
            Vec3::NEG_Z,
            0.0,
        );

        let result = csg_clip_faces(&[face.clone()], &[]);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn shared_wall_between_adjacent_brushes_survives() {
        // Two adjacent box brushes sharing a wall at x=5.
        // Both boundary faces should survive: they're on brush surfaces.
        let brush_a = box_brush(Vec3::ZERO, Vec3::new(5.0, 5.0, 5.0));
        let brush_b = box_brush(Vec3::new(5.0, 0.0, 0.0), Vec3::new(10.0, 5.0, 5.0));

        let face_a_right = make_face(
            vec![
                Vec3::new(5.0, 0.0, 0.0),
                Vec3::new(5.0, 0.0, 5.0),
                Vec3::new(5.0, 5.0, 5.0),
                Vec3::new(5.0, 5.0, 0.0),
            ],
            Vec3::X,
            5.0,
        );

        let face_b_left = make_face(
            vec![
                Vec3::new(5.0, 0.0, 0.0),
                Vec3::new(5.0, 5.0, 0.0),
                Vec3::new(5.0, 5.0, 5.0),
                Vec3::new(5.0, 0.0, 5.0),
            ],
            Vec3::NEG_X,
            -5.0,
        );

        let faces = vec![face_a_right, face_b_left];
        let brushes = vec![brush_a, brush_b];

        let result = csg_clip_faces(&faces, &brushes);
        assert_eq!(
            result.len(),
            2,
            "boundary faces should survive CSG clipping"
        );
    }

    #[test]
    fn face_metadata_preserved_after_clipping() {
        let face = make_face(
            vec![
                Vec3::new(3.0, 1.0, 2.5),
                Vec3::new(7.0, 1.0, 2.5),
                Vec3::new(7.0, 4.0, 2.5),
                Vec3::new(3.0, 4.0, 2.5),
            ],
            Vec3::Z,
            2.5,
        );
        let brush = box_brush(Vec3::ZERO, Vec3::new(5.0, 5.0, 5.0));

        let result = csg_clip_faces(&[face.clone()], &[brush]);
        for f in &result {
            assert_eq!(f.normal, face.normal);
            assert_eq!(f.distance, face.distance);
            assert_eq!(f.texture, face.texture);
        }
    }

    #[test]
    fn multiple_brushes_clip_cumulatively() {
        // Face spans two brushes. Each brush should clip its portion.
        let brush_a = box_brush(Vec3::ZERO, Vec3::new(3.0, 5.0, 5.0));
        let brush_b = box_brush(Vec3::new(7.0, 0.0, 0.0), Vec3::new(10.0, 5.0, 5.0));

        // Face at z=2.5, spanning x=[0,10], y=[1,4]
        let face = make_face(
            vec![
                Vec3::new(0.0, 1.0, 2.5),
                Vec3::new(10.0, 1.0, 2.5),
                Vec3::new(10.0, 4.0, 2.5),
                Vec3::new(0.0, 4.0, 2.5),
            ],
            Vec3::Z,
            2.5,
        );

        let result = csg_clip_faces(&[face], &[brush_a, brush_b]);

        // The middle portion (x=[3,7]) should survive
        let total_area: f32 = result.iter().map(|f| polygon_area(&f.vertices)).sum();
        // Expected: x=[3,7], y=[1,4] -> 4*3 = 12
        assert!(
            (total_area - 12.0).abs() < 1.0,
            "middle portion area should be ~12.0, got {total_area}"
        );
    }

    #[test]
    fn integration_with_test_map() {
        let map_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("assets/maps/test.map");

        let map_data =
            crate::parse::parse_map_file(&map_path, crate::map_format::MapFormat::IdTech2)
                .expect("test.map should parse");

        let clipped = csg_clip_faces(&map_data.world_faces, &map_data.brush_volumes);

        // Clipped face count should be <= original (some faces may be removed)
        assert!(
            clipped.len() <= map_data.world_faces.len() * 2,
            "clipped count ({}) should not explode relative to original ({})",
            clipped.len(),
            map_data.world_faces.len(),
        );

        // All clipped faces should have valid geometry
        for face in &clipped {
            assert!(face.vertices.len() >= 3);
            assert!(face.normal.length() > 0.5);
            // No NaN vertices
            for v in &face.vertices {
                assert!(v.x.is_finite(), "vertex x is not finite");
                assert!(v.y.is_finite(), "vertex y is not finite");
                assert!(v.z.is_finite(), "vertex z is not finite");
            }
        }

        // The full pipeline should still work with clipped faces
        let result = crate::partition::partition(clipped, &map_data.brush_volumes)
            .expect("partition should succeed on clipped faces");

        let section = crate::geometry::extract_geometry(&result.faces, &result.tree);
        assert!(!section.faces.is_empty(), "should produce geometry");
    }

    // -- Helper --

    fn polygon_area(vertices: &[Vec3]) -> f32 {
        if vertices.len() < 3 {
            return 0.0;
        }
        let mut area = Vec3::ZERO;
        for i in 1..vertices.len() - 1 {
            let v0 = vertices[0];
            let v1 = vertices[i];
            let v2 = vertices[i + 1];
            area += (v1 - v0).cross(v2 - v0);
        }
        area.length() * 0.5
    }
}
