// The closet-reveal level script — the SDK addressing surface example — run
// end to end: the shipped `content/dev/scripts/closet-reveal.{ts,luau}`
// evaluated by the production script runtime over a registry shaped like
// `closet-reveal.map`, installed in the level-install order (sequence
// validation, Pass A, Pass B, binder, manifest events, V5), then driven
// through the session frame order (trigger tick, Exit cancels, residual drain
// under the fire's origin, deferred follow-ups, landings).
// See: context/lib/scripting.md §12 (Entity addressing)

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use glam::Vec3;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::light::{FalloffKind, LightComponent, LightKind};
use postretro_entities::components::spawner::SpawnerComponent;
use postretro_entities::data_descriptors::HealthDescriptor;
use postretro_entities::{
    AmmoReserve, ComponentKind, EntityId, KinematicMoverComponent, KinematicMoverConfig,
    KinematicMoverMode, MoverCommand, ReactionDescriptor, ReplicationScope, ScriptCtx, ScriptError,
    SequenceTarget, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue, Transform,
    TriggerActivation, TriggerFireMode, TriggerVolumeComponent,
};
use postretro_foundation::Seat;
use postretro_level_format::data_script::DataScriptSection;
use postretro_net::wire::{ConnectClaim, PlayerClaimId};
use postretro_scripting_core::data_descriptors::LevelManifest;
use postretro_scripting_core::primitive_adapters::StoreSchemaJson;
use postretro_scripting_core::primitives_registry::{ContextScope, PrimitiveRegistry};
use postretro_scripting_core::reaction_dispatch::{
    ResidualOrigin, dispatch_deferred_named_events_with_sequences,
    fire_prepartitioned_reactions_with_sequences, validate_sequence_primitives,
};
use postretro_scripting_core::runtime::{ScriptRuntime, ScriptRuntimeConfig};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_test_log_capture::LogCapture;
use serde_json::Value;

use crate::netcode::SeatTable;
use crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor;
use crate::scripting::primitives::light::register_sequenced_light_primitives;
use crate::scripting::primitives::register_all;
use crate::scripting::reactions::registry::{
    ReactionPrimitiveRegistry, register_emitter_reaction_primitives,
    register_fog_reaction_primitives, register_grant_reactions, register_mover_reaction_primitives,
    register_npc_state_reaction_primitives, register_sequenced_fog_primitives,
    register_sequenced_mover_primitives, register_sequenced_spawner_primitives,
    register_sequenced_trigger_primitives, register_spawner_reaction_primitives,
    register_trigger_reaction_primitives,
};
use crate::scripting::reactions::system_commands::{
    SystemReactionRegistry, register_system_reaction_primitives,
};
use crate::scripting_systems::reaction_scheduler::{
    ReactionScheduler, register_reaction_control_primitives,
};
use crate::scripting_systems::system_reactions::SystemReactionIrBindings;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::spawner::SpawnContext;
use crate::startup::reaction_validation::{
    derive_interruptible_wait_exit_edges, validate_reaction_bodies_pass_a,
    validate_trigger_coupled_pass_b,
};
use crate::trigger_bindings::TriggerBindingTable;
use crate::trigger_commands::TriggerFireContext;
use crate::trigger_system::{
    AuthoritativePlayer, PlayerId, TriggerDispatchInputs, TriggerEventEdge, TriggerSystem,
    TriggerTickInputs,
};

const ARCHETYPE: &str = "reference_enemy";
const SPAWN_COUNT: u32 = 2;
const NPC_HEALTH: f32 = 100.0;
const AMMO_POOL: &str = "shells.buck";
const PLATE_CENTER: Vec3 = Vec3::new(0.0, 0.0, 0.0);
const ON_PLATE: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const OFF_PLATE: Vec3 = Vec3::new(30.0, 1.0, 30.0);
/// Comfortably past the 800 ms wait at the 60 Hz tick.
const FRAMES_PAST_THE_WAIT: usize = 60;

