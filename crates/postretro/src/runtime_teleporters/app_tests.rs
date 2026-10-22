// App-side teleport proofs: arrival cue routing on every machine and mover-carry
// detachment. Netcode owns the generation-change and reconcile cue decisions; these
// tests drive the app consumers of those outputs, and the host seat end to end.
// See: context/lib/audio.md §4 · context/lib/networking.md §Presentation events

use super::*;
use crate::frame_timing::{FrameTiming, InterpolableState};
use crate::kinematic_mover::{MoverTickState, MoverTickStateTable};
use glam::Quat;
use postretro_audio::{SoundAnchor, SoundRequest};
use postretro_entities::EntityId;
use postretro_foundation::{GroundRef, PlayerMovementComponent};
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::teleporters::{TeleportMode, TeleporterRecord};
use postretro_physics::collision::moving::{CombinedCollisionWorld, MoverCollider};
use postretro_physics::movement::{MovementInput, TeleportEvent};

const TICK: f32 = 1.0 / 60.0;
const ALPHAS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];
const TURNTABLE: u32 = 7;

pub(crate) fn region(min: Vec3, max: Vec3) -> EnvironmentBrush {
    let planes = (0..3)
        .flat_map(|axis| {
            let mut normal = [0.0; 3];
            normal[axis] = 1.0;
            let mut negative = [0.0; 3];
            negative[axis] = -1.0;
            [
                EnvironmentPlane {
                    normal,
                    distance: max[axis],
                },
                EnvironmentPlane {
                    normal: negative,
                    distance: -min[axis],
                },
            ]
        })
        .collect();
    EnvironmentBrush {
        aabb_min: min.to_array(),
        aabb_max: max.to_array(),
        planes,
    }
}

pub(crate) fn classic(
    source: u32,
    brush: EnvironmentBrush,
    destination: Vec3,
    yaw: f32,
) -> TeleporterRecord {
    TeleporterRecord {
        source_entity: source,
        mode: TeleportMode::Classic,
        enabled_on_spawn: true,
        brushes: vec![brush],
        destination_origin: destination.to_array(),
        destination_yaw: yaw,
        landmark_origin: [0.0; 3],
        landmark_yaw: 0.0,
        arrival_sound: String::new(),
        on_teleport: String::new(),
    }
}

pub(crate) fn level(records: Vec<TeleporterRecord>) -> LevelWorld {
    let mut world = LevelWorld::new_visibility_only(
        vec![postretro_level_loader::CellData {
            bounds_min: Vec3::splat(-100.0),
            bounds_max: Vec3::splat(100.0),
            face_start: 0,
            face_count: 0,
            portal_ref_start: 0,
            portal_ref_count: 0,
            is_solid: false,
            is_exterior: false,
            is_drawable: false,
        }],
        Vec::new(),
        postretro_level_loader::CellLocatorChild::Cell(0),
        Vec::new(),
        Vec::new(),
        false,
    )
    .unwrap();
    world.teleporters.teleporters = records;
    world
}

/// A connected-client App on `level` holding `registry`: the client lane without a
/// handshake. Its prediction is unarmed, so the correction offset is zero and the
/// movement step comes from [`predicted_step`].
pub(crate) fn client_app(level: &LevelWorld, registry: EntityRegistry) -> crate::App {
    let mut app = crate::startup::lifecycle::tests::test_app();
    let session = app.session.as_mut().unwrap();
    *session.scripting.script_ctx.registry.borrow_mut() = registry;
    session.net_endpoint = crate::netcode::NetEndpoint::from_role(
        &crate::netcode::NetRole::Connect {
            addr: std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 1)),
        },
        None,
    )
    .unwrap();
    app.collision_world = crate::collision::CollisionWorld::from_level(level).unwrap();
    app
}

/// One predicted movement tick of the local pawn through the real substrate, written
/// back as client prediction writes it: a transform snapshot, then a no-lerp write
/// on a teleport. Stands in for the armed prediction `run_client_predicted_tick`
/// calls in production.
pub(crate) fn predicted_step(
    app: &crate::App,
    input: MovementInput,
    client_tick: u32,
) -> Option<crate::netcode::ClientPredictionTickResult> {
    let ctx = app.session.as_ref()?.scripting.script_ctx.clone();
    let mut registry = ctx.registry.borrow_mut();
    let pawn = registry.local_player_movement_pawn()?;
    registry.snapshot_transforms();
    let (_, movement_events) = postretro_sim::sim::run_host_movement_tick(
        &mut registry,
        &app.collision_world,
        postretro_collision::EnvironmentView::level_only(0.0),
        &[(pawn, input)],
        TICK,
    )
    .into_iter()
    .next()?;
    Some(crate::netcode::ClientPredictionTickResult {
        client_tick,
        movement_events,
    })
}

fn movement() -> PlayerMovementComponent {
    PlayerMovementComponent::from_descriptor(&crate::mod_digest::tests::movement_descriptor())
}

/// The input a neutral local snapshot builds from `camera`, as the frame loop does.
fn neutral_input(camera: &crate::camera::Camera) -> MovementInput {
    crate::build_sim_command(
        &crate::input::ActionSnapshot::neutral(),
        camera,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
    )
    .movement
}

fn assert_point_cue(request: &SoundRequest, sound: &str, at: Vec3) {
    assert_eq!(request.sound, sound);
    assert_eq!(request.bus, "sfx");
    assert!(!request.looping);
    let Some(SoundAnchor::Point(point)) = request.anchor else {
        panic!(
            "arrival cue must use a fixed point anchor, got {:?}",
            request.anchor
        );
    };
    assert!(
        (Vec3::from(point) - at).length() < 1.0e-4,
        "cue at {point:?}, expected {at}"
    );
}

