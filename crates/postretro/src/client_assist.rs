// Connected-client assist presentation: the helper's predicted progress, its
// records and their correction toward host samples at the acknowledged input
// tick, and the `player.assistProgress` display projection on every role.
// See: context/lib/networking.md §What gates, and what replicates instead · context/lib/ui.md §3
//
// The prediction never writes the slot store: host samples land there as they
// arrive, so crossings, conditions and store reads see only the host. The
// prediction reaches the HUD only through the UI display map (`display`).
// Success presentation lives in the host-run `assistReceived` /
// `assistGiven` fire lists; nothing here plays a cue.

mod display;
mod glue;

#[cfg(test)]
pub(crate) use display::ClientDisplay;
pub(crate) use display::UiProjection;
pub(crate) use glue::ui_projection;

use std::collections::VecDeque;

use postretro_entities::EntityId;
use postretro_foundation::ASSIST_MAX_RADIUS_M;

use crate::netcode::{AssistPairEntry, AssistProgressSample};
use crate::sim::assist::{
    self, AssistCandidate, AssistHelper, AssistRow, AssistRunner, AssistStepInput, HelperProgress,
    PROGRESS_UNIT, PawnLife,
};

/// About two seconds of forward-predicted ticks. A sample acknowledging an
/// older tick finds no record and keeps the prediction.
const RECORD_CAPACITY: usize = 128;

/// One forward-predicted held tick's prediction, and the increment that
/// tick's chosen pair gave (0 with none chosen or a stalled pair).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Record {
    tick: u32,
    progress: u64,
    increment: u64,
}

/// The owning client's assist step state for its own pawn as helper.
#[derive(Debug, Default)]
pub(crate) struct ClientAssist {
    progress: u64,
    /// The last chosen target, kept across latched ticks so a latch never
    /// reads as a target switch.
    target: Option<EntityId>,
    latched: bool,
    /// Entered only by the correction table's drop row; left by a release or
    /// a host sample above 0.
    following: bool,
    /// The last forward-predicted tick abstained: an assistable pawn with no
    /// pair-table entry was at least as near as any target.
    abstaining: bool,
    /// `assist` was held on the last forward-predicted tick.
    held: bool,
    /// The last forward-predicted tick ran row 6 without reaching U.
    advanced: bool,
    /// The increment that tick advanced by; 0 unless `advanced`.
    last_increment: u64,
    last_tick: Option<u32>,
    /// The tick of the step's last reset.
    last_reset: Option<u32>,
    /// The tick `assist` was last pressed.
    press_tick: Option<u32>,
    /// Each pawn's life state at the last step.
    lives: Vec<(EntityId, PawnLife)>,
    records: VecDeque<Record>,
    /// The newest host sample, for an abstention's resume.
    newest_sample: Option<AssistProgressSample>,
    /// Per-tick candidate buffers; kept across resets.
    scratch: glue::Scratch,
}

