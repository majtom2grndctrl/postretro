// Geometry extraction: fan-triangulate faces, build vertex/index buffers.
// See: context/plans/ready/prl-phase-1-minimum-viable-compiler/

use postretro_level_format::geometry::{FaceMeta, GeometrySection};

use crate::map_data::Face;
use crate::partition::Cluster;

/// Fan-triangulate faces and build a `GeometrySection` with faces ordered by cluster.
///
/// Coordinates are expected to be in engine space (Y-up) — the Quake-to-engine
/// transform is applied earlier, at the parse boundary in `parse.rs`.
pub fn extract_geometry(faces: &[Face], clusters: &[Cluster]) -> GeometrySection {
    if faces.is_empty() {
        return GeometrySection {
            vertices: Vec::new(),
            indices: Vec::new(),
            faces: Vec::new(),
        };
    }

    // Build a face ordering sorted by cluster. Each entry is (face_index, cluster_index).
    let ordered_faces = build_cluster_ordered_faces(clusters);

    // Triangulate all faces in cluster order, deduplicating nothing —
    // each face gets its own vertices for correct normals later.
    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut face_metas: Vec<FaceMeta> = Vec::new();

    for &(face_idx, cluster_idx) in &ordered_faces {
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
            cluster_index: cluster_idx as u32,
        });
    }

    GeometrySection {
        vertices,
        indices,
        faces: face_metas,
    }
}

/// Build a list of (face_index, cluster_index) pairs ordered by cluster ID.
fn build_cluster_ordered_faces(clusters: &[Cluster]) -> Vec<(usize, usize)> {
    let capacity: usize = clusters.iter().map(|c| c.face_indices.len()).sum();
    let mut ordered = Vec::with_capacity(capacity);

    // Clusters are already numbered 0..N by the partition step.
    // Sort by cluster.id to guarantee ordering.
    let mut clusters_sorted: Vec<_> = clusters.iter().collect();
    clusters_sorted.sort_by_key(|c| c.id);

    for cluster in clusters_sorted {
        for &face_idx in &cluster.face_indices {
            ordered.push((face_idx, cluster.id));
        }
    }

    ordered
}

