// The swim slots on the host: real pawns driven through host movement into a fluid volume, read per player and by player events.
// See: context/lib/scripting.md §5 (Per-player engine slots), §12 (Player events)

use glam::{Vec2, Vec3};
use postretro_collision::{
    ConvexLeaf, EnvironmentView, EnvironmentVolume, EnvironmentVolumes, LeafPlane,
};
use postretro_entities::components::immersion::ImmersionComponent;
use postretro_entities::{
    EntityId, ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    Transform,
};
use postretro_foundation::{IrNode, PlayerMovementComponent, Seat, SwimParams};
use postretro_scripting_core::data_descriptors::{
    NamedReaction, PlayerEventEdge, PrimitiveDescriptor, ReactionDescriptor,
};
use postretro_scripting_core::player_slots::{PlayerSlot, player_slot_value};
use serde_json::json;

use super::tests::{World, input, on_player, player_event};
use crate::collision::CollisionWorld;
use crate::movement::MovementInput;
use crate::sim::run_host_movement_tick;

const DT: f32 = 1.0 / 60.0;
const LEVEL_GRAVITY: f32 = -20.0;
const WATER_VISCOSITY: f32 = 2.0;
const LAST_IMMERSION: &str = "swim.lastImmersion";
const ENTRY_DAMAGE: f32 = 5.0;
const EXIT_DAMAGE: f32 = 1.0;
/// Standing on the floor, a pawn is centred here (capsule reach 1.2 m).
const STAND_Y: f32 = 1.21;
const DRY: Vec3 = Vec3::new(0.0, STAND_Y, 0.0);
/// Inside the pool: x in [20, 40], surface at y = 1.9, so a standing pawn's
/// capsule is about 0.79 immersed.
const IN_POOL: Vec3 = Vec3::new(30.0, STAND_Y, 0.0);

/// A floor at y = 0 spanning x, z in [-200, 200].
fn floor_world() -> CollisionWorld {
    let points = vec![
        Vec3::new(-200.0, 0.0, -200.0),
        Vec3::new(200.0, 0.0, -200.0),
        Vec3::new(200.0, 0.0, 200.0),
        Vec3::new(-200.0, 0.0, 200.0),
    ];
    CollisionWorld::from_triangles_for_test(points, vec![[0, 2, 1], [0, 3, 2]])
}

fn pool() -> EnvironmentVolumes {
    let (min, max) = (Vec3::new(20.0, -1.0, -50.0), Vec3::new(40.0, 1.9, 50.0));
    let plane = |normal: Vec3, distance: f32| LeafPlane { normal, distance };
    EnvironmentVolumes::build(vec![EnvironmentVolume {
        source_entity: 1,
        priority: 0,
        gravity: None,
        push: None,
        fluid: Some(0),
        brushes: vec![ConvexLeaf {
            planes: vec![
                plane(Vec3::X, max.x),
                plane(Vec3::NEG_X, -min.x),
                plane(Vec3::Y, max.y),
                plane(Vec3::NEG_Y, -min.y),
                plane(Vec3::Z, max.z),
                plane(Vec3::NEG_Z, -min.z),
            ],
            aabb_min: min,
            aabb_max: max,
        }],
    }])
}

fn idle() -> MovementInput {
    MovementInput {
        wish_dir: Vec2::ZERO,
        jump_pressed: false,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
        facing_pitch: 0.0,
        use_pressed: false,
        drop_pressed: false,
    }
}

/// Two seat-bound swimmers on dry floor, with a shared slot a fired reaction
/// records `byPlayer(on.player)` immersion into.
struct Pool {
    world: World,
    collision: CollisionWorld,
    volumes: EnvironmentVolumes,
    pawns: [EntityId; 2],
}

