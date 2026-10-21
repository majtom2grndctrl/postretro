use super::*;
use postretro_entities::EntityRegistry;

fn sample(tick: u32, position: Vec3, generation: u8, last_teleporter: u16) -> TransformSample {
    TransformSample {
        server_tick: tick,
        transform: Transform {
            position,
            rotation: glam::Quat::from_rotation_y(generation as f32 * 0.1),
            ..Default::default()
        },
        intrinsic_velocity: Some(Vec3::X * 2.0),
        posture: None,
        aim_pitch: generation as f32 * 0.01,
        teleport: Some(TeleportSample {
            generation,
            last_teleporter,
        }),
    }
}

#[test]
fn teleport_observer_cuts_bracketing_pose_pitch_and_heading_at_every_alpha() {
    let mut buffer = RemoteInterpolationBuffer::default();
    let network = NetworkId(7);
    let before = sample(10, Vec3::ZERO, 0, u16::MAX);
    let arrival = sample(20, Vec3::new(100.0, 20.0, 30.0), 2, 1);
    buffer.record(network, before);
    buffer.record(network, arrival);
    for alpha in [0.0, 0.01, 0.25, 0.5, 0.9, 0.999, 1.0] {
        let pose = buffer.presented_pose(network, 10.0 + 10.0 * alpha).unwrap();
        let expected = if alpha < 1.0 { before } else { arrival };
        assert!(
            pose.transform
                .position
                .distance(expected.transform.position)
                < 1.0e-5
        );
        assert!(
            pose.transform
                .rotation
                .angle_between(expected.transform.rotation)
                .abs()
                < 1.0e-5
        );
        assert!((pose.aim_pitch - expected.aim_pitch).abs() < 1.0e-6);
        assert!(pose.speed_xz.abs() < 1.0e-6);
        assert!(pose.horizontal_velocity.length() < 1.0e-6);
        assert_eq!(pose.holds_newest, alpha >= 1.0);
    }
}

#[test]
fn teleport_observer_generation_wrap_starves_at_arrival_then_resumes_local_motion() {
    let mut buffer = RemoteInterpolationBuffer::default();
    let network = NetworkId(7);
    buffer.record(network, sample(10, Vec3::ZERO, 255, 0));
    let arrival = sample(11, Vec3::X * 100.0, 0, 1);
    buffer.record(network, arrival);
    for target in [11.0, 11.5, 15.0, 1000.0] {
        let pose = buffer.presented_pose(network, target).unwrap();
        assert_eq!(pose.source, PoseSource::TeleportCut);
        assert!(
            pose.holds_newest,
            "a starved arrival holds the newest sample"
        );
        assert!(pose.transform.position.distance(arrival.transform.position) < 1.0e-6);
        assert!(pose.horizontal_velocity.length() < 1.0e-6);
        assert!(pose.speed_xz.abs() < 1.0e-6);
    }
    buffer.record(network, sample(12, Vec3::X * 101.0, 0, 1));
    let resumed = buffer.presented_pose(network, 11.5).unwrap();
    assert_eq!(resumed.source, PoseSource::Interpolated);
    assert!((resumed.transform.position.x - 100.5).abs() < 1.0e-6);
    assert!(resumed.horizontal_velocity.x > 0.0);
}

#[test]
fn teleport_observer_cues_are_silent_first_per_change_and_independent_of_delayed_pose() {
    let mut registry = EntityRegistry::new();
    let entity = registry.spawn(Transform::default());
    let network = NetworkId(7);
    let mut buffer = RemoteInterpolationBuffer::default();
    buffer.record(network, sample(10, Vec3::ZERO, 254, 0));
    assert!(buffer.take_observed_teleports(|_| Some(entity)).is_empty());
    // Two accepted samples in one frame, each changing the generation (across
    // the wrap): one cue each, in acceptance order.
    buffer.record(network, sample(11, Vec3::X * 100.0, 255, 0));
    buffer.record(network, sample(12, Vec3::X * 200.0, 0, 1));
    // A later sample of the same generation must not turn the cue into a
    // moving entity anchor. The changed generation's accepted point is fixed.
    buffer.record(network, sample(13, Vec3::X * 201.0, 0, 1));
    let delayed = buffer.presented_pose(network, 10.5).unwrap();
    assert!(delayed.transform.position.length() < 1.0e-6);
    let cues = buffer.take_observed_teleports(|_| Some(entity));
    assert_eq!(cues.len(), 2);
    assert!(cues.iter().all(|cue| cue.entity_id == entity));
    assert_eq!((cues[0].generation, cues[0].last_teleporter), (255, 0));
    assert!((cues[0].arrival.x - 100.0).abs() < 1.0e-6);
    assert_eq!((cues[1].generation, cues[1].last_teleporter), (0, 1));
    assert!((cues[1].arrival.x - 200.0).abs() < 1.0e-6);
    assert!(buffer.take_observed_teleports(|_| Some(entity)).is_empty());
    // Several teleports between two accepted samples coalesce into one cue,
    // the newer sample's last teleporter.
    buffer.record(network, sample(14, Vec3::X * 300.0, 3, 2));
    let cues = buffer.take_observed_teleports(|_| Some(entity));
    assert_eq!(cues.len(), 1);
    assert_eq!((cues[0].generation, cues[0].last_teleporter), (3, 2));
    assert!(buffer.take_observed_teleports(|_| Some(entity)).is_empty());
    // An older inserted sample cannot manufacture another audible change.
    buffer.record(network, sample(9, Vec3::X * -100.0, 253, 0));
    assert!(buffer.take_observed_teleports(|_| Some(entity)).is_empty());
}

#[test]
fn teleport_observer_forget_discards_cues_and_new_identity_is_silent() {
    let mut registry = EntityRegistry::new();
    let entity = registry.spawn(Transform::default());
    let network = NetworkId(7);
    let mut buffer = RemoteInterpolationBuffer::default();
    buffer.record(network, sample(0, Vec3::ZERO, 0, u16::MAX));
    buffer.record(network, sample(1, Vec3::X, 1, 0));
    buffer.forget(network);
    assert!(buffer.take_observed_teleports(|_| Some(entity)).is_empty());
    buffer.record(network, sample(2, Vec3::X * 200.0, 2, 1));
    assert!(buffer.take_observed_teleports(|_| Some(entity)).is_empty());
    assert_ne!(
        buffer.presented_pose(network, 2.0).unwrap().source,
        PoseSource::TeleportCut
    );
}

#[test]
fn teleport_observer_sparse_pose_does_not_reuse_source_intrinsic_velocity() {
    let mut buffer = RemoteInterpolationBuffer::default();
    let network = NetworkId(7);
    buffer.record(network, sample(0, Vec3::ZERO, 0, u16::MAX));
    let mut arrival = sample(1, Vec3::X * 100.0, 1, 0);
    arrival.intrinsic_velocity = None;
    buffer.record(network, arrival);
    let mut after = sample(2, Vec3::X * 101.0, 1, 0);
    after.intrinsic_velocity = None;
    after.teleport = None;
    buffer.record(network, after);
    let pose = buffer.presented_pose(network, 1.5).unwrap();
    assert_eq!(pose.intrinsic_velocity, None);
    assert!((pose.transform.position.x - 100.5).abs() < 1.0e-6);
}
