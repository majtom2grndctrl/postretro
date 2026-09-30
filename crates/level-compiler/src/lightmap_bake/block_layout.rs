// Lightmap cell-block layout: cluster-major block order, per-cell packing, runtime limits, bake layers.
// See: context/lib/build_pipeline.md §PRL section IDs (Lightmap id 22)

use std::collections::BTreeMap;

use postretro_level_format::lightmap::{
    LIGHTMAP_POOL_LAYER_EDGE, LightmapHeader, LightmapMode, MAX_LIGHTMAP_BLOCKS,
};
use rayon::prelude::*;

use super::atlas_pack::{GroupPackError, pack_groups_into_layers};
use super::cell_blocks::{
    CellSubBlock, PackedBlock, pack_cell_blocks_within as pack_cell_blocks_within_cell,
};
use super::charts::Chart;
use super::encode::normalized_direction_texel_scale;
use super::{LightmapBakeError, MAX_ATLAS_DIMENSION, MAX_ATLAS_LAYERS};
use crate::bake_control::BakeControl;
use crate::chart_raster::ChartPlacement;

/// One lightmap block of a cell: its extent, and where the bake placed it inside
/// an internal bake layer. The block id is its index in [`BlockLayout::blocks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellBlock {
    pub cell_id: u32,
    pub width: u32,
    pub height: u32,
    /// Bake layer holding the block.
    pub layer: u32,
    /// Block origin inside its bake layer; multiples of the block alignment.
    pub x: u32,
    pub y: u32,
}

impl CellBlock {
    /// Whether bake-layer rect `(layer, x, y, width, height)` lies inside
    /// this block.
    pub fn contains(&self, layer: u32, x: u32, y: u32, width: u32, height: u32) -> bool {
        layer == self.layer
            && x >= self.x
            && y >= self.y
            && u64::from(x) + u64::from(width) <= u64::from(self.x) + u64::from(self.width)
            && u64::from(y) + u64::from(height) <= u64::from(self.y) + u64::from(self.height)
    }
}

/// The emitted blocks in block-id (cluster-major) order, plus each chart's
/// block. Charts keep bake-layer placements; a chart's block-local position is
/// its placement minus its block's origin.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlockLayout {
    /// Irradiance texels per direction texel along each axis; the id-22
    /// header's `direction_texel_scale`.
    pub direction_texel_scale: u32,
    pub blocks: Vec<CellBlock>,
    /// Block id of each chart, parallel to the charts.
    pub chart_blocks: Vec<u32>,
}

impl BlockLayout {
    /// `lcm(4, direction_texel_scale)`: every block extent and bake-layer
    /// origin is a multiple of it.
    pub fn alignment(&self) -> u32 {
        block_alignment(self.direction_texel_scale)
    }

    /// Block-local origin of a chart's bake-layer placement.
    pub fn local_placement(&self, chart: usize, placement: &ChartPlacement) -> (u32, u32) {
        let block = &self.blocks[self.chart_blocks[chart] as usize];
        debug_assert_eq!(placement.layer, block.layer);
        (placement.x - block.x, placement.y - block.y)
    }

    /// One block per bake layer covering the whole layer, block id = layer.
    /// Lets tests of the bake-layer machinery encode sections without a cell
    /// partition.
    #[cfg(test)]
    pub fn whole_layers(
        atlas_width: u32,
        atlas_height: u32,
        placements: &[ChartPlacement],
        direction_texel_scale: u32,
    ) -> Self {
        let layer_count = placements.iter().map(|p| p.layer + 1).max().unwrap_or(1);
        Self {
            direction_texel_scale,
            blocks: (0..layer_count)
                .map(|layer| CellBlock {
                    cell_id: layer,
                    width: atlas_width,
                    height: atlas_height,
                    layer,
                    x: 0,
                    y: 0,
                })
                .collect(),
            chart_blocks: placements.iter().map(|p| p.layer).collect(),
        }
    }
}

/// The alignment the id-22 header derives from its direction scale.
pub(crate) fn block_alignment(direction_texel_scale: u32) -> u32 {
    LightmapHeader {
        block_count: 0,
        direction_texel_scale,
        irradiance_format: 0,
        mode: LightmapMode::Shadowed,
    }
    .block_alignment()
}