impl Pool {
    fn new() -> Self {
        let world = World::new();
        let pawns = [(Seat(1), 0.0), (Seat(2), 3.0)].map(|(seat, z)| {
            let pawn = world.spawn_player(Some(seat), 100.0);
            let mut movement = crate::trigger_bindings::group_tick_tests::movement();
            movement.swim = Some(SwimParams {
                speed: 4.5,
                accel: 6.0,
                sink_speed: 0.8,
                enter_depth: 0.5,
                exit_depth: 0.35,
                fluid_jump_velocity: Some(6.5),
            });
            let mut registry = world.script_ctx.registry.borrow_mut();
            registry.set_component(pawn, movement).unwrap();
            registry
                .set_component(
                    pawn,
                    Transform {
                        position: DRY + Vec3::Z * z,
                        ..Transform::default()
                    },
                )
                .unwrap();
            pawn
        });
        world
            .script_ctx
            .slot_table
            .borrow_mut()
            .insert(
                LAST_IMMERSION.to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(-1.0)),
                    range: None,
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: ReplicationScope::SharedGlobal,
                    per_owner: false,
                    accumulate: None,
                }),
            )
            .unwrap();
        Self {
            world,
            collision: floor_world(),
            volumes: pool(),
            pawns,
        }
    }

    /// `players().on(becomes(read(player.swimming)), [entered, recordImmersion])`
    /// and `players().on(ceases(read(player.swimming)), [left])`, installed into
    /// a fresh table (so with no edge memory).
    fn install_events(&mut self) {
        let record_immersion = NamedReaction {
            name: "recordImmersion".to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "setState".to_string(),
                target: None,
                kind: None,
                tag: None,
                on_complete: None,
                args: json!({
                    "slot": LAST_IMMERSION,
                    "value": { "op": "input", "name": "player.immersion", "owner": "@player" },
                }),
            }),
        };
        let swimming: IrNode = input("player.swimming");
        self.world.install(
            vec![
                on_player("entered", "applyDamage", json!({ "amount": ENTRY_DAMAGE })),
                on_player("left", "applyDamage", json!({ "amount": EXIT_DAMAGE })),
                record_immersion,
            ],
            vec![
                player_event(
                    PlayerEventEdge::Becomes,
                    swimming.clone(),
                    &["entered", "recordImmersion"],
                ),
                player_event(PlayerEventEdge::Ceases, swimming, &["left"]),
            ],
        );
    }

    fn place(&self, pawn: EntityId, position: Vec3) {
        let mut registry = self.world.script_ctx.registry.borrow_mut();
        let mut transform = *registry.get_component::<Transform>(pawn).unwrap();
        transform.position = position;
        registry.set_component(pawn, transform).unwrap();
    }

    /// One authoritative tick: host movement for both pawns, then the player
    /// events' settled-tick evaluation.
    fn tick(&mut self) {
        let inputs = self.pawns.map(|pawn| (pawn, idle()));
        run_host_movement_tick(
            &mut self.world.script_ctx.registry.borrow_mut(),
            &self.collision,
            EnvironmentView::new(LEVEL_GRAVITY, &self.volumes, &[WATER_VISCOSITY]),
            &inputs,
            DT,
        );
        self.world.tick();
    }

    fn slot(&self, pawn: EntityId, slot: PlayerSlot) -> Option<SlotValue> {
        player_slot_value(&self.world.script_ctx.registry.borrow(), slot, pawn)
    }

    fn immersion_component(&self, pawn: EntityId) -> f32 {
        self.world
            .script_ctx
            .registry
            .borrow()
            .get_component::<ImmersionComponent>(pawn)
            .expect("host movement writes the immersion component")
            .immersion
    }

    fn last_immersion(&self) -> f32 {
        match self
            .world
            .script_ctx
            .slot_table
            .borrow()
            .get(LAST_IMMERSION)
            .unwrap()
            .value
        {
            Some(SlotValue::Number(value)) => value,
            ref other => panic!("{LAST_IMMERSION} is {other:?}"),
        }
    }

    fn health(&self) -> [f32; 2] {
        self.pawns.map(|pawn| self.world.health(pawn))
    }
}

