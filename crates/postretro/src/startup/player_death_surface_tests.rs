//! Proves the authored player death policy end to end: both script runtimes emit
//! identical policy and down-vignette wire, dev registration and the HUD consume it,
//! and a two-seat production tick loop bleeds out, respawns and resets clocks.
//!
//! See: context/lib/scripting.md §12 Reaction Dispatch Model (player life states, respawn)
use super::teleporters_surface_tests::{canonical, workspace};
use crate::startup::lifecycle::tests::{script_ctx, test_app};
use glam::{Vec2, Vec3};
use postretro_entities::components::{
    health::{HealthComponent, set_health_absolute},
    inventory::Inventory,
};
use postretro_entities::{
    EntityId, EntityTypeDescriptor, RespawnPlacement, ScriptCtx, ScriptError, Transform,
};
use postretro_foundation::{
    ActivationInput, Capability, NO_TELEPORTER, PlayerMovementComponent, Seat, player_capability,
};
use postretro_level_format::data_script::DataScriptSection;
use postretro_scripting_core::{
    data_descriptors::LevelManifest,
    primitive_adapters::StoreSchemaJson,
    primitives_registry::{ContextScope, PrimitiveRegistry},
    runtime::{ScriptRuntime, ScriptRuntimeConfig},
};
use postretro_sim::{
    ai_host::AiTickResult,
    player_events::PlayerEventTable,
    sim::{
        PostMovementCommand, SimCommand, TickEvents, TriggerTickContext,
        simulate_tick_with_presentation_aim,
    },
    trigger_bindings::TriggerBindingTable,
    trigger_system::TriggerSystem,
};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc};

const DT: f32 = 1.0 / 60.0;
const JS_CAPTURE: &str = "\n;(function(){const original=globalThis.setupLevel;globalThis.setupLevel=function(ctx){const result=original(ctx);__lifePolicyCapture(result);return result;};})();\n";
const LUA_CAPTURE: &str = "\ndo local original=setupLevel setupLevel=function(ctx) local result=original(ctx) __lifePolicyCapture(result) return result end end\n";

fn script(name: &str) -> PathBuf {
    workspace().join("content/dev/scripts").join(name)
}
fn load(
    runtime: &ScriptRuntime,
    capture: &RefCell<Option<Value>>,
    name: &str,
) -> (LevelManifest, Value) {
    let path = script(name);
    let luau = name.ends_with(".luau");
    let mut source = if luau {
        std::fs::read_to_string(&path).unwrap()
    } else {
        postretro_script_compiler::bundle_entry(&path).expect("authored death policy bundles")
    };
    source.push_str(if luau { LUA_CAPTURE } else { JS_CAPTURE });
    let manifest = runtime.run_data_script(
        &DataScriptSection {
            compiled_bytes: source.into_bytes(),
            source_path: path.to_string_lossy().into_owned(),
        },
        path.parent().unwrap(),
    );
    (
        manifest,
        canonical(
            capture
                .borrow_mut()
                .take()
                .expect("setupLevel returns the authored policy"),
        ),
    )
}
fn twins() -> (LevelManifest, Value, LevelManifest, Value) {
    let ctx = ScriptCtx::new();
    let capture = Rc::new(RefCell::new(None));
    let mut primitives = PrimitiveRegistry::new();
    crate::scripting::primitives::register_all(&mut primitives, ctx.clone());
    primitives
        .register("__lifePolicyCapture", {
            let capture = capture.clone();
            move |value: StoreSchemaJson| -> Result<(), ScriptError> {
                *capture.borrow_mut() = Some(value.0);
                Ok(())
            }
        })
        .scope(ContextScope::Both)
        .doc("test capture of authored death policy")
        .param("value", "unknown")
        .finish();
    let runtime = ScriptRuntime::new(&primitives, &ScriptRuntimeConfig::default(), &ctx).unwrap();
    let (ts, ts_wire) = load(&runtime, &capture, "player-death.ts");
    let (lua, lua_wire) = load(&runtime, &capture, "player-death.luau");
    (ts, ts_wire, lua, lua_wire)
}
fn contains(value: &Value, expected: &Value) -> bool {
    value == expected
        || match value {
            Value::Object(map) => map.values().any(|v| contains(v, expected)),
            Value::Array(items) => items.iter().any(|v| contains(v, expected)),
            _ => false,
        }
}

