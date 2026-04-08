// Geometry extraction: fan-triangulate faces, build vertex/index buffers.
// See: context/lib/build_pipeline.md §PRL

use postretro_level_format::geometry::{FaceMeta, GeometrySection};

use crate::map_data::Face;
use crate::partition::BspTree;

/// Fan-triangulate faces and build a `GeometrySection` with faces ordered by
/// empty BSP leaf.
///
/// Coordinates are expected to be in engine space (Y-up) -- the Quake-to-engine
/// transform is applied earlier, at the parse boundary in `parse.rs`.
///
/// Only empty leaves contribute geometry. Solid leaves are skipped. The
/// `leaf_index` field in `FaceMeta` stores the sequential index among empty
/// leaves (not the raw leaf index in the BSP tree).
pub fn extract_geometry(faces: &[Face], tree: &BspTree) -> GeometrySection {
    if faces.is_empty() {
        return GeometrySection {
            vertices: Vec::new(),
            indices: Vec::new(),
            faces: Vec::new(),
        };
    }

    // Build a face ordering sorted by empty-leaf index. Each entry is
    // (face_index, empty_leaf_sequential_index).
    let ordered_faces = build_leaf_ordered_faces(tree);

    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut face_metas: Vec<FaceMeta> = Vec::new();

    for &(face_idx, leaf_seq_idx) in &ordered_faces {
        let face = &faces[face_idx];

        let base_vertex = vertices.len() as u32;

        // Vertices are already in engine space (transform applied at parse boundary)
        for &v in &face.vertices {
            vertices.push([v.x, v.y, v.z]);
        }

        // Fan-triangulate: (0, 1, 2), (0, 2, 3), ..., (0, n-2, n-1)
        let index_offset = indices.len() as u32;
        let vert_count = face.vertices.len();
        for i in 1..vert_count.saturating_sub(1) {
            indices.push(base_vertex);
            indices.push(base_vertex + i as u32);
            indices.push(base_vertex + i as u32 + 1);
        }
        let index_count = (indices.len() as u32) - index_offset;

        face_metas.push(FaceMeta {
            index_offset,
            index_count,
            leaf_index: leaf_seq_idx as u32,
        });
    }

    GeometrySection {
        vertices,
        indices,
        faces: face_metas,
    }
}

/// Build a list of (face_index, empty_leaf_sequential_index) pairs ordered by
/// sequential empty-leaf index.
///
/// Iterates BSP leaves in order, skipping solid leaves. Each empty leaf gets a
/// sequential index (0, 1, 2, ...) used as the `leaf_index` in face metadata.
fn build_leaf_ordered_faces(tree: &BspTree) -> Vec<(usize, usize)> {
    let capacity: usize = tree
        .leaves
        .iter()
        .filter(|l| !l.is_solid)
        .map(|l| l.face_indices.len())
        .sum();
    let mut ordered = Vec::with_capacity(capacity);

    let mut empty_leaf_idx = 0usize;
    for leaf in &tree.leaves {
        if leaf.is_solid {
            continue;
        }
        for &face_idx in &leaf.face_indices {
            ordered.push((face_idx, empty_leaf_idx));
        }
        empty_leaf_idx += 1;
    }

    ordered
}