/// Wraps `setupLevel` so the raw value it returns — before Rust-side parsing —
/// reaches `__closetRevealCapture`.
const JS_CAPTURE: &str = "\n;(function () {\n  const original = globalThis.setupLevel;\n  globalThis.setupLevel = function (ctx) {\n    const result = original(ctx);\n    __closetRevealCapture(result);\n    return result;\n  };\n})();\n";
const LUAU_CAPTURE: &str = "\ndo\n  local original = setupLevel\n  setupLevel = function(ctx)\n    local result = original(ctx)\n    __closetRevealCapture(result)\n    return result\n  end\nend\n";

fn dev_script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/scripts")
        .join(name)
}

/// The map members and residents of `closet-reveal.map`, plus two seat-bound
/// players: the host's local pawn and a joined remote.
struct Level {
    ctx: ScriptCtx,
    bridge: TriggerVolumeBridge,
    plate: EntityId,
    door: EntityId,
    spawner: EntityId,
    /// Map-placed closet NPCs; they start with aggro disarmed.
    residents: Vec<EntityId>,
    local: AuthoritativePlayer,
    remote: AuthoritativePlayer,
    /// The two alarm lights, west one first.
    lights: [EntityId; 2],
}

impl Level {
    fn new() -> Self {
        let ctx = ScriptCtx::new();
        let mut bridge = TriggerVolumeBridge::new();
        let mut seats = SeatTable::local_only();
        let (plate, door, spawner, residents, local_pawn, remote_pawn, lights) = {
            let mut registry = ctx.registry.borrow_mut();
            let at = |position: Vec3| Transform {
                position,
                ..Transform::default()
            };

            let plate = registry
                .try_spawn(at(PLATE_CENTER), &["closet_reveal_plate".to_string()])
                .unwrap();
            registry
                .set_component(
                    plate,
                    TriggerVolumeComponent::new(
                        TriggerActivation::Touch,
                        String::new(),
                        String::new(),
                        String::new(),
                        MoverCommand::Start,
                        TriggerFireMode::Multiple,
                        0.0,
                        true,
                    ),
                )
                .unwrap();
            bridge.insert_for_test(
                plate,
                PLATE_CENTER + Vec3::new(-1.0, 0.0, -1.0),
                PLATE_CENTER + Vec3::new(1.0, 2.0, 1.0),
            );

            let door = registry
                .try_spawn(at(Vec3::new(5.0, 1.5, 0.0)), &["closet_door".to_string()])
                .unwrap();
            registry
                .set_component(
                    door,
                    KinematicMoverComponent::new(
                        3,
                        KinematicMoverConfig {
                            waypoints: vec![Vec3::new(5.0, 1.5, 0.0), Vec3::new(5.0, -2.0, 0.0)],
                            waypoint_names: vec![
                                "closet_door_closed".to_string(),
                                "closet_door_open".to_string(),
                            ],
                            speed_mps: 1.0,
                            wait_ms: 0.0,
                            mode: KinematicMoverMode::Once,
                            started: false,
                            spin_axis: Vec3::ZERO,
                            initial_spin_rate_rad_s: 0.0,
                            spin_accel_rad_s2: 0.0,
                            carry_yaw: false,
                        },
                    ),
                )
                .unwrap();

            // The spawner's own tag addresses it; its spawns carry `closet`.
            let spawner = registry
                .try_spawn(
                    at(Vec3::new(7.0, 0.5, 0.0)),
                    &["closet_spawner".to_string()],
                )
                .unwrap();
            registry
                .set_component(
                    spawner,
                    SpawnerComponent {
                        archetype_name: ARCHETYPE.to_string(),
                        count: SPAWN_COUNT,
                        spawned_tags: vec!["closet".to_string()],
                        resolved: true,
                    },
                )
                .unwrap();

            let residents: Vec<EntityId> = [Vec3::new(7.0, 0.5, -1.5), Vec3::new(8.0, 0.5, 0.5)]
                .into_iter()
                .map(|position| {
                    let id = registry
                        .try_spawn(at(position), &["enemy".to_string(), "closet".to_string()])
                        .unwrap();
                    registry.set_component(id, full_health()).unwrap();
                    registry.set_component(id, unaggroed_brain()).unwrap();
                    id
                })
                .collect();

            // Spawned west-to-east in reverse, so the script's sort is observable.
            let east = add_alarm_light(&mut registry, Vec3::new(4.0, 3.0, -6.0));
            let west = add_alarm_light(&mut registry, Vec3::new(-4.0, 3.0, -6.0));

            let mut pawn = |position: Vec3| {
                let id = registry.try_spawn(at(position), &[]).unwrap();
                registry.set_component(id, full_health()).unwrap();
                registry.set_component(id, AmmoReserve::new()).unwrap();
                registry
                    .set_component(
                        id,
                        postretro_foundation::PlayerMovementComponent::from_descriptor(
                            &crate::tests::minimal_player_descriptor(),
                        ),
                    )
                    .unwrap();
                id
            };
            let local_pawn = pawn(OFF_PLATE);
            let remote_pawn = pawn(OFF_PLATE * 2.0);
            registry.mark_local_player_pawn(local_pawn).unwrap();
            seats.bind_pawn(&mut registry, Seat(0), local_pawn);
            let remote_seat = seats
                .admit_or_reclaim(
                    7,
                    Some(ConnectClaim {
                        player_id: PlayerClaimId([7; 16]),
                        display_name: "remote".to_string(),
                    }),
                    false,
                )
                .expect("a seat is free")
                .seat;
            seats.bind_pawn(&mut registry, remote_seat, remote_pawn);

            (
                plate,
                door,
                spawner,
                residents,
                local_pawn,
                remote_pawn,
                [west, east],
            )
        };

        // `closet-store.ts`'s slot, which the mod manifest registers.
        ctx.slot_table
            .borrow_mut()
            .insert(
                "closetReveal.alarm".to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(0.0)),
                    range: None,
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: ReplicationScope::SharedGlobal,
                    per_owner: false,
                    accumulate: None,
                }),
            )
            .expect("alarm slot is vacant");

        Self {
            ctx,
            bridge,
            plate,
            door,
            spawner,
            residents,
            local: AuthoritativePlayer {
                id: PlayerId::Local(local_pawn),
                pawn: local_pawn,
            },
            remote: AuthoritativePlayer {
                id: PlayerId::Remote(7),
                pawn: remote_pawn,
            },
            lights,
        }
    }

    fn move_to(&self, pawn: EntityId, position: Vec3) {
        let mut registry = self.ctx.registry.borrow_mut();
        let mut transform = *registry.get_component::<Transform>(pawn).unwrap();
        transform.position = position;
        registry.set_component(pawn, transform).unwrap();
    }

    fn health(&self, id: EntityId) -> f32 {
        self.ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(id)
            .unwrap()
            .current
    }

    fn aggro(&self, id: EntityId) -> bool {
        self.ctx
            .registry
            .borrow()
            .get_component::<BrainComponent>(id)
            .unwrap()
            .aggro_armed
    }

    fn ammo(&self, id: EntityId) -> u32 {
        self.ctx
            .registry
            .borrow()
            .get_component::<AmmoReserve>(id)
            .unwrap()
            .available(AMMO_POOL)
    }

    fn door_started(&self) -> bool {
        self.ctx
            .registry
            .borrow()
            .get_component::<KinematicMoverComponent>(self.door)
            .unwrap()
            .started
    }

    fn brains(&self) -> HashSet<EntityId> {
        self.ctx
            .registry
            .borrow()
            .iter_with_kind(ComponentKind::Brain)
            .map(|(id, _)| id)
            .collect()
    }

    fn position(&self, id: EntityId) -> Vec3 {
        self.ctx
            .registry
            .borrow()
            .get_component::<Transform>(id)
            .unwrap()
            .position
    }

    fn tags(&self, id: EntityId) -> Vec<String> {
        self.ctx.registry.borrow().get_tags(id).unwrap().to_vec()
    }
}