#[test]
fn player_death_twins_emit_identical_policy_and_down_vignette_wire() {
    let (ts, wire, lua, lua_wire) = twins();
    assert_eq!(
        serde_json::to_vec(&wire).unwrap(),
        serde_json::to_vec(&lua_wire).unwrap()
    );
    assert_eq!(ts.reactions, lua.reactions);
    assert_eq!(ts.player_events, lua.player_events);
    assert_eq!(ts.crossings, lua.crossings);
    assert_eq!(
        (
            ts.reactions.len(),
            ts.player_events.len(),
            ts.crossings.len()
        ),
        (4, 2, 2)
    );
    let reaction = |name: &str| {
        wire["reactions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()
    };
    assert_eq!(reaction("life.down")["primitive"], "setLifeState");
    assert_eq!(reaction("life.down")["target"], "@player");
    assert_eq!(reaction("life.down")["args"]["state"], "down");
    assert_eq!(reaction("life.respawn")["primitive"], "respawn");
    assert_eq!(reaction("life.respawn")["target"], "@player");
    assert!(contains(
        &wire["playerEvents"][0],
        &json!({"op":"le","a":{"op":"input","name":"player.health"},"b":{"op":"const","value":0}})
    ));
    assert!(contains(
        &wire["playerEvents"][1],
        &json!({"op":"input","name":"player.inLifeState:down"})
    ));
    assert!(contains(
        &wire["playerEvents"][1],
        &json!({"op":"ge","a":{"op":"input","name":"player.lifeStateMs"},"b":{"op":"const","value":15000}})
    ));
    assert_eq!(reaction("life.downedVignette")["primitive"], "vignette");
    assert_eq!(reaction("life.downedVignette")["args"]["durationMs"], 15000);
    assert_eq!(reaction("life.clearVignette")["args"]["strength"], 0);
    assert!(contains(
        &wire["crossings"],
        &json!({"op":"input","name":"player.inLifeState:down"})
    ));
    // Equal wire can still share an invalid payload (for example a zero-duration
    // clear vignette). Exercise the production system-reaction handler as well.
    let mut system = postretro_scripting_core::reaction_registry::SystemReactionRegistry::new();
    postretro_sim::scripting_systems::system_reactions::register_system_reaction_primitives(
        &mut system,
    );
    for manifest in [&ts, &lua] {
        let queue = postretro_entities::SystemCommandQueue::new();
        for reaction in &manifest.reactions {
            if let postretro_entities::ReactionDescriptor::Primitive(primitive) =
                &reaction.descriptor
                && primitive.primitive == "vignette"
            {
                assert!(
                    system
                        .dispatch(&primitive.primitive, &primitive.args, &queue)
                        .expect("authored vignette must be valid at runtime")
                );
            }
        }
        let commands = queue.take();
        assert_eq!(commands.len(), 2);
        assert!(commands.iter().all(|command| matches!(command,
            postretro_entities::SystemReactionCommand::Vignette { duration_ms, .. } if *duration_ms > 0.0)));
    }
}

fn dev_mod() -> (
    crate::App,
    ScriptCtx,
    postretro_scripting_core::runtime::ModManifestResult,
) {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    let mut primitives = PrimitiveRegistry::new();
    crate::scripting::primitives::register_all(&mut primitives, ctx.clone());
    let manifest = crate::coop_fixture_tests::initialize_dev_mod(&mut app, &primitives);
    ctx.data_registry.borrow_mut().recompose(&[]);
    (app, ctx, manifest)
}

#[test]
fn player_death_dev_registration_and_hud_consume_the_authored_policy() {
    let (ts, _, _, _) = twins();
    let (_app, ctx, manifest) = dev_mod();
    for event in &ts.player_events {
        assert!(
            manifest.player_events.contains(event),
            "dev mod registers each authored death edge"
        );
    }
    for reaction in &ts.reactions {
        assert!(
            manifest
                .reactions
                .iter()
                .any(|scoped| scoped.reaction == *reaction),
            "dev mod registers each authored death reaction"
        );
    }
    let data = ctx.data_registry.borrow();
    let player = data
        .entities
        .iter()
        .find(|e| e.canonical_name.as_deref() == Some("player"))
        .unwrap();
    let states = player.life_states.as_ref().unwrap();
    assert_eq!(states.initial, "alive");
    assert!(states.rejoin.is_empty());
    let down = &states.states["down"];
    assert_eq!(down.movement.speed_scale, 0.3);
    assert!(!down.movement.abilities);
    assert!(down.fire && down.look && down.reload && down.damageable);
    assert!(!down.switch_weapon && !down.use_enabled && !down.touch && !down.occupancy);
    let hud = manifest
        .ui_trees
        .iter()
        .find(|tree| tree.name == "hud.bleedOut")
        .expect("bleed-out HUD is registered");
    assert!(hud.always_on);
    let hud = serde_json::to_value(&hud.tree).unwrap();
    assert!(
        contains(&hud, &json!({"slot":"player.lifeState","equals":"down"})),
        "visibility follows own state name"
    );
    fn bar(value: &Value) -> Option<&Value> {
        if value.get("kind") == Some(&json!("bar")) {
            Some(value)
        } else {
            match value {
                Value::Object(map) => map.values().find_map(bar),
                Value::Array(items) => items.iter().find_map(bar),
                _ => None,
            }
        }
    }
    let meter = bar(&hud).expect("bleed-out bar exists");
    assert_eq!(meter["bind"]["slot"], "player.lifeStateMs");
    assert_eq!(meter["max"].as_f64(), Some(15000.0));
}

struct Consumer {
    _app: crate::App,
    ctx: ScriptCtx,
    world: crate::collision::CollisionWorld,
    descriptors: Vec<EntityTypeDescriptor>,
    table: PlayerEventTable,
    host: EntityId,
    other: EntityId,
    next_tick: u32,
}
fn command() -> SimCommand {
    SimCommand {
        input_tick: 0,
        movement: postretro_physics::movement::MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            facing_pitch: 0.0,
            use_pressed: false,
            drop_pressed: false,
        },
        fire_button: postretro_sim::weapon::FireButtonState {
            pressed: false,
            active: false,
        },
        secondary_button: postretro_sim::weapon::FireButtonState {
            pressed: false,
            active: false,
        },
        activation: ActivationInput::default(),
        reload: false,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    }
}
impl Consumer {
    fn new() -> Self {
        let (mut app, ctx, _) = dev_mod();
        ctx.registry.borrow_mut().clear_for_level_unload();
        let descriptors = ctx.data_registry.borrow().entities.clone();
        let placement = |x| postretro_sim::scripting::map_entity::MapEntity {
            map_ordinal: None,
            classname: "player_spawn".into(),
            origin: Vec3::X * x,
            angles: Vec3::ZERO,
            key_values: HashMap::new(),
            tags: Vec::new(),
        };
        let (host, other) = {
            let mut registry = ctx.registry.borrow_mut();
            let host=postretro_sim::scripting::builtins::data_archetype::spawn_from_player_starts_with_carried_loadout(&[placement(0.0)],&descriptors,&mut registry,None,None).unwrap().pawn;
            let other=postretro_sim::scripting::builtins::net_descriptor::spawn_net_slot_pawn_with_carried_loadout(&placement(50.0),&descriptors,&mut registry,None,None).unwrap();
            let mut seats = crate::netcode::SeatTable::local_only();
            let remote_seat = seats.admit_or_reclaim(41, None, false).unwrap().seat;
            assert_eq!(remote_seat, Seat(1));
            seats.bind_pawn(&mut registry, Seat(0), host);
            seats.bind_pawn(&mut registry, remote_seat, other);
            app.session.as_mut().unwrap().seat_table = Some(seats);
            (host, other)
        };
        {
            let mut placements = ctx.respawn_placements.borrow_mut();
            placements.first_player = Some(0);
            placements.seats = [(Seat(0), 0), (Seat(1), 1)].into();
            placements.map_order = vec![
                RespawnPlacement {
                    map_index: 0,
                    foot_anchor: Vec3::ZERO,
                    yaw: 0.0,
                },
                RespawnPlacement {
                    map_index: 1,
                    foot_anchor: Vec3::X * 50.0,
                    yaw: 0.0,
                },
            ];
        }
        let world = crate::collision::CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(-1000.0, 0.0, -1000.0),
                Vec3::new(1000.0, 0.0, -1000.0),
                Vec3::new(1000.0, 0.0, 1000.0),
                Vec3::new(-1000.0, 0.0, 1000.0),
            ],
            vec![[0, 2, 1], [0, 3, 2]],
        );
        let table = PlayerEventTable::build(&ctx, Default::default(), Default::default(), None);
        assert!(!table.is_empty());
        Self {
            _app: app,
            ctx,
            world,
            descriptors,
            table,
            host,
            other,
            next_tick: 0,
        }
    }
    fn movement(&self, pawn: EntityId) -> PlayerMovementComponent {
        self.ctx
            .registry
            .borrow()
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone()
    }
    fn pose(&self, pawn: EntityId) -> Transform {
        *self
            .ctx
            .registry
            .borrow()
            .get_component::<Transform>(pawn)
            .unwrap()
    }
    fn name(&self, pawn: EntityId) -> String {
        postretro_sim::sim::player_life::life_state_name(
            &self.ctx.registry.borrow(),
            pawn,
            &self.ctx.data_registry.borrow().life_state_names,
        )
        .unwrap()
    }
    fn health(&self, pawn: EntityId) -> f32 {
        self.ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(pawn)
            .unwrap()
            .current
    }
    fn tick(&mut self, mut command: SimCommand) -> TickEvents {
        command.input_tick = self.next_tick;
        self.next_tick += 1;
        let mut triggers = TriggerSystem::default();
        let bridge =
            postretro_sim::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge::new();
        let bindings = TriggerBindingTable::default();
        let edges = HashMap::new();
        let host = self.host;
        let events = simulate_tick_with_presentation_aim(
            self.ctx.registry.clone(),
            &self.world,
            &postretro_sim::scripting_systems::hit_zones::HitZoneStore::new(),
            None,
            postretro_collision::EnvironmentView::level_only(-9.81),
            false,
            0.0,
            (0.0, 0.0),
            &mut Default::default(),
            |_, _, _, _| AiTickResult {
                events: Vec::new(),
                projectile_spawns: Vec::new(),
            },
            &[],
            &mut Default::default(),
            &[],
            &command,
            |registry| {
                let registry = registry.borrow();
                let movement = registry
                    .get_component::<PlayerMovementComponent>(host)
                    .unwrap();
                let transform = registry.get_component::<Transform>(host).unwrap();
                PostMovementCommand {
                    aim_origin: transform.position + Vec3::Y * movement.capsule.eye_height,
                    aim_direction: Vec3::NEG_Z,
                }
            },
            DT,
            &mut Default::default(),
            &self.descriptors,
            0,
            &Default::default(),
            &RefCell::new(Default::default()),
            None,
            &edges,
            &edges,
            Some(TriggerTickContext {
                teleporters: None,
                system: &mut triggers,
                bridge: &bridge,
                bindings: &bindings,
                slot_table: self.ctx.slot_table.clone(),
                script_ctx: Some(self.ctx.clone()),
                auto_close_timers: None,
                use_edges: &edges,
            }),
            |_, _| {},
            |_| {},
            postretro_sim::sim::TimingGate::OFF,
        );
        let mut residuals = Vec::new();
        self.table.run_tick(&self.ctx, &mut residuals);
        assert!(
            residuals.is_empty(),
            "the down/respawn policy is fixed-tick work"
        );
        events
    }
}

