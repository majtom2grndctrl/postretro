// The received-assist stream's rows, and the committed samples the receive
// path feeds it and the helper's correction.
use super::super::super::{ClientStateApply, ReplicatedSlotIdentity, ReplicatedSlotSchema};
use super::*;
use postretro_entities::SlotTable;
use postretro_net::state_slots::{
    RawStateSlotRecord, STATE_RECORD_KIND_DELTA, STATE_RECORD_KIND_FULL_BASELINE, WireSlotValue,
};

/// A 3 s hold: 180 required ticks.
const SLOW: u32 = 180;
/// A 1 s hold: 60 required ticks.
const FAST: u32 = 60;
/// A 240 Hz render frame, in server ticks.
const FRAME: f64 = 0.25;

fn slope(required_ticks: u32) -> f64 {
    increment(required_ticks) as f64 / PROGRESS_UNIT as f64
}

/// The host's progress `ticks` advancing ticks into a hold of `n`, as the
/// slot reports it.
fn progress(n: u32, ticks: u32) -> f32 {
    postretro_sim::sim::assist::reported(increment(n) * u64::from(ticks))
}

/// Advance frame by frame from `from` to `to` (exclusive), asserting AC9's
/// bound between frames, and accepting `samples` (tick, value, n) when the
/// timeline reaches each tick. Returns the displayed value per frame.
fn run(
    stream: &mut ReceivedProgressStream,
    from: f64,
    to: f64,
    samples: &[(u32, f32, u32)],
) -> Vec<f64> {
    let mut shown = Vec::new();
    let mut timeline = from;
    let mut pending = samples.iter().peekable();
    while timeline < to {
        let before = stream.displayed;
        let mut stepped_by_sample = None;
        while let Some(&&(tick, value, n)) = pending.peek() {
            if f64::from(tick) > timeline {
                break;
            }
            let previous = stream.anchor.map(|anchor| anchor.value);
            stream.accept(tick, value, n);
            stepped_by_sample = Some((previous, f64::from(value)));
            pending.next();
        }
        stream.advance(Some(timeline));
        let after = stream.displayed.unwrap_or(0.0);
        if let Some(before) = before {
            let below_predecessor = stepped_by_sample
                .is_some_and(|(previous, value)| previous.is_some_and(|p| value < p));
            let onset = stepped_by_sample
                .is_some_and(|(previous, value)| previous.is_some_and(|p| p <= 0.0) && value > 0.0);
            if !below_predecessor {
                assert!(
                    after >= before - 1e-12,
                    "the display never steps down without a lower sample: {before} -> {after} at {timeline}"
                );
            }
            if !below_predecessor && !onset {
                assert!(
                    after - before <= 2.0 * stream.slope * FRAME + 1e-9,
                    "AC9: at most twice the slope over the advance: {before} -> {after} at {timeline}"
                );
            }
        }
        shown.push(after);
        timeline += FRAME;
    }
    shown
}

/// Samples of one helper holding from `start` on a hold of `n`, one per
/// snapshot interval from `first` to `last`.
fn hold(n: u32, start: u32, first: u32, last: u32) -> Vec<(u32, f32, u32)> {
    (first..=last)
        .step_by(SNAPSHOT_TICK_INTERVAL as usize)
        .map(|tick| (tick, progress(n, tick.saturating_sub(start)), n))
        .collect()
}

/// O17, AC9: the first nonzero sample may step up from 0; from it the display
/// rises continuously at the received slope, with no step at any later sample.
#[test]
fn received_stream_o17_onset_steps_once_then_rises_continuously() {
    let mut stream = ReceivedProgressStream::default();
    let samples = hold(SLOW, 11, 0, 60);
    let shown = run(&mut stream, 0.0, 61.0, &samples);
    // Before onset the display is flat at 0 and never extrapolates.
    assert!(shown[..40].iter().all(|value| *value == 0.0));
    let onset = shown.iter().position(|value| *value > 0.0).unwrap();
    assert!(
        (shown[onset] - f64::from(progress(SLOW, 1))).abs() < 1e-6,
        "the first nonzero sample shows at once"
    );
    // The bound assertions inside `run` cover every later frame; the display
    // ends close to the host line.
    let last = *shown.last().unwrap();
    assert!((last - f64::from(progress(SLOW, 49))).abs() < 3.0 * slope(SLOW));
}

