// Mover probe channel across marks: which probes each analysed mover's mid
// poses enclose, recorded once per (cell, mover) so direct compose blends
// across an enclosed mid pose as bounce does.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)
//
// Enclosure is geometric: it depends on (mover, probe, pose) alone, never on
// a light. The marks are storage sparsity: compose blends between the
// neighbouring stored states and never zeroes a weight, flags or excludes a
// probe.

use std::collections::BTreeMap;

use postretro_level_format::mover_probe_channel::{
    AcrossEntryParts, BounceEntryParts, DirectRecordParts,
};
use rayon::prelude::*;

use super::direct::ProbeCells;
use super::posed_solid::PosedMovers;
use crate::sh_bake::{ProbeGridLayout, vec3_from};

/// Every analysed mover's enclosed (probe, mid pose) pairs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EnclosedMidPoses {
    /// Ascending by mover id.
    movers: Vec<MoverEnclosures>,
}

/// One mover's pose count and the (dense probe, mid pose) pairs its posed
/// brushes enclose, ascending.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MoverEnclosures {
    mover_id: u32,
    pose_count: usize,
    pairs: Vec<(u32, u32)>,
}

impl EnclosedMidPoses {
    /// Each mid pose's enclosed valid probes, by the same test and probe
    /// position every posed trace reads ([`super::posed_solid::PosedSolid::encloses`]).
    /// Termini are left out: an enclosing terminus reads the base.
    pub(crate) fn find(movers: &PosedMovers, layout: &ProbeGridLayout) -> Self {
        let movers = movers
            .movers()
            .iter()
            .map(|mover| {
                let pose_count = mover.solids.len();
                let mids = pose_count.saturating_sub(1);
                let mut pairs: Vec<(u32, u32)> = (1..mids)
                    .into_par_iter()
                    .flat_map_iter(|pose| {
                        let solid = &mover.solids[pose];
                        let bounds = solid.bounds();
                        layout
                            .probe_positions
                            .iter()
                            .enumerate()
                            .filter(move |&(probe, position)| {
                                layout.validity[probe] != 0
                                    && position.cmpge(bounds.min).all()
                                    && position.cmple(bounds.max).all()
                            })
                            .filter_map(move |(probe, &position)| {
                                solid
                                    .encloses(vec3_from(position))
                                    .then_some((probe as u32, pose as u32))
                            })
                    })
                    .collect();
                pairs.sort_unstable();
                MoverEnclosures {
                    mover_id: mover.mover_id,
                    pose_count,
                    pairs,
                }
            })
            .collect();
        Self { movers }
    }

    /// The across entries for the (cell, mover) pairs the section names:
    /// each mid pose's enclosed probes among those `direct`'s records of the
    /// mover mask or its `bounce` entry affects at the cell. A pair with no
    /// enclosed named probe writes nothing.
    pub(crate) fn parts(
        &self,
        direct: &[DirectRecordParts],
        bounce: &[BounceEntryParts],
        cells: &ProbeCells,
    ) -> Vec<AcrossEntryParts> {
        let mut named: BTreeMap<(u32, u32), u64> = BTreeMap::new();
        for part in direct {
            for (mover_id, _) in &part.movers {
                *named.entry((part.cell, *mover_id)).or_default() |= part.record.probe_mask;
            }
        }
        for part in bounce {
            *named.entry((part.cell, part.entry.mover_id)).or_default() |=
                part.entry.affected_probe_mask();
        }
        let mut parts = Vec::new();
        for ((cell, mover_id), mask) in named {
            let Some(mover) = self.mover(mover_id) else {
                continue;
            };
            let pairs = mover.pairs.as_slice();
            let mut mid_poses = vec![0u64; mover.pose_count.saturating_sub(2)];
            for (local, probe) in cells.valid_locals(cell as usize) {
                if mask & (1 << local) == 0 {
                    continue;
                }
                let first = pairs.partition_point(|&(at, _)| at < probe as u32);
                for &(_, pose) in pairs[first..]
                    .iter()
                    .take_while(|&&(at, _)| at == probe as u32)
                {
                    mid_poses[pose as usize - 1] |= 1 << local;
                }
            }
            if mid_poses.iter().any(|&marks| marks != 0) {
                parts.push(AcrossEntryParts {
                    cell,
                    mover_id,
                    mid_poses,
                });
            }
        }
        parts
    }

    fn mover(&self, mover_id: u32) -> Option<&MoverEnclosures> {
        self.movers
            .binary_search_by_key(&mover_id, |mover| mover.mover_id)
            .ok()
            .map(|at| &self.movers[at])
    }

    /// Whether mid pose `pose` of `mover_id` encloses dense probe `probe`.
    #[cfg(test)]
    pub(crate) fn encloses(&self, mover_id: u32, probe: u32, pose: u32) -> bool {
        self.mover(mover_id)
            .is_some_and(|mover| mover.pairs.binary_search(&(probe, pose)).is_ok())
    }
}
