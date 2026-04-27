// BSP tree construction and brush-side face extraction. Wires the
// brush-volume builder (`brush_bsp`) into the world-face emitter
// (`face_extract`) and exposes the combined output to downstream stages.
// See: context/lib/build_pipeline.md §PRL Compilation

pub(crate) mod brush_bsp;
mod bsp;
mod face_extract;
mod types;

pub use brush_bsp::build_bsp_from_brushes;
pub use bsp::find_leaf_for_point;
pub use face_extract::{CoplanarConflict, FaceExtractionResult, extract_faces};
pub use types::*;

use crate::map_data::{BrushVolume, Face};
use anyhow::Result;

/// Partition world brushes into a BSP tree with structural solid/empty leaf
/// classification, then extract world faces from brush sides.
///
/// Coordinates are expected in engine space (Y-up); the Quake-to-engine
/// transform is applied at the parse boundary.
///
/// `extract_faces` populates each empty leaf's `face_indices` in place as a
/// side-effect on the borrowed tree, so the returned `PartitionResult.tree`
/// and `PartitionResult.faces` are co-owned: a leaf's face index list points
/// into the same `faces` vector this function returns.
pub fn partition(brush_volumes: &[BrushVolume]) -> Result<PartitionResult> {
    if brush_volumes.is_empty() {
        return Ok(PartitionResult {
            tree: BspTree {
                nodes: Vec::new(),
                leaves: Vec::new(),
            },
            faces: Vec::new(),
        });
    }

    let mut tree = build_bsp_from_brushes(brush_volumes)?;

    let FaceExtractionResult {
        faces,
        coplanar_conflicts,
    } = extract_faces(&mut tree, brush_volumes);

    if !coplanar_conflicts.is_empty() {
        log::info!(
            "Coplanar dedup resolved {} brush-side conflict(s)",
            coplanar_conflicts.len()
        );
    }

    Ok(PartitionResult { tree, faces })
}

