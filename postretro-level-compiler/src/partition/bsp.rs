// BSP tree construction: plane selection, face splitting, recursive partitioning.
// See: context/lib/build_pipeline.md §PRL

use anyhow::Result;
use glam::DVec3;

use super::types::*;
use crate::geometry_utils::split_polygon;
use crate::map_data::{BrushVolume, Face};

const PLANE_EPSILON: f64 = 0.1;
const SPLIT_PENALTY: i32 = 8;
const IMBALANCE_PENALTY: i32 = 1;
const MAX_LEAF_FACES: usize = 4;

/// Tolerance for the overall-centroid inside-brush test. Generous because
/// the average of multiple face centroids is naturally displaced from any
/// single brush surface.
const SOLID_EPSILON: f64 = 0.5;

/// Tolerance for the per-face-centroid inside-brush test. Tight because
/// individual face centroids sit exactly on their generating brush surface;
/// a generous epsilon would classify every leaf touching a brush as solid.
const FACE_SOLID_EPSILON: f64 = -0.1;

#[derive(Debug, Clone, Copy, PartialEq)]
enum FaceSide {
    Front,
    Back,
    On,
    Spanning,
}

/// Splitting plane candidate.
#[derive(Debug, Clone, Copy)]
struct Plane {
    normal: DVec3,
    distance: f64,
}

/// Classify a point relative to a plane.
fn classify_point(point: DVec3, plane: &Plane) -> FaceSide {
    let d = point.dot(plane.normal) - plane.distance;
    if d > PLANE_EPSILON {
        FaceSide::Front
    } else if d < -PLANE_EPSILON {
        FaceSide::Back
    } else {
        FaceSide::On
    }
}

/// Classify a face relative to a splitting plane.
fn classify_face(face: &Face, plane: &Plane) -> FaceSide {
    let mut front = false;
    let mut back = false;

    for &v in &face.vertices {
        match classify_point(v, plane) {
            FaceSide::Front => front = true,
            FaceSide::Back => back = true,
            FaceSide::On => {}
            FaceSide::Spanning => unreachable!(),
        }
        if front && back {
            return FaceSide::Spanning;
        }
    }

    match (front, back) {
        (true, false) => FaceSide::Front,
        (false, true) => FaceSide::Back,
        (false, false) => FaceSide::On,
        (true, true) => FaceSide::Spanning,
    }
}

/// Split a face polygon by a plane using Sutherland-Hodgman clipping.
/// Returns (front_fragment, back_fragment). Either may be None if the
/// split produces a degenerate polygon (< 3 vertices).
///
/// Delegates to `split_polygon` for the vertex math, then re-attaches
/// Face metadata (normal, distance, texture) to each fragment.
fn split_face(face: &Face, plane: &Plane) -> (Option<Face>, Option<Face>) {
    let (front_verts, back_verts) =
        split_polygon(&face.vertices, plane.normal, plane.distance, PLANE_EPSILON);

    let front = front_verts.map(|verts| Face {
        vertices: verts,
        normal: face.normal,
        distance: face.distance,
        texture: face.texture.clone(),
        tex_projection: face.tex_projection.clone(),
    });

    let back = back_verts.map(|verts| Face {
        vertices: verts,
        normal: face.normal,
        distance: face.distance,
        texture: face.texture.clone(),
        tex_projection: face.tex_projection.clone(),
    });

    (front, back)
}

/// Check if a set of faces is convex (all faces on the same side of every other face's plane).
fn is_convex(faces: &[Face]) -> bool {
    for face in faces {
        let plane = Plane {
            normal: face.normal,
            distance: face.distance,
        };
        for other in faces {
            if std::ptr::eq(face, other) {
                continue;
            }
            let side = classify_face(other, &plane);
            if side == FaceSide::Spanning || side == FaceSide::Front {
                return false;
            }
        }
    }
    true
}

