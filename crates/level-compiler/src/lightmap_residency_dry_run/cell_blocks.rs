//! Cell blocks: each cell's charts packed into the blocks the runtime would
//! allocate, free, and remap with a single UV translation each. Packing is
//! the bake's own `pack_cell_sub_blocks` at the pool layer edge: a cell
//! that fits one pool layer owns one tight block, a larger cell several, so
//! every block fits a pool layer.
//!
//! A block is costed as its own `width × height` region at the stored
//! encodings, each plane it stores charged (`AtlasFormats::cell_block_bytes`).
//! A repacked block stores a plane when any stored block its charts came from
//! does.

use std::ops::Range;

use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;
use postretro_level_format::lightmap_pool::{
    MASK_POOL_LAYER_BYTES, PackScratch, PerPool, PlanePresence, PoolDemand, PoolKind,
    streamed_pool_bytes,
};
use rayon::prelude::*;

use super::{ChartRect, DryRunInput};
pub(crate) use crate::lightmap_bake::CANDIDATE_WIDTHS;
use crate::lightmap_bake::pack_cell_sub_blocks;

/// Edge of one runtime pool layer, in irradiance texels.
pub(crate) const POOL_LAYER_EDGE: u32 = LIGHTMAP_POOL_LAYER_EDGE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockDims {
    pub width: u32,
    pub height: u32,
}

impl BlockDims {
    pub(crate) fn area(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    pub(crate) fn fits_pool_layer(self) -> bool {
        self.width <= POOL_LAYER_EDGE && self.height <= POOL_LAYER_EDGE
    }
}

/// Every cell's blocks and the whole-map packing overhead.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CellBlocks {
    /// Every block, cell-major (cell id, then sub-block), so a cell's blocks
    /// are contiguous as they are in the stored layout.
    pub dims: Vec<BlockDims>,
    /// Id 22 + id 42 bytes of each block, parallel to `dims`.
    pub block_bytes: Vec<u64>,
    /// What the blocks ask of the two pools, as the runtime counts it.
    pub demand: PoolDemand,
    /// Owning cell of each block, parallel to `dims`.
    pub cell_of_block: Vec<u32>,
    /// Blocks of each cell id; empty for a cell with no charts.
    pub cell_blocks: Vec<Range<u32>>,
    /// Padded chart area of each cell.
    pub chart_texels: Vec<u64>,
    /// Bytes of one `POOL_LAYER_EDGE²` layer of each pool.
    pub pool_layer_bytes: PerPool<u64>,
    /// Cells whose recovered charts sit on more than one stored block.
    pub cells_spanning_stored_blocks: usize,
    /// Texel grid every block extent sits on (`AtlasFormats::block_alignment`).
    pub alignment: u32,
}

impl CellBlocks {
    pub(crate) fn new(input: &DryRunInput) -> Self {
        let mut per_cell: Vec<Vec<ChartRect>> = vec![Vec::new(); input.cell_count()];
        for chart in &input.charts {
            per_cell[chart.cell as usize].push(*chart);
        }
        let cells_spanning_stored_blocks = per_cell
            .iter()
            .filter(|charts| charts.iter().any(|c| c.layer != charts[0].layer))
            .count();
        let alignment = input.formats.block_alignment();
        let formats = &input.formats;
        let packed: Vec<Vec<(BlockDims, PlanePresence)>> = per_cell
            .par_iter()
            .map(|charts| {
                let sizes: Vec<(u32, u32)> = charts.iter().map(|c| (c.width, c.height)).collect();
                pack_cell_sub_blocks(&sizes, alignment, POOL_LAYER_EDGE)
                    .into_iter()
                    .map(|sub| {
                        let dims = BlockDims {
                            width: sub.block.width,
                            height: sub.block.height,
                        };
                        let mut planes = [false; 2];
                        for &member in &sub.members {
                            let stored = formats.stored_planes(charts[member].layer as usize);
                            planes = [planes[0] || stored[0], planes[1] || stored[1]];
                        }
                        (dims, planes)
                    })
                    .collect()
            })
            .collect();
        let mut dims = Vec::new();
        let mut planes = Vec::new();
        let mut cell_of_block = Vec::new();
        let mut cell_blocks = Vec::with_capacity(packed.len());
        for (cell, blocks) in packed.into_iter().enumerate() {
            let start = dims.len() as u32;
            cell_of_block.extend(std::iter::repeat_n(cell as u32, blocks.len()));
            for (block_dims, block_planes) in blocks {
                dims.push(block_dims);
                planes.push(block_planes);
            }
            cell_blocks.push(start..dims.len() as u32);
        }
        let block_bytes = dims
            .iter()
            .zip(&planes)
            .map(|(d, &planes)| formats.cell_block_bytes(d.width, d.height, planes))
            .collect();
        let extents = dims.iter().map(|d| (d.width, d.height)).collect();
        let demand = PoolDemand::new(
            alignment,
            extents,
            formats.stored_shadowmask().map(|_| planes),
        );
        Self {
            dims,
            block_bytes,
            demand,
            cell_of_block,
            cell_blocks,
            chart_texels: per_cell
                .iter()
                .map(|charts| charts.iter().map(ChartRect::area).sum())
                .collect(),
            pool_layer_bytes: PerPool {
                lightmap: formats.lightmap_bytes_at(POOL_LAYER_EDGE, POOL_LAYER_EDGE),
                mask: MASK_POOL_LAYER_BYTES,
            },
            cells_spanning_stored_blocks,
            alignment,
        }
    }

