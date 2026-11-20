// Seam stage proofs: the brush-union inside test, the D11 latch table,
// activation gates and occupancy.
// See: context/lib/entity_model.md §7e

use super::*;
use glam::Quat;
use postretro_entities::Transform;
use postretro_entities::components::health::HealthComponent;
use postretro_foundation::{
    AirParams, CapsuleParams, FallParams, GroundParams, LifeStateProfile, PlayerMovementComponent,
    PlayerMovementDescriptor, SpeedParams, enter_life_state,
};
use postretro_level_format::environment_volumes::EnvironmentPlane;
use postretro_level_format::seams::SeamMode;

const RADIUS: f32 = 0.4;
const HALF_HEIGHT: f32 = 0.8;

fn movement() -> PlayerMovementComponent {
    PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
        sounds: None,
        knockback: Default::default(),
        capsule: CapsuleParams {
            radius: RADIUS,
            half_height: HALF_HEIGHT,
            eye_height: 0.5,
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
        dash: None,
        forgiveness: None,
        crouch: None,
        slide: None,
        swim: None,
        vault: None,
        view_feel: None,
    })
}

fn box_brush(min: Vec3, max: Vec3) -> EnvironmentBrush {
    let mut planes = Vec::new();
    for axis in 0..3 {
        let mut normal = Vec3::ZERO;
        normal[axis] = 1.0;
        planes.push(EnvironmentPlane {
            normal: normal.to_array(),
            distance: max[axis],
        });
        planes.push(EnvironmentPlane {
            normal: (-normal).to_array(),
            distance: -min[axis],
        });
    }
    EnvironmentBrush {
        aabb_min: min.to_array(),
        aabb_max: max.to_array(),
        planes,
    }
}

fn record(name: &str, activation: SeamActivation, brushes: Vec<EnvironmentBrush>) -> SeamRecord {
    SeamRecord {
        source_entity: 1,
        name: name.into(),
        target: "next".into(),
        activation,
        mode: SeamMode::Classic,
        enabled_on_spawn: true,
        brushes,
        landmark_origin: [0.0; 3],
        landmark_yaw: 0.0,
        on_arrive: String::new(),
    }
}

/// An L-shaped (bent) seam: a hallway along +X from the origin, bending up
/// +Z at its far end. The corner region beside the bend, inside the union
/// AABB but outside both brushes, is `BESIDE_BEND`.
fn bent() -> Vec<EnvironmentBrush> {
    vec![
        box_brush(Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 3.0, 2.0)),
        box_brush(Vec3::new(8.0, 0.0, 2.0), Vec3::new(10.0, 3.0, 10.0)),
    ]
}
const BESIDE_BEND: Vec3 = Vec3::new(2.0, 1.0, 8.0);
const IN_HALL: Vec3 = Vec3::new(2.0, 1.0, 1.0);
const OUTSIDE: Vec3 = Vec3::new(-20.0, 1.0, -20.0);

struct Rig {
    registry: EntityRegistry,
    stage: SeamStage,
    seam: EntityId,
}

fn rig(activation: SeamActivation, armed: bool) -> Rig {
    let mut registry = EntityRegistry::new();
    let section = SeamsSection {
        seams: vec![record("door", activation, bent())],
    };
    let mut stage = SeamStage::install(&section, false);
    let seam = registry.spawn(Transform::default());
    registry
        .set_component(
            seam,
            SeamComponent {
                index: 0,
                source_entity: 1,
                name: "door".into(),
                armed,
                on_arrive: String::new(),
            },
        )
        .unwrap();
    stage.bind_entity(0, seam);
    Rig {
        registry,
        stage,
        seam,
    }
}

fn spawn_player(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
    let id = registry.spawn(Transform {
        position,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    });
    registry.set_component(id, movement()).unwrap();
    id
}

fn move_to(registry: &mut EntityRegistry, pawn: EntityId, position: Vec3) {
    let mut transform = *registry.get_component::<Transform>(pawn).unwrap();
    transform.position = position;
    registry.set_component(pawn, transform).unwrap();
}

fn set_armed(registry: &mut EntityRegistry, seam: EntityId, armed: bool) {
    let mut component = registry
        .get_component::<SeamComponent>(seam)
        .unwrap()
        .clone();
    component.armed = armed;
    registry.set_component(seam, component).unwrap();
}

fn players(pawns: &[EntityId]) -> Vec<AuthoritativePlayer> {
    pawns
        .iter()
        .map(|&pawn| AuthoritativePlayer {
            id: PlayerId::Local(pawn),
            pawn,
        })
        .collect()
}

