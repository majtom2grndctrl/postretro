// The client assist step, its correction and follow tables, and the display
// rows, driven directly. Row ids name Task 2b's Orderings rows of
// context/plans/in-progress/E18--player-revive-assist (and the bar halves of
// Task 2a's rows).
use std::collections::HashMap;

use glam::Vec3;
use postretro_entities::{SlotTable, SlotValue};

use super::*;
use crate::sim::assist::{Row6Record, increment, reported};
use crate::sim::assist_pairs::PairValues;

const U: u64 = PROGRESS_UNIT;
/// A 100 ms hold.
const SIX: u32 = 6;
const NAME: &str = "player.assistProgress";

fn id(raw: u32) -> EntityId {
    EntityId::from_raw(raw)
}

const HELPER: u32 = 1;
const A: u32 = 2;
const B: u32 = 3;
const UNKNOWN: u32 = 4;

fn pair(required_ticks: u32) -> Option<PairValues> {
    Some(PairValues {
        radius: 1.5,
        required_ticks,
    })
}

fn downed(raw: u32, x: f32, pair: Option<PairValues>) -> AssistCandidate {
    AssistCandidate {
        id: id(raw),
        position: Vec3::new(x, 0.0, 0.0),
        life: PawnLife {
            life_state: 1,
            entry: None,
        },
        assistable: true,
        pair,
    }
}

/// A connected client's step for its own pawn, ticking from client tick 100
/// over a scripted view of the remote pawns.
struct Rig {
    assist: ClientAssist,
    tick: u32,
    candidates: Vec<AssistCandidate>,
}

impl Rig {
    fn new(candidates: Vec<AssistCandidate>) -> Self {
        Self {
            assist: ClientAssist::default(),
            tick: 100,
            candidates,
        }
    }

    /// One downed target `A` at 1 m with a pair of `n` required ticks.
    fn on_target(n: u32) -> Self {
        Self::new(vec![downed(A, 1.0, pair(n))])
    }

    fn helper() -> AssistHelper {
        AssistHelper {
            id: id(HELPER),
            position: Vec3::ZERO,
            permits_use: true,
            life: PawnLife {
                life_state: 0,
                entry: None,
            },
        }
    }

    /// One forward-predicted tick; returns its client tick.
    fn tick(&mut self, held: bool) -> u32 {
        let tick = self.tick;
        let candidates = if held {
            self.candidates.clone()
        } else {
            Vec::new()
        };
        self.assist
            .predict_tick(tick, held, Self::helper(), &candidates);
        self.tick += 1;
        tick
    }

    fn hold(&mut self, ticks: u32) -> u32 {
        let mut last = self.tick;
        for _ in 0..ticks {
            last = self.tick(true);
        }
        last
    }

    fn table(&self) -> Vec<AssistPairEntry> {
        self.candidates
            .iter()
            .filter_map(|candidate| {
                candidate.pair.map(|pair| AssistPairEntry {
                    target: candidate.id,
                    radius: pair.radius,
                    required_ticks: pair.required_ticks,
                })
            })
            .collect()
    }

    fn sample(&mut self, acked_tick: u32, progress: f32) {
        let table = self.table();
        self.assist.apply_samples(
            &[AssistProgressSample {
                acked_tick,
                progress,
            }],
            &table,
        );
    }

    /// The bar a frame at `alpha` shows over an authoritative slot of `slot`.
    fn shown(&self, alpha: f32, slot: f32) -> f32 {
        let mut values = HashMap::from([(NAME.to_string(), SlotValue::Number(slot))]);
        self.assist.client_projection(alpha).project(&mut values);
        match values[NAME] {
            SlotValue::Number(value) => value,
            _ => unreachable!(),
        }
    }

    fn state(&self) -> ClientDisplay {
        match self.assist.client_projection(0.0) {
            UiProjection::Client { state, .. } => state,
            UiProjection::Host { .. } => unreachable!(),
        }
    }
}

/// The host's reported progress after `ticks` advancing ticks of `n`.
fn host(n: u32, ticks: u64) -> f32 {
    reported(increment(n) * ticks)
}