/// Collect unique splitting plane candidates from faces.
/// Two planes are considered the same if their normals and distances are within epsilon.
fn collect_plane_candidates(faces: &[Face]) -> Vec<Plane> {
    let mut planes = Vec::new();
    for face in faces {
        let candidate = Plane {
            normal: face.normal,
            distance: face.distance,
        };
        let is_duplicate = planes.iter().any(|p: &Plane| {
            (p.normal - candidate.normal).length() < 0.001
                && (p.distance - candidate.distance).abs() < 0.001
        });
        if !is_duplicate {
            // Also check the negated plane
            let is_neg_duplicate = planes.iter().any(|p: &Plane| {
                (p.normal + candidate.normal).length() < 0.001
                    && (p.distance + candidate.distance).abs() < 0.001
            });
            if !is_neg_duplicate {
                planes.push(candidate);
            }
        }
    }
    planes
}

/// Score a splitting plane candidate. Lower is better.
fn score_plane(plane: &Plane, faces: &[Face]) -> i32 {
    let mut front_count = 0i32;
    let mut back_count = 0i32;
    let mut split_count = 0i32;

    for face in faces {
        match classify_face(face, plane) {
            FaceSide::Front => front_count += 1,
            FaceSide::Back => back_count += 1,
            FaceSide::On => {
                // Coplanar faces go to the child they face (same side as their "air").
                if face.normal.dot(plane.normal) > 0.0 {
                    front_count += 1;
                } else {
                    back_count += 1;
                }
            }
            FaceSide::Spanning => {
                split_count += 1;
                front_count += 1;
                back_count += 1;
            }
        }
    }

    // Reject planes that don't actually partition
    if front_count == 0 || back_count == 0 {
        return i32::MAX;
    }

    split_count * SPLIT_PENALTY + (front_count - back_count).abs() * IMBALANCE_PENALTY
}

/// Build a BSP tree from world faces.
///
/// The returned face list may differ from the input: spanning faces are replaced
/// by their split fragments. Only faces referenced by leaves are included.
pub fn build_bsp_tree(faces: Vec<Face>) -> Result<(BspTree, Vec<Face>)> {
    let face_count = faces.len();
    let face_indices: Vec<usize> = (0..face_count).collect();

    let mut tree = BspTree {
        nodes: Vec::new(),
        leaves: Vec::new(),
    };
    let mut all_faces = faces;

    build_recursive(&mut tree, &mut all_faces, &face_indices, None)?;

    // Compact: collect only the face indices referenced by leaves, remap them
    // to a contiguous array, and update all leaf references.
    let mut referenced: Vec<bool> = vec![false; all_faces.len()];
    for leaf in &tree.leaves {
        for &fi in &leaf.face_indices {
            referenced[fi] = true;
        }
    }

    let mut old_to_new = vec![0usize; all_faces.len()];
    let mut compacted_faces = Vec::new();
    for (old_idx, is_ref) in referenced.iter().enumerate() {
        if *is_ref {
            old_to_new[old_idx] = compacted_faces.len();
            compacted_faces.push(all_faces[old_idx].clone());
        }
    }

    for leaf in &mut tree.leaves {
        for fi in &mut leaf.face_indices {
            *fi = old_to_new[*fi];
        }
    }

    Ok((tree, compacted_faces))
}

/// Classify each BSP leaf as solid or empty based on brush volumes.
///
/// A leaf is solid when any candidate point from its face geometry lies inside
/// a brush volume. "Inside" means the point is on the back side (negative
/// half-space) of every plane in the brush:
/// `dot(point, plane.normal) - plane.distance <= epsilon` for all planes.
///
/// Candidate points: the overall leaf face centroid plus each individual face's
/// centroid. A leaf is solid if **any** candidate is inside a brush. Faceless
/// leaves are classified as solid — empty space always has bounding faces.
///
/// The individual face centroid test uses a tighter epsilon than the overall
/// centroid because face centroids sit exactly on their generating brush surface.
/// A generous epsilon would false-positive every face as "inside", defeating the
/// purpose of solid/empty classification.
pub fn classify_leaf_solidity(tree: &mut BspTree, faces: &[Face], brush_volumes: &[BrushVolume]) {
    if brush_volumes.is_empty() {
        return;
    }

    for leaf in &mut tree.leaves {
        // Faceless leaves are solid: empty space always has bounding faces.
        if leaf.face_indices.is_empty() {
            leaf.is_solid = true;
            continue;
        }

        // Test the overall leaf centroid first (primary test). Uses a generous
        // epsilon because this centroid averages multiple face positions and is
        // naturally displaced from brush surfaces.
        let overall_centroid = leaf_face_centroid(faces, &leaf.face_indices);
        if point_inside_any_brush(overall_centroid, brush_volumes, SOLID_EPSILON) {
            leaf.is_solid = true;
            continue;
        }

        // Test each individual face centroid. Uses a tight epsilon to avoid
        // false-positiving on face centroids that sit on (not inside) their
        // generating brush surface.
        let any_face_inside = leaf.face_indices.iter().any(|&fi| {
            let face = &faces[fi];
            let face_center: DVec3 =
                face.vertices.iter().copied().sum::<DVec3>() / face.vertices.len() as f64;
            point_inside_any_brush(face_center, brush_volumes, FACE_SOLID_EPSILON)
        });

        leaf.is_solid = any_face_inside;
    }
}

