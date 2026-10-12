// Replay pose memo: resolves every historical mover's pose once per replayed
// tick, stepping a reusable working phase forward instead of re-extrapolating
// from the seed sample on every collision query.
// See: context/lib/networking.md · context/lib/entity_model.md §7
//
// Reconcile replays ticks past the newest snapshot sample, so each lookup used
// to clone the seed phase and step it to the target tick: once per mover, per
// cast, per replayed tick. Resolution here is eager per replayed tick and
// matches `MoverHistoryBuffer::pose_at_tick` exactly — same seed choice, same
// deterministic step sequence — so poses stay bit-identical.

use postretro_entities::{KinematicMoverComponent, Transform};

use super::{MoverHistoryBuffer, tick_le, ticks_between};
use crate::collision::moving::{MoverPose, MoverTranslationLeg};
use crate::kinematic_mover::advance_mover_phase_one_tick_reusing_legs;

/// Scratch owned by [`MoverHistoryBuffer`]. Entries persist across reconciles so
/// their phase buffers are reused. A working phase continues only while the
/// history generation that seeded it is current; any `record` or `clear` forces
/// a reseed.
#[derive(Debug, Default)]
pub(crate) struct ReplayPoseMemo {
    /// Sorted by `mover_id`. Grows only when a new mover id appears in history.
    entries: Vec<ReplayPoseEntry>,
    translation_legs: Vec<MoverTranslationLeg>,
}

#[derive(Debug)]
struct ReplayPoseEntry {
    mover_id: u32,
    /// Pose for the tick most recently passed to `resolve_replay_tick`.
    resolved: Option<MoverPose>,
    /// Working phase stepped from a seed sample; `None` until first seeded.
    working: Option<WorkingPhase>,
}

#[derive(Debug)]
struct WorkingPhase {
    /// History generation and deque index of the seed sample.
    generation: u64,
    seed: usize,
    tick_dt_bits: u32,
    at_tick: u32,
    phase: KinematicMoverComponent,
    transform: Transform,
    pose: MoverPose,
}

impl MoverHistoryBuffer {
    /// Resolve every historical mover's pose at `server_tick`. Replay calls this
    /// once per replayed tick, in increasing tick order, before its queries.
    pub(crate) fn resolve_replay_tick(&mut self, server_tick: u32, tick_dt: f32) {
        let generation = self.generation;
        let memo = &mut self.replay;
        for entry in &mut memo.entries {
            entry.resolved = None;
        }
        for (&mover_id, samples) in &self.samples {
            let index = match memo
                .entries
                .binary_search_by_key(&mover_id, |entry| entry.mover_id)
            {
                Ok(index) => index,
                Err(index) => {
                    memo.entries.insert(
                        index,
                        ReplayPoseEntry {
                            mover_id,
                            resolved: None,
                            working: None,
                        },
                    );
                    index
                }
            };
            let entry = &mut memo.entries[index];

            // One reverse pass finds both candidates `pose_at_tick` would pick:
            // the newest-recorded exact sample, else the newest-recorded sample
            // at or before the tick, else the oldest sample.
            let mut exact = None;
            let mut seed = None;
            for (index, sample) in samples.iter().enumerate().rev() {
                if sample.server_tick == server_tick {
                    exact = Some(index);
                    break;
                }
                if seed.is_none() && tick_le(sample.server_tick, server_tick) {
                    seed = Some(index);
                }
            }
            if let Some(index) = exact {
                entry.resolved = Some(samples[index].pose);
                continue;
            }
            let Some(seed_index) = seed.or((!samples.is_empty()).then_some(0)) else {
                continue;
            };
            let seed_sample = &samples[seed_index];

            let tick_dt_bits = tick_dt.to_bits();
            let continues = entry.working.as_ref().is_some_and(|working| {
                working.generation == generation
                    && working.seed == seed_index
                    && working.tick_dt_bits == tick_dt_bits
                    && tick_le(working.at_tick, server_tick)
            });
            if !continues {
                match &mut entry.working {
                    Some(working) => {
                        working.phase.clone_from(&seed_sample.phase);
                        working.generation = generation;
                        working.seed = seed_index;
                        working.tick_dt_bits = tick_dt_bits;
                        working.at_tick = seed_sample.server_tick;
                        working.transform = seed_sample.pose.transform;
                        working.pose = seed_sample.pose;
                    }
                    None => {
                        entry.working = Some(WorkingPhase {
                            generation,
                            seed: seed_index,
                            tick_dt_bits,
                            at_tick: seed_sample.server_tick,
                            phase: seed_sample.phase.clone(),
                            transform: seed_sample.pose.transform,
                            pose: seed_sample.pose,
                        });
                    }
                }
            }
            let Some(working) = &mut entry.working else {
                continue;
            };
            let steps = ticks_between(working.at_tick, server_tick);
            for _ in 0..steps {
                working.pose = advance_mover_phase_one_tick_reusing_legs(
                    &mut working.phase,
                    &mut working.transform,
                    tick_dt,
                    &mut memo.translation_legs,
                );
            }
            if steps > 0 {
                working.at_tick = server_tick;
            }
            entry.resolved = Some(working.pose);
        }
    }

    /// The pose `resolve_replay_tick` produced for `mover_id`, or `None` when the
    /// mover has no history (callers fall back to the live table).
    pub(crate) fn replay_pose(&self, mover_id: u32) -> Option<MoverPose> {
        let entries = &self.replay.entries;
        entries
            .binary_search_by_key(&mover_id, |entry| entry.mover_id)
            .ok()
            .and_then(|index| entries[index].resolved)
    }

    #[cfg(test)]
    pub(crate) fn replay_scratch_fingerprint(&self) -> Vec<(u32, usize, usize, usize)> {
        let mut fingerprint: Vec<_> = self
            .replay
            .entries
            .iter()
            .filter_map(|entry| {
                let working = entry.working.as_ref()?;
                Some((
                    entry.mover_id,
                    working.phase.waypoints.as_ptr() as usize,
                    working.phase.waypoint_names.as_ptr() as usize,
                    working
                        .phase
                        .waypoint_names
                        .first()
                        .map_or(0, |name| name.as_ptr() as usize),
                ))
            })
            .collect();
        fingerprint.push((
            u32::MAX,
            self.replay.entries.as_ptr() as usize,
            self.replay.entries.capacity(),
            self.replay.translation_legs.capacity(),
        ));
        fingerprint
    }
}

#[cfg(test)]
#[path = "replay_poses_tests.rs"]
mod tests;
