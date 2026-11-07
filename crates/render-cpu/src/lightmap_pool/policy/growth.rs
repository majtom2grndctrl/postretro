// Growth sizing: the layers a drain's first growth reserves for the targeted demand.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap_pool::{LIGHTMAP_POOL_MARGIN, PoolKind};

use super::LightmapPoolModel;

/// The pool sizing policy's identity, recorded by measurement files so a
/// paired comparison never spans two policies. Change this string whenever
/// any lightmap pool sizing rule changes: `LIGHTMAP_POOL_MARGIN`, the
/// first-generation rule, the all-resident clamp in `growth_reserve_layers`,
/// or the at-need extension in `pair.rs`. A record written under the old
/// string then reads as a different policy instead of a silent mismatch.
pub const POOL_POLICY_IDENTITY: &str = "bound-sized split pools, shared margin 5/4";

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
    /// Layers `kind`'s first growth of a drain reserves, `needed` being one
    /// past the layer the growing half landed in.
    ///
    /// Sized to every mandatory and visible target, resident or not, rather
    /// than to the pairs this drain admitted: the shared drain budget admits
    /// a large demand over several drains, and a second growth while the
    /// first generation retires would defer the rest. The reserve is that
    /// demand's from-scratch pack times [`LIGHTMAP_POOL_MARGIN`], bounded by
    /// what the level needs all-resident and by the device limit, and never
    /// below `needed`: a fragmented pool can need more than the all-resident
    /// layers. `needed` never passes the device limit.
    pub(super) fn growth_reserve_layers(&mut self, kind: PoolKind, needed: u32) -> u32 {
        #[cfg(test)]
        if self.growth_sizing == GrowthSizing::PerDrain {
            return needed;
        }
        let pool = &self.pools[kind];
        let ceiling = pool.all_resident_layers.min(pool.max_layers);
        LIGHTMAP_POOL_MARGIN
            .of(self.targeted_demand_layers(kind))
            .min(ceiling)
            .max(needed)
    }

    /// Layers a from-scratch shelf pack of every mandatory and visible
    /// target's items in `kind`'s pool fills, in the shared packing order.
    pub(super) fn targeted_demand_layers(&mut self, kind: PoolKind) -> u32 {
        let mut demand = std::mem::take(&mut self.scratch.demand);
        demand.clear();
        let pool = &self.pools[kind];
        demand.extend(
            pool.items()
                .iter()
                .filter(|&&item| self.is_never_refused(kind.block_of(item)))
                .map(|&item| pool.alloc_extent(item)),
        );
        let layers = super::super::place_all_resident(&demand, pool.alignment(), pool.edge())
            .map_or(0, |pool| pool.layer_count);
        self.scratch.demand = demand;
        layers
    }

    /// `targeted_demand_layers` of both pools, for tests outside this crate that
    /// check a count against the model's own.
    #[cfg(any(test, feature = "test-support"))]
    pub fn targeted_layers(&mut self) -> postretro_level_format::lightmap_pool::PerPool<u32> {
        postretro_level_format::lightmap_pool::PerPool::from_fn(|kind| {
            self.targeted_demand_layers(kind)
        })
    }
}
