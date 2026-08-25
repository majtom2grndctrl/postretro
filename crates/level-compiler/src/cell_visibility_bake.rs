// Static CellVisibility bake over the compiler portal graph.
// See: context/plans/in-progress/cell-visibility-relation/index.md

use std::{
    cmp::Ordering,
    collections::{BinaryHeap, VecDeque},
};

use glam::DVec3;
use postretro_level_format::cell_visibility::{
    CELL_VISIBILITY_APERTURE_FIXED_POINT_SCALE, CELL_VISIBILITY_DISTANCE_CAP,
    CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE, CellVisibilitySection, CoupledPairRecord,
};

use crate::partition::BspTree;
use crate::portals::Portal;

/// Bake the v1 cell-coupling relation with the shipped side-table horizon.
pub fn cell_visibility_bake(
    tree: &BspTree,
    portals: &[Portal],
) -> anyhow::Result<CellVisibilitySection> {
    bake_with_coupling_cap(tree, portals, CELL_VISIBILITY_DISTANCE_CAP)
}

fn bake_with_coupling_cap(
    tree: &BspTree,
    portals: &[Portal],
    coupling_cap: u32,
) -> anyhow::Result<CellVisibilitySection> {
    let cell_count = tree.leaves.len();
    anyhow::ensure!(
        cell_count != 0,
        "CellVisibility requires at least one BSP leaf"
    );
    let cell_count_u32 = u32::try_from(cell_count)
        .map_err(|_| anyhow::anyhow!("CellVisibility cannot encode more than u32::MAX cells"))?;

    let portal_edges = portal_edges(tree, portals);
    let component_ids = component_ids(cell_count, &portal_edges);
    let coupled_pairs = assemble_coupled_pairs(tree, &portal_edges, &component_ids, coupling_cap);

    Ok(CellVisibilitySection {
        cell_count: cell_count_u32,
        component_ids,
        coupled_pairs,
    })
}

#[derive(Clone, Copy)]
struct PortalEdge {
    portal_index: usize,
    front: usize,
    back: usize,
    centroid: Option<DVec3>,
    aperture: f64,
}

/// Collect only portals that preserve the cell graph contract. Their input
/// indices stay attached as the stable portal order used for all tie-breaks.
fn portal_edges(tree: &BspTree, portals: &[Portal]) -> Vec<PortalEdge> {
    portals
        .iter()
        .enumerate()
        .filter_map(|(portal_index, portal)| {
            let front = tree.leaves.get(portal.front_leaf)?;
            let back = tree.leaves.get(portal.back_leaf)?;
            if front.is_solid || back.is_solid {
                return None;
            }

            let metrics = portal_metrics(&portal.polygon);
            Some(PortalEdge {
                portal_index,
                front: portal.front_leaf,
                back: portal.back_leaf,
                centroid: metrics.centroid,
                aperture: metrics.minimum_width,
            })
        })
        .collect()
}

