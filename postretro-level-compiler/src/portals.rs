// Portal generation: emit portal polygons between adjacent BSP leaves.
// Portals are compile-time only — consumed by the vis stage, then discarded.
// Algorithm: recursive portal distribution (ericw-tools shape).
// See: context/lib/build_pipeline.md §PRL Compilation

use glam::DVec3;

use crate::geometry_utils::{clip_polygon_to_front, split_polygon};
use crate::partition::{BspChild, BspTree};

/// Tighter epsilon for portal clipping. Portals are clipped against many
/// ancestor planes in sequence; the generous PLANE_EPSILON (0.1) used for
/// BSP face classification would accumulate too much error. Consistent with
/// ericw-tools' ON_EPSILON for winding operations.
const PORTAL_EPSILON: f64 = 0.01;

/// Half-extent of the initial portal winding. Large enough to cover any
/// reasonable level geometry.
const WINDING_HALF_EXTENT: f64 = 16384.0;

/// Minimum polygon area to keep a portal winding. Slivers below this
/// threshold are discarded to prevent accumulation of degenerate geometry
/// from numerical precision loss.
const MIN_WINDING_AREA: f64 = 0.1;

/// A portal connecting two adjacent BSP leaves through a splitting plane.
pub struct Portal {
    /// Convex polygon in engine coordinates.
    pub polygon: Vec<DVec3>,
    /// Index into `BspTree::leaves` for the front side.
    pub front_leaf: usize,
    /// Index into `BspTree::leaves` for the back side.
    pub back_leaf: usize,
}

/// Generate portals for all adjacent empty leaf pairs in the BSP tree.
///
/// Walks the tree recursively, creating a portal winding at each internal
/// node by clipping a large initial polygon against ancestor splitting planes,
/// then distributing the surviving winding through both subtrees to find
/// actual leaf pairs.
pub fn generate_portals(tree: &BspTree) -> Vec<Portal> {
    if tree.nodes.is_empty() {
        return Vec::new();
    }

    let mut portals = Vec::new();
    let ancestor_planes: Vec<(DVec3, f64)> = Vec::new();

    generate_recursive(tree, 0, &ancestor_planes, &mut portals);

    portals
}

/// Plane representation for ancestor stack entries: (normal, distance).
type PlaneEntry = (DVec3, f64);

/// Phase 1: walk the BSP tree, generate portal windings at each node,
/// then distribute them (Phase 2) to find leaf pairs.
fn generate_recursive(
    tree: &BspTree,
    node_idx: usize,
    ancestor_planes: &[PlaneEntry],
    portals: &mut Vec<Portal>,
) {
    let node = &tree.nodes[node_idx];
    let plane_normal = node.plane_normal;
    let plane_distance = node.plane_distance;

    // Phase 1: create initial winding on this node's splitting plane,
    // clipped against all ancestor planes.
    if let Some(winding) = make_node_portal(plane_normal, plane_distance, ancestor_planes) {
        // Phase 2: distribute the winding through both subtrees.
        distribute_portal(tree, &winding, &node.front, &node.back, portals);
    }

    // Recurse into front child (plane as-is: normal points to front).
    let mut front_ancestors = ancestor_planes.to_vec();
    front_ancestors.push((plane_normal, plane_distance));

    if let BspChild::Node(child_idx) = node.front {
        generate_recursive(tree, child_idx, &front_ancestors, portals);
    }

    // Recurse into back child (negated plane: normal reversed, distance negated).
    let mut back_ancestors = ancestor_planes.to_vec();
    back_ancestors.push((-plane_normal, -plane_distance));

    if let BspChild::Node(child_idx) = node.back {
        generate_recursive(tree, child_idx, &back_ancestors, portals);
    }
}

/// Create a portal winding for a node's splitting plane, clipped against
/// all ancestor splitting planes.
///
/// Returns `None` if the winding is clipped away entirely or becomes degenerate.
fn make_node_portal(
    plane_normal: DVec3,
    plane_distance: f64,
    ancestor_planes: &[PlaneEntry],
) -> Option<Vec<DVec3>> {
    // Build an initial large quad on the splitting plane.
    let mut winding = make_base_winding(plane_normal, plane_distance);

    // Clip against each ancestor plane, keeping the front (positive) side.
    for &(anc_normal, anc_distance) in ancestor_planes {
        winding = clip_polygon_to_front(&winding, anc_normal, anc_distance, PORTAL_EPSILON)?;

        if winding.len() < 3 || polygon_area(&winding) < MIN_WINDING_AREA {
            return None;
        }
    }

    if winding.len() < 3 || polygon_area(&winding) < MIN_WINDING_AREA {
        return None;
    }

    Some(winding)
}

/// Construct a large quad centered on a plane, suitable for clipping down
/// to the actual portal polygon.
///
/// Cross the plane normal with a reference axis to get basis vectors, then
/// form a quad from +/-basis1 +/-basis2 offset to lie on the plane.
fn make_base_winding(normal: DVec3, distance: f64) -> Vec<DVec3> {
    // Choose a reference axis that isn't near-parallel to the normal.
    // If the normal is near +Z or -Z, use +X. Otherwise use +Z.
    let reference = if normal.z.abs() > 0.9 {
        DVec3::X
    } else {
        DVec3::Z
    };

    let basis1 = normal.cross(reference).normalize();
    let basis2 = normal.cross(basis1).normalize();

    let center = normal * distance;
    let half = WINDING_HALF_EXTENT;

    // Quad winding order: consistent CCW when viewed from the front (positive normal side).
    vec![
        center - basis1 * half - basis2 * half,
        center + basis1 * half - basis2 * half,
        center + basis1 * half + basis2 * half,
        center - basis1 * half + basis2 * half,
    ]
}

