// Cell-block packing heuristics and rules: free-rect rule × chart order, chart alignment, rotation, trim.
// See: context/lib/build_pipeline.md §Compiler pipeline (atlas preparation)

use super::atlas_pack::FreeRectRule;

/// The order a heuristic feeds charts to MaxRects: largest key first, ties
/// by input index. Keys read a chart's footprint without its orientation, so
/// a chart turned on input packs the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChartOrder {
    /// Footprint area.
    Area,
    /// Longer footprint side, then area.
    LongSide,
}

impl ChartOrder {
    pub(crate) fn key(self, (w, h): (u32, u32)) -> (u64, u64) {
        let area = u64::from(w) * u64::from(h);
        match self {
            ChartOrder::Area => (area, 0),
            ChartOrder::LongSide => (u64::from(w.max(h)), area),
        }
    }
}

/// One packing heuristic: how MaxRects picks a free rect, and the order
/// charts arrive in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackHeuristic {
    pub(crate) rule: FreeRectRule,
    pub(crate) order: ChartOrder,
}

/// Every heuristic a cell tries, in tie-break order: on equal block area the
/// earlier one is kept. The first is the packer's historical rule.
pub(crate) const PACK_HEURISTICS: [PackHeuristic; 8] = [
    PackHeuristic::new(FreeRectRule::ShortSide, ChartOrder::Area),
    PackHeuristic::new(FreeRectRule::LongSide, ChartOrder::Area),
    PackHeuristic::new(FreeRectRule::Area, ChartOrder::Area),
    PackHeuristic::new(FreeRectRule::BottomLeft, ChartOrder::Area),
    PackHeuristic::new(FreeRectRule::ShortSide, ChartOrder::LongSide),
    PackHeuristic::new(FreeRectRule::LongSide, ChartOrder::LongSide),
    PackHeuristic::new(FreeRectRule::Area, ChartOrder::LongSide),
    PackHeuristic::new(FreeRectRule::BottomLeft, ChartOrder::LongSide),
];

impl PackHeuristic {
    const fn new(rule: FreeRectRule, order: ChartOrder) -> Self {
        Self { rule, order }
    }

    /// The historical rule: short-side fit, largest area first.
    pub(crate) const DEFAULT: PackHeuristic = PACK_HEURISTICS[0];

    /// The `--pack-heuristic` name.
    pub fn name(self) -> &'static str {
        match (self.rule, self.order) {
            (FreeRectRule::ShortSide, ChartOrder::Area) => "short-side-by-area",
            (FreeRectRule::LongSide, ChartOrder::Area) => "long-side-by-area",
            (FreeRectRule::Area, ChartOrder::Area) => "area-by-area",
            (FreeRectRule::BottomLeft, ChartOrder::Area) => "bottom-left-by-area",
            (FreeRectRule::ShortSide, ChartOrder::LongSide) => "short-side-by-long-side",
            (FreeRectRule::LongSide, ChartOrder::LongSide) => "long-side-by-long-side",
            (FreeRectRule::Area, ChartOrder::LongSide) => "area-by-long-side",
            (FreeRectRule::BottomLeft, ChartOrder::LongSide) => "bottom-left-by-long-side",
        }
    }

    /// The heuristic named `name`, as [`Self::name`] spells it.
    pub fn parse(name: &str) -> Option<Self> {
        PACK_HEURISTICS.into_iter().find(|h| h.name() == name)
    }

    /// Every name, in tie-break order, for help text and errors.
    pub fn names() -> Vec<&'static str> {
        PACK_HEURISTICS.iter().map(|h| h.name()).collect()
    }
}

/// How one set of charts packs into a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PackRules {
    /// Block extents are multiples of this.
    pub align: u32,
    /// Every chart packs as its extent rounded up to a multiple of this, so
    /// every chart origin is a multiple of it too.
    pub chart_align: u32,
    /// A non-square chart may be placed turned.
    pub rotate: bool,
    /// Each candidate block trims to the aligned bound of what it placed.
    pub trim: bool,
}

impl PackRules {
    /// Lightmap charts: origins on the direction plane's grid, so its
    /// `scale × scale` reduction groups the same texels wherever a chart lands;
    /// turned charts; trimmed blocks. `align` is the block alignment,
    /// `lcm(4, scale)`.
    pub(crate) fn lightmap(align: u32, direction_texel_scale: u32) -> Self {
        let chart_align = direction_texel_scale.max(1);
        assert!(
            align.is_multiple_of(chart_align),
            "block alignment {align} is not a multiple of the direction scale {chart_align}"
        );
        Self {
            align,
            chart_align,
            rotate: true,
            trim: true,
        }
    }

    /// Mask charts: copied whole in their lightmap orientation, so never
    /// turned or rounded, packed as the mask layout always packed them.
    pub(crate) fn mask(align: u32) -> Self {
        Self {
            align,
            chart_align: 1,
            rotate: false,
            trim: false,
        }
    }

    /// The rect a `(w, h)` chart packs as.
    pub(crate) fn footprint(&self, (w, h): (u32, u32)) -> (u32, u32) {
        let up = |v: u32| v.div_ceil(self.chart_align) * self.chart_align;
        (up(w), up(h))
    }

    /// `(w, h)` in the orientation every turnable chart is first tried in,
    /// landscape, so a chart handed in turned packs to the same rect.
    pub(crate) fn canonical(&self, (w, h): (u32, u32)) -> (u32, u32) {
        if self.rotate && h > w { (h, w) } else { (w, h) }
    }

    pub(crate) fn align_up(&self, value: u32) -> u32 {
        value.div_ceil(self.align) * self.align
    }
}
