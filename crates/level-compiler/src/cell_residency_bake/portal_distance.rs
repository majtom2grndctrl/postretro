//! Untruncated portal-path distance between cells: the id-46 hub metric,
//! recomputed with every partner kept.
//!
//! Id 46 stores, per source cell, only its `CELL_VISIBILITY_FANOUT_K` nearest
//! partners, so a cell with more than K partners inside a bound loses some.
//! This rebuilds the bake's portal hub metric from Cells (id 38) and Portals
//! (id 15) — cell AABB center to portal centroid to next cell center, shortest
//! path — and keeps every partner within a bound. The residency bake reads it
//! as the movement lead; the lightmap residency dry run checks it against the
//! stored id-46 records.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use glam::DVec3;
use postretro_level_format::cell_visibility::{
    CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE, CoupledPairRecord,
};
use postretro_level_format::cells::CellsSection;
use postretro_level_format::portals::PortalsSection;
use rayon::prelude::*;

use crate::cell_visibility_bake::metrics::{fixed_point_value, portal_metrics};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HubCell {
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub solid: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HubPortal {
    pub front: u32,
    pub back: u32,
    pub vertices: Vec<[f32; 3]>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PortalGraphInput {
    pub cells: Vec<HubCell>,
    pub portals: Vec<HubPortal>,
}

/// The hub-metric graph input straight from the encoded sections. A portal
/// whose vertex range falls outside the vertex buffer keeps an empty polygon,
/// which the hub graph skips, instead of panicking.
pub(crate) fn portal_graph_from_sections(
    cells: &CellsSection,
    portals: &PortalsSection,
) -> PortalGraphInput {
    PortalGraphInput {
        cells: cells
            .cells
            .iter()
            .map(|record| HubCell {
                bounds_min: record.bounds_min,
                bounds_max: record.bounds_max,
                solid: record.is_solid(),
            })
            .collect(),
        portals: portals
            .portals
            .iter()
            .map(|record| {
                let start = record.vertex_start as usize;
                let vertices = start
                    .checked_add(record.vertex_count as usize)
                    .and_then(|end| portals.vertices.get(start..end))
                    .unwrap_or_default();
                HubPortal {
                    front: record.front_leaf,
                    back: record.back_leaf,
                    vertices: vertices.to_vec(),
                }
            })
            .collect(),
    }
}

/// The bake's hub graph: one node per usable portal (its centroid) and one
/// hub per non-solid cell with portals (its AABB center), each hub joined to
/// its portals by straight-line meters.
pub(crate) struct HubGraph {
    portal_count: usize,
    /// Node of each cell's hub; `None` for cells the metric cannot reach.
    hub_node: Vec<Option<usize>>,
    /// Cell of each hub, indexed by `node - portal_count`.
    hub_cell: Vec<u32>,
    adjacency: Vec<Vec<(usize, f64)>>,
}

/// One source's settled distances, reused across sources.
pub(crate) struct HubDistances {
    distances: Vec<f64>,
    touched: Vec<usize>,
}

impl HubGraph {
    pub(crate) fn new(graph: &PortalGraphInput) -> Self {
        let cell_count = graph.cells.len();
        let valid = |cell: &HubCell| {
            let min = DVec3::from(cell.bounds_min.map(f64::from));
            let max = DVec3::from(cell.bounds_max.map(f64::from));
            !cell.solid && min.is_finite() && max.is_finite() && min.cmple(max).all()
        };
        let centroid = |cell: &HubCell| {
            (DVec3::from(cell.bounds_min.map(f64::from))
                + DVec3::from(cell.bounds_max.map(f64::from)))
                * 0.5
        };

        let mut portal_centroids = Vec::new();
        let mut incident: Vec<Vec<usize>> = vec![Vec::new(); cell_count];
        for portal in &graph.portals {
            let (front, back) = (portal.front as usize, portal.back as usize);
            if front >= cell_count || back >= cell_count {
                continue;
            }
            if graph.cells[front].solid || graph.cells[back].solid {
                continue;
            }
            let vertices: Vec<DVec3> = portal
                .vertices
                .iter()
                .map(|v| DVec3::from(v.map(f64::from)))
                .collect();
            let Some(center) = portal_metrics(&vertices).centroid else {
                continue;
            };
            let node = portal_centroids.len();
            portal_centroids.push(center);
            incident[front].push(node);
            incident[back].push(node);
        }

        let portal_count = portal_centroids.len();
        let mut hub_node = vec![None; cell_count];
        let mut hub_cell = Vec::new();
        for (cell, info) in graph.cells.iter().enumerate() {
            if valid(info) && !incident[cell].is_empty() {
                hub_node[cell] = Some(portal_count + hub_cell.len());
                hub_cell.push(cell as u32);
            }
        }
        let mut adjacency: Vec<Vec<(usize, f64)>> = vec![Vec::new(); portal_count + hub_cell.len()];
        for (cell, nodes) in incident.iter().enumerate() {
            let Some(hub) = hub_node[cell] else {
                continue;
            };
            let center = centroid(&graph.cells[cell]);
            for &portal in nodes {
                let cost = center.distance(portal_centroids[portal]);
                adjacency[hub].push((portal, cost));
                adjacency[portal].push((hub, cost));
            }
        }
        Self {
            portal_count,
            hub_node,
            hub_cell,
            adjacency,
        }
    }

    pub(crate) fn scratch(&self) -> HubDistances {
        HubDistances {
            distances: vec![f64::INFINITY; self.adjacency.len()],
            touched: Vec::new(),
        }
    }

    /// Settle every node within `max_meters` of `source`'s hub, clearing the
    /// previous source first. False when `source` has no hub.
    pub(crate) fn settle_from(&self, source: u32, max_meters: f64, out: &mut HubDistances) -> bool {
        for &node in &out.touched {
            out.distances[node] = f64::INFINITY;
        }
        out.touched.clear();
        let Some(Some(hub)) = self.hub_node.get(source as usize).copied() else {
            return false;
        };
        bounded_dijkstra(
            &self.adjacency,
            hub,
            max_meters,
            &mut out.distances,
            &mut out.touched,
        );
        true
    }

    /// Hub-metric meters from the last settled source to `cell`, if reached.
    #[cfg(test)]
    pub(crate) fn meters_to(&self, out: &HubDistances, cell: u32) -> Option<f64> {
        let hub = self.hub_node.get(cell as usize).copied().flatten()?;
        let meters = out.distances[hub];
        meters.is_finite().then_some(meters)
    }

    /// Cells settled from the last source, with their meters, source included.
    fn settled_cells<'a>(&'a self, out: &'a HubDistances) -> impl Iterator<Item = (u32, f64)> + 'a {
        out.touched
            .iter()
            .filter(|&&node| node >= self.portal_count)
            .map(|&node| (self.hub_cell[node - self.portal_count], out.distances[node]))
    }
}

/// Every unordered cell pair whose hub-metric distance is within `max_fixed`,
/// sorted by `(cell_a, cell_b)`. Sources settle in parallel; the sort makes
/// the result independent of scheduling.
pub(crate) fn recompute_pairs(graph: &PortalGraphInput, max_fixed: u32) -> Vec<CoupledPairRecord> {
    let hubs = HubGraph::new(graph);
    // Stop expanding once a path exceeds the bound, with slack so rounding at
    // the boundary is decided by the fixed-point value alone.
    let max_meters =
        f64::from(max_fixed) / f64::from(CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE) + 1.0;
    let per_source: Vec<Vec<CoupledPairRecord>> = hubs
        .hub_cell
        .par_iter()
        .map_init(
            || hubs.scratch(),
            |scratch, &source_cell| {
                hubs.settle_from(source_cell, max_meters, scratch);
                hubs.settled_cells(scratch)
                    .filter(|&(target_cell, _)| target_cell > source_cell)
                    .filter_map(|(target_cell, meters)| {
                        let (fixed, _) =
                            fixed_point_value(meters, CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE);
                        (fixed <= max_fixed).then_some(CoupledPairRecord {
                            cell_a: source_cell,
                            cell_b: target_cell,
                            distance: fixed,
                            aperture: 0,
                        })
                    })
                    .collect()
            },
        )
        .collect();
    let mut pairs = per_source.concat();
    pairs.sort_unstable_by_key(|pair| (pair.cell_a, pair.cell_b));
    pairs
}

/// Coupled partners of each cell with their fixed-point distance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Neighbors {
    per_cell: Vec<Vec<(u32, u32)>>,
}

impl Neighbors {
    pub(crate) fn from_pairs(cell_count: usize, pairs: &[CoupledPairRecord]) -> Self {
        let mut per_cell = vec![Vec::new(); cell_count];
        for pair in pairs {
            per_cell[pair.cell_a as usize].push((pair.cell_b, pair.distance));
            per_cell[pair.cell_b as usize].push((pair.cell_a, pair.distance));
        }
        for partners in &mut per_cell {
            partners.sort_unstable_by_key(|&(cell, distance)| (distance, cell));
        }
        Self { per_cell }
    }

    /// Partners within `fixed` (inclusive) with their fixed-point distance,
    /// nearest first.
    pub(crate) fn within_distances(
        &self,
        cell: u32,
        fixed: u32,
    ) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.per_cell[cell as usize]
            .iter()
            .take_while(move |&&(_, distance)| distance <= fixed)
            .copied()
    }

    /// Partners within `fixed` (inclusive), nearest first.
    #[cfg(test)]
    pub(crate) fn within(&self, cell: u32, fixed: u32) -> impl Iterator<Item = u32> + '_ {
        self.within_distances(cell, fixed).map(|(other, _)| other)
    }
}

