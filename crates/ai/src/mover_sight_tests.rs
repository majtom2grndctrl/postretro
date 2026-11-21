// NPC sight sees movers at the tick's pose: perception, awareness meters, the
// fire gate and its grace. A door at x = 0.5 stands between an NPC at the
// origin and a target at x = 1, unless it is raised.
// See: context/lib/entity_model.md §7c (Sight sees movers)

use super::*;
use crate::collision::moving::MoverCollider;
use crate::collision::test_movers::{DoorPoses, door_slab};
use crate::decide::DECIDE_INTERVAL_TICKS;
use glam::Quat;

const DOOR: u32 = 4;

/// The door is still between eye and aim at any of these heights.
fn door_world() -> ([MoverCollider; 1], CollisionWorld) {
    ([door_slab(DOOR, 0.05, 6.0, 2.0)], CollisionWorld::new())
}

fn door(at_x: f32, open: bool) -> DoorPoses {
    let y = if open { 20.0 } else { -3.0 };
    DoorPoses::default().place(DOOR, Vec3::new(at_x, y, 0.0), Quat::IDENTITY)
}

/// One AI tick with the door posed as `poses`; the tick's event addresses.
fn tick_with(
    registry: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    descriptors: &[EntityTypeDescriptor],
    poses: &DoorPoses,
) -> Vec<Cow<'static, str>> {
    let (movers, world) = door_world();
    run_ai_tick_with_navigation_and_impact(
        registry,
        runtime,
        1.0 / 60.0,
        AiTickInputs {
            mover_nav: crate::nav::MoverNav {
                gate: crate::nav::NavGate::OPEN,
                movers: &movers,
                poses,
            },
            nav_graph: None,
            collision_world: Some(&world),
            descriptors,
            descriptor_generation: 0,
            factions: &FactionRegistry::default(),
            faction_sentiment: &RefCell::new(FactionSentimentState::default()),
            clips: None,
            schedule: Default::default(),
            cpu: None,
        },
        |_| {},
    )
    .events
    .into_iter()
    .filter_map(|emission| emission.address)
    .collect()
}

#[test]
fn npc_behind_a_closed_door_never_fills_its_meter_and_acquires_through_the_gap() {
    let graph = super::awareness::noticing_graph(1_000.0, 2_000.0);
    let mut watch = DoorWatch::new(&graph);
    for _ in 0..30 {
        watch.check(&door(5.0, false));
    }
    assert_eq!(watch.meter(), 0.0, "never sighted through the closed door");
    assert_eq!(watch.target(), None);
    watch.check(&door(5.0, true));
    assert_eq!(watch.target(), None, "a first sighting gains nothing");
    let checks = watch.checks_until_target(&door(5.0, true));
    assert_eq!(checks, FILL_CHECKS, "acquires at its fill through the gap");
}

#[test]
fn npc_fire_gate_stays_shut_behind_a_closed_door_and_opens_with_it() {
    let graph = standing_attack_graph();
    let mut registry = EntityRegistry::new();
    let mut runtime = AiRuntime::deciding_every_tick();
    let pawn = spawn_player(&mut registry, Vec3::new(1.0, 0.0, 0.0));
    let enemy = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        authored_brain(&graph, "strike"),
        50.0,
    );
    set_enemy_yaw(&mut registry, enemy, std::f32::consts::FRAC_PI_2);

    for _ in 0..=perception::LOS_GRACE_TICKS {
        let events = tick_with(&mut registry, &mut runtime, &[], &door(0.5, false));
        assert!(events.is_empty(), "no attack through a closed door");
    }
    assert_eq!(player_hp(&registry, pawn), 100.0);

    let events = tick_with(&mut registry, &mut runtime, &[], &door(0.5, true));
    assert_eq!(events, vec![ENEMY_ATTACK_EVENT]);
}

/// Advance every live projectile through the door world until none remain.
fn fly_out(registry: EntityRegistry, poses: &DoorPoses) -> EntityRegistry {
    let (movers, world) = door_world();
    let combined = crate::collision::moving::CombinedCollisionWorld::new(&world, &movers, poses);
    let registry = Rc::new(RefCell::new(registry));
    for _ in 0..60 {
        crate::sim::advance(
            &registry,
            &combined,
            &HitZoneStore::new(),
            0.0,
            1.0 / 60.0,
            &mut |_| {},
        );
    }
    Rc::try_unwrap(registry)
        .ok()
        .expect("the flight holds no other registry handle")
        .into_inner()
}

