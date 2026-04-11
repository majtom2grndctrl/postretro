// Geometry extraction: fan-triangulate faces, compute UVs, build vertex/index buffers.
// See: context/lib/build_pipeline.md §PRL

use std::collections::HashSet;

use glam::DVec3;
use postretro_level_format::geometry::{FaceMetaV2, GeometrySectionV2};
use postretro_level_format::texture_names::TextureNamesSection;

use crate::map_data::{Face, TextureProjection};
use crate::map_format::MapFormat;
use crate::partition::BspTree;

/// Result of geometry extraction: V2 geometry section plus texture name table.
pub struct GeometryResult {
    pub geometry: GeometrySectionV2,
    pub texture_names: TextureNamesSection,
}

/// Fan-triangulate faces, compute texel-space UVs, and build a
/// `GeometrySectionV2` with faces ordered by empty BSP leaf.
///
/// Coordinates are expected to be in engine space (Y-up) -- the Quake-to-engine
/// transform is applied earlier, at the parse boundary in `parse.rs`. UV
/// computation reverses this transform per-vertex to recover Quake-space
/// positions for texture projection.
///
/// Only empty leaves contribute geometry. Solid leaves are skipped. The
/// `leaf_index` field in `FaceMetaV2` stores the sequential index among empty
/// leaves (not the raw leaf index in the BSP tree).
///
/// Leaves listed in `exterior_leaves` contribute no faces but still advance
/// the sequential empty-leaf counter, so `leaf_index` values remain aligned
/// with the encoded `BspLeavesSection` produced by the visibility pass. Pass
/// `&HashSet::new()` to disable exterior culling.
pub fn extract_geometry(
    faces: &[Face],
    tree: &BspTree,
    exterior_leaves: &HashSet<usize>,
) -> GeometryResult {
    if faces.is_empty() {
        return GeometryResult {
            geometry: GeometrySectionV2 {
                vertices: Vec::new(),
                indices: Vec::new(),
                faces: Vec::new(),
            },
            texture_names: TextureNamesSection { names: Vec::new() },
        };
    }

    // Build deduplicated texture name list (encounter order).
    let mut texture_names: Vec<String> = Vec::new();
    let mut texture_indices: Vec<u32> = Vec::with_capacity(faces.len());
    for face in faces {
        let idx = texture_names
            .iter()
            .position(|n| n == &face.texture)
            .unwrap_or_else(|| {
                texture_names.push(face.texture.clone());
                texture_names.len() - 1
            });
        texture_indices.push(idx as u32);
    }

    // Build a face ordering sorted by empty-leaf index. Exterior leaves
    // contribute no faces but still consume a sequential index slot so the
    // `leaf_index` in FaceMetaV2 stays aligned with the BspLeavesSection.
    let ordered_faces = build_leaf_ordered_faces(tree, exterior_leaves);

    let inverse_scale: f64 = 1.0 / MapFormat::IdTech2.units_to_meters();

    let mut vertices: Vec<[f32; 5]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut face_metas: Vec<FaceMetaV2> = Vec::new();

    for &(face_idx, leaf_seq_idx) in &ordered_faces {
        let face = &faces[face_idx];

        let base_vertex = vertices.len() as u32;

        // Emit vertices with position (engine space) + UV (texel space).
        // This is the **output precision boundary** for geometry: all math is
        // computed in f64, and we narrow to f32 only at the PRL vertex buffer write.
        for &v in &face.vertices {
            let quake_pos = engine_to_quake(v) * inverse_scale;
            let (u, v_coord) = compute_texel_uv(quake_pos, face);
            vertices.push([v.x as f32, v.y as f32, v.z as f32, u as f32, v_coord as f32]);
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

        face_metas.push(FaceMetaV2 {
            index_offset,
            index_count,
            leaf_index: leaf_seq_idx as u32,
            texture_index: texture_indices[face_idx],
        });
    }

    GeometryResult {
        geometry: GeometrySectionV2 {
            vertices,
            indices,
            faces: face_metas,
        },
        texture_names: TextureNamesSection {
            names: texture_names,
        },
    }
}

/// Convert engine-space position (Y-up) back to Quake-space (Z-up).
///
/// Inverse of the `quake_to_engine` transform in parse.rs:
///   engine = (-qy, qz, -qx)
///   quake  = (-engine_z, -engine_x, engine_y)
fn engine_to_quake(v: DVec3) -> DVec3 {
    DVec3::new(-v.z, -v.x, v.y)
}

/// Convert engine-space normal back to Quake-space normal.
/// Same transform as positions (direction vector, no scale).
fn engine_normal_to_quake(n: DVec3) -> DVec3 {
    engine_to_quake(n)
}

/// Compute un-normalized texel-space UV for a vertex in Quake space.
///
/// Returns (u, v) in texel units. The engine normalizes by dividing by
/// texture width/height at load time.
fn compute_texel_uv(quake_pos: DVec3, face: &Face) -> (f64, f64) {
    match &face.tex_projection {
        TextureProjection::Standard {
            u_offset,
            v_offset,
            angle,
            scale_u,
            scale_v,
        } => {
            let quake_normal = engine_normal_to_quake(face.normal);
            standard_texel_uv(
                quake_pos,
                quake_normal,
                *u_offset,
                *v_offset,
                *angle,
                *scale_u,
                *scale_v,
            )
        }
        TextureProjection::Valve {
            u_axis,
            u_offset,
            v_axis,
            v_offset,
            scale_u,
            scale_v,
        } => valve_texel_uv(
            quake_pos, *u_axis, *u_offset, *v_axis, *v_offset, *scale_u, *scale_v,
        ),
    }
}

/// Standard (idTech2) texel-space UV: project onto closest axis plane,
/// apply rotation, then scale and offset.
///
/// Mirrors shambler's `standard_uv` but omits the texture-size division,
/// producing texel-space coordinates instead of normalized UVs.
fn standard_texel_uv(
    vertex: DVec3,
    quake_normal: DVec3,
    u_offset: f64,
    v_offset: f64,
    angle: f64,
    scale_u: f64,
    scale_v: f64,
) -> (f64, f64) {
    // Choose projection axes from closest axis to face normal (Quake convention).
    let du = quake_normal.z.abs(); // up axis (Z in Quake)
    let dr = quake_normal.y.abs(); // right axis (Y in Quake)
    let df = quake_normal.x.abs(); // forward axis (X in Quake)

    let (x, y) = if du >= dr && du >= df {
        // Face is most upward/downward: project onto XY plane
        (vertex.x, -vertex.y)
    } else if dr >= du && dr >= df {
        // Face is most left/right: project onto XZ plane
        (vertex.x, -vertex.z)
    } else {
        // Face is most forward/backward: project onto YZ plane
        (vertex.y, -vertex.z)
    };

    // Apply texture rotation
    let rot_rad = angle.to_radians();
    let cos_r = rot_rad.cos();
    let sin_r = rot_rad.sin();
    let rx = x * cos_r - y * sin_r;
    let ry = x * sin_r + y * cos_r;

    // Scale then offset (texel space — no texture-size division).
    let u = rx / scale_u + u_offset;
    let v = ry / scale_v + v_offset;

    (u, v)
}

/// Valve 220 texel-space UV: explicit projection axes with per-axis offset.
///
/// Mirrors shambler's `valve_uv` but omits the texture-size division.
fn valve_texel_uv(
    vertex: DVec3,
    u_axis: DVec3,
    u_offset: f64,
    v_axis: DVec3,
    v_offset: f64,
    scale_u: f64,
    scale_v: f64,
) -> (f64, f64) {
    let u = u_axis.dot(vertex) / scale_u + u_offset;
    let v = v_axis.dot(vertex) / scale_v + v_offset;
    (u, v)
}

/// Build a list of (face_index, empty_leaf_sequential_index) pairs ordered by
/// sequential empty-leaf index.
///
/// Iterates BSP leaves in order, skipping solid leaves. Each empty leaf gets a
/// sequential index (0, 1, 2, ...) used as the `leaf_index` in face metadata.
///
/// Leaves in `exterior_leaves` contribute no faces, but the sequential counter
/// still advances through them. This keeps the sequential index a stable
/// function of the BSP leaf array — interior leaves downstream of an exterior
/// leaf land in the same sequential slot they would occupy without exterior
/// culling — so emitting `face_count = 0` in `BspLeavesSection` for the same
/// exterior set (see `visibility::encode_leaves_and_pvs`) keeps face ranges
/// and geometry storage in lockstep.
fn build_leaf_ordered_faces(
    tree: &BspTree,
    exterior_leaves: &HashSet<usize>,
) -> Vec<(usize, usize)> {
    let capacity: usize = tree
        .leaves
        .iter()
        .enumerate()
        .filter(|(idx, l)| !l.is_solid && !exterior_leaves.contains(idx))
        .map(|(_, l)| l.face_indices.len())
        .sum();
    let mut ordered = Vec::with_capacity(capacity);

    let mut empty_leaf_idx = 0usize;
    for (bsp_leaf_idx, leaf) in tree.leaves.iter().enumerate() {
        if leaf.is_solid {
            continue;
        }
        if !exterior_leaves.contains(&bsp_leaf_idx) {
            for &face_idx in &leaf.face_indices {
                ordered.push((face_idx, empty_leaf_idx));
            }
        }
        empty_leaf_idx += 1;
    }

    ordered
}

/// Log geometry extraction statistics.
pub fn log_stats(result: &GeometryResult, empty_leaf_count: usize) {
    let section = &result.geometry;
    let triangle_count = section.indices.len() / 3;
    log::info!("[Compiler] Vertices: {}", section.vertices.len());
    log::info!("[Compiler] Indices: {}", section.indices.len());
    log::info!("[Compiler] Triangles: {triangle_count}");
    log::info!("[Compiler] Faces: {}", section.faces.len());
    log::info!("[Compiler] Empty leaves: {empty_leaf_count}");
    log::info!(
        "[Compiler] Unique textures: {}",
        result.texture_names.names.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::{Aabb, BspLeaf};
    use postretro_level_format::geometry::NO_TEXTURE;

    /// Shared empty exterior set for tests that don't exercise exterior
    /// culling — matches the no-op pass-through shape used by upstream
    /// call sites before the exterior-cull flood-fill was introduced.
    fn no_exterior() -> HashSet<usize> {
        HashSet::new()
    }

    fn default_projection() -> TextureProjection {
        TextureProjection::Standard {
            u_offset: 0.0,
            v_offset: 0.0,
            angle: 0.0,
            scale_u: 1.0,
            scale_v: 1.0,
        }
    }

    fn triangle_face() -> Face {
        Face {
            vertices: vec![
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(1.0, 0.0, 0.0),
                DVec3::new(0.0, 1.0, 0.0),
            ],
            normal: DVec3::Z,
            distance: 0.0,
            texture: "test".to_string(),
            tex_projection: default_projection(),
            brush_index: 0,
        }
    }

    fn quad_face() -> Face {
        Face {
            vertices: vec![
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(1.0, 0.0, 0.0),
                DVec3::new(1.0, 1.0, 0.0),
                DVec3::new(0.0, 1.0, 0.0),
            ],
            normal: DVec3::Z,
            distance: 0.0,
            texture: "test".to_string(),
            tex_projection: default_projection(),
            brush_index: 0,
        }
    }

    fn pentagon_face() -> Face {
        Face {
            vertices: vec![
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(1.0, 0.0, 0.0),
                DVec3::new(1.5, 1.0, 0.0),
                DVec3::new(0.5, 1.5, 0.0),
                DVec3::new(-0.5, 1.0, 0.0),
            ],
            normal: DVec3::Z,
            distance: 0.0,
            texture: "test".to_string(),
            tex_projection: default_projection(),
            brush_index: 0,
        }
    }

    fn hexagon_face() -> Face {
        Face {
            vertices: vec![
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(1.0, 0.0, 0.0),
                DVec3::new(1.5, 0.5, 0.0),
                DVec3::new(1.0, 1.0, 0.0),
                DVec3::new(0.0, 1.0, 0.0),
                DVec3::new(-0.5, 0.5, 0.0),
            ],
            normal: DVec3::Z,
            distance: 0.0,
            texture: "test".to_string(),
            tex_projection: default_projection(),
            brush_index: 0,
        }
    }

    fn make_tree_with_empty_leaves(leaves: Vec<(Vec<usize>, bool)>) -> BspTree {
        let bsp_leaves: Vec<BspLeaf> = leaves
            .into_iter()
            .map(|(face_indices, is_solid)| BspLeaf {
                face_indices,
                bounds: Aabb {
                    min: DVec3::ZERO,
                    max: DVec3::ONE,
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

        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;

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

        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;

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

        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;

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

        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;

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

        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;

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

        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;

        let sum: u32 = section.faces.iter().map(|f| f.index_count).sum();
        assert_eq!(sum, section.indices.len() as u32);
    }

    // -- Vertex positions passthrough --

    #[test]
    fn vertex_positions_are_passed_through_unchanged() {
        let faces = vec![Face {
            vertices: vec![
                DVec3::new(-2.0, 3.0, -1.0),
                DVec3::new(-5.0, 6.0, -4.0),
                DVec3::new(-8.0, 9.0, -7.0),
            ],
            normal: DVec3::Y,
            distance: 0.0,
            texture: "test".to_string(),
            tex_projection: default_projection(),
            brush_index: 0,
        }];
        let tree = make_tree_with_empty_leaves(vec![(vec![0], false)]);

        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;

        assert_eq!(section.vertices[0][0], -2.0);
        assert_eq!(section.vertices[0][1], 3.0);
        assert_eq!(section.vertices[0][2], -1.0);
        assert_eq!(section.vertices[1][0], -5.0);
        assert_eq!(section.vertices[1][1], 6.0);
        assert_eq!(section.vertices[1][2], -4.0);
        assert_eq!(section.vertices[2][0], -8.0);
        assert_eq!(section.vertices[2][1], 9.0);
        assert_eq!(section.vertices[2][2], -7.0);
    }

    // -- Leaf ordering --

    #[test]
    fn faces_ordered_by_empty_leaf() {
        let faces = vec![triangle_face(), quad_face(), pentagon_face()];
        let tree = make_tree_with_empty_leaves(vec![
            (vec![0], false),    // empty leaf 0
            (vec![1, 2], false), // empty leaf 1
        ]);

        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;

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

        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;

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

        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;

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
        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;

        assert!(section.vertices.is_empty());
        assert!(section.indices.is_empty());
        assert!(section.faces.is_empty());
        assert!(result.texture_names.names.is_empty());
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

        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;

        for (i, face) in section.faces.iter().enumerate() {
            assert!(
                face.index_count >= 3,
                "face {i} has only {} indices (need at least 3)",
                face.index_count
            );
        }
    }

    // -- GeometrySectionV2 round-trip --

    #[test]
    fn geometry_section_round_trip() {
        let faces = vec![triangle_face(), quad_face(), pentagon_face()];
        let tree = make_tree_with_empty_leaves(vec![(vec![0], false), (vec![1, 2], false)]);

        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;
        let bytes = section.to_bytes();
        let restored = GeometrySectionV2::from_bytes(&bytes).unwrap();

        assert_eq!(*section, restored);
    }

    // -- Texture name deduplication --

    #[test]
    fn texture_names_deduplicated() {
        let mut face_a = triangle_face();
        face_a.texture = "metal/floor".to_string();
        let mut face_b = quad_face();
        face_b.texture = "concrete/wall".to_string();
        let mut face_c = pentagon_face();
        face_c.texture = "metal/floor".to_string(); // duplicate

        let faces = vec![face_a, face_b, face_c];
        let tree = make_tree_with_empty_leaves(vec![(vec![0, 1, 2], false)]);

        let result = extract_geometry(&faces, &tree, &no_exterior());

        assert_eq!(result.texture_names.names.len(), 2);
        assert_eq!(result.texture_names.names[0], "metal/floor");
        assert_eq!(result.texture_names.names[1], "concrete/wall");
    }

    #[test]
    fn same_texture_gets_same_index() {
        let mut face_a = triangle_face();
        face_a.texture = "metal/floor".to_string();
        let mut face_b = quad_face();
        face_b.texture = "concrete/wall".to_string();
        let mut face_c = pentagon_face();
        face_c.texture = "metal/floor".to_string();

        let faces = vec![face_a, face_b, face_c];
        let tree = make_tree_with_empty_leaves(vec![(vec![0, 1, 2], false)]);

        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;

        // Face 0 and 2 share texture "metal/floor" -> index 0
        // Face 1 has "concrete/wall" -> index 1
        assert_eq!(section.faces[0].texture_index, 0);
        assert_eq!(section.faces[1].texture_index, 1);
        assert_eq!(section.faces[2].texture_index, 0);
    }

    // -- UV computation --

    #[test]
    fn vertices_have_uv_coordinates() {
        let faces = vec![triangle_face()];
        let tree = make_tree_with_empty_leaves(vec![(vec![0], false)]);

        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;

        // Every vertex should have 5 floats (x, y, z, u, v)
        for vert in &section.vertices {
            assert!(vert[3].is_finite(), "u should be finite");
            assert!(vert[4].is_finite(), "v should be finite");
        }
    }

    #[test]
    fn valve_projection_produces_nonzero_uvs() {
        // A face with Valve projection on non-axis-aligned axes should produce
        // non-zero UVs for vertices away from origin.
        let face = Face {
            vertices: vec![
                DVec3::new(0.0, 0.0, -2.54),   // 100 Quake units forward
                DVec3::new(-2.54, 0.0, -2.54), // 100 right, 100 forward
                DVec3::new(-2.54, 0.0, 0.0),   // 100 right
            ],
            normal: DVec3::Y,
            distance: 0.0,
            texture: "test_valve".to_string(),
            tex_projection: TextureProjection::Valve {
                u_axis: DVec3::new(1.0, 0.0, 0.0),
                u_offset: 0.0,
                v_axis: DVec3::new(0.0, 0.0, -1.0),
                v_offset: 0.0,
                scale_u: 1.0,
                scale_v: 1.0,
            },
            brush_index: 0,
        };

        let faces = vec![face];
        let tree = make_tree_with_empty_leaves(vec![(vec![0], false)]);

        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;

        // At least one vertex should have non-zero UVs
        let has_nonzero = section
            .vertices
            .iter()
            .any(|v| v[3].abs() > 0.01 || v[4].abs() > 0.01);
        assert!(has_nonzero, "Valve projection should produce non-zero UVs");
    }

    #[test]
    fn standard_projection_with_offset_produces_nonzero_uvs() {
        // A face with non-zero offsets and scale
        let face = Face {
            vertices: vec![
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(2.54, 0.0, 0.0), // 100 Quake units in engine X
                DVec3::new(2.54, 2.54, 0.0),
                DVec3::new(0.0, 2.54, 0.0),
            ],
            normal: DVec3::NEG_Z,
            distance: 0.0,
            texture: "test_offset".to_string(),
            tex_projection: TextureProjection::Standard {
                u_offset: 32.0,
                v_offset: 16.0,
                angle: 0.0,
                scale_u: 1.0,
                scale_v: 1.0,
            },
            brush_index: 0,
        };

        let faces = vec![face];
        let tree = make_tree_with_empty_leaves(vec![(vec![0], false)]);

        let result = extract_geometry(&faces, &tree, &no_exterior());
        let section = &result.geometry;

        // The vertex at origin should have UVs equal to the offsets (32, 16)
        let v0 = &section.vertices[0];
        assert!(
            (v0[3] - 32.0).abs() < 0.01,
            "u at origin should be offset (32.0), got {}",
            v0[3]
        );
        assert!(
            (v0[4] - 16.0).abs() < 0.01,
            "v at origin should be offset (16.0), got {}",
            v0[4]
        );
    }

    // -- Coordinate transform round-trip --

    #[test]
    fn engine_to_quake_inverts_quake_to_engine() {
        use crate::parse::quake_to_engine_for_test;

        let original = DVec3::new(100.0, -50.0, 75.0);
        let engine = quake_to_engine_for_test(original);
        let recovered = engine_to_quake(engine);

        assert!(
            (recovered - original).length() < 1e-5,
            "round trip failed: {original} -> {engine} -> {recovered}"
        );
    }

    // -- Integration test with real map --

    #[test]
    fn extract_from_test_map() {
        let map_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("assets/maps/test.map");

        let map_data =
            crate::parse::parse_map_file(&map_path, crate::map_format::MapFormat::IdTech2)
                .expect("test.map should parse");

        let partition_result = crate::partition::partition(&map_data.brush_volumes)
            .expect("partition should succeed on test map");

        let result = extract_geometry(
            &partition_result.faces,
            &partition_result.tree,
            &no_exterior(),
        );
        let section = &result.geometry;

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

        // UVs are finite
        for vert in &section.vertices {
            assert!(vert[3].is_finite(), "u should be finite");
            assert!(vert[4].is_finite(), "v should be finite");
        }

        // Texture names should be non-empty
        assert!(
            !result.texture_names.names.is_empty(),
            "should have at least one texture name"
        );

        // All texture indices valid
        let tex_count = result.texture_names.names.len() as u32;
        for face in &section.faces {
            assert!(
                face.texture_index < tex_count || face.texture_index == NO_TEXTURE,
                "texture_index {} out of range (count: {})",
                face.texture_index,
                tex_count
            );
        }

        // Round-trip serialization
        let bytes = section.to_bytes();
        let restored = GeometrySectionV2::from_bytes(&bytes).unwrap();
        assert_eq!(*section, restored);

        // TextureNames round-trip
        let tex_bytes = result.texture_names.to_bytes();
        let tex_restored = TextureNamesSection::from_bytes(&tex_bytes).unwrap();
        assert_eq!(result.texture_names, tex_restored);
    }
}