/// What orders and aligns blocks: the direction scale and each runtime cell's
/// id-49 cluster.
#[derive(Debug, Clone, Copy)]
pub struct BlockOrdering<'a> {
    /// Configured scale; normalized to a power of two before use.
    pub direction_texel_scale: u32,
    /// Cluster of each runtime cell, indexed by cell id. Empty orders blocks
    /// by cell id alone (tests without a partition).
    pub cell_clusters: &'a [u32],
}

impl BlockOrdering<'_> {
    pub fn by_cell_id(direction_texel_scale: u32) -> BlockOrdering<'static> {
        BlockOrdering {
            direction_texel_scale,
            cell_clusters: &[],
        }
    }

    fn sort_key(&self, cell_id: u32) -> (u32, u32) {
        // Empty `cell_clusters` means no partition, so the cell id alone orders;
        // a cell past a non-empty table sorts after every cluster.
        let cluster = self.cell_clusters.get(cell_id as usize).copied().unwrap_or(
            if self.cell_clusters.is_empty() {
                0
            } else {
                u32::MAX
            },
        );
        (cluster, cell_id)
    }
}

/// Extent of one packed block, for the limit check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockExtent {
    pub cell_id: u32,
    pub width: u32,
    pub height: u32,
    /// Face whose chart is the cell's largest, for the error message.
    pub largest_chart_face: usize,
}

/// The one chokepoint for the runtime's block limits: a vertex names a block
/// as `id + 1` in a `u16`, and every block must fit one pool layer.
pub(crate) fn check_block_limits(
    block_count: usize,
    extents: impl IntoIterator<Item = BlockExtent>,
) -> Result<(), LightmapBakeError> {
    if block_count > MAX_LIGHTMAP_BLOCKS as usize {
        return Err(LightmapBakeError::BlockCountOverflow {
            count: block_count,
            max: MAX_LIGHTMAP_BLOCKS,
        });
    }
    for extent in extents {
        if extent.width > LIGHTMAP_POOL_LAYER_EDGE || extent.height > LIGHTMAP_POOL_LAYER_EDGE {
            return Err(LightmapBakeError::BlockTooLarge {
                cell_id: extent.cell_id,
                width: extent.width,
                height: extent.height,
                max: LIGHTMAP_POOL_LAYER_EDGE,
                largest_chart_face: extent.largest_chart_face,
            });
        }
    }
    Ok(())
}

/// Face of the largest chart among `members` by texel area, the lowest face
/// on ties: the face an oversize-block error names.
fn largest_chart_face(charts: &[Chart], members: &[usize]) -> usize {
    members
        .iter()
        .copied()
        .max_by_key(|&i| {
            (
                u64::from(charts[i].width_texels) * u64::from(charts[i].height_texels),
                std::cmp::Reverse(i),
            )
        })
        .unwrap_or(0)
}

/// Charts packed into cell blocks, and the blocks packed into bake layers.
#[derive(Debug)]
pub(crate) struct BlockedPack {
    pub layout: BlockLayout,
    /// Bake-layer placement of each chart, parallel to the charts.
    pub placements: Vec<ChartPlacement>,
    pub layer_dim: u32,
    pub layer_count: u32,
}

/// Pack each cell's charts into one or more blocks
/// (`pack_cell_blocks_within`), order blocks cluster-major (cluster, then cell
/// id, then sub-block), reject what the runtime cannot hold, and pack blocks
/// in block order into uniform bake layers so the per-layer bake, shadowmask
/// fill, and cache partitions keep their layer loops. A cell's blocks are
/// therefore contiguous in block-id order. Callers reject charts past a pool
/// layer first (`check_chart_extents`).
#[cfg(test)]
pub(crate) fn pack_cell_blocks(
    charts: &[Chart],
    ordering: BlockOrdering<'_>,
    control: &BakeControl,
) -> Result<BlockedPack, LightmapBakeError> {
    pack_cell_blocks_within(charts, ordering, LIGHTMAP_POOL_LAYER_EDGE, control)
}