/// One turntable whose upright yaw advances `yaw_per_tick` each tick about the origin.
fn turntable(yaw_per_tick: f32) -> (Vec<MoverCollider>, MoverTickStateTable) {
    let collider = MoverCollider::from_local_triangles(
        TURNTABLE,
        &[
            Vec3::new(-3.0, 0.0, -3.0),
            Vec3::new(3.0, 0.0, -3.0),
            Vec3::new(3.0, 0.0, 3.0),
            Vec3::new(-3.0, 0.0, 3.0),
        ],
        &[[0, 1, 2], [0, 2, 3]],
    )
    .unwrap();
    let mut states = MoverTickStateTable::default();
    states.publish(
        TURNTABLE,
        MoverTickState {
            entity: EntityId::from_raw(0),
            transform: Transform::default(),
            linear_velocity: Vec3::ZERO,
            tick_delta: Vec3::ZERO,
            angular_velocity: Vec3::Y * (yaw_per_tick / TICK),
            tick_rotation_delta: Quat::from_rotation_y(yaw_per_tick),
            carry_yaw: true,
            tick_dt: TICK,
        },
    );
    (vec![collider], states)
}

struct HostSeatRun {
    yaws: Vec<f32>,
    render_yaws: Vec<[f32; 5]>,
    poses: Vec<(Vec3, Vec3)>,
    carry: GroundRef,
    teleport_tick: Option<usize>,
}

/// Host-seat ticks in the frame loop's order: input-seam mover yaw carry and ground
/// capture, authoritative movement, same-tick camera turn and follow, then hold or push.
fn run_host_seat_on_turntable(yaw_per_tick: f32) -> HostSeatRun {
    let destination = Vec3::new(20.0, 1.2, 0.0);
    let level = level(vec![classic(
        1,
        region(Vec3::new(-2.0, 0.0, -2.0), Vec3::new(2.0, 3.0, 2.0)),
        destination,
        std::f32::consts::FRAC_PI_2,
    )]);
    let collision = crate::collision::CollisionWorld::from_level(&level).unwrap();
    let (movers, states) = turntable(yaw_per_tick);
    let combined = CombinedCollisionWorld::new(&collision, &movers, &states);

    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform {
        position: Vec3::new(0.5, 1.21, 0.0),
        ..Default::default()
    });
    let mut rider = movement();
    rider.ground = GroundRef::Mover(TURNTABLE);
    let eye_height = rider.capsule.eye_height;
    registry.set_component(pawn, rider).unwrap();
    registry.mark_local_player_pawn(pawn).unwrap();

    let mut camera = crate::camera::Camera::new(Vec3::new(0.5, 1.21 + eye_height, 0.0), 0.1, 0.0);
    let mut timing = FrameTiming::new(InterpolableState::new(camera.position));
    // Captured at the preceding input seam while riding.
    let mut carry = GroundRef::Mover(TURNTABLE);
    let mut run = HostSeatRun {
        yaws: Vec::new(),
        render_yaws: Vec::new(),
        poses: Vec::new(),
        carry,
        teleport_tick: None,
    };
    for tick in 0..5 {
        crate::apply_mover_yaw_carry(&mut camera, carry, &states);
        carry = crate::local_player_ground(&registry);
        let before = local_teleport_identity(&registry);
        let input = neutral_input(&camera);
        registry.snapshot_transforms();
        let events = postretro_sim::sim::run_host_movement_tick(
            &mut registry,
            &combined,
            postretro_collision::EnvironmentView::level_only(9.8),
            &[(pawn, input)],
            TICK,
        );
        apply_local_camera_turn(&mut camera, &registry, before);
        crate::follow_camera_to_local_pawn(&mut camera, &registry, Vec3::ZERO);
        if events[0].1.teleport.is_some() {
            run.teleport_tick = Some(tick);
            hold_local_arrival(&camera, &mut timing, &mut carry);
        } else {
            timing.push_state(InterpolableState::new(camera.position));
        }
        run.yaws.push(camera.yaw);
        run.render_yaws.push(
            ALPHAS.map(|alpha| crate::effective_render_yaw(camera.yaw, carry, &states, alpha)),
        );
        let pose = registry.get_component::<Transform>(pawn).unwrap().position;
        let velocity = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .velocity;
        run.poses.push((pose, velocity));
    }
    run.carry = carry;
    run
}

// A teleport off a turning platform keeps none of its rotation after
// arrival, neither in the view (tick-rate carry or render residual) nor in motion.
#[test]
fn teleport_detaches_turntable_carry_in_view_and_motion() {
    let turning = run_host_seat_on_turntable(0.25);
    let still = run_host_seat_on_turntable(0.0);
    assert_eq!(
        turning.teleport_tick,
        Some(0),
        "rider fires on its first tick"
    );
    assert_eq!(still.teleport_tick, Some(0));
    assert_eq!(turning.carry, GroundRef::Airborne);

    for tick in 0..turning.yaws.len() {
        // Classic arrival faces the destination whatever the platform did before.
        assert!(
            (turning.yaws[tick] - still.yaws[tick]).abs() < 1.0e-5,
            "tick {tick}: turntable rotation reached the settled view after arrival"
        );
        for (alpha, yaw) in ALPHAS.iter().zip(turning.render_yaws[tick]) {
            assert!(
                (yaw - turning.yaws[tick]).abs() < 1.0e-6,
                "tick {tick} alpha {alpha}: render residual still carries platform yaw"
            );
        }
        let ((pose, velocity), (still_pose, still_velocity)) =
            (turning.poses[tick], still.poses[tick]);
        assert!(
            (pose - still_pose).length() < 1.0e-4,
            "tick {tick}: platform rotation moved the arrived pawn ({pose} vs {still_pose})"
        );
        assert!(
            (velocity - still_velocity).length() < 1.0e-4,
            "tick {tick}: platform rotation reached arrival velocity"
        );
        assert!((pose.x - 20.0).abs() < 1.0e-3);
    }

    // The fixture's carry is live: the cached ground it detached from would have
    // rotated the render view and the next seam.
    let (_, states) = turntable(0.25);
    assert!(
        (crate::effective_render_yaw(0.0, GroundRef::Mover(TURNTABLE), &states, 0.5) - 0.125).abs()
            < 1.0e-5
    );

    // The connected client's forward-predicted path detaches through the same state.
    let teleporters = postretro_physics::teleporters::Teleporters::from_section(
        &level(vec![classic(
            1,
            region(Vec3::splat(-1.0), Vec3::splat(1.0)),
            Vec3::ZERO,
            0.0,
        )])
        .teleporters,
    );
    let mut camera = crate::camera::Camera::new(Vec3::ZERO, 0.4, 0.0);
    let mut timing = FrameTiming::new(InterpolableState::new(Vec3::ZERO));
    let mut carry = GroundRef::Mover(TURNTABLE);
    let mut sounds = Vec::new();
    apply_predicted_teleport(
        &mut camera,
        &mut carry,
        &mut sounds,
        &teleporters,
        &TeleportEvent {
            teleporter: 0,
            yaw_delta: 0.3,
            arrival: Vec3::X * 20.0,
        },
    );
    camera.position = Vec3::new(20.0, 0.5, 0.0);
    hold_local_arrival(&camera, &mut timing, &mut carry);
    assert!((camera.yaw - 0.7).abs() < 1.0e-6);
    for alpha in ALPHAS {
        assert!(
            (crate::effective_render_yaw(camera.yaw, carry, &states, alpha) - 0.7).abs() < 1.0e-6
        );
    }
    crate::apply_mover_yaw_carry(&mut camera, carry, &states);
    assert!(
        (camera.yaw - 0.7).abs() < 1.0e-6,
        "next input seam carried the turntable"
    );
}

