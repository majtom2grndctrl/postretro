// Portal-based PVS: generate portals, flood-fill visibility, encode bitsets.
// See: context/lib/build_pipeline.md §PRL

pub mod portal_vis;

use crate::partition::{BspChild, BspTree};
use crate::portals::{self, Portal};
use postretro_level_format::bsp::{
    BspLeafRecord, BspLeavesSection, BspNodeRecord, BspNodesSection,
};
use postretro_level_format::leaf_pvs::LeafPvsSection;
use postretro_level_format::visibility::compress_pvs;

/// Computed visibility data from the compiler's visibility pass.
pub struct VisibilityResult {
    pub nodes_section: BspNodesSection,
    pub leaves_section: BspLeavesSection,
    pub leaf_pvs_section: LeafPvsSection,
    pub empty_leaf_count: usize,
    pub compressed_pvs_bytes: usize,
}

/// Build per-leaf PVS via portal flood-fill.
///
/// 1. Generate portals from the BSP tree.
/// 2. Compute per-leaf PVS by BFS through the portal graph.
/// 3. Encode as BspNodes, BspLeaves, and LeafPvs sections.
///
/// Returns the visibility result and the generated portals (for stats/logging).
pub fn build_portal_pvs(tree: &BspTree) -> (VisibilityResult, Vec<Portal>) {
    let generated_portals = portals::generate_portals(tree);

    let solid: Vec<bool> = tree.leaves.iter().map(|l| l.is_solid).collect();
    let leaf_count = tree.leaves.len();

    let pvs = portal_vis::compute_pvs(&generated_portals, leaf_count, &solid);

    let result = encode_bsp_and_pvs(tree, &pvs);
    (result, generated_portals)
}

/// Encode the BSP tree and per-leaf PVS into the new PRL section types.
///
/// PVS bitsets use the full leaf array index (not empty-leaf-only indexing),
/// since BspLeavesSection includes all leaves (solid and empty). Solid leaves
/// have `pvs_offset = 0` and `pvs_size = 0`.
fn encode_bsp_and_pvs(tree: &BspTree, pvs: &[Vec<bool>]) -> VisibilityResult {
    let nodes_section = encode_nodes(tree);
    let (leaves_section, leaf_pvs_section, empty_leaf_count, compressed_pvs_bytes) =
        encode_leaves_and_pvs(tree, pvs);

    VisibilityResult {
        nodes_section,
        leaves_section,
        leaf_pvs_section,
        empty_leaf_count,
        compressed_pvs_bytes,
    }
}

/// Encode BSP interior nodes into the flat node array.
fn encode_nodes(tree: &BspTree) -> BspNodesSection {
    let nodes = tree
        .nodes
        .iter()
        .map(|n| {
            let front = match &n.front {
                BspChild::Node(idx) => *idx as i32,
                BspChild::Leaf(idx) => -1 - (*idx as i32),
            };
            let back = match &n.back {
                BspChild::Node(idx) => *idx as i32,
                BspChild::Leaf(idx) => -1 - (*idx as i32),
            };
            BspNodeRecord {
                plane_normal: [n.plane_normal.x, n.plane_normal.y, n.plane_normal.z],
                plane_distance: n.plane_distance,
                front,
                back,
            }
        })
        .collect();

    BspNodesSection { nodes }
}

/// Encode BSP leaves and their PVS data.
///
/// Faces are ordered by leaf in the geometry section (all faces for leaf 0 first,
/// then leaf 1, etc.). `face_start` / `face_count` index into that ordering.
/// Only empty leaves contribute faces; solid leaves get face_start=0, face_count=0.
fn encode_leaves_and_pvs(
    tree: &BspTree,
    pvs: &[Vec<bool>],
) -> (BspLeavesSection, LeafPvsSection, usize, usize) {
    let leaf_count = tree.leaves.len();
    let pvs_byte_count = leaf_count.div_ceil(8);

    let mut pvs_blob: Vec<u8> = Vec::new();
    let mut leaf_records: Vec<BspLeafRecord> = Vec::with_capacity(leaf_count);
    let mut empty_leaf_count = 0usize;

    // Face cursor tracks the face_start for each empty leaf in the geometry
    // section's leaf-ordered face list.
    let mut face_cursor: u32 = 0;

    for (bsp_leaf_idx, leaf) in tree.leaves.iter().enumerate() {
        let b = &leaf.bounds;

        if leaf.is_solid {
            leaf_records.push(BspLeafRecord {
                face_start: 0,
                face_count: 0,
                bounds_min: [b.min.x, b.min.y, b.min.z],
                bounds_max: [b.max.x, b.max.y, b.max.z],
                pvs_offset: 0,
                pvs_size: 0,
                is_solid: 1,
            });
            continue;
        }

        empty_leaf_count += 1;

        // Build uncompressed PVS bitset using full leaf indices.
        let mut bitset = vec![0u8; pvs_byte_count];
        for (other_idx, &visible) in pvs[bsp_leaf_idx].iter().enumerate() {
            if visible {
                bitset[other_idx / 8] |= 1u8 << (other_idx % 8);
            }
        }

        let compressed = compress_pvs(&bitset);
        let pvs_offset = pvs_blob.len() as u32;
        let pvs_size = compressed.len() as u32;
        pvs_blob.extend_from_slice(&compressed);

        let face_count = leaf.face_indices.len() as u32;

        leaf_records.push(BspLeafRecord {
            face_start: face_cursor,
            face_count,
            bounds_min: [b.min.x, b.min.y, b.min.z],
            bounds_max: [b.max.x, b.max.y, b.max.z],
            pvs_offset,
            pvs_size,
            is_solid: 0,
        });

        face_cursor += face_count;
    }

    let compressed_pvs_bytes = pvs_blob.len();

    (
        BspLeavesSection {
            leaves: leaf_records,
        },
        LeafPvsSection { pvs_data: pvs_blob },
        empty_leaf_count,
        compressed_pvs_bytes,
    )
}

