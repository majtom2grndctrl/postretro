// Gated-texel rows keyed (mover, terminus, light), merged from partitions.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::collections::BTreeMap;

use super::pose::{MoverPoses, Terminus};
use super::texels::MoverTexels;
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
/// samples.
#[derive(Debug, Clone, Default)]
pub(crate) struct RowTally {
    pub(crate) gated_texels: u64,
    /// Of `gated_texels`, those gated at the mover's other terminus too.
    pub(crate) shared_texels: u64,
    /// Whether the light sits inside a posed brush, so every lit texel counts.
    pub(crate) light_inside: bool,
    /// World bounds of the gated texel centres.
    pub(crate) texel_centres: Option<Aabb>,
}

impl RowTally {
    /// Gated here and not at the other terminus.
    pub(crate) fn differing_texels(&self) -> u64 {
        self.gated_texels - self.shared_texels
    }
}

/// Every row with a gated texel, in report order whatever order partitions
/// arrived in.
#[derive(Debug, Clone, Default)]
pub(crate) struct GateTally {
    rows: BTreeMap<RowKey, RowTally>,
}

impl GateTally {
    /// Fold one partition's per-mover counts for light `source_index`.
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
                if let Some(centres) = &counts.centres[k] {
                    row.texel_centres
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
