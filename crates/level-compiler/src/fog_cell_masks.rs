// Fog cell masks bake: produces the FogCellMasks PRL section (ID 31).
//
// For each BSP leaf, sets bit `i` of its mask when fog volume `i`'s
// world-space AABB overlaps the leaf's bounds. Solid leaves are written as
// `0`. The runtime unions visible-cell masks each frame to derive the active
// fog-volume set.
//
// See: context/lib/build_pipeline.md §PRL section IDs

use glam::DVec3;
use postretro_level_format::fog_cell_masks::FogCellMasksSection;
use postretro_level_format::fog_volumes::MAX_FOG_VOLUMES;

use crate::map_data::MapFogVolume;
use crate::partition::{Aabb, BspTree};

/// Bake the per-leaf fog-volume bitmask section.
///
/// Returns `None` when `fog_volumes` is empty so the caller can omit the
/// section entirely.
pub fn bake_fog_cell_masks(
    tree: &BspTree,
    fog_volumes: &[MapFogVolume],
) -> Option<FogCellMasksSection> {
    if fog_volumes.is_empty() {
        return None;
    }

    debug_assert!(
        fog_volumes.len() <= MAX_FOG_VOLUMES,
        "fog volume count {} exceeds MAX_FOG_VOLUMES ({MAX_FOG_VOLUMES}); parse stage should cap",
        fog_volumes.len(),
    );

    // Pre-build the fog AABBs in DVec3 form once; bounds tests run in f64 to
    // match the BSP's native precision (BspLeaf.bounds is DVec3).
    let fog_aabbs: Vec<Aabb> = fog_volumes.iter().map(fog_volume_aabb).collect();

    let mut masks = Vec::with_capacity(tree.leaves.len());
    for leaf in &tree.leaves {
        if leaf.is_solid {
            masks.push(0);
            continue;
        }
        let mut mask: u32 = 0;
        for (i, fog) in fog_aabbs.iter().enumerate() {
            // `MAX_FOG_VOLUMES` (16) keeps us inside u32 bit range; bits
            // 16..31 are reserved/zero by construction.
            if leaf.bounds.intersects(fog) {
                mask |= 1u32 << i;
            }
        }
        masks.push(mask);
    }

    Some(FogCellMasksSection { masks })
}

fn fog_volume_aabb(v: &MapFogVolume) -> Aabb {
    Aabb {
        min: DVec3::new(v.min[0] as f64, v.min[1] as f64, v.min[2] as f64),
        max: DVec3::new(v.max[0] as f64, v.max[1] as f64, v.max[2] as f64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::{BspLeaf, BspNode, BspTree};

    fn aabb(min: [f64; 3], max: [f64; 3]) -> Aabb {
        Aabb {
            min: DVec3::new(min[0], min[1], min[2]),
            max: DVec3::new(max[0], max[1], max[2]),
        }
    }

    fn empty_leaf(bounds: Aabb) -> BspLeaf {
        BspLeaf {
            face_indices: Vec::new(),
            bounds,
            is_solid: false,
        }
    }

    fn solid_leaf(bounds: Aabb) -> BspLeaf {
        BspLeaf {
            face_indices: Vec::new(),
            bounds,
            is_solid: true,
        }
    }

    fn make_tree(leaves: Vec<BspLeaf>) -> BspTree {
        BspTree {
            nodes: Vec::<BspNode>::new(),
            leaves,
        }
    }

    fn fog(min: [f32; 3], max: [f32; 3]) -> MapFogVolume {
        MapFogVolume {
            min,
            max,
            color: [1.0, 1.0, 1.0],
            density: 0.5,
            falloff: 1.0,
            scatter: 0.6,
            height_gradient: 0.0,
            radial_falloff: 0.0,
            tags: Vec::new(),
        }
    }

    #[test]
    fn returns_none_when_no_fog_volumes() {
        let tree = make_tree(vec![empty_leaf(aabb([0.0; 3], [10.0; 3]))]);
        assert!(bake_fog_cell_masks(&tree, &[]).is_none());
    }

    #[test]
    fn overlapping_fog_sets_bit() {
        let tree = make_tree(vec![empty_leaf(aabb([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]))]);
        let volumes = vec![fog([5.0, 5.0, 5.0], [15.0, 15.0, 15.0])];
        let section = bake_fog_cell_masks(&tree, &volumes).expect("section should exist");
        assert_eq!(section.masks, vec![0b1]);
    }

    #[test]
    fn non_overlapping_fog_clears_bit() {
        let tree = make_tree(vec![empty_leaf(aabb([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]))]);
        let volumes = vec![fog([20.0, 20.0, 20.0], [30.0, 30.0, 30.0])];
        let section = bake_fog_cell_masks(&tree, &volumes).expect("section should exist");
        assert_eq!(section.masks, vec![0]);
    }

    #[test]
    fn solid_leaf_is_always_zero_even_when_overlapping() {
        // Solid leaf whose bounds would overlap the fog volume — must still mask 0.
        let tree = make_tree(vec![solid_leaf(aabb([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]))]);
        let volumes = vec![fog([0.0, 0.0, 0.0], [10.0, 10.0, 10.0])];
        let section = bake_fog_cell_masks(&tree, &volumes).expect("section should exist");
        assert_eq!(section.masks, vec![0]);
    }

    #[test]
    fn multiple_volumes_set_correct_bits() {
        // Three leaves: A overlaps volumes 0+2; B overlaps volume 1; C overlaps none.
        let tree = make_tree(vec![
            empty_leaf(aabb([0.0, 0.0, 0.0], [5.0, 5.0, 5.0])),
            empty_leaf(aabb([100.0, 0.0, 0.0], [105.0, 5.0, 5.0])),
            empty_leaf(aabb([1000.0, 0.0, 0.0], [1005.0, 5.0, 5.0])),
        ]);
        let volumes = vec![
            fog([0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),     // 0: in A
            fog([100.0, 0.0, 0.0], [105.0, 5.0, 5.0]), // 1: in B
            fog([-1.0, -1.0, -1.0], [6.0, 6.0, 6.0]),  // 2: in A
        ];
        let section = bake_fog_cell_masks(&tree, &volumes).expect("section should exist");
        assert_eq!(section.masks, vec![0b101, 0b010, 0]);
    }

    #[test]
    fn solid_leaves_emitted_as_zero_among_empty_leaves() {
        // Mix of solid and empty leaves: solid stays 0, empty gets bit 0.
        let tree = make_tree(vec![
            empty_leaf(aabb([0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            solid_leaf(aabb([0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            empty_leaf(aabb([100.0, 0.0, 0.0], [110.0, 10.0, 10.0])),
        ]);
        let volumes = vec![fog([0.0, 0.0, 0.0], [10.0, 10.0, 10.0])];
        let section = bake_fog_cell_masks(&tree, &volumes).expect("section should exist");
        assert_eq!(section.masks, vec![0b1, 0, 0]);
    }
}