/// One stage run; returns whether the seam activated and its counted pawns.
fn tick(rig: &mut Rig, pawns: &[EntityId], use_by: &[EntityId]) -> (bool, Vec<EntityId>) {
    let players = players(pawns);
    let capabilities = TriggerStageCapabilities::capture(&rig.registry, &players);
    let use_pressed: HashMap<PlayerId, bool> = use_by
        .iter()
        .map(|&pawn| (PlayerId::Local(pawn), true))
        .collect();
    let occupied = rig
        .stage
        .run_tick(&rig.registry, &players, &capabilities, &use_pressed);
    let fired = !rig.stage.take_activations().is_empty();
    let counted = occupied
        .into_iter()
        .find(|(volume, _)| *volume == rig.seam)
        .map(|(_, pawns)| pawns)
        .unwrap_or_default();
    (fired, counted)
}

fn capsule_at(center: Vec3) -> SeamCapsule {
    SeamCapsule {
        center,
        radius: RADIUS,
        half_height: HALF_HEIGHT,
    }
}

#[test]
fn seam_inside_test_is_the_brush_union_not_its_bounding_box() {
    let rig = rig(SeamActivation::Touch, true);
    assert!(rig.stage.contains(0, capsule_at(IN_HALL)));
    assert!(
        !rig.stage.contains(0, capsule_at(BESIDE_BEND)),
        "beside the bend is inside the union AABB but outside both brushes"
    );
    // A capsule whose body point is outside but whose side reaches in overlaps.
    assert!(
        rig.stage
            .contains(0, capsule_at(Vec3::new(-RADIUS * 0.5, 1.0, 1.0)))
    );
    // Feet just above the top face still overlap through the segment's end.
    assert!(
        rig.stage
            .contains(0, capsule_at(Vec3::new(2.0, 3.0 + HALF_HEIGHT, 1.0)))
    );
    assert!(!rig.stage.contains(0, capsule_at(OUTSIDE)));
}

#[test]
fn seam_touch_fires_on_entry_but_not_beside_the_bend() {
    let mut rig = rig(SeamActivation::Touch, true);
    let pawn = spawn_player(&mut rig.registry, BESIDE_BEND);
    rig.stage.latch_spawned_in(&rig.registry, pawn);
    assert!(
        !rig.stage.is_latched(pawn, 0),
        "a spawn beside the bend overlaps no brush, so it takes no latch"
    );
    assert_eq!(tick(&mut rig, &[pawn], &[]), (false, vec![]));
    move_to(&mut rig.registry, pawn, IN_HALL);
    assert_eq!(tick(&mut rig, &[pawn], &[]), (true, vec![pawn]));
    // Standing inside does not fire again.
    assert_eq!(tick(&mut rig, &[pawn], &[]).0, false);
}

#[test]
fn seam_latch_follows_the_d11_table_for_touch() {
    let mut rig = rig(SeamActivation::Touch, true);
    // Arrives overlapping it with its body point outside: latched.
    let pawn = spawn_player(&mut rig.registry, Vec3::new(-RADIUS * 0.5, 1.0, 1.0));
    rig.stage.latch_spawned_in(&rig.registry, pawn);
    assert!(rig.stage.is_latched(pawn, 0));
    // Latched, still overlapping, moving deeper: no fire.
    assert!(!tick(&mut rig, &[pawn], &[]).0);
    move_to(&mut rig.registry, pawn, IN_HALL);
    assert!(!tick(&mut rig, &[pawn], &[]).0);
    // Body point leaves first while the capsule still overlaps (pin P4).
    move_to(&mut rig.registry, pawn, Vec3::new(-RADIUS * 0.5, 1.0, 1.0));
    assert!(!tick(&mut rig, &[pawn], &[]).0);
    assert!(rig.stage.is_latched(pawn, 0));
    // Capsule leaves: the latch clears, nothing fires.
    move_to(&mut rig.registry, pawn, OUTSIDE);
    assert!(!tick(&mut rig, &[pawn], &[]).0);
    assert!(!rig.stage.is_latched(pawn, 0));
    // Re-entry fires.
    move_to(&mut rig.registry, pawn, IN_HALL);
    assert!(tick(&mut rig, &[pawn], &[]).0);
}

