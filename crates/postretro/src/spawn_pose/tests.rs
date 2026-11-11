use super::*;
use glam::{EulerRot, Quat};
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{EntityId, Transform};
use postretro_foundation::{
    AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementComponent,
    PlayerMovementDescriptor, SpeedParams,
};
use postretro_sim::scripting::builtins::data_archetype::spawn_from_player_starts;
use postretro_sim::scripting::builtins::net_descriptor::spawn_net_slot_pawn_with_carried_loadout;

pub(crate) fn movement_descriptor(
    name: &str,
    radius: f32,
    half_height: f32,
    eye_height: f32,
) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        canonical_name: Some(name.into()),
        movement: Some(PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius,
                half_height,
                eye_height,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                    crouch: 3.0,
                },
                accel: 10.0,
                step_height: 0.3,
                max_slope: 45.0,
                friction: GroundParams::DEFAULT_FRICTION,
                stop_speed: GroundParams::DEFAULT_STOP_SPEED,
            },
            air: AirParams {
                forward_steer: 0.0,
                accel: 0.7,
                max_control_speed: 0.5,
                bunny_hop: false,
                jumps: 0,
                jump_velocity: 5.5,
                jump_ceiling: 0.0,
            },
            fall: FallParams {
                terminal_velocity: 40.0,
            },
            stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
            stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
            knockback: Default::default(),
            sounds: None,
            dash: None,
            forgiveness: None,
            crouch: None,
            slide: None,
            swim: None,
            vault: None,
            view_feel: None,
        }),
        life_states: None,
        ghost: false,
        faction: None,
        tolerance: None,
        inventory: None,
        light: None,
        emitter: None,
        weapon: None,
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
    }
}

pub(crate) fn player_spawn(origin: Vec3, pitch: f32, yaw: f32) -> MapEntity {
    MapEntity {
        map_ordinal: None,
        classname: "player_spawn".into(),
        origin,
        angles: Vec3::new(pitch, yaw, 0.0),
        key_values: [("entity_class".into(), "player".into())]
            .into_iter()
            .collect(),
        tags: Vec::new(),
    }
}

/// Offers one seat a pose no placement backs.
pub(crate) struct OfferOne {
    pub(crate) seat: Seat,
    pub(crate) spawn: UnbackedSpawn,
    pub(crate) consumed: std::rc::Rc<std::cell::Cell<bool>>,
}

impl UnbackedSpawnSource for OfferOne {
    fn pose_for(&self, seat: Seat) -> Option<UnbackedSpawn> {
        (seat == self.seat && !self.consumed.get()).then(|| self.spawn.clone())
    }

    fn consume(&mut self, seat: Seat) {
        assert_eq!(seat, self.seat, "only the offered seat consumes");
        self.consumed.set(true);
    }
}

fn descriptors() -> Vec<EntityTypeDescriptor> {
    vec![
        movement_descriptor("player", 0.4, 0.8, 0.5),
        movement_descriptor("scout", 0.3, 0.6, 0.9),
    ]
}

/// Three starts; the second faces a nonzero yaw and pitch.
fn spawn_points() -> Vec<MapEntity> {
    vec![
        player_spawn(Vec3::ZERO, 0.0, 0.0),
        player_spawn(Vec3::new(6.0, 1.0, -2.0), -0.3, 1.2),
        player_spawn(Vec3::new(-6.0, 0.5, 4.0), 0.2, -2.0),
    ]
}

struct Fixture {
    registry: EntityRegistry,
    seats: SeatTable,
    host_pawn: EntityId,
    host_placement: usize,
    points: Vec<MapEntity>,
    descriptors: Vec<EntityTypeDescriptor>,
}

impl Fixture {
    /// The host's pawn stands on its install spawn and seat zero holds it, as
    /// install's first-player bind leaves them.
    fn new() -> Self {
        Self::with_points(spawn_points())
    }

