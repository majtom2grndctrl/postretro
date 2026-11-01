// The streamed-SH residency plan: the owner walk over id 49, sparse
// capacity floors, the dense stored-node layout and ownership, sparse pool
// and compose-planner row spaces, the sample-region tables, the sparse
// compose layouts and the dense slot minima. Pure CPU over the stream
// manifest and the runtime cells; the renderer's residency state takes it.
// See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use std::collections::{BTreeMap, BTreeSet};

use postretro_level_format::cluster_directory::{ClusterDirectorySection, ClusterResourceDomain};
use postretro_level_loader::{
    CellData, ShStreamBaseMetadata, ShStreamManifest, ShStreamSourceMetadata,
};

use super::node_map::NodeMap;
use super::node_ownership::{NodeOwnership, derive_node_ownership, derive_owner_dependencies};
use super::ownership::{StoredNode, StoredNodeLayout, derive_dense_node_layout};
use super::sample_region_tables::SampleRegionTables;
use super::sample_regions::{SampleRegionGrid, ShSampleRegion};
use super::sparse_compose::{
    SparseCapacityFloors, SparseComposeLayouts, compose_row_capacity, sparse_capacity_floors,
};
use super::{
    ANIMATED_DIRECT_DELTA_ID, DIRECT_DELTA_ID, INDIRECT_BASE_ID, INDIRECT_DELTA_ID,
    ShResidencyDrainError,
};

/// The dense pool's slot needs before any device sizing: the largest
/// canonical owner's stored-node tiles (at least one), and the whole level's
/// id-34 stored-node prefix total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InitialDenseSlots {
    pub minimum: u32,
    pub whole: u32,
}

/// Everything the renderer's residency state derives from a streamed level
/// before any GPU object exists. Every check here can fail on level data, so
/// a plan error is a load failure, raised before the install.
#[derive(Debug)]
pub struct ShResidencyPlan {
    pub content_tag: [u8; 32],
    pub cluster_count: u32,
    pub dense_owner: Vec<Option<u32>>,
    pub dense_node: Vec<Option<StoredNode>>,
    pub dense_node_local_slot: Vec<Option<u32>>,
    pub node_layouts: NodeMap<StoredNodeLayout>,
    pub node_owner: NodeMap<u32>,
    pub nodes_by_owner: BTreeMap<u32, Vec<StoredNode>>,
    pub owner_dependencies: Vec<BTreeSet<u32>>,
    pub sparse_row_owner: BTreeMap<(u32, u32), u32>,
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
    let mut dense_owner = vec![None; base.probes.len()];
    let mut sparse_row_owner = BTreeMap::new();
    for cluster_id in 0..cluster_count {
        let cluster = &directory.clusters[cluster_id as usize];
        let start = usize::try_from(cluster.range_start).map_err(|_| {
            ShResidencyDrainError::MalformedChunk {
                cluster_id,
                reason: "cluster range start exceeds usize",
            }
        })?;
        let end = start
            .checked_add(usize::try_from(cluster.range_count).map_err(|_| {
                ShResidencyDrainError::MalformedChunk {
                    cluster_id,
                    reason: "cluster range count exceeds usize",
                }
            })?)
            .ok_or(ShResidencyDrainError::MalformedChunk {
                cluster_id,
                reason: "cluster range end overflows",
            })?;
        for range in
            directory
                .ranges
                .get(start..end)
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id,
                    reason: "cluster range exceeds id-49",
                })?
        {
            let resource = directory
                .resources
                .get(range.resource_index as usize)
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id,
                    reason: "range resource index exceeds id-49",
                })?;
            let range_end = range.start.checked_add(range.count).ok_or(
                ShResidencyDrainError::MalformedChunk {
                    cluster_id,
                    reason: "range end overflows",
                },
            )?;
            match resource.domain {
                ClusterResourceDomain::DenseProbe
                    if range.resource_index as usize == dense_resource =>
                {
                    for dense in range.start..range_end {
                        let Some(owner) = dense_owner.get_mut(dense as usize) else {
                            return Err(ShResidencyDrainError::MalformedChunk {
                                cluster_id,
                                reason: "dense range exceeds id-34 metadata",
                            });
                        };
                        let prior: Option<u32> = *owner;
                        *owner = Some(prior.map_or(cluster_id, |prior| prior.min(cluster_id)));
                    }
                }
                ClusterResourceDomain::AffinityCell
                    if matches!(
                        resource.section_id,
                        INDIRECT_DELTA_ID | DIRECT_DELTA_ID | ANIMATED_DIRECT_DELTA_ID
                    ) =>
                {
                    for row in range.start..range_end {
                        let owner = range.owner_cluster_id;
                        let key = (resource.section_id, row);
                        match sparse_row_owner.get(&key) {
                            Some(&prior) if prior != owner => {
                                return Err(ShResidencyDrainError::MalformedChunk {
                                    cluster_id,
                                    reason: "sparse row has conflicting owners",
                                });
                            }
                            _ => {
                                sparse_row_owner.insert(key, owner);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let sparse_capacity_floors = sparse_capacity_floors(sources, &sparse_row_owner)?;
    let dense_layout = derive_dense_node_layout(base)?;
    let dense_node = dense_layout.nodes;
    let NodeOwnership {
        node_owner,
        nodes_by_owner,
    } = derive_node_ownership(&dense_node, &dense_owner, base.grid_dimensions)?;
    let owner_dependencies =
        derive_owner_dependencies(directory, &dense_owner, &dense_node, |node| {
            node_owner.get(node).copied()
        })?;

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
    let initial_dense_slots = initial_dense_slots(&nodes_by_owner, &dense_layout.layouts)?;
    let sparse_compose = SparseComposeLayouts::derive(base, sources)?;

    Ok(ShResidencyPlan {
        content_tag,
        cluster_count,
        dense_owner,
        dense_node,
        dense_node_local_slot: dense_layout.local_slots,
        node_layouts: dense_layout.layouts,
        node_owner,
        nodes_by_owner,
        owner_dependencies,
        sparse_row_owner,
        sparse_capacity_floors,
        sparse_pool_rows,
        row_capacity,
        sample_regions,
        sparse_compose,
        initial_dense_slots,
    })
}

/// The largest canonical owner's stored-node tile count (at least one slot)
/// and the end of the furthest stored node's global slot range.
fn initial_dense_slots(
    nodes_by_owner: &BTreeMap<u32, Vec<StoredNode>>,
    node_layouts: &NodeMap<StoredNodeLayout>,
) -> Result<InitialDenseSlots, ShResidencyDrainError> {
    let mut minimum = 1u32;
    for nodes in nodes_by_owner.values() {
        let slots = nodes.iter().try_fold(0u32, |total, node| {
            total
                .checked_add(
                    node_layouts
                        .get(node)
                        .ok_or(ShResidencyDrainError::SlotOverflow)?
                        .tile_count,
                )
                .ok_or(ShResidencyDrainError::SlotOverflow)
        })?;
        minimum = minimum.max(slots);
    }
    let whole = node_layouts.values().try_fold(0u32, |total, layout| {
        let end = layout
            .global_base_slot
            .checked_add(layout.tile_count)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        Ok::<_, ShResidencyDrainError>(total.max(end))
    })?;
    Ok(InitialDenseSlots { minimum, whole })
}