fn full_health() -> HealthComponent {
    HealthComponent::from_descriptor(&HealthDescriptor {
        max: NPC_HEALTH,
        hitbox: None,
        zone_multipliers: HashMap::new(),
    })
}

fn add_alarm_light(registry: &mut postretro_entities::EntityRegistry, position: Vec3) -> EntityId {
    let id = registry
        .try_spawn(
            Transform {
                position,
                ..Transform::default()
            },
            &["closet_alarm".to_string()],
        )
        .unwrap();
    registry
        .set_component(
            id,
            LightComponent {
                origin: position.to_array(),
                light_type: LightKind::Spot,
                intensity: 1.0,
                color: [1.0, 1.0, 1.0],
                falloff_model: FalloffKind::InverseSquared,
                falloff_range: 12.0,
                cone_angle_inner: None,
                cone_angle_outer: None,
                cone_direction: None,
                is_dynamic: true,
                animated_slot: None,
                follow_transform: false,
                carrier: None,
                animation: None,
            },
        )
        .unwrap();
    id
}

/// A map-placed closet NPC: `enabled_on_spawn "false"` holds its aggro until
/// the reveal arms it.
fn unaggroed_brain() -> BrainComponent {
    let descriptor = behavior_enemy_descriptor(ARCHETYPE);
    let mut brain = BrainComponent::from_graph(descriptor.behavior.as_ref().unwrap());
    brain.aggro_armed = false;
    brain
}