struct HostObserver {
    registry: EntityRegistry,
    collision: crate::collision::CollisionWorld,
    teleporters: postretro_physics::teleporters::Teleporters,
    owners: crate::netcode::MovementOwners,
    allocator: crate::netcode::NetworkIdAllocator,
    pawn: EntityId,
    input: MovementInput,
    tick: u32,
}

impl HostObserver {
    fn new(level: &LevelWorld, start: Vec3, velocity: Vec3) -> Self {
        let collision = crate::collision::CollisionWorld::from_level(level).unwrap();
        let teleporters =
            postretro_physics::teleporters::Teleporters::from_section(&level.teleporters);
        let mut registry = EntityRegistry::new();
        // The host seat's own pawn does not travel; the connected client's does.
        let host_pawn = registry.spawn(Transform {
            position: Vec3::new(-50.0, 0.0, 0.0),
            ..Default::default()
        });
        registry.set_component(host_pawn, movement()).unwrap();
        registry.mark_local_player_pawn(host_pawn).unwrap();
        let pawn = registry.spawn(Transform {
            position: start,
            ..Default::default()
        });
        let mut traveller = movement();
        traveller.velocity = velocity;
        registry.set_component(pawn, traveller).unwrap();
        let mut owners = crate::netcode::MovementOwners::default();
        owners.set_for_test(pawn, 7);
        let mut allocator = crate::netcode::NetworkIdAllocator::for_test();
        allocator.stamp_for_test(pawn);
        let input = neutral_input(&crate::camera::Camera::new(Vec3::ZERO, 0.0, 0.0));
        Self {
            registry,
            collision,
            teleporters,
            owners,
            allocator,
            pawn,
            input,
            tick: 0,
        }
    }
}

/// One host frame of `$ticks` authoritative ticks for the remote traveller, in the
/// frame loop's order: restore the authoritative pose, then per tick movement, the
/// host seat's own traveller cues, the pose record and the observer drain; then,
/// after the ticks, the delayed presentation write. The presentation buffer type is
/// netcode-private, so it is driven only through the public functions the app calls.
/// Yields the frame's cues, its arrival events, the observed changes, and the
/// presented traveller x at each alpha.
macro_rules! host_frame {
    ($host:expr, $buffer:ident, $ticks:expr) => {{
        let host: &mut HostObserver = $host;
        crate::netcode::host_restore_client_pawn_authoritative_poses(
            &$buffer,
            &host.owners,
            &host.allocator,
            &mut host.registry,
        );
        let mut sounds = Vec::new();
        let mut arrivals = Vec::new();
        let mut observed = Vec::new();
        for _ in 0..$ticks {
            host.registry.snapshot_transforms();
            let events = postretro_sim::sim::run_host_movement_tick(
                &mut host.registry,
                &host.collision,
                postretro_collision::EnvironmentView::level_only(0.0),
                &[(host.pawn, host.input.clone())],
                TICK,
            );
            let teleports: Vec<_> = events
                .iter()
                .filter_map(|(pawn, events)| events.teleport.map(|event| (*pawn, event)))
                .collect();
            arrivals.extend(teleports.iter().map(|(_, event)| *event));
            push_local_traveller_cues(
                &mut sounds,
                &host.teleporters,
                host.registry.local_player_movement_pawn(),
                &teleports,
            );
            crate::netcode::host_record_client_pawn_poses(
                &mut $buffer,
                &host.owners,
                &host.allocator,
                &host.registry,
                host.tick,
            );
            let tick_observed = crate::netcode::host_take_client_pawn_teleports(
                &mut $buffer,
                &host.owners,
                &host.allocator,
            );
            crate::sound_events::observed_arrival_sounds(
                &host.teleporters,
                &tick_observed,
                &mut sounds,
            );
            observed.extend(tick_observed);
            host.tick += 1;
        }
        let presented = ALPHAS.map(|alpha| {
            crate::netcode::host_present_client_pawns(
                &$buffer,
                &host.owners,
                &host.allocator,
                &mut host.registry,
                host.tick,
                alpha,
            );
            host.registry
                .interpolated_transform(host.pawn, alpha)
                .unwrap()
                .position
                .x
        });
        (sounds, arrivals, observed, presented)
    }};
}

fn facing() -> f32 {
    neutral_input(&crate::camera::Camera::new(Vec3::ZERO, 0.0, 0.0)).facing_yaw
}

pub(crate) fn sounded(mut record: TeleporterRecord, sound: &str) -> TeleporterRecord {
    record.arrival_sound = sound.into();
    record
}

