// Per-cell lightmap block packing: one cell's padded charts in one or more aligned rectangles.
// See: context/lib/build_pipeline.md §PRL section IDs (Lightmap id 22)

#[cfg(test)]
use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;

use super::atlas_pack::MaxRects;
use super::pack_heuristics::{PackHeuristic, PackRules};

/// Candidate block widths as multiples of `sqrt(chart area)`, each floored at
/// the widest chart.
const WIDTH_FACTORS: [f64; 8] = [0.5, 0.7, 0.85, 1.0, 1.15, 1.3, 1.6, 2.0];

/// Widths tried per cell: every factor, the widest chart alone, and the pool
/// layer edge.
#[cfg(test)]
pub(crate) const CANDIDATE_WIDTHS: usize = WIDTH_FACTORS.len() + 2;

/// Where one chart sits in its block: the top-left of its placed rect, and
/// whether it is placed turned (its rect is `height × width`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockSlot {
    pub x: u32,
    pub y: u32,
    pub transposed: bool,
}

/// One packed block of a cell, in irradiance texels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackedBlock {
    pub width: u32,
    pub height: u32,
    /// Each input chart's slot inside the block, in input order.
    pub placements: Vec<BlockSlot>,
}

impl PackedBlock {
    pub(crate) fn area(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    pub(crate) fn fits(&self, edge: u32) -> bool {
        self.width <= edge && self.height <= edge
    }

    /// Charts placed turned: a tie between equal blocks keeps charts upright.
    pub(crate) fn transposed_count(&self) -> usize {
        self.placements
            .iter()
            .filter(|slot| slot.transposed)
            .count()
    }
}

/// One block of a cell: the cell-local indices of the charts it holds,
/// ascending, and their packing (`block.placements` parallel to `members`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CellSubBlock {
    pub members: Vec<usize>,
    pub block: PackedBlock,
}

/// A cell's packing and the heuristic that won it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CellPack {
    pub blocks: Vec<CellSubBlock>,
    /// Index into the heuristics the cell tried.
    pub heuristic: usize,
}

/// Pack one cell's padded chart extents with every heuristic in
/// `heuristics` ([`pack_cell_sub_blocks_with`]) and keep the smallest total
/// block area; ties keep fewer blocks, then fewer turned charts, then the
/// earlier heuristic. Packing reads only the chart extents, so the choice is
/// fixed whatever the worker count.
pub(crate) fn pack_cell_sub_blocks(
    charts: &[(u32, u32)],
    rules: PackRules,
    pool_edge: u32,
    heuristics: &[PackHeuristic],
) -> CellPack {
    assert!(!heuristics.is_empty(), "a cell packs with some heuristic");
    let mut best: Option<((u64, usize, usize), CellPack)> = None;
    for (index, &heuristic) in heuristics.iter().enumerate() {
        let blocks = pack_cell_sub_blocks_with(charts, rules, pool_edge, heuristic);
        let key = (
            blocks.iter().map(|b| b.block.area()).sum::<u64>(),
            blocks.len(),
            blocks
                .iter()
                .map(|b| b.block.transposed_count())
                .sum::<usize>(),
        );
        if best.as_ref().is_none_or(|(best_key, _)| key < *best_key) {
            best = Some((
                key,
                CellPack {
                    blocks,
                    heuristic: index,
                },
            ));
        }
    }
    best.expect("at least one heuristic ran").1
}

