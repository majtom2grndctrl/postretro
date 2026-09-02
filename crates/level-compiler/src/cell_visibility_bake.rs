// Static CellVisibility bake over the compiler portal graph.
// See: context/lib/build_pipeline.md §PRL section IDs

use std::{
    cmp::Ordering,
    collections::{BinaryHeap, VecDeque},
    num::NonZeroUsize,
};

use glam::DVec3;
use postretro_level_format::cell_visibility::{
    CELL_VISIBILITY_APERTURE_FIXED_POINT_SCALE, CELL_VISIBILITY_DISTANCE_CAP,
    CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE, CELL_VISIBILITY_FANOUT_K, CellVisibilitySection,
    CoupledPairRecord,
};
use rayon::{ThreadPoolBuilder, prelude::*};

use crate::{
    bake_control::BakeControl,
    cache::{CacheKey, StageCache},
    partition::BspTree,
    portals::Portal,
};

mod metrics;

use metrics::{fixed_point_value, portal_metrics};

/// Cache stage for the whole CellVisibility section.
pub const CELL_VISIBILITY_STAGE_ID: &str = "cell_visibility";
/// Bump when CellVisibility coupling, ranking, or wire payload semantics change.
pub const CELL_VISIBILITY_STAGE_VERSION: u32 = 1;

/// Bake the conservative reachability gate and its bounded graded horizon.
pub fn cell_visibility_bake(
    tree: &BspTree,
    portals: &[Portal],
    control: &BakeControl,
) -> anyhow::Result<CellVisibilitySection> {
    let (cell_count, cell_count_u32, progress_units) = cell_visibility_counts(tree)?;
    control.publish_total(progress_units);

    let portal_edges = portal_edges(tree, portals);
    let component_ids = component_ids(cell_count, &portal_edges, control);
    let coupled_pairs = assemble_coupled_pairs(
        tree,
        &portal_edges,
        &component_ids,
        CELL_VISIBILITY_DISTANCE_CAP,
        CELL_VISIBILITY_FANOUT_K,
        control,
    )?;

    Ok(CellVisibilitySection {
        cell_count: cell_count_u32,
        component_ids,
        coupled_pairs,
    })
}

/// Bake or load the CellVisibility payload. The wrapper returns encoded bytes so
/// the pipeline keeps the existing fallible `to_bytes()` contract on both paths.
/// With no cache it delegates directly to the original bake and performs no
/// cache I/O.
pub fn cell_visibility_bake_cached(
    tree: &BspTree,
    portals: &[Portal],
    cache: Option<&StageCache>,
    control: &BakeControl,
) -> anyhow::Result<Vec<u8>> {
    let Some(cache) = cache else {
        return Ok(cell_visibility_bake(tree, portals, control)?.to_bytes()?);
    };

    // Validate exactly as the uncached bake does before a cache hit can bypass
    // its setup. This also supplies the expected wire cell count for decoding.
    let (_, cell_count_u32, progress_units) = cell_visibility_counts(tree)?;
    let key = cell_visibility_cache_key(tree, portals, CELL_VISIBILITY_STAGE_VERSION);

    if let Some(data) = cache.get(&key) {
        match CellVisibilitySection::from_bytes(&data, cell_count_u32) {
            Ok(section) => {
                log::info!("[cache] cell_visibility hit");
                // The worker is skipped on a hit, so reproduce its complete
                // `cell_count * 2` accounting on the orchestrator thread.
                control.publish_total(progress_units);
                control.governor().checkpoint();
                control.advance(progress_units);
                return Ok(section.to_bytes()?);
            }
            Err(error) => {
                log::warn!("[cache] corrupt cell_visibility entry, re-baking: {error}");
                log::info!("[cache] cell_visibility miss");
            }
        }
    } else {
        log::info!("[cache] cell_visibility miss");
    }

    let bytes = cell_visibility_bake(tree, portals, control)?.to_bytes()?;
    cache.put(&key, &bytes);
    Ok(bytes)
}