/// Component IDs cover every BSP leaf, including solid leaves. Portal absence
/// makes a solid (or otherwise isolated) cell a singleton component. Outer
/// traversal is ascending CellId, so each component's representative is its
/// lowest member and dense IDs are ordered by that representative.
fn component_ids(cell_count: usize, portals: &[PortalEdge]) -> Vec<u32> {
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

        component_ids[start] = component_id;
        let mut queue = VecDeque::from([start]);
        while let Some(cell) = queue.pop_front() {
            // Each adjacency vector is populated in portal-index order, so the
            // traversal has no hash-table or completion-order dependency.
            for &neighbor in &adjacency[cell] {
                if component_ids[neighbor] == u32::MAX {
                    component_ids[neighbor] = component_id;
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

/// Populate the canonical unordered side table. The cap is a parameter so the
/// boundary is unit-testable without changing the shipped format contract.
fn assemble_coupled_pairs(
    tree: &BspTree,
    portals: &[PortalEdge],
    component_ids: &[u32],
    coupling_cap: u32,
) -> Vec<CoupledPairRecord> {
    let metric_graph = portal_centroid_graph(tree, portals);
    let aperture_tree = maximum_spanning_tree(tree.leaves.len(), portals);
    let cell_count = tree.leaves.len();
    let mut pairs = Vec::new();

    // Source CellIds and target CellIds are ascending, and targets begin after
    // the source. This chooses Dijkstra(min -> max) exactly once for each
    // unordered pair instead of reconciling two float accumulation orders.
    for source in 0..cell_count {
        if !metric_graph.has_endpoint[source] {
            continue;
        }
        let distances = dijkstra(&metric_graph.adjacency, metric_graph.cell_node(source));
        let apertures = widest_paths_from(&aperture_tree, source);

        for target in source + 1..cell_count {
            if component_ids[source] != component_ids[target] || !metric_graph.has_endpoint[target]
            {
                continue;
            }

            let distance = distances[metric_graph.cell_node(target)];
            if !distance.is_finite() {
                // A generated portal always has a valid polygon centroid. Keep
                // malformed hand-built test input conservative if it does not.
                continue;
            }
            let Some(aperture) = apertures[target] else {
                debug_assert!(
                    false,
                    "a component pair must be connected in the aperture tree"
                );
                continue;
            };

            let distance = fixed_point_value(
                distance,
                CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE,
                CouplingAxis::Distance,
            );
            if distance > coupling_cap {
                continue;
            }
            let aperture = fixed_point_value(
                aperture,
                CELL_VISIBILITY_APERTURE_FIXED_POINT_SCALE,
                CouplingAxis::Aperture,
            );

            pairs.push(CoupledPairRecord {
                cell_a: u32::try_from(source).expect("validated CellVisibility CellId"),
                cell_b: u32::try_from(target).expect("validated CellVisibility CellId"),
                distance,
                aperture,
            });
        }
    }

    pairs
}

struct PortalCentroidGraph {
    adjacency: Vec<Vec<WeightedEdge>>,
    has_endpoint: Vec<bool>,
    portal_count: usize,
}

impl PortalCentroidGraph {
    fn cell_node(&self, cell: usize) -> usize {
        self.portal_count + cell
    }
}

/// Build the distance graph with portals as nodes. A valid cell adds one
/// endpoint node joined to each of its portals; portal pairs sharing a cell are
/// joined directly by their centroid separation. Invalid AABBs add no endpoint
/// node, so faceless leaves remain in the reachability component but never
/// receive graded entries.
fn portal_centroid_graph(tree: &BspTree, portals: &[PortalEdge]) -> PortalCentroidGraph {
    let portal_count = portals
        .iter()
        .map(|portal| portal.portal_index)
        .max()
        .map_or(0, |last| last + 1);
    let cell_count = tree.leaves.len();
    let mut adjacency = vec![Vec::new(); portal_count + cell_count];
    let mut incident_portals = vec![Vec::new(); cell_count];
    let mut portal_centroids = vec![None; portal_count];

    for portal in portals {
        portal_centroids[portal.portal_index] = portal.centroid;
        if portal.centroid.is_some() {
            incident_portals[portal.front].push(portal.portal_index);
            incident_portals[portal.back].push(portal.portal_index);
        }
    }

    let mut has_endpoint = vec![false; cell_count];
    for (cell, leaf) in tree.leaves.iter().enumerate() {
        let incident = &incident_portals[cell];

        // A faceless leaf has no cell-center endpoint, but its portals still
        // share the ordinary portal-to-portal edge. It can therefore carry a
        // metric path between valid neighbours without receiving a graded pair
        // of its own.
        for (offset, &first_portal) in incident.iter().enumerate() {
            let first_centroid =
                portal_centroids[first_portal].expect("incident portals have a centroid");
            for &second_portal in &incident[offset + 1..] {
                let second_centroid =
                    portal_centroids[second_portal].expect("incident portals have a centroid");
                add_weighted_edge(
                    &mut adjacency,
                    first_portal,
                    second_portal,
                    first_centroid.distance(second_centroid),
                );
            }
        }

        // Guard before `centroid()`: Aabb::empty() carries +/- infinity and
        // would otherwise introduce NaN edge weights into Dijkstra.
        if !leaf.bounds.is_valid() {
            continue;
        }
        has_endpoint[cell] = true;
        let cell_centroid = leaf.bounds.centroid();
        let cell_node = portal_count + cell;

        for &portal_index in incident {
            let portal_centroid =
                portal_centroids[portal_index].expect("incident portals have a centroid");
            add_weighted_edge(
                &mut adjacency,
                cell_node,
                portal_index,
                cell_centroid.distance(portal_centroid),
            );
        }
    }

    PortalCentroidGraph {
        adjacency,
        has_endpoint,
        portal_count,
    }
}

fn add_weighted_edge(adjacency: &mut [Vec<WeightedEdge>], first: usize, second: usize, cost: f64) {
    debug_assert!(cost.is_finite() && cost >= 0.0);
    adjacency[first].push(WeightedEdge {
        node_id: second,
        cost,
    });
    adjacency[second].push(WeightedEdge {
        node_id: first,
        cost,
    });
}

#[derive(Clone, Copy)]
struct WeightedEdge {
    node_id: usize,
    cost: f64,
}

#[derive(Clone, Copy)]
struct FrontierEntry {
    cost: f64,
    node_id: usize,
}

impl PartialEq for FrontierEntry {
    fn eq(&self, other: &Self) -> bool {
        self.cost.to_bits() == other.cost.to_bits() && self.node_id == other.node_id
    }
}

impl Eq for FrontierEntry {}

impl PartialOrd for FrontierEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FrontierEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap. Reverse cost so the smallest distance wins,
        // then reverse node ID so equal-cost pops are a genuine total order.
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.node_id.cmp(&self.node_id))
    }
}

fn dijkstra(adjacency: &[Vec<WeightedEdge>], source: usize) -> Vec<f64> {
    let mut distances = vec![f64::INFINITY; adjacency.len()];
    let mut frontier = BinaryHeap::new();
    distances[source] = 0.0;
    frontier.push(FrontierEntry {
        cost: 0.0,
        node_id: source,
    });

    while let Some(entry) = frontier.pop() {
        if entry.cost.total_cmp(&distances[entry.node_id]) == Ordering::Greater {
            continue;
        }
        for edge in &adjacency[entry.node_id] {
            let candidate = entry.cost + edge.cost;
            if candidate < distances[edge.node_id] {
                distances[edge.node_id] = candidate;
                frontier.push(FrontierEntry {
                    cost: candidate,
                    node_id: edge.node_id,
                });
            }
        }
    }

    distances
}

#[derive(Clone, Copy)]
struct ApertureTreeEdge {
    cell: usize,
    aperture: f64,
}

/// Kruskal's algorithm produces a maximum spanning forest. Portal index breaks
/// aperture ties, making the selected tree stable even when several equal-width
/// portals could connect the same components.
fn maximum_spanning_tree(cell_count: usize, portals: &[PortalEdge]) -> Vec<Vec<ApertureTreeEdge>> {
    let mut ranked = portals.to_vec();
    ranked.sort_by(|first, second| {
        second
            .aperture
            .total_cmp(&first.aperture)
            .then_with(|| first.portal_index.cmp(&second.portal_index))
    });

    let mut union_find = UnionFind::new(cell_count);
    let mut tree = vec![Vec::new(); cell_count];
    for portal in ranked {
        if !union_find.union(portal.front, portal.back) {
            continue;
        }
        tree[portal.front].push(ApertureTreeEdge {
            cell: portal.back,
            aperture: portal.aperture,
        });
        tree[portal.back].push(ApertureTreeEdge {
            cell: portal.front,
            aperture: portal.aperture,
        });
    }
    tree
}

fn widest_paths_from(tree: &[Vec<ApertureTreeEdge>], source: usize) -> Vec<Option<f64>> {
    let mut apertures = vec![None; tree.len()];
    apertures[source] = Some(f64::INFINITY);
    let mut queue = VecDeque::from([source]);

    while let Some(cell) = queue.pop_front() {
        let aperture = apertures[cell].expect("queued aperture-tree cell has a bottleneck");
        for edge in &tree[cell] {
            if apertures[edge.cell].is_none() {
                apertures[edge.cell] = Some(aperture.min(edge.aperture));
                queue.push_back(edge.cell);
            }
        }
    }

    apertures
}

struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl UnionFind {
    fn new(size: usize) -> Self {
        Self {
            parent: (0..size).collect(),
            rank: vec![0; size],
        }
    }

    fn find(&mut self, cell: usize) -> usize {
        if self.parent[cell] != cell {
            let root = self.find(self.parent[cell]);
            self.parent[cell] = root;
        }
        self.parent[cell]
    }

    fn union(&mut self, first: usize, second: usize) -> bool {
        let first_root = self.find(first);
        let second_root = self.find(second);
        if first_root == second_root {
            return false;
        }

        let (parent, child) = match self.rank[first_root].cmp(&self.rank[second_root]) {
            Ordering::Less => (second_root, first_root),
            Ordering::Greater => (first_root, second_root),
            Ordering::Equal => {
                if first_root < second_root {
                    self.rank[first_root] += 1;
                    (first_root, second_root)
                } else {
                    self.rank[second_root] += 1;
                    (second_root, first_root)
                }
            }
        };
        self.parent[child] = parent;
        true
    }
}

#[derive(Clone, Copy)]
struct PortalMetrics {
    centroid: Option<DVec3>,
    minimum_width: f64,
}

/// Aperture is the polygon's minimum in-plane width. It is a deterministic
/// opening-capacity key for the widest-path calculation, not a solid-angle or
/// physical acoustics model; the fixed-point aperture scale therefore remains
/// in world-metre units.
fn portal_metrics(vertices: &[DVec3]) -> PortalMetrics {
    if vertices.len() < 3 || vertices.iter().any(|vertex| !vertex.is_finite()) {
        return PortalMetrics {
            centroid: None,
            minimum_width: 0.0,
        };
    }

    let first = vertices[0];
    let mut normal = DVec3::ZERO;
    let mut weighted_centroid = DVec3::ZERO;
    let mut total_area = 0.0;
    for index in 1..vertices.len() - 1 {
        let second = vertices[index];
        let third = vertices[index + 1];
        let cross = (second - first).cross(third - first);
        let area = cross.length() * 0.5;
        normal += cross;
        weighted_centroid += (first + second + third) * (area / 3.0);
        total_area += area;
    }
    if !total_area.is_finite() || total_area <= 0.0 || normal.length_squared() <= 0.0 {
        return PortalMetrics {
            centroid: None,
            minimum_width: 0.0,
        };
    }

    let centroid = weighted_centroid / total_area;
    let normal = normal.normalize();
    let mut minimum_width = f64::INFINITY;
    for index in 0..vertices.len() {
        let edge = vertices[(index + 1) % vertices.len()] - vertices[index];
        if edge.length_squared() <= 0.0 {
            continue;
        }
        let in_plane_normal = normal.cross(edge).normalize();
        let (minimum, maximum) = vertices.iter().fold(
            (f64::INFINITY, f64::NEG_INFINITY),
            |(minimum, maximum), vertex| {
                let projection = vertex.dot(in_plane_normal);
                (minimum.min(projection), maximum.max(projection))
            },
        );
        minimum_width = minimum_width.min(maximum - minimum);
    }

    PortalMetrics {
        centroid: centroid.is_finite().then_some(centroid),
        minimum_width: if minimum_width.is_finite() {
            minimum_width
        } else {
            0.0
        },
    }
}

#[derive(Clone, Copy)]
enum CouplingAxis {
    Distance,
    Aperture,
}

impl CouplingAxis {
    fn label(self) -> &'static str {
        match self {
            Self::Distance => "distance",
            Self::Aperture => "aperture",
        }
    }
}