/// Pack one cell's padded chart extents into as many `rules.align`-multiple
/// blocks as it needs, each within `pool_edge`, under one heuristic. A cell
/// the single-block packer fits within `pool_edge` packs exactly as
/// [`pack_cell_block_within`]. Otherwise the cell fills a pool-edge block
/// largest chart first, trims it to its packed content, and repeats on the
/// charts that did not fit until the rest pack into one block the same way.
/// A chart reaches a later block only after MaxRects, which tracks every
/// maximal free rectangle, refused it in every earlier one; placing charts
/// and trimming only remove free space, so it fits no earlier block at the
/// end either. Empty for a cell without charts.
fn pack_cell_sub_blocks_with(
    charts: &[(u32, u32)],
    rules: PackRules,
    pool_edge: u32,
    heuristic: PackHeuristic,
) -> Vec<CellSubBlock> {
    // The fill path relies on both: trimmed extents round up within the
    // edge, and the largest remaining chart always fits an empty block.
    assert!(
        rules.align > 0 && pool_edge.is_multiple_of(rules.align),
        "block alignment {} does not divide the pool layer edge {pool_edge}",
        rules.align
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
        let area: u64 = remaining
            .iter()
            .map(|&i| area_of(&rules.footprint(charts[i])))
            .sum();
        if area <= pool_area {
            let sizes: Vec<(u32, u32)> = remaining.iter().map(|&i| charts[i]).collect();
            let tight = pack_cell_block_reserving(&sizes, None, rules, pool_edge, heuristic)
                .expect("a non-empty chart set packs a block");
            if tight.fits(pool_edge) {
                blocks.push(CellSubBlock {
                    members: remaining,
                    block: tight,
                });
                break;
            }
        }
        let (filled, rest) = fill_pool_layer_block(charts, &remaining, rules, pool_edge, heuristic);
        blocks.push(filled);
        remaining = rest;
    }
    blocks
}

/// Insert chart `(w, h)` under `rules`: its footprint, tried landscape first
/// when it may turn. The slot says whether it was placed turned relative to
/// the chart's own orientation.
fn insert_chart(
    bin: &mut MaxRects,
    chart: (u32, u32),
    rules: PackRules,
    heuristic: PackHeuristic,
) -> Option<BlockSlot> {
    let footprint = rules.footprint(chart);
    let canonical = rules.canonical(footprint);
    let (x, y, turned) = bin.insert_with(canonical.0, canonical.1, heuristic.rule, rules.rotate)?;
    let placed = if turned {
        (canonical.1, canonical.0)
    } else {
        canonical
    };
    Some(BlockSlot {
        x,
        y,
        transposed: placed != footprint,
    })
}

/// A chart's placed extent in its slot.
fn placed_extent(chart: (u32, u32), slot: &BlockSlot) -> (u32, u32) {
    if slot.transposed {
        (chart.1, chart.0)
    } else {
        chart
    }
}

/// Charts in `heuristic`'s order: largest key first, ties by index.
fn chart_order(
    charts: &[(u32, u32)],
    indices: &[usize],
    rules: PackRules,
    heuristic: PackHeuristic,
) -> Vec<usize> {
    let mut order = indices.to_vec();
    let key = |i: usize| heuristic.order.key(rules.footprint(charts[i]));
    order.sort_by(|&a, &b| key(b).cmp(&key(a)).then(a.cmp(&b)));
    order
}

/// Fill one `pool_edge²` block from `remaining` in the heuristic's order,
/// trimmed to the aligned bounding box of what it placed; return it and the
/// charts it could not place, ascending.
fn fill_pool_layer_block(
    charts: &[(u32, u32)],
    remaining: &[usize],
    rules: PackRules,
    pool_edge: u32,
    heuristic: PackHeuristic,
) -> (CellSubBlock, Vec<usize>) {
    let order = chart_order(charts, remaining, rules, heuristic);
    let mut bin = MaxRects::new(pool_edge, pool_edge);
    let mut placed: Vec<(usize, BlockSlot)> = Vec::new();
    let mut rest = Vec::new();
    for index in order {
        match insert_chart(&mut bin, charts[index], rules, heuristic) {
            Some(slot) => placed.push((index, slot)),
            None => rest.push(index),
        }
    }
    assert!(
        !placed.is_empty(),
        "every chart fits a pool layer, so an empty block places the largest"
    );
    placed.sort_unstable_by_key(|&(index, _)| index);
    rest.sort_unstable();
    let (width, height) = content_bound(placed.iter().map(|&(i, slot)| (charts[i], slot)), rules);
    (
        CellSubBlock {
            members: placed.iter().map(|&(index, _)| index).collect(),
            block: PackedBlock {
                width,
                height,
                placements: placed.iter().map(|&(_, slot)| slot).collect(),
            },
        },
        rest,
    )
}