/// Derive the CellVisibility whole-section key from every structural value the
/// bake reads. Lights intentionally do not participate: they never influence
/// portal components or coupled-pair grading.
pub(crate) fn cell_visibility_cache_key(
    tree: &BspTree,
    portals: &[Portal],
    stage_version: u32,
) -> CacheKey {
    let mut hasher = blake3::Hasher::new();

    // The bake assigns CellIds from leaf position and reads each leaf's bounds
    // for hub distances and solidity for portal admission.
    hasher.update(
        &u64::try_from(tree.leaves.len())
            .expect("leaf count fits u64")
            .to_le_bytes(),
    );
    for leaf in &tree.leaves {
        for coordinate in [
            leaf.bounds.min.x,
            leaf.bounds.min.y,
            leaf.bounds.min.z,
            leaf.bounds.max.x,
            leaf.bounds.max.y,
            leaf.bounds.max.z,
        ] {
            hasher.update(&coordinate.to_le_bytes());
        }
        hasher.update(&[u8::from(leaf.is_solid)]);
    }

    // Input order is significant: equal-aperture maximum-spanning-tree ties
    // resolve by this portal's original input index. The bake reduces each
    // polygon to its metrics, so fold those exact values rather than a render
    // geometry hash or unrelated portal representation.
    hasher.update(
        &u64::try_from(portals.len())
            .expect("portal count fits u64")
            .to_le_bytes(),
    );
    for portal in portals {
        hasher.update(
            &u64::try_from(portal.front_leaf)
                .expect("portal front leaf index fits u64")
                .to_le_bytes(),
        );
        hasher.update(
            &u64::try_from(portal.back_leaf)
                .expect("portal back leaf index fits u64")
                .to_le_bytes(),
        );

        let metrics = portal_metrics(&portal.polygon);
        match metrics.centroid {
            Some(centroid) => {
                hasher.update(&[1]);
                for coordinate in [centroid.x, centroid.y, centroid.z] {
                    hasher.update(&coordinate.to_le_bytes());
                }
            }
            None => {
                hasher.update(&[0]);
            }
        }
        hasher.update(&metrics.minimum_width.to_le_bytes());
    }

    hasher.update(&CELL_VISIBILITY_DISTANCE_CAP.to_le_bytes());
    hasher.update(
        &u64::try_from(CELL_VISIBILITY_FANOUT_K)
            .expect("CellVisibility fanout fits u64")
            .to_le_bytes(),
    );

    CacheKey::new(
        CELL_VISIBILITY_STAGE_ID,
        stage_version,
        hasher.finalize().as_bytes(),
    )
}

fn cell_visibility_counts(tree: &BspTree) -> anyhow::Result<(usize, u32, usize)> {
    let cell_count = tree.leaves.len();
    anyhow::ensure!(
        cell_count != 0,
        "CellVisibility requires at least one BSP leaf"
    );
    let cell_count_u32 = u32::try_from(cell_count)
        .map_err(|_| anyhow::anyhow!("CellVisibility cannot encode more than u32::MAX cells"))?;
    let progress_units = cell_count
        .checked_mul(2)
        .ok_or_else(|| anyhow::anyhow!("CellVisibility progress unit count overflow"))?;
    Ok((cell_count, cell_count_u32, progress_units))
}

#[derive(Clone, Copy)]
struct PortalEdge {
    /// Stable input order used to break equal-aperture MST ties.
    portal_index: usize,
    front: usize,
    back: usize,
    centroid: Option<DVec3>,
    aperture: f64,
}

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

/// Complete, conservative components include every leaf. The ascending outer
/// walk gives dense component IDs ordered by their lowest CellId.
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

/// Build source-directed top-K rows inside an owned bounded Rayon pool, then
/// sort-and-collapse them to canonical unordered records. `cap` and `fanout`
/// are parameters only for focused omission-boundary tests.
fn assemble_coupled_pairs(
    tree: &BspTree,
    portals: &[PortalEdge],
    component_ids: &[u32],
    cap: u32,
    fanout: usize,
    control: &BakeControl,
) -> anyhow::Result<Vec<CoupledPairRecord>> {
    let graph = portal_hub_graph(tree, portals);
    let aperture_tree = maximum_spanning_tree(tree.leaves.len(), portals);
    let worker_count = std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1);
    let pool = ThreadPoolBuilder::new()
        .num_threads(worker_count)
        .build()
        .map_err(|error| {
            anyhow::anyhow!("CellVisibility failed to build bounded Rayon pool: {error}")
        })?;

    // The owned pool prevents RAYON_NUM_THREADS, build_global, or a caller's
    // ambient pool from widening P beyond available_parallelism. Sources never
    // wait on each other: each takes exactly one Governor permit, performs no
    // nested enter/par_iter, and reduces immediately to at most K candidates.
    let rows = pool.install(|| {
        (0..tree.leaves.len())
            .into_par_iter()
            .map(|source| {
                let _permit = control.governor().enter();
                let row = source_top_k(source, &graph, &aperture_tree, component_ids, cap, fanout);
                control.advance(1);
                row
            })
            .collect::<Vec<_>>()
    });
    let clamped_distances = rows.iter().any(|row| row.clamped_distance);
    let clamped_apertures = rows.iter().any(|row| row.clamped_aperture);
    if clamped_distances || clamped_apertures {
        log::warn!(
            "[Compiler] CellVisibility fixed-point value(s) clamped to u32::MAX (distance: {clamped_distances}, aperture: {clamped_apertures})"
        );
    }
    collapse_directed_candidates(rows)
}

#[derive(Clone, Copy, Debug)]
struct DirectedCandidate {
    source: u32,
    target: u32,
    distance: u32,
    aperture: u32,
}

#[derive(Default)]
struct SourceCandidates {
    candidates: Vec<DirectedCandidate>,
    clamped_distance: bool,
    clamped_aperture: bool,
}

