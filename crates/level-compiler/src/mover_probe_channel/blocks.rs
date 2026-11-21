// Id 57's bounce and depth blocks from every mover's states: bounce entries
// per (listed cell, mover) with their affected-probe masks, depth records
// per (mover, probe), and what they weigh.
// See: context/lib/build_pipeline.md §PRL section IDs, §Compiler pipeline step 12

use std::collections::{BTreeMap, BTreeSet};

use postretro_level_format::mover_probe_channel::{
    BounceEntryParts, BouncePoseMasks, DepthRecordParts, MOVER_BOUNCE_RESIDENT_BYTE_BAR,
    MOVER_IRRADIANCE_TILE_F16_COUNT, MoverBounceEntry, MoverDepthRecord, PoseMark,
};
use postretro_level_format::sh_reconstruct::local_xyz;

use super::direct::ProbeCells;
use super::states::{BounceProbe, MoverBounceStates};
use crate::affinity_grid::AFFINITY_FACTOR;

/// `--mover-payload-max-size`'s default: 256 MiB of bounce and depth blocks.
pub(crate) const DEFAULT_PAYLOAD_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// The bounce and depth blocks would pass `--mover-payload-max-size` as a
/// mover's blocks join: the build fails by name, never dropping or
/// coarsening a block.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "MoverProbeChannel (id 57): the bounce and depth blocks would hold {bytes} bytes once mover \
     `{mover}` joins, past --mover-payload-max-size {cap} bytes; movers joined so far: {joined}. \
     Nothing is dropped or coarsened. Raise --mover-payload-max-size, or change less with the \
     movers (lower `light_bounces`, fewer or smaller movers)"
)]
pub(crate) struct MoverPayloadCapExceeded {
    /// The blocks' bytes with the mover's joined.
    pub(crate) bytes: u64,
    pub(crate) cap: u64,
    /// The mover whose blocks would pass the cap.
    pub(crate) mover: String,
    /// The movers whose blocks joined before it, in job order, or `none`.
    pub(crate) joined: String,
}

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
    /// Σ affected probes over bounce entries.
    pub(crate) affected_probes: usize,
    /// Affected probes storing a state at some poses only: the others read
    /// id 34 (at a terminus or a mid pose that marks the base) or blend
    /// across (at a mid pose that marks nothing).
    pub(crate) one_sided_probes: usize,
    pub(crate) bounce_payload_bytes: u64,
    pub(crate) depth_records: usize,
    /// Depth records storing a tile at some poses only.
    pub(crate) one_sided_records: usize,
    /// One renderer-owned live tile per distinct depth probe.
    pub(crate) live_tiles: usize,
    /// CPU-held closed-pose tiles: one per stored (record, pose).
    pub(crate) closed_pose_tiles: usize,
}

impl BounceBlocks {
    /// Every mover's states over `cells`, id 34's grid.
    #[cfg(test)]
    pub(crate) fn new(movers: &[MoverBounceStates], cells: &ProbeCells) -> Self {
        let mut blocks = Self::default();
        for states in movers {
            blocks.push_mover(states, cells);
        }
        blocks
    }