/// Two firing leaves that alternate every tick, so each tick is a fresh dwell
/// and the rifle fires whenever the gate is open.
fn alternating_rifle_graph() -> BehaviorGraphDescriptor {
    let fire = || {
        authored_state(
            "attack",
            MotionVerb::Hold,
            Some(ActionVerb::attack("attack")),
        )
    };
    let always = || IrNode::Const {
        value: IrValue::Bool(true),
    };
    test_behavior_graph!({
        initial: "fire_a".to_string(),
        activities: BTreeMap::from([
            ("fire_a".to_string(), fire()),
            ("fire_b".to_string(), fire()),
        ]),
        transitions: BTreeMap::from([
            ("fire_a".to_string(), vec![edge("fire_b", always())]),
            ("fire_b".to_string(), vec![edge("fire_a", always())]),
        ]),
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: BTreeMap::from([(
            "attack".to_string(),
            AttackParams {
                sound: None,
                weapon: Some("enemy.rifle".to_string()),
                damage: None,
                max_range: None,
                cooldown_ms: None,
                engagement_radius: None,
                standoff_distance: None,
                queue_range: None,
                damage_type: None,
                draws: None,
            },
        )]),
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    })
}

// P13: a door closing on an engaged NPC's retained target. Shots inside the
// grace window meet the door; the NPC stops firing after it, and fires on its
// first run once the door reopens.
#[test]
fn door_closing_on_a_retained_target_takes_the_grace_shots_and_then_stops_fire() {
    let graph = alternating_rifle_graph();
    let descriptors = [projectile_weapon_descriptor("enemy.rifle", 20.0, 13.0, 0.0)];
    let mut registry = EntityRegistry::new();
    let mut runtime = AiRuntime::deciding_every_tick();
    let pawn = spawn_player(&mut registry, Vec3::new(3.0, 0.0, 0.0));
    let mut health = registry
        .get_component::<HealthComponent>(pawn)
        .unwrap()
        .clone();
    health.hitbox = Some(Hitbox {
        half_extents: Vec3::new(0.25, 1.0, 0.25),
        offset: Vec3::ZERO,
    });
    registry.set_component(pawn, health).unwrap();
    let enemy = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        BrainComponent::from_graph(&graph),
        50.0,
    );
    let open = door(1.5, true);
    let closed = door(1.5, false);

    let events = tick_with(&mut registry, &mut runtime, &descriptors, &open);
    assert_eq!(events, vec![ENEMY_ATTACK_EVENT], "clear: it fires");
    assert_eq!(enemy_acquired_target(&registry, enemy), Some(pawn));
    // The door shuts before that first shot arrives.
    let mut grace_shots = 0;
    for _ in 1..=perception::LOS_GRACE_TICKS {
        grace_shots += tick_with(&mut registry, &mut runtime, &descriptors, &closed).len();
    }
    assert!(grace_shots > 0, "the grace window still fires");
    registry = fly_out(registry, &closed);
    assert!(projectile_ids(&registry).is_empty());
    assert_eq!(
        player_hp(&registry, pawn),
        100.0,
        "every grace shot met the door"
    );

    for _ in 0..5 {
        let events = tick_with(&mut registry, &mut runtime, &descriptors, &closed);
        assert!(events.is_empty(), "no fire after the grace window");
    }
    assert_eq!(enemy_acquired_target(&registry, enemy), Some(pawn));

    let events = tick_with(&mut registry, &mut runtime, &descriptors, &open);
    assert_eq!(
        events,
        vec![ENEMY_ATTACK_EVENT],
        "fires on its first run after"
    );
}

