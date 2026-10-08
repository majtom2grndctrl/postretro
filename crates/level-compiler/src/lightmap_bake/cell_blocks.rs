// Per-cell lightmap block packing: one cell's padded charts in one or more aligned rectangles.
// See: context/lib/build_pipeline.md §PRL section IDs (Lightmap id 22)

#[cfg(test)]
use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;

use super::atlas_pack::MaxRects;

/// Candidate block widths as multiples of `sqrt(chart area)`, each floored at
/// the widest chart.
const WIDTH_FACTORS: [f64; 8] = [0.5, 0.7, 0.85, 1.0, 1.15, 1.3, 1.6, 2.0];

/// Widths tried per cell: every factor, the widest chart alone, and the pool
/// layer edge.
#[cfg(test)]
pub(crate) const CANDIDATE_WIDTHS: usize = WIDTH_FACTORS.len() + 2;

/// One packed block of a cell, in irradiance texels.
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

    pub(crate) fn fits(&self, edge: u32) -> bool {
        self.width <= edge && self.height <= edge
    }
}

/// One block of a cell: the cell-local indices of the charts it holds,
/// ascending, and their packing (`block.placements` parallel to `members`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CellSubBlock {
    pub members: Vec<usize>,
    pub block: PackedBlock,
}

/// Pack one cell's padded chart extents into as many `align`-multiple blocks
/// as it needs, each within `pool_edge`. A cell the single-block packer fits
/// within `pool_edge` packs exactly as [`pack_cell_block`]. Otherwise the cell
/// fills a pool-edge block largest chart first, trims it to its packed
/// content, and repeats on the charts that did not fit until the rest pack
/// into one block the same way. A chart reaches a later block only after
/// MaxRects, which tracks every maximal free rectangle, refused it in every
/// earlier one; placing charts and trimming only remove free space, so it
/// fits no earlier block at the end either. Empty for a cell without charts.
pub(crate) fn pack_cell_sub_blocks(
    charts: &[(u32, u32)],
    align: u32,
    pool_edge: u32,
) -> Vec<CellSubBlock> {
    // The fill path relies on both: trimmed extents round up within the
    // edge, and the largest remaining chart always fits an empty block.
    assert!(
        align > 0 && pool_edge.is_multiple_of(align),
        "block alignment {align} does not divide the pool layer edge {pool_edge}"
    );
    assert!(
        charts
            .iter()
            .all(|&(w, h)| w <= pool_edge && h <= pool_edge),
        "every chart fits the {pool_edge} pool edge (the oversize-face cut)"
    );
    let area_of = |&(w, h): &(u32, u32)| u64::from(w) * u64::from(h);
    let pool_area = u64::from(pool_edge) * u64::from(pool_edge);
    let mut remaining: Vec<usize> = (0..charts.len()).collect();
    let mut blocks = Vec::new();
    while !remaining.is_empty() {
        // Charts covering more than a layer's area cannot pack into one, so
        // skip the tight candidate search for them.
        let area: u64 = remaining.iter().map(|&i| area_of(&charts[i])).sum();
        if area <= pool_area {
            let sizes: Vec<(u32, u32)> = remaining.iter().map(|&i| charts[i]).collect();
            let tight = pack_cell_block_within(&sizes, align, pool_edge)
                .expect("a non-empty chart set packs a block");
            if tight.fits(pool_edge) {
                blocks.push(CellSubBlock {
                    members: remaining,
                    block: tight,
                });
                break;
            }
        }
        let (filled, rest) = fill_pool_layer_block(charts, &remaining, align, pool_edge);
        blocks.push(filled);
        remaining = rest;
    }
    blocks
}

/// Fill one `pool_edge²` block from `remaining` (largest first, ties by
/// index), trimmed to the aligned bounding box of what it placed; return it
/// and the charts it could not place, ascending.
fn fill_pool_layer_block(
    charts: &[(u32, u32)],
    remaining: &[usize],
    align: u32,
    pool_edge: u32,
) -> (CellSubBlock, Vec<usize>) {
    let align_up = |value: u32| value.div_ceil(align) * align;
    let area_of = |i: usize| u64::from(charts[i].0) * u64::from(charts[i].1);
    let mut order = remaining.to_vec();
    order.sort_by(|&a, &b| area_of(b).cmp(&area_of(a)).then(a.cmp(&b)));

    let mut bin = MaxRects::new(pool_edge, pool_edge);
    let mut placed: Vec<(usize, (u32, u32))> = Vec::new();
    let mut rest = Vec::new();
    for index in order {
        let (w, h) = charts[index];
        match bin.insert(w, h) {
            Some(origin) => placed.push((index, origin)),
            None => rest.push(index),
        }
    }
    assert!(
        !placed.is_empty(),
        "every chart fits a pool layer, so an empty block places the largest"
    );
    placed.sort_unstable_by_key(|&(index, _)| index);
    rest.sort_unstable();
    let width = align_up(
        placed
            .iter()
            .map(|&(i, (x, _))| x + charts[i].0)
            .max()
            .expect("the block placed a chart"),
    );
    let height = align_up(
        placed
            .iter()
            .map(|&(i, (_, y))| y + charts[i].1)
            .max()
            .expect("the block placed a chart"),
    );
    (
        CellSubBlock {
            members: placed.iter().map(|&(index, _)| index).collect(),
            block: PackedBlock {
                width,
                height,
                placements: placed.iter().map(|&(_, origin)| origin).collect(),
            },
        },
        rest,
    )
}