/// Log geometry extraction statistics.
pub fn log_stats(section: &GeometrySection, cluster_count: usize) {
    let triangle_count = section.indices.len() / 3;
    log::info!("[Compiler] Vertices: {}", section.vertices.len());
    log::info!("[Compiler] Indices: {}", section.indices.len());
    log::info!("[Compiler] Triangles: {triangle_count}");
    log::info!("[Compiler] Faces: {}", section.faces.len());
    log::info!("[Compiler] Clusters: {cluster_count}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::Aabb;
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

    // -- Fan triangulation tests --

    #[test]
    fn triangulate_triangle_produces_one_triangle() {
        let faces = vec![triangle_face()];
        let clusters = vec![Cluster {
            id: 0,
            bounds: Aabb {
                min: Vec3::ZERO,
                max: Vec3::ONE,
            },
            face_indices: vec![0],
        }];

        let section = extract_geometry(&faces, &clusters);

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
        let clusters = vec![Cluster {
            id: 0,
            bounds: Aabb {
                min: Vec3::ZERO,
                max: Vec3::ONE,
            },
            face_indices: vec![0],
        }];

        let section = extract_geometry(&faces, &clusters);

        assert_eq!(section.indices.len(), 6, "quad should produce 6 indices");
        assert_eq!(section.faces[0].index_count, 6);

        // Verify fan pattern: (0,1,2), (0,2,3)
        assert_eq!(section.indices[0], 0); // v0
        assert_eq!(section.indices[1], 1); // v1
        assert_eq!(section.indices[2], 2); // v2
        assert_eq!(section.indices[3], 0); // v0
        assert_eq!(section.indices[4], 2); // v2
        assert_eq!(section.indices[5], 3); // v3
    }

    #[test]
    fn triangulate_pentagon_produces_three_triangles() {
        let faces = vec![pentagon_face()];
        let clusters = vec![Cluster {
            id: 0,
            bounds: Aabb {
                min: Vec3::ZERO,
                max: Vec3::ONE,
            },
            face_indices: vec![0],
        }];

        let section = extract_geometry(&faces, &clusters);

        assert_eq!(
            section.indices.len(),
            9,
            "pentagon should produce 9 indices"
        );
        assert_eq!(section.faces[0].index_count, 9);

        // Fan pattern: (0,1,2), (0,2,3), (0,3,4)
        assert_eq!(section.indices[0], 0);
        assert_eq!(section.indices[1], 1);
        assert_eq!(section.indices[2], 2);
        assert_eq!(section.indices[3], 0);
        assert_eq!(section.indices[4], 2);
        assert_eq!(section.indices[5], 3);
        assert_eq!(section.indices[6], 0);
        assert_eq!(section.indices[7], 3);
        assert_eq!(section.indices[8], 4);
    }

    #[test]
    fn triangulate_hexagon_produces_four_triangles() {
        let faces = vec![hexagon_face()];
        let clusters = vec![Cluster {
            id: 0,
            bounds: Aabb {
                min: Vec3::ZERO,
                max: Vec3::ONE,
            },
            face_indices: vec![0],
        }];

        let section = extract_geometry(&faces, &clusters);

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
        let clusters = vec![Cluster {
            id: 0,
            bounds: Aabb {
                min: Vec3::ZERO,
                max: Vec3::ONE,
            },
            face_indices: vec![0, 1],
        }];

        let section = extract_geometry(&faces, &clusters);

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
        let clusters = vec![Cluster {
            id: 0,
            bounds: Aabb {
                min: Vec3::ZERO,
                max: Vec3::ONE,
            },
            face_indices: vec![0, 1, 2, 3],
        }];

        let section = extract_geometry(&faces, &clusters);

        let sum: u32 = section.faces.iter().map(|f| f.index_count).sum();
        assert_eq!(sum, section.indices.len() as u32);
    }

    // -- Vertex passthrough --
    // Coordinates arrive in engine space from parse.rs; extract_geometry writes them verbatim.

    #[test]
    fn vertices_are_passed_through_unchanged() {
        // Inputs are already in engine space — geometry must not re-transform them.
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
        let clusters = vec![Cluster {
            id: 0,
            bounds: Aabb {
                min: Vec3::ZERO,
                max: Vec3::ONE,
            },
            face_indices: vec![0],
        }];

        let section = extract_geometry(&faces, &clusters);

        assert_eq!(section.vertices[0], [-2.0, 3.0, -1.0]);
        assert_eq!(section.vertices[1], [-5.0, 6.0, -4.0]);
        assert_eq!(section.vertices[2], [-8.0, 9.0, -7.0]);
    }

    // -- Cluster ordering --

    #[test]
    fn faces_ordered_by_cluster() {
        let faces = vec![triangle_face(), quad_face(), pentagon_face()];
        let clusters = vec![
            Cluster {
                id: 0,
                bounds: Aabb {
                    min: Vec3::ZERO,
                    max: Vec3::ONE,
                },
                face_indices: vec![0],
            },
            Cluster {
                id: 1,
                bounds: Aabb {
                    min: Vec3::ZERO,
                    max: Vec3::ONE,
                },
                face_indices: vec![1, 2],
            },
        ];

        let section = extract_geometry(&faces, &clusters);

        assert_eq!(section.faces.len(), 3);
        assert_eq!(section.faces[0].cluster_index, 0);
        assert_eq!(section.faces[1].cluster_index, 1);
        assert_eq!(section.faces[2].cluster_index, 1);
    }

    #[test]
    fn cluster_face_ranges_are_contiguous() {
        let faces = vec![
            triangle_face(),
            quad_face(),
            pentagon_face(),
            hexagon_face(),
        ];
        let clusters = vec![
            Cluster {
                id: 0,
                bounds: Aabb {
                    min: Vec3::ZERO,
                    max: Vec3::ONE,
                },
                face_indices: vec![0, 1],
            },
            Cluster {
                id: 1,
                bounds: Aabb {
                    min: Vec3::ZERO,
                    max: Vec3::ONE,
                },
                face_indices: vec![2, 3],
            },
        ];

        let section = extract_geometry(&faces, &clusters);

        // Cluster 0 faces should be contiguous at the start
        let cluster_0_faces: Vec<_> = section
            .faces
            .iter()
            .filter(|f| f.cluster_index == 0)
            .collect();
        let cluster_1_faces: Vec<_> = section
            .faces
            .iter()
            .filter(|f| f.cluster_index == 1)
            .collect();

        assert_eq!(cluster_0_faces.len(), 2);
        assert_eq!(cluster_1_faces.len(), 2);

        // All cluster-0 faces come before cluster-1 faces
        let last_c0_idx = section
            .faces
            .iter()
            .rposition(|f| f.cluster_index == 0)
            .unwrap();
        let first_c1_idx = section
            .faces
            .iter()
            .position(|f| f.cluster_index == 1)
            .unwrap();
        assert!(last_c0_idx < first_c1_idx);
    }

    // -- Empty input --

    #[test]
    fn empty_input_produces_empty_output() {
        let faces: Vec<Face> = Vec::new();
        let clusters: Vec<Cluster> = Vec::new();
        let section = extract_geometry(&faces, &clusters);

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
        let clusters = vec![Cluster {
            id: 0,
            bounds: Aabb {
                min: Vec3::ZERO,
                max: Vec3::ONE,
            },
            face_indices: vec![0, 1, 2, 3],
        }];

        let section = extract_geometry(&faces, &clusters);

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
        let clusters = vec![
            Cluster {
                id: 0,
                bounds: Aabb {
                    min: Vec3::ZERO,
                    max: Vec3::ONE,
                },
                face_indices: vec![0],
            },
            Cluster {
                id: 1,
                bounds: Aabb {
                    min: Vec3::ZERO,
                    max: Vec3::ONE,
                },
                face_indices: vec![1, 2],
            },
        ];

        let section = extract_geometry(&faces, &clusters);
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

        let map_data = crate::parse::parse_map_file(&map_path).expect("test.map should parse");
        let grid_result = crate::spatial_grid::assign_to_grid(map_data.world_faces, None);
        let clusters = grid_cells_to_clusters(&grid_result.cells);

        let section = extract_geometry(&grid_result.faces, &clusters);

        // Every face should produce triangles
        assert_eq!(section.faces.len(), grid_result.faces.len());
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

        // Faces are ordered by cluster
        let mut prev_cluster = 0u32;
        for face in &section.faces {
            assert!(
                face.cluster_index >= prev_cluster,
                "faces not ordered by cluster"
            );
            prev_cluster = face.cluster_index;
        }

        // Round-trip serialization
        let bytes = section.to_bytes();
        let restored = GeometrySection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    /// Helper: convert grid cells to Cluster type for test compatibility.
    fn grid_cells_to_clusters(cells: &[crate::spatial_grid::GridCell]) -> Vec<Cluster> {
        cells
            .iter()
            .filter(|c| {
                !c.face_indices.is_empty()
                    || c.cell_type.map_or(false, |t| {
                        t != crate::spatial_grid::CellType::Solid
                    })
            })
            .enumerate()
            .map(|(new_id, cell)| Cluster {
                id: new_id,
                bounds: cell.bounds.clone(),
                face_indices: cell.face_indices.clone(),
            })
            .collect()
    }
}
