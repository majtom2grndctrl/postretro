use super::super::{ClientStateApply, ReplicatedSlotIdentity, ReplicatedSlotSchema};
use super::*;
use postretro_entities::SlotTable;
use postretro_net::state_slots::{
    RawStateSlotRecord, STATE_RECORD_KIND_DELTA, STATE_RECORD_KIND_FULL_BASELINE, WireSlotValue,
};

fn records(
    table: &SlotTable,
    tick: u32,
    values: &[(&str, WireSlotValue)],
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
                value: value.clone(),
            })
            .collect(),
    )
}

fn apply(
    client: &mut ClientStateApply,
    table: &mut SlotTable,
    tick: u32,
    values: &[(&str, WireSlotValue)],
) {
    let (fingerprint, records) = records(table, tick, values);
    let outcome = client.apply_snapshot_state(
        table,
        &ReplicatedSlotIdentity::default(),
        tick,
        &fingerprint,
        &records,
    );
    assert!(outcome.state_batch_applied);
    client.accept_timed_slot_sample(tick, table, &outcome);
}

fn number(slot: usize, value: f32) -> WireSlotValue {
    WireSlotValue::Array(vec![slot as f32, value])
}
fn target(client: &ClientStateApply, table: &SlotTable, name: &str) -> Option<f32> {
    let mut values = table
        .iter()
        .filter_map(|(name, record)| record.value.clone().map(|v| (name.to_string(), v)))
        .collect();
    client.apply_timed_slot_targets(&mut values);
    match values.get(name) {
        Some(SlotValue::Number(value)) => Some(*value),
        _ => None,
    }
}
fn close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
}
fn ramp(client: &mut ClientStateApply, table: &mut SlotTable) {
    client.advance_timed_slot_targets(Some(0.0), 0.0);
    for tick in [0, 2, 4] {
        let t = tick as f32;
        apply(
            client,
            table,
            tick,
            &[
                (NAMES[0], number(0, t * 0.025)),
                (NAMES[1], number(0, 100.0 - t * 2.0)),
                (NAMES[2], number(0, 80.0 - t)),
                (NAMES[3], number(0, 10.0 + t)),
                ("player.cellCapacity", number(0, 100.0)),
            ],
        );
        client.advance_timed_slot_targets(
            Some(f64::from(tick)),
            if tick == 0 { 0.0 } else { 2.0 / 60.0 },
        );
    }
}

