//! Level-static block facts: cell to block, extents, read ranges, pins, priority.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use super::LightmapResidencyError;
use super::source::LightmapBlockSource;
use crate::streaming::cluster_hints::ClusterHints;
use crate::streaming::request::ReadRanges;

/// A cell without charts has no block.
const NO_BLOCK: u32 = u32::MAX;

/// What the controller needs about one block for the level's lifetime.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BlockFacts {
    /// Irradiance texels: the unit of the pool's band headroom.
    pub(crate) texels: u64,
    /// Upload bytes of the id-22 half (irradiance plus direction).
    pub(crate) lightmap_bytes: u64,
    /// Upload bytes of the id-42 half (both groups); 0 without id 42.
    pub(crate) shadowmask_bytes: u64,
    /// The pair's absolute file ranges, as the issuer reads them.
    pub(crate) ranges: ReadRanges,
    /// The owning cluster's authored prefetch priority (0 without id 49).
    pub(crate) priority: u8,
    /// The owning cluster is pinned by an authored resident volume.
    pub(crate) pinned: bool,
}

impl BlockFacts {
    pub(crate) fn pair_bytes(&self) -> u64 {
        self.lightmap_bytes + self.shadowmask_bytes
    }
}

/// Built once per level from the block index and the id-49 hints. Maps
/// residency-set cells to blocks; at most one block per cell.
#[derive(Debug)]
pub(crate) struct LevelBlockMap {
    cell_to_block: Vec<u32>,
    blocks: Vec<BlockFacts>,
    /// Blocks of pinned clusters, ascending: mandatory from every camera cell.
    pinned_blocks: Vec<u32>,
}

impl LevelBlockMap {
    pub(crate) fn build(
        source: &dyn LightmapBlockSource,
        cell_count: usize,
        hints: Option<&ClusterHints>,
    ) -> Result<Self, LightmapResidencyError> {
        let invalid = |message: String| Err(LightmapResidencyError::InvalidLevel(message));
        if let Some(hints) = hints
            && hints.cell_to_cluster.len() != cell_count
        {
            return invalid(format!(
                "id-49 maps {} cells but the residency set has {cell_count}",
                hints.cell_to_cluster.len()
            ));
        }
        let block_count = source.block_count();
        let mut cell_to_block = vec![NO_BLOCK; cell_count];
        let mut blocks = Vec::with_capacity(block_count as usize);
        let mut pinned_blocks = Vec::new();
        for block in 0..block_count {
            let Some(summary) = source.block_summary(block) else {
                return invalid(format!("block {block} has no index record"));
            };
            let Some(slot) = cell_to_block.get_mut(summary.cell_id as usize) else {
                return invalid(format!(
                    "block {block} names cell {} past the {cell_count}-cell residency set",
                    summary.cell_id
                ));
            };
            if *slot != NO_BLOCK {
                return invalid(format!("cell {} owns more than one block", summary.cell_id));
            }
            *slot = block;
            let ranges = source
                .block_file_ranges(block)
                .map_err(LightmapResidencyError::Source)?;
            let lightmap_bytes = ranges.lightmap.end - ranges.lightmap.start;
            let (read_ranges, shadowmask_bytes) = match &ranges.shadowmask {
                Some(shadowmask) => (
                    ReadRanges::pair(ranges.lightmap.clone(), shadowmask.clone()),
                    shadowmask.end - shadowmask.start,
                ),
                None => (ReadRanges::one(ranges.lightmap.clone()), 0),
            };
            let cluster = hints.map(|hints| hints.cell_to_cluster[summary.cell_id as usize]);
            let pinned = hints
                .zip(cluster)
                .is_some_and(|(hints, cluster)| hints.pinned.contains(&cluster));
            let priority = hints.zip(cluster).map_or(0, |(hints, cluster)| {
                hints.priority.get(cluster as usize).copied().unwrap_or(0)
            });
            if pinned {
                pinned_blocks.push(block);
            }
            blocks.push(BlockFacts {
                texels: u64::from(summary.width) * u64::from(summary.height),
                lightmap_bytes,
                shadowmask_bytes,
                ranges: read_ranges,
                priority,
                pinned,
            });
        }
        Ok(Self {
            cell_to_block,
            blocks,
            pinned_blocks,
        })
    }

    pub(crate) fn block_count(&self) -> usize {
        self.blocks.len()
    }

    pub(crate) fn block_of_cell(&self, cell: u32) -> Option<u32> {
        self.cell_to_block
            .get(cell as usize)
            .copied()
            .filter(|&block| block != NO_BLOCK)
    }

    pub(crate) fn facts(&self, block: u32) -> &BlockFacts {
        &self.blocks[block as usize]
    }

    pub(crate) fn pinned_blocks(&self) -> &[u32] {
        &self.pinned_blocks
    }
}