/// The aligned bound of every placed chart rect.
fn content_bound(
    placed: impl Iterator<Item = ((u32, u32), BlockSlot)>,
    rules: PackRules,
) -> (u32, u32) {
    let (mut right, mut bottom) = (0, 0);
    for (chart, slot) in placed {
        let (w, h) = placed_extent(chart, &slot);
        right = right.max(slot.x + w);
        bottom = bottom.max(slot.y + h);
    }
    (rules.align_up(right), rules.align_up(bottom))
}

/// The one block the bake packs a cell of these padded chart extents into,
/// at the default direction scale and the runtime pool edge: every heuristic
/// tried, the smallest kept. `None` for a cell without charts; panics for a
/// cell that splits.
#[cfg(test)]
pub(crate) fn pack_cell_block(charts: &[(u32, u32)], align: u32) -> Option<PackedBlock> {
    let rules = PackRules::lightmap(align, super::DIRECTION_TEXEL_SCALE);
    let mut blocks = pack_cell_sub_blocks(
        charts,
        rules,
        LIGHTMAP_POOL_LAYER_EDGE,
        &super::pack_heuristics::PACK_HEURISTICS,
    )
    .blocks;
    assert!(blocks.len() <= 1, "the cell fits one block");
    blocks.pop().map(|sub| sub.block)
}

/// Pack one cell's padded chart extents into the smallest-area block found
/// under `rules` and `heuristic`, preferring one that fits a pool layer. Each
/// candidate width takes the shortest aligned height MaxRects fits every
/// chart into, then (with `rules.trim`) trims to the aligned content bound.
/// `None` for a cell without charts.
///
/// Block extents are multiples of `rules.align`; chart origins are multiples
/// of `rules.chart_align`. The best block may exceed a pool layer; the caller
/// splits such a cell (`pack_cell_sub_blocks`).
#[cfg(test)]
pub(crate) fn pack_cell_block_within(
    charts: &[(u32, u32)],
    rules: PackRules,
    pool_edge: u32,
    heuristic: PackHeuristic,
) -> Option<PackedBlock> {
    if charts.is_empty() {
        return None;
    }
    pack_cell_block_reserving(charts, None, rules, pool_edge, heuristic)
}

/// [`pack_cell_block_within`] around a `reserved` `(width, height)` rect held
/// at the block's origin: no chart lands in it, and the block is at least as
/// large. `None` for no reservation packs exactly as
/// [`pack_cell_block_within`]. The shadowmask's mask layout packs its
/// channel-0/1 charts around its plane-B sub-rect this way. `None` when there
/// is nothing to pack: no chart and no reservation.
pub(crate) fn pack_cell_block_reserving(
    charts: &[(u32, u32)],
    reserved: Option<(u32, u32)>,
    rules: PackRules,
    pool_edge: u32,
    heuristic: PackHeuristic,
) -> Option<PackedBlock> {
    if charts.is_empty() && reserved.is_none() {
        return None;
    }
    assert!(
        rules.align > 0 && pool_edge.is_multiple_of(rules.align),
        "block alignment {} does not divide the pool layer edge {pool_edge}",
        rules.align
    );
    let area_of = |&(w, h): &(u32, u32)| u64::from(w) * u64::from(h);
    let footprints: Vec<(u32, u32)> = charts.iter().map(|&c| rules.footprint(c)).collect();
    let order = chart_order(
        charts,
        &(0..charts.len()).collect::<Vec<_>>(),
        rules,
        heuristic,
    );
    let reserved_rect = reserved.unwrap_or((0, 0));
    let area: u64 = footprints.iter().map(area_of).sum::<u64>() + area_of(&reserved_rect);
    // A turnable chart needs only its short side across; upright, its width.
    let across = |&(w, h): &(u32, u32)| if rules.rotate { w.min(h) } else { w };
    let max_width = footprints
        .iter()
        .map(across)
        .max()
        .unwrap_or(0)
        .max(reserved_rect.0);
    let side = (area as f64).sqrt();

    let mut widths: Vec<u32> = WIDTH_FACTORS
        .iter()
        .map(|&f| rules.align_up(((side * f).ceil() as u32).max(max_width)))
        .collect();
    widths.push(rules.align_up(max_width));
    if rules.align_up(max_width) <= pool_edge {
        widths.push(pool_edge);
    }
    widths.sort_unstable();
    widths.dedup();

    let mut best: Option<PackedBlock> = None;
    for width in widths {
        // Each chart's least height at this width: upright unless only
        // turned fits across, or it may turn and its long side fits across.
        let tallest = footprints
            .iter()
            .map(|&(w, h)| {
                if rules.rotate && w.max(h) <= width {
                    w.min(h)
                } else if rules.rotate && w > width {
                    w
                } else {
                    h
                }
            })
            .max()
            .unwrap_or(0)
            .max(reserved_rect.1);
        let floor = rules.align_up(tallest.max(area.div_ceil(u64::from(width)) as u32));
        let block = shortest_block(charts, reserved, &order, width, floor, rules, heuristic);
        // A block that fits a pool layer beats any that does not; then the
        // smaller area, then the squarer shape, then fewer turned charts.
        let key = |b: &PackedBlock| {
            (
                !b.fits(pool_edge),
                b.area(),
                b.width.max(b.height),
                b.transposed_count(),
            )
        };
        if best.as_ref().is_none_or(|b| key(&block) < key(b)) {
            best = Some(block);
        }
    }
    best
}