// On the host seat every tick is one host sample, so a connected
// client's pawn crossing two teleporters on consecutive catch-up ticks of one host
// frame is two observed generation changes and two cues, each the teleporter's own
// at its arrival point, while the delayed presentation still shows the traveller at
// the source. The observer's first sample and later unchanged samples stay silent.
#[test]
fn teleport_host_seat_hears_each_tick_arrival_once_while_presenting_source() {
    let first_arrival = Vec3::new(10.0, 0.0, 0.0);
    let last_arrival = Vec3::new(30.0, 0.0, 0.0);
    let level = level(vec![
        sounded(
            classic(
                1,
                region(Vec3::new(-1.0, -50.0, -5.0), Vec3::new(1.0, 50.0, 5.0)),
                first_arrival,
                facing(),
            ),
            "sfx/a",
        ),
        sounded(
            classic(
                2,
                region(Vec3::new(10.02, -50.0, -5.0), Vec3::new(12.0, 50.0, 5.0)),
                last_arrival,
                facing(),
            ),
            "sfx/b",
        ),
    ]);
    let mut host = HostObserver::new(&level, Vec3::new(-1.25, 0.0, 0.0), Vec3::X * 6.0);
    let mut buffer = Default::default();

    let (sounds, arrivals, observed, _) = host_frame!(&mut host, buffer, 1);
    assert!(arrivals.is_empty() && observed.is_empty() && sounds.is_empty());

    let (sounds, arrivals, observed, presented) = host_frame!(&mut host, buffer, 3);
    assert_eq!(
        arrivals
            .iter()
            .map(|event| event.teleporter)
            .collect::<Vec<_>>(),
        [0, 1],
        "fixture: two authoritative teleports on consecutive ticks of one host frame"
    );
    assert_eq!(observed.len(), 2, "one observed change per recorded tick");
    assert!(observed.iter().all(|change| change.entity_id == host.pawn));
    assert_eq!(sounds.len(), 2, "one cue per observed generation change");
    assert_point_cue(&sounds[0], "sfx/a", first_arrival);
    assert_point_cue(&sounds[1], "sfx/b", last_arrival);
    assert!(
        presented[0] < -0.9,
        "delayed presentation still shows the source when the cue plays: {presented:?}"
    );
    for x in presented {
        assert!(
            x < -0.9 || (x - first_arrival.x).abs() < 0.2 || (x - last_arrival.x).abs() < 0.2,
            "observer rendered the traveller between source and destination: {x}"
        );
    }

    for _ in 0..3 {
        let (sounds, arrivals, observed, _) = host_frame!(&mut host, buffer, 2);
        assert!(arrivals.is_empty() && observed.is_empty());
        assert!(
            sounds.is_empty(),
            "an unchanged generation never sounds again"
        );
    }
}

// On the host seat a teleport the host refuses (here, a disarmed
// teleporter the client could have predicted through) never advances the generation,
// so no observer hears it; a teleport through a teleporter with no `arrival_sound`
// is observed and still plays nothing.
#[test]
fn teleport_host_seat_stays_silent_for_refused_and_unsounded_teleports() {
    let silent_arrival = Vec3::new(10.0, 0.0, 0.0);
    let mut refused = sounded(
        classic(
            2,
            region(Vec3::new(10.02, -50.0, -5.0), Vec3::new(12.0, 50.0, 5.0)),
            Vec3::new(30.0, 0.0, 0.0),
            facing(),
        ),
        "sfx/refused",
    );
    refused.enabled_on_spawn = false;
    let level = level(vec![
        classic(
            1,
            region(Vec3::new(-1.0, -50.0, -5.0), Vec3::new(1.0, 50.0, 5.0)),
            silent_arrival,
            facing(),
        ),
        refused,
    ]);
    let mut host = HostObserver::new(&level, Vec3::new(-1.25, 0.0, 0.0), Vec3::X * 6.0);
    let mut buffer = Default::default();
    let _ = host_frame!(&mut host, buffer, 1);

    let (sounds, arrivals, observed, _) = host_frame!(&mut host, buffer, 3);
    assert_eq!(
        arrivals.len(),
        1,
        "fixture: only the armed, unsounded teleporter fires"
    );
    assert_eq!(arrivals[0].teleporter, 0);
    assert_eq!(observed.len(), 1, "the observer saw the generation change");
    assert!(sounds.is_empty(), "no arrival_sound plays nothing");
    // Read the authoritative pose, not the delayed presentation write.
    crate::netcode::host_restore_client_pawn_authoritative_poses(
        &buffer,
        &host.owners,
        &host.allocator,
        &mut host.registry,
    );
    let x = host
        .registry
        .get_component::<Transform>(host.pawn)
        .unwrap()
        .position
        .x;
    assert!(
        x > 10.02 && x < 12.0,
        "fixture: the traveller stands in the disarmed region ({x})"
    );

    let (sounds, arrivals, observed, _) = host_frame!(&mut host, buffer, 30);
    assert!(arrivals.is_empty() && observed.is_empty() && sounds.is_empty());
}