#[test]
fn player_death_two_player_policy_bleeds_out_on_tick_900_then_respawns_next_tick() {
    let mut h = Consumer::new();
    for pawn in [h.host, h.other] {
        assert_eq!(h.name(pawn), "alive");
        assert_eq!(h.movement(pawn).life_state_elapsed_ticks, 0);
        assert_eq!(h.health(pawn), 100.0);
    }
    for _ in 0..30 {
        h.tick(command());
    }
    let other_pose = h.pose(h.other);
    let other_inventory = h
        .ctx
        .registry
        .borrow()
        .get_component::<Inventory>(h.other)
        .unwrap()
        .clone();
    let other_elapsed = h.movement(h.other).life_state_elapsed_ticks;
    let generation = h.movement(h.host).teleport.generation;
    set_health_absolute(&mut h.ctx.registry.borrow_mut(), h.host, 0.0);
    let events = h.tick(command());
    assert_eq!(
        events
            .death
            .iter()
            .filter(|event| event.as_str() == "playerDied")
            .count(),
        1
    );
    assert_eq!(h.name(h.host), "down");
    assert_eq!(h.movement(h.host).life_state_elapsed_ticks, 0);
    assert_eq!(h.health(h.other), 100.0);
    let down_start = h.pose(h.host).position;
    let mut fired = false;
    for elapsed in 1..=900 {
        let mut input = command();
        input.movement.wish_dir = Vec2::Y;
        input.movement.running = true;
        input.fire_button = postretro_sim::weapon::FireButtonState {
            pressed: elapsed == 1,
            active: elapsed == 1,
        };
        if elapsed == 1 {
            input.select_slot = Some(1);
        }
        let events = h.tick(input);
        fired |= events
            .weapon
            .iter()
            .any(|event| event.address == "activate");
        assert!(
            events.death.is_empty(),
            "latched zero health does not repeat the legacy event"
        );
        let movement = h.movement(h.host);
        assert_eq!(h.name(h.host), "down");
        assert_eq!(movement.life_state_elapsed_ticks, elapsed);
        assert_eq!(movement.teleport.generation, generation);
        assert_eq!(h.health(h.host), 0.0);
        for capability in [
            Capability::SwitchWeapon,
            Capability::Use,
            Capability::Touch,
            Capability::Occupancy,
        ] {
            assert_eq!(player_capability(&movement, capability), Some(false));
        }
        assert_eq!(player_capability(&movement, Capability::Fire), Some(true));
        if elapsed == 40 {
            let speed = movement.velocity.x.hypot(movement.velocity.z);
            assert!(
                speed > 0.0 && speed <= movement.ground_params.speed.crouch * 0.3 + 0.01,
                "down crawls at its authored target: {speed}"
            );
        }
        assert_eq!(
            h.ctx
                .registry
                .borrow()
                .get_component::<Inventory>(h.host)
                .unwrap()
                .active_slot,
            0,
            "down refuses the attempted weapon switch"
        );
        assert_eq!(
            h.ctx.pending_respawns.borrow().len(),
            usize::from(elapsed == 900),
            "15000ms first holds at exactly 900 whole ticks"
        );
        assert_eq!(h.name(h.other), "alive");
        assert_eq!(h.health(h.other), 100.0);
        assert_eq!(h.pose(h.other), other_pose);
        assert_eq!(
            h.movement(h.other).life_state_elapsed_ticks,
            other_elapsed + elapsed + 1
        );
        assert_eq!(
            h.ctx
                .registry
                .borrow()
                .get_component::<Inventory>(h.other)
                .unwrap()
                .wieldables,
            other_inventory.wieldables
        );
    }
    assert!(fired, "the actual down profile permits a shot at zero HP");
    assert!((h.pose(h.host).position - down_start).length() > 1.0);
    let death_spot = h.pose(h.host).position;
    h.tick(command());
    let movement = h.movement(h.host);
    assert_eq!(h.name(h.host), "alive");
    assert_eq!(h.health(h.host), 100.0);
    assert_eq!(movement.life_state_elapsed_ticks, 1);
    assert_eq!(
        (
            movement.teleport.generation,
            movement.teleport.last_teleporter
        ),
        (generation.wrapping_add(1), NO_TELEPORTER)
    );
    assert!(h.ctx.pending_respawns.borrow().is_empty());
    assert!((h.pose(h.host).position - death_spot).length() > 1.0);
    assert_eq!(h.name(h.other), "alive");
    assert_eq!(h.health(h.other), 100.0);
    assert_eq!(h.pose(h.other), other_pose);
}