/// O18: the highest helper releases; the bar drops at once to the lower value,
/// then rises at the lower helper's rate, from the required ticks the same
/// snapshot carries.
#[test]
fn received_stream_o18_step_down_shows_at_once_then_rises_at_the_new_rate() {
    let mut stream = ReceivedProgressStream::default();
    let mut samples = hold(FAST, 0, 0, 20);
    let lower = hold(SLOW, 0, 22, 40);
    samples.extend(lower.iter().copied());
    let shown = run(&mut stream, 0.0, 41.0, &samples);
    let drop_frame = (22.0 / FRAME) as usize;
    assert!(
        (shown[drop_frame] - f64::from(progress(SLOW, 22))).abs() < 1e-6,
        "drops to the lower helper's value at once"
    );
    assert!(shown[drop_frame] < shown[drop_frame - 1]);
    assert_eq!(stream.slope, slope(SLOW), "the lower helper's slope");
    let rise = shown[drop_frame + 8] - shown[drop_frame];
    assert!(rise > 0.0 && rise <= 2.0 * slope(SLOW) * 8.0 * FRAME + 1e-9);
}

/// Received-stream row 5 and O56: a sample at or above its predecessor whose
/// line is below the display (the host kept a gap-held tick, or the slope
/// fell) holds the display until the line overtakes it, never stepping down.
#[test]
fn received_stream_o56_row5_line_below_the_display_holds_without_stepping_down() {
    let mut stream = ReceivedProgressStream::default();
    // FAST until tick 20, then the host's received required ticks fall to
    // SLOW (a slower pair is now highest at the same progress): the stream
    // extrapolated at the fast slope and now holds until the slow line passes.
    let mut samples = hold(FAST, 0, 0, 20);
    let at = progress(FAST, 20);
    samples.extend((22..=40).step_by(2).map(|tick| {
        let since = tick - 20;
        let value = at + progress(SLOW, since);
        (tick, value, SLOW)
    }));
    let shown = run(&mut stream, 0.0, 41.0, &samples);
    let held_from = (22.0 / FRAME) as usize;
    assert!(
        shown[held_from] > f64::from(samples.iter().find(|s| s.0 == 22).unwrap().1),
        "the display is above the new line when the slope falls"
    );
    assert_eq!(stream.slope, slope(SLOW), "follows the new slope after it");
}

/// Received-stream rows 4 and 6 and O40's bar half: when the received
/// required ticks change between samples, the stream switches slope at the
/// sample that carries the new value.
#[test]
fn received_stream_o40_switches_slope_when_received_required_ticks_change() {
    let mut stream = ReceivedProgressStream::default();
    stream.accept(0, 0.0, SLOW);
    stream.accept(2, progress(SLOW, 2), SLOW);
    stream.advance(Some(2.0));
    assert_eq!(stream.slope, slope(SLOW));
    stream.accept(4, progress(SLOW, 4), FAST);
    assert_eq!(stream.slope, slope(FAST));
    stream.advance(Some(4.0));
    let before = stream.displayed.unwrap();
    stream.advance(Some(5.0));
    let after = stream.displayed.unwrap();
    assert!(
        after > before && after - before <= 2.0 * slope(FAST) + 1e-12,
        "row 6 rises toward the fast line at most twice the fast slope"
    );
    assert!(
        after - before > 2.0 * slope(SLOW),
        "faster than the old slope"
    );
}

/// Received-stream row 1: under received required ticks of 0 or 1 nothing
/// extrapolates; the first sample and a sample below its predecessor show at
/// once.
#[test]
fn received_stream_row1_zero_or_one_required_ticks_shows_samples_raw() {
    for n in [0, 1] {
        let mut stream = ReceivedProgressStream::default();
        stream.advance(Some(0.0));
        stream.accept(2, 0.4, n);
        stream.advance(Some(10.0));
        assert_eq!(stream.displayed, Some(f64::from(0.4_f32)));
        stream.accept(4, 0.2, n);
        assert_eq!(stream.displayed, Some(f64::from(0.2_f32)));
    }
}

