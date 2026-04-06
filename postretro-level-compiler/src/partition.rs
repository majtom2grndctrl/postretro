// BSP tree construction and spatial clustering.
// See: context/plans/ready/prl-phase-1-minimum-viable-compiler/

mod bsp;
mod cluster;
mod types;

pub use types::*;

use crate::map_data::{BrushVolume, Face};
use anyhow::Result;

/// Partition world faces into a BSP tree and spatial clusters.
///
/// Operates in shambler's native coordinate space (right-handed, Z-up).
/// Coordinate transforms happen in later pipeline stages.
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
            clusters: Vec::new(),
        });
    }

    let (mut tree, split_faces) = bsp::build_bsp_tree(faces)?;

    bsp::classify_leaf_solidity(&mut tree, &split_faces, brush_volumes);

    let solid_count = tree.leaves.iter().filter(|l| l.is_solid).count();
    let empty_count = tree.leaves.len() - solid_count;
    log::info!("[Compiler] BSP leaves: {solid_count} solid, {empty_count} empty");

    let clusters = cluster::assign_clusters(&mut tree)?;

    log_stats(&tree, &split_faces, &clusters);
    validate(&tree, &split_faces, &clusters)?;

    Ok(PartitionResult {
        tree,
        faces: split_faces,
        clusters,
    })
}

