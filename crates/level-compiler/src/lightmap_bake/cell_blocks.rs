// Per-cell lightmap block packing: one cell's padded charts in one aligned rectangle.
// See: context/lib/build_pipeline.md §PRL section IDs (Lightmap id 22)

use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;

use super::atlas_pack::MaxRects;

/// Candidate block widths as multiples of `sqrt(chart area)`, each floored at
/// the widest chart.
const WIDTH_FACTORS: [f64; 8] = [0.5, 0.7, 0.85, 1.0, 1.15, 1.3, 1.6, 2.0];

/// Widths tried per cell: every factor, the widest chart alone, and the pool
/// layer edge.
#[cfg(test)]
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

    pub(crate) fn fits_pool_layer(&self) -> bool {
        self.width <= LIGHTMAP_POOL_LAYER_EDGE && self.height <= LIGHTMAP_POOL_LAYER_EDGE
    }
}

/// Pack one cell's padded chart extents `(width, height)` into the
/// smallest-area block found whose extent is a multiple of `align`,
/// preferring one that fits a pool layer. Each candidate width takes the
/// shortest aligned height MaxRects (largest-first, ties by input order) fits
/// every chart into. `None` for a cell without charts.
///
/// Placements are multiples of `align` only at the block origin; charts sit
/// anywhere inside. The caller rejects a block larger than a pool layer.
pub(crate) fn pack_cell_block(charts: &[(u32, u32)], align: u32) -> Option<PackedBlock> {
    if charts.is_empty() {
        return None;
    }
    assert!(
        align > 0 && LIGHTMAP_POOL_LAYER_EDGE % align == 0,
        "block alignment {align} does not divide the pool layer edge"
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
    if align_up(max_width) <= LIGHTMAP_POOL_LAYER_EDGE {
        widths.push(LIGHTMAP_POOL_LAYER_EDGE);
    }
    widths.sort_unstable();
    widths.dedup();

    let mut best: Option<PackedBlock> = None;
    for width in widths {
        let floor = align_up(max_height.max(area.div_ceil(u64::from(width)) as u32));
        let block = shortest_block(charts, &order, width, floor, align);
        // A block that fits a pool layer beats any that does not; then the
        // smaller area, then the squarer shape.
        let key = |b: &PackedBlock| (!b.fits_pool_layer(), b.area(), b.width.max(b.height));
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