#[test]
fn seam_latch_blocks_use_until_the_capsule_leaves() {
    let mut rig = rig(SeamActivation::Use, true);
    let pawn = spawn_player(&mut rig.registry, IN_HALL);
    rig.stage.latch_spawned_in(&rig.registry, pawn);
    assert!(
        !tick(&mut rig, &[pawn], &[pawn]).0,
        "latched: use does not fire"
    );
    move_to(&mut rig.registry, pawn, OUTSIDE);
    assert!(
        !tick(&mut rig, &[pawn], &[pawn]).0,
        "use without overlap does not fire"
    );
    move_to(&mut rig.registry, pawn, IN_HALL);
    assert!(
        !tick(&mut rig, &[pawn], &[]).0,
        "entry alone does not fire a use seam"
    );
    assert!(
        tick(&mut rig, &[pawn], &[pawn]).0,
        "a press while overlapping fires"
    );
}

#[test]
fn seam_arming_a_touch_seam_fires_for_unlatched_standing_occupants_next_tick() {
    let mut rig = rig(SeamActivation::Touch, false);
    let pawn = spawn_player(&mut rig.registry, IN_HALL);
    assert!(
        !tick(&mut rig, &[pawn], &[]).0,
        "a disarmed seam never fires"
    );
    set_armed(&mut rig.registry, rig.seam, true);
    rig.stage.note_newly_armed(&[0]);
    assert!(tick(&mut rig, &[pawn], &[]).0);
    assert!(!tick(&mut rig, &[pawn], &[]).0, "standing on fires once");
}

#[test]
fn seam_arming_keeps_latches() {
    let mut rig = rig(SeamActivation::Touch, false);
    let pawn = spawn_player(&mut rig.registry, IN_HALL);
    rig.stage.latch_spawned_in(&rig.registry, pawn);
    set_armed(&mut rig.registry, rig.seam, true);
    rig.stage.note_newly_armed(&[0]);
    assert!(
        !tick(&mut rig, &[pawn], &[]).0,
        "latched, armed from disarmed: no fire"
    );
    assert!(rig.stage.is_latched(pawn, 0));
}

#[test]
fn seam_latch_in_a_disarmed_seam_clears_on_leave_so_arming_fires() {
    // Pin P18.
    let mut rig = rig(SeamActivation::Touch, false);
    let pawn = spawn_player(&mut rig.registry, IN_HALL);
    rig.stage.latch_spawned_in(&rig.registry, pawn);
    move_to(&mut rig.registry, pawn, OUTSIDE);
    tick(&mut rig, &[pawn], &[]);
    move_to(&mut rig.registry, pawn, IN_HALL);
    assert!(!tick(&mut rig, &[pawn], &[]).0, "still disarmed");
    set_armed(&mut rig.registry, rig.seam, true);
    rig.stage.note_newly_armed(&[0]);
    assert!(tick(&mut rig, &[pawn], &[]).0);
}

#[test]
fn seam_respawn_latch_holds_whatever_the_armed_state() {
    // Pins P14 and P19: a respawn onto a start inside a seam latches, armed or
    // not, and the latch outlives arming on the same tick.
    for armed in [true, false] {
        let mut rig = rig(SeamActivation::Touch, armed);
        let pawn = spawn_player(&mut rig.registry, OUTSIDE);
        tick(&mut rig, &[pawn], &[]);
        move_to(&mut rig.registry, pawn, IN_HALL);
        rig.stage.latch_spawned_in(&rig.registry, pawn);
        if !armed {
            set_armed(&mut rig.registry, rig.seam, true);
            rig.stage.note_newly_armed(&[0]);
        }
        assert!(!tick(&mut rig, &[pawn], &[]).0);
        move_to(&mut rig.registry, pawn, OUTSIDE);
        tick(&mut rig, &[pawn], &[]);
        move_to(&mut rig.registry, pawn, IN_HALL);
        assert!(tick(&mut rig, &[pawn], &[]).0, "leave and re-enter fires");
    }
}

#[test]
fn seam_disarming_keeps_latches_and_never_fires() {
    let mut rig = rig(SeamActivation::Touch, true);
    let pawn = spawn_player(&mut rig.registry, IN_HALL);
    rig.stage.latch_spawned_in(&rig.registry, pawn);
    set_armed(&mut rig.registry, rig.seam, false);
    assert!(!tick(&mut rig, &[pawn], &[]).0);
    assert!(rig.stage.is_latched(pawn, 0));
}

#[test]
fn seam_teleport_arrival_inside_a_touch_seam_fires_on_that_tick() {
    // Pin P9: a teleport is not a spawn and takes no seam latch, so the stage
    // sees an ordinary entry.
    let mut rig = rig(SeamActivation::Touch, true);
    let pawn = spawn_player(&mut rig.registry, OUTSIDE);
    tick(&mut rig, &[pawn], &[]);
    move_to(&mut rig.registry, pawn, IN_HALL);
    assert!(tick(&mut rig, &[pawn], &[]).0);
}

