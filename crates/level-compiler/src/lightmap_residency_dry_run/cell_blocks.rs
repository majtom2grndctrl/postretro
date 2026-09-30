//! Cell blocks: each cell's charts packed into one tight aligned rectangle
//! the runtime would allocate, free, and remap with a single UV translation.
//! Packing is the bake's own `pack_cell_block`.
//!
//! A block is costed as its own `width × height` layer region at the stored
//! encodings (`AtlasFormats::layer_bytes_at`).

use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;
use rayon::prelude::*;

use super::{ChartRect, DryRunInput};
pub(crate) use crate::lightmap_bake::CANDIDATE_WIDTHS;
use crate::lightmap_bake::pack_cell_block;

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

/// Every cell's block and the whole-map packing overhead.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CellBlocks {
    /// Per cell id; `None` for a cell with no charts.
    pub dims: Vec<Option<BlockDims>>,
    /// Id 22 + id 42 bytes of each cell's block; zero without charts.
    pub block_bytes: Vec<u64>,
    /// Padded chart area of each cell.
    pub chart_texels: Vec<u64>,
    /// Widest and tallest padded chart of each cell.
    pub largest_chart: Vec<(u32, u32)>,
    /// Id 22 + id 42 bytes of one `POOL_LAYER_EDGE²` pool layer.
    pub pool_layer_bytes: u64,
    /// Cells whose recovered charts sit on more than one stored layer.
    pub multi_layer_cells: usize,
    /// Texel grid every block extent sits on (`AtlasFormats::block_alignment`).
    pub alignment: u32,
}

impl CellBlocks {
    pub(crate) fn new(input: &DryRunInput) -> Self {
        let mut per_cell: Vec<Vec<ChartRect>> = vec![Vec::new(); input.cell_count()];
        for chart in &input.charts {
            per_cell[chart.cell as usize].push(*chart);
        }
        let multi_layer_cells = per_cell
            .iter()
            .filter(|charts| charts.iter().any(|c| c.layer != charts[0].layer))
            .count();
        let alignment = input.formats.block_alignment();
        let dims: Vec<Option<BlockDims>> = per_cell
            .par_iter()
            .map(|charts| {
                let sizes: Vec<(u32, u32)> = charts.iter().map(|c| (c.width, c.height)).collect();
                pack_cell_block(&sizes, alignment).map(|block| BlockDims {
                    width: block.width,
                    height: block.height,
                })
            })
            .collect();
        let block_bytes = dims
            .iter()
            .map(|dims| dims.map_or(0, |d| input.formats.layer_bytes_at(d.width, d.height)))
            .collect();
        Self {
            dims,
            block_bytes,
            chart_texels: per_cell
                .iter()
                .map(|charts| charts.iter().map(ChartRect::area).sum())
                .collect(),
            largest_chart: per_cell
                .iter()
                .map(|charts| {
                    (
                        charts.iter().map(|c| c.width).max().unwrap_or(0),
                        charts.iter().map(|c| c.height).max().unwrap_or(0),
                    )
                })
                .collect(),
            pool_layer_bytes: input
                .formats
                .layer_bytes_at(POOL_LAYER_EDGE, POOL_LAYER_EDGE),
            multi_layer_cells,
            alignment,
        }
    }

    /// Bytes of a pool of `layers` `POOL_LAYER_EDGE²` layers.
    pub(crate) fn pool_bytes(&self, layers: u32) -> u64 {
        u64::from(layers) * self.pool_layer_bytes
    }

    /// Block bytes resident when `cells`, distinct, are mandatory.
    pub(crate) fn set_bytes(&self, cells: &[u32]) -> u64 {
        cells.iter().map(|&c| self.block_bytes[c as usize]).sum()
    }

    /// Dims of the blocks `cells` own, skipping chartless cells.
    pub(crate) fn set_dims<'a>(
        &'a self,
        cells: &'a [u32],
    ) -> impl Iterator<Item = (u32, BlockDims)> + 'a {
        cells
            .iter()
            .filter_map(|&c| self.dims[c as usize].map(|d| (c, d)))
    }

    pub(crate) fn overhead(&self) -> PackingOverhead {
        let mut ratios: Vec<(f64, u32)> = Vec::new();
        let mut chart_texels = 0u64;
        let mut block_texels = 0u64;
        let mut largest: Option<(u32, BlockDims)> = None;
        let mut over_pool_edge = Vec::new();
        for (cell, dims) in self.dims.iter().enumerate() {
            let Some(dims) = *dims else { continue };
            let charts = self.chart_texels[cell];
            chart_texels += charts;
            block_texels += dims.area();
            ratios.push((dims.area() as f64 / charts.max(1) as f64, cell as u32));
            if !dims.fits_pool_layer() {
                over_pool_edge.push((cell as u32, dims));
            }
            if largest.is_none_or(|(_, best)| dims.area() > best.area()) {
                largest = Some((cell as u32, dims));
            }
        }
        ratios.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        PackingOverhead {
            blocks: ratios.len(),
            chart_texels,
            block_texels,
            ratios,
            largest,
            over_pool_edge,
        }
    }
}

/// Block texels over chart texels, whole map and per cell.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PackingOverhead {
    pub blocks: usize,
    pub chart_texels: u64,
    pub block_texels: u64,
    /// `(block / chart texels, cell)`, largest first.
    pub ratios: Vec<(f64, u32)>,
    /// Largest block by area.
    pub largest: Option<(u32, BlockDims)>,
    /// Blocks wider or taller than one pool layer, ascending by cell.
    pub over_pool_edge: Vec<(u32, BlockDims)>,
}