    /// Blocks `cell` owns; empty for a cell with no charts.
    pub(crate) fn blocks_of_cell(&self, cell: u32) -> Range<u32> {
        self.cell_blocks[cell as usize].clone()
    }

    /// Dims of every block `cell` owns, in block order.
    pub(crate) fn cell_dims(&self, cell: u32) -> &[BlockDims] {
        let blocks = self.blocks_of_cell(cell);
        &self.dims[blocks.start as usize..blocks.end as usize]
    }

    /// Bytes of a lightmap pool of `layers` `POOL_LAYER_EDGE²` layers, no spare:
    /// the single-pool walks' figure.
    pub(crate) fn lightmap_pool_bytes(&self, layers: u32) -> u64 {
        u64::from(layers) * self.pool_layer_bytes.lightmap
    }

    /// Both streamed pools' bytes at `layers`, each with its spare.
    pub(crate) fn streamed_pools_bytes(&self, layers: PerPool<u32>) -> u64 {
        PoolKind::ALL
            .iter()
            .map(|&kind| streamed_pool_bytes(layers[kind], self.pool_layer_bytes[kind]))
            .sum()
    }

    /// Layers `cells`, distinct, need in each pool, packed from scratch as the
    /// bound and the runtime pack.
    pub(crate) fn set_pool_layers(
        &self,
        cells: &[u32],
        blocks: &mut Vec<u32>,
        scratch: &mut PackScratch,
    ) -> PerPool<u32> {
        blocks.clear();
        blocks.extend(self.set_blocks(cells));
        self.demand
            .layers_of(blocks, POOL_LAYER_EDGE, scratch)
            .expect("cell packing bounds every block by the pool edge")
    }

    /// Every block `cells`, distinct, own.
    pub(crate) fn set_blocks<'a>(&'a self, cells: &'a [u32]) -> impl Iterator<Item = u32> + 'a {
        cells.iter().flat_map(|&c| self.blocks_of_cell(c))
    }

    /// Block bytes resident when `cells`, distinct, are mandatory.
    pub(crate) fn set_bytes(&self, cells: &[u32]) -> u64 {
        self.set_blocks(cells)
            .map(|block| self.block_bytes[block as usize])
            .sum()
    }

    /// Dims of every block `cells` own, keyed by block id.
    pub(crate) fn set_dims<'a>(
        &'a self,
        cells: &'a [u32],
    ) -> impl Iterator<Item = (u32, BlockDims)> + 'a {
        self.set_blocks(cells)
            .map(|block| (block, self.dims[block as usize]))
    }

    pub(crate) fn overhead(&self) -> PackingOverhead {
        let mut ratios: Vec<(f64, u32)> = Vec::new();
        let mut chart_texels = 0u64;
        let mut multi_block_cells = Vec::new();
        for (cell, blocks) in self.cell_blocks.iter().enumerate() {
            if blocks.is_empty() {
                continue;
            }
            let charts = self.chart_texels[cell];
            let texels: u64 = self.cell_dims(cell as u32).iter().map(|d| d.area()).sum();
            chart_texels += charts;
            ratios.push((texels as f64 / charts.max(1) as f64, cell as u32));
            if blocks.len() > 1 {
                multi_block_cells.push(cell as u32);
            }
        }
        let mut largest: Option<(u32, BlockDims)> = None;
        for (&dims, &cell) in self.dims.iter().zip(&self.cell_of_block) {
            if largest.is_none_or(|(_, best)| dims.area() > best.area()) {
                largest = Some((cell, dims));
            }
        }
        ratios.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        PackingOverhead {
            blocks: self.dims.len(),
            chart_texels,
            block_texels: self.dims.iter().map(|d| d.area()).sum(),
            ratios,
            largest,
            multi_block_cells,
        }
    }
}

/// Block texels over chart texels, whole map and per cell.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PackingOverhead {
    pub blocks: usize,
    pub chart_texels: u64,
    pub block_texels: u64,
    /// `(cell's block texels / its chart texels, cell)`, largest first.
    pub ratios: Vec<(f64, u32)>,
    /// Largest block by area, and its cell.
    pub largest: Option<(u32, BlockDims)>,
    /// Cells whose charts do not pack into one pool layer, so own several
    /// blocks, ascending.
    pub multi_block_cells: Vec<u32>,
}