/// AC9: the first held tick shows its own progress with no round trip, and
/// the bar advances every rendered frame at the table's rate.
#[test]
fn client_assist_ac9_first_held_tick_predicts_and_advances_every_frame() {
    let mut rig = Rig::on_target(180);
    rig.tick(true);
    assert_eq!(rig.shown(0.0, 0.0), host(180, 1));
    let mut previous = 0.0;
    for alpha in [0.0, 0.25, 0.5, 0.75] {
        let shown = rig.shown(alpha, 0.0);
        assert!(shown > previous || alpha == 0.0);
        assert!((shown - (host(180, 1) + alpha * host(180, 1))).abs() < 1e-6);
        previous = shown;
    }
    rig.tick(true);
    assert_eq!(rig.shown(0.0, 0.0), host(180, 2));
}

/// O13: on a lagged client, samples acknowledging ticks before the hold have
/// no record, so the bar keeps the prediction.
#[test]
fn client_assist_o13_samples_before_the_hold_find_no_record() {
    let mut rig = Rig::on_target(180);
    rig.tick(false);
    let before = rig.tick(false);
    let c0 = rig.hold(4) - 3;
    for ack in (before - 1)..c0 {
        rig.sample(ack, 0.0);
    }
    assert_eq!(rig.state(), ClientDisplay::Advanced);
    assert_eq!(rig.shown(0.0, 0.0), host(180, 4));
}

/// O14: a release then re-press within one round trip discards the old
/// hold's records; the 0s acknowledging ticks up to the re-press find none.
#[test]
fn client_assist_o14_release_then_repress_keeps_the_new_hold() {
    let mut rig = Rig::on_target(180);
    let first = rig.hold(5);
    let released = rig.tick(false);
    rig.tick(false);
    let c2 = rig.tick(true);
    rig.hold(2);
    for ack in (first - 4)..c2 {
        rig.sample(ack, 0.0);
    }
    assert!(released < c2);
    assert_eq!(rig.shown(0.0, 0.0), host(180, 3), "no drop, no rebase");
}

/// O6: the host's reset wins at the first sample acknowledging a tick after
/// it; the bar shows exactly 0 with no alpha term, through later held ticks
/// and later 0 samples, until a release or a sample above 0. Nothing here
/// plays a cue: success presentation lives in host fire lists.
#[test]
fn client_assist_o6_host_reset_drops_to_zero_and_follows_without_an_alpha_term() {
    let mut rig = Rig::on_target(SIX);
    let first = rig.hold(5) - 4;
    assert!(rig.assist.latched || rig.assist.progress > 0);
    rig.sample(first + 1, 0.0);
    assert_eq!(rig.state(), ClientDisplay::Following);
    for alpha in [0.0, 0.5, 0.99] {
        assert_eq!(rig.shown(alpha, 0.3), 0.0);
    }
    for _ in 0..10 {
        rig.tick(true);
        rig.sample(rig.tick - 1, 0.0);
        assert_eq!(rig.shown(0.9, 0.0), 0.0);
    }
    assert!(rig.assist.records.is_empty(), "following records nothing");
}

/// Follow row 2 and O12's bar half: a gap past the hold grace resets the
/// host, the drop row fires at the acknowledged tick, then the first host
/// sample above 0 rebases (counting from its acknowledged tick), ends follow
/// and clears the latch.
#[test]
fn client_assist_o12_gap_drop_then_first_sample_above_zero_rebases() {
    let mut rig = Rig::on_target(180);
    let start = rig.hold(10) - 9;
    rig.sample(start + 5, 0.0);
    assert_eq!(rig.state(), ClientDisplay::Following);
    let resumed = rig.hold(6) - 5;
    rig.sample(resumed + 1, host(180, 2));
    assert_ne!(rig.state(), ClientDisplay::Following);
    assert!(!rig.assist.latched);
    let expected =
        sample_units(host(180, 2)) + increment(180) * u64::from(rig.tick - 1 - (resumed + 1));
    assert_eq!(rig.assist.progress, expected);
    rig.tick(true);
    assert_eq!(rig.assist.progress, expected + increment(180));
}

/// Follow row 3: a release ends follow and clears the latch; the next press
/// predicts from 0 under the correction table.
#[test]
fn client_assist_follow_release_clears_follow_and_the_latch() {
    let mut rig = Rig::on_target(SIX);
    let first = rig.hold(3) - 2;
    rig.sample(first + 2, 0.0);
    assert_eq!(rig.state(), ClientDisplay::Following);
    rig.tick(false);
    assert!(!rig.assist.following && !rig.assist.latched);
    rig.tick(true);
    assert_eq!(rig.shown(0.0, 0.0), host(SIX, 1));
}

