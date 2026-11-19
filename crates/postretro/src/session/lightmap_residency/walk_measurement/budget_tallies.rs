//! The drain-budget spike's per-run tallies: late items by cause and drain admissions, spawn step apart.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency, shared drain budget)

// Units: lightmap late items are block-frames (a drawn block not installed
// after the frame's last drain, once per frame); SH's per-frame tally is
// cluster-frames (a drawn cluster not Sampleable at the frame's first target
// update, once per frame); SH's episode buckets count episodes, each filed by
// its first frame. The spawn step admits a level-entry burst at the play cap,
// which production settles at its own cap and presents nothing, so its
// figures stay out of the after-spawn totals.

use postretro_renderer::{LightmapMissBuckets, ShMissBuckets};
use serde_json::{Value, json};

use super::drain_tally::DrainTally;
use super::frame_driver::FrameOutcome;
use super::sh_walk::ShRunStats;
use crate::lightmap_streaming::controller::LightmapResidencyCounters;

const LIGHTMAP_CAUSES: [&str; 9] = [
    "outside_reach",
    "read_in_flight",
    "waiting_on_permit",
    "waiting_on_in_hand_cap",
    "held_by_drain_budget",
    "awaiting_renderer_drain",
    "deferred_by_pool",
    "refused",
    "failed",
];

fn lightmap_causes(b: &LightmapMissBuckets) -> [u64; 9] {
    [
        b.outside_reach,
        b.read_in_flight,
        b.waiting_on_permit,
        b.waiting_on_in_hand_cap,
        b.held_by_drain_budget,
        b.awaiting_renderer_drain,
        b.deferred_by_pool,
        b.refused,
        b.failed,
    ]
}

const SH_CAUSES: [&str; 8] = [
    "outside_reach",
    "trimmed_by_pressure",
    "read_in_flight",
    "held_by_drain_budget",
    "awaiting_compose",
    "failed",
    "device_limited",
    "at_whole_map_cap",
];

fn sh_causes(b: &ShMissBuckets) -> [u64; 8] {
    [
        b.outside_reach,
        b.trimmed_by_pressure,
        b.read_in_flight,
        b.held_by_drain_budget,
        b.awaiting_compose,
        b.failed,
        b.device_limited,
        b.at_whole_map_cap,
    ]
}

fn add<const N: usize>(into: &mut [u64; N], from: [u64; N]) {
    for (total, value) in into.iter_mut().zip(from) {
        *total += value;
    }
}

