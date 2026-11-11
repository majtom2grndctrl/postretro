// Mover pose knots after the author ramp, and the interval rule every mover
// state consumer blends by: install-time knot expansion, the GPU knot block
// both channel tables carry, and the CPU lookup depth uses (the WGSL mirrors
// it in `mover_channel.wgsl` and `mover_bounce.wgsl`).
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
//
// Knots are mover position only: no light, probe or residency reaches them.

use postretro_level_format::mover_probe_channel::{MoverProbeChannelSection, MoverRecord};

use super::layout::MoverChannelError;

/// Words a mover's knot run starts with: its pose count, then its effective
/// knot count. `(t′ bits, pose)` pairs follow.
const RUN_HEADER_WORDS: usize = 2;

/// One pose compose can read, at its t′.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::render) struct Knot {
    pub(in crate::render) t: f32,
    pub(in crate::render) pose: u32,
}

/// `clamp((t − s) / (e − s), 0, 1)`: the game's ramp
/// (`runtime_movers::travel::ramped_travel_fraction`), mirrored operation
/// for operation so a knot lands on the very t′ the game hands over at that
/// path position. −0.0 reads as 0, as the hand-over stores it.
fn ramped(t: f32, start: f32, end: f32) -> f32 {
    let span = end - start;
    let ramped = if span.is_finite() && span > 0.0 && start.is_finite() {
        (t - start) / span
    } else {
        t
    };
    if ramped.is_finite() {
        ramped.clamp(0.0, 1.0) + 0.0
    } else {
        0.0
    }
}

/// `record`'s effective knots: pose 0 at t′ 0, every pose whose ramped knot
/// falls strictly inside (0, 1), and the last pose at t′ 1. A knot the ramp
/// squeezes onto 0 or 1 is never read, so the first or last pose's state
/// holds across each ramp edge; t′ strictly ascends.
pub(in crate::render) fn effective_knots(record: &MoverRecord) -> Vec<Knot> {
    let last = record.knots.len().saturating_sub(1);
    let mut knots = vec![Knot { t: 0.0, pose: 0 }];
    for (pose, &knot) in record.knots.iter().enumerate().take(last).skip(1) {
        let t = ramped(knot, record.ramp_start, record.ramp_end);
        // Strictly above the previous knot: a squeezed pose, or one rounding
        // onto its neighbour's t′, would open an empty interval.
        if t < 1.0 && t > knots[knots.len() - 1].t {
            knots.push(Knot {
                t,
                pose: pose as u32,
            });
        }
    }
    knots.push(Knot {
        t: 1.0,
        pose: last as u32,
    });
    knots
}

/// Where a mover's t′ falls among the poses one probe defines: blend pose
/// `lo`'s state toward pose `hi`'s by `weight`. `weight == 0` reads pose
/// `lo` exactly (`hi == lo` there).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::render) struct Interval {
    pub(in crate::render) lo: u32,
    pub(in crate::render) hi: u32,
    pub(in crate::render) weight: f32,
}

impl Interval {
    fn exact(pose: u32) -> Self {
        Self {
            lo: pose,
            hi: pose,
            weight: 0.0,
        }
    }
}

/// The interval rule: the defined poses around `t` (a stated t′, ≥ 0) among
/// `knots`, and the blend weight between them. The termini always define a
/// state (stored, or the base); `defined(pose)` says whether a mid pose
/// does, and an undefined one is blended across. At a defined knot the
/// state reads exactly; t′ ≤ 0 reads the first pose and t′ ≥ 1 the last.
/// The lookup is a function of t′ alone, so out and back to one t′ reads
/// alike. The shaders' `mc_interval` and `mb_entry_blend` mirror it.
pub(in crate::render) fn interval(
    knots: &[Knot],
    t: f32,
    defined: impl Fn(u32) -> bool,
) -> Interval {
    let last = knots.len() - 1;
    if t >= 1.0 {
        return Interval::exact(knots[last].pose);
    }
    if t <= 0.0 || t.is_nan() {
        return Interval::exact(knots[0].pose);
    }
    let is_defined = |at: usize| at == 0 || at == last || defined(knots[at].pose);
    // The last knot at or below t′: knot 0 sits at 0 < t′, knot `last` at
    // 1 > t′.
    let at = knots.partition_point(|knot| knot.t <= t) - 1;
    let mut lo = at;
    while !is_defined(lo) {
        lo -= 1;
    }
    if knots[lo].t >= t {
        return Interval::exact(knots[lo].pose);
    }
    let mut hi = at + 1;
    while !is_defined(hi) {
        hi += 1;
    }
    Interval {
        lo: knots[lo].pose,
        hi: knots[hi].pose,
        weight: (t - knots[lo].t) / (knots[hi].t - knots[lo].t),
    }
}

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
            knots.knots.extend(effective_knots(record));
            knots.offsets.push(knots.knots.len() as u32);
            knots.pose_counts.push(record.pose_count() as u32);
        }
        Ok(knots)
    }

    /// Slot `slot`'s effective knots.
    pub(in crate::render) fn slot(&self, slot: usize) -> &[Knot] {
        &self.knots[self.offsets[slot] as usize..self.offsets[slot + 1] as usize]
    }

    /// Slot `slot`'s pose count N.
    pub(in crate::render) fn pose_count(&self, slot: usize) -> usize {
        self.pose_counts[slot] as usize
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