fn fixed_point_value(value: f64, scale: u32, axis: CouplingAxis) -> u32 {
    let scaled = value * f64::from(scale);
    if !scaled.is_finite() || scaled > f64::from(u32::MAX) {
        log::warn!(
            "[Compiler] CellVisibility {} exceeds the u32 fixed-point range; clamping to u32::MAX",
            axis.label()
        );
        return u32::MAX;
    }
    debug_assert!(
        (0.0..=f64::from(u32::MAX)).contains(&scaled),
        "CellVisibility metric must fit the u32 fixed-point range"
    );
    if scaled <= 0.0 {
        return 0;
    }
    scaled.round() as u32
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use log::Level;
    use postretro_test_log_capture::LogCapture;

    use super::*;
    use crate::partition::{Aabb, BspLeaf, BspTree};

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

    fn cap_boundary_fixture() -> (BspTree, Vec<Portal>) {
        let tree = tree(
            &[
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(2.0, 0.0, 0.0),
                DVec3::new(4.000_976_562_5, 0.0, 0.0),
                DVec3::new(10.0, 0.0, 0.0),
            ],
            &[false, false, false, false],
        );
        let portals = vec![
            square_portal(0, 1, DVec3::new(1.0, 0.0, 0.0), 2.0),
            square_portal(1, 2, DVec3::new(3.000_488_281_25, 0.0, 0.0), 2.0),
        ];
        (tree, portals)
    }

    #[test]
    fn coupled_pairs_are_symmetric_in_query_order_and_obey_cap_boundary() {
        let (tree, portals) = cap_boundary_fixture();
        let cap = 2 * CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;
        let section = bake_with_coupling_cap(&tree, &portals, cap).unwrap();

        assert_eq!(section.component_ids, vec![0, 0, 0, 1]);
        assert_eq!(
            section.coupled_pairs,
            vec![CoupledPairRecord {
                cell_a: 0,
                cell_b: 1,
                distance: cap,
                aperture: 2 * CELL_VISIBILITY_APERTURE_FIXED_POINT_SCALE,
            }]
        );

        assert_eq!(coupled_pair(&section, 0, 1), coupled_pair(&section, 1, 0));
        assert!(
            coupled_pair(&section, 0, 0).is_none(),
            "diagonal is never graded"
        );
        assert!(
            coupled_pair(&section, 1, 2).is_none(),
            "distance == cap + 1 must omit both axes"
        );
        assert!(
            coupled_pair(&section, 0, 3).is_none(),
            "non-perceivable cells are never graded"
        );
    }

    fn coupled_pair(
        section: &CellVisibilitySection,
        first: u32,
        second: u32,
    ) -> Option<CoupledPairRecord> {
        let (cell_a, cell_b) = (first.min(second), first.max(second));
        section
            .coupled_pairs
            .iter()
            .copied()
            .find(|pair| pair.cell_a == cell_a && pair.cell_b == cell_b)
    }

    fn oracle_fixture() -> (BspTree, Vec<Portal>, Vec<DVec3>, Vec<(usize, usize, f64)>) {
        let centres = vec![
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(10.0, 0.0, 0.0),
            DVec3::new(20.0, 0.0, 0.0),
            DVec3::new(10.0, 10.0, 0.0),
        ];
        let portal_specs = vec![
            (0, 1, DVec3::new(5.0, 0.0, 0.0), 4.0),
            (1, 2, DVec3::new(15.0, 0.0, 0.0), 4.0),
            (0, 3, DVec3::new(0.0, 5.0, 0.0), 9.0),
            (3, 2, DVec3::new(15.0, 10.0, 0.0), 9.0),
        ];
        let portals = portal_specs
            .iter()
            .map(|&(front, back, centre, width)| square_portal(front, back, centre, width))
            .collect();
        let aperture_edges = portal_specs
            .iter()
            .map(|&(front, back, _, width)| (front, back, width))
            .collect();
        (
            tree(&centres, &[false; 4]),
            portals,
            centres,
            aperture_edges,
        )
    }

    #[test]
    fn graded_axes_match_independent_shortest_and_bottleneck_oracles() {
        let (tree, portals, centres, aperture_edges) = oracle_fixture();
        let section = cell_visibility_bake(&tree, &portals).unwrap();
        let portal_centres: Vec<_> = portals
            .iter()
            .map(|portal| portal_metrics(&portal.polygon).centroid.unwrap())
            .collect();

        for source in 0..centres.len() {
            for target in source + 1..centres.len() {
                let pair = coupled_pair(&section, source as u32, target as u32)
                    .expect("fixture pairs stay below the shipped coupling cap");
                assert_eq!(
                    pair.distance,
                    (independent_shortest_path(&centres, &portals, &portal_centres, source, target)
                        * f64::from(CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE))
                    .round() as u32,
                    "distance mismatch for {source} -> {target}"
                );
                assert_eq!(
                    pair.aperture,
                    (independent_widest_path(&aperture_edges, centres.len(), source, target)
                        * f64::from(CELL_VISIBILITY_APERTURE_FIXED_POINT_SCALE))
                    .round() as u32,
                    "aperture mismatch for {source} -> {target}"
                );
            }
        }

        let baseline = coupled_pair(&section, 0, 2).unwrap().aperture;
        let mut narrowed_portals = portals;
        narrowed_portals[2] = square_portal(0, 3, DVec3::new(0.0, 5.0, 0.0), 1.0);
        narrowed_portals[3] = square_portal(3, 2, DVec3::new(15.0, 10.0, 0.0), 1.0);
        let narrowed = cell_visibility_bake(&tree, &narrowed_portals).unwrap();
        assert!(
            coupled_pair(&narrowed, 0, 2).unwrap().aperture <= baseline,
            "narrowing the sole widest path must not raise its bottleneck"
        );
    }

    /// A deliberately simple O(V^2) Dijkstra oracle, independent from the
    /// production heap traversal, over the same pinned portal-centroid metric.
    fn independent_shortest_path(
        centres: &[DVec3],
        portals: &[Portal],
        portal_centres: &[DVec3],
        source: usize,
        target: usize,
    ) -> f64 {
        let portal_count = portals.len();
        let node_count = portal_count + centres.len();
        let mut weights = vec![vec![f64::INFINITY; node_count]; node_count];
        for node in 0..node_count {
            weights[node][node] = 0.0;
        }
        for (portal_index, portal) in portals.iter().enumerate() {
            for &cell in &[portal.front_leaf, portal.back_leaf] {
                let cell_node = portal_count + cell;
                let distance = centres[cell].distance(portal_centres[portal_index]);
                weights[cell_node][portal_index] = distance;
                weights[portal_index][cell_node] = distance;
            }
        }
        for cell in 0..centres.len() {
            let incident: Vec<_> = portals
                .iter()
                .enumerate()
                .filter_map(|(portal_index, portal)| {
                    (portal.front_leaf == cell || portal.back_leaf == cell).then_some(portal_index)
                })
                .collect();
            for (offset, &first) in incident.iter().enumerate() {
                for &second in &incident[offset + 1..] {
                    let distance = portal_centres[first].distance(portal_centres[second]);
                    weights[first][second] = distance;
                    weights[second][first] = distance;
                }
            }
        }

        let mut distances = vec![f64::INFINITY; node_count];
        let mut visited = vec![false; node_count];
        distances[portal_count + source] = 0.0;
        for _ in 0..node_count {
            let Some(node) = (0..node_count)
                .filter(|&node| !visited[node])
                .min_by(|&first, &second| distances[first].total_cmp(&distances[second]))
            else {
                break;
            };
            if !distances[node].is_finite() {
                break;
            }
            visited[node] = true;
            for neighbor in 0..node_count {
                let candidate = distances[node] + weights[node][neighbor];
                if candidate < distances[neighbor] {
                    distances[neighbor] = candidate;
                }
            }
        }
        distances[portal_count + target]
    }

    /// A cell-graph maximin relaxation oracle, intentionally distinct from the
    /// production maximum-spanning-tree traversal.
    fn independent_widest_path(
        edges: &[(usize, usize, f64)],
        cell_count: usize,
        source: usize,
        target: usize,
    ) -> f64 {
        let mut best = vec![f64::NEG_INFINITY; cell_count];
        let mut visited = vec![false; cell_count];
        best[source] = f64::INFINITY;
        for _ in 0..cell_count {
            let Some(cell) = (0..cell_count)
                .filter(|&cell| !visited[cell])
                .max_by(|&first, &second| best[first].total_cmp(&best[second]))
            else {
                break;
            };
            if !best[cell].is_finite() && cell != source {
                break;
            }
            visited[cell] = true;
            for &(first, second, aperture) in edges {
                let neighbor = if first == cell {
                    Some(second)
                } else if second == cell {
                    Some(first)
                } else {
                    None
                };
                if let Some(neighbor) = neighbor {
                    best[neighbor] = best[neighbor].max(best[cell].min(aperture));
                }
            }
        }
        best[target]
    }

    #[test]
    fn faceless_cells_stay_perceivable_without_graded_entries() {
        let mut tree = tree(
            &[
                DVec3::ZERO,
                DVec3::new(2.0, 0.0, 0.0),
                DVec3::new(4.0, 0.0, 0.0),
            ],
            &[false, false, false],
        );
        tree.leaves[1].bounds = Aabb::empty();
        let portals = vec![
            square_portal(0, 1, DVec3::X, 1.0),
            square_portal(1, 2, DVec3::new(3.0, 0.0, 0.0), 1.0),
        ];

        let section = cell_visibility_bake(&tree, &portals).unwrap();
        assert_eq!(section.component_ids, vec![0, 0, 0]);
        assert!(coupled_pair(&section, 0, 1).is_none());
        assert!(coupled_pair(&section, 1, 2).is_none());
        assert!(
            coupled_pair(&section, 0, 2).is_some(),
            "faceless cells still carry portal-to-portal distance edges"
        );
    }

    #[test]
    fn fixed_point_overflow_clamps_and_logs_a_warning() {
        let capture = LogCapture::start();
        let overflow =
            f64::from(u32::MAX) / f64::from(CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE) + 1.0;
        assert_eq!(
            fixed_point_value(
                overflow,
                CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE,
                CouplingAxis::Distance,
            ),
            u32::MAX
        );
        capture.assert_logged_once(
            Level::Warn,
            "[Compiler] CellVisibility distance exceeds the u32 fixed-point range",
        );
    }

    #[test]
    fn fixture_graded_section_is_byte_identical_across_two_compiles() {
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
