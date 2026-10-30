// Growth sizing: the layers a drain's first growth reserves for the targeted demand.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::cmp::Reverse;

use super::LightmapPoolModel;

/// Headroom a growth reserves over the targeted demand's from-scratch pack.
/// The runtime pool packs blocks as they arrive and frees them as they
/// leave, so it holds more layers than a fresh pack of the same set: the
/// hallway's runtime pool occupied 16 layers where the shelf worst was 12
/// (`context/plans/done/spatial-residency--lightmap-cell-blocks/findings.md`).
pub(super) const GROWTH_HEADROOM: Ratio = Ratio {
    numerator: 4,
    denominator: 3,
};

/// The pool sizing policy's identity, recorded by measurement files so a
/// paired comparison never spans two policies. Change this string whenever
/// any lightmap pool sizing rule changes: the `GROWTH_HEADROOM` ratio, the
/// all-resident clamp in `growth_reserve_layers`, or the
/// at-need extension in `placement.rs`. A record written under the old string
/// then reads as a different policy instead of a silent mismatch.
pub const POOL_POLICY_IDENTITY: &str = "demand-sized growth headroom 4/3";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Ratio {
    pub numerator: u32,
    pub denominator: u32,
}

impl Ratio {
    /// `layers × self`, rounded up.
    pub(super) fn of(self, layers: u32) -> u32 {
        let scaled =
            (u64::from(layers) * u64::from(self.numerator)).div_ceil(u64::from(self.denominator));
        u32::try_from(scaled).unwrap_or(u32::MAX)
    }
}

/// How a growth is sized. `PerDrain` is the sizing demand sizing replaced,
/// kept so tests can show the deferrals it caused.
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum GrowthSizing {
    #[default]
    Demand,
    /// Exactly the layers the drain's own pairs land in.
    PerDrain,
}

impl LightmapPoolModel {
    /// Layers a drain's first growth reserves, `needed` being one past the
    /// layer the growing pair landed in.
    ///
    /// Sized to every mandatory and visible target, resident or not, rather
    /// than to the pairs this drain admitted: the shared drain budget admits
    /// a large demand over several drains, and a second growth while the
    /// first generation retires would defer the rest. The reserve is that
    /// demand's from-scratch pack times [`GROWTH_HEADROOM`], bounded by what
    /// the level needs all-resident and by the device limit, and never below
    /// `needed`: a fragmented pool can need more than the all-resident
    /// layers. `needed` never passes the device limit.
    pub(super) fn growth_reserve_layers(&mut self, needed: u32) -> u32 {
        #[cfg(test)]
        if self.growth_sizing == GrowthSizing::PerDrain {
            return needed;
        }
        let ceiling = self.all_resident_layers.min(self.max_layers);
        GROWTH_HEADROOM
            .of(self.targeted_demand_layers())
            .min(ceiling)
            .max(needed)
    }

    /// Layers a from-scratch shelf pack of every mandatory and visible
    /// target fills, tallest first as the drain places them.
    fn targeted_demand_layers(&mut self) -> u32 {
        let mut demand = std::mem::take(&mut self.scratch.demand);
        demand.clear();
        demand.extend(
            (0..self.block_count())
                .filter(|&block| self.target(block).is_some_and(|t| !t.is_band()))
                .map(|block| self.alloc_extent(block)),
        );
        demand.sort_unstable_by_key(|&(width, height)| (Reverse(height), Reverse(width)));
        let layers = super::super::place_all_resident(&demand, self.alignment, self.edge)
            .map_or(0, |pool| pool.layer_count);
        self.scratch.demand = demand;
        layers
    }
}