/// O16 and I2: the prediction never writes the slot store; the display map
/// alone carries it, so crossings and store reads see only host samples.
#[test]
fn client_assist_o16_prediction_reaches_only_the_display_map() {
    let mut rig = Rig::on_target(SIX);
    rig.hold(SIX);
    let mut slots = SlotTable::new();
    slots
        .get_mut(NAME)
        .unwrap()
        .write_value(Some(SlotValue::Number(0.25)));
    let values = crate::App::build_ui_display_slot_snapshot(
        &slots,
        None,
        None,
        Some(rig.assist.client_projection(0.5)),
    );
    assert_eq!(
        values[NAME],
        SlotValue::Number(1.0),
        "the prediction reached U"
    );
    assert_eq!(
        slots.get(NAME).unwrap().value,
        Some(SlotValue::Number(0.25)),
        "the store keeps the host sample"
    );
    // The host then resets (O6): the bar drops; the store still holds only
    // what the host sent.
    rig.sample(rig.tick - 1, 0.0);
    let values = crate::App::build_ui_display_slot_snapshot(
        &slots,
        None,
        None,
        Some(rig.assist.client_projection(0.5)),
    );
    assert_eq!(values[NAME], SlotValue::Number(0.0));
}

/// O15: a frame that runs no tick but applies a snapshot shows the prediction
/// plus render alpha, never the host's older store value.
#[test]
fn client_assist_o15_zero_tick_frame_with_a_snapshot_advances_by_alpha() {
    let mut rig = Rig::on_target(180);
    let last = rig.hold(8);
    let at_tick = rig.shown(0.0, 0.0);
    rig.sample(last - 3, host(180, 5));
    let store = host(180, 5);
    assert!(rig.shown(0.6, store) > at_tick);
    assert!((rig.shown(0.6, store) - (host(180, 8) + 0.6 * host(180, 1))).abs() < 1e-6);
}

/// O49: a zero-tick frame applying a table with a smaller increment never
/// steps the bar back: the alpha term keeps the last tick's increment.
#[test]
fn client_assist_o49_smaller_increment_table_on_a_zero_tick_frame_does_not_step_back() {
    let mut rig = Rig::on_target(60);
    let last = rig.hold(10);
    let before = rig.shown(0.8, 0.0);
    rig.candidates[0].pair = pair(3600);
    rig.sample(last - 2, host(60, 8));
    assert_eq!(rig.shown(0.8, 0.0), before);
}

/// O28 and O46: a confirmation drops the bar and latches the client, whether
/// its prediction reached U (O28) or not (O46); held on, it never predicts on
/// a second downed target until a release and re-press, and no second drop
/// follows.
#[test]
fn client_assist_o28_o46_confirmation_latches_until_a_release() {
    for ticks in [SIX, SIX - 2] {
        let mut rig = Rig::new(vec![downed(A, 1.0, pair(SIX)), downed(B, 1.2, pair(SIX))]);
        let last = rig.hold(ticks);
        assert_eq!(rig.assist.progress >= U, ticks == SIX);
        rig.sample(last, 0.0);
        assert_eq!(rig.state(), ClientDisplay::Following);
        // A is revived; B stays in range.
        rig.candidates.remove(0);
        for _ in 0..20 {
            rig.tick(true);
            assert_eq!(rig.shown(0.5, 0.0), 0.0);
        }
        rig.sample(rig.tick - 1, 0.0);
        assert_eq!(rig.shown(0.5, 0.0), 0.0, "no second drop, no rise");
        rig.tick(false);
        rig.tick(true);
        assert_eq!(rig.assist.target, Some(id(B)));
        assert_eq!(rig.shown(0.0, 0.0), host(SIX, 1));
    }
}

/// O30: a host that refuses the fresh hold keeps sampling its committed 0
/// (the channel omits the unchanged slot); the bar drops at the first sample
/// acknowledging a tick of the new hold whose record exceeds the tolerance.
#[test]
fn client_assist_o30_refusing_host_drops_at_the_first_record_above_tolerance() {
    let mut rig = Rig::on_target(180);
    rig.hold(4);
    let idle = rig.tick(false);
    rig.sample(idle, 0.0);
    let c0 = rig.hold(4) - 3;
    rig.sample(c0, 0.0);
    assert_eq!(
        rig.state(),
        ClientDisplay::Advanced,
        "one increment is within tolerance"
    );
    rig.sample(c0 + 1, 0.0);
    assert_eq!(rig.state(), ClientDisplay::Following);
    assert_eq!(rig.shown(0.5, 0.0), 0.0);
}

