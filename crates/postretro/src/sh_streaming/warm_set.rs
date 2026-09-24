//! Camera-centered SH prefetch warm set.
//!
//! The warm set depends on the camera cell alone, never on the view frustum,
//! so turning in place cannot change prefetch. It is a bounded shortest-path
//! walk over the baked id-46 coupled pairs, mapped to id-49 clusters.
//! See: context/lib/rendering_pipeline.md §4

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};

use postretro_level_format::cell_visibility::CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;
use postretro_level_loader::{CellVisibility, CoupledCellPair};

use super::controller::{
    PREFETCH_HOPS, ShResidencyControllerError, WARM_SET_CLUSTERS, WARM_WALK_MAX_SETTLED_CELLS,
};
use super::topology::PlannerTopology;

/// Where one controller's warm set comes from, fixed at construction.
#[derive(Debug)]
pub(super) enum WarmSource {
    CellGraph(CellGraph),
    /// id 46 is absent or disagrees with the id-49 cell map. The warm set is
    /// then the `PREFETCH_HOPS` cluster-adjacency expansion of the camera's
    /// cluster, unranked, which reproduces the legacy horizon ordering.
    ClusterHops,
}

/// Undirected per-cell adjacency over the id-46 coupled pairs, in CSR form:
/// cell `c`'s edges are `edges[offsets[c]..offsets[c + 1]]`.
#[derive(Debug)]
pub(super) struct CellGraph {
    offsets: Vec<usize>,
    edges: Vec<CellEdge>,
}

#[derive(Debug, Clone, Copy)]
struct CellEdge {
    neighbor: usize,
    distance: u32,
    aperture: u32,
}

/// The warm clusters for one camera cell, each with its rank ordinal
/// (0 = nearest). Rank is the tie-break inside a request class and priority;
/// under pressure the highest rank yields first.
#[derive(Debug, Default)]
pub(super) struct WarmSet {
    camera_cell: Option<usize>,
    ranks: BTreeMap<u32, u32>,
}

impl WarmSet {
    pub(super) fn camera_cell(&self) -> Option<usize> {
        self.camera_cell
    }

    pub(super) fn len(&self) -> usize {
        self.ranks.len()
    }

    pub(super) fn clusters(&self) -> impl Iterator<Item = u32> + '_ {
        self.ranks.keys().copied()
    }

    /// Clusters outside the warm set rank after every warm cluster.
    pub(super) fn rank(&self, cluster_id: u32) -> u32 {
        self.ranks.get(&cluster_id).copied().unwrap_or(u32::MAX)
    }
}

impl WarmSource {
    /// Logs the fallback once, here, so each controller warns at most once.
    pub(super) fn from_cell_visibility(
        cell_visibility: Option<&CellVisibility>,
        runtime_cell_count: usize,
    ) -> Self {
        Self::resolve(
            cell_visibility.map(|section| (section.component_ids().len(), section.coupled_pairs())),
            runtime_cell_count,
        )
    }

    /// `section` is the id-46 cell count and its coupled pairs, when present.
    pub(super) fn resolve<'a>(
        section: Option<(usize, impl IntoIterator<Item = &'a CoupledCellPair>)>,
        runtime_cell_count: usize,
    ) -> Self {
        let graph = match section {
            None => Err("is absent".to_owned()),
            Some((section_cell_count, pairs)) => {
                CellGraph::from_pairs(section_cell_count, runtime_cell_count, pairs)
            }
        };
        match graph {
            Ok(graph) => Self::CellGraph(graph),
            Err(reason) => {
                log::warn!(
                    "[SH streaming] CellVisibility (id 46) {reason}; prefetch falls back to \
                     {PREFETCH_HOPS}-hop cluster adjacency from the camera cluster"
                );
                Self::ClusterHops
            }
        }
    }

    /// Computes the warm set for one camera cell. `None` (no level) is empty.
    pub(super) fn warm_set(
        &self,
        topology: &PlannerTopology,
        camera_cell: Option<usize>,
    ) -> Result<WarmSet, ShResidencyControllerError> {
        let Some(camera_cell) = camera_cell else {
            return Ok(WarmSet::default());
        };
        let camera_cluster = *topology.cell_to_cluster.get(camera_cell).ok_or_else(|| {
            ShResidencyControllerError::InvalidTopology(format!(
                "camera cell {camera_cell} is outside the id-49 cell map"
            ))
        })?;
        let ranks = match self {
            Self::CellGraph(graph) => graph.walk(&topology.cell_to_cluster, camera_cell),
            Self::ClusterHops => cluster_hops(&topology.adjacency, camera_cluster)?,
        };
        Ok(WarmSet {
            camera_cell: Some(camera_cell),
            ranks,
        })
    }
}