    /// Append one mover's skipped states over `cells`, id 34's grid. No
    /// offset spans movers, and assembly sorts bounce entries by (cell,
    /// mover) and depth records by their order key, so the order movers
    /// join in never reaches the section's bytes.
    ///
    /// Every vector is sized exactly, so the blocks grow by exactly
    /// [`Self::mover_bytes`].
    pub(crate) fn push_mover(&mut self, states: &MoverBounceStates, cells: &ProbeCells) {
        #[cfg(debug_assertions)]
        let before = self.heap_bytes();
        #[cfg(debug_assertions)]
        let predicted = Self::mover_bytes(states, cells);
        let blocks = &mut *self;
        let by_cell = bounce_by_cell(states, cells);
        blocks.bounce.reserve_exact(by_cell.len());
        blocks.depth.reserve_exact(states.depth.len());
        for (cell, mut probes) in by_cell {
            probes.sort_unstable_by_key(|&(local, _)| local);
            let mut poses = vec![BouncePoseMasks::default(); states.pose_count];
            for &(local, probe) in &probes {
                debug_assert_eq!(probe.marks.len(), states.pose_count);
                for (masks, mark) in poses.iter_mut().zip(&probe.marks) {
                    match mark {
                        PoseMark::Stored => masks.stored |= 1 << local,
                        PoseMark::Base => masks.base |= 1 << local,
                        PoseMark::Unmarked => {}
                    }
                }
            }
            // A probe's tiles run pose by pose; the section's run pose
            // after pose, then probe by stored rank.
            let stored: usize = probes
                .iter()
                .map(|(_, probe)| stored_tiles(&probe.marks))
                .sum();
            let mut tiles = Vec::with_capacity(stored * MOVER_IRRADIANCE_TILE_F16_COUNT);
            for pose in 0..states.pose_count {
                for &(_, probe) in &probes {
                    if probe.marks[pose] == PoseMark::Stored {
                        let at = probe.marks[..pose]
                            .iter()
                            .filter(|&&mark| mark == PoseMark::Stored)
                            .count();
                        tiles.extend_from_slice(
                            &probe.tiles[at * MOVER_IRRADIANCE_TILE_F16_COUNT..]
                                [..MOVER_IRRADIANCE_TILE_F16_COUNT],
                        );
                    }
                }
            }
            blocks.bounce.push(BounceEntryParts {
                cell,
                entry: MoverBounceEntry {
                    mover_id: states.mover_id,
                    poses,
                },
                tiles,
            });
        }
        for probe in &states.depth {
            blocks.depth.push(DepthRecordParts {
                record: MoverDepthRecord {
                    mover_id: states.mover_id,
                    probe_index: probe.probe,
                    marks: probe.marks.clone(),
                },
                tiles: probe.tiles.clone(),
            });
        }
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            self.heap_bytes(),
            before + predicted,
            "a mover's blocks grow the blocks by exactly their predicted bytes"
        );
    }

    /// What the blocks hold, by capacity: every byte
    /// `--mover-payload-max-size` counts.
    pub(crate) fn heap_bytes(&self) -> u64 {
        let bounce: usize = self
            .bounce
            .iter()
            .map(|part| {
                part.tiles.capacity() * size_of::<u16>()
                    + part.entry.poses.capacity() * size_of::<BouncePoseMasks>()
            })
            .sum();
        let depth: usize = self
            .depth
            .iter()
            .map(|part| {
                part.tiles.capacity() * size_of::<[u16; 2]>()
                    + part.record.marks.capacity() * size_of::<PoseMark>()
            })
            .sum();
        (self.bounce.capacity() * size_of::<BounceEntryParts>()
            + self.depth.capacity() * size_of::<DepthRecordParts>()
            + bounce
            + depth) as u64
    }

    /// The bytes [`Self::push_mover`] adds for `states`, worked out from
    /// the states alone, before any block is encoded: the payload cap is
    /// checked against it, so blocks past the cap are never built.
    pub(crate) fn mover_bytes(states: &MoverBounceStates, cells: &ProbeCells) -> u64 {
        let entries = bounce_by_cell(states, cells).len();
        let stored: usize = states
            .bounce
            .iter()
            .map(|probe| stored_tiles(&probe.marks))
            .sum();
        let bounce = entries
            * (size_of::<BounceEntryParts>() + states.pose_count * size_of::<BouncePoseMasks>())
            + stored * MOVER_IRRADIANCE_TILE_F16_COUNT * size_of::<u16>();
        let depth: usize = states
            .depth
            .iter()
            .map(|probe| {
                size_of::<DepthRecordParts>()
                    + probe.marks.len() * size_of::<PoseMark>()
                    + probe.tiles.len() * size_of::<[u16; 2]>()
            })
            .sum();
        (bounce + depth) as u64
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
                .map(|part| part.entry.affected_probe_mask().count_ones() as usize)
                .sum(),
            one_sided_probes: self
                .bounce
                .iter()
                .map(|part| {
                    let every = part
                        .entry
                        .poses
                        .iter()
                        .fold(u64::MAX, |mask, pose| mask & pose.stored);
                    (part.entry.affected_probe_mask() & !every).count_ones() as usize
                })
                .sum(),
            bounce_payload_bytes: self
                .bounce
                .iter()
                .map(|part| part.tiles.len() as u64 * 2)
                .sum(),
            depth_records: self.depth.len(),
            one_sided_records: self
                .depth
                .iter()
                .filter(|part| part.record.stored_tile_count() < part.record.pose_count())
                .count(),
            live_tiles: self.live_tile_count(),
            closed_pose_tiles: self
                .depth
                .iter()
                .map(|part| part.record.stored_tile_count())
                .sum(),
        }
    }

    /// Distinct probes carrying mover depth: one live tile each, however many
    /// movers change the probe.
    pub(crate) fn live_tile_count(&self) -> usize {
        self.live_probes().len()
    }

    /// The probes carrying mover depth, each composing a live tile.
    pub(crate) fn live_probes(&self) -> BTreeSet<u32> {
        self.depth
            .iter()
            .map(|part| part.record.probe_index)
            .collect()
    }

    /// Log the blocks once per build. Where each payload is placed is
    /// decided with the direct block, when id 57 assembles.
    pub(crate) fn log(&self) {
        let counts = self.counts();
        log::info!(
            "[MoverLight] MoverProbeChannel (id 57) bounce block: {} entr(y/ies) from {} \
             mover(s), {} affected probe(s) ({} storing some poses only), payload {} B (resident \
             bar {} B); depth block: {} record(s) ({} storing some poses only) from {} mover(s), {} \
             live tile(s), {} CPU-held closed-pose tile(s)",
            counts.bounce_entries,
            counts.bounce_movers,
            counts.affected_probes,
            counts.one_sided_probes,
            counts.bounce_payload_bytes,
            MOVER_BOUNCE_RESIDENT_BYTE_BAR,
            counts.depth_records,
            counts.one_sided_records,
            counts.depth_movers,
            counts.live_tiles,
            counts.closed_pose_tiles,
        );
    }
}

/// Each cell's affected probes of `states`, x-fastest by local index as the
/// masks read them (dense probe order is not local order).
fn bounce_by_cell<'s>(
    states: &'s MoverBounceStates,
    cells: &ProbeCells,
) -> BTreeMap<u32, Vec<(usize, &'s BounceProbe)>> {
    let mut by_cell: BTreeMap<u32, Vec<(usize, &BounceProbe)>> = BTreeMap::new();
    for probe in &states.bounce {
        let (cell, local) = cell_of(probe.probe, cells);
        debug_assert!(
            cells.valid_masks[cell as usize] & (1 << local) != 0,
            "an affected probe is valid"
        );
        by_cell.entry(cell).or_default().push((local, probe));
    }
    by_cell
}

/// Tiles a probe's marks store.
fn stored_tiles(marks: &[PoseMark]) -> usize {
    marks
        .iter()
        .filter(|&&mark| mark == PoseMark::Stored)
        .count()
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
