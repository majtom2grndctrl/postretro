// Static CellVisibility bake over the compiler portal graph.
// See: context/plans/in-progress/cell-visibility-relation/index.md

use std::collections::VecDeque;

use postretro_level_format::cell_visibility::CellVisibilitySection;

use crate::bake_control::BakeControl;
use crate::partition::BspTree;
use crate::portals::Portal;

/// Bake the conservative v1 portal-reachability gate.
///
/// The component array is complete on this thin slice. The graded side table
/// remains empty until the controlled per-source metric pass fills it; the
/// runtime query already performs a real lookup rather than relying on that
/// placeholder state.
pub fn cell_visibility_bake(
    tree: &BspTree,
    portals: &[Portal],
    control: &BakeControl,
) -> anyhow::Result<CellVisibilitySection> {
    let cell_count = tree.leaves.len();
    anyhow::ensure!(
        cell_count != 0,
        "CellVisibility requires at least one BSP leaf"
    );
    let cell_count_u32 = u32::try_from(cell_count)
        .map_err(|_| anyhow::anyhow!("CellVisibility cannot encode more than u32::MAX cells"))?;

    control.publish_total(cell_count);
    let portal_edges = portal_edges(tree, portals);
    let component_ids = component_ids(cell_count, &portal_edges, control);

    Ok(CellVisibilitySection {
        cell_count: cell_count_u32,
        component_ids,
        coupled_pairs: Vec::new(),
    })
}

#[derive(Clone, Copy)]
struct PortalEdge {
    front: usize,
    back: usize,
}

/// Collect only portals that preserve the cell graph contract. Generated
/// portals already exclude solid leaves, but keeping the guard makes the bake
/// conservative for hand-built inputs used by focused tests.
fn portal_edges(tree: &BspTree, portals: &[Portal]) -> Vec<PortalEdge> {
    portals
        .iter()
        .filter_map(|portal| {
            let front = tree.leaves.get(portal.front_leaf)?;
            let back = tree.leaves.get(portal.back_leaf)?;
            if front.is_solid || back.is_solid {
                return None;
            }
            Some(PortalEdge {
                front: portal.front_leaf,
                back: portal.back_leaf,
            })
        })
        .collect()
}

/// Component IDs cover every BSP leaf, including solid leaves. Portal absence
/// makes a solid (or otherwise isolated) cell a singleton component. Outer
/// traversal is ascending CellId, so each component's representative is its
/// lowest member and dense IDs are ordered by that representative.
fn component_ids(cell_count: usize, portals: &[PortalEdge], control: &BakeControl) -> Vec<u32> {
    let mut adjacency = vec![Vec::new(); cell_count];
    for portal in portals {
        adjacency[portal.front].push(portal.back);
        adjacency[portal.back].push(portal.front);
    }

    let mut component_ids = vec![u32::MAX; cell_count];
    let mut component_id = 0u32;
    for start in 0..cell_count {
        if component_ids[start] != u32::MAX {
            continue;
        }

        control.governor().checkpoint();
        component_ids[start] = component_id;
        control.advance(1);
        let mut queue = VecDeque::from([start]);
        while let Some(cell) = queue.pop_front() {
            // Each adjacency vector is populated in portal input order, so the
            // traversal has no hash-table or completion-order dependency.
            for &neighbor in &adjacency[cell] {
                if component_ids[neighbor] == u32::MAX {
                    component_ids[neighbor] = component_id;
                    control.advance(1);
                    queue.push_back(neighbor);
                }
            }
        }
        component_id = component_id
            .checked_add(1)
            .expect("CellVisibility component count exceeds u32::MAX");
    }

    component_ids
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;

    use glam::DVec3;

    use super::*;
    use crate::governor::Governor;
    use crate::partition::{Aabb, BspLeaf, BspTree};
    use crate::reporter::StageProgress;

    fn tree(centres: &[DVec3], solid: &[bool]) -> BspTree {
        assert_eq!(centres.len(), solid.len());
        BspTree {
            nodes: Vec::new(),
            leaves: centres
                .iter()
                .zip(solid)
                .map(|(&centre, &is_solid)| BspLeaf {
                    face_indices: Vec::new(),
                    is_solid,
                    bounds: Aabb {
                        min: centre - DVec3::splat(0.5),
                        max: centre + DVec3::splat(0.5),
                    },
                    defining_planes: Vec::new(),
                })
                .collect(),
        }
    }

    fn square_portal(front_leaf: usize, back_leaf: usize, centre: DVec3, width: f64) -> Portal {
        let half_width = width * 0.5;
        Portal {
            polygon: vec![
                centre + DVec3::new(-half_width, -half_width, 0.0),
                centre + DVec3::new(half_width, -half_width, 0.0),
                centre + DVec3::new(half_width, half_width, 0.0),
                centre + DVec3::new(-half_width, half_width, 0.0),
            ],
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

    fn test_control(progress: &StageProgress) -> BakeControl {
        BakeControl::new(Arc::new(Governor::new(1, false)), progress)
    }

    #[test]
    fn components_match_independent_portal_bfs_including_solid_singletons() {
        let tree = tree(
            &[DVec3::ZERO, DVec3::X, DVec3::Y, DVec3::Z, DVec3::ONE],
            &[false, false, true, false, false],
        );
        let portals = vec![
            square_portal(0, 1, DVec3::splat(0.5), 1.0),
            square_portal(1, 2, DVec3::new(1.0, 0.5, 0.0), 1.0),
            square_portal(3, 4, DVec3::new(0.5, 0.5, 1.0), 1.0),
        ];
        let progress = StageProgress::indeterminate();
        let section = cell_visibility_bake(&tree, &portals, &test_control(&progress)).unwrap();

        assert_eq!(section.component_ids, vec![0, 0, 1, 2, 2]);
        assert!(section.coupled_pairs.is_empty());
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
        assert_eq!(progress.total(), Some(tree.leaves.len()));
        assert_eq!(progress.completed(), tree.leaves.len());
    }

    #[test]
    fn component_only_section_is_byte_identical_across_two_bakes() {
        let tree = tree(&[DVec3::ZERO, DVec3::X, DVec3::Y], &[false, false, false]);
        let portals = vec![square_portal(0, 1, DVec3::splat(0.5), 1.0)];

        let first_progress = StageProgress::indeterminate();
        let first = cell_visibility_bake(&tree, &portals, &test_control(&first_progress))
            .unwrap()
            .to_bytes()
            .unwrap();
        let second_progress = StageProgress::indeterminate();
        let second = cell_visibility_bake(&tree, &portals, &test_control(&second_progress))
            .unwrap()
            .to_bytes()
            .unwrap();

        assert_eq!(first, second);
    }
}