/// Log geometry extraction statistics.
pub fn log_stats(section: &GeometrySection, empty_leaf_count: usize) {
    let triangle_count = section.indices.len() / 3;
    log::info!("[Compiler] Vertices: {}", section.vertices.len());
    log::info!("[Compiler] Indices: {}", section.indices.len());
    log::info!("[Compiler] Triangles: {triangle_count}");
    log::info!("[Compiler] Faces: {}", section.faces.len());
    log::info!("[Compiler] Empty leaves: {empty_leaf_count}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::{Aabb, BspLeaf};
    use glam::Vec3;

    fn triangle_face() -> Face {
        Face {
            vertices: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
            normal: Vec3::Z,
            distance: 0.0,
            texture: "test".to_string(),
        }
    }

    fn quad_face() -> Face {
        Face {
            vertices: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
            normal: Vec3::Z,
            distance: 0.0,
            texture: "test".to_string(),
        }
    }

    fn pentagon_face() -> Face {
        Face {
            vertices: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(1.5, 1.0, 0.0),
                Vec3::new(0.5, 1.5, 0.0),
                Vec3::new(-0.5, 1.0, 0.0),
            ],
            normal: Vec3::Z,
            distance: 0.0,
            texture: "test".to_string(),
        }
    }

    fn hexagon_face() -> Face {
        Face {
            vertices: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(1.5, 0.5, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(-0.5, 0.5, 0.0),
            ],
            normal: Vec3::Z,
            distance: 0.0,
            texture: "test".to_string(),
        }
    }

    fn make_tree_with_empty_leaves(leaves: Vec<(Vec<usize>, bool)>) -> BspTree {
        let bsp_leaves: Vec<BspLeaf> = leaves
            .into_iter()
            .map(|(face_indices, is_solid)| BspLeaf {
                face_indices,
                bounds: Aabb {
                    min: Vec3::ZERO,
                    max: Vec3::ONE,
                },
                is_solid,
            })
            .collect();

        BspTree {
            nodes: Vec::new(),
            leaves: bsp_leaves,
        }
    }

    // -- Fan triangulation tests --

    #[test]
    fn triangulate_triangle_produces_one_triangle() {
        let faces = vec![triangle_face()];
        let tree = make_tree_with_empty_leaves(vec![(vec![0], false)]);

        let section = extract_geometry(&faces, &tree);

        assert_eq!(section.faces.len(), 1);
        assert_eq!(
            section.indices.len(),
            3,
            "triangle should produce 3 indices"
        );
        assert_eq!(section.faces[0].index_count, 3);
    }

    #[test]
    fn triangulate_quad_produces_two_triangles() {
        let faces = vec![quad_face()];
        let tree = make_tree_with_empty_leaves(vec![(vec![0], false)]);

        let section = extract_geometry(&faces, &tree);

        assert_eq!(section.indices.len(), 6, "quad should produce 6 indices");
        assert_eq!(section.faces[0].index_count, 6);

        // Verify fan pattern: (0,1,2), (0,2,3)
        assert_eq!(section.indices[0], 0);
        assert_eq!(section.indices[1], 1);
        assert_eq!(section.indices[2], 2);
        assert_eq!(section.indices[3], 0);
        assert_eq!(section.indices[4], 2);
        assert_eq!(section.indices[5], 3);
    }

    #[test]
    fn triangulate_pentagon_produces_three_triangles() {
        let faces = vec![pentagon_face()];
        let tree = make_tree_with_empty_leaves(vec![(vec![0], false)]);

        let section = extract_geometry(&faces, &tree);

        assert_eq!(
            section.indices.len(),
            9,
            "pentagon should produce 9 indices"
        );
        assert_eq!(section.faces[0].index_count, 9);
    }

    #[test]
    fn triangulate_hexagon_produces_four_triangles() {
        let faces = vec![hexagon_face()];
        let tree = make_tree_with_empty_leaves(vec![(vec![0], false)]);

        let section = extract_geometry(&faces, &tree);

        assert_eq!(
            section.indices.len(),
            12,
            "hexagon should produce 12 indices"
        );
        assert_eq!(section.faces[0].index_count, 12);

        // Fan pattern: (0,1,2), (0,2,3), (0,3,4), (0,4,5)
        for tri in 0..4 {
            let base = tri * 3;
            assert_eq!(
                section.indices[base], 0,
                "triangle {tri} should fan from v0"
            );
            assert_eq!(section.indices[base + 1], (tri + 1) as u32);
            assert_eq!(section.indices[base + 2], (tri + 2) as u32);
        }
    }

    // -- Index validity --

    #[test]
    fn indices_are_in_bounds() {
        let faces = vec![quad_face(), pentagon_face()];
        let tree = make_tree_with_empty_leaves(vec![(vec![0, 1], false)]);

        let section = extract_geometry(&faces, &tree);

        let vertex_count = section.vertices.len() as u32;
        for &idx in &section.indices {
            assert!(
                idx < vertex_count,
                "index {idx} out of bounds (vertex count: {vertex_count})"
            );
        }
    }

    // -- Face metadata completeness --

    #[test]
    fn face_index_counts_sum_to_total_indices() {
        let faces = vec![
            triangle_face(),
            quad_face(),
            pentagon_face(),
            hexagon_face(),
        ];
        let tree = make_tree_with_empty_leaves(vec![(vec![0, 1, 2, 3], false)]);

        let section = extract_geometry(&faces, &tree);

        let sum: u32 = section.faces.iter().map(|f| f.index_count).sum();
        assert_eq!(sum, section.indices.len() as u32);
    }

    // -- Vertex passthrough --

    #[test]
    fn vertices_are_passed_through_unchanged() {
        let faces = vec![Face {
            vertices: vec![
                Vec3::new(-2.0, 3.0, -1.0),
                Vec3::new(-5.0, 6.0, -4.0),
                Vec3::new(-8.0, 9.0, -7.0),
            ],
            normal: Vec3::Y,
            distance: 0.0,
            texture: "test".to_string(),
        }];
        let tree = make_tree_with_empty_leaves(vec![(vec![0], false)]);

        let section = extract_geometry(&faces, &tree);

        assert_eq!(section.vertices[0], [-2.0, 3.0, -1.0]);
        assert_eq!(section.vertices[1], [-5.0, 6.0, -4.0]);
        assert_eq!(section.vertices[2], [-8.0, 9.0, -7.0]);
    }

    // -- Leaf ordering --

    #[test]
    fn faces_ordered_by_empty_leaf() {
        let faces = vec![triangle_face(), quad_face(), pentagon_face()];
        let tree = make_tree_with_empty_leaves(vec![
            (vec![0], false),    // empty leaf 0
            (vec![1, 2], false), // empty leaf 1
        ]);

        let section = extract_geometry(&faces, &tree);

        assert_eq!(section.faces.len(), 3);
        assert_eq!(section.faces[0].leaf_index, 0);
        assert_eq!(section.faces[1].leaf_index, 1);
        assert_eq!(section.faces[2].leaf_index, 1);
    }

    #[test]
    fn solid_leaves_skipped_in_geometry() {
        let faces = vec![triangle_face(), quad_face(), pentagon_face()];
        // Leaf 0: solid (face 0 -- skipped)
        // Leaf 1: empty (faces 1, 2 -- included as empty leaf 0)
        let tree = make_tree_with_empty_leaves(vec![
            (vec![0], true),     // solid leaf -- skipped
            (vec![1, 2], false), // empty leaf 0
        ]);

        let section = extract_geometry(&faces, &tree);

        // Only faces from the empty leaf should appear
        assert_eq!(section.faces.len(), 2);
        assert_eq!(section.faces[0].leaf_index, 0);
        assert_eq!(section.faces[1].leaf_index, 0);
    }

    #[test]
    fn leaf_face_ranges_are_contiguous() {
        let faces = vec![
            triangle_face(),
            quad_face(),
            pentagon_face(),
            hexagon_face(),
        ];
        let tree = make_tree_with_empty_leaves(vec![
            (vec![0, 1], false), // empty leaf 0
            (vec![2, 3], false), // empty leaf 1
        ]);

        let section = extract_geometry(&faces, &tree);

        // Empty leaf 0 faces should come before empty leaf 1 faces
        let last_leaf0_idx = section
            .faces
            .iter()
            .rposition(|f| f.leaf_index == 0)
            .unwrap();
        let first_leaf1_idx = section
            .faces
            .iter()
            .position(|f| f.leaf_index == 1)
            .unwrap();
        assert!(last_leaf0_idx < first_leaf1_idx);
    }

    // -- Empty input --

    #[test]
    fn empty_input_produces_empty_output() {
        let faces: Vec<Face> = Vec::new();
        let tree = BspTree {
            nodes: Vec::new(),
            leaves: Vec::new(),
        };
        let section = extract_geometry(&faces, &tree);

        assert!(section.vertices.is_empty());
        assert!(section.indices.is_empty());
        assert!(section.faces.is_empty());
    }

    // -- Every face produces at least one triangle --

    #[test]
    fn every_face_produces_triangles() {
        let faces = vec![
            triangle_face(),
            quad_face(),
            pentagon_face(),
            hexagon_face(),
        ];
        let tree = make_tree_with_empty_leaves(vec![(vec![0, 1, 2, 3], false)]);

        let section = extract_geometry(&faces, &tree);

        for (i, face) in section.faces.iter().enumerate() {
            assert!(
                face.index_count >= 3,
                "face {i} has only {} indices (need at least 3)",
                face.index_count
            );
        }
    }

    // -- Geometry section round-trip --

    #[test]
    fn geometry_section_round_trip() {
        let faces = vec![triangle_face(), quad_face(), pentagon_face()];
        let tree = make_tree_with_empty_leaves(vec![(vec![0], false), (vec![1, 2], false)]);

        let section = extract_geometry(&faces, &tree);
        let bytes = section.to_bytes();
        let restored = GeometrySection::from_bytes(&bytes).unwrap();

        assert_eq!(section, restored);
    }

    // -- Integration test with real map --

    #[test]
    fn extract_from_test_map() {
        let map_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("assets/maps/test.map");

        let map_data = crate::parse::parse_map_file(&map_path, crate::map_format::MapFormat::IdTech2).expect("test.map should parse");

        let result = crate::partition::partition(map_data.world_faces, &map_data.brush_volumes)
            .expect("partition should succeed on test map");

        let section = extract_geometry(&result.faces, &result.tree);

        // Every face should produce triangles
        for face in &section.faces {
            assert!(face.index_count >= 3);
        }

        // All indices in bounds
        let vert_count = section.vertices.len() as u32;
        for &idx in &section.indices {
            assert!(idx < vert_count);
        }

        // Face index counts sum to total
        let sum: u32 = section.faces.iter().map(|f| f.index_count).sum();
        assert_eq!(sum, section.indices.len() as u32);

        // Faces are ordered by leaf index
        let mut prev_leaf = 0u32;
        for face in &section.faces {
            assert!(face.leaf_index >= prev_leaf, "faces not ordered by leaf");
            prev_leaf = face.leaf_index;
        }

        // Round-trip serialization
        let bytes = section.to_bytes();
        let restored = GeometrySection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }
}