/// Compute the centroid of a leaf's face geometry (average of all face centroids).
fn leaf_face_centroid(faces: &[Face], face_indices: &[usize]) -> DVec3 {
    let mut sum = DVec3::ZERO;
    let mut count = 0usize;

    for &fi in face_indices {
        let face = &faces[fi];
        let face_center: DVec3 =
            face.vertices.iter().copied().sum::<DVec3>() / face.vertices.len() as f64;
        sum += face_center;
        count += 1;
    }

    if count > 0 {
        sum / count as f64
    } else {
        DVec3::ZERO
    }
}

/// Test whether a point is inside any brush volume.
///
/// `epsilon` controls how close to the surface counts as "inside". Positive
/// values expand the brush (generous), negative values shrink it (strict).
fn point_inside_any_brush(point: DVec3, brush_volumes: &[BrushVolume], epsilon: f64) -> bool {
    brush_volumes.iter().any(|brush| {
        brush
            .planes
            .iter()
            .all(|plane| point.dot(plane.normal) - plane.distance <= epsilon)
    })
}

/// Recursive BSP construction. Returns the child reference for the subtree built.
fn build_recursive(
    tree: &mut BspTree,
    all_faces: &mut Vec<Face>,
    face_indices: &[usize],
    parent: Option<usize>,
) -> Result<BspChild> {
    // Terminal conditions: make a leaf when the face set is convex. A small
    // non-convex set cannot be a valid leaf: it spans multiple brush volumes
    // and further splitting is required to separate solid and air regions.
    // The size cap acts as a safety valve to bound recursion depth.
    if is_face_set_convex(all_faces, face_indices) && face_indices.len() <= MAX_LEAF_FACES {
        return Ok(make_leaf(tree, all_faces, face_indices));
    }

    // Collect candidate splitting planes from this face set
    let faces_subset: Vec<Face> = face_indices.iter().map(|&i| all_faces[i].clone()).collect();
    let candidates = collect_plane_candidates(&faces_subset);

    // Find the best splitting plane
    let best = candidates
        .iter()
        .map(|p| (p, score_plane(p, &faces_subset)))
        .filter(|(_, score)| *score < i32::MAX)
        .min_by_key(|(_, score)| *score);

    let best_plane = match best {
        Some((plane, _)) => *plane,
        None => {
            // No useful partition found
            return Ok(make_leaf(tree, all_faces, face_indices));
        }
    };

    // Classify and split faces
    let mut front_indices = Vec::new();
    let mut back_indices = Vec::new();

    for &fi in face_indices {
        let face = &all_faces[fi];
        match classify_face(face, &best_plane) {
            FaceSide::Front => {
                front_indices.push(fi);
            }
            FaceSide::Back => {
                back_indices.push(fi);
            }
            FaceSide::On => {
                // Coplanar faces go to the child they face.
                if face.normal.dot(best_plane.normal) > 0.0 {
                    front_indices.push(fi);
                } else {
                    back_indices.push(fi);
                }
            }
            FaceSide::Spanning => {
                let face_clone = all_faces[fi].clone();
                let (front_frag, back_frag) = split_face(&face_clone, &best_plane);

                if let Some(front_face) = front_frag {
                    let new_idx = all_faces.len();
                    all_faces.push(front_face);
                    front_indices.push(new_idx);
                }
                if let Some(back_face) = back_frag {
                    let new_idx = all_faces.len();
                    all_faces.push(back_face);
                    back_indices.push(new_idx);
                }
                // Original spanning face is replaced by its fragments;
                // it is no longer referenced by any leaf.
            }
        }
    }

    // Safety valve: if partitioning didn't actually separate faces, make a leaf
    if front_indices.is_empty() || back_indices.is_empty() {
        let all_indices: Vec<usize> = front_indices.into_iter().chain(back_indices).collect();
        return Ok(make_leaf(tree, all_faces, &all_indices));
    }

    // Reserve a node index
    let node_idx = tree.nodes.len();
    tree.nodes.push(BspNode {
        plane_normal: best_plane.normal,
        plane_distance: best_plane.distance,
        front: BspChild::Leaf(0), // placeholder
        back: BspChild::Leaf(0),  // placeholder
        parent,
    });

    let front_child = build_recursive(tree, all_faces, &front_indices, Some(node_idx))?;
    let back_child = build_recursive(tree, all_faces, &back_indices, Some(node_idx))?;

    tree.nodes[node_idx].front = front_child;
    tree.nodes[node_idx].back = back_child;

    Ok(BspChild::Node(node_idx))
}

