// Portal flood-fill PVS: per-leaf visibility via BFS through portal graph.
// See: context/plans/in-progress/portal-bsp-vis/task-04-portal-vis.md

use std::collections::VecDeque;

use rayon::prelude::*;

use crate::portals::Portal;

/// Build an adjacency list: for each leaf, which portals touch it?
///
/// A portal touches a leaf if `front_leaf == leaf_idx` or `back_leaf == leaf_idx`.
fn leaf_portals(portals: &[Portal], leaf_count: usize) -> Vec<Vec<usize>> {
    let mut result = vec![Vec::new(); leaf_count];
    for (portal_idx, portal) in portals.iter().enumerate() {
        result[portal.front_leaf].push(portal_idx);
        result[portal.back_leaf].push(portal_idx);
    }
    result
}

/// Compute per-leaf PVS by BFS flood-fill through the portal graph.
///
/// Returns `pvs[leaf_idx][other_leaf_idx] = true` if `other_leaf_idx` is
/// potentially visible from `leaf_idx`. Solid leaves have all-false PVS rows
/// and are never marked visible from any other leaf.
///
/// Each non-solid leaf's BFS is independent, so computation is parallelized
/// with rayon.
pub fn compute_pvs(portals: &[Portal], leaf_count: usize, solid: &[bool]) -> Vec<Vec<bool>> {
    let adjacency = leaf_portals(portals, leaf_count);

    (0..leaf_count)
        .into_par_iter()
        .map(|leaf_idx| {
            if solid[leaf_idx] {
                // Solid leaves have empty PVS -- they are never a camera leaf.
                return vec![false; leaf_count];
            }

            let mut visible = vec![false; leaf_count];
            let mut queue = VecDeque::new();

            // A leaf always sees itself.
            visible[leaf_idx] = true;
            queue.push_back(leaf_idx);

            while let Some(current) = queue.pop_front() {
                for &portal_idx in &adjacency[current] {
                    let portal = &portals[portal_idx];
                    let neighbor = if portal.front_leaf == current {
                        portal.back_leaf
                    } else {
                        portal.front_leaf
                    };

                    if !solid[neighbor] && !visible[neighbor] {
                        visible[neighbor] = true;
                        queue.push_back(neighbor);
                    }
                }
            }

            visible
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn make_portal(front_leaf: usize, back_leaf: usize) -> Portal {
        // Minimal valid portal polygon (not used by PVS, only topology matters).
        Portal {
            polygon: vec![Vec3::ZERO, Vec3::X, Vec3::Y],
            front_leaf,
            back_leaf,
        }
    }

    #[test]
    fn two_leaves_connected_by_one_portal_see_each_other() {
        let portals = vec![make_portal(0, 1)];
        let solid = vec![false, false];

        let pvs = compute_pvs(&portals, 2, &solid);

        // Leaf 0 sees leaf 0 and leaf 1.
        assert!(pvs[0][0], "leaf 0 should see itself");
        assert!(pvs[0][1], "leaf 0 should see leaf 1");

        // Leaf 1 sees leaf 0 and leaf 1.
        assert!(pvs[1][0], "leaf 1 should see leaf 0");
        assert!(pvs[1][1], "leaf 1 should see itself");
    }

    #[test]
    fn three_leaves_in_chain_all_see_each_other() {
        // A -- B -- C (A=0, B=1, C=2)
        let portals = vec![make_portal(0, 1), make_portal(1, 2)];
        let solid = vec![false, false, false];

        let pvs = compute_pvs(&portals, 3, &solid);

        // A sees B and C (transitive through B).
        assert!(pvs[0][0], "A sees itself");
        assert!(pvs[0][1], "A sees B");
        assert!(pvs[0][2], "A sees C");

        // C sees A and B (transitive through B).
        assert!(pvs[2][0], "C sees A");
        assert!(pvs[2][1], "C sees B");
        assert!(pvs[2][2], "C sees itself");

        // B sees all.
        assert!(pvs[1][0], "B sees A");
        assert!(pvs[1][1], "B sees itself");
        assert!(pvs[1][2], "B sees C");
    }

    #[test]
    fn two_leaves_with_no_portal_see_neither() {
        // No portals at all.
        let portals: Vec<Portal> = Vec::new();
        let solid = vec![false, false];

        let pvs = compute_pvs(&portals, 2, &solid);

        // Each leaf sees only itself.
        assert!(pvs[0][0], "leaf 0 sees itself");
        assert!(!pvs[0][1], "leaf 0 should not see leaf 1");

        assert!(!pvs[1][0], "leaf 1 should not see leaf 0");
        assert!(pvs[1][1], "leaf 1 sees itself");
    }

    #[test]
    fn solid_leaf_never_visible() {
        // Leaf 0 (empty) -- portal -- Leaf 1 (solid) -- portal -- Leaf 2 (empty)
        let portals = vec![make_portal(0, 1), make_portal(1, 2)];
        let solid = vec![false, true, false];

        let pvs = compute_pvs(&portals, 3, &solid);

        // Leaf 0: sees itself, does NOT see solid leaf 1, does NOT see leaf 2
        // (because the only path to leaf 2 goes through solid leaf 1).
        assert!(pvs[0][0], "leaf 0 sees itself");
        assert!(!pvs[0][1], "leaf 0 should not see solid leaf 1");
        assert!(
            !pvs[0][2],
            "leaf 0 should not see leaf 2 (blocked by solid)"
        );

        // Solid leaf 1: all-false PVS.
        assert!(!pvs[1][0], "solid leaf should not see anything");
        assert!(!pvs[1][1], "solid leaf should not see itself");
        assert!(!pvs[1][2], "solid leaf should not see anything");

        // Leaf 2: same as leaf 0 — isolated by solid leaf.
        assert!(
            !pvs[2][0],
            "leaf 2 should not see leaf 0 (blocked by solid)"
        );
        assert!(!pvs[2][1], "leaf 2 should not see solid leaf 1");
        assert!(pvs[2][2], "leaf 2 sees itself");
    }

    #[test]
    fn empty_portals_and_leaves_produces_empty_pvs() {
        let pvs = compute_pvs(&[], 0, &[]);
        assert!(pvs.is_empty());
    }

    #[test]
    fn single_leaf_sees_itself() {
        let pvs = compute_pvs(&[], 1, &[false]);
        assert_eq!(pvs.len(), 1);
        assert!(pvs[0][0], "single leaf should see itself");
    }

    #[test]
    fn diamond_topology_all_empty_all_visible() {
        // 4 leaves, diamond: 0-1, 0-2, 1-3, 2-3
        let portals = vec![
            make_portal(0, 1),
            make_portal(0, 2),
            make_portal(1, 3),
            make_portal(2, 3),
        ];
        let solid = vec![false, false, false, false];

        let pvs = compute_pvs(&portals, 4, &solid);

        for i in 0..4 {
            for j in 0..4 {
                assert!(
                    pvs[i][j],
                    "leaf {i} should see leaf {j} in fully connected diamond"
                );
            }
        }
    }
}