/// Shortest `align`-multiple height at `width` that packs every chart,
/// searched upward geometrically from `floor` and then bisected; with
/// `rules.trim`, trimmed to the aligned bound of what it placed.
fn shortest_block(
    charts: &[(u32, u32)],
    reserved: Option<(u32, u32)>,
    order: &[usize],
    width: u32,
    floor: u32,
    rules: PackRules,
    heuristic: PackHeuristic,
) -> PackedBlock {
    let align = rules.align;
    let mut fail = None;
    let mut height = floor;
    let mut placed = loop {
        match try_pack(charts, reserved, order, width, height, rules, heuristic) {
            Some(placements) => break placements,
            None => {
                fail = Some(height);
                height = rules.align_up(height + (height / 8).max(align));
            }
        }
    };
    if let Some(mut low) = fail {
        // `low` fails and `height` packs, both aligned; bisect on the
        // alignment grid. MaxRects fit is not strictly monotone in height, so
        // this finds a short packing height, not provably the shortest.
        while height - low > align {
            let mid = rules.align_up(low + (height - low) / 2);
            match try_pack(charts, reserved, order, width, mid, rules, heuristic) {
                Some(placements) => {
                    height = mid;
                    placed = placements;
                }
                None => low = mid,
            }
        }
    }
    let (width, height) = if rules.trim {
        let (right, bottom) =
            content_bound(charts.iter().copied().zip(placed.iter().copied()), rules);
        let (reserved_w, reserved_h) = reserved.unwrap_or((0, 0));
        (
            right.max(rules.align_up(reserved_w)),
            bottom.max(rules.align_up(reserved_h)),
        )
    } else {
        (width, height)
    };
    PackedBlock {
        width,
        height,
        placements: placed,
    }
}

fn try_pack(
    charts: &[(u32, u32)],
    reserved: Option<(u32, u32)>,
    order: &[usize],
    width: u32,
    height: u32,
    rules: PackRules,
    heuristic: PackHeuristic,
) -> Option<Vec<BlockSlot>> {
    let mut bin = MaxRects::new(width, height);
    if let Some((reserved_width, reserved_height)) = reserved {
        bin.occupy(0, 0, reserved_width, reserved_height);
    }
    let mut placements = vec![
        BlockSlot {
            x: 0,
            y: 0,
            transposed: false,
        };
        charts.len()
    ];
    for &index in order {
        placements[index] = insert_chart(&mut bin, charts[index], rules, heuristic)?;
    }
    Some(placements)
}

#[cfg(test)]
mod tests;