/// Phase 2: distribute a portal winding through the BSP subtrees to find
/// the leaf pairs it actually connects.
fn distribute_portal(
    tree: &BspTree,
    winding: &[DVec3],
    front_child: &BspChild,
    back_child: &BspChild,
    portals: &mut Vec<Portal>,
) {
    match (front_child, back_child) {
        // Base case: both sides are leaves.
        (BspChild::Leaf(f), BspChild::Leaf(b)) => {
            let front_leaf = &tree.leaves[*f];
            let back_leaf = &tree.leaves[*b];
            if !front_leaf.is_solid && !back_leaf.is_solid {
                portals.push(Portal {
                    polygon: winding.to_vec(),
                    front_leaf: *f,
                    back_leaf: *b,
                });
            }
        }

        // Front is a node: split winding by that node's plane, recurse.
        (BspChild::Node(n), _) => {
            let split_node = &tree.nodes[*n];
            let (front_winding, back_winding) = split_polygon(
                winding,
                split_node.plane_normal,
                split_node.plane_distance,
                PORTAL_EPSILON,
            );

            if let Some(fw) = front_winding {
                if fw.len() >= 3 && polygon_area(&fw) >= MIN_WINDING_AREA {
                    distribute_portal(tree, &fw, &split_node.front, back_child, portals);
                }
            }
            if let Some(bw) = back_winding {
                if bw.len() >= 3 && polygon_area(&bw) >= MIN_WINDING_AREA {
                    distribute_portal(tree, &bw, &split_node.back, back_child, portals);
                }
            }
        }

        // Back is a node: split winding by that node's plane, recurse.
        (_, BspChild::Node(n)) => {
            let split_node = &tree.nodes[*n];
            let (front_winding, back_winding) = split_polygon(
                winding,
                split_node.plane_normal,
                split_node.plane_distance,
                PORTAL_EPSILON,
            );

            if let Some(fw) = front_winding {
                if fw.len() >= 3 && polygon_area(&fw) >= MIN_WINDING_AREA {
                    distribute_portal(tree, &fw, front_child, &split_node.front, portals);
                }
            }
            if let Some(bw) = back_winding {
                if bw.len() >= 3 && polygon_area(&bw) >= MIN_WINDING_AREA {
                    distribute_portal(tree, &bw, front_child, &split_node.back, portals);
                }
            }
        }
    }
}