/// `a` is strictly after `b` in serial client-tick order.
fn tick_after(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

/// Ticks from `from` (exclusive) through `to` (inclusive); 0 when `to` is not
/// after `from`.
fn ticks_between(from: u32, to: u32) -> u64 {
    if tick_after(to, from) {
        u64::from(to.wrapping_sub(from))
    } else {
        0
    }
}

/// A sample in U units: `round(s × U)`, clamped to `[0, U]`.
fn sample_units(progress: f32) -> u64 {
    (f64::from(progress).clamp(0.0, 1.0) * PROGRESS_UNIT as f64).round() as u64
}

/// Whether an assistable pawn with no pair-table entry lies within the cap and
/// no farther than the target the step would otherwise choose.
fn abstains(helper: &AssistHelper, candidates: &[AssistCandidate]) -> bool {
    let chosen = assist::assist_candidate(helper, candidates)
        .map(|target| helper.position.distance(target.position));
    candidates.iter().any(|candidate| {
        if candidate.id == helper.id || !candidate.assistable || candidate.pair.is_some() {
            return false;
        }
        let distance = helper.position.distance(candidate.position);
        distance <= ASSIST_MAX_RADIUS_M && chosen.is_none_or(|nearest| distance <= nearest)
    })
}

impl ClientAssist {
    /// Clear every hold: level unload, demotion, and a newly armed local pawn.
    /// No hold carries onto a new level or pawn under a recurring id (O35).
    pub(crate) fn reset(&mut self) {
        let mut lives = std::mem::take(&mut self.lives);
        let mut records = std::mem::take(&mut self.records);
        let scratch = std::mem::take(&mut self.scratch);
        lives.clear();
        records.clear();
        *self = Self {
            lives,
            records,
            scratch,
            ..Self::default()
        };
    }

    /// One forward-predicted tick, never a reconcile replay: run the client
    /// step for the local pawn as helper over `candidates` (the remote pawns,
    /// with `pair` from the host's pair table, `None` where no entry has
    /// arrived), unless the step abstains.
    pub(crate) fn predict_tick(
        &mut self,
        tick: u32,
        held: bool,
        helper: AssistHelper,
        candidates: &[AssistCandidate],
    ) {
        if held && !self.held {
            self.press_tick = Some(tick);
        }
        self.held = held;
        self.last_tick = Some(tick);
        self.advanced = false;
        self.last_increment = 0;
        if held && !self.latched && abstains(&helper, candidates) {
            self.abstaining = true;
            return;
        }
        let resuming = std::mem::take(&mut self.abstaining) && held;
        let outcome = assist::step(&AssistStepInput {
            runner: AssistRunner::Client,
            helper,
            held,
            real_command: true,
            candidates,
            previous: HelperProgress {
                progress: self.progress,
                target: self.target,
                latched: self.latched,
            },
            previous_lives: &self.lives,
        });
        self.lives.clear();
        self.lives.push((helper.id, helper.life));
        self.lives.extend(
            candidates
                .iter()
                .map(|candidate| (candidate.id, candidate.life)),
        );
        if outcome.step_reset {
            self.records.clear();
            self.following = false;
            self.last_reset = Some(tick);
        }
        self.progress = outcome.progress;
        self.latched = outcome.latched;
        if outcome.row != AssistRow::Latched {
            self.target = outcome.target;
        }
        let increment = if outcome.target.is_some() {
            assist::increment(outcome.required_ticks)
        } else {
            0
        };
        if resuming && outcome.row == AssistRow::Advance {
            self.resume_from_newest_sample(tick, increment);
        }
        self.advanced = outcome.row == AssistRow::Advance && self.progress < PROGRESS_UNIT;
        if self.advanced {
            self.last_increment = increment;
        }
        if held && !self.following {
            if self.records.len() == RECORD_CAPACITY {
                self.records.pop_front();
            }
            self.records.push_back(Record {
                tick,
                progress: self.progress,
                increment,
            });
        }
    }

    /// The pair table arrived for the pawn the step abstained on: rebase from
    /// the newest host sample of this hold, counting from its acknowledged
    /// tick, so the bar never drops below the host value it showed.
    fn resume_from_newest_sample(&mut self, tick: u32, increment: u64) {
        let Some(sample) = self.newest_sample.filter(|sample| {
            sample.progress > 0.0
                && self
                    .press_tick
                    .is_none_or(|press| !tick_after(press, sample.acked_tick))
        }) else {
            return;
        };
        let advancing = ticks_between(sample.acked_tick, tick);
        self.progress = sample_units(sample.progress)
            .saturating_add(increment.saturating_mul(advancing))
            .min(PROGRESS_UNIT);
        self.latched = self.progress >= PROGRESS_UNIT;
    }

    /// The increment of the current target's pair in the current table: the
    /// rate the host runs at after an acknowledged tick.
    fn current_increment(&self, pair_table: &[AssistPairEntry]) -> u64 {
        self.target
            .and_then(|target| pair_table.iter().find(|entry| entry.target == target))
            .filter(|entry| entry.radius > 0.0)
            .map_or(0, |entry| assist::increment(entry.required_ticks))
    }

    /// Consume this frame's host samples in receive order, after the receive
    /// step: the follow table while following, the correction table otherwise.
    pub(crate) fn apply_samples(
        &mut self,
        samples: &[AssistProgressSample],
        pair_table: &[AssistPairEntry],
    ) {
        for sample in samples {
            self.newest_sample = Some(*sample);
            let current = self.current_increment(pair_table);
            if self.following {
                self.follow(sample, current);
            } else {
                self.correct(sample, current);
            }
        }
    }

    /// Follow rows 1 and 2; row 3 (a release) is the step's own reset.
    fn follow(&mut self, sample: &AssistProgressSample, current: u64) {
        if sample.progress <= 0.0 {
            return;
        }
        let from = match self.last_reset {
            Some(reset) if tick_after(reset, sample.acked_tick) => reset,
            _ => sample.acked_tick,
        };
        let advancing = self.last_tick.map_or(0, |last| ticks_between(from, last));
        self.progress = sample_units(sample.progress)
            .saturating_add(current.saturating_mul(advancing))
            .min(PROGRESS_UNIT);
        self.following = false;
        self.latched = false;
    }

    /// The correction table, rows 2–5, against the record at the sample's
    /// acknowledged tick.
    fn correct(&mut self, sample: &AssistProgressSample, current: u64) {
        let Some(index) = self
            .records
            .iter()
            .position(|record| record.tick == sample.acked_tick)
        else {
            // Row 2: before the step's last reset, an idle tick, or pruned.
            return;
        };
        let record = self.records[index];
        let tolerance = record.increment.min(PROGRESS_UNIT / 2);
        let units = sample_units(sample.progress);
        if sample.progress <= 0.0 && record.progress > tolerance {
            // Row 3: a host reset or a confirmed completion.
            self.progress = 0;
            self.latched = true;
            self.following = true;
            self.records.clear();
            return;
        }
        self.records.drain(..index);
        if units.abs_diff(record.progress) <= tolerance {
            // Row 4.
            return;
        }
        // Row 5: rebase, rewriting every later record. Each held tick after
        // the acknowledged one ran on the host at the current value; the
        // records all postdate the step's last reset, which discarded older
        // ones.
        let mut progress = units;
        for record in self.records.iter_mut() {
            progress = units
                .saturating_add(
                    current.saturating_mul(ticks_between(sample.acked_tick, record.tick)),
                )
                .min(PROGRESS_UNIT);
            record.progress = progress;
        }
        self.progress = progress;
        self.latched = progress >= PROGRESS_UNIT;
    }
}

#[cfg(test)]
#[path = "client_assist_tests.rs"]
mod tests;