/// Received-stream row 1 after extrapolation: received required ticks fall
/// to 0 (a stalled pair) or 1 mid-assist while the display stands above the
/// newest sample. A sample at or above its predecessor never steps the
/// display down; a higher sample lifts it at once, since no slope paces the
/// rise; only a sample below its predecessor lowers it.
// Regression: row 1 showed every sample raw, stepping the bar down below the
// extrapolated value without a lower sample.
#[test]
fn received_stream_row1_switch_from_extrapolating_to_raw_never_steps_down() {
    for n in [0, 1] {
        let mut stream = ReceivedProgressStream::default();
        let samples = hold(SLOW, 0, 0, 20);
        run(&mut stream, 0.0, 22.0, &samples);
        let extrapolated = stream.displayed.unwrap();
        let newest = f64::from(progress(SLOW, 20));
        assert!(extrapolated > newest, "the line ran ahead of the samples");

        // The same value again under the stalled count: hold, never extrapolate.
        stream.accept(22, progress(SLOW, 20), n);
        assert_eq!(stream.displayed, Some(extrapolated));
        for frame in 0..20 {
            stream.advance(Some(22.0 + f64::from(frame) * FRAME));
            assert_eq!(stream.displayed, Some(extrapolated));
        }
        // A sample above the predecessor but below the display still holds.
        let between = ((newest + extrapolated) / 2.0) as f32;
        stream.accept(24, between, n);
        assert_eq!(stream.displayed, Some(extrapolated));
        // A sample above the display shows at once.
        stream.accept(26, 0.9, n);
        assert_eq!(stream.displayed, Some(f64::from(0.9_f32)));
        // Only a sample below its predecessor lowers it.
        stream.accept(28, 0.5, n);
        assert_eq!(stream.displayed, Some(f64::from(0.5_f32)));
        // Back on a real slope, the stream rises from there under the
        // ordinary rows.
        stream.accept(30, 0.5, SLOW);
        stream.advance(Some(30.0));
        stream.advance(Some(31.0));
        let risen = stream.displayed.unwrap();
        assert!(
            risen >= f64::from(0.5_f32) && risen <= f64::from(0.5_f32) + 2.0 * slope(SLOW) + 1e-9
        );
    }
}

/// O50's bar half: a target revived in the pass reads 0 with received
/// required ticks 0 at that snapshot; the bar shows 0 from it and never
/// extrapolates at the losing helper's slope.
#[test]
fn received_stream_o50_revived_target_shows_zero_and_never_extrapolates() {
    let mut stream = ReceivedProgressStream::default();
    let samples = hold(SLOW, 0, 0, 30);
    run(&mut stream, 0.0, 31.0, &samples);
    assert!(stream.displayed.unwrap() > 0.1);
    stream.accept(32, 0.0, 0);
    assert_eq!(stream.displayed, Some(0.0));
    for frame in 0..40 {
        stream.advance(Some(32.0 + f64::from(frame) * FRAME));
        assert_eq!(stream.displayed, Some(0.0));
    }
    // And a 0 under a nonzero count is a flat line at 0 too.
    stream.accept(34, 0.0, SLOW);
    stream.advance(Some(50.0));
    assert_eq!(stream.displayed, Some(0.0));
}

/// The line extrapolates at most two snapshot intervals past its anchor, and
/// with no timeline the display holds.
#[test]
fn received_stream_caps_extrapolation_and_holds_without_a_timeline() {
    let mut stream = ReceivedProgressStream::default();
    stream.advance(Some(0.0));
    stream.accept(0, 0.5, SLOW);
    for frame in 1..200 {
        stream.advance(Some(f64::from(frame) * FRAME));
    }
    let capped = 0.5_f32 as f64 + slope(SLOW) * LEAD_CAP_TICKS;
    assert!((stream.displayed.unwrap() - capped).abs() < 1e-9);
    stream.advance(None);
    assert!((stream.displayed.unwrap() - capped).abs() < 1e-9);
}