/// Compute the area of a convex polygon using the cross-product method.
fn polygon_area(vertices: &[DVec3]) -> f64 {
    if vertices.len() < 3 {
        return 0.0;
    }

    let mut area = DVec3::ZERO;
    let v0 = vertices[0];
    for i in 1..vertices.len() - 1 {
        let edge1 = vertices[i] - v0;
        let edge2 = vertices[i + 1] - v0;
        area += edge1.cross(edge2);
    }
    area.length() * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_data::{BrushPlane, BrushVolume, Face};
    use crate::partition::{self, BspTree};

    // -- Helper: build a box room's faces and brush volumes --

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
                brush_index: 0,
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
                brush_index: 0,
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
                brush_index: 0,
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
                brush_index: 0,
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
                brush_index: 0,
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
                brush_index: 0,
            },
        ]
    }

    fn box_brush(min: DVec3, max: DVec3) -> BrushVolume {
        use crate::map_data::{BrushSide, TextureProjection};
        let tex = "test".to_string();
        let projection = TextureProjection::default();
        let sides = vec![
            BrushSide {
                vertices: vec![
                    DVec3::new(max.x, min.y, min.z),
                    DVec3::new(max.x, min.y, max.z),
                    DVec3::new(max.x, max.y, max.z),
                    DVec3::new(max.x, max.y, min.z),
                ],
                normal: DVec3::X,
                distance: max.x,
                texture: tex.clone(),
                tex_projection: projection.clone(),
            },
            BrushSide {
                vertices: vec![
                    DVec3::new(min.x, min.y, min.z),
                    DVec3::new(min.x, max.y, min.z),
                    DVec3::new(min.x, max.y, max.z),
                    DVec3::new(min.x, min.y, max.z),
                ],
                normal: DVec3::NEG_X,
                distance: -min.x,
                texture: tex.clone(),
                tex_projection: projection.clone(),
            },
            BrushSide {
                vertices: vec![
                    DVec3::new(min.x, max.y, min.z),
                    DVec3::new(max.x, max.y, min.z),
                    DVec3::new(max.x, max.y, max.z),
                    DVec3::new(min.x, max.y, max.z),
                ],
                normal: DVec3::Y,
                distance: max.y,
                texture: tex.clone(),
                tex_projection: projection.clone(),
            },
            BrushSide {
                vertices: vec![
                    DVec3::new(min.x, min.y, min.z),
                    DVec3::new(min.x, min.y, max.z),
                    DVec3::new(max.x, min.y, max.z),
                    DVec3::new(max.x, min.y, min.z),
                ],
                normal: DVec3::NEG_Y,
                distance: -min.y,
                texture: tex.clone(),
                tex_projection: projection.clone(),
            },
            BrushSide {
                vertices: vec![
                    DVec3::new(min.x, min.y, max.z),
                    DVec3::new(max.x, min.y, max.z),
                    DVec3::new(max.x, max.y, max.z),
                    DVec3::new(min.x, max.y, max.z),
                ],
                normal: DVec3::Z,
                distance: max.z,
                texture: tex.clone(),
                tex_projection: projection.clone(),
            },
            BrushSide {
                vertices: vec![
                    DVec3::new(min.x, min.y, min.z),
                    DVec3::new(max.x, min.y, min.z),
                    DVec3::new(max.x, max.y, min.z),
                    DVec3::new(min.x, max.y, min.z),
                ],
                normal: DVec3::NEG_Z,
                distance: -min.z,
                texture: tex,
                tex_projection: projection,
            },
        ];
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
            sides,
            aabb: crate::partition::Aabb { min, max },
        }
    }

    /// Build a hollow room from 6 wall brushes (floor, ceiling, 4 walls).
    ///
    /// Returns both the outward-facing face list and the brush volumes that
    /// own them. Tests that only need brushes (the new brush-volume partition
    /// pipeline) can ignore the face list with `let (_, brushes) = ...`.
    fn hollow_room(min: DVec3, max: DVec3, wall: f64) -> (Vec<Face>, Vec<BrushVolume>) {
        let mut faces = Vec::new();
        let mut brushes = Vec::new();

        // Floor slab
        let b_min = DVec3::new(min.x, min.y, min.z);
        let b_max = DVec3::new(max.x, min.y + wall, max.z);
        faces.extend(make_box_faces(b_min, b_max));
        brushes.push(box_brush(b_min, b_max));

        // Ceiling slab
        let b_min = DVec3::new(min.x, max.y - wall, min.z);
        let b_max = DVec3::new(max.x, max.y, max.z);
        faces.extend(make_box_faces(b_min, b_max));
        brushes.push(box_brush(b_min, b_max));

        // Wall -X
        let b_min = DVec3::new(min.x, min.y, min.z);
        let b_max = DVec3::new(min.x + wall, max.y, max.z);
        faces.extend(make_box_faces(b_min, b_max));
        brushes.push(box_brush(b_min, b_max));

        // Wall +X
        let b_min = DVec3::new(max.x - wall, min.y, min.z);
        let b_max = DVec3::new(max.x, max.y, max.z);
        faces.extend(make_box_faces(b_min, b_max));
        brushes.push(box_brush(b_min, b_max));

        // Wall -Z
        let b_min = DVec3::new(min.x, min.y, min.z);
        let b_max = DVec3::new(max.x, max.y, min.z + wall);
        faces.extend(make_box_faces(b_min, b_max));
        brushes.push(box_brush(b_min, b_max));

        // Wall +Z
        let b_min = DVec3::new(min.x, min.y, max.z - wall);
        let b_max = DVec3::new(max.x, max.y, max.z);
        faces.extend(make_box_faces(b_min, b_max));
        brushes.push(box_brush(b_min, b_max));

        (faces, brushes)
    }

    // -- Unit test helpers --

    fn assert_portal_polygon_valid(portal: &Portal) {
        assert!(
            portal.polygon.len() >= 3,
            "portal polygon has {} vertices, need at least 3",
            portal.polygon.len()
        );

        // Compute portal plane from first 3 vertices.
        let v0 = portal.polygon[0];
        let v1 = portal.polygon[1];
        let v2 = portal.polygon[2];
        let normal = (v1 - v0).cross(v2 - v0);
        if normal.length() < 1e-6 {
            // Degenerate triangle — skip planarity check.
            return;
        }
        let normal = normal.normalize();
        let distance = v0.dot(normal);

        // All vertices should be within epsilon of the portal plane.
        for (i, v) in portal.polygon.iter().enumerate() {
            let d = v.dot(normal) - distance;
            assert!(
                d.abs() < 0.05,
                "portal vertex {i} is {d:.6} off the portal plane (limit 0.05)"
            );
        }
    }

    // -- Tests --

    #[test]
    fn base_winding_lies_on_plane() {
        let normal = DVec3::Y;
        let distance = 5.0;
        let winding = make_base_winding(normal, distance);

        assert_eq!(winding.len(), 4);
        for v in &winding {
            let d = v.dot(normal) - distance;
            assert!(d.abs() < 1e-4, "winding vertex {v} not on plane (d={d})");
        }
    }

    #[test]
    fn base_winding_non_degenerate_for_axis_aligned_normals() {
        for normal in [
            DVec3::X,
            DVec3::Y,
            DVec3::Z,
            DVec3::NEG_X,
            DVec3::NEG_Y,
            DVec3::NEG_Z,
        ] {
            let winding = make_base_winding(normal, 0.0);
            let area = polygon_area(&winding);
            assert!(
                area > 1.0,
                "winding for normal {normal} has area {area}, expected large"
            );
        }
    }

    #[test]
    fn polygon_area_of_unit_square() {
        let verts = vec![
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(1.0, 1.0, 0.0),
            DVec3::new(0.0, 1.0, 0.0),
        ];
        let area = polygon_area(&verts);
        assert!((area - 1.0).abs() < 1e-4, "expected area 1.0, got {area}");
    }

    #[test]
    fn empty_tree_produces_no_portals() {
        let tree = BspTree {
            nodes: Vec::new(),
            leaves: Vec::new(),
        };
        let portals = generate_portals(&tree);
        assert!(portals.is_empty());
    }

    #[test]
    fn single_box_room_produces_portals() {
        // A box room is a single solid brush. The BSP tree classifies every
        // sub-region of its interior as solid. Portal generation should find
        // no portals (no adjacent empty leaves).
        let _faces = make_box_faces(DVec3::ZERO, DVec3::new(64.0, 64.0, 64.0));
        let brushes = vec![box_brush(DVec3::ZERO, DVec3::new(64.0, 64.0, 64.0))];

        let result = partition::partition(&brushes).expect("partition should succeed");

        let portals = generate_portals(&result.tree);

        // With a single box, all leaves are likely solid (the box is a solid brush),
        // so we expect 0 portals between empty leaves. This is correct behavior:
        // a solid box has no air space.
        // All portals (if any) should have valid polygons.
        for portal in &portals {
            assert_portal_polygon_valid(portal);
        }
    }

    #[test]
    fn minimal_room_divided_by_one_plane_produces_one_portal() {
        // Construct a minimal BSP tree manually: one node splitting two empty leaves.
        use crate::partition::{Aabb, BspChild, BspLeaf, BspNode};

        let tree = BspTree {
            nodes: vec![BspNode {
                plane_normal: DVec3::X,
                plane_distance: 32.0,
                front: BspChild::Leaf(0),
                back: BspChild::Leaf(1),
                parent: None,
            }],
            leaves: vec![
                BspLeaf {
                    face_indices: vec![0],
                    bounds: Aabb {
                        min: DVec3::new(32.0, 0.0, 0.0),
                        max: DVec3::new(64.0, 64.0, 64.0),
                    },
                    is_solid: false,
                },
                BspLeaf {
                    face_indices: vec![1],
                    bounds: Aabb {
                        min: DVec3::new(0.0, 0.0, 0.0),
                        max: DVec3::new(32.0, 64.0, 64.0),
                    },
                    is_solid: false,
                },
            ],
        };

        let portals = generate_portals(&tree);
        assert_eq!(
            portals.len(),
            1,
            "one splitting plane between two empty leaves should produce exactly 1 portal"
        );
        assert_portal_polygon_valid(&portals[0]);

        // The portal should reference both leaves.
        let leaf_set = [portals[0].front_leaf, portals[0].back_leaf];
        assert!(leaf_set.contains(&0), "portal should reference leaf 0");
        assert!(leaf_set.contains(&1), "portal should reference leaf 1");
    }

    #[test]
    fn solid_leaves_excluded_from_portals() {
        // One node splitting a solid leaf and an empty leaf — no portal emitted.
        use crate::partition::{Aabb, BspChild, BspLeaf, BspNode};

        let tree = BspTree {
            nodes: vec![BspNode {
                plane_normal: DVec3::X,
                plane_distance: 32.0,
                front: BspChild::Leaf(0),
                back: BspChild::Leaf(1),
                parent: None,
            }],
            leaves: vec![
                BspLeaf {
                    face_indices: vec![0],
                    bounds: Aabb {
                        min: DVec3::new(32.0, 0.0, 0.0),
                        max: DVec3::new(64.0, 64.0, 64.0),
                    },
                    is_solid: true, // solid
                },
                BspLeaf {
                    face_indices: vec![1],
                    bounds: Aabb {
                        min: DVec3::new(0.0, 0.0, 0.0),
                        max: DVec3::new(32.0, 64.0, 64.0),
                    },
                    is_solid: false,
                },
            ],
        };

        let portals = generate_portals(&tree);
        assert!(
            portals.is_empty(),
            "portal between solid and empty leaf should not be emitted"
        );
    }

    #[test]
    fn portal_polygons_are_planar() {
        // Three empty leaves in a chain: leaf0 | leaf1 | leaf2
        use crate::partition::{Aabb, BspChild, BspLeaf, BspNode};

        let tree = BspTree {
            nodes: vec![
                // Root: split at X=32
                BspNode {
                    plane_normal: DVec3::X,
                    plane_distance: 32.0,
                    front: BspChild::Node(1),
                    back: BspChild::Leaf(0),
                    parent: None,
                },
                // Child: split at X=64
                BspNode {
                    plane_normal: DVec3::X,
                    plane_distance: 64.0,
                    front: BspChild::Leaf(2),
                    back: BspChild::Leaf(1),
                    parent: Some(0),
                },
            ],
            leaves: vec![
                BspLeaf {
                    face_indices: vec![],
                    bounds: Aabb {
                        min: DVec3::ZERO,
                        max: DVec3::new(32.0, 64.0, 64.0),
                    },
                    is_solid: false,
                },
                BspLeaf {
                    face_indices: vec![],
                    bounds: Aabb {
                        min: DVec3::new(32.0, 0.0, 0.0),
                        max: DVec3::new(64.0, 64.0, 64.0),
                    },
                    is_solid: false,
                },
                BspLeaf {
                    face_indices: vec![],
                    bounds: Aabb {
                        min: DVec3::new(64.0, 0.0, 0.0),
                        max: DVec3::new(96.0, 64.0, 64.0),
                    },
                    is_solid: false,
                },
            ],
        };

        let portals = generate_portals(&tree);
        assert_eq!(portals.len(), 2, "chain of 3 leaves should have 2 portals");

        for portal in &portals {
            assert_portal_polygon_valid(portal);
        }
    }

    #[test]
    fn two_room_map_produces_portals_at_doorway() {
        let wall = 16.0;

        // Room A
        let (_faces, mut brushes) = hollow_room(DVec3::ZERO, DVec3::splat(128.0), wall);

        // Corridor connecting rooms
        let (_corr_faces, corr_brushes) = hollow_room(
            DVec3::new(112.0, 0.0, 40.0),
            DVec3::new(272.0, 128.0, 88.0),
            wall,
        );
        brushes.extend(corr_brushes);

        // Room B
        let (_room_b_faces, room_b_brushes) = hollow_room(
            DVec3::new(256.0, 0.0, 0.0),
            DVec3::new(384.0, 128.0, 128.0),
            wall,
        );
        brushes.extend(room_b_brushes);

        let result = partition::partition(&brushes).expect("partition should succeed");

        let portals = generate_portals(&result.tree);

        // Should have at least 1 portal (doorway connections).
        assert!(
            !portals.is_empty(),
            "two-room map with corridor should produce at least 1 portal"
        );

        // Every portal should be a valid polygon.
        for portal in &portals {
            assert_portal_polygon_valid(portal);
        }

        // Every portal should connect two empty leaves.
        for portal in &portals {
            let fl = &result.tree.leaves[portal.front_leaf];
            let bl = &result.tree.leaves[portal.back_leaf];
            assert!(!fl.is_solid, "portal front_leaf should not be solid");
            assert!(!bl.is_solid, "portal back_leaf should not be solid");
        }
    }

    #[test]
    fn portals_with_test_map() {
        let map_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("assets/maps/test.map");

        let map_data =
            crate::parse::parse_map_file(&map_path, crate::map_format::MapFormat::IdTech2)
                .expect("test.map should parse");

        let result = partition::partition(&map_data.brush_volumes)
            .expect("partition should succeed on test map");

        let portals = generate_portals(&result.tree);

        // All portals should be valid polygons between empty leaves.
        for portal in &portals {
            assert_portal_polygon_valid(portal);
            assert!(
                !result.tree.leaves[portal.front_leaf].is_solid,
                "portal front_leaf should not be solid"
            );
            assert!(
                !result.tree.leaves[portal.back_leaf].is_solid,
                "portal back_leaf should not be solid"
            );
        }
    }

    #[test]
    fn room_with_pillar_produces_portals_on_both_sides() {
        // A hollow room divided by a wall with two doorways (one on each side
        // of a central pillar). Portal generation must produce portals for BOTH
        // doorways, not just one.
        //
        // Room interior: (16,16,16) to (240,112,112) — 224 wide x 96 tall x 96 deep.
        // Dividing wall at X=120..136 with two doorways:
        //   - Wall left section: Z=16..62 (blocks left part)
        //   - LEFT DOORWAY: Z=62..64 (2 units wide)
        //   - Central pillar: Z=64..66 (2 units wide)
        //   - RIGHT DOORWAY: Z=66..68 (2 units wide)
        //   - Wall right section: Z=68..112 (blocks right part)
        let (_faces, mut brushes) =
            hollow_room(DVec3::ZERO, DVec3::new(256.0, 128.0, 128.0), 16.0);

        // Wall left section: blocks Z=16..62
        let wall_left_min = DVec3::new(120.0, 16.0, 16.0);
        let wall_left_max = DVec3::new(136.0, 112.0, 62.0);
        brushes.push(box_brush(wall_left_min, wall_left_max));

        // Central pillar: Z=64..66
        let pillar_min = DVec3::new(120.0, 16.0, 64.0);
        let pillar_max = DVec3::new(136.0, 112.0, 66.0);
        brushes.push(box_brush(pillar_min, pillar_max));

        // Wall right section: blocks Z=68..112
        let wall_right_min = DVec3::new(120.0, 16.0, 68.0);
        let wall_right_max = DVec3::new(136.0, 112.0, 112.0);
        brushes.push(box_brush(wall_right_min, wall_right_max));

        let result = partition::partition(&brushes).expect("partition should succeed");
        let portals = generate_portals(&result.tree);

        // The wall at X=120..136 has two doorways: left (Z=62..64) and right (Z=66..68).
        // There must be portals through BOTH doorways — portals that cross the
        // X=120..136 wall region. A portal crosses the wall if its polygon spans
        // or lies within the X range and the Z range of either gap.
        //
        // We identify "wall-crossing" portals as portals whose polygon lies on a
        // plane with X between 120 and 136 (inclusive) — these are the portals that
        // pass through the wall.
        let wall_x_min = 119.0;
        let wall_x_max = 137.0;
        let left_gap_z_min = 61.0;
        let left_gap_z_max = 65.0;
        let right_gap_z_min = 65.0;
        let right_gap_z_max = 69.0;

        let mut has_left_gap_portal = false;
        let mut has_right_gap_portal = false;

        for portal in &portals {
            // Check if portal polygon is in the wall's X range.
            let all_in_wall_x = portal.polygon.iter().all(|v| v.x > wall_x_min && v.x < wall_x_max);
            if !all_in_wall_x {
                continue;
            }

            // Check which gap this portal corresponds to by its Z range.
            let z_min = portal.polygon.iter().map(|v| v.z).fold(f64::MAX, f64::min);
            let z_max = portal.polygon.iter().map(|v| v.z).fold(f64::MIN, f64::max);

            // Check if this portal's Z range falls within a gap.
            if z_max > left_gap_z_min && z_min < left_gap_z_max && z_max <= left_gap_z_max {
                has_left_gap_portal = true;
            }
            if z_min >= right_gap_z_min && z_min < right_gap_z_max {
                has_right_gap_portal = true;
            }
        }

        assert!(
            has_left_gap_portal,
            "no portal found through the LEFT doorway (Z=62..64). \
             The wall's BSP splits may have clipped away the left gap portal."
        );
        assert!(
            has_right_gap_portal,
            "no portal found through the RIGHT doorway (Z=66..68). \
             The pillar's adjacent solid brush may have caused the right gap's \
             BSP leaf to be misclassified as solid, preventing portal generation."
        );
    }

    /// Floating cube near the ceiling of a hollow room.
    ///
    /// Originally a reproduction for the "missing cube faces" bug: faces of
    /// floating cube brushes near the ceiling disappeared from the compiled
    /// geometry when the face-driven BSP classified their containing leaves
    /// as solid. Under brush-volume construction the bug cannot form — leaf
    /// solidity is authoritative by construction — but the test is retained
    /// as a regression guard on the new pipeline.
    ///
    /// Each emitted face is matched to its source cube by plane distance and
    /// centroid footprint (room walls share cardinal normals, so normal alone
    /// isn't enough).
    #[test]
    fn floating_cube_near_ceiling_faces_survive_pipeline() {
        use crate::map_data::Face;

        // Match map-2's actual compiled dimensions (in engine space):
        //   room x=-32..32 (64), y=0..9 (interior 0..8, walls add 1 unit),
        //   z=-29..29 (58). Cubes are 2x2x3 slabs at y=5..7 (top ~1 unit from
        //   interior ceiling at y=8).
        //
        // This is the geometric configuration map-2 actually compiles down to,
        // so if the bug reproduces anywhere programmatically it should be here.
        let room_min = DVec3::new(-32.0, 0.0, -29.0);
        let room_max = DVec3::new(32.0, 9.0, 29.0);
        let wall = 1.0;
        let (_room_faces, mut brushes) = hollow_room(room_min, room_max, wall);

        // Floating cube: 32x32x32 centered horizontally, top 8 units below ceiling.
        // Interior ceiling plane is at y = room_max.y - wall = 112.
        // Put cube_max.y = 104 so there's an 8-unit gap above.
        // Match map-2's compiled cube geometry:
        //   ~2-3 units in X/Z footprint, y=5..7 (2 units tall).
        //   Top face at y=7, 1 unit below interior ceiling at y=8.
        let cube_xz_size = 3.0;
        let cube_y_min = 5.0;
        let cube_y_max = 7.0;
        let x_nudge = 0.0;
        let z_nudge = 0.0;
        let cube_x_center = (room_min.x + room_max.x) * 0.5 + x_nudge;
        let cube_z_center = (room_min.z + room_max.z) * 0.5 + z_nudge;
        let cube_min = DVec3::new(
            cube_x_center - cube_xz_size * 0.5,
            cube_y_min,
            cube_z_center - cube_xz_size * 0.5,
        );
        let cube_max = DVec3::new(
            cube_x_center + cube_xz_size * 0.5,
            cube_y_max,
            cube_z_center + cube_xz_size * 0.5,
        );

        // Collect (min, max) for every floating cube so we can check ALL of
        // them, not just the first.
        let mut cube_bounds: Vec<(DVec3, DVec3)> = Vec::new();
        cube_bounds.push((cube_min, cube_max));

        brushes.push(box_brush(cube_min, cube_max));

        // Add a second cube right next to the first with only a 2-unit X gap
        // (matching the typical cube spacing in map-2). This gives the BSP
        // tree a narrow-gap topology similar to what triggers the bug in the
        // real map.
        let cube2_dx = cube_xz_size + 2.0;
        let c2_min = DVec3::new(cube_min.x + cube2_dx, cube_min.y, cube_min.z);
        let c2_max = DVec3::new(cube_max.x + cube2_dx, cube_max.y, cube_max.z);
        brushes.push(box_brush(c2_min, c2_max));
        cube_bounds.push((c2_min, c2_max));

        // Return (cube_index, normal_index 0..=5) if `face` is a face of one
        // of the floating cubes, else None. A face belongs to cube i if:
        //   - its normal is axis-aligned
        //   - its plane distance matches cube i's bounding plane on that axis
        //   - its centroid lies within cube i's horizontal footprint (and at
        //     the cube's vertical extent for Y-axis faces) — this disambiguates
        //     it from coincident room-wall faces.
        let axes: [(DVec3, usize); 6] = [
            (DVec3::X, 0),
            (DVec3::NEG_X, 1),
            (DVec3::Y, 2),
            (DVec3::NEG_Y, 3),
            (DVec3::Z, 4),
            (DVec3::NEG_Z, 5),
        ];
        let cube_axis_distance = |bounds: (DVec3, DVec3), axis_idx: usize| -> f64 {
            let (mn, mx) = bounds;
            match axis_idx {
                0 => mx.x,
                1 => -mn.x,
                2 => mx.y,
                3 => -mn.y,
                4 => mx.z,
                5 => -mn.z,
                _ => f64::NAN,
            }
        };
        let classify_cube_face = |face: &Face| -> Option<(usize, usize)> {
            let n = face.normal;
            let axis_idx = axes
                .iter()
                .find(|(a, _)| (*a - n).length() < 1e-6)
                .map(|(_, i)| *i)?;

            let centroid: DVec3 = face.vertices.iter().copied().sum::<DVec3>()
                / face.vertices.len() as f64;

            for (ci, bounds) in cube_bounds.iter().enumerate() {
                let expected_d = cube_axis_distance(*bounds, axis_idx);
                if !expected_d.is_finite() {
                    continue;
                }
                if (face.distance - expected_d).abs() > 1e-4 {
                    continue;
                }

                // Centroid must lie within the cube's footprint on the other
                // two axes. Slop is tight enough to exclude room-wall faces
                // but loose enough to absorb brush-side projection splits.
                let (mn, mx) = *bounds;
                let slop = 0.5;
                let inside = centroid.x >= mn.x - slop
                    && centroid.x <= mx.x + slop
                    && centroid.y >= mn.y - slop
                    && centroid.y <= mx.y + slop
                    && centroid.z >= mn.z - slop
                    && centroid.z <= mx.z + slop;
                if inside {
                    return Some((ci, axis_idx));
                }
            }
            None
        };

        // Back-compat alias retained for the first cube's reporting path below.
        let is_cube_face = |face: &Face| -> bool {
            matches!(classify_cube_face(face), Some((0, _)))
        };
        let cube_centroid = (cube_min + cube_max) * 0.5;

        // Track which cube normals we've seen at each stage (as a set).
        let normal_key = |n: DVec3| -> &'static str {
            if (n - DVec3::X).length() < 1e-6 {
                "+X"
            } else if (n - DVec3::NEG_X).length() < 1e-6 {
                "-X"
            } else if (n - DVec3::Y).length() < 1e-6 {
                "+Y (top)"
            } else if (n - DVec3::NEG_Y).length() < 1e-6 {
                "-Y (bottom)"
            } else if (n - DVec3::Z).length() < 1e-6 {
                "+Z"
            } else if (n - DVec3::NEG_Z).length() < 1e-6 {
                "-Z"
            } else {
                "?"
            }
        };

        // Stage 1: partition (brush-volume BSP + face extraction).
        let result = partition::partition(&brushes)
            .expect("partition should succeed on floating cube scene");

        let mut stage2_count = 0usize;
        let mut stage2_normals: std::collections::BTreeSet<&'static str> =
            std::collections::BTreeSet::new();
        let mut cube_faces_in_solid_leaves = 0usize;
        let mut cube_faces_in_empty_leaves = 0usize;
        // (leaf_idx, face_idx, normal_key, is_solid, centroid)
        let mut cube_face_locations: Vec<(usize, usize, &'static str, bool, DVec3)> =
            Vec::new();

        for (leaf_idx, leaf) in result.tree.leaves.iter().enumerate() {
            for &fi in &leaf.face_indices {
                let f = &result.faces[fi];
                if is_cube_face(f) {
                    stage2_count += 1;
                    let key = normal_key(f.normal);
                    stage2_normals.insert(key);
                    if leaf.is_solid {
                        cube_faces_in_solid_leaves += 1;
                    } else {
                        cube_faces_in_empty_leaves += 1;
                    }
                    let centroid: DVec3 = f.vertices.iter().copied().sum::<DVec3>()
                        / f.vertices.len() as f64;
                    cube_face_locations.push((leaf_idx, fi, key, leaf.is_solid, centroid));
                }
            }
        }

        eprintln!(
            "[STAGE 2] Post-partition cube face fragments: {stage2_count} (in empty leaves: {cube_faces_in_empty_leaves}, in solid leaves: {cube_faces_in_solid_leaves})"
        );
        eprintln!(
            "[STAGE 2] Distinct cube normals present: {:?}",
            stage2_normals
        );
        for (leaf_idx, fi, key, is_solid, centroid) in &cube_face_locations {
            eprintln!(
                "  cube face {fi} normal={key} leaf={leaf_idx} solid={is_solid} centroid=({:.1},{:.1},{:.1})",
                centroid.x, centroid.y, centroid.z
            );
        }

        // Report which cube-1 normals are MISSING after partition.
        let all_keys: std::collections::BTreeSet<&'static str> = [
            "+X", "-X", "+Y (top)", "-Y (bottom)", "+Z", "-Z",
        ]
        .into_iter()
        .collect();
        let missing_after_partition: Vec<&&'static str> =
            all_keys.difference(&stage2_normals).collect();
        eprintln!(
            "[STAGE 2] Cube 0 normals MISSING from BSP tree: {:?}",
            missing_after_partition
        );

        // --- Per-cube coverage check across ALL cubes ---
        // For every cube, count which of its 6 axis faces survived each stage.
        let num_cubes = cube_bounds.len();
        let mut stage2_per_cube: Vec<[usize; 6]> = vec![[0; 6]; num_cubes];
        let mut stage2_per_cube_solid: Vec<[usize; 6]> = vec![[0; 6]; num_cubes];
        for leaf in &result.tree.leaves {
            for &fi in &leaf.face_indices {
                let f = &result.faces[fi];
                if let Some((ci, ai)) = classify_cube_face(f) {
                    stage2_per_cube[ci][ai] += 1;
                    if leaf.is_solid {
                        stage2_per_cube_solid[ci][ai] += 1;
                    }
                }
            }
        }

        // Stage 3: geometry extraction. extract_geometry iterates only empty
        // leaves; any face that lives solely in solid leaves is silently
        // dropped. This is the stage where the visible bug surfaces.
        let geo = crate::geometry::extract_geometry(
            &result.faces,
            &result.tree,
            &std::collections::HashSet::new(),
        );
        let mut stage3_per_cube: Vec<[usize; 6]> = vec![[0; 6]; num_cubes];
        // Classify a geometry face by axis: all vertices must lie on one of
        // the cube's axis-aligned bounding planes within epsilon.
        for meta in &geo.geometry.faces {
            let start = meta.index_offset as usize;
            let end = start + meta.index_count as usize;
            let mut unique_verts: Vec<DVec3> = Vec::new();
            for idx in start..end {
                let vi = geo.geometry.indices[idx] as usize;
                let p = &geo.geometry.vertices[vi];
                let v = DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64);
                if !unique_verts
                    .iter()
                    .any(|u| (*u - v).length_squared() < 1e-6)
                {
                    unique_verts.push(v);
                }
            }
            if unique_verts.is_empty() {
                continue;
            }
            let centroid: DVec3 =
                unique_verts.iter().copied().sum::<DVec3>() / unique_verts.len() as f64;
            // Check each cube and each axis-aligned face plane.
            for (ci, bounds) in cube_bounds.iter().enumerate() {
                let (mn, mx) = *bounds;
                // For each of the 6 planes, check if all vertices lie on it
                // and the centroid is within the cube footprint.
                let plane_eps = 0.05;
                let footprint_slop = 0.5;
                let planes: [(f64, f64, usize); 6] = [
                    // (axis_value, plane_coord, axis_idx 0..5)
                    // +X plane
                    (mx.x, 0.0, 0),
                    // -X plane
                    (mn.x, 0.0, 1),
                    // +Y plane (top)
                    (mx.y, 1.0, 2),
                    // -Y plane (bot)
                    (mn.y, 1.0, 3),
                    // +Z plane
                    (mx.z, 2.0, 4),
                    // -Z plane
                    (mn.z, 2.0, 5),
                ];
                for (plane_val, axis, axis_idx) in planes {
                    let on_plane = unique_verts.iter().all(|v| {
                        let coord = match axis as i32 {
                            0 => v.x,
                            1 => v.y,
                            _ => v.z,
                        };
                        (coord - plane_val).abs() < plane_eps
                    });
                    if !on_plane {
                        continue;
                    }
                    let inside_footprint = centroid.x >= mn.x - footprint_slop
                        && centroid.x <= mx.x + footprint_slop
                        && centroid.y >= mn.y - footprint_slop
                        && centroid.y <= mx.y + footprint_slop
                        && centroid.z >= mn.z - footprint_slop
                        && centroid.z <= mx.z + footprint_slop;
                    if inside_footprint {
                        stage3_per_cube[ci][axis_idx] += 1;
                    }
                }
            }
        }

        let axis_labels = ["+X", "-X", "+Y(top)", "-Y(bot)", "+Z", "-Z"];
        eprintln!(
            "[PER-CUBE] stage2 (post-partition) / stage3 (geometry):"
        );
        let mut any_cube_missing_bsp = false;
        let mut any_cube_missing_geometry = false;
        for ci in 0..num_cubes {
            let s2 = stage2_per_cube[ci];
            let s2_solid = stage2_per_cube_solid[ci];
            let s3 = stage3_per_cube[ci];
            let (mn, mx) = cube_bounds[ci];
            eprintln!(
                "  cube {ci} min=({:.0},{:.0},{:.0}) max=({:.0},{:.0},{:.0})",
                mn.x, mn.y, mn.z, mx.x, mx.y, mx.z
            );
            for ai in 0..6 {
                let label = axis_labels[ai];
                let bsp_marker = if s2[ai] == 0 { " BSP_MISSING!" } else { "" };
                let solid_marker = if s2_solid[ai] > 0 {
                    " (in solid leaf)"
                } else {
                    ""
                };
                let geo_marker = if s3[ai] == 0 { " GEO_MISSING!" } else { "" };
                eprintln!(
                    "    {label}: s2={} s3={}{}{}{}",
                    s2[ai], s3[ai], bsp_marker, solid_marker, geo_marker
                );
                if s2[ai] == 0 {
                    any_cube_missing_bsp = true;
                }
                if s3[ai] == 0 {
                    any_cube_missing_geometry = true;
                }
            }
        }
        let any_cube_missing_face = any_cube_missing_bsp || any_cube_missing_geometry;

        // Stage 3: portal generation.
        let portals = generate_portals(&result.tree);
        eprintln!(
            "[STAGE 3] Portal count: {} (cube has 6 sides, each adjacent to an empty leaf)",
            portals.len()
        );

        // Count portals adjacent to leaves that contain cube faces (empty-leaf only).
        let mut leaves_with_cube_faces: std::collections::BTreeSet<usize> =
            std::collections::BTreeSet::new();
        for (leaf_idx, _, _, is_solid, _) in &cube_face_locations {
            if !is_solid {
                leaves_with_cube_faces.insert(*leaf_idx);
            }
        }

        let mut portals_touching_cube_leaves = 0usize;
        for p in &portals {
            if leaves_with_cube_faces.contains(&p.front_leaf)
                || leaves_with_cube_faces.contains(&p.back_leaf)
            {
                portals_touching_cube_leaves += 1;
            }
        }
        eprintln!(
            "[STAGE 3] Portals touching a leaf that owns a cube face: {portals_touching_cube_leaves}"
        );

        // Also report the leaf the cube centroid lands in.
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
        let cube_centroid_leaf = find_leaf_for_point(&result.tree, cube_centroid);
        eprintln!(
            "[STAGE 3] Cube centroid ({:.1},{:.1},{:.1}) resides in leaf {} (solid={})",
            cube_centroid.x,
            cube_centroid.y,
            cube_centroid.z,
            cube_centroid_leaf,
            result.tree.leaves[cube_centroid_leaf].is_solid
        );

        // Probe the air-space leaves just outside each cube face and see whether
        // there's a portal path from any of them back to the room's main air
        // space. If there isn't, those faces will be invisible from the player's
        // viewpoint (portal-vis culls them).
        let probe_offset = 2.0;
        let probes: [(DVec3, &'static str); 6] = [
            (
                DVec3::new(cube_max.x + probe_offset, cube_centroid.y, cube_centroid.z),
                "+X side",
            ),
            (
                DVec3::new(cube_min.x - probe_offset, cube_centroid.y, cube_centroid.z),
                "-X side",
            ),
            (
                DVec3::new(cube_centroid.x, cube_max.y + probe_offset, cube_centroid.z),
                "+Y side (above cube, below ceiling)",
            ),
            (
                DVec3::new(cube_centroid.x, cube_min.y - probe_offset, cube_centroid.z),
                "-Y side (below cube)",
            ),
            (
                DVec3::new(cube_centroid.x, cube_centroid.y, cube_max.z + probe_offset),
                "+Z side",
            ),
            (
                DVec3::new(cube_centroid.x, cube_centroid.y, cube_min.z - probe_offset),
                "-Z side",
            ),
        ];
        // Also probe a point near the floor to represent "player starting position".
        let player_probe = DVec3::new(
            (room_min.x + room_max.x) * 0.5,
            room_min.y + wall + 16.0,
            (room_min.z + room_max.z) * 0.5,
        );
        let player_leaf = find_leaf_for_point(&result.tree, player_probe);
        eprintln!(
            "[STAGE 3] Player probe ({:.1},{:.1},{:.1}) -> leaf {} (solid={})",
            player_probe.x,
            player_probe.y,
            player_probe.z,
            player_leaf,
            result.tree.leaves[player_leaf].is_solid
        );

        for (probe, label) in &probes {
            let leaf_idx = find_leaf_for_point(&result.tree, *probe);
            let leaf = &result.tree.leaves[leaf_idx];
            eprintln!(
                "  probe {label} at ({:.1},{:.1},{:.1}) -> leaf {leaf_idx} (solid={}, faces={})",
                probe.x, probe.y, probe.z, leaf.is_solid, leaf.face_indices.len()
            );
        }

        // -- Assertions --
        // Hard invariant: every face of every floating cube must appear at
        // least once in the BSP output. If this fails, the bug has reproduced
        // and the PER-CUBE diagnostic above shows which faces were lost.
        assert!(
            !any_cube_missing_face,
            "at least one floating-cube face is missing after partition — see [PER-CUBE] output"
        );

        // All cube faces should live in empty leaves (the cube surface bounds
        // the air space above/beside/below the cube, not the solid interior).
        assert_eq!(
            cube_faces_in_solid_leaves, 0,
            "no cube-0 face should live in a solid leaf; found {cube_faces_in_solid_leaves}"
        );
    }

}
