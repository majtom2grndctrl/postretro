// Id 57's bounce and depth blocks from every mover's states: bounce entries
// per (listed cell, mover) with their affected-probe masks, depth records
// per (mover, probe), and what they weigh.
// See: context/lib/build_pipeline.md §PRL section IDs, §Compiler pipeline step 12

use std::collections::{BTreeMap, BTreeSet};

use postretro_level_format::mover_probe_channel::{
    BounceEntryParts, DepthRecordParts, MOVER_BOUNCE_RESIDENT_BYTE_BAR, MoverBounceEntry,
    MoverDepthRecord,
};
use postretro_level_format::sh_reconstruct::local_xyz;

use super::direct::ProbeCells;
use super::states::MoverBounceStates;
use crate::affinity_grid::AFFINITY_FACTOR;

/// The bounce and depth blocks of id 57, laid out for assembly.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct BounceBlocks {
    pub(crate) bounce: Vec<BounceEntryParts>,
    pub(crate) depth: Vec<DepthRecordParts>,
}

/// What the blocks hold, for the log and the summary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BounceBlockCounts {
    /// Movers with a bounce entry, and with a depth record.
    pub(crate) bounce_movers: usize,
    pub(crate) depth_movers: usize,
    pub(crate) bounce_entries: usize,
    /// Σ affected probes over bounce entries: two tiles each.
    pub(crate) affected_probes: usize,
    pub(crate) bounce_payload_bytes: u64,
    pub(crate) depth_records: usize,
    /// One renderer-owned live tile per distinct depth probe.
    pub(crate) live_tiles: usize,
    /// Two CPU-held closed-pose tiles per depth record.
    pub(crate) closed_pose_tiles: usize,
}

impl BounceBlocks {
    /// Every mover's states over `cells`, id 34's grid.
    pub(crate) fn new(movers: &[MoverBounceStates], cells: &ProbeCells) -> Self {
        let mut blocks = Self::default();
        for states in movers {
            // Each cell's affected probes, x-fastest by local index as the
            // mask reads them (dense probe order is not local order).
            let mut by_cell: BTreeMap<u32, Vec<(usize, &[u16])>> = BTreeMap::new();
            for probe in &states.bounce {
                let (cell, local) = cell_of(probe.probe, cells);
                debug_assert!(
                    cells.valid_masks[cell as usize] & (1 << local) != 0,
                    "an affected probe is valid"
                );
                by_cell
                    .entry(cell)
                    .or_default()
                    .push((local, probe.tiles.as_slice()));
            }
            for (cell, mut probes) in by_cell {
                probes.sort_unstable_by_key(|&(local, _)| local);
                blocks.bounce.push(BounceEntryParts {
                    cell,
                    entry: MoverBounceEntry {
                        mover_id: states.mover_id,
                        affected_probe_mask: probes
                            .iter()
                            .fold(0u64, |mask, &(local, _)| mask | 1 << local),
                    },
                    tiles: probes
                        .into_iter()
                        .flat_map(|(_, tiles)| tiles.iter().copied())
                        .collect(),
                });
            }
            for probe in &states.depth {
                blocks.depth.push(DepthRecordParts {
                    record: MoverDepthRecord {
                        mover_id: states.mover_id,
                        probe_index: probe.probe,
                    },
                    tiles: probe.tiles.clone(),
                });
            }
        }
        blocks
    }

    pub(crate) fn counts(&self) -> BounceBlockCounts {
        let movers = |ids: &mut dyn Iterator<Item = u32>| ids.collect::<BTreeSet<_>>().len();
        BounceBlockCounts {
            bounce_movers: movers(&mut self.bounce.iter().map(|part| part.entry.mover_id)),
            depth_movers: movers(&mut self.depth.iter().map(|part| part.record.mover_id)),
            bounce_entries: self.bounce.len(),
            affected_probes: self
                .bounce
                .iter()
                .map(|part| part.entry.affected_probe_mask.count_ones() as usize)
                .sum(),
            bounce_payload_bytes: self
                .bounce
                .iter()
                .map(|part| part.tiles.len() as u64 * 2)
                .sum(),
            depth_records: self.depth.len(),
            live_tiles: self.live_tile_count(),
            closed_pose_tiles: 2 * self.depth.len(),
        }
    }

    /// Distinct probes carrying mover depth: one live tile each, however many
    /// movers change the probe.
    pub(crate) fn live_tile_count(&self) -> usize {
        self.depth
            .iter()
            .map(|part| part.record.probe_index)
            .collect::<BTreeSet<_>>()
            .len()
    }

    /// Log the blocks once per build. Where each payload is placed is
    /// decided with the direct block, when id 57 assembles.
    pub(crate) fn log(&self) {
        let counts = self.counts();
        log::info!(
            "[MoverLight] MoverProbeChannel (id 57) bounce block: {} entr(y/ies) from {} \
             mover(s), {} affected probe(s), payload {} B (resident bar {} B); depth block: {} \
             record(s) from {} mover(s), {} live tile(s), {} CPU-held closed-pose tile(s)",
            counts.bounce_entries,
            counts.bounce_movers,
            counts.affected_probes,
            counts.bounce_payload_bytes,
            MOVER_BOUNCE_RESIDENT_BYTE_BAR,
            counts.depth_records,
            counts.depth_movers,
            counts.live_tiles,
            counts.closed_pose_tiles,
        );
    }
}

/// The affinity cell of dense probe `probe` and its local x-fastest index.
fn cell_of(probe: u32, cells: &ProbeCells) -> (u32, usize) {
    let [nx, ny, _] = cells.dims;
    let (x, y, z) = (probe % nx, probe / nx % ny, probe / (nx * ny));
    let [ax, ay, _] = cells.affinity_dims;
    let cell = x / AFFINITY_FACTOR + y / AFFINITY_FACTOR * ax + z / AFFINITY_FACTOR * ax * ay;
    let local = (x % AFFINITY_FACTOR
        + (y % AFFINITY_FACTOR) * AFFINITY_FACTOR
        + (z % AFFINITY_FACTOR) * AFFINITY_FACTOR * AFFINITY_FACTOR) as usize;
    debug_assert_eq!(
        local_xyz(local),
        (
            (x % AFFINITY_FACTOR) as usize,
            (y % AFFINITY_FACTOR) as usize,
            (z % AFFINITY_FACTOR) as usize
        )
    );
    (cell, local)
}