fn log_stats(tree: &BspTree, faces: &[Face], clusters: &[Cluster]) {
    let max_depth = compute_max_depth(tree);
    let avg_faces: f32 = if clusters.is_empty() {
        0.0
    } else {
        let total: usize = clusters.iter().map(|c| c.face_indices.len()).sum();
        total as f32 / clusters.len() as f32
    };

    log::info!("[Compiler] BSP nodes: {}", tree.nodes.len());
    log::info!("[Compiler] BSP leaves: {}", tree.leaves.len());
    log::info!("[Compiler] BSP max depth: {max_depth}");
    log::info!("[Compiler] Clusters: {}", clusters.len());
    log::info!("[Compiler] Total faces (after splits): {}", faces.len());
    log::info!("[Compiler] Average faces per cluster: {avg_faces:.1}");
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

fn validate(tree: &BspTree, faces: &[Face], clusters: &[Cluster]) -> Result<()> {
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

    // Every leaf is assigned to exactly one cluster
    for (i, leaf) in tree.leaves.iter().enumerate() {
        anyhow::ensure!(
            leaf.cluster < clusters.len(),
            "leaf {i} assigned to non-existent cluster {}",
            leaf.cluster
        );
    }

    // Every face is transitively in exactly one cluster
    let mut face_cluster_count = vec![0usize; faces.len()];
    for cluster in clusters {
        for &fi in &cluster.face_indices {
            face_cluster_count[fi] += 1;
        }
    }
    for (i, count) in face_cluster_count.iter().enumerate() {
        anyhow::ensure!(
            *count == 1,
            "face {i} appears in {count} clusters (expected exactly 1)"
        );
    }

    // Cluster bounding volumes are finite
    for cluster in clusters {
        let b = &cluster.bounds;
        anyhow::ensure!(
            b.min.x.is_finite()
                && b.min.y.is_finite()
                && b.min.z.is_finite()
                && b.max.x.is_finite()
                && b.max.y.is_finite()
                && b.max.z.is_finite(),
            "cluster {} has non-finite bounding volume",
            cluster.id
        );
        anyhow::ensure!(
            b.min.x <= b.max.x && b.min.y <= b.max.y && b.min.z <= b.max.z,
            "cluster {} has inverted bounding volume",
            cluster.id
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
    use glam::Vec3;

    fn make_box_faces(min: Vec3, max: Vec3) -> Vec<Face> {
        let texture = "test".to_string();

        // 6 faces of an axis-aligned box
        vec![
            // -X face
            Face {
                vertices: vec![
                    Vec3::new(min.x, min.y, min.z),
                    Vec3::new(min.x, max.y, min.z),
                    Vec3::new(min.x, max.y, max.z),
                    Vec3::new(min.x, min.y, max.z),
                ],
                normal: Vec3::NEG_X,
                distance: -min.x,
                texture: texture.clone(),
            },
            // +X face
            Face {
                vertices: vec![
                    Vec3::new(max.x, min.y, min.z),
                    Vec3::new(max.x, min.y, max.z),
                    Vec3::new(max.x, max.y, max.z),
                    Vec3::new(max.x, max.y, min.z),
                ],
                normal: Vec3::X,
                distance: max.x,
                texture: texture.clone(),
            },
            // -Y face
            Face {
                vertices: vec![
                    Vec3::new(min.x, min.y, min.z),
                    Vec3::new(min.x, min.y, max.z),
                    Vec3::new(max.x, min.y, max.z),
                    Vec3::new(max.x, min.y, min.z),
                ],
                normal: Vec3::NEG_Y,
                distance: -min.y,
                texture: texture.clone(),
            },
            // +Y face
            Face {
                vertices: vec![
                    Vec3::new(min.x, max.y, min.z),
                    Vec3::new(max.x, max.y, min.z),
                    Vec3::new(max.x, max.y, max.z),
                    Vec3::new(min.x, max.y, max.z),
                ],
                normal: Vec3::Y,
                distance: max.y,
                texture: texture.clone(),
            },
            // -Z face
            Face {
                vertices: vec![
                    Vec3::new(min.x, min.y, min.z),
                    Vec3::new(max.x, min.y, min.z),
                    Vec3::new(max.x, max.y, min.z),
                    Vec3::new(min.x, max.y, min.z),
                ],
                normal: Vec3::NEG_Z,
                distance: -min.z,
                texture: texture.clone(),
            },
            // +Z face
            Face {
                vertices: vec![
                    Vec3::new(min.x, min.y, max.z),
                    Vec3::new(max.x, min.y, max.z),
                    Vec3::new(max.x, max.y, max.z),
                    Vec3::new(min.x, max.y, max.z),
                ],
                normal: Vec3::Z,
                distance: max.z,
                texture: texture.clone(),
            },
        ]
    }

    #[test]
    fn single_brush_produces_one_cluster() {
        let faces = make_box_faces(Vec3::ZERO, Vec3::new(64.0, 64.0, 64.0));
        let result = partition(faces, &[]).expect("partition should succeed");

        assert_eq!(
            result.clusters.len(),
            1,
            "single brush should produce one cluster"
        );
        assert!(!result.faces.is_empty());
        assert_eq!(
            result.clusters[0].face_indices.len(),
            result.faces.len(),
            "the single cluster should contain all faces"
        );
    }

    #[test]
    fn two_disjoint_brushes_produce_at_least_two_clusters() {
        let mut faces = make_box_faces(Vec3::ZERO, Vec3::new(64.0, 64.0, 64.0));
        faces.extend(make_box_faces(
            Vec3::new(1000.0, 1000.0, 1000.0),
            Vec3::new(1064.0, 1064.0, 1064.0),
        ));

        let result = partition(faces, &[]).expect("partition should succeed");

        assert!(
            result.clusters.len() >= 2,
            "two distant brushes should produce at least 2 clusters, got {}",
            result.clusters.len()
        );
    }

    #[test]
    fn empty_input_produces_empty_output() {
        let result = partition(Vec::new(), &[]).expect("empty partition should succeed");
        assert!(result.tree.nodes.is_empty());
        assert!(result.tree.leaves.is_empty());
        assert!(result.clusters.is_empty());
        assert!(result.faces.is_empty());
    }

    #[test]
    fn every_face_maps_to_exactly_one_cluster() {
        let mut faces = make_box_faces(Vec3::ZERO, Vec3::new(64.0, 64.0, 64.0));
        faces.extend(make_box_faces(
            Vec3::new(200.0, 0.0, 0.0),
            Vec3::new(264.0, 64.0, 64.0),
        ));
        faces.extend(make_box_faces(
            Vec3::new(0.0, 200.0, 0.0),
            Vec3::new(64.0, 264.0, 64.0),
        ));

        let result = partition(faces, &[]).expect("partition should succeed");

        let mut face_cluster = vec![None; result.faces.len()];
        for cluster in &result.clusters {
            for &fi in &cluster.face_indices {
                assert!(
                    face_cluster[fi].is_none(),
                    "face {fi} assigned to multiple clusters"
                );
                face_cluster[fi] = Some(cluster.id);
            }
        }
        for (i, c) in face_cluster.iter().enumerate() {
            assert!(c.is_some(), "face {i} not assigned to any cluster");
        }
    }

    #[test]
    fn cluster_bounds_are_valid() {
        let faces = make_box_faces(Vec3::ZERO, Vec3::new(64.0, 64.0, 64.0));
        let result = partition(faces, &[]).expect("partition should succeed");

        for cluster in &result.clusters {
            let b = &cluster.bounds;
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

        let map_data = crate::parse::parse_map_file(&map_path).expect("test.map should parse");

        let result = partition(map_data.world_faces, &map_data.brush_volumes)
            .expect("partition should succeed on test map");

        assert!(!result.clusters.is_empty(), "should produce clusters");
        assert!(!result.tree.leaves.is_empty(), "should produce leaves");
        assert!(!result.faces.is_empty(), "should have faces");

        // Reasonable cluster count for a 10-brush map
        assert!(
            result.clusters.len() <= 100,
            "too many clusters ({}) for a small map",
            result.clusters.len()
        );
    }
}