#[derive(Clone, Copy)]
struct Frontier {
    cost: f64,
    node: usize,
}

impl PartialEq for Frontier {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Frontier {}
impl PartialOrd for Frontier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Frontier {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed for a min-heap on (cost, node).
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.node.cmp(&self.node))
    }
}

/// Dijkstra from `source`, settling nodes no farther than `max_cost`.
/// `distances` must be all-infinite on entry; `touched` lists every node
/// written so the caller can reset them.
fn bounded_dijkstra(
    adjacency: &[Vec<(usize, f64)>],
    source: usize,
    max_cost: f64,
    distances: &mut [f64],
    touched: &mut Vec<usize>,
) {
    touched.clear();
    distances[source] = 0.0;
    touched.push(source);
    let mut frontier = BinaryHeap::from([Frontier {
        cost: 0.0,
        node: source,
    }]);
    while let Some(entry) = frontier.pop() {
        if entry.cost > distances[entry.node] {
            continue;
        }
        for &(next, cost) in &adjacency[entry.node] {
            let candidate = entry.cost + cost;
            if candidate <= max_cost && candidate < distances[next] {
                if distances[next].is_infinite() {
                    touched.push(next);
                }
                distances[next] = candidate;
                frontier.push(Frontier {
                    cost: candidate,
                    node: next,
                });
            }
        }
    }
}
