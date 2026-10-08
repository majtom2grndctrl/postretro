// Canonical cell partition, resolved once in atlas preparation, after the face cut, for block order and id 49.
// See: context/lib/build_pipeline.md §PRL section IDs (ClusterDirectory, Lightmap)

use std::collections::HashSet;

use postretro_level_format::bsp::BspLeavesSection;
use postretro_level_format::bvh::BvhSection;
use postretro_level_format::cells::CellsSection;
use postretro_level_format::cluster_directory::CanonicalCellPartition;
use postretro_level_format::portals::PortalsSection;

use super::{pack, portals};
use crate::cluster_directory_bake::default_cell_partition;
use crate::map_data::{MapStreamingHintRegion, MapStreamingPriorityRegion};
use crate::streaming_hints::{ResolvedStreamingHints, resolve_streaming_hints};

/// The runtime cells, portals, resolved streaming hints, and canonical
/// partition the cluster directory is built from. Lightmap blocks are stored
/// in this partition's cluster order, so it resolves inside atlas preparation,
/// after the oversize-face cut and before packing; the ClusterDirectory stage
/// consumes it rather than recomputing. Its inputs are final once the cut has
/// rebuilt the leaf face ranges and the BVH: later stages stamp only animated
/// chunk ranges onto BVH leaves, which the partition does not read.
pub(super) struct CellPartitionPlan {
    pub(super) portals: PortalsSection,
    pub(super) cells: CellsSection,
    pub(super) streaming_hints: ResolvedStreamingHints,
    pub(super) partition: CanonicalCellPartition,
}

pub(super) struct CellPartitionInputs<'a> {
    pub(super) generated_portals: &'a [portals::Portal],
    pub(super) streaming_seam_regions: &'a [MapStreamingHintRegion],
    pub(super) stream_resident_regions: &'a [MapStreamingHintRegion],
    pub(super) stream_priority_regions: &'a [MapStreamingPriorityRegion],
    pub(super) leaves: &'a BspLeavesSection,
    pub(super) exterior_leaves: &'a HashSet<usize>,
    pub(super) bvh: &'a BvhSection,
}

pub(super) fn plan_cell_partition(
    inputs: CellPartitionInputs<'_>,
) -> anyhow::Result<CellPartitionPlan> {
    let portals = pack::encode_portals(inputs.generated_portals)?;
    let cells = pack::encode_cells(inputs.leaves, &portals, inputs.exterior_leaves)?;
    let streaming_hints = resolve_streaming_hints(
        inputs.streaming_seam_regions,
        inputs.stream_resident_regions,
        inputs.stream_priority_regions,
        inputs.generated_portals,
        &portals,
        &cells,
    )?;
    let partition = default_cell_partition(&cells, &portals, inputs.bvh, &streaming_hints)?;
    Ok(CellPartitionPlan {
        portals,
        cells,
        streaming_hints,
        partition,
    })
}

impl CellPartitionPlan {
    /// Cluster of each runtime cell, indexed by cell id.
    pub(super) fn cell_clusters(&self) -> Vec<u32> {
        let mut clusters = vec![u32::MAX; self.cells.cells.len()];
        for (cluster_id, cluster) in self.partition.clusters.iter().enumerate() {
            let start = cluster.member_start as usize;
            let end = start + cluster.member_count as usize;
            for &cell in &self.partition.members[start..end] {
                clusters[cell as usize] = cluster_id as u32;
            }
        }
        clusters
    }
}