/// The production registries `Session::build` assembles, over `ctx`.
struct Scripting {
    runtime: ScriptRuntime,
    captured: Rc<RefCell<Option<Value>>>,
    scheduler: ReactionScheduler,
    spawn_context: SpawnContext,
    sequence_registry: SequencedPrimitiveRegistry,
    reaction_registry: ReactionPrimitiveRegistry,
    system_registry: SystemReactionRegistry,
}

impl Scripting {
    fn new(ctx: &ScriptCtx) -> Self {
        let captured: Rc<RefCell<Option<Value>>> = Rc::new(RefCell::new(None));
        let mut primitives = PrimitiveRegistry::new();
        register_all(&mut primitives, ctx.clone());
        primitives
            .register("__closetRevealCapture", {
                let captured = captured.clone();
                move |value: StoreSchemaJson| -> Result<(), ScriptError> {
                    *captured.borrow_mut() = Some(value.0);
                    Ok(())
                }
            })
            .scope(ContextScope::Both)
            .doc("test capture of the raw setupLevel return")
            .param("value", "unknown")
            .finish();
        let runtime = ScriptRuntime::new(&primitives, &ScriptRuntimeConfig::default(), ctx)
            .expect("script runtime constructs");

        let mut descriptor = behavior_enemy_descriptor(ARCHETYPE);
        descriptor.health = Some(HealthDescriptor {
            max: NPC_HEALTH,
            hitbox: None,
            zone_multipliers: HashMap::new(),
        });
        let spawn_context = SpawnContext::default();
        spawn_context.replace_level_data([(ARCHETYPE.to_string(), descriptor)].into(), None);

        let scheduler = ReactionScheduler::default();
        scheduler.set_enabled(true);
        let diagnostics = crate::mover_commands::MoverCommandDiagnostics::default();
        let auto_close = crate::kinematic_mover::MoverAutoCloseTimers::default();
        let mut sequence_registry = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut sequence_registry, scheduler.clone());
        register_sequenced_light_primitives(&mut sequence_registry, ctx.clone());
        register_sequenced_fog_primitives(&mut sequence_registry, ctx.clone());
        register_sequenced_mover_primitives(
            &mut sequence_registry,
            ctx.clone(),
            diagnostics.clone(),
            auto_close.clone(),
        );
        register_sequenced_trigger_primitives(
            &mut sequence_registry,
            ctx.clone(),
            diagnostics.clone(),
        );
        register_sequenced_spawner_primitives(
            &mut sequence_registry,
            ctx.clone(),
            spawn_context.clone(),
        );
        let mut reaction_registry = ReactionPrimitiveRegistry::new();
        register_emitter_reaction_primitives(&mut reaction_registry);
        register_npc_state_reaction_primitives(&mut reaction_registry);
        register_grant_reactions(&mut reaction_registry);
        register_fog_reaction_primitives(&mut reaction_registry);
        register_mover_reaction_primitives(&mut reaction_registry, diagnostics.clone(), auto_close);
        register_spawner_reaction_primitives(&mut reaction_registry, spawn_context.clone());
        register_trigger_reaction_primitives(&mut reaction_registry, diagnostics);
        let mut system_registry = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system_registry);

        Self {
            runtime,
            captured,
            scheduler,
            spawn_context,
            sequence_registry,
            reaction_registry,
            system_registry,
        }
    }

    /// Run a shipped closet-reveal twin's `setupLevel` the way level load does.
    /// Returns the parsed manifest and the raw returned value as canonical
    /// JSON (see [`canonical`]).
    fn run(&self, script: &str) -> (LevelManifest, String) {
        let path = dev_script(script);
        let is_luau = path.extension().is_some_and(|ext| ext == "luau");
        let mut source = if is_luau {
            std::fs::read_to_string(&path).expect("Luau twin reads")
        } else {
            postretro_script_compiler::bundle_entry(&path).expect("closet-reveal.ts bundles")
        };
        source.push_str(if is_luau { LUAU_CAPTURE } else { JS_CAPTURE });
        let manifest = self.runtime.run_data_script(
            &DataScriptSection {
                compiled_bytes: source.into_bytes(),
                source_path: path.to_string_lossy().into_owned(),
            },
            path.parent().unwrap(),
        );
        let raw = self
            .captured
            .borrow_mut()
            .take()
            .unwrap_or_else(|| panic!("{script}: setupLevel never returned"));
        (manifest, serde_json::to_string(&canonical(raw)).unwrap())
    }
}