#[test]
fn seam_zero_health_undeclared_pawn_cannot_activate_and_a_declared_one_follows_its_state() {
    let mut first = rig(SeamActivation::Touch, true);
    let corpse = spawn_player(&mut first.registry, OUTSIDE);
    let mut health = HealthComponent::from_descriptor(&postretro_entities::HealthDescriptor {
        max: 100.0,
        ..Default::default()
    });
    health.current = 0.0;
    first
        .registry
        .set_component(corpse, health.clone())
        .unwrap();
    tick(&mut first, &[corpse], &[]);
    move_to(&mut first.registry, corpse, IN_HALL);
    assert!(!tick(&mut first, &[corpse], &[]).0, "as it cannot teleport");

    for touch in [false, true] {
        let mut declared = rig(SeamActivation::Touch, true);
        let rig = &mut declared;
        let downed = spawn_player(&mut rig.registry, OUTSIDE);
        rig.registry.set_component(downed, health.clone()).unwrap();
        let mut movement = rig
            .registry
            .get_component::<PlayerMovementComponent>(downed)
            .unwrap()
            .clone();
        let profile = LifeStateProfile {
            touch,
            ..LifeStateProfile::default()
        };
        enter_life_state(&mut movement, 0, &profile, 0);
        rig.registry.set_component(downed, movement).unwrap();
        tick(rig, &[downed], &[]);
        move_to(&mut rig.registry, downed, IN_HALL);
        assert_eq!(tick(rig, &[downed], &[]).0, touch);
    }
}

#[test]
fn seam_occupancy_counts_by_the_inside_test_armed_or_not() {
    for armed in [true, false] {
        let mut rig = rig(SeamActivation::Touch, armed);
        let inside = spawn_player(&mut rig.registry, IN_HALL);
        let beside = spawn_player(&mut rig.registry, BESIDE_BEND);
        let corpse = spawn_player(&mut rig.registry, IN_HALL);
        let mut health = HealthComponent::from_descriptor(&postretro_entities::HealthDescriptor {
            max: 100.0,
            ..Default::default()
        });
        health.current = 0.0;
        rig.registry.set_component(corpse, health).unwrap();
        let (_, counted) = tick(&mut rig, &[inside, beside, corpse], &[]);
        assert_eq!(counted, vec![inside], "armed={armed}");
    }
}

#[test]
fn seam_in_a_backdrop_never_activates() {
    let mut registry = EntityRegistry::new();
    let section = SeamsSection {
        seams: vec![record("door", SeamActivation::Touch, bent())],
    };
    let mut stage = SeamStage::install(&section, true);
    let pawn = spawn_player(&mut registry, OUTSIDE);
    let players = players(&[pawn]);
    let run = |stage: &mut SeamStage, registry: &EntityRegistry| {
        let capabilities = TriggerStageCapabilities::capture(registry, &players);
        stage.run_tick(registry, &players, &capabilities, &HashMap::new());
    };
    run(&mut stage, &registry);
    move_to(&mut registry, pawn, IN_HALL);
    run(&mut stage, &registry);
    assert!(stage.take_activations().is_empty());
}

#[test]
fn seam_activations_record_each_seam_once_in_activation_order() {
    let mut registry = EntityRegistry::new();
    let section = SeamsSection {
        seams: vec![
            record(
                "first",
                SeamActivation::Touch,
                vec![box_brush(Vec3::ZERO, Vec3::splat(2.0))],
            ),
            record(
                "second",
                SeamActivation::Touch,
                vec![box_brush(
                    Vec3::new(10.0, 0.0, 0.0),
                    Vec3::new(12.0, 2.0, 2.0),
                )],
            ),
        ],
    };
    let mut stage = SeamStage::install(&section, false);
    let a = spawn_player(&mut registry, OUTSIDE);
    let b = spawn_player(&mut registry, Vec3::new(11.0, 1.0, 1.0));
    let players = players(&[a, b]);
    let mut run = |registry: &EntityRegistry| {
        let capabilities = TriggerStageCapabilities::capture(registry, &players);
        stage.run_tick(registry, &players, &capabilities, &HashMap::new());
    };
    // The second seam in map order activates first, the first a tick later.
    run(&registry);
    move_to(&mut registry, a, Vec3::splat(1.0));
    run(&registry);
    assert_eq!(stage.take_activations(), vec![1, 0]);
    assert!(stage.take_activations().is_empty());
}