pub fn log_stats(tree: &BspTree, faces: &[Face]) {
    let solid_count = tree.leaves.iter().filter(|l| l.is_solid).count();
    let empty_count = tree.leaves.len() - solid_count;
    log::info!("BSP leaves: {solid_count} solid, {empty_count} empty");

    let max_depth = compute_max_depth(tree);
    let avg_faces: f64 = if tree.leaves.is_empty() {
        0.0
    } else {
        let total: usize = tree.leaves.iter().map(|l| l.face_indices.len()).sum();
        total as f64 / tree.leaves.len() as f64
    };

    log::info!("BSP nodes: {}", tree.nodes.len());
    log::info!("BSP leaves: {}", tree.leaves.len());
    log::info!("BSP max depth: {max_depth}");
    log::info!("Total faces (after brush-side extraction): {}", faces.len());
    log::info!("Average faces per leaf: {avg_faces:.1}");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_data::{BrushPlane, BrushSide, BrushVolume, TextureProjection};
    use glam::DVec3;

    fn tex_projection() -> TextureProjection {
        TextureProjection::default()
    }

    /// Build an axis-aligned box brush with textured sides, suitable as a
    /// direct input to the brush-volume partition pipeline.
    fn box_brush(min: DVec3, max: DVec3) -> BrushVolume {
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
                texture: "test".to_string(),
                tex_projection: tex_projection(),
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
                texture: "test".to_string(),
                tex_projection: tex_projection(),
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
                texture: "test".to_string(),
                tex_projection: tex_projection(),
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
                texture: "test".to_string(),
                tex_projection: tex_projection(),
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
                texture: "test".to_string(),
                tex_projection: tex_projection(),
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
                texture: "test".to_string(),
                tex_projection: tex_projection(),
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
            aabb: Aabb { min, max },
        }
    }

    /// Build a hollow room from 6 wall brushes (floor, ceiling, 4 walls).
    fn hollow_room(min: DVec3, max: DVec3, wall: f64) -> Vec<BrushVolume> {
        vec![
            box_brush(
                DVec3::new(min.x, min.y, min.z),
                DVec3::new(max.x, min.y + wall, max.z),
            ),
            box_brush(
                DVec3::new(min.x, max.y - wall, min.z),
                DVec3::new(max.x, max.y, max.z),
            ),
            box_brush(
                DVec3::new(min.x, min.y, min.z),
                DVec3::new(min.x + wall, max.y, max.z),
            ),
            box_brush(
                DVec3::new(max.x - wall, min.y, min.z),
                DVec3::new(max.x, max.y, max.z),
            ),
            box_brush(
                DVec3::new(min.x, min.y, min.z),
                DVec3::new(max.x, max.y, min.z + wall),
            ),
            box_brush(
                DVec3::new(min.x, min.y, max.z - wall),
                DVec3::new(max.x, max.y, max.z),
            ),
        ]
    }

    #[test]
    fn single_brush_produces_leaves() {
        let brushes = vec![box_brush(DVec3::ZERO, DVec3::new(64.0, 64.0, 64.0))];
        let result = partition(&brushes).expect("partition should succeed");

        assert!(!result.tree.leaves.is_empty(), "should produce leaves");
        assert!(!result.faces.is_empty());
    }

    #[test]
    fn two_disjoint_brushes_produce_multiple_leaves() {
        let brushes = vec![
            box_brush(DVec3::ZERO, DVec3::new(64.0, 64.0, 64.0)),
            box_brush(
                DVec3::new(1000.0, 1000.0, 1000.0),
                DVec3::new(1064.0, 1064.0, 1064.0),
            ),
        ];

        let result = partition(&brushes).expect("partition should succeed");

        assert!(
            result.tree.leaves.len() >= 2,
            "two distant brushes should produce at least 2 leaves, got {}",
            result.tree.leaves.len()
        );
    }

    #[test]
    fn empty_input_produces_empty_output() {
        let result = partition(&[]).expect("empty partition should succeed");
        assert!(result.tree.nodes.is_empty());
        assert!(result.tree.leaves.is_empty());
        assert!(result.faces.is_empty());
    }

    #[test]
    fn every_face_maps_to_exactly_one_leaf() {
        let brushes = vec![
            box_brush(DVec3::ZERO, DVec3::new(64.0, 64.0, 64.0)),
            box_brush(DVec3::new(200.0, 0.0, 0.0), DVec3::new(264.0, 64.0, 64.0)),
            box_brush(DVec3::new(0.0, 200.0, 0.0), DVec3::new(64.0, 264.0, 64.0)),
        ];

        let result = partition(&brushes).expect("partition should succeed");

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
        let brushes = vec![box_brush(DVec3::ZERO, DVec3::new(64.0, 64.0, 64.0))];
        let result = partition(&brushes).expect("partition should succeed");

        for leaf in &result.tree.leaves {
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
            .and_then(|p| p.parent())
            .expect("workspace root")
            .join("assets/maps/test.map");

        let map_data =
            crate::parse::parse_map_file(&map_path, crate::map_format::MapFormat::IdTech2)
                .expect("test.map should parse");

        let result =
            partition(&map_data.brush_volumes).expect("partition should succeed on test map");

        assert!(!result.tree.leaves.is_empty(), "should produce leaves");
        assert!(!result.faces.is_empty(), "should have faces");

        // Reasonable leaf count for a 10-brush map.
        assert!(
            result.tree.leaves.len() <= 1000,
            "too many leaves ({}) for a small map",
            result.tree.leaves.len()
        );

        // With structural solidity, every face-bearing leaf is empty by
        // construction. We still expect multiple empty leaves for a map with
        // any interior air space.
        let empty_count = result.tree.leaves.iter().filter(|l| !l.is_solid).count();
        assert!(empty_count >= 1, "should have at least 1 empty leaf");
    }

    /// Two box rooms connected by a corridor. Partition should produce both
    /// solid and empty leaves, and the face extraction should emit geometry
    /// bounding the air spaces.
    #[test]
    fn two_room_map_produces_solid_and_empty_leaves() {
        let wall = 16.0;

        let mut brushes = hollow_room(DVec3::ZERO, DVec3::splat(128.0), wall);

        brushes.extend(hollow_room(
            DVec3::new(112.0, 0.0, 40.0),
            DVec3::new(272.0, 128.0, 88.0),
            wall,
        ));

        brushes.extend(hollow_room(
            DVec3::new(256.0, 0.0, 0.0),
            DVec3::new(384.0, 128.0, 128.0),
            wall,
        ));

        let result = partition(&brushes).expect("two-room partition should succeed");

        let empty_count = result.tree.leaves.iter().filter(|l| !l.is_solid).count();
        let solid_count = result.tree.leaves.len() - empty_count;

        assert!(
            empty_count >= 2,
            "two-room map should produce at least 2 empty leaves, got {empty_count}"
        );
        assert!(
            solid_count >= 1,
            "two-room map should produce at least 1 solid leaf, got {solid_count}"
        );
    }
}