/// [`pack_cell_blocks`] against `pool_edge`: production passes the runtime's
/// pool layer edge, tests a smaller one to drive multi-block cells cheaply.
pub(crate) fn pack_cell_blocks_within(
    charts: &[Chart],
    ordering: BlockOrdering<'_>,
    pool_edge: u32,
    control: &BakeControl,
) -> Result<BlockedPack, LightmapBakeError> {
    let direction_texel_scale = normalized_direction_texel_scale(ordering.direction_texel_scale);
    let alignment = block_alignment(direction_texel_scale);

    // Charts of each cell in chart (face) order.
    let mut by_cell: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (index, chart) in charts.iter().enumerate() {
        by_cell.entry(chart.leaf_index).or_default().push(index);
    }
    let mut cells: Vec<(u32, Vec<usize>)> = by_cell.into_iter().collect();
    cells.sort_by_key(|(cell_id, _)| ordering.sort_key(*cell_id));

    let per_cell: Vec<Vec<CellSubBlock>> = cells
        .par_iter()
        .map(|(_, members)| {
            let _permit = control.governor().enter();
            let sizes: Vec<(u32, u32)> = members
                .iter()
                .map(|&i| (charts[i].width_texels, charts[i].height_texels))
                .collect();
            pack_cell_blocks_within_cell(&sizes, alignment, pool_edge)
        })
        .collect();

    // Flatten to block-id order; members become chart indices.
    let packed: Vec<(u32, Vec<usize>, PackedBlock)> = cells
        .iter()
        .zip(per_cell)
        .flat_map(|((cell_id, members), sub_blocks)| {
            sub_blocks.into_iter().map(move |sub| {
                let charts_of_block = sub.members.iter().map(|&m| members[m]).collect();
                (*cell_id, charts_of_block, sub.block)
            })
        })
        .collect();

    check_block_limits(
        packed.len(),
        packed.iter().map(|(cell_id, members, block)| BlockExtent {
            cell_id: *cell_id,
            width: block.width,
            height: block.height,
            largest_chart_face: largest_chart_face(charts, members),
        }),
    )?;

    let extents: Vec<(u32, u32)> = packed.iter().map(|(_, _, b)| (b.width, b.height)).collect();
    let singles: Vec<Vec<usize>> = (0..packed.len()).map(|i| vec![i]).collect();
    let layers = pack_groups_into_layers(&extents, &singles, MAX_ATLAS_DIMENSION, MAX_ATLAS_LAYERS)
        .map_err(|error| match error {
            GroupPackError::LayerOverflow { layer_count, max } => {
                LightmapBakeError::LayerOverflow { layer_count, max }
            }
            // Unreachable after the pool-layer check: a block is at most one
            // pool layer, which is smaller than a bake layer's cap.
            GroupPackError::GroupTooLarge { group } => LightmapBakeError::BlockTooLarge {
                cell_id: packed[group].0,
                width: packed[group].2.width,
                height: packed[group].2.height,
                max: MAX_ATLAS_DIMENSION,
                largest_chart_face: largest_chart_face(charts, &packed[group].1),
            },
        })?;

    let mut blocks = Vec::with_capacity(packed.len());
    let mut chart_blocks = vec![0u32; charts.len()];
    let mut placements = vec![
        ChartPlacement {
            x: 0,
            y: 0,
            layer: 0,
        };
        charts.len()
    ];
    for (block_id, ((cell_id, members, block), origin)) in
        packed.iter().zip(&layers.placements).enumerate()
    {
        assert!(
            origin.x % alignment == 0 && origin.y % alignment == 0,
            "cell {cell_id} block origin ({}, {}) is not a multiple of {alignment}",
            origin.x,
            origin.y
        );
        blocks.push(CellBlock {
            cell_id: *cell_id,
            width: block.width,
            height: block.height,
            layer: origin.layer,
            x: origin.x,
            y: origin.y,
        });
        for (&chart, &(x, y)) in members.iter().zip(&block.placements) {
            chart_blocks[chart] = block_id as u32;
            placements[chart] = ChartPlacement {
                x: origin.x + x,
                y: origin.y + y,
                layer: origin.layer,
            };
        }
    }

    Ok(BlockedPack {
        layout: BlockLayout {
            direction_texel_scale,
            blocks,
            chart_blocks,
        },
        placements,
        layer_dim: layers.dim,
        layer_count: layers.layer_count,
    })
}

#[cfg(test)]
mod tests;
