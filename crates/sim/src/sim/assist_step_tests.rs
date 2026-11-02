//! The accumulator's pure step and target choice, driven directly: every
//! step-table row under both runners, the reset before the table, and the
//! fixed-point completion tick.
//! See: context/lib/scripting.md §12 (Assist)

use super::*;

const DOWN: u8 = 1;
const ALIVE: u8 = 0;

fn id(index: u32) -> EntityId {
    // Fresh registries hand out ids in spawn order; the step only compares them.
    let mut registry = postretro_entities::EntityRegistry::new();
    let mut last = None;
    for _ in 0..=index {
        last = Some(registry.spawn(postretro_entities::Transform::default()));
    }
    last.unwrap()
}

fn life(life_state: u8, elapsed: u32) -> PawnLife {
    PawnLife {
        life_state,
        elapsed: Some(elapsed),
    }
}

fn pair(required_ticks: u32) -> PairValues {
    PairValues {
        radius: 1.5,
        required_ticks,
    }
}

struct Rig {
    runner: AssistRunner,
    helper: AssistHelper,
    target: AssistCandidate,
    previous: HelperProgress,
    lives: Vec<(EntityId, PawnLife)>,
}

impl Rig {
    fn new(runner: AssistRunner, required_ticks: u32) -> Self {
        Self {
            runner,
            helper: AssistHelper {
                id: id(0),
                position: Vec3::ZERO,
                permits_use: true,
                life: life(ALIVE, 100),
            },
            target: AssistCandidate {
                id: id(1),
                position: Vec3::new(1.0, 0.0, 0.0),
                life: life(DOWN, 100),
                assistable: true,
                pair: Some(pair(required_ticks)),
            },
            previous: HelperProgress::default(),
            lives: Vec::new(),
        }
    }

    /// One pass: elapsed advances one tick for both pawns, as the host's does
    /// between passes, then the step runs and its result is kept.
    fn tick(&mut self, held: bool, real_command: bool) -> AssistStep {
        for pawn_life in [&mut self.helper.life, &mut self.target.life] {
            if !self.lives.is_empty() {
                pawn_life.elapsed = pawn_life.elapsed.map(|elapsed| elapsed + 1);
            }
        }
        self.run(held, real_command)
    }

    /// One pass with no elapsed advance (the caller set the lives).
    fn run(&mut self, held: bool, real_command: bool) -> AssistStep {
        let candidates = [self.target];
        let result = step(&AssistStepInput {
            runner: self.runner,
            helper: self.helper,
            held,
            real_command,
            candidates: &candidates,
            previous: self.previous,
            previous_lives: &self.lives,
        });
        self.previous = HelperProgress {
            progress: result.progress,
            target: result.target,
            latched: result.latched,
        };
        self.lives = vec![
            (self.helper.id, self.helper.life),
            (self.target.id, self.target.life),
        ];
        result
    }

    /// Hold on real commands until a completion; the advancing tick it lands
    /// on, or `None` within `limit` ticks.
    fn completion_tick(&mut self, limit: u32) -> Option<u32> {
        (1..=limit).find(|_| self.tick(true, true).completed)
    }
}

const RUNNERS: [AssistRunner; 2] = [AssistRunner::Host, AssistRunner::Client];

#[test]
fn assist_step_row_1_release_resets_progress_and_clears_the_latch() {
    for runner in RUNNERS {
        let mut rig = Rig::new(runner, 10);
        rig.tick(true, true);
        rig.previous.latched = true;
        let result = rig.tick(false, true);
        assert_eq!(result.row, AssistRow::Release);
        assert_eq!((result.progress, result.latched), (0, false));
        assert!(result.step_reset && !result.completed);
        assert_eq!((result.target, result.required_ticks), (None, 0));
    }
}

/// Row 2 (host half of O12, O29): a non-real command not holding `assist`
/// resets progress as a release does but keeps the latch.
#[test]
fn assist_step_row_2_o12_o29_neutral_resets_progress_and_keeps_the_latch() {
    for runner in RUNNERS {
        for latched in [false, true] {
            let mut rig = Rig::new(runner, 10);
            rig.tick(true, true);
            rig.previous.latched = latched;
            let result = rig.tick(false, false);
            assert_eq!(result.row, AssistRow::Neutral);
            assert_eq!((result.progress, result.latched), (0, latched));
            assert!(result.step_reset);
        }
    }
}

