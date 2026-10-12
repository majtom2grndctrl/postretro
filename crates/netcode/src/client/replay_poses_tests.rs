//! The replay memo must reproduce `MoverHistoryBuffer::pose_at_tick` exactly,
//! for every mover at every replayed tick, and reuse its buffers once warm.

use glam::{Quat, Vec3};
use postretro_entities::{
    KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode, Transform,
};

use super::super::{MoverHistoryBuffer, MoverHistorySample};
use crate::collision::moving::MoverPose;
use crate::kinematic_mover::advance_mover_phase_one_tick;

const DT: f32 = 1.0 / 60.0;

fn mover(
    id: u32,
    waypoints: Vec<Vec3>,
    speed_mps: f32,
    wait_ms: f32,
    spin_rate: f32,
) -> KinematicMoverComponent {
    let names = (0..waypoints.len())
        .map(|index| format!("node-{index}"))
        .collect();
    KinematicMoverComponent::new(
        id,
        KinematicMoverConfig {
            waypoints,
            waypoint_names: names,
            speed_mps,
            wait_ms,
            mode: KinematicMoverMode::PingPong,
            started: true,
            spin_axis: if spin_rate == 0.0 {
                Vec3::ZERO
            } else {
                Vec3::Y
            },
            initial_spin_rate_rad_s: spin_rate,
            spin_accel_rad_s2: 0.0,
            carry_yaw: false,
        },
    )
}

/// The mover set every test replays: a fast ping-pong that reverses within a
/// few ticks, a waiting door, a slow lift, a spinner and a held (blocked) mover.
fn movers() -> Vec<KinematicMoverComponent> {
    let mut held = mover(5, vec![Vec3::ZERO, Vec3::Z * 3.0], 2.0, 0.0, 0.0);
    held.blocked = true;
    vec![
        mover(1, vec![Vec3::ZERO, Vec3::X * 0.2], 3.0, 0.0, 0.0),
        mover(
            2,
            vec![Vec3::ZERO, Vec3::Y * 2.0, Vec3::new(1.0, 2.0, 0.0)],
            4.0,
            50.0,
            0.0,
        ),
        mover(3, vec![Vec3::ZERO, Vec3::Y * 8.0], 0.5, 0.0, 0.0),
        mover(4, vec![Vec3::ZERO], 0.0, 0.0, 2.5),
        held,
    ]
}

/// Run each mover as a host would and record the ticks in `recorded`, in the
/// order given (which may repeat or run out of order).
fn host_history(start_tick: u32, recorded: &[u32]) -> MoverHistoryBuffer {
    let mut history = MoverHistoryBuffer::default();
    record_host_ticks(&mut history, start_tick, recorded);
    history
}

fn record_host_ticks(history: &mut MoverHistoryBuffer, start_tick: u32, recorded: &[u32]) {
    for mut phase in movers() {
        let mut transform = Transform::default();
        let mut pose = MoverPose {
            transform,
            linear_velocity: Vec3::ZERO,
            tick_delta: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            tick_rotation_delta: Quat::IDENTITY,
            carry_yaw: false,
            tick_dt: DT,
        };
        let mut tick = start_tick;
        let mut by_tick = std::collections::HashMap::new();
        let last = recorded
            .iter()
            .map(|tick| tick.wrapping_sub(start_tick))
            .max()
            .unwrap_or(0);
        for _ in 0..=last {
            by_tick.insert(tick, (pose, phase.clone()));
            pose = advance_mover_phase_one_tick(&mut phase, &mut transform, DT);
            tick = tick.wrapping_add(1);
        }
        for tick in recorded {
            let (pose, phase) = by_tick[tick].clone();
            history.record(
                phase.mover_id,
                MoverHistorySample {
                    server_tick: *tick,
                    pose,
                    phase,
                },
            );
        }
    }
}

/// Replay `ticks` in order through the memo and hold every pose to the reference.
fn assert_replay_matches(history: &mut MoverHistoryBuffer, ticks: impl IntoIterator<Item = u32>) {
    for tick in ticks {
        history.resolve_replay_tick(tick, DT);
        for id in 0..=6u32 {
            assert_bit_identical(
                history.replay_pose(id),
                history.pose_at_tick(id, tick, DT),
                &format!("mover {id} at replayed tick {tick}"),
            );
        }
    }
}