/// The resolved fixed-point distance is intentionally the single source for
/// rank, cap comparison, and stored value (not the pre-round float).
fn source_top_k(
    source: usize,
    graph: &PortalHubGraph,
    aperture_tree: &[Vec<ApertureTreeEdge>],
    component_ids: &[u32],
    cap: u32,
    fanout: usize,
) -> SourceCandidates {
    let Some(source_node) = graph.hub_node[source] else {
        // Keep rows aligned to true CellIds; faceless/solid/isolated sources
        // have an empty top-K row rather than compacting the source list.
        return SourceCandidates::default();
    };
    if fanout == 0 {
        return SourceCandidates::default();
    }
    let distances = dijkstra(&graph.adjacency, source_node);
    let apertures = widest_paths_from(aperture_tree, source);
    let source_id = u32::try_from(source).expect("validated CellVisibility CellId");
    let mut candidates = Vec::with_capacity(fanout.min(64));
    let mut clamped_distance = false;
    let mut clamped_aperture = false;

    for target in 0..component_ids.len() {
        if source == target || component_ids[source] != component_ids[target] {
            continue;
        }
        let Some(target_node) = graph.hub_node[target] else {
            continue;
        };
        let distance = distances[target_node];
        if !distance.is_finite() {
            // A faceless node deliberately severs metric paths. The component
            // gate stays conservative and simply reports no graded detail.
            continue;
        }
        let Some(aperture) = apertures[target] else {
            debug_assert!(false, "metric-connected cells need an aperture path");
            continue;
        };
        let (distance, did_clamp_distance) =
            fixed_point_value(distance, CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE);
        clamped_distance |= did_clamp_distance;
        if distance > cap {
            continue;
        }
        let (aperture, did_clamp_aperture) =
            fixed_point_value(aperture, CELL_VISIBILITY_APERTURE_FIXED_POINT_SCALE);
        clamped_aperture |= did_clamp_aperture;
        insert_top_k(
            &mut candidates,
            DirectedCandidate {
                source: source_id,
                target: u32::try_from(target).expect("validated CellVisibility CellId"),
                distance,
                aperture,
            },
            fanout,
        );
    }
    SourceCandidates {
        candidates,
        clamped_distance,
        clamped_aperture,
    }
}

fn insert_top_k(kept: &mut Vec<DirectedCandidate>, candidate: DirectedCandidate, fanout: usize) {
    kept.push(candidate);
    kept.sort_unstable_by(|first, second| {
        first
            .distance
            .cmp(&second.distance)
            .then_with(|| first.target.cmp(&second.target))
    });
    if kept.len() > fanout {
        kept.pop();
    }
}

/// A mutually kept pair occurs twice. Sorting by canonical endpoints and then
/// source makes min-to-max the deterministic retained direction.
fn collapse_directed_candidates(
    rows: Vec<SourceCandidates>,
) -> anyhow::Result<Vec<CoupledPairRecord>> {
    let directed_count = rows.iter().try_fold(0usize, |total, row| {
        total
            .checked_add(row.candidates.len())
            .ok_or_else(|| anyhow::anyhow!("CellVisibility directed pair count overflow"))
    })?;
    u32::try_from(directed_count).map_err(|_| {
        anyhow::anyhow!(
            "CellVisibility directed pair count {directed_count} exceeds the u32 wire limit"
        )
    })?;
    let mut directed = Vec::new();
    directed
        .try_reserve_exact(directed_count)
        .map_err(|error| {
            anyhow::anyhow!(
                "CellVisibility cannot reserve {directed_count} directed candidates: {error}"
            )
        })?;
    for row in rows {
        directed.extend(row.candidates);
    }
    directed.sort_unstable_by(|first, second| {
        (
            first.source.min(first.target),
            first.source.max(first.target),
        )
            .cmp(&(
                second.source.min(second.target),
                second.source.max(second.target),
            ))
            .then_with(|| first.source.cmp(&second.source))
    });

    let mut pairs = Vec::new();
    pairs.try_reserve_exact(directed_count).map_err(|error| {
        anyhow::anyhow!(
            "CellVisibility cannot reserve {directed_count} coupled pair records: {error}"
        )
    })?;
    let mut cursor = 0;
    while cursor < directed.len() {
        let candidate = directed[cursor];
        let cell_a = candidate.source.min(candidate.target);
        let cell_b = candidate.source.max(candidate.target);
        pairs.push(CoupledPairRecord {
            cell_a,
            cell_b,
            distance: candidate.distance,
            aperture: candidate.aperture,
        });
        cursor += 1;
        while cursor < directed.len()
            && (
                directed[cursor].source.min(directed[cursor].target),
                directed[cursor].source.max(directed[cursor].target),
            ) == (cell_a, cell_b)
        {
            cursor += 1;
        }
    }
    u32::try_from(pairs.len()).map_err(|_| {
        anyhow::anyhow!(
            "CellVisibility pair count {} exceeds the u32 wire limit",
            pairs.len()
        )
    })?;
    Ok(pairs)
}