fn records(
    table: &SlotTable,
    tick: u32,
    values: &[(&str, f32)],
) -> ([u8; 32], Vec<RawStateSlotRecord>) {
    let schema = ReplicatedSlotSchema::build(table, &ReplicatedSlotIdentity::default());
    (
        *schema.fingerprint(),
        values
            .iter()
            .map(|(name, value)| RawStateSlotRecord {
                slot_id: schema.id_for(name).unwrap().0,
                kind: STATE_RECORD_KIND_FULL_BASELINE,
                has_baseline_ref: false,
                baseline_ref: 0,
                baseline_id: tick + 1,
                value: WireSlotValue::Number(*value),
            })
            .collect(),
    )
}

/// The helper's samples: an applied batch samples the committed value, an
/// applied batch that omits the unchanged slot still samples it (O30), and a
/// rejected batch or a slot held for a baseline refresh yields none.
#[test]
fn assist_progress_sample_o30_reads_committed_values_under_the_timer_guards() {
    const NAME: &str = "player.assistProgress";
    let mut client = ClientStateApply::new();
    let mut table = SlotTable::new();
    let identity = ReplicatedSlotIdentity::default();
    let (fingerprint, first) = records(&table, 1, &[(NAME, 0.25)]);
    let applied = client.apply_snapshot_state(&mut table, &identity, 1, &fingerprint, &first);
    assert_eq!(client.assist_progress_sample(&table, &applied), Some(0.25));

    let omitted = client.apply_snapshot_state(&mut table, &identity, 2, &fingerprint, &[]);
    assert!(omitted.state_batch_applied);
    assert_eq!(
        client.assist_progress_sample(&table, &omitted),
        Some(0.25),
        "a batch with no record for the slot samples the held value"
    );

    let rejected = client.apply_snapshot_state(&mut table, &identity, 3, &[0; 32], &first);
    assert!(!rejected.state_batch_applied);
    assert_eq!(client.assist_progress_sample(&table, &rejected), None);

    let mut stale = first.clone();
    stale[0].kind = STATE_RECORD_KIND_DELTA;
    stale[0].has_baseline_ref = true;
    stale[0].baseline_ref = 0;
    stale[0].baseline_id = 1;
    let held = client.apply_snapshot_state(&mut table, &identity, 4, &fingerprint, &stale);
    assert!(held.state_batch_applied);
    assert_eq!(held.refresh_requests.len(), 1);
    assert_eq!(client.assist_progress_sample(&table, &held), None);
}

/// The receive path feeds the received stream the committed
/// `player.assistReceivedProgress` with the snapshot's received required
/// ticks, and the UI display map takes the stream's value without the store
/// changing.
#[test]
fn accept_timed_slot_sample_feeds_the_received_stream_with_its_required_ticks() {
    let mut client = ClientStateApply::new();
    let mut table = SlotTable::new();
    let identity = ReplicatedSlotIdentity::default();
    client.advance_timed_slot_targets(Some(0.0), 0.0);
    let (fingerprint, batch) = records(&table, 0, &[(RECEIVED_NAME, 0.5)]);
    let outcome = client.apply_snapshot_state(&mut table, &identity, 0, &fingerprint, &batch);
    client.accept_timed_slot_sample(0, &table, &outcome, SLOW);
    assert_eq!(client.received_assist_displayed(), Some(0.5));
    client.advance_timed_slot_targets(Some(1.0), 1.0 / 60.0);
    let mut values =
        std::collections::HashMap::from([(RECEIVED_NAME.to_string(), SlotValue::Number(0.5))]);
    client.apply_timed_slot_targets(&mut values);
    let SlotValue::Number(shown) = values[RECEIVED_NAME] else {
        unreachable!()
    };
    assert!(shown > 0.5 && f64::from(shown) <= 0.5 + 2.0 * slope(SLOW) + 1e-6);
    assert_eq!(
        table.get(RECEIVED_NAME).unwrap().value,
        Some(SlotValue::Number(0.5)),
        "the store keeps the host sample"
    );
}
