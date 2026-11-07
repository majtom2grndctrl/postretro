//! The lightmap pools as an author reads them: the bytes the runtime will
//! allocate first, with a warning past what the other pools leave of the
//! memory target.
//! See: context/lib/build_pipeline.md §PRL section IDs (Lightmap pool bound)

use std::time::Duration;

use postretro_level_format::cells::CellsSection;
use postretro_level_format::lightmap_pool::first_generation_layers;

use super::WorstDemand;
use super::demand::{LayerPrices, PoolLayers};
use crate::pack::bound_report::{CellPlace, MIB, metres, mib};

/// What the other pools leave of the 2 GB target: 2048 MiB minus SH's 512 MiB,
/// the shadow pools (about 427 MiB) and everything else (about 693 MiB) is
/// about 416 MiB, rounded down to a multiple of 64 MiB. Recompute from those
/// figures when any of them moves. Fixed, not an author key.
const LIGHTMAP_POOL_WARN_BYTES: u64 = 384 * MIB;

/// How much of the level the pools hold at once.
#[derive(Debug, Clone, PartialEq)]
enum PoolReach {
    /// The level streams, sized from its worst camera cell.
    Lead {
        lead: u32,
        bound: PoolLayers,
        /// `None` when the level has no camera cell to name.
        peak: Option<CellPlace>,
    },
    /// No id 51: every block stays loaded.
    WholeMap,
}

#[derive(Debug, Clone, PartialEq)]
struct PricedPools {
    /// The first generation of both pools, which is what the runtime allocates.
    bytes: u64,
    layers: PoolLayers,
    reach: PoolReach,
}

/// What the Build Summary says about the pools. No pools means the level has
/// no lightmap blocks.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LightmapPoolReport {
    pools: Option<PricedPools>,
    /// The bound loop alone, inside the Packing stage.
    pub(crate) elapsed: Duration,
}

impl LightmapPoolReport {
    pub(super) fn streamed(
        lead: u32,
        worst: &WorstDemand,
        all_resident: PoolLayers,
        prices: LayerPrices,
        cells: &CellsSection,
        elapsed: Duration,
    ) -> Self {
        let layers = PoolLayers {
            lightmap: first_generation_layers(worst.layers.lightmap, all_resident.lightmap),
            mask: first_generation_layers(worst.layers.mask, all_resident.mask),
        };
        Self {
            pools: Some(PricedPools {
                bytes: prices.streamed_bytes(layers),
                layers,
                reach: PoolReach::Lead {
                    lead,
                    bound: worst.layers,
                    peak: worst.peak.map(|peak| CellPlace::of(peak.cell, cells)),
                },
            }),
            elapsed,
        }
    }

    pub(super) fn all_resident(layers: PoolLayers, prices: LayerPrices, elapsed: Duration) -> Self {
        Self {
            pools: Some(PricedPools {
                bytes: prices.all_resident_bytes(layers),
                layers,
                reach: PoolReach::WholeMap,
            }),
            elapsed,
        }
    }

    /// The Build Summary block.
    pub(crate) fn lines(&self) -> Vec<String> {
        let Some(pools) = &self.pools else {
            return vec!["Lightmap pool memory: none (this level has no baked lightmaps)".into()];
        };
        let PoolLayers { lightmap, mask } = pools.layers;
        let sized = format!("{lightmap} lightmap and {mask} shadow mask layers");
        let bytes = mib(pools.bytes);
        let mut lines = vec![match &pools.reach {
            PoolReach::Lead { lead, bound, peak } => {
                let fit = format!(
                    "Lightmap pool memory: {bytes} at {} (how far ahead of the camera lightmaps \
                     load), {sized}, sized from a worst case of {} and {}",
                    metres(*lead),
                    bound.lightmap,
                    bound.mask,
                );
                match peak {
                    Some(peak) => format!("{fit}; demand peaks {}", peak.describe()),
                    None => fit,
                }
            }
            PoolReach::WholeMap => format!(
                "Lightmap pool memory: {bytes}, {sized}, the whole map: without usable portals \
                 every lightmap stays loaded"
            ),
        }];
        lines.extend(
            self.warning()
                .map(|warning| format!("  warning: {warning}")),
        );
        lines
    }

    pub(super) fn log(&self) {
        if let Some(pools) = &self.pools {
            log::info!(
                "[Compiler] Lightmap pool bound: {}, {:.3} ms",
                mib(pools.bytes),
                self.elapsed.as_secs_f64() * 1000.0,
            );
        }
        if let Some(warning) = self.warning() {
            log::warn!("[Compiler] {warning}");
        }
    }

    /// Fires strictly above the threshold.
    fn warning(&self) -> Option<String> {
        let pools = self.pools.as_ref()?;
        if pools.bytes <= LIGHTMAP_POOL_WARN_BYTES {
            return None;
        }
        let over = format!(
            "Lightmap pool memory {} is above the {} budget, so this level may not fit a 2 GB GPU.",
            mib(pools.bytes),
            mib(LIGHTMAP_POOL_WARN_BYTES),
        );
        Some(match &pools.reach {
            PoolReach::Lead {
                peak: Some(peak), ..
            } => format!(
                "{over} Add walls or doors near {}, where lightmap demand peaks, so less of the map can be seen and reached from there.",
                peak.position(),
            ),
            PoolReach::Lead { peak: None, .. } => over,
            PoolReach::WholeMap => format!(
                "{over} Seal the level so it has usable portals, which let lightmaps load by area."
            ),
        })
    }
}