fn is_face_set_convex(all_faces: &[Face], face_indices: &[usize]) -> bool {
    let subset: Vec<Face> = face_indices.iter().map(|&i| all_faces[i].clone()).collect();
    is_convex(&subset)
}

fn make_leaf(tree: &mut BspTree, all_faces: &[Face], face_indices: &[usize]) -> BspChild {
    let mut bounds = Aabb::empty();
    for &fi in face_indices {
        for &v in &all_faces[fi].vertices {
            bounds.expand_point(v);
        }
    }

    let leaf_idx = tree.leaves.len();
    tree.leaves.push(BspLeaf {
        face_indices: face_indices.to_vec(),
        bounds,
        is_solid: false, // assigned later by classify_leaf_solidity
    });
    BspChild::Leaf(leaf_idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_quad(normal: DVec3, distance: f64, verts: [DVec3; 4]) -> Face {
        Face {
            vertices: verts.to_vec(),
            normal,
            distance,
            texture: "test".to_string(),
            tex_projection: Default::default(),
        }
    }

    #[test]
    fn classify_point_front_back_on() {
        let plane = Plane {
            normal: DVec3::X,
            distance: 0.0,
        };

        assert_eq!(
            classify_point(DVec3::new(1.0, 0.0, 0.0), &plane),
            FaceSide::Front
        );
        assert_eq!(
            classify_point(DVec3::new(-1.0, 0.0, 0.0), &plane),
            FaceSide::Back
        );
        assert_eq!(
            classify_point(DVec3::new(0.05, 0.0, 0.0), &plane),
            FaceSide::On
        );
    }

    #[test]
    fn classify_face_all_front() {
        let plane = Plane {
            normal: DVec3::X,
            distance: 0.0,
        };
        let face = make_quad(
            DVec3::Z,
            1.0,
            [
                DVec3::new(1.0, 0.0, 1.0),
                DVec3::new(2.0, 0.0, 1.0),
                DVec3::new(2.0, 1.0, 1.0),
                DVec3::new(1.0, 1.0, 1.0),
            ],
        );
        assert_eq!(classify_face(&face, &plane), FaceSide::Front);
    }

    #[test]
    fn classify_face_spanning() {
        let plane = Plane {
            normal: DVec3::X,
            distance: 0.0,
        };
        let face = make_quad(
            DVec3::Z,
            1.0,
            [
                DVec3::new(-1.0, 0.0, 1.0),
                DVec3::new(1.0, 0.0, 1.0),
                DVec3::new(1.0, 1.0, 1.0),
                DVec3::new(-1.0, 1.0, 1.0),
            ],
        );
        assert_eq!(classify_face(&face, &plane), FaceSide::Spanning);
    }

    #[test]
    fn split_face_produces_valid_polygons() {
        let plane = Plane {
            normal: DVec3::X,
            distance: 0.0,
        };
        let face = make_quad(
            DVec3::Z,
            1.0,
            [
                DVec3::new(-2.0, 0.0, 1.0),
                DVec3::new(2.0, 0.0, 1.0),
                DVec3::new(2.0, 2.0, 1.0),
                DVec3::new(-2.0, 2.0, 1.0),
            ],
        );

        let (front, back) = split_face(&face, &plane);
        let front = front.expect("front fragment should exist");
        let back = back.expect("back fragment should exist");

        assert!(front.vertices.len() >= 3, "front should have >= 3 verts");
        assert!(back.vertices.len() >= 3, "back should have >= 3 verts");

        // All front vertices should be on the front side or on the plane
        for v in &front.vertices {
            let d = v.dot(plane.normal) - plane.distance;
            assert!(d >= -PLANE_EPSILON, "front vertex at d={d} is behind plane");
        }

        // All back vertices should be on the back side or on the plane
        for v in &back.vertices {
            let d = v.dot(plane.normal) - plane.distance;
            assert!(
                d <= PLANE_EPSILON,
                "back vertex at d={d} is in front of plane"
            );
        }

        // Fragments inherit original face's plane
        assert_eq!(front.normal, face.normal);
        assert_eq!(front.distance, face.distance);
        assert_eq!(back.normal, face.normal);
        assert_eq!(back.distance, face.distance);
    }

    #[test]
    fn split_degenerate_discards_fragment() {
        // A triangle that barely touches the plane on one side
        let plane = Plane {
            normal: DVec3::X,
            distance: 0.0,
        };
        let face = Face {
            vertices: vec![
                DVec3::new(1.0, 0.0, 0.0),
                DVec3::new(2.0, 1.0, 0.0),
                DVec3::new(1.0, 1.0, 0.0),
            ],
            normal: DVec3::NEG_Z,
            distance: 0.0,
            texture: "test".to_string(),
            tex_projection: Default::default(),
        };

        let (front, back) = split_face(&face, &plane);
        // Entire face is on the front side, no back fragment
        assert!(front.is_some(), "front fragment should exist");
        // Back fragment should be None or have < 3 verts (discarded)
        if let Some(b) = &back {
            // If back exists, it should be degenerate or all on-plane
            assert!(
                b.vertices.len() < 3
                    || b.vertices.iter().all(|v| {
                        let d = v.dot(plane.normal) - plane.distance;
                        d.abs() <= PLANE_EPSILON
                    }),
                "unexpected back fragment"
            );
        }
    }

    #[test]
    fn build_bsp_tree_single_face() {
        let face = Face {
            vertices: vec![
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(1.0, 0.0, 0.0),
                DVec3::new(1.0, 1.0, 0.0),
            ],
            normal: DVec3::Z,
            distance: 0.0,
            texture: "test".to_string(),
            tex_projection: Default::default(),
        };

        let (tree, faces) = build_bsp_tree(vec![face]).expect("should build");
        assert_eq!(tree.nodes.len(), 0, "single face needs no interior nodes");
        assert_eq!(tree.leaves.len(), 1, "single face -> one leaf");
        assert_eq!(tree.leaves[0].face_indices.len(), 1);
        assert_eq!(faces.len(), 1);
    }

    #[test]
    fn build_bsp_tree_opposing_faces() {
        let faces = vec![
            make_quad(
                DVec3::X,
                10.0,
                [
                    DVec3::new(10.0, 0.0, 0.0),
                    DVec3::new(10.0, 0.0, 10.0),
                    DVec3::new(10.0, 10.0, 10.0),
                    DVec3::new(10.0, 10.0, 0.0),
                ],
            ),
            make_quad(
                DVec3::NEG_X,
                10.0,
                [
                    DVec3::new(-10.0, 0.0, 0.0),
                    DVec3::new(-10.0, 10.0, 0.0),
                    DVec3::new(-10.0, 10.0, 10.0),
                    DVec3::new(-10.0, 0.0, 10.0),
                ],
            ),
        ];

        let (tree, result_faces) = build_bsp_tree(faces).expect("should build");
        // With only 2 faces, should become a leaf (below MAX_LEAF_FACES)
        assert!(!tree.leaves.is_empty());
        assert!(result_faces.len() >= 2);
    }

    #[test]
    fn score_rejects_non_partitioning_planes() {
        let plane = Plane {
            normal: DVec3::X,
            distance: -100.0,
        };
        let faces = vec![make_quad(
            DVec3::Z,
            0.0,
            [
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(1.0, 0.0, 0.0),
                DVec3::new(1.0, 1.0, 0.0),
                DVec3::new(0.0, 1.0, 0.0),
            ],
        )];
        let score = score_plane(&plane, &faces);
        assert_eq!(
            score,
            i32::MAX,
            "plane with all faces on one side should score MAX"
        );
    }

    // -- Helpers for geometry-based tests --

    fn make_box_faces(min: DVec3, max: DVec3) -> Vec<Face> {
        let texture = "test".to_string();
        vec![
            Face {
                vertices: vec![
                    DVec3::new(min.x, min.y, min.z),
                    DVec3::new(min.x, max.y, min.z),
                    DVec3::new(min.x, max.y, max.z),
                    DVec3::new(min.x, min.y, max.z),
                ],
                normal: DVec3::NEG_X,
                distance: -min.x,
                texture: texture.clone(),
                tex_projection: Default::default(),
            },
            Face {
                vertices: vec![
                    DVec3::new(max.x, min.y, min.z),
                    DVec3::new(max.x, min.y, max.z),
                    DVec3::new(max.x, max.y, max.z),
                    DVec3::new(max.x, max.y, min.z),
                ],
                normal: DVec3::X,
                distance: max.x,
                texture: texture.clone(),
                tex_projection: Default::default(),
            },
            Face {
                vertices: vec![
                    DVec3::new(min.x, min.y, min.z),
                    DVec3::new(min.x, min.y, max.z),
                    DVec3::new(max.x, min.y, max.z),
                    DVec3::new(max.x, min.y, min.z),
                ],
                normal: DVec3::NEG_Y,
                distance: -min.y,
                texture: texture.clone(),
                tex_projection: Default::default(),
            },
            Face {
                vertices: vec![
                    DVec3::new(min.x, max.y, min.z),
                    DVec3::new(max.x, max.y, min.z),
                    DVec3::new(max.x, max.y, max.z),
                    DVec3::new(min.x, max.y, max.z),
                ],
                normal: DVec3::Y,
                distance: max.y,
                texture: texture.clone(),
                tex_projection: Default::default(),
            },
            Face {
                vertices: vec![
                    DVec3::new(min.x, min.y, min.z),
                    DVec3::new(max.x, min.y, min.z),
                    DVec3::new(max.x, max.y, min.z),
                    DVec3::new(min.x, max.y, min.z),
                ],
                normal: DVec3::NEG_Z,
                distance: -min.z,
                texture: texture.clone(),
                tex_projection: Default::default(),
            },
            Face {
                vertices: vec![
                    DVec3::new(min.x, min.y, max.z),
                    DVec3::new(max.x, min.y, max.z),
                    DVec3::new(max.x, max.y, max.z),
                    DVec3::new(min.x, max.y, max.z),
                ],
                normal: DVec3::Z,
                distance: max.z,
                texture: texture.clone(),
                tex_projection: Default::default(),
            },
        ]
    }

    fn box_brush(min: DVec3, max: DVec3) -> BrushVolume {
        use crate::map_data::BrushPlane;
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
            aabb: Aabb { min, max },
        }
    }

    /// Build a hollow room from 6 wall brushes (floor, ceiling, 4 walls).
    fn hollow_room(min: DVec3, max: DVec3, wall: f64) -> (Vec<Face>, Vec<BrushVolume>) {
        let mut faces = Vec::new();
        let mut brushes = Vec::new();

        let slabs = [
            // Floor
            (
                DVec3::new(min.x, min.y, min.z),
                DVec3::new(max.x, min.y + wall, max.z),
            ),
            // Ceiling
            (
                DVec3::new(min.x, max.y - wall, min.z),
                DVec3::new(max.x, max.y, max.z),
            ),
            // Wall -X
            (
                DVec3::new(min.x, min.y, min.z),
                DVec3::new(min.x + wall, max.y, max.z),
            ),
            // Wall +X
            (
                DVec3::new(max.x - wall, min.y, min.z),
                DVec3::new(max.x, max.y, max.z),
            ),
            // Wall -Z
            (
                DVec3::new(min.x, min.y, min.z),
                DVec3::new(max.x, max.y, min.z + wall),
            ),
            // Wall +Z
            (
                DVec3::new(min.x, min.y, max.z - wall),
                DVec3::new(max.x, max.y, max.z),
            ),
        ];

        for (b_min, b_max) in slabs {
            faces.extend(make_box_faces(b_min, b_max));
            brushes.push(box_brush(b_min, b_max));
        }

        (faces, brushes)
    }

    /// Walk the BSP tree to find which leaf contains a given point.
    fn find_leaf_for_point(tree: &BspTree, point: DVec3) -> usize {
        if tree.nodes.is_empty() {
            return 0;
        }
        let mut child = BspChild::Node(0);
        loop {
            match child {
                BspChild::Leaf(idx) => return idx,
                BspChild::Node(idx) => {
                    let node = &tree.nodes[idx];
                    let dist = point.dot(node.plane_normal) - node.plane_distance;
                    child = if dist >= 0.0 {
                        node.front.clone()
                    } else {
                        node.back.clone()
                    };
                }
            }
        }
    }

    #[test]
    fn floating_cube_air_space_leaves_stay_empty() {
        // A hollow room with a wall that has a narrow gap. The wall is made of
        // two brushes separated by a small air gap. BSP partitioning can create
        // a leaf that spans part of a brush and the adjacent air gap. The leaf
        // solidity classifier must NOT mark such a leaf as solid when it
        // contains air space.
        //
        // This reproduces the same topology as a pillar with a doorway: adjacent
        // solid brushes with a narrow air gap between them.
        let (mut faces, mut brushes) =
            hollow_room(DVec3::ZERO, DVec3::new(256.0, 128.0, 128.0), 16.0);

        // Interior is (16,16,16) to (240,112,112).
        // Add two wall sections at X=120..136, separated by a small air gap.
        //   - Solid wall: Z=16..64
        //   - AIR GAP: Z=64..66 (only 2 units)
        //   - Solid pillar: Z=66..68 (small, 2 units)
        //   - AIR GAP: Z=68..70 (only 2 units)
        //   - Solid wall: Z=70..112
        let wall1_min = DVec3::new(120.0, 16.0, 16.0);
        let wall1_max = DVec3::new(136.0, 112.0, 64.0);
        faces.extend(make_box_faces(wall1_min, wall1_max));
        brushes.push(box_brush(wall1_min, wall1_max));

        let pillar_min = DVec3::new(120.0, 16.0, 66.0);
        let pillar_max = DVec3::new(136.0, 112.0, 68.0);
        faces.extend(make_box_faces(pillar_min, pillar_max));
        brushes.push(box_brush(pillar_min, pillar_max));

        let wall2_min = DVec3::new(120.0, 16.0, 70.0);
        let wall2_max = DVec3::new(136.0, 112.0, 112.0);
        faces.extend(make_box_faces(wall2_min, wall2_max));
        brushes.push(box_brush(wall2_min, wall2_max));

        let (mut tree, split_faces) = build_bsp_tree(faces).expect("BSP build should succeed");
        classify_leaf_solidity(&mut tree, &split_faces, &brushes);

        // The air gaps are at Z=64..66 and Z=68..70 within the wall at X=120..136.
        // Test points in these air gaps to ensure they're not classified as solid.
        let air_gap_points = [
            DVec3::new(128.0, 64.0, 65.0),   // center of left air gap
            DVec3::new(128.0, 64.0, 69.0),   // center of right air gap
        ];

        for &point in &air_gap_points {
            let leaf_idx = find_leaf_for_point(&tree, point);
            let leaf = &tree.leaves[leaf_idx];
            assert!(
                !leaf.is_solid,
                "air-gap point ({}, {}, {}) falls in leaf {leaf_idx} which is classified solid. \
                 A narrow air gap between brushes was misclassified as solid. \
                 Leaf bounds: ({:.1},{:.1},{:.1})..({:.1},{:.1},{:.1}), faces: {}",
                point.x, point.y, point.z,
                leaf.bounds.min.x, leaf.bounds.min.y, leaf.bounds.min.z,
                leaf.bounds.max.x, leaf.bounds.max.y, leaf.bounds.max.z,
                leaf.face_indices.len()
            );
        }
    }
}
