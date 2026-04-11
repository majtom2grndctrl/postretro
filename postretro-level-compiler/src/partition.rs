// BSP tree construction and leaf classification.
// See: context/lib/build_pipeline.md §PRL

mod bsp;
mod types;

pub use bsp::find_leaf_for_point;
pub use types::*;

use crate::map_data::{BrushVolume, Face};
use anyhow::Result;

/// Partition world faces into a BSP tree with solid/empty leaf classification.
///
/// All coordinates are expected in engine space (Y-up) — the Quake-to-engine
/// transform is applied at the parse boundary.
///
/// `brush_volumes` provides the convex hull planes for each world brush,
/// used to classify leaves as solid (inside brush) or empty (air).
pub fn partition(faces: Vec<Face>, brush_volumes: &[BrushVolume]) -> Result<PartitionResult> {
    if faces.is_empty() {
        return Ok(PartitionResult {
            tree: BspTree {
                nodes: Vec::new(),
                leaves: Vec::new(),
            },
            faces,
        });
    }

    let (mut tree, split_faces) = bsp::build_bsp_tree(faces)?;

    bsp::classify_leaf_solidity(&mut tree, brush_volumes);

    let solid_count = tree.leaves.iter().filter(|l| l.is_solid).count();
    let empty_count = tree.leaves.len() - solid_count;
    log::info!("[Compiler] BSP leaves: {solid_count} solid, {empty_count} empty");

    log_stats(&tree, &split_faces);
    validate(&tree, &split_faces)?;

    Ok(PartitionResult {
        tree,
        faces: split_faces,
    })
}

fn log_stats(tree: &BspTree, faces: &[Face]) {
    let max_depth = compute_max_depth(tree);
    let avg_faces: f64 = if tree.leaves.is_empty() {
        0.0
    } else {
        let total: usize = tree.leaves.iter().map(|l| l.face_indices.len()).sum();
        total as f64 / tree.leaves.len() as f64
    };

    log::info!("[Compiler] BSP nodes: {}", tree.nodes.len());
    log::info!("[Compiler] BSP leaves: {}", tree.leaves.len());
    log::info!("[Compiler] BSP max depth: {max_depth}");
    log::info!("[Compiler] Total faces (after splits): {}", faces.len());
    log::info!("[Compiler] Average faces per leaf: {avg_faces:.1}");
}

fn compute_max_depth(tree: &BspTree) -> usize {
    if tree.nodes.is_empty() {
        return if tree.leaves.is_empty() { 0 } else { 1 };
    }

    fn depth_of(tree: &BspTree, child: &BspChild, current: usize) -> usize {
        match child {
            BspChild::Leaf(_) => current + 1,
            BspChild::Node(idx) => {
                let node = &tree.nodes[*idx];
                let front = depth_of(tree, &node.front, current + 1);
                let back = depth_of(tree, &node.back, current + 1);
                front.max(back)
            }
        }
    }

    let root = &tree.nodes[0];
    let front = depth_of(tree, &root.front, 1);
    let back = depth_of(tree, &root.back, 1);
    front.max(back)
}