// An NPC without a target fills its meter through an open door; the door
// closes and the meter drains; reopened, it refills from the drained value at
// its fill rate, never acquiring on the first passing check.
#[test]
fn closed_door_drains_a_meter_and_reopening_refills_it_from_there() {
    let graph = super::awareness::noticing_graph(1_000.0, 2_000.0);
    let mut watch = DoorWatch::new(&graph);
    let (open, closed) = (door(5.0, true), door(5.0, false));
    for _ in 0..10 {
        watch.check(&open);
    }
    let filled = watch.meter();
    assert!(filled > 0.5 && watch.target().is_none(), "{filled}");
    for _ in 0..7 {
        watch.check(&closed);
    }
    let drained = watch.meter();
    assert!(drained < filled, "the closed door drains it: {drained}");
    watch.check(&open);
    assert!(
        (watch.meter() - drained).abs() < 1.0e-3,
        "a first sighting after the door gains nothing"
    );
    assert_eq!(watch.target(), None, "never on the first passing check");
    let mut previous = watch.meter();
    let mut checks = 0;
    while watch.target().is_none() {
        watch.check(&open);
        checks += 1;
        if watch.target().is_none() {
            let gained = watch.meter() - previous;
            assert!(
                (gained - CHECK_MS / 1_000.0).abs() < 1.0e-3,
                "check {checks} refills at its fill rate: {gained}"
            );
            previous = watch.meter();
        }
        assert!(checks < 40, "refills");
    }
    let needed = ((1.0 - drained) * 1_000.0 / CHECK_MS).ceil() as usize;
    // The meter crosses 1 on the check its rate reaches it; f32 accumulation
    // can push that one check later, never earlier.
    assert!(
        (needed..=needed + 1).contains(&checks),
        "{checks} vs {needed}"
    );
}

/// Milliseconds between two acquisition checks.
const CHECK_MS: f32 = DECIDE_INTERVAL_TICKS as f32 * 1_000.0 / 60.0;
/// Checks a 1000 ms fill takes after a first sighting.
const FILL_CHECKS: usize = 15;

/// A player at x = 10 and an NPC at the origin deciding at production cadence,
/// a door at x = 5 between them in whatever pose each check passes.
struct DoorWatch {
    registry: EntityRegistry,
    runtime: AiRuntime,
    npc: EntityId,
    player: EntityId,
    ticks: u64,
}

impl DoorWatch {
    fn new(graph: &BehaviorGraphDescriptor) -> Self {
        let mut registry = EntityRegistry::new();
        let player = spawn_player(&mut registry, Vec3::new(10.0, 0.0, 0.0));
        let mut brain = BrainComponent::from_graph(graph);
        brain.schedule_identity = Some(identity_deciding_on(1));
        let npc = spawn_enemy(&mut registry, Vec3::ZERO, brain, 1_000.0);
        Self {
            registry,
            runtime: AiRuntime::new(),
            npc,
            player,
            ticks: 0,
        }
    }

    /// Run to and through the NPC's next acquisition check.
    fn check(&mut self, poses: &DoorPoses) {
        loop {
            tick_with(&mut self.registry, &mut self.runtime, &[], poses);
            self.ticks += 1;
            if self.ticks % DECIDE_INTERVAL_TICKS == 1 {
                break;
            }
        }
    }

    fn checks_until_target(&mut self, poses: &DoorPoses) -> usize {
        (1..=40)
            .find(|_| {
                self.check(poses);
                self.target().is_some()
            })
            .expect("acquires")
    }

    fn brain(&self) -> &BrainComponent {
        self.registry
            .get_component::<BrainComponent>(self.npc)
            .unwrap()
    }

    fn meter(&self) -> f32 {
        self.brain().awareness.value(self.player)
    }

    fn target(&self) -> Option<EntityId> {
        self.brain().acquired_target
    }
}

// NPC-on-NPC sight reads the same world: an NPC never notices a hostile NPC
// standing behind a closed door, and does through the gap.
#[test]
fn npc_watching_a_hostile_npc_sees_the_door_as_it_does_for_a_player() {
    let graph = super::awareness::noticing_graph(1_000.0, 2_000.0);
    let mut watch = DoorWatch::new(&graph);
    let peer = spawn_enemy(
        &mut watch.registry,
        Vec3::new(10.0, 0.0, 1.0),
        BrainComponent::from_graph(&graph),
        50.0,
    );
    watch
        .registry
        .entity_state_mut(peer)
        .unwrap()
        .set(FACTION_STATE_FIELD, 7.0);
    // Only the peer counts.
    watch.registry.despawn(watch.player).unwrap();
    for _ in 0..30 {
        watch.check(&door(5.0, false));
    }
    assert_eq!(watch.brain().awareness.value(peer), 0.0);
    assert_eq!(watch.target(), None);
    watch.checks_until_target(&door(5.0, true));
    assert_eq!(watch.target(), Some(peer));
}