/// `{:?}` prints the shortest round-tripping form of every float, so for
/// non-NaN values equal strings mean equal bits (every NaN prints alike; poses
/// are finite). `PartialEq` would let `-0.0` pass for `0.0`.
fn assert_bit_identical(memo: Option<MoverPose>, reference: Option<MoverPose>, context: &str) {
    assert_eq!(format!("{memo:?}"), format!("{reference:?}"), "{context}");
}

/// Heap addresses and capacities of the memo's reusable storage.
fn scratch_fingerprint(history: &MoverHistoryBuffer) -> Vec<(u32, usize, usize, usize)> {
    let memo = &history.replay;
    let mut fingerprint: Vec<_> = memo
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
        memo.entries.as_ptr() as usize,
        memo.entries.capacity(),
        memo.translation_legs.capacity(),
    ));
    fingerprint
}

#[test]
fn memo_matches_reference_across_replay_depths() {
    for depth in [0u32, 1, 2, 7, 31, 64] {
        let mut history = host_history(100, &[100, 101, 102, 103]);
        assert_replay_matches(&mut history, (1..=depth).map(|offset| 103 + offset));
    }
}

#[test]
fn memo_matches_reference_through_reversals_and_waits() {
    // Mover 1 crosses its 0.2 m path in four ticks; mover 2 waits at nodes.
    let mut history = host_history(0, &[0, 1, 2]);
    assert_replay_matches(&mut history, 3..=200);
}

#[test]
fn memo_matches_reference_when_history_runs_past_the_replay_start() {
    // Samples newer than the authoritative tick: replay hits exact samples at
    // 12 and 15 mid-sequence and must reseed from each.
    let mut history = host_history(0, &[8, 9, 10, 12, 15]);
    assert_replay_matches(&mut history, 11..=30);
}

#[test]
fn memo_matches_reference_for_out_of_order_and_duplicate_samples() {
    let mut history = host_history(0, &[4, 6, 5, 6, 3]);
    assert_replay_matches(&mut history, 5..=20);
}

#[test]
fn memo_matches_reference_before_the_oldest_sample() {
    // Every replayed tick precedes the history: the reference falls back to the
    // oldest sample unadvanced.
    let mut history = host_history(0, &[40, 41]);
    assert_replay_matches(&mut history, 30..=45);
}

#[test]
fn memo_matches_reference_across_tick_wraparound() {
    let start = u32::MAX - 4;
    let mut history = host_history(start, &[start, start + 1, start + 2]);
    assert_replay_matches(
        &mut history,
        (3..40u32).map(|offset| start.wrapping_add(offset)),
    );
}

#[test]
fn memo_reports_no_pose_for_movers_without_history() {
    let mut history = MoverHistoryBuffer::default();
    history.resolve_replay_tick(10, DT);
    assert_eq!(history.replay_pose(1), None);

    let mut history = host_history(0, &[0]);
    history.resolve_replay_tick(1, DT);
    assert_eq!(
        history.replay_pose(99),
        None,
        "unknown ids fall back to the live table"
    );
    assert!(history.replay_pose(1).is_some());
}

#[test]
fn memo_reseeds_between_reconciles() {
    let mut history = host_history(0, &[0, 1]);
    assert_replay_matches(&mut history, 2..=10);
    // A second reconcile restarts behind the memo's working tick.
    assert_replay_matches(&mut history, 2..=10);
    // New history arrives; the working phase must not continue from stale seeds.
    record_host_ticks(&mut history, 0, &[2, 3]);
    assert_replay_matches(&mut history, 4..=12);
    // Repeating a tick within one replay returns the same pose.
    assert_replay_matches(&mut history, [13, 13, 14]);
}

#[test]
fn memo_reseeds_when_a_sample_is_replaced_in_place() {
    let mut history = host_history(0, &[0, 1]);
    assert_replay_matches(&mut history, 2..=10);
    // A corrected sample for tick 1 replaces the newest one in place: same
    // deque index, same tick, different phase (taken five host ticks later).
    record_host_ticks(&mut history, 1u32.wrapping_sub(5), &[1]);
    assert_replay_matches(&mut history, 11..=20);
}