// Traveller and observer cues on a connected client, app half. Netcode decides
// which adoption names a cue and which remote changes are accepted; the app plays the
// forward-predicted cue on its tick, an adoption's cue only when one is named, and
// one cue per accepted remote change, each at its fixed arrival point.
#[test]
fn teleport_client_cues_follow_forward_tick_adoption_cue_and_observed_changes() {
    let source = Vec3::new(0.0, 1.2, 0.0);
    let first = Vec3::new(10.0, 1.2, 0.0);
    let last = Vec3::new(30.0, 1.2, 0.0);
    let observed_at = Vec3::new(-20.0, 1.2, 4.0);
    let any = || region(Vec3::splat(-1.0), Vec3::splat(1.0));
    let teleporters = postretro_physics::teleporters::Teleporters::from_section(
        &level(vec![
            sounded(classic(1, any(), first, 0.0), "sfx/a"),
            sounded(classic(2, any(), last, 0.0), "sfx/b"),
            classic(3, any(), last, 0.0),
        ])
        .teleporters,
    );
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform {
        position: first,
        ..Default::default()
    });
    registry.set_component(pawn, movement()).unwrap();
    registry.mark_local_player_pawn(pawn).unwrap();
    let mut camera = crate::camera::Camera::new(source, 0.0, 0.0);
    let mut timing = FrameTiming::new(InterpolableState::new(source));
    let mut carry = GroundRef::World;
    let mut sounds = Vec::new();
    let adopt = |sounds: &mut Vec<SoundRequest>,
                 camera: &mut crate::camera::Camera,
                 timing: &mut FrameTiming,
                 carry: &mut GroundRef,
                 adoption: postretro_netcode::LocalTeleportAdoption| {
        apply_local_adoptions(
            camera,
            timing,
            carry,
            sounds,
            &teleporters,
            &registry,
            Vec3::ZERO,
            &[adoption],
            None,
        );
    };

    // The forward-predicted crossing starts its cue on that tick.
    apply_predicted_teleport(
        &mut camera,
        &mut carry,
        &mut sounds,
        &teleporters,
        &TeleportEvent {
            teleporter: 0,
            yaw_delta: 0.5,
            arrival: first,
        },
    );
    assert_eq!(sounds.len(), 1);
    assert_point_cue(&sounds[0], "sfx/a", first);
    // An agreeing reconcile replay reports no adoption and constructs no tick events.
    apply_local_adoptions(
        &mut camera,
        &mut timing,
        &mut carry,
        &mut sounds,
        &teleporters,
        &registry,
        Vec3::ZERO,
        &[],
        None,
    );
    assert_eq!(sounds.len(), 1, "reconcile replay never repeats the cue");

    // The host refused it; the cut back restores the yaw silently.
    adopt(
        &mut sounds,
        &mut camera,
        &mut timing,
        &mut carry,
        postretro_netcode::LocalTeleportAdoption {
            entity_id: pawn,
            yaw_delta: -0.5,
            camera_cut: true,
            identity_changed: true,
            arrival: source,
            cue_teleporter: None,
        },
    );
    assert!(
        camera.yaw.abs() < 1.0e-6,
        "refusal restores the pre-teleport yaw"
    );
    assert_eq!(sounds.len(), 1, "the cut back plays nothing");

    // A same-generation replacement through another teleporter with another
    // cue retains the speculative cue without a second playback.
    adopt(
        &mut sounds,
        &mut camera,
        &mut timing,
        &mut carry,
        postretro_netcode::LocalTeleportAdoption {
            entity_id: pawn,
            yaw_delta: 0.25,
            camera_cut: true,
            identity_changed: true,
            arrival: last,
            cue_teleporter: None,
        },
    );
    assert_eq!(sounds.len(), 1, "replacement adds no cue");

    // An ahead adoption (unseen teleports, coalesced to the last) plays the
    // authoritative last teleporter's cue once at the adopted arrival.
    adopt(
        &mut sounds,
        &mut camera,
        &mut timing,
        &mut carry,
        postretro_netcode::LocalTeleportAdoption {
            entity_id: pawn,
            yaw_delta: 1.0,
            camera_cut: true,
            identity_changed: true,
            arrival: last,
            cue_teleporter: Some(1),
        },
    );
    assert_eq!(sounds.len(), 2);
    assert_point_cue(&sounds[1], "sfx/b", last);
    assert!((camera.yaw - 1.25).abs() < 1.0e-6);
    for alpha in ALPHAS {
        let eye = timing
            .previous_state
            .lerp(&timing.current_state, alpha)
            .position;
        assert!(
            (eye - camera.position).length() < 1.0e-6,
            "adopted cut holds the arrival eye"
        );
    }

    // As an observer: one cue per accepted remote change, the last teleporter's, at
    // the sample's arrival point; a silent or unknown last teleporter plays nothing.
    let observed = [
        postretro_netcode::ObservedTeleport {
            entity_id: EntityId::from_raw(40),
            generation: 3,
            last_teleporter: 1,
            arrival: observed_at,
        },
        postretro_netcode::ObservedTeleport {
            entity_id: EntityId::from_raw(41),
            generation: 1,
            last_teleporter: 2,
            arrival: observed_at,
        },
        postretro_netcode::ObservedTeleport {
            entity_id: EntityId::from_raw(42),
            generation: 9,
            last_teleporter: postretro_foundation::NO_TELEPORTER,
            arrival: observed_at,
        },
    ];
    let mut observer_sounds = Vec::new();
    crate::sound_events::observed_arrival_sounds(&teleporters, &observed, &mut observer_sounds);
    assert_eq!(observer_sounds.len(), 1);
    assert_point_cue(&observer_sounds[0], "sfx/b", observed_at);
    crate::sound_events::observed_arrival_sounds(&teleporters, &[], &mut observer_sounds);
    assert_eq!(observer_sounds.len(), 1);
}

// A teleporter with no `arrival_sound` plays nothing through any app cue path:
// forward prediction, adoption, the host seat's own crossing, or an observed change.
#[test]
fn teleport_without_arrival_sound_plays_nothing_on_any_cue_path() {
    let arrival = Vec3::new(5.0, 0.0, 0.0);
    let any = || region(Vec3::splat(-1.0), Vec3::splat(1.0));
    let teleporters = postretro_physics::teleporters::Teleporters::from_section(
        &level(vec![
            classic(1, any(), arrival, 0.0),
            sounded(classic(2, any(), arrival, 0.0), "sfx/control"),
        ])
        .teleporters,
    );
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    registry.set_component(pawn, movement()).unwrap();
    registry.mark_local_player_pawn(pawn).unwrap();
    for (index, expected) in [(0_u16, 0_usize), (1, 1)] {
        let event = TeleportEvent {
            teleporter: index,
            yaw_delta: 0.0,
            arrival,
        };
        let mut camera = crate::camera::Camera::new(Vec3::ZERO, 0.0, 0.0);
        let mut timing = FrameTiming::new(InterpolableState::new(Vec3::ZERO));
        let mut carry = GroundRef::World;

        let mut sounds = Vec::new();
        apply_predicted_teleport(&mut camera, &mut carry, &mut sounds, &teleporters, &event);
        assert_eq!(
            sounds.len(),
            expected,
            "forward prediction, teleporter {index}"
        );

        let mut sounds = Vec::new();
        apply_local_adoptions(
            &mut camera,
            &mut timing,
            &mut carry,
            &mut sounds,
            &teleporters,
            &registry,
            Vec3::ZERO,
            &[postretro_netcode::LocalTeleportAdoption {
                entity_id: pawn,
                yaw_delta: 0.0,
                camera_cut: true,
                identity_changed: true,
                arrival,
                cue_teleporter: Some(index),
            }],
            None,
        );
        assert_eq!(sounds.len(), expected, "adoption, teleporter {index}");

        let mut sounds = Vec::new();
        push_local_traveller_cues(&mut sounds, &teleporters, Some(pawn), &[(pawn, event)]);
        assert_eq!(
            sounds.len(),
            expected,
            "host seat traveller, teleporter {index}"
        );

        let mut sounds = Vec::new();
        crate::sound_events::observed_arrival_sounds(
            &teleporters,
            &[postretro_netcode::ObservedTeleport {
                entity_id: pawn,
                generation: 1,
                last_teleporter: index,
                arrival,
            }],
            &mut sounds,
        );
        assert_eq!(sounds.len(), expected, "observer, teleporter {index}");
    }
}