    fn with_points(points: Vec<MapEntity>) -> Self {
        let mut registry = EntityRegistry::new();
        let descriptors = descriptors();
        let first = spawn_from_player_starts(&points, &descriptors, &mut registry, None)
            .expect("host takes the first start");
        let mut seats = SeatTable::new().unwrap();
        seats.bind_level_spawn_placement(first.pawn, first.placement);
        seats.bind_pawn(&mut registry, Seat(0), first.pawn);
        Self {
            registry,
            seats,
            host_pawn: first.pawn,
            host_placement: first.placement,
            points,
            descriptors,
        }
    }

    fn admit(&mut self, client_id: u64) -> Seat {
        self.seats
            .admit_or_reclaim(client_id, None, false)
            .expect("seat minted")
            .seat
    }

    fn resolve(&mut self, resolver: &mut SpawnPoseResolver, seat: Seat) -> Option<ResolvedSpawn> {
        let placements = SpawnPlacements::joinable(&self.points, &self.descriptors, None);
        resolver.resolve(seat, &mut self.seats, &self.registry, &placements)
    }

    fn promote(&mut self, resolver: &mut SpawnPoseResolver, seat: Seat) -> Option<ResolvedSpawn> {
        self.promotion(resolver, seat)
            .map(|promotion| promotion.spawn)
    }

    fn promotion(
        &mut self,
        resolver: &mut SpawnPoseResolver,
        seat: Seat,
    ) -> Option<PromotionSpawn> {
        let placements = SpawnPlacements::joinable(&self.points, &self.descriptors, None);
        resolver.resolve_for_promotion(seat, &mut self.seats, &self.registry, &placements)
    }

    /// Materialize `spawn` through the production net-slot spawn.
    fn spawn(&mut self, spawn: &ResolvedSpawn) -> EntityId {
        let placement = spawn
            .spawn_placement(&self.points, &self.descriptors, None)
            .expect("spawn class resolves");
        spawn_net_slot_pawn_with_carried_loadout(
            &placement,
            &self.descriptors,
            &mut self.registry,
            None,
            None,
        )
        .expect("pawn spawns")
    }

    /// The eye the client computes for a followed pawn.
    fn eye(&self, pawn: EntityId) -> Vec3 {
        let movement = self
            .registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap();
        self.registry
            .get_component::<Transform>(pawn)
            .unwrap()
            .position
            + Vec3::Y * movement.capsule.eye_height
    }
}

fn assert_pawn_at_pose(fixture: &Fixture, pawn: EntityId, pose: SpawnPose) {
    let eye = fixture.eye(pawn);
    assert!(
        eye.distance(pose.eye) < 0.01,
        "pawn eye {eye:?} is within 1 cm of the hinted eye {:?}",
        pose.eye
    );
    let rotation = fixture
        .registry
        .get_component::<Transform>(pawn)
        .unwrap()
        .rotation;
    let expected = Quat::from_euler(EulerRot::YXZ, pose.yaw, pose.pitch, 0.0);
    assert!(
        rotation.angle_between(expected) < 1e-4,
        "spawn rotation {rotation:?} carries the hinted yaw and pitch"
    );
}

#[test]
fn placement_pose_is_standing_eye_facing_the_placements_yaw_and_pitch() {
    let mut fixture = Fixture::new();
    let seat = fixture.admit(7);
    let mut resolver = SpawnPoseResolver::default();

    let spawn = fixture.resolve(&mut resolver, seat).unwrap();

    let ResolvedSpawn::Placement { map_index, pose } = spawn else {
        panic!("a player start backs the pose");
    };
    assert_eq!(map_index, 1, "the lowest start the host left free");
    let start = &fixture.points[1];
    assert!(
        pose.eye
            .distance(start.origin + Vec3::Y * (0.4 + 0.8 + 0.5))
            < 1e-5
    );
    assert_eq!((pose.yaw, pose.pitch), (start.angles.y, start.angles.x));
}

#[test]
fn placement_backed_promotion_spawns_the_pawn_eye_at_the_hinted_pose() {
    let mut fixture = Fixture::new();
    let seat = fixture.admit(7);
    let mut resolver = SpawnPoseResolver::default();
    let hinted = fixture.resolve(&mut resolver, seat).unwrap();

    let spawn = fixture.promote(&mut resolver, seat).unwrap();

    assert_eq!(spawn, hinted, "promotion reads the recorded pose");
    let pawn = fixture.spawn(&spawn);
    assert_pawn_at_pose(&fixture, pawn, hinted.pose());
}