/// O31: `holdMs: 0` shows 1 from the first held tick, then 0 from the sample
/// acknowledging it until a release and re-press.
#[test]
fn client_assist_o31_zero_hold_shows_full_then_zero_from_the_confirming_sample() {
    let mut rig = Rig::on_target(1);
    let c0 = rig.tick(true);
    assert_eq!(rig.shown(0.7, 0.0), 1.0);
    rig.tick(true);
    rig.sample(c0, 0.0);
    assert_eq!(rig.shown(0.7, 0.0), 0.0);
    rig.hold(3);
    assert_eq!(rig.shown(0.7, 0.0), 0.0);
    rig.tick(false);
    rig.tick(true);
    assert_eq!(rig.shown(0.0, 0.0), 1.0);
}

/// O32: a host ahead by the interpolation lead rebases the prediction up to
/// the host value plus later increments, rewriting later records, so the bar
/// reaches 1 on the host's completion tick rather than dropping mid-bar.
#[test]
fn client_assist_o32_host_ahead_rebases_up_and_reaches_one_by_confirmation() {
    const LEAD: u64 = 3;
    let mut rig = Rig::on_target(SIX * 3);
    let start = rig.hold(6) - 5;
    let ack = start + 2;
    rig.sample(ack, host(SIX * 3, 3 + LEAD));
    let now = rig.tick - 1;
    assert_eq!(
        rig.assist.progress,
        sample_units(host(SIX * 3, 3 + LEAD)) + increment(SIX * 3) * u64::from(now - ack)
    );
    // A later sample at the rebased rate is within tolerance: no second step.
    let rebased = rig.assist.progress;
    rig.sample(ack + 1, host(SIX * 3, 4 + LEAD));
    assert_eq!(rig.assist.progress, rebased);
    // The host completes on its 18th advancing tick, LEAD ticks before the
    // client's own count: the client is at U by then.
    let host_completion = start + 17 - LEAD as u32;
    while rig.tick <= host_completion {
        rig.tick(true);
    }
    assert_eq!(rig.shown(0.0, 0.0), 1.0);
    rig.sample(host_completion, 0.0);
    assert_eq!(rig.shown(0.0, 0.0), 0.0, "a record held at U meets the 0");
}

/// Tolerance and rebase in U units: a sample within one recorded increment
/// keeps the prediction; one beyond rebases to exactly `round(s × U)` plus
/// the current increment per later tick.
#[test]
fn client_assist_tolerance_and_rebase_work_in_u_units() {
    let n = 180;
    let mut rig = Rig::on_target(n);
    let start = rig.hold(5) - 4;
    let record = increment(n) * 3;
    let within = (record + increment(n) - 4096) as f64 / U as f64;
    rig.sample(start + 2, within as f32);
    assert_eq!(rig.assist.progress, increment(n) * 5, "kept");
    // The record at start + 3 holds four increments; the host shows six.
    let beyond = (increment(n) * 6) as f64 / U as f64;
    rig.sample(start + 3, beyond as f32);
    assert_eq!(
        rig.assist.progress,
        sample_units(beyond as f32) + increment(n),
        "rebased onto the sample plus one current increment"
    );
}

/// O48: the required count drops from 180 to 1 before the host's completion
/// sample arrives in the same snapshot as the new table: records keep their
/// own increment, so the tolerance never widens and the bar drops, never
/// showing full.
#[test]
fn client_assist_o48_lowered_count_never_widens_the_tolerance() {
    let mut rig = Rig::on_target(180);
    let last = rig.hold(20);
    rig.candidates[0].pair = pair(1);
    rig.sample(last, 0.0);
    assert_eq!(rig.state(), ClientDisplay::Following);
    assert_eq!(rig.shown(0.9, 0.0), 0.0);
    rig.tick(true);
    assert_eq!(rig.shown(0.9, 0.0), 0.0);
}

/// O58: a stalled pair (`required_ticks` 0) records an increment of 0, so any
/// difference rebases onto the sample with nothing added; no drop and no
/// visible sawtooth.
#[test]
fn client_assist_o58_stalled_pair_rebases_onto_the_sample() {
    let mut rig = Rig::on_target(SIX);
    rig.hold(3);
    rig.candidates[0].pair = pair(0);
    let stalled = rig.hold(5) - 4;
    let held = rig.assist.progress;
    assert_eq!(held, increment(SIX) * 3);
    for (offset, nudge) in [(0u32, 1.0e-6f32), (1, -1.0e-6), (2, 0.0)] {
        let sample = reported(held) + nudge;
        rig.sample(stalled + offset, sample);
        assert_eq!(rig.assist.progress, sample_units(sample));
        assert!((rig.shown(0.9, 0.0) - sample).abs() < 1e-6);
        assert_ne!(rig.state(), ClientDisplay::Following);
    }
}