struct PortalHubGraph {
    adjacency: Vec<Vec<WeightedEdge>>,
    /// Complete CellId -> compact graph-hub map. `None` preserves source ID
    /// alignment for solid, faceless, and zero-portal cells.
    hub_node: Vec<Option<usize>>,
}

/// Linear hub metric: valid portal centroids are nodes and each valid-bounds
/// portal-holding cell has one hub. The only edges are hub <-> incident portal;
/// in-cell travel through the cell center is the intentionally chosen distance
/// key, not an approximation of a direct portal-centroid chord.
fn portal_hub_graph(tree: &BspTree, portals: &[PortalEdge]) -> PortalHubGraph {
    let portal_count = portals.len();
    let mut incident_portals = vec![Vec::new(); tree.leaves.len()];
    for (portal_node, portal) in portals.iter().enumerate() {
        if portal.centroid.is_some() {
            incident_portals[portal.front].push(portal_node);
            incident_portals[portal.back].push(portal_node);
        }
    }

    let mut hub_node = vec![None; tree.leaves.len()];
    let mut hub_count = 0;
    for (cell, leaf) in tree.leaves.iter().enumerate() {
        // Check before centroid(): invalid faceless bounds use infinities.
        if !leaf.is_solid && leaf.bounds.is_valid() && !incident_portals[cell].is_empty() {
            hub_node[cell] = Some(portal_count + hub_count);
            hub_count += 1;
        }
    }
    let mut adjacency = vec![Vec::new(); portal_count + hub_count];
    for (portal_node, portal) in portals.iter().enumerate() {
        let Some(portal_centroid) = portal.centroid else {
            continue;
        };
        for &cell in &[portal.front, portal.back] {
            let Some(hub) = hub_node[cell] else {
                continue;
            };
            add_weighted_edge(
                &mut adjacency,
                hub,
                portal_node,
                tree.leaves[cell]
                    .bounds
                    .centroid()
                    .distance(portal_centroid),
            );
        }
    }
    PortalHubGraph {
        adjacency,
        hub_node,
    }
}

#[derive(Clone, Copy)]
struct WeightedEdge {
    node_id: usize,
    cost: f64,
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
        // BinaryHeap is a max heap; reversed total (cost, node_id) gives the
        // deterministic lowest-cost first Dijkstra frontier.
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

/// The aperture metric is a portal polygon's minimum in-plane width (metres):
/// a deterministic opening-capacity coupling key, not solid angle/acoustics.
fn maximum_spanning_tree(cell_count: usize, portals: &[PortalEdge]) -> Vec<Vec<ApertureTreeEdge>> {
    let mut ranked = portals.to_vec();
    ranked.sort_unstable_by(|first, second| {
        second
            .aperture
            .total_cmp(&first.aperture)
            .then_with(|| first.portal_index.cmp(&second.portal_index))
    });
    let mut union_find = UnionFind::new(cell_count);
    let mut tree = vec![Vec::new(); cell_count];
    for portal in ranked {
        if union_find.union(portal.front, portal.back) {
            tree[portal.front].push(ApertureTreeEdge {
                cell: portal.back,
                aperture: portal.aperture,
            });
            tree[portal.back].push(ApertureTreeEdge {
                cell: portal.front,
                aperture: portal.aperture,
            });
        }
    }
    tree
}

fn widest_paths_from(tree: &[Vec<ApertureTreeEdge>], source: usize) -> Vec<Option<f64>> {
    let mut apertures = vec![None; tree.len()];
    apertures[source] = Some(f64::INFINITY);
    let mut queue = VecDeque::from([source]);
    while let Some(cell) = queue.pop_front() {
        let aperture = apertures[cell].expect("queued aperture tree cell has a bottleneck");
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
            self.parent[cell] = self.find(self.parent[cell]);
        }
        self.parent[cell]
    }
    fn union(&mut self, first: usize, second: usize) -> bool {
        let first = self.find(first);
        let second = self.find(second);
        if first == second {
            return false;
        }
        let (parent, child) = match self.rank[first].cmp(&self.rank[second]) {
            Ordering::Less => (second, first),
            Ordering::Greater => (first, second),
            Ordering::Equal if first < second => {
                self.rank[first] += 1;
                (first, second)
            }
            Ordering::Equal => {
                self.rank[second] += 1;
                (second, first)
            }
        };
        self.parent[child] = parent;
        true
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        path::PathBuf,
        sync::Arc,
        thread,
        time::{Duration, Instant},
    };

    use glam::DVec3;
    use log::Level;
    use postretro_test_log_capture::LogCapture;

    use super::*;
    use crate::{
        cache::StageCache,
        governor::Governor,
        partition::{Aabb, BspLeaf, BspTree},
        reporter::StageProgress,
    };

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

    fn test_control(progress: &StageProgress, permits: usize) -> BakeControl {
        BakeControl::new(Arc::new(Governor::new(permits, false)), progress)
    }

