use super::*;

const NET: NetworkId = NetworkId(7);
const ON: GroundRef = GroundRef::Mover(3);

fn sample(tick: u32, x: f32, ground: Option<GroundRef>, generation: u8) -> TransformSample {
    TransformSample {
        teleport: Some(TeleportSample {
            generation,
            last_teleporter: u16::MAX,
        }),
        server_tick: tick,
        transform: Transform {
            position: Vec3::new(x, 0.0, 0.0),
            ..Default::default()
        },
        intrinsic_velocity: ground.map(|_| Vec3::ZERO),
        stance: None,
        capsule_half_height: None,
        aim_pitch: 0.0,
        ground,
    }
}

fn movement(ground: GroundRef) -> MovementOnlySample {
    MovementOnlySample {
        intrinsic_velocity: Vec3::ZERO,
        stance: LocomotionStance::default(),
        capsule_half_height: 0.9,
        aim_pitch: 0.0,
        teleport: Some(TeleportSample {
            generation: 0,
            last_teleporter: u16::MAX,
        }),
        ground,
    }
}

fn bracket_grounds(
    buffer: &RemoteInterpolationBuffer,
    tick: f64,
) -> (Option<GroundRef>, Option<GroundRef>) {
    let bracket = buffer
        .presented_pose(NET, tick)
        .and_then(|pose| pose.rider_bracket)
        .expect("bracketed pose carries its samples");
    (bracket.from.ground, bracket.to.ground)
}

#[test]
fn bracket_carries_each_sample_tick_pose_and_blend() {
    let mut buffer = RemoteInterpolationBuffer::new();
    buffer.record(NET, sample(10, 0.0, Some(ON), 0));
    buffer.record(NET, sample(12, 2.0, Some(ON), 0));
    let pose = buffer.presented_pose(NET, 11.5).unwrap();
    let bracket = pose.rider_bracket.unwrap();
    assert_eq!(bracket.from.server_tick, 10);
    assert_eq!(bracket.to.server_tick, 12);
    assert!((bracket.alpha - 0.75).abs() < 1.0e-6);
    assert_eq!(bracket.from.transform.position.x, 0.0);
    assert_eq!(bracket.to.transform.position.x, 2.0);
}

#[test]
fn transform_only_samples_take_the_newest_ground_at_or_before_their_tick() {
    let mut buffer = RemoteInterpolationBuffer::new();
    buffer.record(NET, sample(10, 0.0, Some(ON), 0));
    buffer.record(NET, sample(12, 1.0, None, 0));
    buffer.record(NET, sample(14, 2.0, None, 0));
    assert_eq!(bracket_grounds(&buffer, 13.0), (Some(ON), Some(ON)));
}

#[test]
fn movement_only_samples_carry_their_ground() {
    let mut buffer = RemoteInterpolationBuffer::new();
    buffer.record(NET, sample(10, 0.0, Some(GroundRef::World), 0));
    buffer.record_intrinsic_velocity(NET, 12, movement(ON));
    buffer.record(NET, sample(14, 2.0, None, 0));
    assert_eq!(
        bracket_grounds(&buffer, 11.0),
        (Some(GroundRef::World), Some(ON))
    );
    assert_eq!(bracket_grounds(&buffer, 13.0), (Some(ON), Some(ON)));
}

// Pin P9: a ground sample arriving after a newer sparse sample still decides
// every later sample it precedes, so the rider never attaches to a mover it left.
#[test]
fn late_ground_sample_decides_every_later_sample_it_precedes() {
    let mut buffer = RemoteInterpolationBuffer::new();
    buffer.record(NET, sample(10, 0.0, Some(ON), 0));
    buffer.record(NET, sample(14, 2.0, None, 0));
    buffer.record(NET, sample(16, 3.0, None, 0));
    assert_eq!(bracket_grounds(&buffer, 15.0), (Some(ON), Some(ON)));
    // The rider stepped off at tick 12; that sample lands late.
    buffer.record(NET, sample(12, 1.0, Some(GroundRef::World), 0));
    assert_eq!(
        bracket_grounds(&buffer, 15.0),
        (Some(GroundRef::World), Some(GroundRef::World))
    );
    assert_eq!(
        bracket_grounds(&buffer, 11.0),
        (Some(ON), Some(GroundRef::World))
    );
}

#[test]
fn ground_never_reaches_back_across_a_teleport_generation() {
    let mut buffer = RemoteInterpolationBuffer::new();
    buffer.record(NET, sample(10, 0.0, Some(ON), 0));
    // Arrival in a new generation, Transform-only: its ground is unknown.
    let mut arrival = sample(12, 50.0, None, 1);
    arrival.intrinsic_velocity = None;
    buffer.record(NET, arrival);
    buffer.record(NET, sample(14, 51.0, None, 1));
    // The cut interval carries no bracket.
    assert_eq!(
        buffer.presented_pose(NET, 11.0).unwrap().rider_bracket,
        None
    );
    // The first interval after the cut names no mover until one arrives.
    assert_eq!(bracket_grounds(&buffer, 13.0), (None, None));
    buffer.record(NET, sample(16, 52.0, Some(ON), 1));
    buffer.record(NET, sample(18, 53.0, Some(ON), 1));
    assert_eq!(bracket_grounds(&buffer, 15.0), (None, Some(ON)));
    assert_eq!(bracket_grounds(&buffer, 17.0), (Some(ON), Some(ON)));
}

#[test]
fn held_and_extrapolated_poses_bracket_one_sample() {
    let mut buffer = RemoteInterpolationBuffer::new();
    buffer.record(NET, sample(10, 0.0, Some(ON), 0));
    buffer.record(NET, sample(12, 1.0, Some(ON), 0));

    let oldest = buffer.presented_pose(NET, 9.0).unwrap();
    assert_eq!(oldest.source, PoseSource::HeldOldest);
    let held = oldest.rider_bracket.unwrap();
    assert_eq!((held.from, held.alpha), (held.to, 0.0));
    assert_eq!(held.from.server_tick, 10);

    let extrapolated = buffer.presented_pose(NET, 12.5).unwrap();
    assert_eq!(extrapolated.source, PoseSource::Extrapolated);
    let held = extrapolated.rider_bracket.unwrap();
    assert_eq!(held.from, held.to);
    assert_eq!(held.from.server_tick, 12);

    let newest = buffer.presented_pose(NET, 40.0).unwrap();
    assert_eq!(newest.source, PoseSource::HeldNewest);
    let held = newest.rider_bracket.unwrap();
    assert_eq!(held.from, held.to);
    assert_eq!(held.from.server_tick, 12);
    assert_eq!(held.from.ground, Some(ON));
}
