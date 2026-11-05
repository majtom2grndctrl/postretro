// The streamed-SH residency plan: each cluster's own demand from id 49, the
// dense stored-node layout, sparse pool and compose-planner row spaces, the
// sample-region tables and the sparse compose layouts. Pure CPU over the
// stream manifest and the runtime cells; the renderer's residency state takes
// it.
// See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use postretro_level_format::cluster_directory::{ClusterDirectorySection, ClusterResourceDomain};
use postretro_level_loader::{
    CellData, ShStreamBaseMetadata, ShStreamManifest, ShStreamSourceMetadata,
};

use super::dense_spans::{DenseSpan, DenseSpans};
use super::node_map::NodeMap;
use super::sample_region_tables::SampleRegionTables;
use super::sample_regions::{SampleRegionGrid, ShSampleRegion};
use super::sparse_compose::{
    SparseCapacityFloors, SparseComposeLayouts, compose_row_capacity, sparse_capacity_floors,
};
use super::stored_nodes::{
    DenseNodeLayout, StoredNode, StoredNodeLayout, derive_dense_node_layout,
};
use super::{
    ANIMATED_DIRECT_DELTA_ID, DIRECT_DELTA_ID, INDIRECT_BASE_ID, INDIRECT_DELTA_ID,
    ShResidencyDrainError,
};

/// The dense pool's slot needs before any device sizing: the stored-node
/// tiles of the largest single cluster (at least one), and the whole level's
/// id-34 stored-node prefix total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InitialDenseSlots {
    pub minimum: u32,
    pub whole: u32,
}

/// What one cluster names in id 49, as its own ranges: enough to price a
/// target whose chunk is not in hand.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClusterStorage {
    /// Dense probe ranges.
    pub dense: Vec<Range<u32>>,
    /// Each sparse family's rows, as `(section_id, rows)`.
    pub sparse: Vec<(u32, Range<u32>)>,
}

/// Everything the renderer's residency state derives from a streamed level
/// before any GPU object exists. Every check here can fail on level data, so
/// a plan error is a load failure, raised before the install.
#[derive(Debug)]
pub struct ShResidencyPlan {
    pub content_tag: [u8; 32],
    pub cluster_count: u32,
    pub dense_node: Vec<Option<StoredNode>>,
    pub dense_node_local_slot: Vec<Option<u32>>,
    pub node_layouts: NodeMap<StoredNodeLayout>,
    /// Which clusters name each dense probe, for residency diagnostics.
    pub dense_spans: DenseSpans,
    /// Each cluster's named storage, indexed by cluster.
    pub cluster_storage: Vec<ClusterStorage>,
    pub sparse_capacity_floors: SparseCapacityFloors,
    /// Each present sparse source's affinity row count, keyed by section id:
    /// the CPU sparse pools' row spaces.
    pub sparse_pool_rows: BTreeMap<u32, usize>,
    /// The affinity-row id space for dense compose-planner state.
    pub row_capacity: usize,
    pub sample_regions: SampleRegionTables,
    pub sparse_compose: SparseComposeLayouts,
    pub initial_dense_slots: InitialDenseSlots,
}

// Compile-time guard: the plan is built on a load-pool thread.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<ShResidencyPlan>();
};

/// Plan a streamed level's residency. `cells` are the level's runtime cells;
/// the sampled-row gate indexes their bounds here once, and frames name
/// cells by id.
pub fn plan_sh_residency(
    manifest: &ShStreamManifest,
    cells: &[CellData],
) -> Result<ShResidencyPlan, ShResidencyDrainError> {
    plan_sh_residency_parts(
        manifest.content_tag(),
        manifest.cluster_directory(),
        manifest.base(),
        manifest.sources(),
        cells,
    )
}

pub fn plan_sh_residency_parts(
    content_tag: [u8; 32],
    directory: &ClusterDirectorySection,
    base: &ShStreamBaseMetadata,
    sources: &ShStreamSourceMetadata,
    cells: &[CellData],
) -> Result<ShResidencyPlan, ShResidencyDrainError> {
    let cluster_count = u32::try_from(directory.clusters.len()).map_err(|_| {
        ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "cluster count exceeds u32",
        }
    })?;
    let dense_resource = directory
        .resources
        .iter()
        .position(|resource| {
            resource.section_id == INDIRECT_BASE_ID
                && resource.domain == ClusterResourceDomain::DenseProbe
        })
        .ok_or(ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "id-49 has no id-34 dense resource",
        })?;
    let dense_layout = derive_dense_node_layout(base)?;
    let demand = walk_cluster_demand(directory, dense_resource, &dense_layout)?;
    if let Some(dense_index) = demand.first_uncovered_probe {
        return Err(ShResidencyDrainError::UncoveredDenseProbe { dense_index });
    }
    let sparse_capacity_floors = sparse_capacity_floors(sources, &demand.cluster_sparse_rows)?;
    let dense_node = dense_layout.nodes;

    let mut sparse_pool_rows = BTreeMap::new();
    for (section_id, metadata) in [
        (INDIRECT_DELTA_ID, sources.indirect_delta.as_ref()),
        (DIRECT_DELTA_ID, sources.direct_delta.as_ref()),
        (
            ANIMATED_DIRECT_DELTA_ID,
            sources.animated_direct_delta.as_ref(),
        ),
    ] {
        if let Some(metadata) = metadata {
            sparse_pool_rows.insert(
                section_id,
                metadata.affinity_offsets.len().saturating_sub(1),
            );
        }
    }

    let row_capacity = compose_row_capacity(base.grid_dimensions, sources)?;
    let sample_regions = SampleRegionTables::build(
        &SampleRegionGrid {
            origin: base.grid_origin,
            cell_size: base.cell_size,
            dimensions: base.grid_dimensions,
            dense_nodes: &dense_node,
        },
        cells
            .iter()
            .map(|cell| ShSampleRegion::new(cell.bounds_min, cell.bounds_max)),
    )?;
    let initial_dense_slots = InitialDenseSlots {
        minimum: demand.largest_cluster_slots,
        whole: whole_dense_slots(&dense_layout.layouts)?,
    };
    let sparse_compose = SparseComposeLayouts::derive(base, sources)?;

    Ok(ShResidencyPlan {
        content_tag,
        cluster_count,
        dense_node,
        dense_node_local_slot: dense_layout.local_slots,
        node_layouts: dense_layout.layouts,
        dense_spans: demand.dense_spans,
        cluster_storage: demand.cluster_storage,
        sparse_capacity_floors,
        sparse_pool_rows,
        row_capacity,
        sample_regions,
        sparse_compose,
        initial_dense_slots,
    })
}

