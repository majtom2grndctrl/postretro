//! Cell blocks: each cell's charts packed into one tight BC-aligned rectangle
//! the runtime would allocate, free, and remap with a single UV translation.
//!
//! A block is costed as its own `width × height` layer region at the stored
//! encodings (`AtlasFormats::layer_bytes_at`), so the doubled-width id 42 is
//! charged exactly as a layer of that width would carry it.

use rayon::prelude::*;

use super::{BC_BLOCK_EDGE, ChartRect, DryRunInput};
use crate::lightmap_bake::MaxRects;

/// Edge of one runtime pool layer, in irradiance texels.
pub(crate) const POOL_LAYER_EDGE: u32 = 2048;

/// Candidate block widths as multiples of `sqrt(chart area)`, each floored at
/// the widest chart.
const WIDTH_FACTORS: [f64; 8] = [0.5, 0.7, 0.85, 1.0, 1.15, 1.3, 1.6, 2.0];

/// Widths tried per cell: every factor, the widest chart alone, and the pool
/// layer edge.
pub(crate) const CANDIDATE_WIDTHS: usize = WIDTH_FACTORS.len() + 2;

/// One cell's packed block, in irradiance texels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackedBlock {
    pub width: u32,
    pub height: u32,
    /// Top-left of each input chart inside the block, in input order.
    pub placements: Vec<(u32, u32)>,
}

impl PackedBlock {
    pub(crate) fn area(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
}

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
        let dims: Vec<Option<BlockDims>> = per_cell
            .par_iter()
            .map(|charts| {
                pack_cell_block(charts).map(|block| BlockDims {
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

fn align_up(value: u32) -> u32 {
    value.div_ceil(BC_BLOCK_EDGE) * BC_BLOCK_EDGE
}

/// Pack one cell's padded charts into the smallest-area BC-aligned block
/// found, preferring one that fits a pool layer: for each candidate width, the shortest 4-aligned height the bake's
/// MaxRects (largest-first, as `place_leaf` orders a leaf) fits every chart
/// into. `None` for a cell without charts.
pub(crate) fn pack_cell_block(charts: &[ChartRect]) -> Option<PackedBlock> {
    if charts.is_empty() {
        return None;
    }
    let mut order: Vec<usize> = (0..charts.len()).collect();
    order.sort_by(|&a, &b| charts[b].area().cmp(&charts[a].area()).then(a.cmp(&b)));
    let area: u64 = charts.iter().map(ChartRect::area).sum();
    let max_width = charts.iter().map(|c| c.width).max().unwrap_or(0);
    let max_height = charts.iter().map(|c| c.height).max().unwrap_or(0);
    let side = (area as f64).sqrt();

    let mut widths: Vec<u32> = WIDTH_FACTORS
        .iter()
        .map(|&f| align_up(((side * f).ceil() as u32).max(max_width)))
        .collect();
    widths.push(align_up(max_width));
    if align_up(max_width) <= POOL_LAYER_EDGE {
        widths.push(POOL_LAYER_EDGE);
    }
    widths.sort_unstable();
    widths.dedup();

    let mut best: Option<PackedBlock> = None;
    for width in widths {
        let floor = align_up(max_height.max(area.div_ceil(u64::from(width)) as u32));
        let block = shortest_block(charts, &order, width, floor);
        // A block that fits a pool layer beats any that does not; then the
        // smaller area, then the squarer shape.
        let key = |b: &PackedBlock| {
            let dims = BlockDims {
                width: b.width,
                height: b.height,
            };
            (!dims.fits_pool_layer(), b.area(), b.width.max(b.height))
        };
        let better = best.as_ref().is_none_or(|b| key(&block) < key(b));
        if better {
            best = Some(block);
        }
    }
    best
}

/// Shortest 4-aligned height at `width` that packs every chart, searched
/// upward geometrically from `floor` and then bisected.
fn shortest_block(charts: &[ChartRect], order: &[usize], width: u32, floor: u32) -> PackedBlock {
    let mut fail = None;
    let mut height = floor;
    let mut placed = loop {
        match try_pack(charts, order, width, height) {
            Some(placements) => break placements,
            None => {
                fail = Some(height);
                height = align_up(height + (height / 8).max(BC_BLOCK_EDGE));
            }
        }
    };
    if let Some(mut low) = fail {
        // `low` fails and `height` packs, both 4-aligned; bisect on the
        // 4-texel grid. MaxRects fit is not strictly monotone in height, so
        // this finds a short packing height, not provably the shortest.
        while height - low > BC_BLOCK_EDGE {
            let mid = align_up(low + (height - low) / 2);
            match try_pack(charts, order, width, mid) {
                Some(placements) => {
                    height = mid;
                    placed = placements;
                }
                None => low = mid,
            }
        }
    }
    PackedBlock {
        width,
        height,
        placements: placed,
    }
}

fn try_pack(
    charts: &[ChartRect],
    order: &[usize],
    width: u32,
    height: u32,
) -> Option<Vec<(u32, u32)>> {
    let mut bin = MaxRects::new(width, height);
    let mut placements = vec![(0, 0); charts.len()];
    for &index in order {
        placements[index] = bin.insert(charts[index].width, charts[index].height)?;
    }
    Some(placements)
}
