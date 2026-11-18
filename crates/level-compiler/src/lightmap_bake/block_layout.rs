// Lightmap cell-block layout: cluster-major block order, per-cell packing, runtime limits, bake layers.
// See: context/lib/build_pipeline.md §PRL section IDs (Lightmap id 22)

use std::collections::BTreeMap;

#[cfg(test)]
use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;
use postretro_level_format::lightmap::{LightmapHeader, LightmapMode, MAX_LIGHTMAP_BLOCKS};
use rayon::prelude::*;

use super::atlas_pack::{GroupPackError, pack_groups_into_layers};
use super::cell_blocks::{CellPack, PackedBlock, pack_cell_sub_blocks};
use super::charts::Chart;
use super::encode::normalized_direction_texel_scale;
use super::pack_heuristics::{PACK_HEURISTICS, PackHeuristic, PackRules};
use super::{LightmapBakeError, MAX_ATLAS_DIMENSION};
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

/// What orders, aligns and packs blocks: the direction scale, each runtime
/// cell's id-49 cluster, and the packing heuristics.
#[derive(Debug, Clone, Copy)]
pub struct BlockOrdering<'a> {
    /// Configured scale; normalized to a power of two before use.
    pub direction_texel_scale: u32,
    /// Cluster of each runtime cell, indexed by cell id. Empty orders blocks
    /// by cell id alone (tests without a partition).
    pub cell_clusters: &'a [u32],
    /// `Some` packs every cell with this one heuristic (`--pack-heuristic`);
    /// `None` tries them all per cell and keeps the smallest.
    pub heuristic: Option<PackHeuristic>,
}

impl BlockOrdering<'_> {
    pub fn by_cell_id(direction_texel_scale: u32) -> BlockOrdering<'static> {
        BlockOrdering {
            direction_texel_scale,
            cell_clusters: &[],
            heuristic: None,
        }
    }

    fn heuristics(&self) -> &[PackHeuristic] {
        match &self.heuristic {
            Some(forced) => std::slice::from_ref(forced),
            None => &PACK_HEURISTICS,
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

/// The one chokepoint for the runtime's block-count limit: a vertex names a
/// block as `id + 1` in a `u16`. Block extents need no check: the
/// oversize-face cut bounds every chart, and cell packing every block, by the
/// pool layer edge.
pub(crate) fn check_block_limits(block_count: usize) -> Result<(), LightmapBakeError> {
    if block_count > MAX_LIGHTMAP_BLOCKS as usize {
        return Err(LightmapBakeError::BlockCountOverflow {
            count: block_count,
            max: MAX_LIGHTMAP_BLOCKS,
        });
    }
    Ok(())
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
/// therefore contiguous in block-id order. Every chart must fit the pool
/// layer edge (the oversize-face cut).
///
/// Charts may be placed transposed, and every chart origin is a multiple of
/// the direction scale, so the direction plane's reduction groups the same
/// chart texels wherever a chart lands.
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
    let rules = PackRules::lightmap(alignment, direction_texel_scale);
    let heuristics = ordering.heuristics();

    // Charts of each cell in chart (face) order.
    let mut by_cell: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (index, chart) in charts.iter().enumerate() {
        by_cell.entry(chart.leaf_index).or_default().push(index);
    }
    let mut cells: Vec<(u32, Vec<usize>)> = by_cell.into_iter().collect();
    cells.sort_by_key(|(cell_id, _)| ordering.sort_key(*cell_id));

    let per_cell: Vec<CellPack> = cells
        .par_iter()
        .map(|(_, members)| {
            let _permit = control.governor().enter();
            let sizes: Vec<(u32, u32)> = members
                .iter()
                .map(|&i| (charts[i].width_texels, charts[i].height_texels))
                .collect();
            pack_cell_sub_blocks(&sizes, rules, pool_edge, heuristics)
        })
        .collect();
    log_pack_choices(&per_cell, heuristics);

    // Flatten to block-id order; members become chart indices.
    let packed: Vec<(u32, Vec<usize>, PackedBlock)> = cells
        .iter()
        .zip(per_cell)
        .flat_map(|((cell_id, members), pack)| {
            pack.blocks.into_iter().map(move |sub| {
                let charts_of_block = sub.members.iter().map(|&m| members[m]).collect();
                (*cell_id, charts_of_block, sub.block)
            })
        })
        .collect();

    check_block_limits(packed.len())?;

    let extents: Vec<(u32, u32)> = packed.iter().map(|(_, _, b)| (b.width, b.height)).collect();
    let singles: Vec<Vec<usize>> = (0..packed.len()).map(|i| vec![i]).collect();
    let layers = pack_groups_into_layers(&extents, &singles, MAX_ATLAS_DIMENSION, u32::MAX)
        .map_err(|error| match error {
            GroupPackError::LayerOverflow { layer_count, max } => {
                LightmapBakeError::LayerOverflow { layer_count, max }
            }
            // Cell packing bounds every block by the pool layer edge, which
            // never exceeds a bake layer's cap.
            GroupPackError::GroupTooLarge { group } => {
                unreachable!("block {group} exceeds an empty bake layer")
            }
        })?;

    let mut blocks = Vec::with_capacity(packed.len());
    let mut chart_blocks = vec![0u32; charts.len()];
    let mut placements = vec![ChartPlacement::at(0, 0, 0); charts.len()];
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
        for (&chart, slot) in members.iter().zip(&block.placements) {
            chart_blocks[chart] = block_id as u32;
            placements[chart] = ChartPlacement {
                x: origin.x + slot.x,
                y: origin.y + slot.y,
                layer: origin.layer,
                transposed: slot.transposed,
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

/// One line on how the cells packed: charts placed transposed, and how many
/// cells each heuristic won.
fn log_pack_choices(per_cell: &[CellPack], heuristics: &[PackHeuristic]) {
    let mut wins = vec![0usize; heuristics.len()];
    let (mut charts, mut transposed) = (0usize, 0usize);
    for pack in per_cell {
        wins[pack.heuristic] += 1;
        for sub in &pack.blocks {
            charts += sub.block.placements.len();
            transposed += sub
                .block
                .placements
                .iter()
                .filter(|slot| slot.transposed)
                .count();
        }
    }
    let wins: Vec<String> = heuristics
        .iter()
        .zip(&wins)
        .filter(|&(_, &count)| count > 0)
        .map(|(heuristic, count)| format!("{} {count}", heuristic.name()))
        .collect();
    log::info!(
        "[Compiler] lightmap packing: {transposed} of {charts} chart(s) transposed; cells won by heuristic: {}",
        wins.join(", ")
    );
}

#[cfg(test)]
mod tests;