    fn fresh_cache_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "postretro_cell_visibility_cache_{label}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn pair(section: &CellVisibilitySection, first: u32, second: u32) -> Option<CoupledPairRecord> {
        let (cell_a, cell_b) = (first.min(second), first.max(second));
        section
            .coupled_pairs
            .iter()
            .copied()
            .find(|pair| pair.cell_a == cell_a && pair.cell_b == cell_b)
    }

    #[test]
    fn cache_key_folds_structural_inputs_and_stage_version() {
        let bsp = tree(
            &[
                DVec3::ZERO,
                DVec3::new(2.0, 0.0, 0.0),
                DVec3::new(4.0, 0.0, 0.0),
            ],
            &[false, false, false],
        );
        let portals = vec![
            square_portal(0, 1, DVec3::X, 2.0),
            square_portal(1, 2, DVec3::new(3.0, 0.0, 0.0), 1.0),
        ];
        let baseline = cell_visibility_cache_key(&bsp, &portals, CELL_VISIBILITY_STAGE_VERSION);

        let changed_version =
            cell_visibility_cache_key(&bsp, &portals, CELL_VISIBILITY_STAGE_VERSION + 1);
        assert_ne!(baseline.as_filename(), changed_version.as_filename());

        let mut changed_bounds = tree(
            &[
                DVec3::ZERO,
                DVec3::new(2.0, 0.0, 0.0),
                DVec3::new(4.0, 0.0, 0.0),
            ],
            &[false, false, false],
        );
        changed_bounds.leaves[1].bounds.max.x += 0.25;
        assert_ne!(
            baseline.as_filename(),
            cell_visibility_cache_key(&changed_bounds, &portals, CELL_VISIBILITY_STAGE_VERSION)
                .as_filename(),
            "leaf bounds drive portal-hub distances and must invalidate the cache"
        );

        let changed_solidity = tree(
            &[
                DVec3::ZERO,
                DVec3::new(2.0, 0.0, 0.0),
                DVec3::new(4.0, 0.0, 0.0),
            ],
            &[false, true, false],
        );
        assert_ne!(
            baseline.as_filename(),
            cell_visibility_cache_key(&changed_solidity, &portals, CELL_VISIBILITY_STAGE_VERSION)
                .as_filename(),
            "leaf solidity gates portal admission and must invalidate the cache"
        );

        let changed_adjacency = vec![
            square_portal(0, 1, DVec3::X, 2.0),
            square_portal(0, 2, DVec3::new(3.0, 0.0, 0.0), 1.0),
        ];
        assert_ne!(
            baseline.as_filename(),
            cell_visibility_cache_key(&bsp, &changed_adjacency, CELL_VISIBILITY_STAGE_VERSION)
                .as_filename(),
            "portal leaf adjacency must invalidate the cache"
        );

        let changed_portal_metrics = vec![
            square_portal(0, 1, DVec3::X, 2.0),
            square_portal(1, 2, DVec3::new(3.25, 0.0, 0.0), 0.5),
        ];
        assert_ne!(
            baseline.as_filename(),
            cell_visibility_cache_key(&bsp, &changed_portal_metrics, CELL_VISIBILITY_STAGE_VERSION)
                .as_filename(),
            "portal centroid and aperture must invalidate the cache"
        );

        let reordered_portals = vec![
            square_portal(1, 2, DVec3::new(3.0, 0.0, 0.0), 2.0),
            square_portal(0, 1, DVec3::X, 2.0),
        ];
        assert_ne!(
            baseline.as_filename(),
            cell_visibility_cache_key(&bsp, &reordered_portals, CELL_VISIBILITY_STAGE_VERSION)
                .as_filename(),
            "portal order breaks maximum-spanning-tree ties and must invalidate the cache"
        );

        // The builder accepts only tree and portal inputs; lights cannot enter
        // this key, so a light-only edit reuses this unchanged structural key.
        assert_eq!(
            baseline.as_filename(),
            cell_visibility_cache_key(&bsp, &portals, CELL_VISIBILITY_STAGE_VERSION).as_filename()
        );
    }

    #[test]
    fn cached_bake_is_byte_identical_and_replays_progress() {
        let tree = tree(&[DVec3::ZERO, DVec3::new(2.0, 0.0, 0.0)], &[false, false]);
        let portals = vec![square_portal(0, 1, DVec3::X, 2.0)];
        let dir = fresh_cache_dir("round_trip");
        let cache = StageCache::new(&dir).expect("create cache dir");

        let cold_progress = StageProgress::indeterminate();
        let cold =
            cell_visibility_bake_cached(&tree, &portals, None, &test_control(&cold_progress, 1))
                .expect("uncached CellVisibility bake");

        let first_progress = StageProgress::indeterminate();
        let first = cell_visibility_bake_cached(
            &tree,
            &portals,
            Some(&cache),
            &test_control(&first_progress, 1),
        )
        .expect("cache-miss CellVisibility bake");

        let warm_progress = StageProgress::indeterminate();
        let warm = cell_visibility_bake_cached(
            &tree,
            &portals,
            Some(&cache),
            &test_control(&warm_progress, 1),
        )
        .expect("cache-hit CellVisibility bake");

        assert_eq!(cold, first, "cache miss must retain uncached bytes");
        assert_eq!(first, warm, "cache hit must retain baked bytes exactly");
        assert_eq!(first_progress.total(), Some(tree.leaves.len() * 2));
        assert_eq!(first_progress.completed(), tree.leaves.len() * 2);
        assert_eq!(warm_progress.total(), Some(tree.leaves.len() * 2));
        assert_eq!(warm_progress.completed(), tree.leaves.len() * 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_cached_section_is_a_soft_miss_and_rebakes() {
        let tree = tree(&[DVec3::ZERO, DVec3::new(2.0, 0.0, 0.0)], &[false, false]);
        let portals = vec![square_portal(0, 1, DVec3::X, 2.0)];
        let dir = fresh_cache_dir("corrupt");
        let cache = StageCache::new(&dir).expect("create cache dir");

        let reference = cell_visibility_bake_cached(
            &tree,
            &portals,
            Some(&cache),
            &BakeControl::unrestricted(),
        )
        .expect("initial CellVisibility bake");
        let key = cell_visibility_cache_key(&tree, &portals, CELL_VISIBILITY_STAGE_VERSION);
        // This remains a valid StageCache entry, but fails the CellVisibility
        // codec's expected-cell-count validation and must fall through to bake.
        cache.put(&key, b"not a CellVisibility section");

        let progress = StageProgress::indeterminate();
        let capture = LogCapture::start();
        let rebaked =
            cell_visibility_bake_cached(&tree, &portals, Some(&cache), &test_control(&progress, 1))
                .expect("malformed cached section must re-bake");

        assert_eq!(rebaked, reference);
        assert_eq!(progress.total(), Some(tree.leaves.len() * 2));
        assert_eq!(progress.completed(), tree.leaves.len() * 2);
        capture.assert_logged_once(Level::Warn, "[cache] corrupt cell_visibility entry");
        capture.assert_logged_once(Level::Info, "[cache] cell_visibility miss");

        let _ = std::fs::remove_dir_all(&dir);
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
        let section = cell_visibility_bake(&tree, &portals, &test_control(&progress, 1)).unwrap();
        assert_eq!(section.component_ids, vec![0, 0, 1, 2, 2]);
        for start in 0..tree.leaves.len() {
            let mut reachable = vec![false; tree.leaves.len()];
            reachable[start] = true;
            if !tree.leaves[start].is_solid {
                let mut queue = VecDeque::from([start]);
                while let Some(cell) = queue.pop_front() {
                    for portal in &portals {
                        let next = if portal.front_leaf == cell {
                            Some(portal.back_leaf)
                        } else if portal.back_leaf == cell {
                            Some(portal.front_leaf)
                        } else {
                            None
                        };
                        if let Some(next) = next.filter(|&next| !tree.leaves[next].is_solid) {
                            if !reachable[next] {
                                reachable[next] = true;
                                queue.push_back(next);
                            }
                        }
                    }
                }
            }
            for target in 0..tree.leaves.len() {
                assert_eq!(
                    section.component_ids[start] == section.component_ids[target],
                    reachable[target]
                );
            }
        }
        assert_eq!(progress.total(), Some(tree.leaves.len() * 2));
        assert_eq!(progress.completed(), tree.leaves.len() * 2);
    }

    #[test]
    fn hub_metric_cap_boundary_and_aperture_are_fixed_point_values() {
        let tree = tree(
            &[
                DVec3::ZERO,
                DVec3::new(2.0, 0.0, 0.0),
                DVec3::new(4.000_976_562_5, 0.0, 0.0),
            ],
            &[false, false, false],
        );
        let portals = vec![
            square_portal(0, 1, DVec3::X, 6.0),
            square_portal(1, 2, DVec3::new(3.000_488_281_25, 0.0, 0.0), 2.0),
        ];
        let edges = portal_edges(&tree, &portals);
        let progress = StageProgress::indeterminate();
        let control = test_control(&progress, 1);
        let ids = component_ids(tree.leaves.len(), &edges, &control);
        let cap = 2 * CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;
        let pairs = assemble_coupled_pairs(&tree, &edges, &ids, cap, 8, &control).unwrap();
        let section = CellVisibilitySection {
            cell_count: 3,
            component_ids: ids,
            coupled_pairs: pairs,
        };
        assert_eq!(pair(&section, 0, 1).unwrap().distance, cap);
        assert_eq!(
            pair(&section, 0, 1).unwrap().aperture,
            6 * CELL_VISIBILITY_APERTURE_FIXED_POINT_SCALE
        );
        assert!(pair(&section, 1, 2).is_none(), "cap + 1 must be omitted");
    }

    #[test]
    fn faceless_cells_stay_perceivable_without_metric_entries() {
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
        let progress = StageProgress::indeterminate();
        let section = cell_visibility_bake(&tree, &portals, &test_control(&progress, 1)).unwrap();
        assert_eq!(section.component_ids, vec![0, 0, 0]);
        assert!(section.coupled_pairs.is_empty());
    }

    #[test]
    fn fanout_caps_directed_selection_not_final_degree() {
        let tree = tree(
            &[
                DVec3::ZERO,
                DVec3::new(2.0, 0.0, 0.0),
                DVec3::new(-2.0, 0.0, 0.0),
                DVec3::new(0.0, 2.0, 0.0),
            ],
            &[false, false, false, false],
        );
        let portals = vec![
            square_portal(0, 1, DVec3::new(1.0, 0.0, 0.0), 1.0),
            square_portal(0, 2, DVec3::new(-1.0, 0.0, 0.0), 1.0),
            square_portal(0, 3, DVec3::new(0.0, 1.0, 0.0), 1.0),
        ];
        let edges = portal_edges(&tree, &portals);
        let progress = StageProgress::indeterminate();
        let control = test_control(&progress, 1);
        let ids = component_ids(tree.leaves.len(), &edges, &control);
        let graph = portal_hub_graph(&tree, &edges);
        let aperture_tree = maximum_spanning_tree(tree.leaves.len(), &edges);
        for source in 0..tree.leaves.len() {
            assert!(
                source_top_k(
                    source,
                    &graph,
                    &aperture_tree,
                    &ids,
                    CELL_VISIBILITY_DISTANCE_CAP,
                    1
                )
                .candidates
                .len()
                    <= 1
            );
        }
        let pairs = assemble_coupled_pairs(
            &tree,
            &edges,
            &ids,
            CELL_VISIBILITY_DISTANCE_CAP,
            1,
            &control,
        )
        .unwrap();
        assert_eq!(pairs.len(), 3);
        assert!(pairs.len() <= tree.leaves.len());
        assert_eq!(pairs.iter().filter(|pair| pair.cell_a == 0).count(), 3);
    }

    #[test]
    fn equal_fixed_point_distances_rank_by_target_cell_id() {
        let mut kept = Vec::new();
        insert_top_k(
            &mut kept,
            DirectedCandidate {
                source: 0,
                target: 9,
                distance: 42,
                aperture: 1,
            },
            1,
        );
        insert_top_k(
            &mut kept,
            DirectedCandidate {
                source: 0,
                target: 3,
                distance: 42,
                aperture: 1,
            },
            1,
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].target, 3);
    }

    #[test]
    fn sort_then_collapse_keeps_one_min_to_max_candidate() {
        let pairs = collapse_directed_candidates(vec![
            SourceCandidates {
                candidates: vec![DirectedCandidate {
                    source: 5,
                    target: 2,
                    distance: 88,
                    aperture: 7,
                }],
                ..SourceCandidates::default()
            },
            SourceCandidates {
                candidates: vec![DirectedCandidate {
                    source: 2,
                    target: 5,
                    distance: 77,
                    aperture: 7,
                }],
                ..SourceCandidates::default()
            },
        ])
        .unwrap();
        assert_eq!(
            pairs,
            vec![CoupledPairRecord {
                cell_a: 2,
                cell_b: 5,
                distance: 77,
                aperture: 7,
            }]
        );
    }

    #[test]
    fn faceless_source_does_not_shift_later_cell_ids() {
        let mut tree = tree(
            &[
                DVec3::ZERO,
                DVec3::new(2.0, 0.0, 0.0),
                DVec3::new(4.0, 0.0, 0.0),
            ],
            &[false, false, false],
        );
        tree.leaves[0].bounds = Aabb::empty();
        let portals = vec![square_portal(1, 2, DVec3::new(3.0, 0.0, 0.0), 1.0)];
        let progress = StageProgress::indeterminate();
        let section = cell_visibility_bake(&tree, &portals, &test_control(&progress, 1)).unwrap();

        assert_eq!(section.component_ids, vec![0, 1, 1]);
        assert!(pair(&section, 1, 2).is_some());
        assert!(pair(&section, 0, 1).is_none());
    }

    #[test]
    fn bytes_are_identical_across_governor_permit_counts() {
        let tree = tree(
            &[
                DVec3::ZERO,
                DVec3::new(2.0, 0.0, 0.0),
                DVec3::new(4.0, 0.0, 0.0),
                DVec3::new(2.0, 2.0, 0.0),
            ],
            &[false, false, false, false],
        );
        let portals = vec![
            square_portal(0, 1, DVec3::new(1.0, 0.0, 0.0), 2.0),
            square_portal(1, 2, DVec3::new(3.0, 0.0, 0.0), 2.0),
            square_portal(0, 3, DVec3::new(1.0, 1.0, 0.0), 2.0),
            square_portal(3, 2, DVec3::new(3.0, 1.0, 0.0), 2.0),
        ];
        let one_progress = StageProgress::indeterminate();
        let one = cell_visibility_bake(&tree, &portals, &test_control(&one_progress, 1))
            .unwrap()
            .to_bytes()
            .unwrap();
        let many_progress = StageProgress::indeterminate();
        let many = cell_visibility_bake(
            &tree,
            &portals,
            &test_control(
                &many_progress,
                std::thread::available_parallelism().map_or(1, NonZeroUsize::get),
            ),
        )
        .unwrap()
        .to_bytes()
        .unwrap();
        assert_eq!(one, many);
    }

    #[test]
    fn live_governor_permit_change_keeps_bake_bytes_identical() {
        let pool_width = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        if pool_width == 1 {
            // There is no wider live setting to exercise on a single-core
            // machine; the scoped pool still enforces the same P = 1 bound.
            return;
        }

        // The chain makes the p=1 source pass long enough to observe a source
        // completion before the whole pass ends without relying on a timer to
        // guess whether the permit change landed mid-bake.
        const CELL_COUNT: usize = 256;
        let centres: Vec<_> = (0..CELL_COUNT)
            .map(|cell| DVec3::new(cell as f64 * 2.0, 0.0, 0.0))
            .collect();
        let solid = vec![false; CELL_COUNT];
        let tree = Arc::new(tree(&centres, &solid));
        let portals = Arc::new(
            (0..CELL_COUNT - 1)
                .map(|cell| {
                    square_portal(
                        cell,
                        cell + 1,
                        DVec3::new(cell as f64 * 2.0 + 1.0, 0.0, 0.0),
                        1.0,
                    )
                })
                .collect::<Vec<_>>(),
        );
        let live_progress = StageProgress::indeterminate();
        let governor = Arc::new(Governor::new(1, false));
        let live_control = BakeControl::new(Arc::clone(&governor), &live_progress);
        let worker_tree = Arc::clone(&tree);
        let worker_portals = Arc::clone(&portals);
        let worker_control = live_control.clone();
        let worker = thread::spawn(move || {
            cell_visibility_bake(&worker_tree, &worker_portals, &worker_control)
                .unwrap()
                .to_bytes()
                .unwrap()
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        while live_progress.completed() <= CELL_COUNT {
            assert!(
                Instant::now() < deadline,
                "live-permit test did not finish one source row"
            );
            thread::yield_now();
        }
        assert!(
            live_progress.completed() < CELL_COUNT * 2,
            "permit change must land while the per-source pass is still active"
        );
        governor.set_permits(pool_width);
        let live_bytes = worker.join().expect("live-permit bake worker panicked");

        let fixed_progress = StageProgress::indeterminate();
        let fixed_bytes = cell_visibility_bake(&tree, &portals, &test_control(&fixed_progress, 1))
            .unwrap()
            .to_bytes()
            .unwrap();
        assert_eq!(live_bytes, fixed_bytes);
    }

    #[test]
    fn narrowing_a_sole_best_path_does_not_raise_aperture() {
        let tree = tree(
            &[
                DVec3::ZERO,
                DVec3::new(2.0, 0.0, 0.0),
                DVec3::new(4.0, 0.0, 0.0),
            ],
            &[false, false, false],
        );
        let wide = vec![
            square_portal(0, 1, DVec3::X, 4.0),
            square_portal(1, 2, DVec3::new(3.0, 0.0, 0.0), 4.0),
        ];
        let narrow = vec![
            square_portal(0, 1, DVec3::X, 4.0),
            square_portal(1, 2, DVec3::new(3.0, 0.0, 0.0), 1.0),
        ];
        let wide_progress = StageProgress::indeterminate();
        let wide = cell_visibility_bake(&tree, &wide, &test_control(&wide_progress, 1)).unwrap();
        let narrow_progress = StageProgress::indeterminate();
        let narrow =
            cell_visibility_bake(&tree, &narrow, &test_control(&narrow_progress, 1)).unwrap();
        assert!(pair(&narrow, 0, 2).unwrap().aperture <= pair(&wide, 0, 2).unwrap().aperture);
    }

    #[test]
    fn fixed_point_overflow_clamps_and_logs_a_warning() {
        let overflow =
            f64::from(u32::MAX) / f64::from(CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE) + 1.0;
        assert_eq!(
            fixed_point_value(overflow, CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE),
            (u32::MAX, true)
        );
        let tree = tree(
            &[DVec3::ZERO, DVec3::new(10_000_000.0, 0.0, 0.0)],
            &[false, false],
        );
        let portals = vec![square_portal(0, 1, DVec3::new(5_000_000.0, 0.0, 0.0), 1.0)];
        let progress = StageProgress::indeterminate();
        let capture = LogCapture::start();
        cell_visibility_bake(&tree, &portals, &test_control(&progress, 1)).unwrap();
        capture.assert_logged_once(
            Level::Warn,
            "[Compiler] CellVisibility fixed-point value(s) clamped to u32::MAX",
        );
    }
}