/// Row 3: held while latched never advances. The host's stays at the 0 its
/// completion left; the client holds its value.
#[test]
fn assist_step_row_3_latched_hold_does_not_advance_under_either_runner() {
    for (runner, expected) in [
        (AssistRunner::Host, 0),
        (AssistRunner::Client, PROGRESS_UNIT),
    ] {
        let mut rig = Rig::new(runner, 1);
        let first = rig.tick(true, true);
        assert!(first.completed && first.latched);
        assert_eq!(first.progress, expected);
        for _ in 0..5 {
            let result = rig.tick(true, true);
            assert_eq!(result.row, AssistRow::Latched);
            assert_eq!(result.progress, expected);
            assert!(result.latched && !result.completed && !result.step_reset);
            assert_eq!(result.target, None);
        }
    }
}

#[test]
fn assist_step_row_4_no_target_resets_progress() {
    for runner in RUNNERS {
        let mut rig = Rig::new(runner, 10);
        rig.tick(true, true);
        rig.target.position = Vec3::new(5.0, 0.0, 0.0);
        let result = rig.tick(true, true);
        assert_eq!(result.row, AssistRow::NoTarget);
        assert_eq!((result.progress, result.target), (0, None));
        assert!(result.step_reset && !result.latched);
    }
}

/// Row 5 (O11): a gap-held tick keeps progress without advancing it, so a
/// late release one held tick before the required count never completes.
#[test]
fn assist_step_row_5_o11_gap_held_keeps_progress_and_a_late_release_completes_nothing() {
    for runner in RUNNERS {
        let mut rig = Rig::new(runner, 4);
        for _ in 0..3 {
            rig.tick(true, true);
        }
        let before = rig.previous.progress;
        // The host frontier-freezes on the previous held command.
        for _ in 0..3 {
            let result = rig.tick(true, false);
            assert_eq!(result.row, AssistRow::GapHeld);
            assert_eq!(result.progress, before);
            assert_eq!(result.target, Some(rig.target.id));
            assert!(!result.completed && !result.step_reset);
        }
        // Then it consumes the release.
        let release = rig.tick(false, true);
        assert_eq!(release.row, AssistRow::Release);
        assert!(!release.completed);
    }
}

#[test]
fn assist_step_row_6_advances_and_completes_per_runner() {
    for (runner, at_completion) in [
        (AssistRunner::Host, 0),
        (AssistRunner::Client, PROGRESS_UNIT),
    ] {
        let mut rig = Rig::new(runner, 3);
        let first = rig.tick(true, true);
        assert_eq!(first.row, AssistRow::Advance);
        assert_eq!(first.progress, increment(3));
        assert_eq!(first.required_ticks, 3);
        assert!(first.step_reset, "a new target resets before the table");
        let second = rig.tick(true, true);
        assert!(!second.step_reset);
        assert_eq!(second.progress, 2 * increment(3));
        let third = rig.tick(true, true);
        assert_eq!(third.row, AssistRow::Advance);
        assert!(third.completed && third.latched);
        assert_eq!(third.progress, at_completion);
        assert!(!third.step_reset, "the completion reset is not the step's");
    }
}

/// O44: a pair whose `required_ticks` is 0 keeps progress without advancing,
/// and advancing resumes from it when the value recovers.
#[test]
fn assist_step_o44_zero_required_ticks_keeps_progress_until_it_recovers() {
    let mut rig = Rig::new(AssistRunner::Host, 10);
    rig.tick(true, true);
    rig.tick(true, true);
    let kept = rig.previous.progress;
    rig.target.pair = Some(pair(0));
    for _ in 0..4 {
        let result = rig.tick(true, true);
        assert_eq!(result.row, AssistRow::Advance);
        assert_eq!(result.progress, kept);
        assert!(!result.completed);
    }
    rig.target.pair = Some(pair(10));
    assert_eq!(rig.tick(true, true).progress, kept + increment(10));
}

/// O39: a changed required count keeps progress and applies the new increment
/// from that pass; the hold completes on the first advancing tick whose
/// progress reaches one whole.
#[test]
fn assist_step_o39_changed_required_count_keeps_progress_and_applies_the_new_increment() {
    let mut rig = Rig::new(AssistRunner::Host, 10);
    for _ in 0..5 {
        rig.tick(true, true);
    }
    let kept = rig.previous.progress;
    assert_eq!(kept, 5 * increment(10));
    rig.target.pair = Some(pair(4));
    let result = rig.tick(true, true);
    assert_eq!(result.progress, kept + increment(4));
    // 5/10 + 2/4 reaches one whole on the second tick at the new rate.
    assert!(rig.tick(true, true).completed);
}