// On a connected client, through the frame loop's own predicted
// tick seam: a forward-predicted teleport on the first, middle or last of three
// catch-up ticks turns the view on that tick, starts its cue once, detaches carry,
// and holds the camera on the arrival eye, so neither camera nor pawn mesh renders
// between source and destination at any alpha, that frame or a zero-tick frame after.
#[test]
fn teleport_client_predicted_tick_cuts_on_first_middle_last_and_zero_tick_frames() {
    use std::f32::consts::FRAC_PI_2;
    let destination = Vec3::new(20.0, 0.0, 0.0);
    let level = level(vec![sounded(
        classic(
            1,
            region(Vec3::new(-1.0, -50.0, -5.0), Vec3::new(1.0, 50.0, 5.0)),
            destination,
            facing() + FRAC_PI_2,
        ),
        "sfx/a",
    )]);
    for teleport_tick in 0..3_u32 {
        let start = Vec3::new(-1.0 - 0.1 * (teleport_tick as f32 + 0.5), 0.0, 0.0);
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform {
            position: start,
            ..Default::default()
        });
        let mut traveller = movement();
        traveller.velocity = Vec3::X * 6.0;
        let eye_height = traveller.capsule.eye_height;
        registry.set_component(pawn, traveller).unwrap();
        registry.mark_local_player_pawn(pawn).unwrap();
        let mut app = client_app(&level, registry);
        app.camera = crate::camera::Camera::new(start + Vec3::Y * eye_height, 0.0, 0.2);
        app.frame_timing = FrameTiming::new(InterpolableState::new(app.camera.position));
        app.mover_yaw_carry_ground = GroundRef::Mover(TURNTABLE);

        let rendered_x = |app: &crate::App| {
            let ctx = app.session.as_ref().unwrap().scripting.script_ctx.clone();
            let registry = ctx.registry.borrow();
            ALPHAS.map(|alpha| {
                let eye = app
                    .frame_timing
                    .previous_state
                    .lerp(&app.frame_timing.current_state, alpha)
                    .position
                    .x;
                let mesh = registry
                    .interpolated_transform(pawn, alpha)
                    .unwrap()
                    .position
                    .x;
                (eye, mesh)
            })
        };
        let mut teleports = Vec::new();
        for tick in 0..3_u32 {
            let input = neutral_input(&app.camera);
            let result =
                app.run_client_predicted_tick(true, |app| predicted_step(app, input, tick));
            if let Some(event) = result.and_then(|result| result.movement_events.teleport) {
                teleports.push((tick, event));
            }
            for (eye, mesh) in rendered_x(&app) {
                for x in [eye, mesh] {
                    assert!(
                        x < -0.9 || (x - destination.x).abs() < 1.0e-3,
                        "teleport tick {teleport_tick}, tick {tick}: rendered between source \
                         and destination at x {x}"
                    );
                }
            }
        }
        assert_eq!(teleports.len(), 1, "one predicted teleport");
        assert_eq!(
            teleports[0].0, teleport_tick,
            "fixture: crosses on that tick"
        );
        assert!(
            (app.camera.yaw - FRAC_PI_2).abs() < 1.0e-5,
            "the view turned by the predicted delta"
        );
        assert!((app.camera.pitch - 0.2).abs() < 1.0e-6, "pitch untouched");
        assert_eq!(app.mover_yaw_carry_ground, GroundRef::Airborne);
        assert_eq!(app.pending_teleport_sounds.len(), 1, "the cue starts once");
        assert_point_cue(
            &app.pending_teleport_sounds[0],
            "sfx/a",
            teleports[0].1.arrival,
        );
        // A zero-tick frame performs no push or prediction; the cut still holds.
        for (eye, mesh) in rendered_x(&app) {
            assert!((eye - destination.x).abs() < 1.0e-3);
            assert!((mesh - destination.x).abs() < 1.0e-3);
        }
    }
}

// Traveller cues coalesce per frame and carry detaches on identity alone: several
// adoptions drained in one frame play only the last cue, at its own arrival point;
// a plain teleport-class correction (three metres or more, or a composed
// presentation offset reaching that floor, teleport record unchanged) holds the
// camera but keeps the platform's yaw carry.
#[test]
fn teleport_frame_adoptions_coalesce_to_the_last_cue_and_detach_carry_on_identity_change() {
    let first = Vec3::new(10.0, 1.2, 0.0);
    let last = Vec3::new(30.0, 1.2, 0.0);
    let any = || region(Vec3::splat(-1.0), Vec3::splat(1.0));
    let teleporters = postretro_physics::teleporters::Teleporters::from_section(
        &level(vec![
            sounded(classic(1, any(), first, 0.0), "sfx/a"),
            sounded(classic(2, any(), last, 0.0), "sfx/b"),
        ])
        .teleporters,
    );
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform {
        position: last,
        ..Default::default()
    });
    let movement = movement();
    let eye = last + Vec3::Y * movement.capsule.eye_height;
    registry.set_component(pawn, movement).unwrap();
    registry.mark_local_player_pawn(pawn).unwrap();
    let adoption = |yaw_delta, identity_changed, arrival, cue_teleporter| {
        postretro_netcode::LocalTeleportAdoption {
            entity_id: pawn,
            yaw_delta,
            camera_cut: true,
            identity_changed,
            arrival,
            cue_teleporter,
        }
    };
    let mut camera = crate::camera::Camera::new(Vec3::ZERO, 0.0, 0.0);
    let mut timing = FrameTiming::new(InterpolableState::new(Vec3::ZERO));
    let mut carry = GroundRef::Mover(TURNTABLE);
    let mut sounds = Vec::new();
    apply_local_adoptions(
        &mut camera,
        &mut timing,
        &mut carry,
        &mut sounds,
        &teleporters,
        &registry,
        Vec3::ZERO,
        &[
            adoption(0.5, true, first, Some(0)),
            adoption(0.25, true, last, Some(1)),
        ],
        None,
    );
    assert_eq!(
        sounds.len(),
        1,
        "a frame's unseen teleports coalesce to one cue"
    );
    assert_point_cue(&sounds[0], "sfx/b", last);
    assert!(
        (camera.yaw - 0.75).abs() < 1.0e-6,
        "every adoption turns the view"
    );
    assert_eq!(carry, GroundRef::Airborne);

    let mut camera = crate::camera::Camera::new(Vec3::ZERO, 0.0, 0.0);
    let mut timing = FrameTiming::new(InterpolableState::new(Vec3::ZERO));
    let mut carry = GroundRef::Mover(TURNTABLE);
    let mut sounds = Vec::new();
    apply_local_adoptions(
        &mut camera,
        &mut timing,
        &mut carry,
        &mut sounds,
        &teleporters,
        &registry,
        Vec3::ZERO,
        &[adoption(0.0, false, last, None)],
        None,
    );
    assert_eq!(
        carry,
        GroundRef::Mover(TURNTABLE),
        "a plain correction keeps the platform's carry"
    );
    assert!(sounds.is_empty() && camera.yaw.abs() < 1.0e-6);
    for alpha in ALPHAS {
        let shown = timing
            .previous_state
            .lerp(&timing.current_state, alpha)
            .position;
        assert!(
            (shown - eye).length() < 1.0e-5,
            "the correction cut still holds"
        );
    }
}

