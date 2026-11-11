// Mover probe channel across marks in the direct table: each carried (listed
// cell, mover)'s mid-pose masks, expanded once at install into a run of word
// pairs its mover entries point at. `mover_channel.wgsl` reads a run once per
// (record, probe, mover) to blend across a marked pose.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
//
// Storage sparsity only: a marked pose is one compose blends across, between
// the defined poses around t′. Nothing flags, zeroes or excludes a probe.

use postretro_level_format::mover_probe_channel::MoverProbeChannelSection;

/// A mover entry's across word where its (cell, mover) has no marks.
pub(super) const NO_ACROSS: u32 = u32::MAX;

/// The across runs of the cells a table carries, in section order.
#[derive(Debug, Clone, Default)]
pub(super) struct AcrossRuns {
    /// `((listed position, mover), word offset into `words`)`, ascending.
    runs: Vec<((u32, u32), u32)>,
    /// Per run, per mid pose 1 to N − 2: the mask's low word, then its high.
    words: Vec<u32>,
}

impl AcrossRuns {
    /// The runs of every across entry at a listed position `carried` says
    /// the table carries.
    pub(super) fn new(section: &MoverProbeChannelSection, carried: impl Fn(usize) -> bool) -> Self {
        let mut runs = Self::default();
        for entry in &section.across_entries {
            if !carried(entry.listed as usize) {
                continue;
            }
            runs.runs
                .push(((entry.listed, entry.mover_id), runs.words.len() as u32));
            for &mask in &entry.mid_poses {
                runs.words.extend([mask as u32, (mask >> 32) as u32]);
            }
        }
        runs
    }

    pub(super) fn word_count(&self) -> usize {
        self.words.len()
    }

    /// The absolute word of (listed `position`, `mover`)'s run once the
    /// block sits at word `at`, or [`NO_ACROSS`].
    pub(super) fn run_word(&self, at: usize, position: usize, mover: u32) -> u32 {
        self.runs
            .binary_search_by_key(&(position as u32, mover), |&(key, _)| key)
            .map_or(NO_ACROSS, |found| at as u32 + self.runs[found].1)
    }

    /// Write the block at word `at` of a table.
    pub(super) fn write(&self, words: &mut [u32], at: usize) {
        words[at..at + self.words.len()].copy_from_slice(&self.words);
    }
}
