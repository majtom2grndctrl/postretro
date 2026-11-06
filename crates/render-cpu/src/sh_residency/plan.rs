// The streamed-SH residency plan: each cluster's own demand from id 49, the
// dense stored-node layout, sparse pool and compose-planner row spaces, the
// sample-region tables and the sparse compose layouts. Pure CPU over the
// stream manifest and the runtime cells; the renderer's residency state takes
// it.
// See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use std::collections::BTreeMap;
use std::ops::Range;

use postretro_level_format::cluster_directory::{ClusterDirectorySection, ClusterResourceDomain};
use postretro_level_loader::{
    CellData, ShStreamBaseMetadata, ShStreamManifest, ShStreamSourceMetadata,
};

use super::dense_spans::{DenseSpan, DenseSpans};
use super::node_map::NodeMap;
use super::pool_bound::{ShPoolBound, stored_pool_bound};
use super::sample_region_tables::SampleRegionTables;
use super::sample_regions::{SampleRegionGrid, ShSampleRegion};
use super::sparse_compose::{SparseComposeLayouts, compose_row_capacity, sparse_row_demand};
use super::stored_nodes::{
    DenseNodeLayout, StoredNode, StoredNodeLayout, derive_dense_node_layout,
};
use super::{
    ANIMATED_DIRECT_DELTA_ID, DIRECT_DELTA_ID, INDIRECT_BASE_ID, INDIRECT_DELTA_ID,
    ShResidencyDrainError,
};

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
    /// The stored figure the pools size from.
    pub pool_bound: ShPoolBound,
    /// Each present sparse source's affinity row count, keyed by section id:
    /// the CPU sparse pools' row spaces.
    pub sparse_pool_rows: BTreeMap<u32, usize>,
    /// The affinity-row id space for dense compose-planner state.
    pub row_capacity: usize,
    pub sample_regions: SampleRegionTables,
    pub sparse_compose: SparseComposeLayouts,
    /// The level's id-34 stored-node prefix total: the dense unique union,
    /// which no sum of per-cluster figures can give.
    pub whole_dense_slots: u32,
}

// Compile-time guard: the plan is built on a load-pool thread.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<ShResidencyPlan>();
};

/// Plan a streamed level's residency. `cells` are the level's runtime cells;
/// the sampled-row gate indexes their bounds here once, and frames name
/// cells by id. `baked_max_lead` is id 51's maximum, `None` without a usable
/// id 51.
pub fn plan_sh_residency(
    manifest: &ShStreamManifest,
    cells: &[CellData],
    baked_max_lead: Option<u32>,
) -> Result<ShResidencyPlan, ShResidencyDrainError> {
    plan_sh_residency_parts(
        manifest.content_tag(),
        manifest.cluster_directory(),
        manifest.base(),
        manifest.sources(),
        cells,
        baked_max_lead,
    )
}

pub fn plan_sh_residency_parts(
    content_tag: [u8; 32],
    directory: &ClusterDirectorySection,
    base: &ShStreamBaseMetadata,
    sources: &ShStreamSourceMetadata,
    cells: &[CellData],
    baked_max_lead: Option<u32>,
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
    let demand = walk_cluster_demand(directory, dense_resource, &dense_layout, sources)?;
    if let Some(dense_index) = demand.first_uncovered_probe {
        return Err(ShResidencyDrainError::UncoveredDenseProbe { dense_index });
    }
    let pool_bound = stored_pool_bound(directory, baked_max_lead)?;
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
    let whole_dense_slots = whole_dense_slots(&dense_layout.layouts)?;
    let sparse_compose = SparseComposeLayouts::derive(base, sources)?;

    Ok(ShResidencyPlan {
        content_tag,
        cluster_count,
        dense_node,
        dense_node_local_slot: dense_layout.local_slots,
        node_layouts: dense_layout.layouts,
        dense_spans: demand.dense_spans,
        cluster_storage: demand.cluster_storage,
        pool_bound,
        sparse_pool_rows,
        row_capacity,
        sample_regions,
        sparse_compose,
        whole_dense_slots,
    })
}

/// What id 49 says each cluster's chunk carries. Clusters are independent, so
/// every figure here is one cluster's full demand, shared storage included.
struct ClusterDemand {
    dense_spans: DenseSpans,
    cluster_storage: Vec<ClusterStorage>,
    /// The first node-bearing probe no cluster names, if any.
    first_uncovered_probe: Option<u32>,
}

fn walk_cluster_demand(
    directory: &ClusterDirectorySection,
    dense_resource: usize,
    dense_layout: &DenseNodeLayout,
    sources: &ShStreamSourceMetadata,
) -> Result<ClusterDemand, ShResidencyDrainError> {
    let mut covered = vec![false; dense_layout.nodes.len()];
    let mut spans = Vec::new();
    let mut cluster_storage = vec![ClusterStorage::default(); directory.clusters.len()];
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
                        .get_mut(probes)
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
                }
                ClusterResourceDomain::AffinityCell
                    if matches!(
                        resource.section_id,
                        INDIRECT_DELTA_ID | DIRECT_DELTA_ID | ANIMATED_DIRECT_DELTA_ID
                    ) =>
                {
                    // A row no chunk could price fails the load, not a frame.
                    for row in range.start..range_end {
                        sparse_row_demand(sources, cluster_id, resource.section_id, row)?;
                    }
                    cluster_storage[cluster_index]
                        .sparse
                        .push((resource.section_id, range.start..range_end));
                }
                _ => {}
            }
        }
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