#[test]
fn memo_reseeds_when_the_tick_length_changes() {
    let mut history = host_history(0, &[0, 1]);
    assert_replay_matches(&mut history, 2..=10);
    // Same generation and seed, later tick, different step length: the working
    // phase was stepped at DT and must not continue.
    let half = DT * 0.5;
    for tick in 11..=16 {
        history.resolve_replay_tick(tick, half);
        for id in 0..=6u32 {
            assert_bit_identical(
                history.replay_pose(id),
                history.pose_at_tick(id, tick, half),
                &format!("mover {id} at tick {tick} with a half-length step"),
            );
        }
    }
}

#[test]
fn memo_reuses_its_buffers_once_warm() {
    let mut history = host_history(0, &[0, 1, 2]);
    assert_replay_matches(&mut history, 3..=20);
    let warm = scratch_fingerprint(&history);

    // Next snapshot: same movers, newer samples. Reseeding goes through
    // `clone_from`, so every phase buffer and the memo's own storage stay put.
    record_host_ticks(&mut history, 0, &[3, 4]);
    assert_replay_matches(&mut history, 5..=25);
    assert_eq!(scratch_fingerprint(&history), warm);
}

#[test]
fn clear_drops_the_memo() {
    let mut history = host_history(0, &[0]);
    history.resolve_replay_tick(3, DT);
    assert!(history.replay_pose(1).is_some());
    history.clear();
    assert_eq!(history.replay_pose(1), None);
    assert_eq!(
        scratch_fingerprint(&history).len(),
        1,
        "only the memo summary row"
    );
}

/// Manual measurement, not a gate: pose-resolution cost of replaying `k` ticks
/// with 12 movers and 6 mover passes per tick, reference vs memo.
/// `cargo test -p postretro-netcode --release --lib replay_pose_cost -- --ignored --nocapture --test-threads=1`
#[test]
#[ignore]
fn replay_pose_cost() {
    use std::hint::black_box;
    use std::time::Instant;

    const MOVERS: u32 = 12;
    const PASSES: usize = 6;
    let mut history = MoverHistoryBuffer::default();
    for id in 0..MOVERS {
        let phase = mover(
            id,
            vec![Vec3::ZERO, Vec3::X * 4.0, Vec3::new(4.0, 2.0, 0.0)],
            1.0,
            0.0,
            0.0,
        );
        for tick in 0..=10u32 {
            history.record(
                id,
                MoverHistorySample {
                    server_tick: tick,
                    pose: MoverPose {
                        transform: Transform::default(),
                        linear_velocity: Vec3::ZERO,
                        tick_delta: Vec3::ZERO,
                        angular_velocity: Vec3::ZERO,
                        tick_rotation_delta: Quat::IDENTITY,
                        carry_yaw: false,
                        tick_dt: DT,
                    },
                    phase: phase.clone(),
                },
            );
        }
    }
    const RUNS: u32 = 200;
    for k in [1u32, 6, 12, 32] {
        let start = Instant::now();
        let mut found = 0usize;
        for _ in 0..RUNS {
            for tick in 11..11 + k {
                for _ in 0..PASSES {
                    for id in 0..MOVERS {
                        found += black_box(history.pose_at_tick(id, tick, DT)).is_some() as usize;
                    }
                }
            }
        }
        let reference = start.elapsed().as_secs_f64() * 1.0e6 / f64::from(RUNS);

        let start = Instant::now();
        for _ in 0..RUNS {
            for tick in 11..11 + k {
                history.resolve_replay_tick(tick, DT);
                for _ in 0..PASSES {
                    for id in 0..MOVERS {
                        found += black_box(history.replay_pose(id)).is_some() as usize;
                    }
                }
            }
        }
        let memo = start.elapsed().as_secs_f64() * 1.0e6 / f64::from(RUNS);
        println!(
            "replay k={k:>2}: reference {reference:9.1} us  memo {memo:7.1} us  per reconcile ({found})"
        );
    }
}