/// Keys sorted, and null-valued keys dropped: a Luau table cannot hold `nil`,
/// so an SDK builder's explicit `null` in TS is an absent key in Luau, and the
/// manifest parse reads both the same.
fn canonical(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map
                .into_iter()
                .filter(|(_, value)| !value.is_null())
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonical(value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
        other => other,
    }
}

/// Install `manifest` in the production level-install order
/// (`install_world_cpu`): sequence validation, populate, Pass A, Pass B,
/// binder build, manifest events, V5 Exit-edge derivation.
fn install(
    level: &Level,
    scripting: &Scripting,
    mut manifest: LevelManifest,
) -> TriggerBindingTable {
    let ctx = &level.ctx;
    manifest.reactions =
        validate_sequence_primitives(manifest.reactions, &scripting.sequence_registry);
    ctx.data_registry
        .borrow_mut()
        .populate_level_with_trigger_events(
            manifest.reactions,
            manifest.crossings,
            manifest.trigger_events,
            manifest.trigger_pools,
            &[],
        );
    validate_reaction_bodies_pass_a(ctx);
    let mut system_bindings = SystemReactionIrBindings::default();
    system_bindings.rebuild(&ctx.data_registry.borrow(), ctx);
    validate_trigger_coupled_pass_b(ctx, &system_bindings);
    let mut table = TriggerBindingTable::build_with_script_ctx_and_diagnostics(
        &ctx.registry.borrow(),
        &ctx.data_registry.borrow(),
        ctx,
        Default::default(),
        scripting.spawn_context.clone(),
    );
    table.install_manifest_events(&ctx.registry.borrow(), &ctx.data_registry.borrow(), ctx);
    derive_interruptible_wait_exit_edges(ctx, &mut table);
    table
}