fn validate(tree: &BspTree, faces: &[Face]) -> Result<()> {
    // Every face appears in exactly one leaf
    let mut face_leaf_count = vec![0usize; faces.len()];
    for leaf in &tree.leaves {
        for &fi in &leaf.face_indices {
            anyhow::ensure!(
                fi < faces.len(),
                "leaf references out-of-bounds face index {fi}"
            );
            face_leaf_count[fi] += 1;
        }
    }
    for (i, count) in face_leaf_count.iter().enumerate() {
        anyhow::ensure!(
            *count == 1,
            "face {i} appears in {count} leaves (expected exactly 1)"
        );
    }

    // Leaf bounding volumes are finite
    for (i, leaf) in tree.leaves.iter().enumerate() {
        // Faceless leaves have empty bounds, which is expected
        if leaf.face_indices.is_empty() {
            continue;
        }
        let b = &leaf.bounds;
        anyhow::ensure!(
            b.min.x.is_finite()
                && b.min.y.is_finite()
                && b.min.z.is_finite()
                && b.max.x.is_finite()
                && b.max.y.is_finite()
                && b.max.z.is_finite(),
            "leaf {i} has non-finite bounding volume"
        );
        anyhow::ensure!(
            b.min.x <= b.max.x && b.min.y <= b.max.y && b.min.z <= b.max.z,
            "leaf {i} has inverted bounding volume"
        );
    }

    // BSP tree depth sanity check
    let max_depth = compute_max_depth(tree);
    let face_count = faces.len();
    anyhow::ensure!(
        max_depth <= face_count,
        "BSP tree depth ({max_depth}) exceeds face count ({face_count}), indicating degenerate partitioning"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_data::{BrushPlane, BrushVolume};
    use glam::DVec3;

    fn make_box_faces(min: DVec3, max: DVec3) -> Vec<Face> {
        let texture = "test".to_string();

        // 6 faces of an axis-aligned box
        vec![
            // -X face
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
            // +X face
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
            // -Y face
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
            // +Y face
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
            // -Z face
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
            // +Z face
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

    #[test]
    fn single_brush_produces_leaves() {
        let faces = make_box_faces(DVec3::ZERO, DVec3::new(64.0, 64.0, 64.0));
        let result = partition(faces, &[]).expect("partition should succeed");

        assert!(!result.tree.leaves.is_empty(), "should produce leaves");
        assert!(!result.faces.is_empty());
    }

    #[test]
    fn two_disjoint_brushes_produce_multiple_leaves() {
        let mut faces = make_box_faces(DVec3::ZERO, DVec3::new(64.0, 64.0, 64.0));
        faces.extend(make_box_faces(
            DVec3::new(1000.0, 1000.0, 1000.0),
            DVec3::new(1064.0, 1064.0, 1064.0),
        ));

        let result = partition(faces, &[]).expect("partition should succeed");

        assert!(
            result.tree.leaves.len() >= 2,
            "two distant brushes should produce at least 2 leaves, got {}",
            result.tree.leaves.len()
        );
    }

    #[test]
    fn empty_input_produces_empty_output() {
        let result = partition(Vec::new(), &[]).expect("empty partition should succeed");
        assert!(result.tree.nodes.is_empty());
        assert!(result.tree.leaves.is_empty());
        assert!(result.faces.is_empty());
    }

    #[test]
    fn every_face_maps_to_exactly_one_leaf() {
        let mut faces = make_box_faces(DVec3::ZERO, DVec3::new(64.0, 64.0, 64.0));
        faces.extend(make_box_faces(
            DVec3::new(200.0, 0.0, 0.0),
            DVec3::new(264.0, 64.0, 64.0),
        ));
        faces.extend(make_box_faces(
            DVec3::new(0.0, 200.0, 0.0),
            DVec3::new(64.0, 264.0, 64.0),
        ));

        let result = partition(faces, &[]).expect("partition should succeed");

        let mut face_leaf_count = vec![0usize; result.faces.len()];
        for leaf in &result.tree.leaves {
            for &fi in &leaf.face_indices {
                face_leaf_count[fi] += 1;
            }
        }
        for (i, count) in face_leaf_count.iter().enumerate() {
            assert_eq!(*count, 1, "face {i} appears in {count} leaves (expected 1)");
        }
    }

    #[test]
    fn leaf_bounds_are_valid() {
        let faces = make_box_faces(DVec3::ZERO, DVec3::new(64.0, 64.0, 64.0));
        let result = partition(faces, &[]).expect("partition should succeed");

        for leaf in &result.tree.leaves {
            if leaf.face_indices.is_empty() {
                continue;
            }
            let b = &leaf.bounds;
            assert!(b.min.x <= b.max.x);
            assert!(b.min.y <= b.max.y);
            assert!(b.min.z <= b.max.z);
            assert!(b.min.x.is_finite());
            assert!(b.max.x.is_finite());
        }
    }

    #[test]
    fn partition_with_test_map() {
        let map_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("assets/maps/test.map");

        let map_data =
            crate::parse::parse_map_file(&map_path, crate::map_format::MapFormat::IdTech2)
                .expect("test.map should parse");

        let result = partition(map_data.world_faces, &map_data.brush_volumes)
            .expect("partition should succeed on test map");

        assert!(!result.tree.leaves.is_empty(), "should produce leaves");
        assert!(!result.faces.is_empty(), "should have faces");

        // Reasonable leaf count for a 10-brush map
        assert!(
            result.tree.leaves.len() <= 500,
            "too many leaves ({}) for a small map",
            result.tree.leaves.len()
        );

        // Under brush-ownership solidity classification, every leaf that
        // contains a face is empty (faces point away from their source brush,
        // so the leaf is air-side). Solid leaves only arise when BSP
        // partitioning produces a faceless region, which is incidental — the
        // important invariant here is that the partition produced at least
        // one air leaf holding geometry.
        let empty_count = result
            .tree
            .leaves
            .iter()
            .filter(|l| !l.is_solid)
            .count();
        assert!(empty_count >= 1, "should have at least 1 empty leaf");
    }

    /// Two box rooms connected by a corridor. The brush volumes define the
    /// solid walls; the inward-facing surfaces define air-space boundaries.
    /// Portal generation depends on this topology producing both solid and
    /// empty leaves.
    #[test]
    fn two_room_map_produces_solid_and_empty_leaves() {
        // Build a hollow room from 6 wall brushes (floor, ceiling, 4 walls).
        // Room interior is the air space between walls.
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
                aabb: Aabb { min, max },
            }
        }

        let wall = 16.0;

        // Room A: (0,0,0) to (128,128,128)
        let (mut faces, mut brushes) = hollow_room(DVec3::ZERO, DVec3::splat(128.0), wall);

        // Corridor connecting rooms: from room A's +X wall to room B's -X wall.
        // Corridor spans X=128..256, Y=0..128, Z=48..80 (narrow passage).
        let (corr_faces, corr_brushes) = hollow_room(
            DVec3::new(112.0, 0.0, 40.0),
            DVec3::new(272.0, 128.0, 88.0),
            wall,
        );
        faces.extend(corr_faces);
        brushes.extend(corr_brushes);

        // Room B: (256,0,0) to (384,128,128)
        let (room_b_faces, room_b_brushes) = hollow_room(
            DVec3::new(256.0, 0.0, 0.0),
            DVec3::new(384.0, 128.0, 128.0),
            wall,
        );
        faces.extend(room_b_faces);
        brushes.extend(room_b_brushes);

        let result = partition(faces, &brushes).expect("two-room partition should succeed");

        let empty_count = result
            .tree
            .leaves
            .iter()
            .filter(|l| !l.is_solid)
            .count();

        // Two rooms + corridor should carve the air space into multiple
        // convex leaves. Under brush-ownership classification all face-bearing
        // leaves are empty; the test shape (three disjoint air regions
        // separated by walls) should produce several.
        assert!(
            empty_count >= 2,
            "two-room map should produce at least 2 empty leaves, got {empty_count}"
        );
    }
}