impl CellGraph {
    fn from_pairs<'a>(
        section_cell_count: usize,
        runtime_cell_count: usize,
        pairs: impl IntoIterator<Item = &'a CoupledCellPair>,
    ) -> Result<Self, String> {
        if section_cell_count != runtime_cell_count {
            return Err(format!(
                "has {section_cell_count} cells but the id-49 cell map has {runtime_cell_count}"
            ));
        }
        let pairs: Vec<_> = pairs
            .into_iter()
            .filter(|pair| pair.cell_a != pair.cell_b)
            .collect();
        let mut degrees = vec![0usize; runtime_cell_count];
        for pair in &pairs {
            for cell in [pair.cell_a, pair.cell_b] {
                let degree = degrees
                    .get_mut(cell)
                    .ok_or_else(|| format!("pairs name cell {cell} outside the id-49 cell map"))?;
                *degree += 1;
            }
        }
        let mut offsets = Vec::with_capacity(runtime_cell_count + 1);
        offsets.push(0);
        for degree in &degrees {
            offsets.push(offsets.last().copied().unwrap_or(0) + degree);
        }
        let mut cursor = offsets[..runtime_cell_count].to_vec();
        let placeholder = CellEdge {
            neighbor: 0,
            distance: 0,
            aperture: 0,
        };
        let mut edges = vec![placeholder; offsets[runtime_cell_count]];
        for pair in pairs {
            for (from, to) in [(pair.cell_a, pair.cell_b), (pair.cell_b, pair.cell_a)] {
                edges[cursor[from]] = CellEdge {
                    neighbor: to,
                    distance: pair.distance,
                    aperture: pair.aperture,
                };
                cursor[from] += 1;
            }
        }
        Ok(Self { offsets, edges })
    }

    fn neighbors(&self, cell: usize) -> &[CellEdge] {
        &self.edges[self.offsets[cell]..self.offsets[cell + 1]]
    }

    /// Dijkstra from the camera cell. A path's aperture is its bottleneck
    /// (minimum edge aperture); among equal-distance paths the wider one
    /// wins. The frontier pops by (distance, wider aperture, lower cell id),
    /// so the result is identical for a given camera cell and level.
    fn walk(&self, cell_to_cluster: &[u32], camera_cell: usize) -> BTreeMap<u32, u32> {
        let mut best = BTreeMap::from([(camera_cell, (0u64, u32::MAX))]);
        let mut settled = BTreeSet::new();
        let mut frontier = BinaryHeap::from([Reverse((0u64, Reverse(u32::MAX), camera_cell))]);
        // Each cluster's key comes from its first settled cell.
        let mut reached = BTreeMap::<u32, (u64, Reverse<u32>)>::new();
        let units_per_metre = u64::from(CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE);
        while let Some(Reverse((distance, Reverse(aperture), cell))) = frontier.pop() {
            if best.get(&cell) != Some(&(distance, aperture)) || !settled.insert(cell) {
                continue;
            }
            reached
                .entry(cell_to_cluster[cell])
                .or_insert((distance / units_per_metre, Reverse(aperture)));
            if reached.len() == WARM_SET_CLUSTERS || settled.len() == WARM_WALK_MAX_SETTLED_CELLS {
                break;
            }
            for edge in self.neighbors(cell) {
                if settled.contains(&edge.neighbor) {
                    continue;
                }
                let candidate = (
                    distance.saturating_add(u64::from(edge.distance)),
                    aperture.min(edge.aperture),
                );
                let improves =
                    best.get(&edge.neighbor)
                        .is_none_or(|&(known_distance, known_aperture)| {
                            candidate.0 < known_distance
                                || (candidate.0 == known_distance && candidate.1 > known_aperture)
                        });
                if improves {
                    best.insert(edge.neighbor, candidate);
                    frontier.push(Reverse((candidate.0, Reverse(candidate.1), edge.neighbor)));
                }
            }
        }
        // Rank: whole-metre distance bucket, then wider aperture, then id.
        let mut ordered: Vec<_> = reached
            .into_iter()
            .map(|(cluster_id, (bucket, aperture))| (bucket, aperture, cluster_id))
            .collect();
        ordered.sort_unstable();
        ordered
            .into_iter()
            .zip(0u32..)
            .map(|((_, _, cluster_id), rank)| (cluster_id, rank))
            .collect()
    }
}

/// Fallback expansion. Every cluster shares rank 0, so request and pressure
/// order among them stays the legacy cluster-id and LRU order.
fn cluster_hops(
    adjacency: &[Vec<u32>],
    camera_cluster: u32,
) -> Result<BTreeMap<u32, u32>, ShResidencyControllerError> {
    let mut reached = BTreeMap::from([(camera_cluster, 0)]);
    let mut queue = VecDeque::from([(camera_cluster, 0u8)]);
    while let Some((cluster_id, depth)) = queue.pop_front() {
        if depth == PREFETCH_HOPS {
            continue;
        }
        let neighbors = adjacency.get(cluster_id as usize).ok_or_else(|| {
            ShResidencyControllerError::InvalidTopology("camera cluster exceeds adjacency".into())
        })?;
        for &neighbor in neighbors {
            if reached.insert(neighbor, 0).is_none() {
                queue.push_back((neighbor, depth + 1));
            }
        }
    }
    Ok(reached)
}