#[test]
fn resolution_reserves_the_start_before_any_pawn_exists() {
    let mut fixture = Fixture::new();
    let first = fixture.admit(7);
    let second = fixture.admit(8);
    let mut resolver = SpawnPoseResolver::default();

    fixture.resolve(&mut resolver, first).unwrap();
    let ResolvedSpawn::Placement { map_index, .. } =
        fixture.resolve(&mut resolver, second).unwrap()
    else {
        panic!("a player start backs the pose");
    };

    assert_eq!(fixture.seats.placement_for_seat(first), Some(1));
    assert_eq!(
        map_index, 2,
        "the first seat's unspawned reservation is taken"
    );
}

#[test]
fn joiners_never_take_the_hosts_install_spawn() {
    let mut fixture = Fixture::new();
    let host_origin = fixture
        .registry
        .get_component::<Transform>(fixture.host_pawn)
        .unwrap()
        .position;
    let mut resolver = SpawnPoseResolver::default();

    for client_id in 1..=2 {
        let seat = fixture.admit(client_id);
        let ResolvedSpawn::Placement { map_index, .. } =
            fixture.resolve(&mut resolver, seat).unwrap()
        else {
            panic!("a player start backs the pose");
        };
        assert_ne!(map_index, fixture.host_placement);
    }

    assert_eq!(
        fixture.seats.placement_for_seat(Seat(0)),
        Some(fixture.host_placement)
    );
    assert_eq!(
        fixture
            .registry
            .get_component::<Transform>(fixture.host_pawn)
            .unwrap()
            .position,
        host_origin,
        "the host's pawn keeps its install spawn"
    );
}

fn scout_pose() -> UnbackedSpawn {
    UnbackedSpawn {
        pose: SpawnPose {
            eye: Vec3::new(20.0, 3.0, -9.0),
            yaw: 0.8,
            pitch: 0.25,
        },
        entity_class: "scout".into(),
        single_use: false,
    }
}

#[test]
fn unbacked_pose_reserves_no_placement_and_the_next_seat_takes_the_lowest_free() {
    let mut fixture = Fixture::new();
    let offered = fixture.admit(7);
    let other = fixture.admit(8);
    let mut resolver = SpawnPoseResolver::default();
    resolver.set_unbacked_source(Some(Box::new(OfferOne {
        seat: offered,
        spawn: scout_pose(),
        consumed: Default::default(),
    })));

    let spawn = fixture.resolve(&mut resolver, offered).unwrap();
    let after = fixture.resolve(&mut resolver, other).unwrap();

    assert_eq!(spawn, ResolvedSpawn::Unbacked(scout_pose()));
    assert_eq!(fixture.seats.placement_for_seat(offered), None);
    assert!(
        matches!(after, ResolvedSpawn::Placement { map_index: 1, .. }),
        "as if the first seat were absent: {after:?}"
    );
}

#[test]
fn unbacked_promotion_spawns_the_named_class_at_the_pose() {
    let mut fixture = Fixture::new();
    let seat = fixture.admit(7);
    let mut resolver = SpawnPoseResolver::default();
    resolver.set_unbacked_source(Some(Box::new(OfferOne {
        seat,
        spawn: scout_pose(),
        consumed: Default::default(),
    })));
    fixture.resolve(&mut resolver, seat).unwrap();

    let spawn = fixture.promote(&mut resolver, seat).unwrap();
    let pawn = fixture.spawn(&spawn);

    assert_pawn_at_pose(&fixture, pawn, scout_pose().pose);
    assert_eq!(
        fixture
            .registry
            .get_component::<DescriptorProvenance>(pawn)
            .unwrap()
            .canonical_name,
        "scout"
    );
    assert!(
        fixture
            .registry
            .get_component::<PlayerMovementComponent>(pawn)
            .is_ok_and(|movement| (movement.capsule.eye_height - 0.9).abs() < 1e-6),
        "the pawn takes the named class's eye height"
    );
    assert_eq!(fixture.seats.placement_for_seat(seat), None);
}

