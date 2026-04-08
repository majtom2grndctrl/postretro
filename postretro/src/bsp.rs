// BSP loading: parse BSP2 files via qbsp, produce engine-side geometry.
// See: context/lib/rendering_pipeline.md

use std::path::Path;

use glam::Vec3;
use thiserror::Error;

#[cfg(test)]
const DEFAULT_BSP_PATH: &str = "assets/maps/test.bsp";

// --- Error types ---

#[derive(Debug, Error)]
pub enum BspLoadError {
    #[error("BSP file not found: {0}")]
    FileNotFound(String),
    #[error("failed to read BSP file: {0}")]
    IoError(#[from] std::io::Error),
    #[error("failed to parse BSP file: {0}")]
    ParseError(#[from] qbsp::BspParseError),
    #[error("BSP contains no models (missing worldspawn)")]
    NoModels,
    #[error(
        "face {face_idx} references out-of-bounds edge index {edge_idx} (surface_edges len: {surf_edges_len})"
    )]
    EdgeOutOfBounds {
        face_idx: usize,
        edge_idx: u32,
        surf_edges_len: usize,
    },
    #[error(
        "face {face_idx} references out-of-bounds vertex index {vertex_idx} (vertices len: {vertices_len})"
    )]
    VertexOutOfBounds {
        face_idx: usize,
        vertex_idx: u32,
        vertices_len: usize,
    },
}

// --- Engine types ---

#[derive(Debug, Clone)]
pub struct FaceMeta {
    /// Byte offset (in indices, not bytes) into the index buffer where this face's triangles start.
    pub index_offset: u32,
    /// Number of indices (triangles * 3) for this face.
    pub index_count: u32,
    /// BSP leaf index this face belongs to. A face may appear in multiple leaves via
    /// mark_surfaces, but we store the first leaf encountered during the build pass.
    /// Currently informational — may be consumed by future diagnostics or debug visualization.
    #[allow(dead_code)]
    pub leaf_index: u32,
}

#[derive(Debug, Clone)]
pub struct BspNodeData {
    pub plane_normal: Vec3,
    pub plane_dist: f32,
    /// Index of the front child. If `front_is_leaf` is true, this indexes into leaves;
    /// otherwise it indexes into nodes.
    pub front: u32,
    pub front_is_leaf: bool,
    /// Index of the back child. If `back_is_leaf` is true, this indexes into leaves;
    /// otherwise it indexes into nodes.
    pub back: u32,
    pub back_is_leaf: bool,
}

#[derive(Debug, Clone)]
pub struct BspLeafData {
    pub mins: Vec3,
    pub maxs: Vec3,
    /// Indices into `BspWorld::face_meta` for faces visible from this leaf.
    pub face_indices: Vec<u32>,
    /// Offset into the raw visdata where this leaf's PVS begins, or -1 if none.
    pub visdata_offset: i32,
}

#[derive(Debug)]
pub struct BspWorld {
    /// Flat vertex positions, Y-up coordinate system.
    pub vertices: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub face_meta: Vec<FaceMeta>,
    /// BSP tree nodes for point-in-leaf traversal.
    pub nodes: Vec<BspNodeData>,
    pub leaves: Vec<BspLeafData>,
    pub visdata: Vec<u8>,
    /// Root node index for the world model (model 0).
    pub root_node: u32,
}

// --- Coordinate transform ---

/// Scale factor: Quake units (inches) to engine units (meters). 1 inch = 0.0254 meters.
const QUAKE_TO_METERS: f32 = 0.0254;

/// Convert a Quake Z-up position to engine Y-up, scaled to meters.
/// Quake: +X forward, +Y left, +Z up (units = inches)
/// Engine: +X right, +Y up, -Z forward (units = meters)
/// Swizzle: engine_x = -quake_y, engine_y = quake_z, engine_z = -quake_x
/// Scale: multiply by QUAKE_TO_METERS to convert inches to meters.
fn quake_to_engine(v: Vec3) -> Vec3 {
    Vec3::new(-v.y, v.z, -v.x) * QUAKE_TO_METERS
}

// --- Fan triangulation ---

/// Fan-triangulate a convex polygon.
/// For vertices [v0, v1, v2, ..., vN], emits triangles:
/// (v0, v1, v2), (v0, v2, v3), ..., (v0, v(N-1), vN)
///
/// Returns the indices to append to the index buffer.
fn fan_triangulate(base_vertex: u32, vertex_count: u32) -> Vec<u32> {
    if vertex_count < 3 {
        return Vec::new();
    }
    let tri_count = vertex_count - 2;
    let mut indices = Vec::with_capacity(tri_count as usize * 3);
    for i in 0..tri_count {
        indices.push(base_vertex);
        indices.push(base_vertex + i + 1);
        indices.push(base_vertex + i + 2);
    }
    indices
}

