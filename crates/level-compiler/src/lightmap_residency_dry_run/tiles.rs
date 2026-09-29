//! Fixed-size tile residency: each owner unit's charts packed into its own
//! P×P tiles, so a tile is resident exactly when its owner is mandatory.
//!
//! Charts keep their padding and never straddle tiles; a chart wider or taller
//! than P gets a dedicated block of `ceil(w/P) × ceil(h/P)` tiles. The virtual
//! tile count is the page table's size, unbounded by the layer limit.

use super::{BC_BLOCK_EDGE, ChartRect, DryRunInput};
use crate::chart_raster::CHART_PADDING_TEXELS;
use crate::lightmap_bake::MaxRects;

/// Tile edges measured, in irradiance texels.
pub(crate) const TILE_SIZES: [u32; 3] = [128, 256, 512];

/// What one tile's residency is keyed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TileUnit {
    Cell,
    /// A Cluster (id 49): a tile is resident when any member cell is mandatory.
    Cluster,
}

impl TileUnit {
    pub(crate) const ALL: [TileUnit; 2] = [TileUnit::Cell, TileUnit::Cluster];

    pub(crate) fn label(self) -> &'static str {
        match self {
            TileUnit::Cell => "cell",
            TileUnit::Cluster => "cluster",
        }
    }
}

/// Charts that cannot fit one tile at a given P.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct OversizeCharts {
    pub charts: usize,
    /// Padded chart area of those charts.
    pub chart_texels: u64,
    /// Dedicated tiles they occupy.
    pub tiles: u64,
    /// Of `charts`, those whose unpadded interior fits P: the padding alone
    /// pushes them over.
    pub interior_fits: usize,
}

/// Tiles one unit's charts occupy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct UnitTiles {
    /// All tiles, oversize blocks included.
    pub tiles: u64,
    pub oversize: OversizeCharts,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TileLayout {
    pub unit: TileUnit,
    pub tile_size: u32,
    /// Owner unit of each cell: the cell itself, or its cluster.
    unit_of_cell: Vec<u32>,
    /// Tiles each unit owns; zero for a unit with no charts.
    pub unit_tiles: Vec<u64>,
    /// Id 22 + id 42 bytes of one tile.
    pub tile_bytes: u64,
    pub oversize: OversizeCharts,
    /// Padded chart area over the whole map, the texel-exact denominator.
    pub chart_texels: u64,
}

impl TileLayout {
    pub(crate) fn virtual_tiles(&self) -> u64 {
        self.unit_tiles.iter().sum()
    }

    fn tile_area(&self) -> u64 {
        u64::from(self.tile_size) * u64::from(self.tile_size)
    }

    /// Tile texels per texel-exact chart texel over the whole map.
    pub(crate) fn overhead(&self) -> f64 {
        (self.virtual_tiles() * self.tile_area()) as f64 / self.chart_texels.max(1) as f64
    }

    /// `overhead` restricted to oversize charts and their dedicated tiles.
    pub(crate) fn oversize_overhead(&self) -> f64 {
        (self.oversize.tiles * self.tile_area()) as f64 / self.oversize.chart_texels.max(1) as f64
    }

    /// `overhead` restricted to charts that fit a tile and the tiles they share.
    pub(crate) fn shared_overhead(&self) -> f64 {
        let tiles = self.virtual_tiles() - self.oversize.tiles;
        let texels = self.chart_texels - self.oversize.chart_texels;
        (tiles * self.tile_area()) as f64 / texels.max(1) as f64
    }

    /// Resident bytes when the units owning `cells` are resident.
    /// Duplicate cells, and cells sharing a unit, count their unit once.
    pub(crate) fn mandatory_bytes(&self, cells: &[u32], stamp: &mut UnitStamp) -> u64 {
        stamp.begin(self.unit_tiles.len());
        let tiles: u64 = cells
            .iter()
            .map(|&cell| self.unit_of_cell[cell as usize])
            .filter(|&unit| stamp.first_visit(unit))
            .map(|unit| self.unit_tiles[unit as usize])
            .sum();
        tiles * self.tile_bytes
    }
}

/// Reusable per-set visit marks over units.
#[derive(Debug, Default)]
pub(crate) struct UnitStamp {
    marks: Vec<u32>,
    generation: u32,
}