/// One session frame for the two players: the authoritative trigger tick, the
/// scheduler's countdown with this tick's Exit cancels, the residual drain
/// under each fire's origin, the deferred follow-ups, then the landings.
/// Returns the trigger edges that fired.
fn frame(
    level: &Level,
    scripting: &Scripting,
    system: &mut TriggerSystem,
    table: &TriggerBindingTable,
) -> Vec<(EntityId, TriggerEventEdge)> {
    let ctx = &level.ctx;
    let players = [level.local, level.remote];
    let alive: HashSet<PlayerId> = players.iter().map(|player| player.id).collect();
    scripting.scheduler.begin_frame();
    let mut fired = Vec::new();
    let mut residuals = Vec::new();
    let mut exits = Vec::new();
    {
        let mut registry = ctx.registry.borrow_mut();
        system.run_authoritative_tick_with_dispatch(
            &mut registry,
            &level.bridge,
            TriggerTickInputs {
                players: &players,
                use_pressed: &HashMap::new(),
                tick_dt: 1.0 / 60.0,
            },
            TriggerDispatchInputs {
                alive_players: &alive,
                bound_edges: table.bound_edges(),
            },
            |event, occupancy, registry| {
                fired.push((event.fire.trigger, event.edge));
                let activator = players
                    .iter()
                    .find(|player| player.id == event.fire.player)
                    .map(|player| player.pawn);
                let execution = table.execute_with_script_ctx(
                    event.fire.trigger,
                    event.edge,
                    registry,
                    ctx,
                    &TriggerFireContext {
                        fired_trigger: Some(event.fire.trigger),
                        activator,
                        occupancy,
                    },
                );
                if let Some(handle) = execution.residual() {
                    residuals.push((handle, event.fire.trigger, event.fire.player));
                }
                if event.edge == TriggerEventEdge::Exit {
                    exits.push((event.fire.trigger, event.fire.player));
                }
            },
        );
    }
    scripting.scheduler.evaluate(&exits);
    let mut follow_ups = Vec::new();
    for (handle, trigger, player) in residuals {
        let standing = system.paired_enters().contains(&(trigger, player));
        let _origin = scripting.scheduler.begin_origin(trigger, player, standing);
        follow_ups.extend(fire_prepartitioned_reactions_with_sequences(
            table.residual(handle).unwrap().steps(),
            &scripting.sequence_registry,
            &scripting.reaction_registry,
            &scripting.system_registry,
            ctx,
            ResidualOrigin::TriggerBinding,
        ));
    }
    if !follow_ups.is_empty() {
        dispatch_deferred_named_events_with_sequences(
            follow_ups,
            &ctx.data_registry.borrow(),
            &scripting.sequence_registry,
            &scripting.reaction_registry,
            &scripting.system_registry,
            ctx,
        );
    }
    scripting.scheduler.drain_landings(
        &ctx.data_registry.borrow(),
        &scripting.sequence_registry,
        &scripting.reaction_registry,
        &scripting.system_registry,
        ctx,
    );
    fired
}

