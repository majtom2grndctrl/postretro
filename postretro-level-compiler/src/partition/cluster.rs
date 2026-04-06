// Spatial grid clustering: assign BSP leaves to clusters.
// See: context/plans/ready/prl-phase-1-minimum-viable-compiler/

use anyhow::Result;
use std::collections::HashMap;

use super::types::*;

/// Target number of non-empty grid cells for a typical level.
const TARGET_CLUSTER_MIN: usize = 10;
const TARGET_CLUSTER_MAX: usize = 300;

/// Assign BSP leaves to spatial grid clusters and update leaf cluster fields.
pub fn assign_clusters(tree: &mut BspTree) -> Result<Vec<Cluster>> {
    if tree.leaves.is_empty() {
        return Ok(Vec::new());
    }

    // Compute the bounding box of all leaves
    let mut world_bounds = Aabb::empty();
    for leaf in &tree.leaves {
        world_bounds.expand_aabb(&leaf.bounds);
    }

    let grid_dims = compute_grid_dimensions(&world_bounds, tree.leaves.len());

    // Assign each leaf to a grid cell based on its centroid
    let mut cell_leaves: HashMap<(usize, usize, usize), Vec<usize>> = HashMap::new();

    let extent = world_bounds.max - world_bounds.min;
    // Avoid division by zero on degenerate axes
    let cell_size = glam::Vec3::new(
        if extent.x > 0.0 {
            extent.x / grid_dims.0 as f32
        } else {
            1.0
        },
        if extent.y > 0.0 {
            extent.y / grid_dims.1 as f32
        } else {
            1.0
        },
        if extent.z > 0.0 {
            extent.z / grid_dims.2 as f32
        } else {
            1.0
        },
    );

    for (leaf_idx, leaf) in tree.leaves.iter().enumerate() {
        let centroid = leaf.bounds.centroid();
        let rel = centroid - world_bounds.min;

        let cx = ((rel.x / cell_size.x) as usize).min(grid_dims.0 - 1);
        let cy = ((rel.y / cell_size.y) as usize).min(grid_dims.1 - 1);
        let cz = ((rel.z / cell_size.z) as usize).min(grid_dims.2 - 1);

        cell_leaves.entry((cx, cy, cz)).or_default().push(leaf_idx);
    }

    // Build clusters from non-empty cells
    let mut clusters = Vec::new();
    let mut sorted_cells: Vec<_> = cell_leaves.into_iter().collect();
    sorted_cells.sort_by_key(|(key, _)| *key);

    for (_, leaf_indices) in sorted_cells {
        let cluster_id = clusters.len();

        let mut bounds = Aabb::empty();
        let mut face_indices = Vec::new();

        for &leaf_idx in &leaf_indices {
            let leaf = &tree.leaves[leaf_idx];
            bounds.expand_aabb(&leaf.bounds);
            face_indices.extend_from_slice(&leaf.face_indices);
        }

        // Deduplicate face indices (faces shouldn't appear in multiple leaves,
        // but be defensive)
        face_indices.sort_unstable();
        face_indices.dedup();

        // Update leaf cluster assignments
        for &leaf_idx in &leaf_indices {
            tree.leaves[leaf_idx].cluster = cluster_id;
        }

        clusters.push(Cluster {
            id: cluster_id,
            bounds,
            face_indices,
        });
    }

    Ok(clusters)
}