/// What id 49 says each cluster's chunk carries. Clusters are independent, so
/// every figure here is one cluster's full demand, shared storage included.
struct ClusterDemand {
    /// Stored-node tiles of the largest single cluster, at least one slot.
    largest_cluster_slots: u32,
    /// Each cluster's `(section_id, row)` pairs, indexed by cluster.
    cluster_sparse_rows: Vec<BTreeSet<(u32, u32)>>,
    dense_spans: DenseSpans,
    cluster_storage: Vec<ClusterStorage>,
    /// The first node-bearing probe no cluster names, if any.
    first_uncovered_probe: Option<u32>,
}

fn walk_cluster_demand(
    directory: &ClusterDirectorySection,
    dense_resource: usize,
    dense_layout: &DenseNodeLayout,
) -> Result<ClusterDemand, ShResidencyDrainError> {
    let mut covered = vec![false; dense_layout.nodes.len()];
    let mut spans = Vec::new();
    let mut cluster_sparse_rows = vec![BTreeSet::new(); directory.clusters.len()];
    let mut cluster_storage = vec![ClusterStorage::default(); directory.clusters.len()];
    let mut largest_cluster_slots = 1u32;
    let mut cluster_nodes = BTreeSet::new();
    for (cluster_index, cluster) in directory.clusters.iter().enumerate() {
        let cluster_id =
            u32::try_from(cluster_index).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        let malformed = |reason| ShResidencyDrainError::MalformedChunk { cluster_id, reason };
        let start = usize::try_from(cluster.range_start)
            .map_err(|_| malformed("cluster range start exceeds usize"))?;
        let end = start
            .checked_add(
                usize::try_from(cluster.range_count)
                    .map_err(|_| malformed("cluster range count exceeds usize"))?,
            )
            .ok_or(malformed("cluster range end overflows"))?;
        cluster_nodes.clear();
        for range in directory
            .ranges
            .get(start..end)
            .ok_or(malformed("cluster range exceeds id-49"))?
        {
            let resource = directory
                .resources
                .get(range.resource_index as usize)
                .ok_or(malformed("range resource index exceeds id-49"))?;
            let range_end = range
                .start
                .checked_add(range.count)
                .ok_or(malformed("range end overflows"))?;
            match resource.domain {
                ClusterResourceDomain::DenseProbe
                    if range.resource_index as usize == dense_resource =>
                {
                    let probes = range.start as usize..range_end as usize;
                    covered
                        .get_mut(probes.clone())
                        .ok_or(malformed("dense range exceeds id-34 metadata"))?
                        .fill(true);
                    spans.push(DenseSpan {
                        start: range.start,
                        end: range_end,
                        cluster: cluster_id,
                    });
                    cluster_storage[cluster_index]
                        .dense
                        .push(range.start..range_end);
                    // Consecutive probes mostly repeat the previous node.
                    let mut last = None;
                    for &node in dense_layout.nodes[probes].iter().flatten() {
                        if last != Some(node) {
                            cluster_nodes.insert(node);
                            last = Some(node);
                        }
                    }
                }
                ClusterResourceDomain::AffinityCell
                    if matches!(
                        resource.section_id,
                        INDIRECT_DELTA_ID | DIRECT_DELTA_ID | ANIMATED_DIRECT_DELTA_ID
                    ) =>
                {
                    cluster_sparse_rows[cluster_index]
                        .extend((range.start..range_end).map(|row| (resource.section_id, row)));
                    cluster_storage[cluster_index]
                        .sparse
                        .push((resource.section_id, range.start..range_end));
                }
                _ => {}
            }
        }
        let slots = cluster_nodes.iter().try_fold(0u32, |total, node| {
            total
                .checked_add(
                    dense_layout
                        .layouts
                        .get(node)
                        .ok_or(ShResidencyDrainError::SlotOverflow)?
                        .tile_count,
                )
                .ok_or(ShResidencyDrainError::SlotOverflow)
        })?;
        largest_cluster_slots = largest_cluster_slots.max(slots);
    }
    let first_uncovered_probe = dense_layout
        .nodes
        .iter()
        .zip(&covered)
        .position(|(node, &covered)| node.is_some() && !covered)
        .map(u32::try_from)
        .transpose()
        .map_err(|_| ShResidencyDrainError::SlotOverflow)?;
    Ok(ClusterDemand {
        largest_cluster_slots,
        cluster_sparse_rows,
        dense_spans: DenseSpans::new(spans),
        cluster_storage,
        first_uncovered_probe,
    })
}

/// The end of the furthest stored node's global slot range.
fn whole_dense_slots(
    node_layouts: &NodeMap<StoredNodeLayout>,
) -> Result<u32, ShResidencyDrainError> {
    node_layouts.values().try_fold(0u32, |total, layout| {
        let end = layout
            .global_base_slot
            .checked_add(layout.tile_count)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        Ok(total.max(end))
    })
}
