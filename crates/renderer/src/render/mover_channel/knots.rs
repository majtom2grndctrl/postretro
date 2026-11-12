// Mover pose knots after the author ramp, expanded at install: the GPU knot
// block both channel tables carry, and the CPU lookup depth uses (the WGSL
// mirrors the shared interval rule in `mover_channel.wgsl` and
// `mover_bounce.wgsl`).
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
//
// Knots are mover position only: no light, probe or residency reaches them.

use postretro_level_format::mover_probe_channel::MoverProbeChannelSection;

use super::layout::MoverChannelError;

/// Words a mover's knot run starts with: its pose count, then its effective
/// knot count. `(t′ bits, pose)` pairs follow.
const RUN_HEADER_WORDS: usize = 2;

// The knot expansion and the interval rule live with the format, shared with
// the compiler's redundant-pose skipping so the bake judges exactly what
// compose reads.
pub(in crate::render) use postretro_level_format::mover_probe_channel::{
    Knot, pose_interval as interval,
};
#[cfg(test)]
use postretro_level_format::mover_probe_channel::{
    PoseInterval as Interval, ramped_knot as ramped,
};

/// Every mover slot's effective knots and pose count, expanded once at
/// install from id 57's mover records.
#[derive(Clone, Debug, Default)]
pub(in crate::render) struct MoverKnots {
    /// CSR over `knots`, one range per slot.
    offsets: Vec<u32>,
    knots: Vec<Knot>,
    pose_counts: Vec<u32>,
}

impl MoverKnots {
    /// The knots of each mover in `mover_ids` (slot order). Every mover a
    /// block names has a record (the load refuses a section without one).
    pub(in crate::render) fn new(
        section: Option<&MoverProbeChannelSection>,
        mover_ids: &[u32],
    ) -> Result<Self, MoverChannelError> {
        let mut knots = Self {
            offsets: vec![0],
            ..Self::default()
        };
        for &mover in mover_ids {
            let Some(record) = section.and_then(|section| section.mover_record(mover)) else {
                return Err(MoverChannelError(format!(
                    "mover {mover} has entries but no mover record"
                )));
            };
            knots.knots.extend(record.effective_knots());
            knots.offsets.push(knots.knots.len() as u32);
            knots.pose_counts.push(record.pose_count() as u32);
        }
        Ok(knots)
    }

    /// Mover slots the knots cover.
    pub(in crate::render) fn slot_count(&self) -> usize {
        self.pose_counts.len()
    }

    /// Slot `slot`'s effective knots.
    pub(in crate::render) fn slot(&self, slot: usize) -> &[Knot] {
        &self.knots[self.offsets[slot] as usize..self.offsets[slot + 1] as usize]
    }

    /// Slot `slot`'s pose count N.
    pub(in crate::render) fn pose_count(&self, slot: usize) -> usize {
        self.pose_counts[slot] as usize
    }

    /// Slot `slot`'s knot bracket at `t`: how many of its interior knots (1
    /// to E − 2) sit at or below `t`. For a stated `t` below 1 that is the
    /// index of the last knot at or below it. Found once per mover per frame
    /// and carried in the t′ block, so the shaders' interval walk
    /// (`mc_interval`, `mb_entry_blend`) starts there and steps outward only
    /// across poses a probe blends across, at a cost flat in knot count.
    /// Always below E − 1, so the knot above it exists. A per-mover lookup,
    /// never a per-probe flag: no probe, light or residency reaches it. A
    /// subnormal t′ gets the same bracket whether or not a GPU flushes it to
    /// zero, while no interior knot's t′ is itself subnormal, which knot
    /// placement (merged within centimetres of travel) never produces.
    pub(in crate::render) fn bracket(&self, slot: usize, t: f32) -> u32 {
        let knots = self.slot(slot);
        knots[1..knots.len() - 1].partition_point(|knot| knot.t <= t) as u32
    }

    /// `slots` two-pose movers: knots (0, pose 0) and (1, pose 1).
    #[cfg(test)]
    pub(in crate::render) fn termini(slots: usize) -> Self {
        let termini = [Knot { t: 0.0, pose: 0 }, Knot { t: 1.0, pose: 1 }];
        Self {
            offsets: (0..=slots as u32).map(|slot| 2 * slot).collect(),
            knots: (0..slots).flat_map(|_| termini).collect(),
            pose_counts: vec![2; slots],
        }
    }

    /// Words of the GPU knot block: one run word per slot, then each slot's
    /// run.
    pub(in crate::render) fn table_word_count(&self) -> usize {
        self.pose_counts.len() * (1 + RUN_HEADER_WORDS) + self.knots.len() * 2
    }

    /// Write the knot block at word `at` of a table: word `at + slot` holds
    /// the absolute word of slot `slot`'s run, `[N, E, (t′ bits, pose) × E]`.
    pub(in crate::render) fn write_table(&self, words: &mut [u32], at: usize) {
        let slots = self.pose_counts.len();
        let mut run = at + slots;
        for slot in 0..slots {
            words[at + slot] = run as u32;
            let knots = self.slot(slot);
            words[run] = self.pose_counts[slot];
            words[run + 1] = knots.len() as u32;
            for (index, knot) in knots.iter().enumerate() {
                words[run + RUN_HEADER_WORDS + 2 * index] = knot.t.to_bits();
                words[run + RUN_HEADER_WORDS + 2 * index + 1] = knot.pose;
            }
            run += RUN_HEADER_WORDS + 2 * knots.len();
        }
        debug_assert_eq!(run, at + self.table_word_count());
    }
}

#[cfg(test)]
#[path = "knots_tests.rs"]
mod tests;
