// Binary-only integration coverage for compiler modules that consume parsing
// or the full fixture pipeline. Keeping these at the binary root prevents the
// narrow library target from acquiring the compiler pipeline as a test-only
// dependency.

use std::collections::HashSet;

use glam::DVec3;
use postretro_level_format::geometry::{GeometrySection, NO_TEXTURE};
use postretro_level_format::texture_names::TextureNamesSection;

use crate::geometry::extract_geometry;
use crate::map_format::MapFormat;
use crate::{fixture_pipeline, parse, partition};

fn campaign_test_map() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root")
        .join("content/dev/maps/campaign-test.map")
}

#[test]
fn extract_from_test_map() {
    let map_data = parse::parse_map_file(&campaign_test_map(), MapFormat::IdTech2)
        .expect("campaign-test.map should parse");
    let partition_result = partition::partition(&map_data.brush_volumes)
        .expect("partition should succeed on test map");
    let result = extract_geometry(
        &partition_result.faces,
        &partition_result.tree,
        &HashSet::new(),
    );
    let section = &result.geometry;

    for range in &result.face_index_ranges {
        assert!(range.index_count >= 3);
    }

    let vertex_count = section.vertices.len() as u32;
    for &index in &section.indices {
        assert!(index < vertex_count);
    }

    let index_count: u32 = result
        .face_index_ranges
        .iter()
        .map(|range| range.index_count)
        .sum();
    assert_eq!(index_count, section.indices.len() as u32);

    let mut previous_leaf = 0u32;
    for face in &section.faces {
        assert!(
            face.leaf_index >= previous_leaf,
            "faces not ordered by leaf"
        );
        previous_leaf = face.leaf_index;
    }

    for vertex in &section.vertices {
        assert!(vertex.uv[0].is_finite(), "u should be finite");
        assert!(vertex.uv[1].is_finite(), "v should be finite");
    }

    assert!(
        !result.texture_names.names.is_empty(),
        "should have at least one texture name"
    );
    let texture_count = result.texture_names.names.len() as u32;
    for face in &section.faces {
        assert!(
            face.texture_index < texture_count || face.texture_index == NO_TEXTURE,
            "texture_index {} out of range (count: {texture_count})",
            face.texture_index
        );
    }

    for (index, vertex) in section.vertices.iter().enumerate() {
        let normal = vertex.decode_normal();
        let tangent = vertex.decode_tangent();
        let normal_length =
            (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        assert!(
            (normal_length - 1.0).abs() < 0.01,
            "vertex {index}: normal not unit: {normal_length}"
        );
        let tangent_length =
            (tangent[0] * tangent[0] + tangent[1] * tangent[1] + tangent[2] * tangent[2]).sqrt();
        assert!(
            (tangent_length - 1.0).abs() < 0.01,
            "vertex {index}: tangent not unit: {tangent_length}"
        );
        let dot = normal[0] * tangent[0] + normal[1] * tangent[1] + normal[2] * tangent[2];
        assert!(
            dot.abs() < 0.05,
            "vertex {index}: n.t not perpendicular: {dot}"
        );
    }

    let bytes = section.to_bytes();
    let restored = GeometrySection::from_bytes(&bytes).unwrap();
    assert_eq!(*section, restored);

    let texture_bytes = result.texture_names.to_bytes();
    let restored_textures = TextureNamesSection::from_bytes(&texture_bytes).unwrap();
    assert_eq!(result.texture_names, restored_textures);
}

#[test]
fn partition_with_test_map() {
    let map_data = parse::parse_map_file(&campaign_test_map(), MapFormat::IdTech2)
        .expect("campaign-test.map should parse without error");
    let result = partition::partition(&map_data.brush_volumes)
        .expect("partition should succeed on test map");

    assert!(!result.tree.leaves.is_empty(), "should produce leaves");
    assert!(!result.faces.is_empty(), "should have faces");
    assert!(
        result.tree.leaves.len() <= 1000,
        "too many leaves ({}) — BSP may be exploding",
        result.tree.leaves.len()
    );

    let empty_count = result
        .tree
        .leaves
        .iter()
        .filter(|leaf| !leaf.is_solid)
        .count();
    assert!(empty_count >= 1, "should have at least 1 empty leaf");
}

#[test]
fn partition_with_wedge_fixture_culls_internal_plane_and_bounds_faces() {
    const FACE_CLIP_EPSILON: f64 = 0.1;

    let fixture = fixture_pipeline::load_fixture("wedge-shared-plane");
    let internal_normal = DVec3::new(1.0, 0.0, -1.0).normalize();
    let internal_distance = 0.0;

    for face in &fixture.faces {
        let same_orientation = (face.normal - internal_normal).length_squared() < 1e-8
            && (face.distance - internal_distance).abs() < 1e-4;
        let opposite_orientation = (face.normal + internal_normal).length_squared() < 1e-8
            && (face.distance + internal_distance).abs() < 1e-4;
        assert!(
            !same_orientation && !opposite_orientation,
            "wedge fixture emitted a face on the shared internal plane: normal={:?} distance={}",
            face.normal,
            face.distance
        );
    }

    for (leaf_index, leaf) in fixture.tree.leaves.iter().enumerate() {
        assert!(
            leaf.bounds.is_valid(),
            "leaf {leaf_index} has invalid bounds: min={:?} max={:?}",
            leaf.bounds.min,
            leaf.bounds.max
        );

        for &face_index in &leaf.face_indices {
            let face = &fixture.faces[face_index];
            for &vertex in &face.vertices {
                assert!(
                    vertex.x >= leaf.bounds.min.x - FACE_CLIP_EPSILON
                        && vertex.x <= leaf.bounds.max.x + FACE_CLIP_EPSILON
                        && vertex.y >= leaf.bounds.min.y - FACE_CLIP_EPSILON
                        && vertex.y <= leaf.bounds.max.y + FACE_CLIP_EPSILON
                        && vertex.z >= leaf.bounds.min.z - FACE_CLIP_EPSILON
                        && vertex.z <= leaf.bounds.max.z + FACE_CLIP_EPSILON,
                    "leaf {leaf_index} bounds min={:?} max={:?} do not contain face {face_index} vertex {:?}",
                    leaf.bounds.min,
                    leaf.bounds.max,
                    vertex
                );
            }
        }
    }
}
