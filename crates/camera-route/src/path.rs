// Seeded camera-cell paths over portal adjacency: the walk's cell sequence.
// See: context/lib/rendering_pipeline.md §12

// A port of the dry run's `camera_walks.rs` (level-compiler
// `lightmap_residency_dry_run/`), test-only code in the `prl-build` binary.
// Same camera cells (neither solid nor exterior, ascending id), adjacency,
// SplitMix64 and seed, so a runtime walk of N steps is the dry run's first N.

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkKind {
    /// Uniform random portal neighbour each step; teleports when stalled.
    Random,
    /// Shortest hop path to the farthest unvisited camera cell, repeatedly.
    FarPointTour,
}

impl WalkKind {
    pub const ALL: [WalkKind; 2] = [WalkKind::Random, WalkKind::FarPointTour];

    /// The harness's run label. Trip records pair runs on it, so it never
    /// changes.
    pub fn label(self) -> &'static str {
        match self {
            WalkKind::Random => "random walk",
            WalkKind::FarPointTour => "far-point tour",
        }
    }

    /// The kind's name on a command line and in a pose file's origin.
    pub fn key(self) -> &'static str {
        match self {
            WalkKind::Random => "random",
            WalkKind::FarPointTour => "far-point",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }
}

/// The dry run's `SIM_SEED`.
pub const WALK_SEED: u64 = 0x5EED_B10C;
/// The dry run's `STALL_TELEPORT_STEPS`.
pub const STALL_TELEPORT_STEPS: usize = 1_000;

/// Deterministic SplitMix64, as the dry run uses.
pub struct SplitMix64(pub u64);

impl SplitMix64 {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Portal adjacency between camera cells, by index into `camera_cells`.
/// `portals` are (front, back) cell pairs.
pub fn camera_adjacency(
    cell_count: usize,
    portals: impl Iterator<Item = (usize, usize)>,
    camera_cells: &[u32],
) -> Vec<Vec<u32>> {
    let mut index = vec![u32::MAX; cell_count];
    for (i, &cell) in camera_cells.iter().enumerate() {
        index[cell as usize] = i as u32;
    }
    let mut adjacency = vec![Vec::new(); camera_cells.len()];
    for (front, back) in portals {
        let (a, b) = (index[front], index[back]);
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
pub fn walk_path(
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
            let mut visited = vec![false; n];
            visited[current as usize] = true;
            let mut unvisited = n - 1;
            let mut since_new = 0;
            while path.len() < steps {
                let neighbours = &adjacency[current as usize];
                current = if neighbours.is_empty() {
                    teleports += 1;
                    rng.below(n) as u32
                } else if since_new >= STALL_TELEPORT_STEPS && unvisited > 0 {
                    teleports += 1;
                    let pick = rng.below(unvisited);
                    let target = (0..n).filter(|&c| !visited[c]).nth(pick);
                    target.expect("an unvisited cell remains") as u32
                } else {
                    neighbours[rng.below(neighbours.len())]
                };
                if visited[current as usize] {
                    since_new += 1;
                } else {
                    visited[current as usize] = true;
                    unvisited -= 1;
                    since_new = 0;
                }
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