#[test]
fn accepted_host_samples_project_four_timers_without_mutating_raw_slots() {
    let mut client = ClientStateApply::new();
    let mut table = SlotTable::new();
    ramp(&mut client, &mut table);
    let raw: Vec<_> = table
        .iter()
        .map(|(name, r)| (name.to_string(), r.value.clone()))
        .collect();
    client.advance_timed_slot_targets(Some(5.0), 1.0 / 60.0);
    for (name, expected) in NAMES.into_iter().zip([0.125, 90.0, 75.0, 15.0]) {
        close(target(&client, &table, name).unwrap(), expected);
    }
    assert_eq!(
        raw,
        table
            .iter()
            .map(|(name, r)| (name.to_string(), r.value.clone()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn held_empty_and_held_id_batches_sample_but_rejections_and_refreshes_do_not() {
    let mut client = ClientStateApply::new();
    let mut table = SlotTable::new();
    apply(&mut client, &mut table, 1, &[(NAMES[2], number(0, 80.0))]);
    let (fingerprint, mut resend) = records(&table, 1, &[(NAMES[2], number(0, 80.0))]);
    resend[0].kind = STATE_RECORD_KIND_DELTA;
    resend[0].has_baseline_ref = true;
    resend[0].baseline_ref = 2;
    let out = client.apply_snapshot_state(
        &mut table,
        &ReplicatedSlotIdentity::default(),
        2,
        &fingerprint,
        &resend,
    );
    assert!(out.state_batch_applied);
    assert!(out.fresh_slots.is_empty());
    client.accept_timed_slot_sample(2, &table, &out);
    assert_eq!(client.timed_slot_targets.ramps[2].newest_tick(), Some(2.0));
    apply(&mut client, &mut table, 3, &[]);
    assert_eq!(client.timed_slot_targets.ramps[2].newest_tick(), Some(3.0));
    let rejected = client.apply_snapshot_state(
        &mut table,
        &ReplicatedSlotIdentity::default(),
        4,
        &[0; 32],
        &resend,
    );
    assert!(!rejected.state_batch_applied);
    client.accept_timed_slot_sample(4, &table, &rejected);
    assert_eq!(client.timed_slot_targets.ramps[2].newest_tick(), Some(3.0));
    resend[0].baseline_id = 1;
    let excluded = client.apply_snapshot_state(
        &mut table,
        &ReplicatedSlotIdentity::default(),
        5,
        &fingerprint,
        &resend,
    );
    assert!(excluded.state_batch_applied);
    assert_eq!(excluded.refresh_requests.len(), 1);
    client.accept_timed_slot_sample(5, &table, &excluded);
    assert_eq!(client.timed_slot_targets.ramps[2].newest_tick(), Some(3.0));
}

#[test]
fn future_sample_feeds_on_timeline_and_break_discards_waiting_line() {
    let mut client = ClientStateApply::new();
    let mut table = SlotTable::new();
    ramp(&mut client, &mut table);
    // An arriving agreeing sample is ahead of playback and cannot snap it there.
    apply(&mut client, &mut table, 8, &[(NAMES[0], number(0, 0.2))]);
    close(target(&client, &table, NAMES[0]).unwrap(), 0.1);
    client.advance_timed_slot_targets(Some(5.0), 1.0 / 60.0);
    close(target(&client, &table, NAMES[0]).unwrap(), 0.125);
    apply(&mut client, &mut table, 10, &[(NAMES[0], number(0, 0.0))]);
    close(target(&client, &table, NAMES[0]).unwrap(), 0.0);
    client.advance_timed_slot_targets(Some(20.0), 15.0 / 60.0);
    close(target(&client, &table, NAMES[0]).unwrap(), 0.0);
}

#[test]
fn outage_holds_at_cap_and_first_late_agreeing_sample_retargets_its_projection() {
    let mut client = ClientStateApply::new();
    let mut table = SlotTable::new();
    ramp(&mut client, &mut table);
    client.advance_timed_slot_targets(Some(30.0), 26.0 / 60.0);
    close(target(&client, &table, NAMES[0]).unwrap(), 0.5);
    client.advance_timed_slot_targets(Some(40.0), 10.0 / 60.0);
    close(target(&client, &table, NAMES[0]).unwrap(), 0.5);
    apply(&mut client, &mut table, 12, &[(NAMES[0], number(0, 0.3))]);
    close(target(&client, &table, NAMES[0]).unwrap(), 0.7);
}

#[test]
fn sync_first_sample_reset_switch_and_absence_never_reuse_old_timeline_or_slope() {
    let mut client = ClientStateApply::new();
    let mut table = SlotTable::new();
    for tick in [100, 102, 104] {
        apply(
            &mut client,
            &mut table,
            tick,
            &[(NAMES[0], number(0, (tick - 100) as f32 * 0.025))],
        );
        client.advance_timed_slot_targets(None, 1.0 / 60.0);
    }
    close(target(&client, &table, NAMES[0]).unwrap(), 0.1);
    client.advance_timed_slot_targets(Some(103.0), 1.0 / 60.0);
    assert!(target(&client, &table, NAMES[0]).unwrap() >= 0.1);
    apply(&mut client, &mut table, 106, &[(NAMES[0], number(0, 0.15))]);
    client.advance_timed_slot_targets(Some(107.0), 4.0 / 60.0);
    assert!(target(&client, &table, NAMES[0]).unwrap() > 0.15);
    client.reset_schema();
    apply(&mut client, &mut table, 1, &[(NAMES[0], number(0, 0.25))]);
    client.advance_timed_slot_targets(Some(1.0), 1.0 / 60.0);
    close(target(&client, &table, NAMES[0]).unwrap(), 0.25);
    assert_eq!(client.timed_slot_targets.timeline, Some(1.0));
    apply(
        &mut client,
        &mut table,
        2,
        &[(NAMES[0], number(1, 0.75)), (NAMES[2], number(1, 40.0))],
    );
    close(target(&client, &table, NAMES[0]).unwrap(), 0.75);
    apply(
        &mut client,
        &mut table,
        3,
        &[(NAMES[2], WireSlotValue::Array(vec![1.0]))],
    );
    assert_eq!(target(&client, &table, NAMES[2]), None);
    apply(&mut client, &mut table, 4, &[(NAMES[2], number(1, 20.0))]);
    close(target(&client, &table, NAMES[2]).unwrap(), 20.0);
}

#[test]
fn timeline_slews_through_forward_and_backward_sync_corrections_without_rewind() {
    let mut targets = TimedSlotTargets::default();
    targets.advance(Some(10.0), 0.0);
    for estimate in [1000.0, -1000.0, 15.0] {
        let before = targets.timeline.unwrap();
        targets.advance(Some(estimate), 1.0 / 60.0);
        let step = targets.timeline.unwrap() - before;
        assert!((0.75 - 1e-10..=TOP_SLEW_RATE + 1e-10).contains(&step));
    }
}

#[test]
fn cell_requires_capacity_naming_its_host_slot_and_bounds_are_immediate() {
    let mut client = ClientStateApply::new();
    let mut table = SlotTable::new();
    client.advance_timed_slot_targets(Some(0.0), 0.0);
    for tick in [0, 2, 4] {
        apply(
            &mut client,
            &mut table,
            tick,
            &[
                (NAMES[3], number(1, 10.0 + tick as f32)),
                ("player.cellCapacity", number(0, 50.0)),
            ],
        );
        client.advance_timed_slot_targets(
            Some(f64::from(tick)),
            if tick == 0 { 0.0 } else { 2.0 / 60.0 },
        );
    }
    client.advance_timed_slot_targets(Some(5.0), 1.0 / 60.0);
    close(target(&client, &table, NAMES[3]).unwrap(), 14.0);
    apply(
        &mut client,
        &mut table,
        6,
        &[
            (NAMES[3], number(1, 16.0)),
            ("player.cellCapacity", number(1, 18.0)),
        ],
    );
    client.advance_timed_slot_targets(Some(7.0), 2.0 / 60.0);
    assert!(target(&client, &table, NAMES[3]).unwrap() > 16.0);
    client.advance_timed_slot_targets(Some(40.0), 33.0 / 60.0);
    close(target(&client, &table, NAMES[3]).unwrap(), 18.0);
    apply(
        &mut client,
        &mut table,
        8,
        &[
            (NAMES[0], number(0, 1.0)),
            (NAMES[1], number(0, 0.0)),
            (NAMES[2], number(0, 0.0)),
        ],
    );
    for (name, bound) in NAMES[..3].iter().zip([1.0, 0.0, 0.0]) {
        close(target(&client, &table, name).unwrap(), bound);
    }
}

#[test]
fn slot_round_trip_and_both_resource_absences_drop_waiting_history() {
    let mut client = ClientStateApply::new();
    let mut table = SlotTable::new();
    ramp(&mut client, &mut table);
    apply(&mut client, &mut table, 8, &[(NAMES[0], number(0, 0.2))]);
    apply(&mut client, &mut table, 10, &[(NAMES[0], number(1, 0.6))]);
    client.advance_timed_slot_targets(Some(12.0), 8.0 / 60.0);
    close(target(&client, &table, NAMES[0]).unwrap(), 0.6);
    apply(&mut client, &mut table, 12, &[(NAMES[0], number(0, 0.3))]);
    client.advance_timed_slot_targets(Some(16.0), 4.0 / 60.0);
    close(target(&client, &table, NAMES[0]).unwrap(), 0.3);
    apply(
        &mut client,
        &mut table,
        14,
        &[
            (NAMES[2], WireSlotValue::Array(vec![0.0])),
            (NAMES[3], WireSlotValue::Array(vec![0.0])),
        ],
    );
    assert_eq!(target(&client, &table, NAMES[2]), None);
    assert_eq!(target(&client, &table, NAMES[3]), None);
    apply(
        &mut client,
        &mut table,
        16,
        &[(NAMES[2], number(0, 40.0)), (NAMES[3], number(0, 20.0))],
    );
    client.advance_timed_slot_targets(Some(17.0), 1.0 / 60.0);
    close(target(&client, &table, NAMES[2]).unwrap(), 40.0);
    close(target(&client, &table, NAMES[3]).unwrap(), 20.0);
}

#[test]
fn fresh_rejoin_clock_and_apply_resume_lower_host_ticks_without_an_old_clamp() {
    use crate::endpoint::ClientTimeSync;
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    let mut retired = ClientStateApply::new();
    retired.advance_timed_slot_targets(Some(500.0), 0.0);
    let now = Arc::new(AtomicU64::new(0));
    let mut sync = ClientTimeSync::with_virtual_clock(Arc::clone(&now));
    let probe = sync.maybe_send_probe(0).unwrap();
    now.store(100_000, Ordering::Relaxed);
    assert!(sync.estimator.ingest_echo(&probe.echo(1, 0), 100_000));
    let estimate = |sync: &ClientTimeSync| {
        sync.estimated_server_tick()
            .zip(sync.rtt_micros())
            .map(|(tick, rtt)| {
                tick - rtt / (2.0 * postretro_net::timesync::DEFAULT_MICROS_PER_TICK as f64)
            })
    };
    let mut rejoined = ClientStateApply::new();
    let mut table = SlotTable::new();
    rejoined.advance_timed_slot_targets(estimate(&sync), 0.0);
    for tick in [1, 3, 5] {
        now.store(
            100_000 + u64::from(tick - 1) * 1_000_000 / 60,
            Ordering::Relaxed,
        );
        apply(
            &mut rejoined,
            &mut table,
            tick,
            &[(NAMES[0], number(0, tick as f32 * 0.05))],
        );
        rejoined
            .advance_timed_slot_targets(estimate(&sync), if tick == 1 { 0.0 } else { 2.0 / 60.0 });
    }
    assert!(rejoined.timed_slot_debug().playback_tick.unwrap() < 6.0);
    now.store(100_000 + 5 * 1_000_000 / 60, Ordering::Relaxed);
    rejoined.advance_timed_slot_targets(estimate(&sync), 1.0 / 60.0);
    assert!(target(&rejoined, &table, NAMES[0]).unwrap() > 0.25);
    assert!(retired.timed_slot_debug().playback_tick.unwrap() > 400.0);
}
