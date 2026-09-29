//! Deterministic camera walks over portal adjacency between camera cells:
//! a seeded random walk and a far-point tour, for the cell-block pool
//! simulation.

use std::collections::VecDeque;

use super::portal_distance::PortalGraphInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WalkKind {
    /// Uniform random portal neighbour each step.
    Random,
    /// Walk the shortest hop path to the farthest unvisited camera cell,
    /// repeatedly: long traversals that cover the map.
    FarPointTour,
}

impl WalkKind {
    pub(crate) const ALL: [WalkKind; 2] = [WalkKind::Random, WalkKind::FarPointTour];

    pub(crate) fn label(self) -> &'static str {
        match self {
            WalkKind::Random => "random walk",
            WalkKind::FarPointTour => "far-point tour",
        }
    }
}

/// Deterministic SplitMix64; the dry run takes no RNG dependency.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Portal adjacency between camera cells, by index into `camera_cells`.
pub(crate) fn camera_adjacency(graph: &PortalGraphInput, camera_cells: &[u32]) -> Vec<Vec<u32>> {
    let mut index = vec![u32::MAX; graph.cells.len()];
    for (i, &cell) in camera_cells.iter().enumerate() {
        index[cell as usize] = i as u32;
    }
    let mut adjacency = vec![Vec::new(); camera_cells.len()];
    for portal in &graph.portals {
        let (a, b) = (index[portal.front as usize], index[portal.back as usize]);
        if a != u32::MAX && b != u32::MAX && a != b {
            adjacency[a as usize].push(b);
            adjacency[b as usize].push(a);
        }
    }
    for neighbours in &mut adjacency {
        neighbours.sort_unstable();
        neighbours.dedup();
    }
    adjacency
}

/// A walk of `steps` camera-cell indices and its teleport count.
pub(crate) fn walk_path(
    kind: WalkKind,
    adjacency: &[Vec<u32>],
    steps: usize,
    seed: u64,
) -> (Vec<u32>, usize) {
    let n = adjacency.len();
    if n == 0 {
        return (Vec::new(), 0);
    }
    let mut rng = SplitMix64(seed);
    let mut current = rng.below(n) as u32;
    let mut path = vec![current];
    let mut teleports = 0;
    match kind {
        WalkKind::Random => {
            while path.len() < steps {
                let neighbours = &adjacency[current as usize];
                current = if neighbours.is_empty() {
                    teleports += 1;
                    rng.below(n) as u32
                } else {
                    neighbours[rng.below(neighbours.len())]
                };
                path.push(current);
            }
        }
        WalkKind::FarPointTour => {
            let mut visited = vec![false; n];
            visited[current as usize] = true;
            let mut parent = vec![u32::MAX; n];
            let mut depth = vec![u32::MAX; n];
            let mut queue = VecDeque::new();
            while path.len() < steps {
                depth.fill(u32::MAX);
                depth[current as usize] = 0;
                queue.push_back(current);
                let mut target: Option<(u32, u32)> = None;
                while let Some(cell) = queue.pop_front() {
                    let d = depth[cell as usize];
                    if !visited[cell as usize] && target.is_none_or(|(best, _)| d > best) {
                        target = Some((d, cell));
                    }
                    for &next in &adjacency[cell as usize] {
                        if depth[next as usize] == u32::MAX {
                            depth[next as usize] = d + 1;
                            parent[next as usize] = cell;
                            queue.push_back(next);
                        }
                    }
                }
                match target {
                    Some((_, target)) => {
                        let mut leg = Vec::new();
                        let mut cell = target;
                        while cell != current {
                            leg.push(cell);
                            cell = parent[cell as usize];
                        }
                        for &cell in leg.iter().rev() {
                            visited[cell as usize] = true;
                            path.push(cell);
                        }
                        current = target;
                    }
                    None => {
                        if let Some(unvisited) = visited.iter().position(|&v| !v) {
                            teleports += 1;
                            current = unvisited as u32;
                            visited[unvisited] = true;
                        } else {
                            visited.fill(false);
                            visited[current as usize] = true;
                            if n == 1 {
                                current = 0;
                            } else {
                                continue;
                            }
                        }
                        path.push(current);
                    }
                }
            }
            path.truncate(steps);
        }
    }
    (path, teleports)
}