// --- Leaf-to-face mapping ---

/// Build a map from face index -> first leaf index that references it.
/// BSP leaves reference faces via mark_surfaces (an indirection table).
fn build_face_to_leaf_map(bsp: &qbsp::BspData) -> Vec<u32> {
    let face_count = bsp.faces.len();
    // Default to 0 (the out-of-bounds leaf) for faces not referenced by any leaf.
    let mut face_to_leaf = vec![0u32; face_count];

    for (leaf_idx, leaf) in bsp.leaves.iter().enumerate() {
        let start = leaf.face_idx.0 as usize;
        let count = leaf.face_num.0 as usize;
        for ms_idx in start..start + count {
            if let Some(mark_surface) = bsp.mark_surfaces.get(ms_idx) {
                let face_idx = mark_surface.0 as usize;
                if face_idx < face_count {
                    // First leaf wins — don't overwrite if already assigned to a real leaf.
                    if face_to_leaf[face_idx] == 0 {
                        face_to_leaf[face_idx] = leaf_idx as u32;
                    }
                }
            }
        }
    }

    face_to_leaf
}

// --- BSP loading ---

#[cfg(test)]
pub fn resolve_bsp_path(args: &[String]) -> String {
    // First positional argument after the binary name is the BSP path.
    if args.len() > 1 {
        args[1].clone()
    } else {
        DEFAULT_BSP_PATH.to_string()
    }
}

