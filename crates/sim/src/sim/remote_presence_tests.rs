//! Remote presence before the first command: an owned remote pawn takes part in
//! triggers and touch from spawn, while movement stays command-gated.
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
    fn new() -> Self {
        Self {
            registry: Rc::new(RefCell::new(EntityRegistry::new())),
            world: CollisionWorld::new(),
            trigger_system: TriggerSystem::default(),
            bridge: TriggerVolumeBridge::new(),
            touch_system: TouchSystem::default(),
        }
    }

    /// A remote pawn as participation entry spawns it: movement and inventory,
    /// no local-player mark, no command queued.
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
        let started =
            finish_tick_start(pending, &mut self.registry.borrow_mut(), &self.world, |_| None).0;
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
        registry.get_component::<Inventory>(pawn).unwrap().wieldables[0],
        Some(item),
        "touch collects the pickup for the joiner"
    );
    assert_eq!(
        registry.get_component::<Transform>(pawn).unwrap().position,
        spawn,
        "no command, so movement holds the spawn pose despite gravity"
    );
}

#[test]
fn undeclared_dead_joiner_is_present_but_occupies_nothing() {
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
    fixture.plate(Vec3::ZERO);

    fixture.tick(&[(7, pawn)]);

    let stage = player_life::TriggerStageCapabilities::capture(
        &fixture.registry.borrow(),
        &[AuthoritativePlayer {
            id: PlayerId::Remote(7),
            pawn,
        }],
    );
    assert!(
        !stage.occupants.contains(&PlayerId::Remote(7)),
        "a present pawn at zero health is no occupant"
    );
}

#[test]
fn leaving_the_roster_fires_exactly_one_exit_and_a_fresh_pawn_enters_again() {
    let mut fixture = Fixture::new();
    let pawn = fixture.joiner(Vec3::ZERO);
    let plate = fixture.plate(Vec3::ZERO);
    assert_eq!(edges(&fixture.tick(&[(7, pawn)]), TriggerEventEdge::Enter).len(), 1);

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
fn an_empty_roster_with_no_pawn_ticks_quietly() {
    let mut fixture = Fixture::new();
    fixture.plate(Vec3::ZERO);
    let events = fixture.tick(&[]);
    assert!(events.trigger_fires.is_empty());
}

#[test]
fn two_joiners_on_one_tick_enter_in_client_id_order_every_run() {
    for _ in 0..3 {
        let mut fixture = Fixture::new();
        let first = fixture.joiner(Vec3::ZERO);
        let second = fixture.joiner(Vec3::ZERO);
        let plate = fixture.plate(Vec3::ZERO);
        let events = fixture.tick(&[(3, second), (9, first)]);
        assert_eq!(
            edges(&events, TriggerEventEdge::Enter),
            vec![(plate, PlayerId::Remote(3)), (plate, PlayerId::Remote(9))],
        );
    }
}
