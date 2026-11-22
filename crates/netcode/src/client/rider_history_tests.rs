//! Rider presentation reads each sample's authoritative mover pose from history:
//! the newest delivered sample at or before the rider's tick, never a predicted
//! one, and nothing when the history no longer reaches back (pin P8).

use glam::{Quat, Vec3};
use postretro_entities::{
    KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode, Transform,
};

use super::{MOVER_HISTORY_LIMIT, MoverHistoryBuffer, MoverHistorySample};
use crate::collision::moving::MoverPose;

const MOVER: u32 = 4;

fn sample(tick: u32, x: f32, authoritative: bool) -> MoverHistorySample {
    MoverHistorySample {
        server_tick: tick,
        pose: MoverPose {
            transform: Transform {
                position: Vec3::new(x, 0.0, 0.0),
                ..Transform::default()
            },
            linear_velocity: Vec3::ZERO,
            tick_delta: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            tick_rotation_delta: Quat::IDENTITY,
            carry_yaw: false,
            tick_dt: 1.0 / 60.0,
        },
        phase: KinematicMoverComponent::new(
            MOVER,
            KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO],
                waypoint_names: vec!["a".into()],
                speed_mps: 0.0,
                wait_ms: 0.0,
                mode: KinematicMoverMode::Once,
                started: false,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        ),
        authoritative,
    }
}

fn x_at(history: &MoverHistoryBuffer, tick: u32) -> Option<f32> {
    history
        .authoritative_transform_at_or_before(MOVER, tick)
        .map(|transform| transform.position.x)
}

#[test]
fn rider_mover_pose_is_newest_authoritative_sample_at_or_before_its_tick() {
    let mut history = MoverHistoryBuffer::default();
    // Production order: each apply records the snapshot's sample, then a
    // prediction at the fast-forward tick, so ticks interleave.
    for (tick, x, authoritative) in [
        (10, 1.0, true),
        (15, 99.0, false),
        (12, 2.0, true),
        (17, 99.0, false),
        (16, 3.0, true),
        (21, 99.0, false),
    ] {
        history.record(MOVER, sample(tick, x, authoritative));
    }
    assert_eq!(x_at(&history, 10), Some(1.0));
    assert_eq!(x_at(&history, 11), Some(1.0));
    assert_eq!(x_at(&history, 12), Some(2.0));
    // An exact-tick predicted sample never stands in for the snapshot.
    assert_eq!(x_at(&history, 15), Some(2.0));
    assert_eq!(x_at(&history, 16), Some(3.0));
    assert_eq!(x_at(&history, 40), Some(3.0));
    assert_eq!(
        history.authoritative_transform_at_or_before(MOVER + 1, 12),
        None
    );
}

// Pin P8: replay falls back to the oldest pose when a tick precedes history;
// rider presentation treats it as missing.
#[test]
fn rider_mover_pose_is_missing_once_history_no_longer_reaches_back() {
    let mut history = MoverHistoryBuffer::default();
    history.record(MOVER, sample(30, 1.0, false));
    assert_eq!(x_at(&history, 30), None, "predicted samples only");
    let first = 100u32;
    for offset in 0..=MOVER_HISTORY_LIMIT as u32 {
        history.record(MOVER, sample(first + offset, offset as f32, true));
    }
    assert_eq!(x_at(&history, first), None, "tick {first} was evicted");
    assert_eq!(x_at(&history, first + 1), Some(1.0));
    assert_eq!(x_at(&history, 50), None);
}