// Regression: a camera cut followed by a smoothed correction in the same frame held
// the bare registry eye, dropping the composed offset, so the next tick's presented
// eye jumped back by the correction before gliding.
#[test]
fn teleport_cut_then_smoothed_correction_in_one_frame_holds_presented_eye() {
    let cut_pose = Vec3::new(10.0, 1.2, 0.0);
    // The smoothed reconcile after the cut moved the registry by -correction and left
    // `correction` as the composed offset, so registry plus offset is the cut pose.
    let correction = Vec3::new(0.3, 0.0, -0.2);
    let teleporters =
        postretro_physics::teleporters::Teleporters::from_section(&level(Vec::new()).teleporters);
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform {
        position: cut_pose - correction,
        ..Default::default()
    });
    let movement = movement();
    let cut_eye = cut_pose + Vec3::Y * movement.capsule.eye_height;
    registry.set_component(pawn, movement).unwrap();
    registry.mark_local_player_pawn(pawn).unwrap();
    // What the next predicted tick follows: registry plus offset plus eye height.
    let presented = crate::local_pawn_eye_position(&registry, correction).unwrap();
    assert!(
        (presented - cut_eye).length() < 1.0e-5,
        "fixture: the presented eye is the cut eye"
    );
    let cut = postretro_netcode::LocalTeleportAdoption {
        entity_id: pawn,
        yaw_delta: 0.0,
        camera_cut: true,
        identity_changed: false,
        arrival: cut_pose,
        cue_teleporter: None,
    };
    // Hold with `held_offset`, then run the next tick's follow and push at rest.
    let rendered = |held_offset: Vec3| {
        let mut camera = crate::camera::Camera::new(Vec3::ZERO, 0.0, 0.0);
        let mut timing = FrameTiming::new(InterpolableState::new(Vec3::ZERO));
        let mut carry = GroundRef::World;
        let mut sounds = Vec::new();
        apply_local_adoptions(
            &mut camera,
            &mut timing,
            &mut carry,
            &mut sounds,
            &teleporters,
            &registry,
            held_offset,
            &[cut],
            None,
        );
        let held = ALPHAS.map(|alpha| {
            timing
                .previous_state
                .lerp(&timing.current_state, alpha)
                .position
        });
        crate::follow_camera_to_local_pawn(&mut camera, &registry, correction);
        timing.push_state(InterpolableState::new(camera.position));
        let next = ALPHAS.map(|alpha| {
            timing
                .previous_state
                .lerp(&timing.current_state, alpha)
                .position
        });
        (held, next)
    };
    let (held, next) = rendered(correction);
    for eye in held.into_iter().chain(next) {
        assert!(
            (eye - presented).length() < 1.0e-5,
            "the held and next-tick eye stay on the presented eye: {eye} vs {presented}"
        );
    }
    // Control: holding the bare registry eye bounces by the correction.
    let (bare, _) = rendered(Vec3::ZERO);
    assert!(
        (bare[0] - (presented - correction)).length() < 1.0e-5,
        "control: a bare registry hold omits the offset"
    );
}

/// An animated avatar mesh, so the tick's pose pass writes its aim inputs.
fn avatar_mesh() -> postretro_entities::components::mesh::MeshComponent {
    use postretro_entities::components::animation::{
        AnimationState, InterruptPolicy, MeshAnimation,
    };
    let states = std::collections::HashMap::from([(
        "idle".to_string(),
        AnimationState {
            clip: "idle".into(),
            looping: true,
            crossfade_ms: 0.0,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: None,
            clip_index: Some(0),
        },
    )]);
    let mut mesh = postretro_entities::components::mesh::MeshComponent::stateless("avatar".into());
    mesh.animation = Some(MeshAnimation::new(states, "idle".into()));
    mesh
}