/// O57: a radius going from 0 to positive mid-hold: the entry was there, so
/// no abstention; prediction starts with the new table and rebases upward on
/// host samples; the bar never drops.
#[test]
fn client_assist_o57_radius_zero_to_positive_predicts_without_abstaining() {
    let mut rig = Rig::new(vec![downed(
        A,
        1.0,
        Some(PairValues {
            radius: 0.0,
            required_ticks: 180,
        }),
    )]);
    rig.hold(3);
    assert_eq!(rig.state(), ClientDisplay::Held);
    assert_eq!(rig.shown(0.5, 0.0), 0.0);
    rig.candidates[0].pair = pair(180);
    let mut shown = Vec::new();
    let first = rig.hold(4) - 3;
    shown.push(rig.shown(0.0, 0.0));
    rig.sample(first + 1, host(180, 4));
    shown.push(rig.shown(0.0, 0.0));
    rig.hold(2);
    shown.push(rig.shown(0.0, 0.0));
    assert!(shown.windows(2).all(|pair| pair[1] >= pair[0]), "{shown:?}");
    assert!(shown[1] > host(180, 4));
}

/// O45's bar half: a radius-0 entry is a known answer: no prediction and no
/// abstention.
#[test]
fn client_assist_o45_radius_zero_entry_neither_predicts_nor_abstains() {
    let mut rig = Rig::new(vec![downed(
        A,
        1.0,
        Some(PairValues {
            radius: 0.0,
            required_ticks: SIX,
        }),
    )]);
    rig.hold(10);
    assert_eq!(rig.state(), ClientDisplay::Held);
    assert_eq!(rig.assist.progress, 0);
    assert_eq!(
        rig.shown(0.5, 0.4),
        0.0,
        "shows the prediction, not host samples"
    );
}

/// O43 (a): before the target's `assistable` bool arrives the step finds no
/// target and shows 0; with the bool and entry it predicts from 0, and host
/// samples ahead rebase it upward.
#[test]
fn client_assist_o43a_unknown_bool_shows_zero_then_predicts_and_rebases_up() {
    let mut candidate = downed(A, 1.0, None);
    candidate.assistable = false;
    let mut rig = Rig::new(vec![candidate]);
    rig.hold(3);
    assert_eq!(rig.state(), ClientDisplay::Held);
    assert_eq!(rig.shown(0.0, 0.2), 0.0);
    rig.candidates[0] = downed(A, 1.0, pair(180));
    let first = rig.hold(3) - 2;
    assert_eq!(rig.shown(0.0, 0.0), host(180, 3));
    rig.sample(first, host(180, 5));
    assert!(rig.shown(0.0, 0.0) > host(180, 5));
}

/// O43 (b): with the bool applied and no entry, the step abstains and the
/// bar shows host samples; when the entry arrives it resumes from the newest
/// host sample, counting from its acknowledged tick, never below a value it
/// showed.
#[test]
fn client_assist_o43b_abstains_then_resumes_from_the_newest_host_sample() {
    let mut rig = Rig::new(vec![downed(A, 1.0, None)]);
    let press = rig.hold(6) - 5;
    assert_eq!(rig.state(), ClientDisplay::Abstaining);
    assert!(rig.assist.records.is_empty(), "abstention records nothing");
    rig.sample(press + 1, host(180, 2));
    assert_eq!(
        rig.shown(0.5, host(180, 2)),
        host(180, 2),
        "the authoritative slot"
    );
    rig.candidates[0].pair = pair(180);
    let resumed = rig.tick(true);
    assert_ne!(rig.state(), ClientDisplay::Abstaining);
    assert_eq!(
        rig.assist.progress,
        sample_units(host(180, 2)) + increment(180) * u64::from(resumed - (press + 1))
    );
    assert!(rig.shown(0.0, 0.0) >= host(180, 2));
}