#[test]
fn single_use_pose_is_consumed_by_the_spawn_and_the_seat_resolves_to_a_start() {
    let mut fixture = Fixture::new();
    let seat = fixture.admit(7);
    let consumed = std::rc::Rc::new(std::cell::Cell::new(false));
    let mut resolver = SpawnPoseResolver::default();
    resolver.set_unbacked_source(Some(Box::new(OfferOne {
        seat,
        spawn: UnbackedSpawn {
            single_use: true,
            ..scout_pose()
        },
        consumed: consumed.clone(),
    })));
    fixture.resolve(&mut resolver, seat).unwrap();
    let spawn = fixture.promote(&mut resolver, seat).unwrap();
    fixture.spawn(&spawn);

    resolver.spawned(seat);

    assert!(consumed.get(), "the source is told its pose was used");
    assert_eq!(resolver.recorded(seat), None);
    assert!(matches!(
        fixture.resolve(&mut resolver, seat),
        Some(ResolvedSpawn::Placement { map_index: 1, .. })
    ));
    assert_eq!(fixture.seats.placement_for_seat(seat), Some(1));
}

#[test]
fn reusable_unbacked_pose_survives_the_spawn() {
    let mut fixture = Fixture::new();
    let seat = fixture.admit(7);
    let consumed = std::rc::Rc::new(std::cell::Cell::new(false));
    let mut resolver = SpawnPoseResolver::default();
    resolver.set_unbacked_source(Some(Box::new(OfferOne {
        seat,
        spawn: scout_pose(),
        consumed: consumed.clone(),
    })));
    fixture.resolve(&mut resolver, seat).unwrap();

    resolver.spawned(seat);

    assert!(!consumed.get());
    assert_eq!(
        resolver.recorded(seat),
        Some(&ResolvedSpawn::Unbacked(scout_pose()))
    );
}

#[test]
fn seat_with_no_recorded_pose_resolves_at_promotion() {
    let mut fixture = Fixture::new();
    let seat = fixture.admit(7);
    let mut resolver = SpawnPoseResolver::default();

    let spawn = fixture.promote(&mut resolver, seat).unwrap();

    assert!(matches!(
        spawn,
        ResolvedSpawn::Placement { map_index: 1, .. }
    ));
    assert_eq!(resolver.recorded(seat), Some(&spawn));
    assert_eq!(fixture.seats.placement_for_seat(seat), Some(1));
}

#[test]
fn level_unload_forgets_recorded_poses() {
    let mut fixture = Fixture::new();
    let seat = fixture.admit(7);
    let mut resolver = SpawnPoseResolver::default();
    fixture.resolve(&mut resolver, seat).unwrap();

    resolver.clear_for_level_unload();

    assert_eq!(resolver.recorded(seat), None);
}

#[test]
fn level_with_no_resolvable_start_resolves_nothing() {
    let mut fixture = Fixture::new();
    let seat = fixture.admit(7);
    let mut resolver = SpawnPoseResolver::default();
    let empty: Vec<MapEntity> = Vec::new();
    let placements = SpawnPlacements::joinable(&empty, &fixture.descriptors, None);

    assert_eq!(
        resolver.resolve(seat, &mut fixture.seats, &fixture.registry, &placements),
        None
    );
    assert_eq!(resolver.recorded(seat), None);
}

#[test]
fn hint_carries_the_install_tag_and_the_pose() {
    let spawn = ResolvedSpawn::Unbacked(scout_pose());

    let hint = spawn.hint("arena".into(), Some(3));

    assert_eq!(
        hint,
        SpawnPoseHint {
            level_identity: "arena".into(),
            level_life: Some(3),
            eye: [20.0, 3.0, -9.0],
            yaw: 0.8,
            pitch: 0.25,
        }
    );
}

