// Gated texel and probe rows keyed (mover, terminus, light), merged from the
// static partitions, the animated weights and the probe gate.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::collections::BTreeMap;

use super::pose::{MoverPoses, Terminus};
use super::probes::MoverProbes;
use super::rows::{MoverTexels, mask_estimate};
use crate::partition::Aabb;

/// A row's key. Field order is the report's order: `mover_id`, then
/// terminus (first, last), then light in authored entity order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RowKey {
    pub(crate) mover_id: u32,
    pub(crate) terminus: Terminus,
    /// The light's index in the map's light list.
    pub(crate) source_index: usize,
}

/// One (mover, terminus, light) row's counts. Every count is an estimate:
/// one centre segment stands in for the bake's footprint and emitter
/// samples, and for the SH bake's probe ray set.
#[derive(Debug, Clone, Default)]
pub(crate) struct RowTally {
    pub(crate) gated_texels: u64,
    /// Of `gated_texels`, those gated at the mover's other terminus too.
    pub(crate) shared_texels: u64,
    /// Whether the light sits inside a posed brush, so every lit texel counts.
    pub(crate) light_inside: bool,
    /// World bounds of the gated texel centres.
    pub(crate) texel_centres: Option<Aabb>,
    /// Distinct lightmap cell blocks holding a gated texel.
    pub(crate) touched_blocks: u64,
    /// Distinct lightmap cell blocks holding a differing texel.
    pub(crate) differing_blocks: u64,
    /// Valid SH probes the light reaches whose centre segment touches the
    /// pose. Never in the byte estimate, which prices texel masks alone.
    pub(crate) gated_probes: u64,
    /// Of `gated_probes`, those gated at the mover's other terminus too.
    pub(crate) shared_probes: u64,
    /// World bounds of the gated probe centres.
    pub(crate) probe_centres: Option<Aabb>,
}

impl RowTally {
    /// Gated here and not at the other terminus.
    pub(crate) fn differing_texels(&self) -> u64 {
        self.gated_texels - self.shared_texels
    }

    /// Probes gated here and not at the other terminus.
    pub(crate) fn differing_probes(&self) -> u64 {
        self.gated_probes - self.shared_probes
    }

    /// Estimated bytes of a mask over every gated texel.
    pub(crate) fn every_gated_estimate(&self) -> u64 {
        mask_estimate(self.gated_texels, self.touched_blocks)
    }

    /// Estimated bytes of a mask over the differing texels alone.
    pub(crate) fn differing_only_estimate(&self) -> u64 {
        mask_estimate(self.differing_texels(), self.differing_blocks)
    }
}

/// Every row with a gated texel or probe, in report order whatever order
/// partitions, tiers or lights arrived in.
#[derive(Debug, Clone, Default)]
pub(crate) struct GateTally {
    rows: BTreeMap<RowKey, RowTally>,
}

impl GateTally {
    /// Fold one partition's per-mover texel counts for light
    /// `source_index`: a static light's partition on one bake layer, or an
    /// animated light's whole weight map.
    ///
    /// Block counts add across a light's partitions without double counting:
    /// a partition holds one bake layer, and a cell block lies wholly in one
    /// layer (`CellBlock::layer`), so no block reaches two partitions of one
    /// light. An animated light folds once, its blocks already distinct.
    pub(crate) fn add_partition(
        &mut self,
        poses: &MoverPoses,
        source_index: usize,
        movers: &[MoverTexels],
    ) {
        for counts in movers {
            let mover_id = poses.analysed[counts.mover].mover_id;
            for terminus in Terminus::BOTH {
                let k = terminus.index();
                if counts.gated[k] == 0 {
                    continue;
                }
                let row = self
                    .rows
                    .entry(RowKey {
                        mover_id,
                        terminus,
                        source_index,
                    })
                    .or_default();
                row.gated_texels += counts.gated[k];
                row.shared_texels += counts.both;
                row.light_inside |= counts.light_inside[k];
                row.touched_blocks += counts.blocks[k].len() as u64;
                row.differing_blocks += counts.differing_blocks[k].len() as u64;
                if let Some(centres) = &counts.centres[k] {
                    row.texel_centres
                        .get_or_insert_with(Aabb::empty)
                        .expand_aabb(centres);
                }
            }
        }
    }

    /// Fold light `source_index`'s per-mover probe counts; once per light.
    pub(crate) fn add_probes(
        &mut self,
        poses: &MoverPoses,
        source_index: usize,
        movers: &[MoverProbes],
    ) {
        for counts in movers {
            let mover_id = poses.analysed[counts.mover].mover_id;
            for terminus in Terminus::BOTH {
                let k = terminus.index();
                if counts.gated[k] == 0 {
                    continue;
                }
                let row = self
                    .rows
                    .entry(RowKey {
                        mover_id,
                        terminus,
                        source_index,
                    })
                    .or_default();
                row.gated_probes += counts.gated[k];
                row.shared_probes += counts.both;
                row.light_inside |= counts.light_inside[k];
                if let Some(centres) = &counts.centres[k] {
                    row.probe_centres
                        .get_or_insert_with(Aabb::empty)
                        .expand_aabb(centres);
                }
            }
        }
    }

    pub(crate) fn rows(&self) -> impl Iterator<Item = (&RowKey, &RowTally)> {
        self.rows.iter()
    }
}