/// Abstention edges: an unknown assistable pawn beyond the cap, or farther
/// than the chosen target, causes none; one as near as the target does; a
/// latched step never abstains.
#[test]
fn client_assist_abstention_edges() {
    let target = downed(A, 1.0, pair(180));
    let far = Rig::new(vec![
        target,
        downed(UNKNOWN, ASSIST_MAX_RADIUS_M + 0.5, None),
    ]);
    let farther = Rig::new(vec![target, downed(UNKNOWN, 1.4, None)]);
    let level = Rig::new(vec![target, downed(UNKNOWN, -1.0, None)]);
    let alone = Rig::new(vec![downed(UNKNOWN, ASSIST_MAX_RADIUS_M, None)]);
    for (mut rig, abstains) in [(far, false), (farther, false), (level, true), (alone, true)] {
        rig.tick(true);
        assert_eq!(rig.state() == ClientDisplay::Abstaining, abstains);
    }
    let mut latched = Rig::new(vec![target]);
    let last = latched.hold(3);
    latched.sample(last, 0.0);
    latched.candidates.push(downed(UNKNOWN, 0.5, None));
    latched.tick(true);
    assert_eq!(latched.state(), ClientDisplay::Following);
}

/// O47: a client-side target switch while following never restarts the
/// prediction: the bar stays 0 until a release or a host sample above 0.
#[test]
fn client_assist_o47_target_switch_while_following_stays_zero() {
    let mut rig = Rig::new(vec![downed(A, 1.0, pair(SIX)), downed(B, 1.4, pair(SIX))]);
    let last = rig.hold(3);
    rig.sample(last, 0.0);
    rig.candidates[1].position = Vec3::new(0.5, 0.0, 0.0);
    for _ in 0..8 {
        rig.tick(true);
        assert_eq!(rig.shown(0.5, 0.0), 0.0);
    }
    assert_eq!(
        rig.assist.target,
        Some(id(A)),
        "keeps the target held at the drop"
    );
}

/// O35: a hold across a level change starts the new level at 0, even under a
/// recurring entity id.
#[test]
fn client_assist_o35_reset_starts_the_next_level_at_zero() {
    let mut rig = Rig::on_target(10);
    rig.hold(8);
    assert!(rig.shown(0.0, 0.0) >= 0.8);
    rig.assist.reset();
    assert_eq!(rig.shown(0.5, 0.8), 0.0);
    rig.tick(true);
    assert_eq!(rig.shown(0.0, 0.8), host(10, 1));
}

/// O36: from a release the bar shows 0; rising samples of the released hold
/// reach only the store.
#[test]
fn client_assist_o36_release_shows_zero_while_samples_rise() {
    let mut rig = Rig::on_target(180);
    let start = rig.hold(10) - 9;
    rig.tick(false);
    for k in 0..5 {
        rig.sample(start + k, host(180, u64::from(k) + 1));
        assert_eq!(rig.shown(0.5, host(180, u64::from(k) + 1)), 0.0);
    }
}

/// O19's bar half: a target disconnect resets the host; the helper's bar
/// drops through the host sample.
#[test]
fn client_assist_o19_target_disconnect_drops_through_the_host_sample() {
    let mut rig = Rig::on_target(180);
    let last = rig.hold(10);
    rig.sample(last - 2, 0.0);
    assert_eq!(rig.shown(0.5, 0.0), 0.0);
    rig.candidates.clear();
    rig.tick(true);
    assert_eq!(rig.shown(0.5, 0.0), 0.0);
}

/// O22's bar half: a reload removing `assistable` turns the retained bool
/// false and drops the entry; the prediction stops.
#[test]
fn client_assist_o22_reload_removing_assistable_stops_the_prediction() {
    let mut rig = Rig::on_target(180);
    rig.hold(10);
    rig.candidates[0].assistable = false;
    rig.candidates[0].pair = None;
    rig.tick(true);
    assert_eq!(rig.assist.progress, 0);
    assert_eq!(
        rig.state(),
        ClientDisplay::Held,
        "no abstention on a known false bool"
    );
    assert_eq!(rig.shown(0.5, 0.3), 0.0);
}

/// O42's bar half: a reloaded expression's new `required_ticks` changes the
/// predicted rate, and a divergence beyond the tolerance rebases at the
/// current increment.
#[test]
fn client_assist_o42_new_required_ticks_change_the_rate_and_rebase() {
    let mut rig = Rig::on_target(180);
    let start = rig.hold(10) - 9;
    rig.candidates[0].pair = pair(60);
    rig.tick(true);
    assert_eq!(rig.assist.progress, increment(180) * 10 + increment(60));
    rig.hold(4);
    // The host applied the new value two ticks earlier than the client.
    let ack = start + 9;
    let host_value = reported(increment(180) * 8 + increment(60) * 2);
    rig.sample(ack, host_value);
    assert_eq!(
        rig.assist.progress,
        sample_units(host_value) + increment(60) * u64::from(rig.tick - 1 - ack)
    );
}

