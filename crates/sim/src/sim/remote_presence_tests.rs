//! Remote presence before the first command: an owned remote pawn takes part in
//! triggers and touch from spawn, while movement stays command-gated.
// See: context/lib/networking.md §Host input command queue
use postretro_entities::Transform;
use postretro_entities::components::{
    health::HealthComponent, inventory::Inventory, touchable::TouchableComponent,
};
use postretro_entities::{
    DescriptorProvenance, DescriptorSpawnPath, TouchMode, TriggerActivation, TriggerCommand,
    TriggerFireMode, TriggerVolumeComponent,
};

use super::tests::{sim_command, trigger_movement, weapon_component};
use super::*;
use crate::trigger_system::TriggerEventEdge;

struct Fixture {
    registry: Rc<RefCell<EntityRegistry>>,
    world: CollisionWorld,
    trigger_system: TriggerSystem,
    bridge: TriggerVolumeBridge,
    touch_system: TouchSystem,
}

impl Fixture {
    /// A listen host with its own marked pawn, far from every plate and pickup.
    fn new() -> Self {
        let fixture = Self::without_host_pawn();
        {
            let mut registry = fixture.registry.borrow_mut();
            let host = registry.spawn(Transform {
                position: Vec3::new(100.0, 0.0, 0.0),
                ..Transform::default()
            });
            registry.set_component(host, trigger_movement()).unwrap();
            registry.mark_local_player_pawn(host).unwrap();
        }
        fixture
    }

    /// A host whose own pawn is gone. The registry's local fallback skips a
    /// joiner bound to its seat; an unseated one is left for the tick's guard.
    fn without_host_pawn() -> Self {
        Self {
            registry: Rc::new(RefCell::new(EntityRegistry::new())),
            world: CollisionWorld::new(),
            trigger_system: TriggerSystem::default(),
            bridge: TriggerVolumeBridge::new(),
            touch_system: TouchSystem::default(),
        }
    }

    /// A remote pawn as participation entry spawns it: movement and inventory,
    /// no local-player mark, no command queued. Unlike entry, it binds no seat.
    fn joiner(&self, at: Vec3) -> EntityId {
        let mut registry = self.registry.borrow_mut();
        let pawn = registry.spawn(Transform {
            position: at,
            ..Transform::default()
        });
        registry.set_component(pawn, trigger_movement()).unwrap();
        registry.set_component(pawn, Inventory::default()).unwrap();
        pawn
    }

    fn plate(&mut self, at: Vec3) -> EntityId {
        let trigger = {
            let mut registry = self.registry.borrow_mut();
            let trigger = registry.spawn(Transform {
                position: at,
                ..Transform::default()
            });
            let mut component = TriggerVolumeComponent::new(
                TriggerActivation::Touch,
                String::new(),
                "plate".into(),
                String::new(),
                TriggerCommand::Start,
                TriggerFireMode::Multiple,
                0.0,
                true,
            );
            component.on_exit = "plate.exit".into();
            registry.set_component(trigger, component).unwrap();
            trigger
        };
        self.bridge
            .insert_for_test(trigger, at - Vec3::splat(2.0), at + Vec3::splat(2.0));
        trigger
    }

    fn pickup(&self, at: Vec3) -> EntityId {
        let mut registry = self.registry.borrow_mut();
        let item = registry.spawn(Transform {
            position: at,
            ..Transform::default()
        });
        registry
            .set_component(item, weapon_component("pickup"))
            .unwrap();
        registry
            .set_component(
                item,
                DescriptorProvenance {
                    canonical_name: "pickup".into(),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::MapPlacement,
                },
            )
            .unwrap();
        registry
            .set_component(
                item,
                TouchableComponent {
                    mode: TouchMode::Auto,
                    radius: 1.0,
                },
            )
            .unwrap();
        item
    }