pub fn load_bsp(path: &str) -> Result<BspWorld, BspLoadError> {
    let path_ref = Path::new(path);
    if !path_ref.exists() {
        return Err(BspLoadError::FileNotFound(path.to_string()));
    }

    let file_data = std::fs::read(path_ref)?;

    let bsp = qbsp::BspData::parse(qbsp::BspParseInput {
        bsp: &file_data,
        lit: None,
        settings: qbsp::BspParseSettings::default(),
    })?;

    if bsp.models.is_empty() {
        return Err(BspLoadError::NoModels);
    }

    let world_model = &bsp.models[0];
    let face_to_leaf = build_face_to_leaf_map(&bsp);

    let first_face = world_model.first_face as usize;
    let num_faces = world_model.num_faces as usize;

    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut face_meta: Vec<FaceMeta> = Vec::with_capacity(num_faces);

    for face_offset in 0..num_faces {
        let face_idx = first_face + face_offset;
        let face = &bsp.faces[face_idx];

        let face_base_vertex = vertices.len() as u32;
        let face_first_edge = face.first_edge;
        let face_num_edges = face.num_edges.0;

        // Extract vertices for this face via the edge/surfedge indirection.
        for i in 0..face_num_edges {
            let se_idx = (face_first_edge + i) as usize;
            if se_idx >= bsp.surface_edges.len() {
                return Err(BspLoadError::EdgeOutOfBounds {
                    face_idx,
                    edge_idx: face_first_edge + i,
                    surf_edges_len: bsp.surface_edges.len(),
                });
            }

            let surf_edge = bsp.surface_edges[se_idx];
            let edge = &bsp.edges[surf_edge.unsigned_abs() as usize];
            let vert_idx = if surf_edge.is_negative() {
                edge.b.0
            } else {
                edge.a.0
            };

            if vert_idx as usize >= bsp.vertices.len() {
                return Err(BspLoadError::VertexOutOfBounds {
                    face_idx,
                    vertex_idx: vert_idx,
                    vertices_len: bsp.vertices.len(),
                });
            }

            let pos = quake_to_engine(bsp.vertices[vert_idx as usize]);
            vertices.push(pos.to_array());
        }

        let index_offset = indices.len() as u32;
        let tri_indices = fan_triangulate(face_base_vertex, face_num_edges);
        let index_count = tri_indices.len() as u32;
        indices.extend(tri_indices);

        if face_num_edges >= 3 {
            let v0 = Vec3::from(vertices[face_base_vertex as usize]);
            let v1 = Vec3::from(vertices[face_base_vertex as usize + 1]);
            let v2 = Vec3::from(vertices[face_base_vertex as usize + 2]);
            let edge_a = v1 - v0;
            let edge_b = v2 - v0;
            let normal = edge_a.cross(edge_b);
            if normal.length_squared() > 0.0 {
                let normal = normal.normalize();
                log::trace!(
                    "[BSP] face {face_idx} normal: ({:.3}, {:.3}, {:.3}), plane_side: {}",
                    normal.x,
                    normal.y,
                    normal.z,
                    face.plane_side.0,
                );
            }
        }

        let leaf_index = face_to_leaf.get(face_idx).copied().unwrap_or(0);

        face_meta.push(FaceMeta {
            index_offset,
            index_count,
            leaf_index,
        });
    }

    let nodes: Vec<BspNodeData> = bsp
        .nodes
        .iter()
        .map(|node| {
            let plane = &bsp.planes[node.plane_idx as usize];
            let plane_normal = quake_to_engine(plane.normal);
            // The plane normal is converted via quake_to_engine (swizzle + scale).
            // The plane equation is dot(normal, point) = dist. The normal passed
            // through quake_to_engine has its magnitude scaled by QUAKE_TO_METERS,
            // so the distance must be scaled by the same factor to keep the plane
            // equation consistent with scaled vertex positions.
            let plane_dist = plane.dist * QUAKE_TO_METERS;

            let (front, front_is_leaf) = match *node.front {
                qbsp::data::nodes::BspNodeRef::Node(i) => (i, false),
                qbsp::data::nodes::BspNodeRef::Leaf(i) => (i, true),
            };
            let (back, back_is_leaf) = match *node.back {
                qbsp::data::nodes::BspNodeRef::Node(i) => (i, false),
                qbsp::data::nodes::BspNodeRef::Leaf(i) => (i, true),
            };

            BspNodeData {
                plane_normal,
                plane_dist,
                front,
                front_is_leaf,
                back,
                back_is_leaf,
            }
        })
        .collect();

    let leaves: Vec<BspLeafData> = bsp
        .leaves
        .iter()
        .map(|leaf| {
            let mins = quake_to_engine(leaf.bound.min);
            let maxs = quake_to_engine(leaf.bound.max);

            // After coordinate transform, min/max may swap per component.
            let real_mins = mins.min(maxs);
            let real_maxs = mins.max(maxs);

            let start = leaf.face_idx.0 as usize;
            let count = leaf.face_num.0 as usize;
            let mut face_indices = Vec::with_capacity(count);
            for ms_idx in start..start + count {
                if let Some(mark_surface) = bsp.mark_surfaces.get(ms_idx) {
                    let face_idx = mark_surface.0 as usize;
                    // Convert absolute face index to our face_meta index
                    // (relative to world model's first_face).
                    if face_idx >= first_face && face_idx < first_face + num_faces {
                        face_indices.push((face_idx - first_face) as u32);
                    }
                }
            }

            let visdata_offset = match leaf.visdata {
                qbsp::data::visdata::VisDataRef::Offset(o) => o,
                qbsp::data::visdata::VisDataRef::Cluster(c) => c.0,
            };

            BspLeafData {
                mins: real_mins,
                maxs: real_maxs,
                face_indices,
                visdata_offset,
            }
        })
        .collect();

    let visdata = bsp.visibility.visdata.clone();

    let root_node = match bsp.models[0].hulls.root {
        qbsp::data::nodes::BspNodeRef::Node(i) => i,
        qbsp::data::nodes::BspNodeRef::Leaf(_) => 0,
    };

    log::info!(
        "[BSP] Loaded: {} vertices, {} indices ({} triangles), {} faces, {} nodes, {} leaves, {} bytes visdata",
        vertices.len(),
        indices.len(),
        indices.len() / 3,
        face_meta.len(),
        nodes.len(),
        leaves.len(),
        visdata.len(),
    );

    Ok(BspWorld {
        vertices,
        indices,
        face_meta,
        nodes,
        leaves,
        visdata,
        root_node,
    })
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    // -- Fan triangulation --

    #[test]
    fn fan_triangulate_triangle_produces_single_triangle() {
        let indices = fan_triangulate(0, 3);
        assert_eq!(indices, vec![0, 1, 2]);
    }

    #[test]
    fn fan_triangulate_quad_produces_two_triangles() {
        let indices = fan_triangulate(0, 4);
        assert_eq!(indices, vec![0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn fan_triangulate_pentagon_produces_three_triangles() {
        let indices = fan_triangulate(0, 5);
        assert_eq!(indices, vec![0, 1, 2, 0, 2, 3, 0, 3, 4]);
    }

    #[test]
    fn fan_triangulate_hexagon_produces_four_triangles() {
        let indices = fan_triangulate(0, 6);
        assert_eq!(indices, vec![0, 1, 2, 0, 2, 3, 0, 3, 4, 0, 4, 5]);
    }

    #[test]
    fn fan_triangulate_with_base_offset() {
        // Simulates a face starting at vertex 10 in the global buffer.
        let indices = fan_triangulate(10, 4);
        assert_eq!(indices, vec![10, 11, 12, 10, 12, 13]);
    }

    #[test]
    fn fan_triangulate_degenerate_returns_empty() {
        assert!(fan_triangulate(0, 0).is_empty());
        assert!(fan_triangulate(0, 1).is_empty());
        assert!(fan_triangulate(0, 2).is_empty());
    }

    #[test]
    fn fan_triangulate_index_count_formula() {
        // For N vertices, expect (N-2) * 3 indices.
        for n in 3..=20 {
            let indices = fan_triangulate(0, n);
            assert_eq!(
                indices.len(),
                (n as usize - 2) * 3,
                "wrong index count for {n}-gon"
            );
        }
    }

    // -- Coordinate transform --

    #[test]
    fn coordinate_transform_z_up_to_y_up() {
        // Quake: point straight up = (0, 0, 1) inch
        // Engine Y-up: should become (0, 0.0254, 0) meters
        let result = quake_to_engine(Vec3::new(0.0, 0.0, 1.0));
        assert_vec3_approx(result, Vec3::new(0.0, QUAKE_TO_METERS, 0.0));
    }

    #[test]
    fn coordinate_transform_forward_axis() {
        // Quake: +X is forward = (1, 0, 0) inch
        // Engine: forward is -Z, so (1, 0, 0) -> (0, 0, -0.0254) meters
        let result = quake_to_engine(Vec3::new(1.0, 0.0, 0.0));
        assert_vec3_approx(result, Vec3::new(0.0, 0.0, -QUAKE_TO_METERS));
    }

    #[test]
    fn coordinate_transform_left_axis() {
        // Quake: +Y is left = (0, 1, 0) inch
        // Engine: left becomes -X, so (0, 1, 0) -> (-0.0254, 0, 0) meters
        let result = quake_to_engine(Vec3::new(0.0, 1.0, 0.0));
        assert_vec3_approx(result, Vec3::new(-QUAKE_TO_METERS, 0.0, 0.0));
    }

    #[test]
    fn coordinate_transform_preserves_distance() {
        // The transform is a rotation + uniform scale by QUAKE_TO_METERS.
        // The output magnitude equals the input magnitude times the scale factor.
        let original = Vec3::new(3.0, 4.0, 5.0);
        let transformed = quake_to_engine(original);
        let expected_length = original.length() * QUAKE_TO_METERS;
        let epsilon = 1e-6;
        assert!(
            (transformed.length() - expected_length).abs() < epsilon,
            "expected length {}, got {}",
            expected_length,
            transformed.length(),
        );
    }

    #[test]
    fn coordinate_transform_roundtrip_orthogonality() {
        // Verify the transform preserves orthogonality: applying it to all three
        // basis vectors produces three mutually orthogonal vectors. The uniform
        // QUAKE_TO_METERS scale does not affect this — dot products between
        // distinct output axes remain zero.
        let ex = quake_to_engine(Vec3::X);
        let ey = quake_to_engine(Vec3::Y);
        let ez = quake_to_engine(Vec3::Z);
        let epsilon = 1e-6;
        assert!(ex.dot(ey).abs() < epsilon, "X and Y not orthogonal");
        assert!(ex.dot(ez).abs() < epsilon, "X and Z not orthogonal");
        assert!(ey.dot(ez).abs() < epsilon, "Y and Z not orthogonal");
    }

    // -- BSP file path resolution --

    #[test]
    fn resolve_bsp_path_uses_cli_arg_when_present() {
        let args = vec!["postretro".to_string(), "my_map.bsp".to_string()];
        assert_eq!(resolve_bsp_path(&args), "my_map.bsp");
    }

    #[test]
    fn resolve_bsp_path_falls_back_to_default() {
        let args = vec!["postretro".to_string()];
        assert_eq!(resolve_bsp_path(&args), DEFAULT_BSP_PATH);
    }

    // -- Error cases --

    #[test]
    fn load_bsp_missing_file_returns_file_not_found() {
        let result = load_bsp("nonexistent/path/to/map.bsp");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, BspLoadError::FileNotFound(_)),
            "expected FileNotFound, got: {err}"
        );
    }

    // -- Helper --

    fn assert_vec3_approx(actual: Vec3, expected: Vec3) {
        let epsilon = 1e-6;
        assert!(
            (actual.x - expected.x).abs() < epsilon
                && (actual.y - expected.y).abs() < epsilon
                && (actual.z - expected.z).abs() < epsilon,
            "expected ({:.3}, {:.3}, {:.3}), got ({:.3}, {:.3}, {:.3})",
            expected.x,
            expected.y,
            expected.z,
            actual.x,
            actual.y,
            actual.z,
        );
    }
}