fn causes_line<const N: usize>(names: [&str; N], values: [u64; N]) -> String {
    names
        .iter()
        .zip(values)
        .map(|(name, value)| format!("{} {value}", name.replace('_', " ")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One step kind's late lightmap block-frames: the controller's buckets, and
/// its count beside the walk's own tally over every drawn block.
#[derive(Debug, Default, Clone, Copy)]
struct LightmapLate {
    buckets: [u64; 9],
    /// The controller's drawn-not-resident block-frames.
    controller: u64,
    /// The walk's tally past the controller's, summed per frame: drawn blocks
    /// a held frame leaves uncounted, because the controller counts only the
    /// drawn blocks it holds.
    uncounted: u64,
    uncounted_frames: u64,
    /// The controller's count past the walk's, summed per frame.
    beyond_walk: u64,
}

impl LightmapLate {
    fn add(&mut self, outcome: &FrameOutcome) {
        add(&mut self.buckets, lightmap_causes(&outcome.lightmap_misses));
        let controller = u64::from(outcome.not_resident);
        let walk = u64::from(outcome.deferred) + u64::from(outcome.left_behind);
        self.controller += controller;
        self.uncounted += walk.saturating_sub(controller);
        self.uncounted_frames += u64::from(walk > controller);
        self.beyond_walk += controller.saturating_sub(walk);
    }

    fn sum(&self) -> u64 {
        self.buckets.iter().sum()
    }
}

/// One step kind's late SH cluster-frames by cause, classified every frame.
#[derive(Debug, Default, Clone, Copy)]
struct ShLateFrames {
    buckets: [u64; 8],
}

#[derive(Debug, Default, Clone)]
struct StepTallies {
    lightmap: LightmapLate,
    sh: ShLateFrames,
    drains: DrainTally,
}

impl StepTallies {
    fn add(&mut self, outcome: &FrameOutcome) {
        self.lightmap.add(outcome);
        if let Some(frame) = &outcome.sh_miss_frames {
            add(&mut self.sh.buckets, sh_causes(frame));
        }
        for &sample in &outcome.drain_samples[..outcome.drains as usize] {
            self.drains.add(sample);
        }
    }
}

/// A run's tallies, the spawn step's apart from the rest.
#[derive(Debug, Default, Clone)]
pub(super) struct BudgetTallies {
    spawn: StepTallies,
    play: StepTallies,
}

impl BudgetTallies {
    pub(super) fn add(&mut self, spawn_step: bool, outcome: &FrameOutcome) {
        if spawn_step {
            self.spawn.add(outcome);
        } else {
            self.play.add(outcome);
        }
    }

    /// Each step kind's lightmap buckets sum to its drawn-not-resident
    /// block-frames, and both together equal the controller's run totals:
    /// every counted frame was read once.
    pub(super) fn assert_sums(&self, counters: &LightmapResidencyCounters) {
        for (label, step) in [("spawn step", &self.spawn), ("after spawn", &self.play)] {
            assert_eq!(
                step.lightmap.sum(),
                step.lightmap.controller,
                "{label}: lightmap cause buckets sum to the controller's drawn-not-resident \
                 block-frames"
            );
        }
        assert_eq!(
            self.spawn.lightmap.controller + self.play.lightmap.controller,
            counters.drawn_not_resident,
            "the frames read cover the controller's drawn-not-resident block-frames"
        );
        let mut whole = self.spawn.lightmap.buckets;
        add(&mut whole, self.play.lightmap.buckets);
        assert_eq!(
            whole,
            lightmap_causes(&counters.miss_buckets),
            "the frames read cover the controller's cause buckets"
        );
    }

    pub(super) fn print(&self, sh: Option<&ShRunStats>) {
        for (label, step) in [("after spawn", &self.play), ("spawn step", &self.spawn)] {
            let lightmap = &step.lightmap;
            println!(
                "   lightmap late {label} by cause (block-frames): {}; sum {} = controller \
                 drawn-not-resident {}",
                causes_line(LIGHTMAP_CAUSES, lightmap.buckets),
                lightmap.sum(),
                lightmap.controller,
            );
        }
        let (play, spawn) = (&self.play.lightmap, &self.spawn.lightmap);
        println!(
            "   drawn blocks a held frame leaves uncounted (walk tally past the controller's, \
             block-frames): after spawn {} in {} frames, spawn step {} in {} frames; controller \
             past the walk tally: after spawn {}, spawn step {}",
            play.uncounted,
            play.uncounted_frames,
            spawn.uncounted,
            spawn.uncounted_frames,
            play.beyond_walk,
            spawn.beyond_walk,
        );
        println!("   drains after spawn: {}", self.play.drains.line());
        println!("   drains, spawn step: {}", self.spawn.drains.line());
        let Some(sh) = sh else {
            return;
        };
        println!(
            "   SH late per frame by cause (cluster-frames, classified every frame): after spawn \
             {}; spawn step {}",
            causes_line(SH_CAUSES, self.play.sh.buckets),
            causes_line(SH_CAUSES, self.spawn.sh.buckets),
        );
        let (after, at_spawn) = episodes(sh);
        println!(
            "   SH late episodes by cause (episodes, filed by each episode's first frame): after \
             spawn {}; spawn step {}",
            causes_line(SH_CAUSES, after),
            causes_line(SH_CAUSES, at_spawn),
        );
    }

    /// The result keys: lightmap buckets as `lm_late_<cause>`, SH's per-frame
    /// tally as `sh_frames_<cause>` and its episodes as `sh_episodes_<cause>`
    /// (null with SH off), and the drain keys; each also with `_spawn`.
    pub(super) fn json(&self, sh: Option<&ShRunStats>) -> serde_json::Map<String, Value> {
        let mut keys = serde_json::Map::new();
        let episodes = sh.map(episodes);
        for (suffix, step, sh_episodes) in [
            ("", &self.play, episodes.map(|(after, _)| after)),
            (
                "_spawn",
                &self.spawn,
                episodes.map(|(_, at_spawn)| at_spawn),
            ),
        ] {
            let lightmap = &step.lightmap;
            for (name, value) in LIGHTMAP_CAUSES.iter().zip(lightmap.buckets) {
                keys.insert(format!("lm_late_{name}{suffix}"), json!(value));
            }
            for (key, value) in [
                ("lm_drawn_not_resident", lightmap.controller),
                ("lm_held_frame_uncounted", lightmap.uncounted),
                ("lm_counted_beyond_walk", lightmap.beyond_walk),
            ] {
                keys.insert(format!("{key}{suffix}"), json!(value));
            }
            for (index, name) in SH_CAUSES.iter().enumerate() {
                keys.insert(
                    format!("sh_frames_{name}{suffix}"),
                    json!(sh.map(|_| step.sh.buckets[index])),
                );
                keys.insert(
                    format!("sh_episodes_{name}{suffix}"),
                    json!(sh_episodes.map(|buckets| buckets[index])),
                );
            }
            step.drains.json(suffix, &mut keys);
        }
        keys
    }
}

/// SH's episode buckets after the spawn step and at it.
fn episodes(sh: &ShRunStats) -> ([u64; 8], [u64; 8]) {
    let at_spawn = sh_causes(&sh.spawn_buckets);
    let mut after = sh_causes(&sh.counters.miss_buckets);
    for (total, spawn) in after.iter_mut().zip(at_spawn) {
        *total -= spawn;
    }
    (after, at_spawn)
}

#[cfg(test)]
mod tests {
    use super::super::drain_tally::DrainSample;
    use super::*;

    fn frame(misses: LightmapMissBuckets, deferred: u32, left_behind: u32) -> FrameOutcome {
        FrameOutcome {
            not_resident: misses.total() as u32,
            lightmap_misses: misses,
            deferred,
            left_behind,
            drains: 1,
            ..FrameOutcome::default()
        }
    }

    // P9, P13: the spawn step's buckets stay apart from play's; each sums to
    // its own drawn-not-resident count; a held frame's walk-tally excess is
    // the uncounted line, not a bucket.
    #[test]
    fn spawn_and_play_sum_apart_and_a_held_frame_gap_is_reported_alone() {
        let held = LightmapMissBuckets {
            read_in_flight: 2,
            ..LightmapMissBuckets::default()
        };
        let budget = LightmapMissBuckets {
            held_by_drain_budget: 3,
            ..LightmapMissBuckets::default()
        };
        let mut tallies = BudgetTallies::default();
        tallies.add(true, &frame(budget, 0, 3));
        tallies.add(false, &frame(held, 0, 5));
        tallies.add(false, &frame(budget, 1, 2));

        let counters = LightmapResidencyCounters {
            drawn_not_resident: 8,
            miss_buckets: LightmapMissBuckets {
                read_in_flight: 2,
                held_by_drain_budget: 6,
                ..LightmapMissBuckets::default()
            },
            ..LightmapResidencyCounters::default()
        };
        tallies.assert_sums(&counters);

        let keys = tallies.json(None);
        assert_eq!(keys["lm_late_held_by_drain_budget"], json!(3));
        assert_eq!(keys["lm_late_held_by_drain_budget_spawn"], json!(3));
        assert_eq!(keys["lm_late_read_in_flight"], json!(2));
        assert_eq!(keys["lm_drawn_not_resident"], json!(5));
        assert_eq!(keys["lm_held_frame_uncounted"], json!(3));
        assert_eq!(keys["lm_held_frame_uncounted_spawn"], json!(0));
        assert_eq!(keys["sh_frames_held_by_drain_budget"], Value::Null);
    }

    #[test]
    #[should_panic(expected = "the frames read cover")]
    fn a_frame_the_walk_never_read_fails_the_sum() {
        let mut tallies = BudgetTallies::default();
        tallies.add(false, &frame(LightmapMissBuckets::default(), 0, 0));
        tallies.assert_sums(&LightmapResidencyCounters {
            drawn_not_resident: 1,
            ..LightmapResidencyCounters::default()
        });
    }

    // P13: SH's per-frame tally and the drains split by step kind; the
    // episode buckets subtract the spawn step's snapshot.
    #[test]
    fn sh_frames_episodes_and_drains_split_by_step_kind() {
        let held = ShMissBuckets {
            held_by_drain_budget: 1,
            ..ShMissBuckets::default()
        };
        let cut = DrainSample {
            sh_bytes: 9,
            lightmap_bytes: 1,
            items: 2,
            same_class_cut: true,
        };
        let outcome = FrameOutcome {
            sh_miss_frames: Some(held),
            drains: 2,
            drain_samples: [cut, DrainSample::default()],
            ..FrameOutcome::default()
        };
        let mut tallies = BudgetTallies::default();
        tallies.add(true, &outcome);
        for _ in 0..4 {
            tallies.add(false, &outcome);
        }
        let sh = ShRunStats {
            spawn_buckets: ShMissBuckets {
                read_in_flight: 2,
                ..ShMissBuckets::default()
            },
            counters: crate::sh_streaming::controller::ShResidencyCounters {
                miss_buckets: ShMissBuckets {
                    read_in_flight: 3,
                    held_by_drain_budget: 1,
                    ..ShMissBuckets::default()
                },
                ..Default::default()
            },
            ..ShRunStats::default()
        };
        let keys = tallies.json(Some(&sh));
        assert_eq!(keys["sh_frames_held_by_drain_budget"], json!(4));
        assert_eq!(keys["sh_frames_held_by_drain_budget_spawn"], json!(1));
        assert_eq!(keys["sh_episodes_read_in_flight"], json!(1));
        assert_eq!(keys["sh_episodes_read_in_flight_spawn"], json!(2));
        assert_eq!(keys["sh_episodes_held_by_drain_budget"], json!(1));
        assert_eq!(keys["same_class_cuts"], json!(4));
        assert_eq!(keys["same_class_cuts_spawn"], json!(1));
        assert_eq!(keys["drains"], json!(8));
        assert_eq!(keys["drains_admitting"], json!(4));
        assert_eq!(keys["drain_admitted_bytes_max"], json!(10));
    }
}