    /// One production-entry tick with `present` as the owned-remote roster and
    /// no remote command at all.
    fn tick(&mut self, present: &[(u64, EntityId)]) -> TickEvents {
        let pending = begin_tick(&self.registry, None);
        let started = finish_tick_start(
            pending,
            &mut self.registry.borrow_mut(),
            &self.world,
            |_| None,
        )
        .0;
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_runtime = postretro_ai::AiRuntime::new();
        let mut mover_states = MoverTickStateTable::default();
        let bindings = TriggerBindingTable::default();
        let slot_table = Rc::new(RefCell::new(SlotTable::new()));
        let edges = HashMap::new();
        simulate_started_tick_with_presentation_aim(
            started,
            self.registry.clone(),
            &self.world,
            &hit_zones,
            None,
            postretro_collision::EnvironmentView::level_only(-9.81),
            false,
            0.0,
            (0.0, 0.0),
            &mut progress,
            postretro_ai::test_tick_runner!(&mut ai_runtime),
            &[],
            &mut mover_states,
            &[],
            present,
            &sim_command(false, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            1.0 / 60.0,
            &mut self.touch_system,
            &mut Default::default(),
            &[],
            0,
            &FactionRegistry::default(),
            &RefCell::new(FactionSentimentState::default()),
            None,
            &edges,
            &edges,
            Some(TriggerTickContext {
                teleporters: None,
                system: &mut self.trigger_system,
                bridge: &self.bridge,
                bindings: &bindings,
                slot_table,
                script_ctx: None,
                auto_close_timers: None,
                use_edges: &edges,
            }),
            |_, _| {},
            |_| {},
            crate::sim::TimingGate::OFF,
        )
    }
}

fn edges(events: &TickEvents, edge: TriggerEventEdge) -> Vec<(EntityId, PlayerId)> {
    events
        .trigger_fires
        .iter()
        .filter(|event| event.edge == edge)
        .map(|event| (event.fire.trigger, event.fire.player))
        .collect()
}

#[test]
fn joiner_without_a_command_presses_a_plate_and_collects_a_pickup_while_holding_its_pose() {
    let mut fixture = Fixture::new();
    let spawn = Vec3::new(0.0, 5.0, 0.0);
    let pawn = fixture.joiner(spawn);
    let plate = fixture.plate(spawn);
    let item = fixture.pickup(spawn);

    let events = fixture.tick(&[(7, pawn)]);

    assert_eq!(
        edges(&events, TriggerEventEdge::Enter),
        vec![(plate, PlayerId::Remote(7))],
        "the joiner's pawn enters the plate on its first tick"
    );
    let registry = fixture.registry.borrow();
    assert_eq!(
        registry
            .get_component::<Inventory>(pawn)
            .unwrap()
            .wieldables[0],
        Some(item),
        "touch collects the pickup for the joiner"
    );
    let position = registry.get_component::<Transform>(pawn).unwrap().position;
    assert!(
        (position - spawn).length() < 1e-6,
        "no command, so movement holds the spawn pose despite gravity: {position:?}"
    );
}

#[test]
fn undeclared_dead_joiner_without_a_command_still_reaches_the_trigger_stage() {
    let mut fixture = Fixture::new();
    let pawn = fixture.joiner(Vec3::ZERO);
    let mut health = HealthComponent::from_descriptor(&postretro_entities::HealthDescriptor {
        max: 100.0,
        hitbox: None,
        zone_multipliers: Default::default(),
        ..Default::default()
    });
    health.current = 0.0;
    fixture
        .registry
        .borrow_mut()
        .set_component(pawn, health)
        .unwrap();
    let plate = fixture.plate(Vec3::ZERO);

    // An undeclared pawn at zero health keeps touch (its occupancy is the life
    // rule's, covered in `player_life_tests`); presence is what this proves.
    assert_eq!(
        edges(&fixture.tick(&[(7, pawn)]), TriggerEventEdge::Enter),
        vec![(plate, PlayerId::Remote(7))],
    );
}

#[test]
fn leaving_the_roster_fires_exactly_one_exit_and_a_fresh_pawn_enters_again() {
    let mut fixture = Fixture::new();
    let pawn = fixture.joiner(Vec3::ZERO);
    let plate = fixture.plate(Vec3::ZERO);
    assert_eq!(
        edges(&fixture.tick(&[(7, pawn)]), TriggerEventEdge::Enter).len(),
        1
    );

    // Demotion despawns the pawn and clears its owner entry.
    fixture.registry.borrow_mut().despawn(pawn).unwrap();
    let demoted = fixture.tick(&[]);
    assert_eq!(
        edges(&demoted, TriggerEventEdge::Exit),
        vec![(plate, PlayerId::Remote(7))],
        "demotion fires exactly one Exit"
    );
    assert!(edges(&fixture.tick(&[]), TriggerEventEdge::Exit).is_empty());

    // Re-promotion spawns a new pawn under the same client.
    let again = fixture.joiner(Vec3::ZERO);
    assert_eq!(
        edges(&fixture.tick(&[(7, again)]), TriggerEventEdge::Enter),
        vec![(plate, PlayerId::Remote(7))],
        "the re-promoted pawn gets a fresh Enter"
    );
}

#[test]
fn two_joiners_on_one_tick_both_enter_in_client_id_order_whatever_the_roster_order() {
    let mut fixture = Fixture::new();
    let first = fixture.joiner(Vec3::ZERO);
    let second = fixture.joiner(Vec3::ZERO);
    let plate = fixture.plate(Vec3::ZERO);
    let events = fixture.tick(&[(9, first), (3, second)]);
    assert_eq!(
        edges(&events, TriggerEventEdge::Enter),
        vec![(plate, PlayerId::Remote(3)), (plate, PlayerId::Remote(9))],
    );
}

#[test]
fn a_host_without_a_pawn_never_lists_a_joiner_as_local() {
    let mut fixture = Fixture::without_host_pawn();
    let pawn = fixture.joiner(Vec3::ZERO);
    let plate = fixture.plate(Vec3::ZERO);
    assert_eq!(
        edges(&fixture.tick(&[(7, pawn)]), TriggerEventEdge::Enter),
        vec![(plate, PlayerId::Remote(7))],
        "the registry's local fallback names the unseated joiner, which enters once, as remote"
    );
}

#[test]
fn a_host_without_a_pawn_never_steps_a_seated_joiner_with_host_input() {
    let mut fixture = Fixture::without_host_pawn();
    let spawn = Vec3::new(0.0, 5.0, 0.0);
    let pawn = fixture.joiner(spawn);
    {
        let mut registry = fixture.registry.borrow_mut();
        registry.bind_pawn_seat(pawn, postretro_foundation::Seat(7));
        let mut movement = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone();
        postretro_foundation::enter_life_state(
            &mut movement,
            1,
            &postretro_foundation::LifeStateProfile::default(),
            0,
        );
        registry.set_component(pawn, movement).unwrap();
    }

    fixture.tick(&[(7, pawn)]);

    let registry = fixture.registry.borrow();
    let position = registry.get_component::<Transform>(pawn).unwrap().position;
    assert!(
        (position - spawn).length() < 1e-6,
        "host input never steps the joiner, which holds its pose until its own command: {position:?}"
    );
    assert_eq!(
        registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .life_state_elapsed_ticks,
        1,
        "an unstepped joiner's life-state clock still advances"
    );
}

fn depleted_health() -> HealthComponent {
    let mut health = HealthComponent::from_descriptor(&postretro_entities::HealthDescriptor {
        max: 100.0,
        ..Default::default()
    });
    health.current = 0.0;
    health
}

// P16: a joiner's pawn is in play from the tick it spawns, before its first
// command arrives. Session player status reads this capture.
#[test]
fn joiner_counts_in_play_from_spawn_before_first_command() {
    let mut fixture = Fixture::new();
    let joiner = fixture.joiner(Vec3::ZERO);
    let events = fixture.tick(&[(7, joiner)]);
    assert!(events.trigger_stage_pawns.captured.contains(&joiner));
    assert!(events.trigger_stage_pawns.counted.contains(&joiner));
    let host = fixture.registry.borrow().local_player_pawn().unwrap();
    assert!(events.trigger_stage_pawns.counted.contains(&host));
    assert_eq!(events.trigger_stage_pawns.captured.len(), 2);
}

// P17: client pawns count by the effective rule. An undeclared pawn at zero
// health is captured but not counted; one with no health component counts; a
// declared pawn counts by its state's `occupancy` capability.
#[test]
fn trigger_stage_pawns_count_client_pawns_by_the_effective_rule() {
    let mut fixture = Fixture::without_host_pawn();
    let dead = fixture.joiner(Vec3::ZERO);
    let no_health = fixture.joiner(Vec3::new(10.0, 0.0, 0.0));
    let declared_down = fixture.joiner(Vec3::new(20.0, 0.0, 0.0));
    let declared_occupying = fixture.joiner(Vec3::new(30.0, 0.0, 0.0));
    {
        let mut registry = fixture.registry.borrow_mut();
        registry.set_component(dead, depleted_health()).unwrap();
        for (pawn, occupancy) in [(declared_down, false), (declared_occupying, true)] {
            registry.set_component(pawn, depleted_health()).unwrap();
            let mut movement = registry
                .get_component::<PlayerMovementComponent>(pawn)
                .cloned()
                .unwrap();
            postretro_foundation::enter_life_state(
                &mut movement,
                0,
                &postretro_foundation::LifeStateProfile {
                    occupancy,
                    ..Default::default()
                },
                0,
            );
            registry.set_component(pawn, movement).unwrap();
        }
    }
    let events = fixture.tick(&[
        (1, dead),
        (2, no_health),
        (3, declared_down),
        (4, declared_occupying),
    ]);
    let pawns = &events.trigger_stage_pawns;
    for pawn in [dead, no_health, declared_down, declared_occupying] {
        assert!(pawns.captured.contains(&pawn));
    }
    assert!(!pawns.counted.contains(&dead));
    assert!(pawns.counted.contains(&no_health));
    assert!(!pawns.counted.contains(&declared_down));
    assert!(pawns.counted.contains(&declared_occupying));
}

// O3 and S4's occupant half: occupancy is physical membership filtered by the
// capture. A disarmed or latched plate still counts its occupant, a `use`
// volume (what a `switch` compiles to) counts a player standing in it without
// a press, and an undeclared pawn at zero health occupies nothing.
#[test]
fn occupied_volumes_ignore_arm_and_latch_and_count_use_volumes() {
    let mut fixture = Fixture::without_host_pawn();
    let plate = fixture.plate(Vec3::ZERO);
    let switch = {
        let at = Vec3::new(10.0, 0.0, 0.0);
        let trigger = {
            let mut registry = fixture.registry.borrow_mut();
            let trigger = registry.spawn(Transform {
                position: at,
                ..Transform::default()
            });
            let component = TriggerVolumeComponent::new(
                TriggerActivation::Use,
                String::new(),
                "press".into(),
                String::new(),
                TriggerCommand::Start,
                TriggerFireMode::Multiple,
                0.0,
                true,
            );
            registry.set_component(trigger, component).unwrap();
            trigger
        };
        fixture
            .bridge
            .insert_for_test(trigger, at - Vec3::splat(2.0), at + Vec3::splat(2.0));
        trigger
    };
    let on_plate = fixture.joiner(Vec3::ZERO);
    let at_switch = fixture.joiner(Vec3::new(10.0, 0.0, 0.0));
    let present = [(1, on_plate), (2, at_switch)];

    let events = fixture.tick(&present);
    assert_eq!(
        events.trigger_stage_pawns.occupied_volumes,
        vec![(plate, vec![on_plate]), (switch, vec![at_switch])]
    );

    for mutate in [
        |volume: &mut TriggerVolumeComponent| volume.armed = false,
        |volume: &mut TriggerVolumeComponent| volume.latched = true,
    ] {
        {
            let mut registry = fixture.registry.borrow_mut();
            let mut volume = registry
                .get_component::<TriggerVolumeComponent>(plate)
                .cloned()
                .unwrap();
            mutate(&mut volume);
            registry.set_component(plate, volume).unwrap();
        }
        let events = fixture.tick(&present);
        assert!(
            events
                .trigger_stage_pawns
                .occupied_volumes
                .contains(&(plate, vec![on_plate]))
        );
    }

    fixture
        .registry
        .borrow_mut()
        .set_component(on_plate, depleted_health())
        .unwrap();
    let events = fixture.tick(&present);
    assert_eq!(
        events.trigger_stage_pawns.occupied_volumes,
        vec![(switch, vec![at_switch])],
        "a dead undeclared pawn stays a physical member but occupies nothing"
    );
}