// U1: the Scripting surface example as shipped. Stepping onto the plate raises
// the alarm and parks the reveal; after the 800 ms wait lands, the door
// starts, the spawner releases its count tagged `closet`, the `closet` NPC
// group — placed residents and the just-released NPCs alike — is aggroed and
// damaged, and every player, the remote included, receives the ammo.
#[test]
fn closet_reveal_surface_example_rouses_placed_and_spawned_npcs_and_resupplies_every_player() {
    let level = Level::new();
    let scripting = Scripting::new(&level.ctx);
    let (manifest, _) = scripting.run("closet-reveal.ts");
    assert_eq!(manifest.reactions.len(), 5, "every reaction parses");
    let capture = LogCapture::start();
    let table = install(&level, &scripting, manifest);
    // The one expected note: the alarm write is a `fire` step, so it drains at
    // the frame end rather than inside the trigger's tick.
    let loud: Vec<_> = capture
        .records()
        .into_iter()
        .filter(|record| record.level <= log::Level::Warn)
        .filter(|record| {
            !record
                .message
                .contains("buries consequential work behind onComplete `closet.raiseAlarm`")
        })
        .collect();
    assert!(loud.is_empty(), "the example installs cleanly: {loud:?}");

    // The members the script inspected: the alarm pulse is staggered west to
    // east, so the sort by position put the west light first.
    let reactions = level.ctx.data_registry.borrow().reactions.clone();
    let body = |name: &str| {
        reactions
            .iter()
            .find(|reaction| reaction.name == name)
            .unwrap_or_else(|| panic!("`{name}` installs"))
            .descriptor
            .clone()
    };
    let ReactionDescriptor::Sequence(pulses) = body("closet.alarmLight") else {
        panic!("the alarm light is a member sequence");
    };
    let lit: Vec<SequenceTarget> = pulses.iter().map(|step| step.id.clone()).collect();
    assert_eq!(
        lit,
        vec![level.lights[0].into(), level.lights[1].into()],
        "the sort by position reached the wire"
    );
    assert!(
        matches!(body("closet.timedReveal"), ReactionDescriptor::Sequence(steps) if !steps.is_empty()),
        "install validation keeps the reveal"
    );
    assert!(
        table
            .bound_edges()
            .contains(&(level.plate, TriggerEventEdge::Enter))
    );
    assert!(
        table
            .bound_edges()
            .contains(&(level.plate, TriggerEventEdge::Exit))
    );

    let mut system = TriggerSystem::default();
    let before = level.brains();
    level.move_to(level.local.pawn, ON_PLATE);
    assert_eq!(
        frame(&level, &scripting, &mut system, &table),
        vec![(level.plate, TriggerEventEdge::Enter)]
    );
    assert_eq!(
        scripting.scheduler.pending_len(),
        1,
        "the reveal parks on the wait"
    );
    // Nothing past the wait has run yet.
    assert!(!level.door_started());
    assert_eq!(level.brains(), before, "the spawner has not fired");
    for &npc in &level.residents {
        assert!(!level.aggro(npc));
        assert_eq!(level.health(npc), NPC_HEALTH);
    }
    assert_eq!(level.ammo(level.local.pawn), 0);

    for _ in 0..FRAMES_PAST_THE_WAIT {
        assert!(frame(&level, &scripting, &mut system, &table).is_empty());
    }
    assert_eq!(scripting.scheduler.pending_len(), 0, "the wait landed");

    assert!(level.door_started(), "the door member started");
    let spawned: Vec<EntityId> = level.brains().difference(&before).copied().collect();
    assert_eq!(
        spawned.len(),
        SPAWN_COUNT as usize,
        "the spawner spawned its count"
    );
    for &npc in &spawned {
        let tags = level.tags(npc);
        assert!(
            tags.contains(&"closet".to_string()) && !tags.contains(&"closet_spawner".to_string()),
            "a spawn carries `spawned_tags`, never the spawner's own tag: {tags:?}"
        );
    }
    let spawner_at = level.position(level.spawner);
    for &npc in &spawned {
        assert!(
            (level.position(npc) - spawner_at).length() < 3.0,
            "the closet spawner released them"
        );
    }
    for &npc in level.residents.iter().chain(&spawned) {
        assert!(level.aggro(npc), "every closet NPC is aggroed");
        assert_eq!(
            level.health(npc),
            NPC_HEALTH - 5.0,
            "every closet NPC is damaged"
        );
    }
    for player in [level.local, level.remote] {
        assert_eq!(level.ammo(player.pawn), 8, "every player received the ammo");
    }
}

// U1, twin half: the Luau twin emits byte-identical wire data — the raw value
// `setupLevel` returns, as canonical JSON. The parsed manifests keep `args` as
// raw JSON, where the TS light builders' explicit nulls survive, so the parsed
// comparison covers everything but step `args`.
#[test]
fn closet_reveal_twins_emit_byte_identical_wire_data() {
    let level = Level::new();
    let scripting = Scripting::new(&level.ctx);
    let (ts_manifest, ts_wire) = scripting.run("closet-reveal.ts");
    let (luau_manifest, luau_wire) = scripting.run("closet-reveal.luau");
    assert_eq!(ts_wire, luau_wire, "TS and Luau wire diverged");
    let names = |manifest: &LevelManifest| -> Vec<String> {
        manifest
            .reactions
            .iter()
            .map(|reaction| reaction.name.clone())
            .collect()
    };
    assert_eq!(names(&ts_manifest), names(&luau_manifest));
    assert_eq!(ts_manifest.trigger_events, luau_manifest.trigger_events);
    assert_eq!(ts_manifest.crossings, luau_manifest.crossings);
    assert_eq!(ts_manifest.reactions.len(), 5);
    assert_eq!(
        ts_manifest.trigger_events.len(),
        2,
        "enter and exit on the plate"
    );
    assert_eq!(ts_manifest.crossings.len(), 1);
}