/// The listen-host and single-player rows: the authoritative slot, plus the
/// alpha term of the row-6 record's increment when the last tick ran row 6
/// without completing, capped at 1.
#[test]
fn client_assist_listen_host_and_single_player_display_rows() {
    let project = |row6: Row6Record, alpha: f32, slot: f32| {
        let mut values = HashMap::from([(NAME.to_string(), SlotValue::Number(slot))]);
        UiProjection::Host { row6, alpha }.project(&mut values);
        match values[NAME] {
            SlotValue::Number(value) => value,
            _ => unreachable!(),
        }
    };
    let advancing = Row6Record {
        advanced_without_completing: true,
        required_ticks: 180,
    };
    assert!((project(advancing, 0.5, 0.25) - (0.25 + 0.5 * host(180, 1))).abs() < 1e-6);
    assert_eq!(project(advancing, 0.5, 0.999_99), 1.0);
    let idle = Row6Record {
        advanced_without_completing: false,
        required_ticks: 180,
    };
    assert_eq!(project(idle, 0.5, 0.25), 0.25);
    assert_eq!(project(Row6Record::default(), 0.9, 0.0), 0.0);
}

/// Every applied local-pawn record (each delta and held-baseline resend)
/// reports its pawn as armed. Consecutive records for the same pawn keep the
/// hold: its prediction, records and latch; only a different pawn resets.
// Regression: every snapshot's re-arm reset the prediction about 30 times a
// second.
#[test]
fn client_assist_rearming_the_same_pawn_keeps_the_hold_and_a_new_pawn_resets() {
    let pawn = id(HELPER);
    let mut rig = Rig::on_target(SIX);
    rig.assist.absorb_receive(Some(pawn), &[], Vec::new);
    let last = rig.hold(SIX);
    assert!(rig.assist.latched, "a completed prediction latches");
    let records = rig.assist.records.clone();
    assert!(!records.is_empty());
    let table = rig.table();
    for ack in [last - 3, last - 2] {
        rig.assist.absorb_receive(
            Some(pawn),
            &[AssistProgressSample {
                acked_tick: ack,
                progress: host(SIX, u64::from(ack - (last - 5)) + 1),
            }],
            || table.clone(),
        );
        rig.assist
            .absorb_receive(Some(pawn), &[], || unreachable!());
    }
    assert_eq!(rig.assist.progress, U);
    assert!(rig.assist.latched);
    assert_eq!(rig.shown(0.0, 0.0), 1.0);
    assert_eq!(
        rig.assist.records.back(),
        records.back(),
        "the newest record survives"
    );

    let other = EntityId::from_raw(HELPER + 100);
    rig.assist.absorb_receive(Some(other), &[], Vec::new);
    assert_eq!(rig.assist.progress, 0);
    assert!(!rig.assist.latched);
    assert!(rig.assist.records.is_empty());
}

/// Track every bar value a frame shows, asserting it never drops below the
/// highest host sample it already showed.
struct Shown {
    host_floor: f32,
    values: Vec<f32>,
}

impl Shown {
    fn new() -> Self {
        Self {
            host_floor: 0.0,
            values: Vec::new(),
        }
    }

    fn frame(&mut self, rig: &Rig) {
        let shown = rig.shown(0.0, 0.0);
        assert!(
            shown + 1e-6 >= self.host_floor,
            "the bar dropped below a host value it showed: {shown} < {} ({:?})",
            self.host_floor,
            self.values
        );
        self.values.push(shown);
    }

    fn showed_host(&mut self, rig: &Rig, sample: f32) {
        let shown = rig.shown(0.0, 0.0);
        if shown + 1e-6 >= sample {
            self.host_floor = self.host_floor.max(sample);
        }
        self.frame(rig);
    }
}