#[test]
fn assist_step_resets_before_the_table_on_target_switch_and_life_state_change() {
    for runner in RUNNERS {
        // Target switch: a nearer assistable pawn takes over.
        let mut rig = Rig::new(runner, 10);
        rig.tick(true, true);
        rig.tick(true, true);
        rig.target.id = id(2);
        let switched = rig.tick(true, true);
        assert!(switched.step_reset);
        assert_eq!(switched.progress, increment(10));

        // Target re-enters its state (elapsed reset, O9/O25 shape).
        let mut rig = Rig::new(runner, 10);
        rig.tick(true, true);
        rig.tick(true, true);
        rig.target.life.elapsed = Some(0);
        let reentered = rig.run(true, true);
        assert!(reentered.step_reset);
        assert_eq!(reentered.progress, increment(10));

        // Helper changes state (same target).
        let mut rig = Rig::new(runner, 10);
        rig.tick(true, true);
        rig.tick(true, true);
        rig.helper.life = life(2, 0);
        let changed = rig.run(true, true);
        assert!(changed.step_reset);
        assert_eq!(changed.progress, increment(10));

        // Gap-held after a reset keeps the reset 0.
        let mut rig = Rig::new(runner, 10);
        rig.tick(true, true);
        rig.target.life.elapsed = Some(0);
        let gap = rig.run(true, false);
        assert_eq!((gap.row, gap.progress), (AssistRow::GapHeld, 0));
        assert!(gap.step_reset);
    }
}

/// O25: a helper whose descriptor declares no states (undeclared id, elapsed
/// held at zero) keeps its progress across a respawn; one in a declared state
/// restarts from 0.
#[test]
fn assist_step_o25_undeclared_helper_keeps_progress_across_respawn_declared_one_restarts() {
    let mut rig = Rig::new(AssistRunner::Host, 10);
    rig.helper.life = life(postretro_foundation::UNDECLARED_LIFE_STATE, 0);
    rig.tick(true, true);
    rig.helper.life.elapsed = Some(0);
    rig.target.life.elapsed = rig.target.life.elapsed.map(|e| e + 1);
    let kept = rig.run(true, true);
    assert!(!kept.step_reset);
    assert_eq!(kept.progress, 2 * increment(10));

    let mut rig = Rig::new(AssistRunner::Host, 10);
    rig.tick(true, true);
    rig.helper.life.elapsed = Some(0);
    rig.target.life.elapsed = rig.target.life.elapsed.map(|e| e + 1);
    let restarted = rig.run(true, true);
    assert!(restarted.step_reset);
    assert_eq!(restarted.progress, increment(10));
}

/// An absent elapsed compares ids alone: a stalled elapsed is no reset, a
/// changed id still is (the client runner retains no remote elapsed).
#[test]
fn assist_step_absent_elapsed_compares_life_state_ids_alone() {
    for runner in RUNNERS {
        let mut rig = Rig::new(runner, 10);
        rig.target.life.elapsed = None;
        rig.tick(true, true);
        // The helper's elapsed advances; the target's stays absent.
        let kept = rig.tick(true, true);
        assert!(!kept.step_reset);
        assert_eq!(kept.progress, 2 * increment(10));
        rig.target.life.life_state = 2;
        let changed = rig.tick(true, true);
        assert!(changed.step_reset);
    }
    let stored = PawnLife {
        life_state: DOWN,
        elapsed: Some(50),
    };
    assert!(!life(DOWN, 10).changed_since(Some(PawnLife {
        elapsed: None,
        ..stored
    })));
    assert!(life(DOWN, 10).changed_since(Some(stored)));
    assert!(
        life(DOWN, 10).changed_since(None),
        "a new pawn counts as changed"
    );
}

/// I6 and AC5: the required count for `holdMs` 0, 3000, 60000 and a value
/// that is not a whole number of ticks completes on exactly its n-th
/// advancing tick, and a release one advancing tick before it is refused.
#[test]
fn assist_step_i6_completes_on_exactly_the_nth_advancing_tick_and_an_early_release_refuses() {
    for (hold_ms, required) in [(0.0, 1), (3000.0, 180), (60000.0, 3600), (100.0, 6)] {
        assert_eq!(required_count(hold_ms), required, "holdMs {hold_ms}");
        for runner in RUNNERS {
            let mut rig = Rig::new(runner, required);
            assert_eq!(rig.completion_tick(required + 5), Some(required));

            if required > 1 {
                let mut early = Rig::new(runner, required);
                for _ in 0..required - 1 {
                    assert!(!early.tick(true, true).completed);
                }
                assert!(!early.tick(false, true).completed);
                // The release restarts the hold from 0.
                assert_eq!(early.completion_tick(required + 5), Some(required));
            }
        }
    }
}