// The Decision: a seat whose pose no placement backs holds none.
#[test]
fn unbacked_resolution_releases_the_start_the_seat_held() {
    let mut fixture = Fixture::new();
    let seat = fixture.admit(7);
    let mut resolver = SpawnPoseResolver::default();
    fixture.resolve(&mut resolver, seat).unwrap();
    assert_eq!(fixture.seats.placement_for_seat(seat), Some(1));
    resolver.set_unbacked_source(Some(Box::new(OfferOne {
        seat,
        spawn: scout_pose(),
        consumed: Default::default(),
    })));

    fixture.resolve(&mut resolver, seat).unwrap();
    let next = fixture.admit(8);

    assert_eq!(fixture.seats.placement_for_seat(seat), None);
    assert!(matches!(
        fixture.resolve(&mut resolver, next),
        Some(ResolvedSpawn::Placement { map_index: 1, .. })
    ));
}

#[test]
fn promotion_rehints_a_seat_it_resolves_or_moves_but_not_a_kept_start() {
    let mut fixture = Fixture::with_points(row_of_starts(4));
    let kept = fixture.admit(7);
    let unresolved = fixture.admit(8);
    let mut resolver = SpawnPoseResolver::default();
    fixture.resolve(&mut resolver, kept).unwrap();

    assert!(!fixture.promotion(&mut resolver, kept).unwrap().rehint);
    assert!(fixture.promotion(&mut resolver, unresolved).unwrap().rehint);

    // The host's pawn steps onto the kept seat's start.
    let onto = fixture.points[1].origin + Vec3::Y * (0.4 + 0.8);
    let mut transform = *fixture
        .registry
        .get_component::<Transform>(fixture.host_pawn)
        .unwrap();
    transform.position = onto;
    fixture
        .registry
        .set_component(fixture.host_pawn, transform)
        .unwrap();
    let moved = fixture.promotion(&mut resolver, kept).unwrap();
    assert!(moved.rehint);
    assert!(
        matches!(moved.spawn, ResolvedSpawn::Placement { map_index: 3, .. }),
        "the free start past the unresolved seat's: {moved:?}"
    );
}

fn row_of_starts(count: usize) -> Vec<MapEntity> {
    (0..count)
        .map(|index| player_spawn(Vec3::X * (8.0 * index as f32), 0.0, 0.0))
        .collect()
}

// R7: free starts remain.
#[test]
fn seats_resolved_before_spawning_take_distinct_starts_past_the_host_and_a_held_seat() {
    let mut fixture = Fixture::with_points(row_of_starts(4));
    let mut resolver = SpawnPoseResolver::default();
    let held = fixture.admit(7);
    fixture.resolve(&mut resolver, held).unwrap();
    fixture
        .seats
        .hold_disconnected_client(&mut fixture.registry, 7)
        .unwrap();
    let first = fixture.admit(8);
    let second = fixture.admit(9);

    fixture.resolve(&mut resolver, first).unwrap();
    fixture.resolve(&mut resolver, second).unwrap();

    assert_eq!(fixture.seats.placement_for_seat(held), Some(1));
    assert_eq!(fixture.seats.placement_for_seat(first), Some(2));
    assert_eq!(fixture.seats.placement_for_seat(second), Some(3));
}

// R7: every start taken. Sharing skips the start the host's pawn stands on.
#[test]
fn overflowing_seats_share_starts_round_robin_and_each_warns_once() {
    let mut fixture = Fixture::with_points(row_of_starts(3));
    let mut resolver = SpawnPoseResolver::default();
    let seats: Vec<Seat> = (7..=10).map(|client| fixture.admit(client)).collect();
    let capture = postretro_test_log_capture::LogCapture::start();

    for &seat in &seats {
        fixture.resolve(&mut resolver, seat).unwrap();
    }
    // Promotion resolves each again; the host's pawn still stands on start 0.
    for _ in 0..2 {
        for &seat in &seats {
            fixture.promote(&mut resolver, seat).unwrap();
        }
    }

    let placements: Vec<_> = seats
        .iter()
        .map(|&seat| fixture.seats.placement_for_seat(seat))
        .collect();
    assert_eq!(
        placements,
        [Some(1), Some(2), Some(1), Some(2)],
        "round-robin shares, past the host's occupied start"
    );
    let warnings = capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level == log::Level::Warn
                && record
                    .message
                    .contains("every player_spawn placement is occupied")
        })
        .count();
    assert_eq!(warnings, 2, "one warning per overflowing seat");
}