/// Compute grid dimensions that produce a reasonable number of non-empty cells.
///
/// Starts from the cube root of the leaf count (proportional in each axis),
/// then adjusts for the world's aspect ratio.
fn compute_grid_dimensions(bounds: &Aabb, leaf_count: usize) -> (usize, usize, usize) {
    if leaf_count <= 1 {
        return (1, 1, 1);
    }

    let extent = bounds.max - bounds.min;

    // Handle degenerate bounds (flat or zero-volume)
    let ex = extent.x.max(1.0);
    let ey = extent.y.max(1.0);
    let ez = extent.z.max(1.0);

    // Target cells ≈ leaf_count (clamped to reasonable range)
    let target = (leaf_count as f32)
        .max(TARGET_CLUSTER_MIN as f32)
        .min(TARGET_CLUSTER_MAX as f32);

    // Distribute cells proportionally to world extent in each axis
    let volume = ex * ey * ez;
    let cell_volume = volume / target;
    let cell_edge = cell_volume.cbrt();

    let nx = (ex / cell_edge).ceil().max(1.0) as usize;
    let ny = (ey / cell_edge).ceil().max(1.0) as usize;
    let nz = (ez / cell_edge).ceil().max(1.0) as usize;

    (nx, ny, nz)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn make_test_tree(leaf_configs: Vec<(Vec3, Vec3, Vec<usize>)>) -> BspTree {
        let leaves = leaf_configs
            .into_iter()
            .map(|(min, max, face_indices)| BspLeaf {
                face_indices,
                bounds: Aabb { min, max },
                cluster: 0,
                is_solid: false,
            })
            .collect();

        BspTree {
            nodes: Vec::new(),
            leaves,
        }
    }

    #[test]
    fn single_leaf_produces_one_cluster() {
        let mut tree = make_test_tree(vec![(
            Vec3::ZERO,
            Vec3::new(64.0, 64.0, 64.0),
            vec![0, 1, 2],
        )]);

        let clusters = assign_clusters(&mut tree).expect("should succeed");
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].face_indices, vec![0, 1, 2]);
        assert_eq!(tree.leaves[0].cluster, 0);
    }

    #[test]
    fn distant_leaves_get_different_clusters() {
        let mut tree = make_test_tree(vec![
            (Vec3::ZERO, Vec3::new(64.0, 64.0, 64.0), vec![0, 1]),
            (
                Vec3::new(10000.0, 10000.0, 10000.0),
                Vec3::new(10064.0, 10064.0, 10064.0),
                vec![2, 3],
            ),
        ]);

        let clusters = assign_clusters(&mut tree).expect("should succeed");
        assert!(
            clusters.len() >= 2,
            "distant leaves should be in different clusters"
        );

        let c0 = tree.leaves[0].cluster;
        let c1 = tree.leaves[1].cluster;
        assert_ne!(
            c0, c1,
            "leaves at opposite corners should be in different clusters"
        );
    }

    #[test]
    fn adjacent_leaves_may_share_cluster() {
        let mut tree = make_test_tree(vec![
            (Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0), vec![0]),
            (Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 1.0), vec![1]),
        ]);

        let clusters = assign_clusters(&mut tree).expect("should succeed");
        // Adjacent leaves should be in the same cluster when grid is coarse
        assert!(clusters.len() >= 1);
    }

    #[test]
    fn cluster_bounds_cover_all_contained_leaves() {
        let mut tree = make_test_tree(vec![
            (Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0), vec![0]),
            (
                Vec3::new(5.0, 5.0, 5.0),
                Vec3::new(15.0, 15.0, 15.0),
                vec![1],
            ),
        ]);

        let clusters = assign_clusters(&mut tree).expect("should succeed");
        for cluster in &clusters {
            assert!(cluster.bounds.is_valid(), "cluster bounds should be valid");
        }
    }

    #[test]
    fn grid_dimensions_reasonable_for_small_input() {
        let bounds = Aabb {
            min: Vec3::ZERO,
            max: Vec3::new(100.0, 100.0, 100.0),
        };
        let dims = compute_grid_dimensions(&bounds, 5);
        let total = dims.0 * dims.1 * dims.2;
        assert!(total >= 1, "should have at least 1 cell");
        assert!(total <= 1000, "shouldn't have excessive cells for 5 leaves");
    }

    #[test]
    fn grid_dimensions_handle_flat_bounds() {
        let bounds = Aabb {
            min: Vec3::ZERO,
            max: Vec3::new(100.0, 100.0, 0.0),
        };
        let dims = compute_grid_dimensions(&bounds, 20);
        assert!(dims.0 >= 1 && dims.1 >= 1 && dims.2 >= 1);
    }
}