/// O4: `holdMs: 0` completes on the first held tick, once; holding on
/// completes nothing more until a release and a fresh hold, even though the
/// target stays assistable (O10's shape).
#[test]
fn assist_step_o4_zero_hold_completes_once_until_released_and_held_anew() {
    for runner in RUNNERS {
        let mut rig = Rig::new(runner, required_count(0.0));
        assert!(rig.tick(true, true).completed);
        for _ in 0..10 {
            assert!(!rig.tick(true, true).completed);
        }
        // A gap neutral does not end the ended hold (O29).
        rig.tick(false, false);
        assert!(!rig.tick(true, true).completed);
        rig.tick(false, true);
        assert!(rig.tick(true, true).completed);
    }
}

fn candidate(index: u32, x: f32, radius: f32) -> AssistCandidate {
    AssistCandidate {
        id: id(index),
        position: Vec3::new(x, 0.0, 0.0),
        life: life(DOWN, 0),
        assistable: true,
        pair: Some(PairValues {
            radius,
            required_ticks: 10,
        }),
    }
}

fn helper() -> AssistHelper {
    AssistHelper {
        id: id(0),
        position: Vec3::ZERO,
        permits_use: true,
        life: life(ALIVE, 0),
    }
}

#[test]
fn assist_candidate_picks_the_nearest_in_range_and_ties_go_to_players_order() {
    let near = candidate(2, 1.0, 1.5);
    let far = candidate(1, 1.4, 1.5);
    assert_eq!(
        assist_candidate(&helper(), &[far, near]).map(|c| c.id),
        Some(near.id)
    );
    let tie_a = candidate(1, 1.0, 1.5);
    let tie_b = AssistCandidate {
        position: Vec3::new(-1.0, 0.0, 0.0),
        ..candidate(2, 0.0, 1.5)
    };
    assert_eq!(
        assist_candidate(&helper(), &[tie_a, tie_b]).map(|c| c.id),
        Some(tie_a.id)
    );
    assert_eq!(
        assist_candidate(&helper(), &[tie_b, tie_a]).map(|c| c.id),
        Some(tie_b.id)
    );
}

/// AC5's refuse edges at the candidate: out of range, not assistable, radius
/// at or below 0, no pair values, `use` forbidden, and the helper itself.
#[test]
fn assist_candidate_refuses_every_ac5_edge_and_never_selects_the_helper() {
    let in_range = candidate(1, 1.0, 1.5);
    assert!(assist_candidate(&helper(), &[in_range]).is_some());

    let out_of_range = candidate(1, 1.6, 1.5);
    let not_assistable = AssistCandidate {
        assistable: false,
        ..in_range
    };
    let radius_zero = candidate(1, 0.0, 0.0);
    let no_pair = AssistCandidate {
        pair: None,
        ..in_range
    };
    for refused in [out_of_range, not_assistable, radius_zero, no_pair] {
        assert!(
            assist_candidate(&helper(), &[refused]).is_none(),
            "{refused:?}"
        );
    }
    let no_use = AssistHelper {
        permits_use: false,
        ..helper()
    };
    assert!(assist_candidate(&no_use, &[in_range]).is_none());

    // A helper whose state permits `use` and declares `assistable` is listed
    // among the candidates at distance 0; it never selects itself.
    let own = AssistCandidate {
        id: helper().id,
        ..candidate(0, 0.0, 1.5)
    };
    assert!(assist_candidate(&helper(), &[own]).is_none());
    assert_eq!(
        assist_candidate(&helper(), &[own, in_range]).map(|c| c.id),
        Some(in_range.id)
    );
}

#[test]
fn assist_reported_progress_is_progress_over_one_whole_capped_at_one() {
    assert_eq!(reported(0), 0.0);
    assert_eq!(reported(PROGRESS_UNIT / 2), 0.5);
    assert_eq!(reported(PROGRESS_UNIT), 1.0);
    assert_eq!(reported(PROGRESS_UNIT * 2), 1.0);
}
