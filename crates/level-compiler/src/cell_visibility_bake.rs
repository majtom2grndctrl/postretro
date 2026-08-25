// Static CellVisibility reachability bake over the compiler portal graph.
// See: context/plans/in-progress/cell-visibility-relation/index.md

use std::collections::VecDeque;

use postretro_level_format::cell_visibility::CellVisibilitySection;

use crate::partition::BspTree;
use crate::portals::Portal;

/// Bake the v1 conservative cell-coupling gate.
///
/// Component IDs cover every BSP leaf, including solid leaves. Portal absence
/// makes a solid (or otherwise isolated) cell a singleton component. Outer
/// traversal is ascending cell ID, so the first member of each component is
/// its representative and dense IDs are ordered by that representative.
pub fn cell_visibility_bake(
    tree: &BspTree,
    portals: &[Portal],
) -> anyhow::Result<CellVisibilitySection> {
    let cell_count = tree.leaves.len();
    anyhow::ensure!(
        cell_count != 0,
        "CellVisibility requires at least one BSP leaf"
    );
    let cell_count_u32 = u32::try_from(cell_count)
        .map_err(|_| anyhow::anyhow!("CellVisibility cannot encode more than u32::MAX cells"))?;

    let mut adjacency = vec![Vec::new(); cell_count];
    for portal in portals {
        let front = portal.front_leaf;
        let back = portal.back_leaf;
        if front >= cell_count
            || back >= cell_count
            || tree.leaves[front].is_solid
            || tree.leaves[back].is_solid
        {
            continue;
        }
        adjacency[front].push(back);
        adjacency[back].push(front);
    }
    for neighbors in &mut adjacency {
        neighbors.sort_unstable();
        neighbors.dedup();
    }

    let mut component_ids = vec![u32::MAX; cell_count];
    let mut component_id = 0u32;
    for start in 0..cell_count {
        if component_ids[start] != u32::MAX {
            continue;
        }

        component_ids[start] = component_id;
        if !tree.leaves[start].is_solid {
            let mut queue = VecDeque::from([start]);
            while let Some(cell) = queue.pop_front() {
                for &neighbor in &adjacency[cell] {
                    if component_ids[neighbor] == u32::MAX {
                        component_ids[neighbor] = component_id;
                        queue.push_back(neighbor);
                    }
                }
            }
        }
        component_id = component_id
            .checked_add(1)
            .expect("CellVisibility component count exceeds u32::MAX");
    }

    Ok(CellVisibilitySection {
        cell_count: cell_count_u32,
        component_ids,
        coupled_pairs: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use glam::DVec3;

    use super::*;
    use crate::partition::{Aabb, BspLeaf, BspTree};

    fn tree(solid: &[bool]) -> BspTree {
        BspTree {
            nodes: Vec::new(),
            leaves: solid
                .iter()
                .enumerate()
                .map(|(index, &is_solid)| BspLeaf {
                    face_indices: Vec::new(),
                    is_solid,
                    bounds: Aabb {
                        min: DVec3::splat(index as f64),
                        max: DVec3::splat(index as f64 + 1.0),
                    },
                    defining_planes: Vec::new(),
                })
                .collect(),
        }
    }

    fn portal(front_leaf: usize, back_leaf: usize) -> Portal {
        Portal {
            polygon: Vec::new(),
            front_leaf,
            back_leaf,
        }
    }

    fn independently_reachable(tree: &BspTree, portals: &[Portal], start: usize) -> Vec<bool> {
        let mut reachable = vec![false; tree.leaves.len()];
        reachable[start] = true;
        if tree.leaves[start].is_solid {
            return reachable;
        }

        let mut queue = VecDeque::from([start]);
        while let Some(cell) = queue.pop_front() {
            for portal in portals {
                let neighbor = if portal.front_leaf == cell {
                    Some(portal.back_leaf)
                } else if portal.back_leaf == cell {
                    Some(portal.front_leaf)
                } else {
                    None
                };
                if let Some(neighbor) = neighbor.filter(|&neighbor| !tree.leaves[neighbor].is_solid)
                {
                    if !reachable[neighbor] {
                        reachable[neighbor] = true;
                        queue.push_back(neighbor);
                    }
                }
            }
        }
        reachable
    }

    #[test]
    fn components_match_independent_portal_bfs_including_solid_singletons() {
        let tree = tree(&[false, false, true, false, false]);
        let portals = vec![portal(0, 1), portal(1, 2), portal(3, 4)];
        let section = cell_visibility_bake(&tree, &portals).unwrap();

        assert_eq!(section.component_ids, vec![0, 0, 1, 2, 2]);
        for start in 0..tree.leaves.len() {
            let expected = independently_reachable(&tree, &portals, start);
            for target in 0..tree.leaves.len() {
                assert_eq!(
                    section.component_ids[start] == section.component_ids[target],
                    expected[target],
                    "reachability mismatch for {start} -> {target}"
                );
            }
        }
    }

    #[test]
    fn component_only_bytes_are_deterministic() {
        let tree = tree(&[false, false, true]);
        let portals = vec![portal(0, 1)];
        assert_eq!(
            cell_visibility_bake(&tree, &portals).unwrap().to_bytes(),
            cell_visibility_bake(&tree, &portals).unwrap().to_bytes()
        );
    }

    #[test]
    fn fixture_component_only_section_is_byte_identical_across_two_compiles() {
        let first = crate::fixture_pipeline::load_fixture("test_animated_weight_maps_single");
        let second = crate::fixture_pipeline::load_fixture("test_animated_weight_maps_single");
        let first_portals = crate::portals::generate_portals(&first.tree);
        let second_portals = crate::portals::generate_portals(&second.tree);

        assert_eq!(
            cell_visibility_bake(&first.tree, &first_portals)
                .unwrap()
                .to_bytes(),
            cell_visibility_bake(&second.tree, &second_portals)
                .unwrap()
                .to_bytes()
        );
    }
}