/// Correction row 5 then the step's acquisition (O32, O43): samples above 0
/// acknowledging ticks on which the step held no target rebase onto the host
/// value; the ticks still without a target hold it, and the acquisition
/// continues from it rather than resetting to 0. The bar never drops below a
/// host value it showed.
// Regression: the acquisition reset discarded the rebase and dropped the bar.
#[test]
fn client_assist_rebase_on_no_target_ticks_survives_the_acquisition() {
    let mut candidate = downed(A, 1.0, pair(180));
    candidate.assistable = false;
    let mut rig = Rig::new(vec![candidate]);
    let mut shown = Shown::new();
    // Each no-target tick is a step reset, so only the newest one keeps a
    // record for a sample to find.
    let mut ack = rig.hold(4);
    shown.frame(&rig);
    for k in 2..5 {
        let sample = host(180, k);
        rig.sample(ack, sample);
        shown.showed_host(&rig, sample);
        ack = rig.tick(true);
        shown.frame(&rig);
    }
    assert_eq!(shown.host_floor, host(180, 4));
    // The retained bool arrives: the step acquires A and continues from the
    // newest host value plus A's increment per tick since it.
    rig.candidates[0].assistable = true;
    let acquired = rig.tick(true);
    shown.frame(&rig);
    assert_eq!(
        rig.assist.progress,
        sample_units(host(180, 4)) + increment(180) * u64::from(acquired - (ack - 1))
    );
    for _ in 0..4 {
        rig.tick(true);
        shown.frame(&rig);
    }
    assert!(
        shown.values.windows(2).all(|pair| pair[1] >= pair[0]),
        "{:?}",
        shown.values
    );
    // A host 0 acknowledging a carried no-target tick still drops the bar.
    let mut dropped = Rig::new(vec![{
        let mut unknown = downed(A, 1.0, pair(180));
        unknown.assistable = false;
        unknown
    }]);
    let newest = dropped.hold(4);
    dropped.sample(newest, host(180, 3));
    let carried = dropped.tick(true);
    dropped.sample(carried, 0.0);
    assert_eq!(dropped.state(), ClientDisplay::Following);
}

/// Follow row 2 then a target switch (O47, O43): the host sample above 0
/// ends follow onto a value the step continues from when its interpolated
/// view then chooses a different target, so the switch reset never discards
/// it and the bar never drops below the host value.
// Regression: the switch reset dropped the follow exit's rebase to 0.
#[test]
fn client_assist_follow_exit_onto_a_different_target_keeps_the_host_value() {
    let mut rig = Rig::new(vec![downed(A, 1.0, pair(180)), downed(B, 1.4, pair(60))]);
    let last = rig.hold(5);
    rig.sample(last, 0.0);
    assert_eq!(rig.state(), ClientDisplay::Following);
    // The host moves to B, which the client's view will choose next.
    rig.candidates[1].position = Vec3::new(0.5, 0.0, 0.0);
    let resumed = rig.hold(3) - 2;
    assert_eq!(rig.assist.target, Some(id(A)), "following keeps A");
    let mut shown = Shown::new();
    let sample = host(60, 2);
    rig.sample(resumed + 1, sample);
    assert_ne!(rig.state(), ClientDisplay::Following);
    shown.showed_host(&rig, sample);
    let before = rig.assist.progress;
    let switched = rig.tick(true);
    shown.frame(&rig);
    assert_eq!(rig.assist.target, Some(id(B)));
    assert_eq!(rig.assist.progress, before + increment(60));
    assert!(switched > resumed);
    for _ in 0..4 {
        rig.tick(true);
        shown.frame(&rig);
    }
    assert!(
        shown.values.windows(2).all(|pair| pair[1] >= pair[0]),
        "{:?}",
        shown.values
    );
}

/// A helper whose state forbids `use` chooses no target, so an unknown
/// assistable pawn beside it causes no abstention.
#[test]
fn client_assist_helper_without_use_never_abstains() {
    let mut rig = Rig::new(vec![downed(UNKNOWN, 0.5, None)]);
    let mut helper = Rig::helper();
    helper.permits_use = false;
    rig.assist
        .predict_tick(rig.tick, true, helper, &rig.candidates.clone());
    assert_ne!(rig.state(), ClientDisplay::Abstaining);
    assert_eq!(rig.shown(0.5, 0.4), 0.0, "the prediction, not host samples");
    rig.tick(true);
    assert_eq!(rig.state(), ClientDisplay::Abstaining, "with `use` it does");
}

/// The client step runs after this tick's movement prediction inside the
/// `predict` closure, so the helper stands where the host's pass, which runs
/// after movement, sees it.
#[test]
fn client_assist_step_runs_after_movement_prediction() {
    let frame_loop = include_str!("frame_loop/mod.rs");
    let movement = frame_loop
        .find("app.client_predict_movement_tick(&command, tick_dt)")
        .expect("the predict closure runs movement prediction");
    let assist = frame_loop
        .find("app.predict_client_assist(&command, input_tick)")
        .expect("the predict closure runs the assist step");
    assert!(movement < assist, "assist must follow movement prediction");
}
