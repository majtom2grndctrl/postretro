//! Untruncated recompute of the id-46 coupled-pair distance.
//!
//! Id 46 stores, per source cell, only its `CELL_VISIBILITY_FANOUT_K` nearest
//! partners, so a cell with more than K partners inside a bound loses some.
//! This rebuilds the bake's portal hub metric from Cells (id 38) and Portals
//! (id 15) — cell AABB center to portal centroid to next cell center, shortest
//! path — keeps every partner within a bound, and checks itself against the
//! stored records.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use glam::DVec3;
use postretro_level_format::cell_visibility::{
    CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE, CoupledPairRecord,
};

use crate::cell_visibility_bake::metrics::{fixed_point_value, portal_metrics};

/// Fixed-point slack for f32 section bounds versus the bake's f64 leaves.
pub(crate) const VALIDATION_TOLERANCE_FIXED: u32 = 2;

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DistanceValidation {
    /// Stored id-46 records within the recompute bound.
    pub checked: usize,
    pub matched: usize,
    /// Found by the recompute but more than the tolerance apart.
    pub mismatched: usize,
    /// Stored records the recompute found no path for within the bound.
    pub missing: usize,
    pub max_abs_diff: u32,
}

/// Every unordered cell pair whose hub-metric distance is within `max_fixed`.
pub(crate) fn recompute_pairs(graph: &PortalGraphInput, max_fixed: u32) -> Vec<CoupledPairRecord> {
    let cell_count = graph.cells.len();
    let valid = |cell: &HubCell| {
        let min = DVec3::from(cell.bounds_min.map(f64::from));
        let max = DVec3::from(cell.bounds_max.map(f64::from));
        !cell.solid && min.is_finite() && max.is_finite() && min.cmple(max).all()
    };
    let centroid = |cell: &HubCell| {
        (DVec3::from(cell.bounds_min.map(f64::from)) + DVec3::from(cell.bounds_max.map(f64::from)))
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

    // Stop expanding once a path exceeds the bound, with slack so rounding at
    // the boundary is decided by the fixed-point value alone.
    let max_meters =
        f64::from(max_fixed) / f64::from(CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE) + 1.0;
    let mut pairs = Vec::new();
    let mut distances = vec![f64::INFINITY; adjacency.len()];
    let mut touched = Vec::new();
    for (source_slot, &source_cell) in hub_cell.iter().enumerate() {
        let source = portal_count + source_slot;
        bounded_dijkstra(&adjacency, source, max_meters, &mut distances, &mut touched);
        for &node in &touched {
            if node < portal_count || node == source {
                continue;
            }
            let target_cell = hub_cell[node - portal_count];
            if target_cell <= source_cell {
                continue;
            }
            let (fixed, _) =
                fixed_point_value(distances[node], CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE);
            if fixed <= max_fixed {
                pairs.push(CoupledPairRecord {
                    cell_a: source_cell,
                    cell_b: target_cell,
                    distance: fixed,
                    aperture: 0,
                });
            }
        }
        for &node in &touched {
            distances[node] = f64::INFINITY;
        }
    }
    pairs.sort_unstable_by_key(|pair| (pair.cell_a, pair.cell_b));
    pairs
}

/// Compare stored id-46 records inside `max_fixed` with the recompute.
pub(crate) fn validate_against_stored(
    stored: &[CoupledPairRecord],
    recomputed: &[CoupledPairRecord],
    max_fixed: u32,
) -> DistanceValidation {
    let mut result = DistanceValidation::default();
    for pair in stored.iter().filter(|pair| pair.distance <= max_fixed) {
        result.checked += 1;
        match recomputed.binary_search_by_key(&(pair.cell_a, pair.cell_b), |r| (r.cell_a, r.cell_b))
        {
            Ok(index) => {
                let diff = recomputed[index].distance.abs_diff(pair.distance);
                result.max_abs_diff = result.max_abs_diff.max(diff);
                if diff <= VALIDATION_TOLERANCE_FIXED {
                    result.matched += 1;
                } else {
                    result.mismatched += 1;
                }
            }
            Err(_) => result.missing += 1,
        }
    }
    result
}

#[derive(Clone, Copy)]
struct Frontier {
    cost: f64,
    node: usize,
}

impl DistanceValidation {
    pub(crate) fn all_matched(&self) -> bool {
        self.matched == self.checked
    }
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