// On the host seat, through the full authoritative tick: the teleport
// tick's yaw delta reaches every aim captured before movement. The host seat's
// presentation aim and a remote pawn's aim and facing turn for the avatar pose pass,
// and a remote shot's delivered start aim turns with them, so that shot leaves the
// arrival eye along the rotated view.
#[test]
fn teleport_same_tick_yaw_reaches_presentation_aims_and_a_remote_start_aim() {
    use postretro_entities::components::inventory::Inventory;
    use postretro_entities::components::mesh::MeshComponent;
    use postretro_entities::components::weapon::WeaponComponent;
    use postretro_foundation::{ActivationLane, ActivationToken, ShotId};
    use std::f32::consts::FRAC_PI_2;
    let destination = Vec3::new(20.0, 0.0, 0.0);
    let level = level(vec![classic(
        1,
        region(Vec3::new(-1.0, -50.0, -5.0), Vec3::new(1.0, 50.0, 5.0)),
        destination,
        facing() + FRAC_PI_2,
    )]);
    let collision = crate::collision::CollisionWorld::from_level(&level).unwrap();
    let registry = std::rc::Rc::new(std::cell::RefCell::new(EntityRegistry::new()));
    let (local, remote, weapon) = {
        let mut registry = registry.borrow_mut();
        let local = registry.spawn(Transform::default());
        registry.set_component(local, movement()).unwrap();
        registry.set_component(local, avatar_mesh()).unwrap();
        registry.mark_local_player_pawn(local).unwrap();
        let remote = registry.spawn(Transform {
            position: Vec3::new(0.0, 0.0, 2.0),
            ..Default::default()
        });
        registry.set_component(remote, movement()).unwrap();
        registry.set_component(remote, avatar_mesh()).unwrap();
        let descriptor: postretro_foundation::WeaponDescriptor = serde_json::from_value(
            serde_json::json!({
                "damage": 5.0, "range": 20.0, "resolution": "projectile",
                "primary": {"trigger": "press", "recoveryMs": 50.0, "steps": [{"kind": "shot"}]},
                "projectile": {"speed": 4.0, "radius": 0.1, "lifetimeMs": 1000.0,
                    "visual": {"body": {"kind": "sprite", "sprite": "sprites/projectiles/plasma.png", "size": 0.2}}}
            }),
        )
        .unwrap();
        let weapon = registry.spawn(Transform::default());
        registry
            .set_component(weapon, WeaponComponent::from_descriptor(&descriptor))
            .unwrap();
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(weapon);
        registry.set_component(remote, inventory).unwrap();
        (local, remote, weapon)
    };
    let mut command = crate::build_sim_command(
        &crate::input::ActionSnapshot::neutral(),
        &crate::camera::Camera::new(Vec3::ZERO, 0.0, 0.0),
        false,
        false,
        false,
        false,
        false,
        false,
        false,
    );
    let local_command = command.clone();
    let start = ActivationToken {
        start_tick: 9,
        lane: ActivationLane::Primary,
    };
    command.input_tick = 9;
    command.fire_button = crate::weapon::FireButtonState {
        pressed: true,
        active: true,
    };
    command.activation.initiation = Some(start);
    let remote_command = postretro_sim::sim::RemotePawnCommand {
        real_command: true,
        rejected_activation: None,
        pawn: remote,
        owner_client_id: 7,
        weapon: Some(weapon),
        shot_id: Some(ShotId::from_parts(42, 9, ActivationLane::Primary, 0)),
        fire_tick: 9,
        client_tick: 9,
        aim_pitch: 0.0,
        // The start's own command declared the pre-teleport view.
        start_aim: Some(postretro_sim::sim::RemoteStartAim {
            pitch: 0.0,
            yaw: command.movement.facing_yaw,
        }),
        command,
    };
    let presentation_aim = (0.1, facing());
    let mut progress = postretro_scripting_core::reaction_dispatch::ProgressTracker::new();
    let mut ai = postretro_ai::AiRuntime::new();
    let mut movers = MoverTickStateTable::default();
    let mut touch = crate::sim::touch::TouchSystem::default();
    let factions = postretro_entities::FactionRegistry::default();
    let sentiment = std::cell::RefCell::new(postretro_entities::FactionSentimentState::default());
    let events = postretro_sim::sim::simulate_tick_with_presentation_aim(
        registry.clone(),
        &collision,
        &crate::scripting_systems::hit_zones::HitZoneStore::new(),
        None,
        postretro_collision::EnvironmentView::level_only(0.0),
        false,
        0.0,
        presentation_aim,
        &mut progress,
        postretro_ai::tick_runner!(&mut ai),
        &[],
        &mut movers,
        &[remote_command],
        &local_command,
        |_| postretro_sim::sim::PostMovementCommand {
            aim_origin: Vec3::ZERO,
            aim_direction: Vec3::NEG_Z,
        },
        TICK,
        &mut touch,
        &[],
        0,
        &factions,
        &sentiment,
        None,
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
        None,
        |_, _| {},
        |_| {},
        postretro_sim::sim::TimingGate::OFF,
    );
    assert_eq!(
        events.teleports.len(),
        2,
        "fixture: both pawns teleport this tick"
    );
    let registry = registry.borrow();
    let pose = |pawn| {
        registry
            .get_component::<MeshComponent>(pawn)
            .unwrap()
            .pose_inputs
            .unwrap()
    };
    let yaw_of = |pawn| {
        registry
            .get_component::<Transform>(pawn)
            .unwrap()
            .rotation
            .to_euler(glam::EulerRot::YXZ)
            .0
    };
    let turned = facing() + FRAC_PI_2;
    assert!(
        (pose(local).aim_yaw - turned).abs() < 1.0e-5,
        "host seat avatar aims along its rotated view"
    );
    assert!(
        (pose(local).aim_pitch - 0.1).abs() < 1.0e-5,
        "pitch unchanged"
    );
    assert!(
        (pose(remote).aim_yaw - turned).abs() < 1.0e-5,
        "remote aim rotated"
    );
    assert!(
        (yaw_of(remote) - turned).abs() < 1.0e-5,
        "remote facing rotated"
    );
    let [shot] = events.authorized_shots.as_slice() else {
        panic!(
            "the remote start fires once: {:?}",
            events.authorized_shots.len()
        );
    };
    let eye_height = registry
        .get_component::<PlayerMovementComponent>(remote)
        .unwrap()
        .capsule
        .eye_height;
    assert!(
        shot.shot
            .fire_origin
            .distance(destination + Vec3::Y * eye_height)
            < 1.0e-4,
        "the remote shot leaves the arrival eye: {}",
        shot.shot.fire_origin
    );
    let rotated_view = Vec3::new(-turned.sin(), 0.0, -turned.cos());
    assert!(
        shot.shot
            .projectile_direction
            .unwrap()
            .distance(rotated_view)
            < 1.0e-4,
        "along the rotated start aim"
    );
}