impl UnitStamp {
    fn begin(&mut self, unit_count: usize) {
        if self.marks.len() != unit_count {
            self.marks = vec![0; unit_count];
            self.generation = 0;
        }
        self.generation += 1;
    }

    fn first_visit(&mut self, unit: u32) -> bool {
        let mark = &mut self.marks[unit as usize];
        let first = *mark != self.generation;
        *mark = self.generation;
        first
    }
}

/// Every (unit, P) tile layout, units outer.
pub(crate) fn tile_layouts(input: &DryRunInput) -> Vec<TileLayout> {
    TileUnit::ALL
        .iter()
        .flat_map(|&unit| TILE_SIZES.map(|tile_size| tile_layout(input, unit, tile_size)))
        .collect()
}

pub(crate) fn tile_layout(input: &DryRunInput, unit: TileUnit, tile_size: u32) -> TileLayout {
    // Tile origins sit on multiples of P in the virtual page grid, so a
    // 4-aligned P keeps every tile on the BC block grid.
    assert_eq!(
        tile_size % BC_BLOCK_EDGE,
        0,
        "tile size {tile_size} breaks BC block alignment"
    );
    let unit_of_cell: Vec<u32> = match unit {
        TileUnit::Cell => (0..input.cell_count() as u32).collect(),
        TileUnit::Cluster => input.cells.iter().map(|info| info.cluster).collect(),
    };
    let unit_count = match unit {
        TileUnit::Cell => input.cell_count(),
        TileUnit::Cluster => input.cluster_count as usize,
    };
    let mut unit_charts: Vec<Vec<ChartRect>> = vec![Vec::new(); unit_count];
    for chart in &input.charts {
        unit_charts[unit_of_cell[chart.cell as usize] as usize].push(*chart);
    }
    let packed: Vec<UnitTiles> = unit_charts
        .iter()
        .map(|charts| pack_unit_tiles(charts, tile_size))
        .collect();
    let oversize = packed
        .iter()
        .fold(OversizeCharts::default(), |sum, unit| OversizeCharts {
            charts: sum.charts + unit.oversize.charts,
            chart_texels: sum.chart_texels + unit.oversize.chart_texels,
            tiles: sum.tiles + unit.oversize.tiles,
            interior_fits: sum.interior_fits + unit.oversize.interior_fits,
        });
    TileLayout {
        unit,
        tile_size,
        unit_of_cell,
        unit_tiles: packed.iter().map(|unit| unit.tiles).collect(),
        tile_bytes: input.formats.layer_bytes_at(tile_size, tile_size),
        oversize,
        chart_texels: input.charts.iter().map(ChartRect::area).sum(),
    }
}

/// Pack one unit's padded charts into `tile × tile` tiles with the bake's
/// MaxRects bins, first-fit decreasing by area. The bake's own
/// `pack_layers_with_layer_limit` cannot be reused here: it sizes each layer
/// to the largest leaf rather than to a fixed P, and fills only its newest
/// layer, which suits leaf-ordered layers but not per-unit tiles.
pub(crate) fn pack_unit_tiles(charts: &[ChartRect], tile: u32) -> UnitTiles {
    let mut order: Vec<&ChartRect> = charts.iter().collect();
    order.sort_by_key(|chart| std::cmp::Reverse(chart.area()));
    let mut bins: Vec<MaxRects> = Vec::new();
    let mut oversize = OversizeCharts::default();
    for chart in order {
        if chart.width > tile || chart.height > tile {
            let tiles =
                u64::from(chart.width.div_ceil(tile)) * u64::from(chart.height.div_ceil(tile));
            oversize.charts += 1;
            oversize.chart_texels += chart.area();
            oversize.tiles += tiles;
            let pad = 2 * CHART_PADDING_TEXELS;
            if chart.width.saturating_sub(pad) <= tile && chart.height.saturating_sub(pad) <= tile {
                oversize.interior_fits += 1;
            }
            continue;
        }
        let placed = bins
            .iter_mut()
            .any(|bin| bin.insert(chart.width, chart.height).is_some());
        if !placed {
            let mut bin = MaxRects::new(tile, tile);
            bin.insert(chart.width, chart.height)
                .expect("a chart no larger than the tile fits an empty tile");
            bins.push(bin);
        }
    }
    UnitTiles {
        tiles: bins.len() as u64 + oversize.tiles,
        oversize,
    }
}