#[test]
fn player_death_restart_unload_and_materialization_reset_both_initial_clocks() {
    let mut h = Consumer::new();
    {
        let mut registry = h.ctx.registry.borrow_mut();
        for pawn in [h.host, h.other] {
            set_health_absolute(&mut registry, pawn, 0.0);
        }
    }
    h.tick(command());
    h.tick(command());
    for pawn in [h.host, h.other] {
        assert_eq!(h.name(pawn), "down");
        assert_eq!(h.movement(pawn).life_state_elapsed_ticks, 1);
    }
    h._app.unload_level();
    assert!(!h.ctx.registry.borrow().exists(h.host));
    assert!(!h.ctx.registry.borrow().exists(h.other));
    let seats = h
        ._app
        .session
        .as_ref()
        .unwrap()
        .seat_table
        .as_ref()
        .unwrap();
    let carried = [
        seats.carried_state(Seat(0)).unwrap().clone(),
        seats.carried_state(Seat(1)).unwrap().clone(),
    ];
    assert!(carried.iter().all(|state| state.life_state.is_none()));
    let descriptors = h.ctx.data_registry.borrow().entities.clone();
    let placement = |x| postretro_sim::scripting::map_entity::MapEntity {
        map_ordinal: None,
        classname: "player_spawn".into(),
        origin: Vec3::X * x,
        angles: Vec3::ZERO,
        key_values: HashMap::new(),
        tags: Vec::new(),
    };
    let (host, other) = {
        let mut registry = h.ctx.registry.borrow_mut();
        let host = postretro_sim::scripting::builtins::data_archetype::spawn_from_player_starts_with_carried_loadout(
            &[placement(0.0)],&descriptors,&mut registry,None,Some(&carried[0])).unwrap().pawn;
        let other = postretro_sim::scripting::builtins::net_descriptor::spawn_net_slot_pawn_with_carried_loadout(
            &placement(50.0),&descriptors,&mut registry,None,Some(&carried[1])).unwrap();
        (host, other)
    };
    for pawn in [host, other] {
        assert_eq!(h.name(pawn), "alive");
        assert_eq!(h.movement(pawn).life_state_elapsed_ticks, 0);
        assert_eq!(h.health(pawn), 100.0);
    }
}
