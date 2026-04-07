// Pass-through leaf visibility: all empty leaves mutually visible.
// See: context/lib/build_pipeline.md §PRL
//
// This is a placeholder until portal-based PVS is implemented (Tasks 03-04).
// Every empty leaf can see every other empty leaf (degenerate PVS -- all bits set).

use crate::partition::BspTree;
use postretro_level_format::visibility::{ClusterInfo, ClusterVisibilitySection, compress_pvs};

/// Computed visibility data from the compiler's visibility pass.
pub struct VisibilityResult {
    pub section: ClusterVisibilitySection,
    pub empty_leaf_count: usize,
    pub compressed_pvs_bytes: usize,
}

/// Build a degenerate PVS where all empty leaves are mutually visible.
///
/// This is a placeholder that produces valid .prl files while the portal vis
/// pipeline is under construction (Tasks 03-04). Every empty leaf has a full
/// PVS bitset (all bits set), meaning no visibility culling occurs at runtime.
pub fn build_passthrough_pvs(tree: &BspTree) -> VisibilityResult {
    // Collect only empty leaves, in BSP leaf order.
    let empty_leaves: Vec<(usize, &crate::partition::BspLeaf)> = tree
        .leaves
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.is_solid)
        .collect();

    let empty_leaf_count = empty_leaves.len();

    if empty_leaf_count == 0 {
        return VisibilityResult {
            section: ClusterVisibilitySection {
                clusters: Vec::new(),
                pvs_data: Vec::new(),
            },
            empty_leaf_count: 0,
            compressed_pvs_bytes: 0,
        };
    }

    // Build the all-visible PVS bitset (shared by every leaf).
    let pvs_byte_count = empty_leaf_count.div_ceil(8);
    let mut full_pvs = vec![0xFFu8; pvs_byte_count];
    // Mask off trailing bits in the last byte so only valid leaf indices are set.
    let trailing_bits = empty_leaf_count % 8;
    if trailing_bits > 0 {
        full_pvs[pvs_byte_count - 1] = (1u8 << trailing_bits) - 1;
    }

    let compressed_full = compress_pvs(&full_pvs);

    // All leaves share the same all-visible bitset, so store it once.
    let pvs_blob = compressed_full.clone();
    let pvs_size = compressed_full.len() as u32;
    let mut cluster_infos = Vec::with_capacity(empty_leaf_count);

    // Track face counts to compute face_start offsets. Faces are ordered by
    // empty-leaf index in the geometry section (see geometry.rs).
    let mut face_cursor: u32 = 0;

    for (_, leaf) in &empty_leaves {
        let face_count = leaf.face_indices.len() as u32;

        let b = &leaf.bounds;
        cluster_infos.push(ClusterInfo {
            bounds_min: [b.min.x, b.min.y, b.min.z],
            bounds_max: [b.max.x, b.max.y, b.max.z],
            face_start: face_cursor,
            face_count,
            pvs_offset: 0,
            pvs_size,
        });

        face_cursor += face_count;
    }

    let compressed_pvs_bytes = pvs_blob.len();

    VisibilityResult {
        section: ClusterVisibilitySection {
            clusters: cluster_infos,
            pvs_data: pvs_blob,
        },
        empty_leaf_count,
        compressed_pvs_bytes,
    }
}

/// Log visibility statistics.
pub fn log_stats(result: &VisibilityResult) {
    log::info!(
        "[Compiler] Visibility: {} empty leaves, {} bytes compressed PVS (pass-through: all visible)",
        result.empty_leaf_count,
        result.compressed_pvs_bytes,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::{Aabb, BspLeaf, BspTree};
    use glam::Vec3;
    use postretro_level_format::visibility::decompress_pvs;

    fn make_tree(leaves: Vec<(Vec<usize>, bool)>) -> BspTree {
        let bsp_leaves: Vec<BspLeaf> = leaves
            .into_iter()
            .map(|(face_indices, is_solid)| BspLeaf {
                face_indices,
                bounds: Aabb {
                    min: Vec3::ZERO,
                    max: Vec3::splat(64.0),
                },
                is_solid,
            })
            .collect();

        BspTree {
            nodes: Vec::new(),
            leaves: bsp_leaves,
        }
    }

    #[test]
    fn empty_tree_produces_empty_vis() {
        let tree = BspTree {
            nodes: Vec::new(),
            leaves: Vec::new(),
        };
        let result = build_passthrough_pvs(&tree);
        assert_eq!(result.empty_leaf_count, 0);
        assert!(result.section.clusters.is_empty());
        assert!(result.section.pvs_data.is_empty());
    }

    #[test]
    fn all_solid_leaves_produce_empty_vis() {
        let tree = make_tree(vec![(vec![0], true), (vec![1], true)]);
        let result = build_passthrough_pvs(&tree);
        assert_eq!(result.empty_leaf_count, 0);
        assert!(result.section.clusters.is_empty());
    }

    #[test]
    fn single_empty_leaf_sees_itself() {
        let tree = make_tree(vec![(vec![0, 1], false)]);

        let result = build_passthrough_pvs(&tree);
        assert_eq!(result.empty_leaf_count, 1);
        assert_eq!(result.section.clusters.len(), 1);
        assert_eq!(result.section.clusters[0].face_start, 0);
        assert_eq!(result.section.clusters[0].face_count, 2);

        // PVS should show leaf 0 can see leaf 0
        let pvs_bytes = (1 + 7) / 8;
        let decompressed = decompress_pvs(
            &result.section.pvs_data[result.section.clusters[0].pvs_offset as usize
                ..(result.section.clusters[0].pvs_offset + result.section.clusters[0].pvs_size)
                    as usize],
            pvs_bytes,
        );
        assert_ne!(decompressed[0] & 1, 0, "leaf 0 should see itself");
    }

    #[test]
    fn two_empty_leaves_see_each_other() {
        let tree = make_tree(vec![
            (vec![0], false),
            (vec![1], true), // solid -- skipped
            (vec![2], false),
        ]);

        let result = build_passthrough_pvs(&tree);
        assert_eq!(result.empty_leaf_count, 2);
        assert_eq!(result.section.clusters.len(), 2);

        // Check face ranges
        assert_eq!(result.section.clusters[0].face_start, 0);
        assert_eq!(result.section.clusters[0].face_count, 1);
        assert_eq!(result.section.clusters[1].face_start, 1);
        assert_eq!(result.section.clusters[1].face_count, 1);

        // Both leaves should see both leaves
        let pvs_bytes = (2 + 7) / 8;
        for i in 0..2 {
            let ci = &result.section.clusters[i];
            let decompressed = decompress_pvs(
                &result.section.pvs_data
                    [ci.pvs_offset as usize..(ci.pvs_offset + ci.pvs_size) as usize],
                pvs_bytes,
            );
            assert_ne!(decompressed[0] & 0b01, 0, "leaf {i} should see leaf 0");
            assert_ne!(decompressed[0] & 0b10, 0, "leaf {i} should see leaf 1");
        }
    }

    #[test]
    fn pvs_section_round_trips() {
        let tree = make_tree(vec![(vec![0], false), (vec![1], false), (vec![2], false)]);

        let result = build_passthrough_pvs(&tree);
        let bytes = result.section.to_bytes();
        let restored = ClusterVisibilitySection::from_bytes(&bytes).unwrap();
        assert_eq!(result.section, restored);
    }
}