/// Pack one cell's padded chart extents `(width, height)` into the
/// smallest-area block found whose extent is a multiple of `align`,
/// preferring one that fits a pool layer. Each candidate width takes the
/// shortest aligned height MaxRects (largest-first, ties by input order) fits
/// every chart into. `None` for a cell without charts.
///
/// Placements are multiples of `align` only at the block origin; charts sit
/// anywhere inside. The best block may exceed a pool layer; the caller splits
/// such a cell (`pack_cell_sub_blocks`).
#[cfg(test)]
pub(crate) fn pack_cell_block(charts: &[(u32, u32)], align: u32) -> Option<PackedBlock> {
    pack_cell_block_within(charts, align, LIGHTMAP_POOL_LAYER_EDGE)
}

/// [`pack_cell_block`] against a `pool_edge` other than the runtime's, so
/// tests can drive multi-block cells with small charts.
pub(crate) fn pack_cell_block_within(
    charts: &[(u32, u32)],
    align: u32,
    pool_edge: u32,
) -> Option<PackedBlock> {
    if charts.is_empty() {
        return None;
    }
    assert!(
        align > 0 && pool_edge.is_multiple_of(align),
        "block alignment {align} does not divide the pool layer edge {pool_edge}"
    );
    let area_of = |&(w, h): &(u32, u32)| u64::from(w) * u64::from(h);
    let align_up = |value: u32| value.div_ceil(align) * align;
    let mut order: Vec<usize> = (0..charts.len()).collect();
    order.sort_by(|&a, &b| {
        area_of(&charts[b])
            .cmp(&area_of(&charts[a]))
            .then(a.cmp(&b))
    });
    let area: u64 = charts.iter().map(area_of).sum();
    let max_width = charts.iter().map(|c| c.0).max().unwrap_or(0);
    let max_height = charts.iter().map(|c| c.1).max().unwrap_or(0);
    let side = (area as f64).sqrt();

    let mut widths: Vec<u32> = WIDTH_FACTORS
        .iter()
        .map(|&f| align_up(((side * f).ceil() as u32).max(max_width)))
        .collect();
    widths.push(align_up(max_width));
    if align_up(max_width) <= pool_edge {
        widths.push(pool_edge);
    }
    widths.sort_unstable();
    widths.dedup();

    let mut best: Option<PackedBlock> = None;
    for width in widths {
        let floor = align_up(max_height.max(area.div_ceil(u64::from(width)) as u32));
        let block = shortest_block(charts, &order, width, floor, align);
        // A block that fits a pool layer beats any that does not; then the
        // smaller area, then the squarer shape.
        let key = |b: &PackedBlock| (!b.fits(pool_edge), b.area(), b.width.max(b.height));
        if best.as_ref().is_none_or(|b| key(&block) < key(b)) {
            best = Some(block);
        }
    }
    best
}

/// Shortest `align`-multiple height at `width` that packs every chart,
/// searched upward geometrically from `floor` and then bisected.
fn shortest_block(
    charts: &[(u32, u32)],
    order: &[usize],
    width: u32,
    floor: u32,
    align: u32,
) -> PackedBlock {
    let align_up = |value: u32| value.div_ceil(align) * align;
    let mut fail = None;
    let mut height = floor;
    let mut placed = loop {
        match try_pack(charts, order, width, height) {
            Some(placements) => break placements,
            None => {
                fail = Some(height);
                height = align_up(height + (height / 8).max(align));
            }
        }
    };
    if let Some(mut low) = fail {
        // `low` fails and `height` packs, both aligned; bisect on the
        // alignment grid. MaxRects fit is not strictly monotone in height, so
        // this finds a short packing height, not provably the shortest.
        while height - low > align {
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
    charts: &[(u32, u32)],
    order: &[usize],
    width: u32,
    height: u32,
) -> Option<Vec<(u32, u32)>> {
    let mut bin = MaxRects::new(width, height);
    let mut placements = vec![(0, 0); charts.len()];
    for &index in order {
        placements[index] = bin.insert(charts[index].0, charts[index].1)?;
    }
    Some(placements)
}