/// Log visibility statistics.
pub fn log_stats(result: &VisibilityResult, portal_count: usize) {
    if result.empty_leaf_count == 0 {
        log::info!("[Compiler] Visibility: 0 empty leaves, 0 portals");
        return;
    }

    let leaf_count = result.leaves_section.leaves.len();
    let pvs_byte_count = leaf_count.div_ceil(8);
    let mut visible_counts: Vec<usize> = Vec::with_capacity(result.empty_leaf_count);

    for leaf in &result.leaves_section.leaves {
        if leaf.is_solid != 0 {
            continue;
        }

        let compressed = &result.leaf_pvs_section.pvs_data
            [leaf.pvs_offset as usize..(leaf.pvs_offset + leaf.pvs_size) as usize];
        let decompressed =
            postretro_level_format::visibility::decompress_pvs(compressed, pvs_byte_count);

        let mut count = 0usize;
        for byte in decompressed.iter().take(pvs_byte_count) {
            count += byte.count_ones() as usize;
        }
        visible_counts.push(count);
    }

    let min_vis = visible_counts.iter().copied().min().unwrap_or(0);
    let max_vis = visible_counts.iter().copied().max().unwrap_or(0);
    let avg_vis = if visible_counts.is_empty() {
        0.0
    } else {
        visible_counts.iter().sum::<usize>() as f64 / visible_counts.len() as f64
    };

    log::info!(
        "[Compiler] Visibility: {} empty leaves, {} portals, {} bytes compressed PVS",
        result.empty_leaf_count,
        portal_count,
        result.compressed_pvs_bytes,
    );
    log::info!(
        "[Compiler] Visible leaves per leaf: min={min_vis}, max={max_vis}, avg={avg_vis:.1}",
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
        let (result, portals) = build_portal_pvs(&tree);
        assert_eq!(result.empty_leaf_count, 0);
        assert!(result.leaves_section.leaves.is_empty());
        assert!(result.leaf_pvs_section.pvs_data.is_empty());
        assert!(portals.is_empty());
    }

    #[test]
    fn all_solid_leaves_produce_empty_pvs_blob() {
        let tree = make_tree(vec![(vec![0], true), (vec![1], true)]);
        let (result, _) = build_portal_pvs(&tree);
        assert_eq!(result.empty_leaf_count, 0);
        // Solid leaves exist but have no PVS data
        assert_eq!(result.leaves_section.leaves.len(), 2);
        assert!(result.leaf_pvs_section.pvs_data.is_empty());
        for leaf in &result.leaves_section.leaves {
            assert_eq!(leaf.is_solid, 1);
            assert_eq!(leaf.pvs_offset, 0);
            assert_eq!(leaf.pvs_size, 0);
        }
    }

    #[test]
    fn single_empty_leaf_sees_itself() {
        let tree = make_tree(vec![(vec![0, 1], false)]);

        let (result, _) = build_portal_pvs(&tree);
        assert_eq!(result.empty_leaf_count, 1);
        assert_eq!(result.leaves_section.leaves.len(), 1);
        assert_eq!(result.leaves_section.leaves[0].face_start, 0);
        assert_eq!(result.leaves_section.leaves[0].face_count, 2);

        let pvs_bytes = 1usize.div_ceil(8);
        let leaf = &result.leaves_section.leaves[0];
        let decompressed = decompress_pvs(
            &result.leaf_pvs_section.pvs_data
                [leaf.pvs_offset as usize..(leaf.pvs_offset + leaf.pvs_size) as usize],
            pvs_bytes,
        );
        assert_ne!(decompressed[0] & 1, 0, "leaf 0 should see itself");
    }

    #[test]
    fn bsp_nodes_section_round_trips() {
        let tree = make_tree(vec![(vec![0], false), (vec![1], false), (vec![2], false)]);

        let (result, _) = build_portal_pvs(&tree);
        let bytes = result.nodes_section.to_bytes();
        let restored = BspNodesSection::from_bytes(&bytes).unwrap();
        assert_eq!(result.nodes_section, restored);
    }

    #[test]
    fn bsp_leaves_section_round_trips() {
        let tree = make_tree(vec![(vec![0], false), (vec![1], false), (vec![2], false)]);

        let (result, _) = build_portal_pvs(&tree);
        let bytes = result.leaves_section.to_bytes();
        let restored = BspLeavesSection::from_bytes(&bytes).unwrap();
        assert_eq!(result.leaves_section, restored);
    }

    #[test]
    fn leaf_pvs_section_round_trips() {
        let tree = make_tree(vec![(vec![0], false), (vec![1], false), (vec![2], false)]);

        let (result, _) = build_portal_pvs(&tree);
        let bytes = result.leaf_pvs_section.to_bytes();
        let restored = LeafPvsSection::from_bytes(&bytes).unwrap();
        assert_eq!(result.leaf_pvs_section, restored);
    }
}