// Host side: the swim slots read false and 0 for a dry pawn, and true and
// the tick's resolved immersion fraction for a swimming one, per pawn.
#[test]
fn swim_slots_read_each_pawns_dry_and_swimming_values_from_host_movement() {
    let mut pool = Pool::new();
    let [swimmer, walker] = pool.pawns;
    pool.place(swimmer, IN_POOL);
    for _ in 0..3 {
        pool.tick();
    }

    let swimming_movement = pool
        .world
        .script_ctx
        .registry
        .borrow()
        .get_component::<PlayerMovementComponent>(swimmer)
        .unwrap()
        .movement_state
        .kind();
    assert_eq!(
        swimming_movement,
        postretro_foundation::MovementStateKind::Swim
    );
    let immersion = pool.immersion_component(swimmer);
    assert!(
        immersion > 0.5 && immersion < 1.0,
        "a fraction of the capsule is under: {immersion}"
    );
    assert_eq!(
        pool.slot(swimmer, PlayerSlot::Swimming),
        Some(SlotValue::Boolean(true))
    );
    assert_eq!(
        pool.slot(swimmer, PlayerSlot::Immersion),
        Some(SlotValue::Number(immersion))
    );
    assert_eq!(
        pool.slot(walker, PlayerSlot::Swimming),
        Some(SlotValue::Boolean(false))
    );
    assert_eq!(
        pool.slot(walker, PlayerSlot::Immersion),
        Some(SlotValue::Number(0.0))
    );
}

// Through `byPlayer(on.player)`: `becomes(read(player.swimming))`
// fires once for the player who enters swim, naming them as `on.player` and
// reading their own immersion; `ceases` fires once when they leave; the other
// player's later entry fires once more, for them; and a player first observed
// already swimming fires `becomes` once.
#[test]
fn swim_player_events_fire_once_per_player_per_edge_and_read_that_players_immersion() {
    let mut pool = Pool::new();
    let [first, second] = pool.pawns;
    pool.install_events();
    for _ in 0..3 {
        pool.tick();
    }
    assert_eq!(pool.health(), [100.0, 100.0], "nobody swims yet");

    pool.place(first, IN_POOL);
    pool.tick();
    assert_eq!(
        pool.health(),
        [100.0 - ENTRY_DAMAGE, 100.0],
        "`becomes` fires for the entering player, naming them"
    );
    let first_immersion = pool.immersion_component(first);
    assert!(first_immersion > 0.5, "{first_immersion}");
    assert_eq!(
        pool.last_immersion(),
        first_immersion,
        "byPlayer(on.player) reads the entering player's immersion"
    );
    for _ in 0..5 {
        pool.tick();
    }
    assert_eq!(
        pool.health(),
        [100.0 - ENTRY_DAMAGE, 100.0],
        "a held swim fires nothing more"
    );

    pool.place(first, DRY);
    pool.tick();
    pool.tick();
    assert_eq!(
        pool.health(),
        [100.0 - ENTRY_DAMAGE - EXIT_DAMAGE, 100.0],
        "`ceases` fires once when they leave"
    );

    pool.place(second, IN_POOL + Vec3::Z * 3.0);
    pool.tick();
    let second_immersion = pool.immersion_component(second);
    assert_eq!(
        pool.health(),
        [100.0 - ENTRY_DAMAGE - EXIT_DAMAGE, 100.0 - ENTRY_DAMAGE],
        "the other player's entry fires once more, for them"
    );
    assert_eq!(pool.last_immersion(), second_immersion);

    // A fresh table has no edge memory: the swimming player is first observed
    // swimming and fires `becomes` once; the dry one fires nothing.
    pool.install_events();
    pool.tick();
    pool.tick();
    assert_eq!(
        pool.health(),
        [
            100.0 - ENTRY_DAMAGE - EXIT_DAMAGE,
            100.0 - 2.0 * ENTRY_DAMAGE
        ],
        "first sight of a swimming player fires `becomes` once"
    );
}
