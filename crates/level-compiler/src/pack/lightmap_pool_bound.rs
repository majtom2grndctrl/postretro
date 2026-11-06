//! The worst mandatory lightmap and mask pool demand of any camera cell, for
//! id 58 and the Build Summary.
//! See: context/lib/build_pipeline.md §PRL section IDs (Lightmap pool bound)

use std::cmp::Reverse;
use std::collections::HashMap;
use std::ops::Range;
use std::time::Instant;

use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_format::cells::CellsSection;
use postretro_level_format::cluster_directory::ClusterDirectorySection;
use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::lightmap_pool_bound::LightmapPoolBoundSection;
use postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection;

use crate::cell_residency_bake::camera_cells;

mod cell_input;
mod demand;
mod report;
#[cfg(test)]
mod tests;

use cell_input::BoundCellInput;
pub(crate) use demand::PoolDemand;
use demand::PoolLayers;
use demand::{LayerPrices, PackScratch};
pub(crate) use report::LightmapPoolReport;

/// The bound's two destinations: id 58 and the Build Summary.
pub(super) struct LightmapPoolBound {
    /// `None` unless id 22 has blocks and id 51 is emitted.
    pub(super) section: Option<LightmapPoolBoundSection>,
    pub(super) report: LightmapPoolReport,
}

pub(super) struct LightmapPoolBoundInputs<'a> {
    pub(super) lightmap: &'a LightmapSection,
    pub(super) shadowmask: Option<&'a ShadowmaskAtlasSection>,
    pub(super) cells: &'a CellsSection,
    pub(super) directory: &'a ClusterDirectorySection,
    /// `None` when no id 51 was emitted: the level cannot stream.
    pub(super) residency: Option<&'a CellResidencySetSection>,
}

/// Compute, price and report a level's bound. Runs on every build, cache hits
/// included, because pack is uncached.
pub(super) fn resolve_lightmap_pool_bound(
    inputs: LightmapPoolBoundInputs<'_>,
) -> anyhow::Result<LightmapPoolBound> {
    let LightmapPoolBoundInputs {
        lightmap,
        shadowmask,
        cells,
        directory,
        residency,
    } = inputs;
    if lightmap.blocks.is_empty() {
        return Ok(LightmapPoolBound {
            section: None,
            report: LightmapPoolReport::default(),
        });
    }
    let started = Instant::now();
    let demand = PoolDemand::new(lightmap, shadowmask)?;
    let prices = LayerPrices::of(lightmap)?;
    let all_resident = demand.all_resident()?;
    let bound = match residency {
        Some(residency) => {
            let input = BoundCellInput::new(residency, directory)?;
            let worst = worst_cell_demand(
                &camera_cells(cells),
                |cell, set| input.cells_of(cell, set),
                &CellBlocks::new(lightmap, cells.cells.len())?,
                &demand,
                prices,
            )?;
            Some((input.lead(), worst))
        }
        None => None,
    };
    let elapsed = started.elapsed();
    let section = bound.as_ref().map(|(_, worst)| LightmapPoolBoundSection {
        worst_lightmap_layers: worst.layers.lightmap,
        worst_mask_layers: worst.layers.mask,
    });
    let report = match bound {
        Some((lead, worst)) => {
            LightmapPoolReport::streamed(lead, &worst, all_resident, prices, cells, elapsed)
        }
        None => LightmapPoolReport::all_resident(all_resident, prices, elapsed),
    };
    report.log();
    Ok(LightmapPoolBound { section, report })
}

/// Each cell's blocks, a contiguous run of ids: the runtime's block map refuses
/// any other layout, so this does too rather than bound a level it cannot load.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CellBlocks(Vec<Range<u32>>);

impl CellBlocks {
    fn new(lightmap: &LightmapSection, cell_count: usize) -> anyhow::Result<Self> {
        let mut runs = vec![0..0; cell_count];
        for (block, record) in lightmap.blocks.iter().enumerate() {
            let block = block as u32;
            let run = runs.get_mut(record.cell_id as usize).ok_or_else(|| {
                anyhow::anyhow!(
                    "Lightmap pool bound: block {block} names cell {} past the \
                     {cell_count} cells",
                    record.cell_id
                )
            })?;
            if run.start == run.end {
                *run = block..block + 1;
            } else {
                anyhow::ensure!(
                    run.end == block,
                    "Lightmap pool bound: cell {}'s blocks are not contiguous",
                    record.cell_id
                );
                run.end += 1;
            }
        }
        Ok(Self(runs))
    }

    /// `set`'s blocks into `blocks`, ascending and each once.
    fn collect(&self, set: &[u32], blocks: &mut Vec<u32>) -> anyhow::Result<()> {
        blocks.clear();
        for &cell in set {
            let run = self.0.get(cell as usize).ok_or_else(|| {
                anyhow::anyhow!("Lightmap pool bound: a set names cell {cell} past the level")
            })?;
            blocks.extend(run.clone());
        }
        blocks.sort_unstable();
        blocks.dedup();
        Ok(())
    }
}

/// Each pool's worst layers, and the cell the author should look at.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct WorstDemand {
    /// Each pool's own maximum, across different cells if need be: each pool
    /// is a separate allocation and must hold whichever cell is worst for it.
    layers: PoolLayers,
    /// `None` when the level has no camera cell.
    peak: Option<PeakCell>,
}

/// The cell with the most bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PeakCell {
    cell: u32,
    /// That cell's own layers across both pools, exactly: a pool's margin,
    /// cap and spare are not the cell's, and the cap would tie unequal cells.
    bytes: u64,
}

impl WorstDemand {
    fn offer(&mut self, cell: u32, layers: PoolLayers, prices: LayerPrices) {
        self.layers.lightmap = self.layers.lightmap.max(layers.lightmap);
        self.layers.mask = self.layers.mask.max(layers.mask);
        let bytes = prices.all_resident_bytes(layers);
        let beats = |peak: PeakCell| (bytes, Reverse(cell)) > (peak.bytes, Reverse(peak.cell));
        if self.peak.is_none_or(beats) {
            self.peak = Some(PeakCell { cell, bytes });
        }
    }
}

/// Pack every camera cell's set and keep the worst. `cell_set` is the bound's
/// one cell input. Neighbouring cells often hold the same set, so each distinct
/// block set packs once.
fn worst_cell_demand(
    camera_cells: &[u32],
    mut cell_set: impl FnMut(u32, &mut Vec<u32>),
    cell_blocks: &CellBlocks,
    demand: &PoolDemand,
    prices: LayerPrices,
) -> anyhow::Result<WorstDemand> {
    let mut packed: HashMap<Box<[u32]>, PoolLayers> = HashMap::new();
    let mut set = Vec::new();
    let mut blocks = Vec::with_capacity(demand.block_count());
    let mut scratch = PackScratch::default();
    let mut worst = WorstDemand::default();
    for &cell in camera_cells {
        set.clear();
        cell_set(cell, &mut set);
        cell_blocks.collect(&set, &mut blocks)?;
        let layers = match packed.get(blocks.as_slice()) {
            Some(&layers) => layers,
            None => {
                let layers = demand.layers_of(&blocks, &mut scratch)?;
                packed.insert(blocks.as_slice().into(), layers);
                layers
            }
        };
        worst.offer(cell, layers, prices);
    }
    Ok(worst)
}
