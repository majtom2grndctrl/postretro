// The teleporter Scripting surface run as a real content/dev level: the shipped
// `teleporters.map` and its Luau twin compiled by `prl-build`, loaded from the
// PRL, installed through `install_world_cpu` under the real dev mod, then driven
// through the authoritative fixed tick and the frame-end drains production runs
// (named deaths, trigger and teleport residuals, follow-ups, removal → progress).
// See: context/lib/scripting.md §12 (Teleport events) · context/lib/movement.md §6

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

use glam::{Quat, Vec3};
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, apply_damage_with_context,
};
use postretro_entities::components::light::LightComponent;
use postretro_entities::{
    ComponentKind, EntityId, KinematicMoverComponent, ScriptCtx, ScriptError, TeleporterComponent,
    Transform,
};
use postretro_foundation::{DamagePayload, PlayerMovementComponent};
use postretro_level_loader::LevelWorld;
use postretro_physics::movement::{MovementInput, TeleportEvent};
use postretro_scripting_core::primitive_adapters::StoreSchemaJson;
use postretro_scripting_core::primitives_registry::{ContextScope, PrimitiveRegistry};
use postretro_scripting_core::reaction_dispatch::dispatch_deferred_named_events_with_sequences;
use postretro_sim::ai_host::AiTickResult;
use postretro_sim::residual_drain::ResidualRegistries;
use postretro_sim::sim::{PostMovementCommand, SimCommand, TriggerTickContext, simulate_tick};
use postretro_sim::trigger_system::PlayerId;
use serde_json::Value;

use crate::coop_fixture_tests::initialize_dev_mod;
use crate::frame_loop::named_drains::{NamedFrameEvents, drain_frame_named_events};
use crate::startup::StartupTimings;
use crate::startup::lifecycle::tests::{script_ctx, test_app};
use crate::startup::lifecycle::{WorldInstallHandles, install_world_cpu};

const DT: f32 = 1.0 / 60.0;
/// Frames that let a placed pawn settle onto the floor before it acts.
const SETTLE: usize = 30;
/// Metres above the bare floor's standing height that clear a 16-unit pad.
const PAD_CLEARANCE: f32 = 0.5;
/// Frames a lone kill is given to (wrongly) re-arm the gate.
const PROGRESS_FRAMES: usize = 4;
/// The gate lamp's brightness while the gate is disarmed and armed (`teleporters.ts`).
const LAMP_DISARMED: f32 = 0.2;
const LAMP_ARMED: f32 = 1.0;

/// Wraps `setupLevel` so its raw return, before Rust-side parsing, reaches the capture.
pub(super) const JS_CAPTURE: &str = "\n;(function () {\n  const original = globalThis.setupLevel;\n  globalThis.setupLevel = function (ctx) {\n    const result = original(ctx);\n    __teleportFixtureCapture(result);\n    return result;\n  };\n})();\n";
pub(super) const LUAU_CAPTURE: &str = "\ndo\n  local original = setupLevel\n  setupLevel = function(ctx)\n    local result = original(ctx)\n    __teleportFixtureCapture(result)\n    return result\n  end\nend\n";

pub(super) fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Compile `content/dev/maps/<map>.map` with the real `prl-build`, as an author
/// does, and load the PRL the engine would load from its own copy in `out`.
///
/// The compiler pipeline lives in the `prl-build` binary, not the library, so each
/// compile is a nested `cargo run`. One compile per map per test process: tests
/// running in parallel would otherwise contend on cargo's build lock and rebuild
/// the same map. Each test still writes and loads its own PRL, since a loaded
/// level may keep its file open for streaming.
pub(super) fn compile(map: &'static str, out: &Path) -> LevelWorld {
    static COMPILED: OnceLock<Mutex<HashMap<&'static str, Vec<u8>>>> = OnceLock::new();
    let bytes = COMPILED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(map)
        .or_insert_with(|| run_prl_build(map))
        .clone();
    let prl = out.join(format!("{map}.prl"));
    std::fs::write(&prl, bytes).expect("write the compiled PRL");
    postretro_level_loader::load_prl(prl.to_str().unwrap()).expect("compiled PRL loads")
}

fn run_prl_build(map: &str) -> Vec<u8> {
    run_prl_build_on(
        &workspace().join(format!("content/dev/maps/{map}.map")),
        &[],
    )
}

/// Compile `source` with the real `prl-build`, cold, with `extra` arguments
/// after the defaults, and return the PRL bytes.
pub(crate) fn run_prl_build_on(source: &Path, extra: &[&str]) -> Vec<u8> {
    let out = tempfile::tempdir().unwrap();
    let map = source.file_stem().unwrap().to_string_lossy();
    let prl = out.path().join(format!("{map}.prl"));
    let output = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "run",
            "--quiet",
            "-p",
            "postretro-level-compiler",
            "--bin",
            "prl-build",
            "--",
        ])
        .arg(source)
        .arg("-o")
        .arg(&prl)
        .arg("--no-tui")
        .arg("--no-cache")
        .arg("--baked-root")
        .arg(out.path().join("baked"))
        .args(extra)
        .current_dir(workspace())
        .output()
        .expect("launch prl-build");
    assert!(
        output.status.success(),
        "`{map}` failed to compile\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    std::fs::read(&prl).expect("read the compiled PRL")
}

/// Keys sorted; values untouched, so an explicit `null` is a divergence. The
/// one exception is an impact-event handle's `override` method: a function, not
/// manifest data, which each VM's capture converts differently (`{}` in JS,
/// `null` in Luau) and the manifest reader never sees.
pub(super) fn canonical(value: Value) -> Value {
    match value {
        Value::Object(mut map) => {
            if map.get("kind").and_then(Value::as_str) == Some("impact") {
                map.remove("override");
            }
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
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

/// One installed teleporter level on a single-player host.
struct Level {
    app: crate::App,
    ctx: ScriptCtx,
    world: LevelWorld,
    /// The raw value `setupLevel` returned, as canonical JSON.
    wire: String,
    pawn: EntityId,
    input_tick: u32,
    anim_time: f64,
}

impl Level {
    /// Load the compiled level and install it the way level load does, under the
    /// production dev mod (descriptors, the `enemy-death` base, reaction primitives).
    fn install(world: LevelWorld) -> Self {
        let mut app = test_app();
        let ctx = script_ctx(&app);
        let captured: Rc<RefCell<Option<Value>>> = Rc::new(RefCell::new(None));
        let mut primitives = PrimitiveRegistry::new();
        crate::scripting::primitives::register_all(&mut primitives, ctx.clone());
        primitives
            .register("__teleportFixtureCapture", {
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
        initialize_dev_mod(&mut app, &primitives);
        register_teleporter_and_trigger_primitives(&mut app, &ctx);

        let mut world = world;
        let script = world
            .data_script
            .as_mut()
            .expect("the level carries its data script");
        let luau = script.source_path.ends_with(".luau");
        script
            .compiled_bytes
            .extend_from_slice(if luau { LUAU_CAPTURE } else { JS_CAPTURE }.as_bytes());

        let content_root = workspace().join("content/dev");
        let nav_graph = crate::startup::lifecycle::install_world_gravity_and_nav(&world, &ctx);
        let products = {
            let session = app.session.as_mut().unwrap();
            session.light_bridge.populate_from_level(
                &world.lights,
                &mut ctx.registry.borrow_mut(),
                0,
            );
            let handles = WorldInstallHandles {
                world: &world,
                script_ctx: &ctx,
                command_diagnostics: session.scripting.command_diagnostics.clone(),
                mover_auto_close_ms: crate::runtime_movers::ENGINE_AUTO_CLOSE_MS,
                spawn_context: session.scripting.spawn_context.clone(),
                content_root: &content_root,
                active_level_tags: &[],
                nav_graph: nav_graph.as_ref(),
                fog_volume_bridge: &mut session.fog_volume_bridge,
                trigger_volume_bridge: &mut session.trigger_volume_bridge,
                classname_dispatch: &session.classname_dispatch,
                script_runtime: &session.scripting.script_runtime,
                sequence_registry: &session.scripting.sequence_registry,
                reaction_registry: &session.scripting.reaction_registry,
                system_registry: &session.scripting.system_registry,
                modal_stack: &mut session.modal_stack,
                progress_tracker: &mut session.progress_tracker,
                crossing_detector: &mut session.crossing_detector,
                slot_accumulator_bindings: &mut session.scripting.slot_accumulator_bindings,
                impact_policy_runtime: &mut session.scripting.impact_policy_runtime,
                mesh_clip_tables: &mut session.mesh_clip_tables,
                hit_zone_store: &mut session.hit_zone_store,
                trigger_pool_policy: crate::trigger_pools::TriggerPoolSeedPolicy::ArmAll,
                suppress_host_replicated: false,
                suppress_boot_pawn: false,
                local_carried_loadout: None,
            };
            install_world_cpu(
                handles,
                &mut StartupTimings::new(),
                |_models, _clips| {
                    crate::scripting_systems::hit_zones::ModelLoadWarningOwner::GameSide
                },
                |_first_player| {},
            )
        };
        // The windowed install's post-install wiring (`lifecycle.rs`).
        app.collision_world =
            crate::collision::CollisionWorld::from_level(&world).expect("static collision builds");
        crate::runtime_teleporters::bind_loaded_teleporters(
            &mut app.collision_world.teleporters,
            &ctx.registry.borrow(),
        );
        app.rebuild_active_system_reaction_bindings();
        app.kinematic_mover_colliders = products.mover_colliders;
        app.kinematic_mover_tick_states = products.mover_tick_states;
        app.trigger_bindings = products.trigger_bindings;
        app.teleporter_bindings = products.teleporter_bindings;
        app.rebuild_player_events(false);

        let wire = captured
            .borrow_mut()
            .take()
            .map(|raw| serde_json::to_string(&canonical(raw)).unwrap())
            .expect("the shipped setupLevel returned");
        let pawn = ctx
            .registry
            .borrow()
            .local_player_pawn()
            .expect("the boot pawn spawned at player_spawn");
        Self {
            app,
            ctx,
            world,
            wire,
            pawn,
            input_tick: 0,
            anim_time: 0.0,
        }
    }

    fn teleporter(&self, tag: &str) -> EntityId {
        let registry = self.ctx.registry.borrow();
        let found: Vec<EntityId> = registry
            .query_by_component_and_tag(ComponentKind::Teleporter, Some(tag))
            .map(|(id, _)| id)
            .collect();
        assert_eq!(found.len(), 1, "one `{tag}` teleporter");
        found[0]
    }

    fn component(&self, teleporter: EntityId) -> TeleporterComponent {
        self.ctx
            .registry
            .borrow()
            .get_component::<TeleporterComponent>(teleporter)
            .unwrap()
            .clone()
    }

    fn armed(&self, teleporter: EntityId) -> bool {
        self.component(teleporter).armed
    }

    /// The `gate_lamp` light's runtime brightness multiplier. The script writes a
    /// one-sample looping curve, which the bridge and GPU sample as a constant.
    fn lamp(&self) -> f32 {
        let registry = self.ctx.registry.borrow();
        let lamps: Vec<EntityId> = registry
            .query_by_component_and_tag(ComponentKind::Light, Some("gate_lamp"))
            .map(|(id, _)| id)
            .collect();
        assert_eq!(lamps.len(), 1, "one `gate_lamp` light");
        let light = registry.get_component::<LightComponent>(lamps[0]).unwrap();
        assert!(!light.is_dynamic, "an animated baked light");
        let animation = light
            .animation
            .as_ref()
            .expect("the script drives the lamp from load");
        assert_eq!(
            animation.play_count, None,
            "the level holds until rewritten"
        );
        match animation.brightness.as_deref() {
            Some(&[brightness]) => brightness,
            other => panic!("the lamp holds one constant brightness: {other:?}"),
        }
    }

    fn record(
        &self,
        teleporter: EntityId,
    ) -> &postretro_level_format::teleporters::TeleporterRecord {
        &self.world.teleporters.teleporters[usize::from(self.component(teleporter).index)]
    }

    /// The union AABB of a teleporter's compiled brushes.
    fn region(&self, teleporter: EntityId) -> (Vec3, Vec3) {
        let brushes = &self.record(teleporter).brushes;
        brushes.iter().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(min, max), brush| {
                (
                    min.min(Vec3::from(brush.aabb_min)),
                    max.max(Vec3::from(brush.aabb_max)),
                )
            },
        )
    }

    fn triggers(&self, tag: &str) -> Vec<(EntityId, Vec3, Vec3)> {
        let registry = self.ctx.registry.borrow();
        let bridge = &self.app.session.as_ref().unwrap().trigger_volume_bridge;
        registry
            .query_by_component_and_tag(ComponentKind::TriggerVolume, Some(tag))
            .map(|(id, _)| {
                let (min, max) = bridge.aabb(id).expect("trigger volume is bridged");
                (id, min, max)
            })
            .collect()
    }

    /// The `tag` trigger whose box centre lies inside `within`, or the only one.
    fn trigger_center(&self, tag: &str, within: Option<(Vec3, Vec3)>) -> Vec3 {
        let found: Vec<Vec3> = self
            .triggers(tag)
            .into_iter()
            .map(|(_, min, max)| (min + max) * 0.5)
            .filter(|center| {
                within.is_none_or(|(min, max)| {
                    center.x > min.x && center.x < max.x && center.z > min.z && center.z < max.z
                })
            })
            .collect();
        assert_eq!(found.len(), 1, "one `{tag}` trigger in that region");
        found[0]
    }

    fn position(&self) -> Vec3 {
        self.ctx
            .registry
            .borrow()
            .get_component::<Transform>(self.pawn)
            .unwrap()
            .position
    }

    fn movement(&self) -> PlayerMovementComponent {
        self.ctx
            .registry
            .borrow()
            .get_component::<PlayerMovementComponent>(self.pawn)
            .unwrap()
            .clone()
    }

    /// Put the pawn at rest above `at`, high enough to clear the fixture's
    /// floor pads (at most 16 units), from where it drops onto whatever is there.
    fn place(&self, at: Vec3, grounded: f32) {
        let height = grounded + PAD_CLEARANCE;
        let mut registry = self.ctx.registry.borrow_mut();
        let mut transform = *registry.get_component::<Transform>(self.pawn).unwrap();
        transform.position = Vec3::new(at.x, height, at.z);
        registry.set_component(self.pawn, transform).unwrap();
        let mut movement = registry
            .get_component::<PlayerMovementComponent>(self.pawn)
            .unwrap()
            .clone();
        movement.velocity = Vec3::ZERO;
        registry.set_component(self.pawn, movement).unwrap();
    }

    fn residents(&self) -> Vec<EntityId> {
        self.ctx
            .registry
            .borrow()
            .query_by_component_and_tag(ComponentKind::Health, Some("teleport_closet"))
            .map(|(id, _)| id)
            .collect()
    }

    fn frame(&mut self) -> Vec<(EntityId, TeleportEvent)> {
        self.frame_with(false, &[])
    }

    /// One single-player host frame of one fixed tick: the authoritative tick,
    /// then the frame-end drains in `frame_loop` order. `kill` applies lethal
    /// in-tick damage, as a shot would, at the AI stage.
    fn frame_with(
        &mut self,
        use_pressed: bool,
        kill: &[EntityId],
    ) -> Vec<(EntityId, TeleportEvent)> {
        let ctx = self.ctx.clone();
        let pawn = self.pawn;
        let app = &mut self.app;
        let session = app.session.as_mut().unwrap();
        session.scripting.scheduler.begin_frame();
        let mut deaths = std::mem::take(&mut session.pending_death_events);
        let use_edges: HashMap<PlayerId, bool> = if use_pressed {
            [(PlayerId::Local(pawn), true)].into()
        } else {
            HashMap::new()
        };
        let command = SimCommand {
            input_tick: self.input_tick,
            movement: MovementInput {
                wish_dir: glam::Vec2::ZERO,
                jump_pressed: false,
                dash_pressed: false,
                running: false,
                crouch_intent: false,
                use_pressed,
                drop_pressed: false,
                facing_yaw: 0.0,
                facing_pitch: 0.0,
            },
            fire_button: postretro_sim::weapon::FireButtonState {
                pressed: false,
                active: false,
            },
            reload: false,
            secondary_button: postretro_sim::weapon::FireButtonState {
                pressed: false,
                active: false,
            },
            activation: postretro_foundation::ActivationInput::default(),
            firing_slot: 0,
            select_slot: None,
            use_pressed,
            drop_pressed: false,
        };
        self.input_tick += 1;
        let auto_close = session.scripting.auto_close_timers.clone();
        let scripting = &mut session.scripting;
        let tick = simulate_tick(
            ctx.registry.clone(),
            &app.collision_world,
            &session.hit_zone_store,
            None,
            postretro_collision::EnvironmentView::level_only(ctx.gravity.get()),
            None,
            self.anim_time,
            &mut session.progress_tracker,
            |registry, _, _, host| {
                for &target in kill {
                    let mut context =
                        DamageContext::new("teleport-fixture", DamageProducer::InTick);
                    context.attacker = Some(pawn);
                    apply_damage_with_context(
                        registry,
                        target,
                        &DamagePayload {
                            amount: 10_000.0,
                            impulse: Vec3::ZERO,
                        },
                        context,
                    );
                    host.on_impact(registry);
                }
                AiTickResult {
                    events: Vec::new(),
                    projectile_spawns: Vec::new(),
                }
            },
            &app.kinematic_mover_colliders,
            &mut app.kinematic_mover_tick_states,
            &[],
            &command,
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            DT,
            Some(TriggerTickContext {
                teleporters: Some(&app.teleporter_bindings),
                system: &mut session.trigger_system,
                bridge: &session.trigger_volume_bridge,
                bindings: &app.trigger_bindings,
                slot_table: ctx.slot_table.clone(),
                script_ctx: Some(ctx.clone()),
                auto_close_timers: Some(auto_close),
                use_edges: &use_edges,
            }),
            |registry| scripting.evaluate_pending_in_tick_impacts(registry),
        );
        self.anim_time += f64::from(DT);
        session
            .scripting
            .scheduler
            .evaluate(&tick.trigger_exit_fires);
        deaths.extend(tick.death.iter().cloned());

        let registries = || ResidualRegistries {
            sequence: &session.scripting.sequence_registry,
            reaction: &session.scripting.reaction_registry,
            system: &session.scripting.system_registry,
        };
        let mut follow_ups = drain_frame_named_events(
            NamedFrameEvents {
                movement: &[],
                ai: &[],
                observer: &[],
                weapon: &[],
                movers: &[],
                deaths: &deaths,
            },
            &session.scripting,
            &ctx,
        );
        postretro_sim::residual_drain::drain_frame_residuals(
            &tick.trigger_residuals,
            &app.trigger_bindings,
            &session.trigger_system,
            &[],
            &app.player_events,
            &session.scripting.scheduler,
            registries(),
            &ctx,
            &mut follow_ups,
            |player, registry| match player {
                PlayerId::Local(pawn) => registry.seat_for_pawn(pawn),
                PlayerId::Remote(_) => None,
            },
        );
        postretro_sim::teleporter_system::drain_teleport_residuals(
            &tick.teleport_residuals,
            &app.teleporter_bindings,
            &session.scripting.scheduler,
            registries(),
            &ctx,
            &mut follow_ups,
        );
        if !follow_ups.is_empty() {
            dispatch_deferred_named_events_with_sequences(
                follow_ups,
                &ctx.data_registry.borrow(),
                &session.scripting.sequence_registry,
                &session.scripting.reaction_registry,
                &session.scripting.system_registry,
                &ctx,
            );
        }
        session.scripting.scheduler.drain_landings(
            &ctx.data_registry.borrow(),
            &session.scripting.sequence_registry,
            &session.scripting.reaction_registry,
            &session.scripting.system_registry,
            &ctx,
        );
        session
            .scripting
            .impact_policy_runtime
            .discard_app_drain_pending();
        crate::impact_effects::run_end_of_frame_removal_pass(
            &mut ctx.registry.borrow_mut(),
            |removal| {
                let fired = removal.report_to_progress(&mut session.progress_tracker);
                session.pending_death_events.extend(fired);
            },
        );
        tick.teleports
    }

    fn settle(&mut self) {
        for _ in 0..SETTLE {
            assert!(self.frame().is_empty(), "nothing teleports while settling");
        }
    }
}

/// The registrars `Session::build` adds beyond the shared dev-mod fixture set.
pub(super) fn register_teleporter_and_trigger_primitives(app: &mut crate::App, ctx: &ScriptCtx) {
    let scripting = &mut app.session.as_mut().unwrap().scripting;
    postretro_sim::trigger_system::register_sequenced_trigger_primitives(
        &mut scripting.sequence_registry,
        ctx.clone(),
        Default::default(),
    );
    postretro_sim::teleporter_commands::register_sequenced_teleporter_primitives(
        &mut scripting.sequence_registry,
        ctx.clone(),
    );
    postretro_sim::teleporter_commands::register_teleporter_reaction_primitives(
        &mut scripting.reaction_registry,
        ctx.clone(),
    );
}

const LEVEL_REACTIONS: [&str; 8] = [
    "levelLoad",
    "gate.open",
    "gate.shut",
    "closet.cleared",
    "closet.reveal",
    "era.shift",
    "era.reset",
    "gate.used",
];

// AC18: the Luau twin installs the same level manifest and produces
// byte-identical teleporter wire — the raw `setupLevel` return as canonical
// JSON, and the compiled id-55 section both maps carry.
#[test]
fn teleport_level_twins_emit_byte_identical_wire_data() {
    let out = tempfile::tempdir().unwrap();
    let ts = Level::install(compile("teleporters", out.path()));
    let luau = Level::install(compile("teleporters-luau", out.path()));

    assert_eq!(ts.wire, luau.wire, "TS and Luau setupLevel wire diverged");
    assert_eq!(
        ts.world.teleporters.to_bytes(),
        luau.world.teleporters.to_bytes(),
        "the twins compile one teleporter section"
    );
    assert_eq!(ts.world.teleporters.teleporters.len(), 5);

    let manifest = |level: &Level| {
        let data = level.ctx.data_registry.borrow();
        let reactions: Vec<_> = data
            .reactions
            .iter()
            .filter(|reaction| LEVEL_REACTIONS.contains(&reaction.name.as_str()))
            .cloned()
            .collect();
        (
            reactions,
            data.teleport_events().to_vec(),
            data.volume_trigger_events().to_vec(),
        )
    };
    let (reactions, teleport_events, trigger_events) = manifest(&ts);
    assert_eq!(
        (
            reactions.clone(),
            teleport_events.clone(),
            trigger_events.clone()
        ),
        manifest(&luau)
    );
    let mut names: Vec<&str> = reactions.iter().map(|r| r.name.as_str()).collect();
    names.sort_unstable();
    let mut expected = LEVEL_REACTIONS.to_vec();
    expected.sort_unstable();
    assert_eq!(
        names, expected,
        "every level reaction installs in both runtimes"
    );
    let gate = ts.teleporter("vault_gate");
    assert_eq!(
        teleport_events.len(),
        1,
        "the one-way gate binds `teleport`"
    );
    assert_eq!(teleport_events[0].teleporter, gate);
    assert_eq!(teleport_events[0].fire, ["gate.used"]);
    // button, plate, and both rooms' era button, plate and reset pads.
    assert_eq!(trigger_events.len(), 8);
}

// AC18: the brief's loop on the installed level. The gate starts disarmed, the
// button arms it, the plate disarms it and opens the closet, killing the
// closet's map-placed monsters re-arms it through `progress` once the authored
// despawn removes them, and using it disarms it. Standing in room A's central
// region, arming `era_shift` shifts the player into room B, carried relative to
// the landmark pair, without bouncing back. The gate lamp is dim whenever the
// gate is disarmed and bright whenever it is armed.
#[test]
fn teleport_dev_level_runs_the_closet_gate_loop_and_era_shift() {
    let out = tempfile::tempdir().unwrap();
    let mut level = Level::install(compile("teleporters", out.path()));
    let gate = level.teleporter("vault_gate");
    let era_a = {
        let registry = level.ctx.registry.borrow();
        let ids: Vec<EntityId> = registry
            .query_by_component_and_tag(ComponentKind::Teleporter, Some("era_shift"))
            .map(|(id, _)| id)
            .collect();
        ids
    };
    assert_eq!(era_a.len(), 2, "both era directions carry `era_shift`");
    let (era_a_to_b, era_b_to_a) = (era_a[0], era_a[1]);
    assert!(
        !level.armed(gate),
        "`enabled_on_spawn` 0 starts the gate disarmed"
    );
    assert!(!level.armed(era_a_to_b) && !level.armed(era_b_to_a));
    assert_eq!(
        level.lamp(),
        LAMP_DISARMED,
        "`levelLoad` starts the lamp dim"
    );
    level.settle();
    // The standing height on the bare floor; `place` drops from above it.
    let height = level.position().y;

    // The button arms the gate on the tick after the use.
    level.place(level.trigger_center("vault_button", None), height);
    level.settle();
    assert!(!level.armed(gate));
    assert!(level.frame_with(true, &[]).is_empty());
    level.frame();
    assert!(level.armed(gate), "armed after the button");
    assert_eq!(level.lamp(), LAMP_ARMED, "the button brightens the lamp");

    // The plate disarms it and reveals the closet.
    let residents = level.residents();
    assert_eq!(residents.len(), 2, "two map-placed closet monsters");
    for &npc in &residents {
        assert!(
            !level
                .ctx
                .registry
                .borrow()
                .get_component::<BrainComponent>(npc)
                .unwrap()
                .aggro_armed,
            "the closet monsters start passive until the plate"
        );
    }
    level.place(level.trigger_center("closet_plate", None), height);
    level.frame();
    level.frame();
    assert!(!level.armed(gate), "disarmed after the plate");
    assert_eq!(level.lamp(), LAMP_DISARMED, "the plate dims the lamp");
    let door = {
        let registry = level.ctx.registry.borrow();
        let ids: Vec<EntityId> = registry
            .query_by_component_and_tag(ComponentKind::KinematicMover, Some("closet_door"))
            .map(|(id, _)| id)
            .collect();
        ids[0]
    };
    assert!(
        level
            .ctx
            .registry
            .borrow()
            .get_component::<KinematicMoverComponent>(door)
            .unwrap()
            .started,
        "the closet door opened"
    );
    for &npc in &residents {
        assert!(
            level
                .ctx
                .registry
                .borrow()
                .get_component::<BrainComponent>(npc)
                .unwrap()
                .aggro_armed,
            "the closet monsters were roused"
        );
    }

    // Kill one: no re-arm. Kill the other: the authored policy removes it at the
    // frame end, `progress` reaches 1.0, and `gate.open` arms the gate.
    level.frame_with(false, &residents[..1]);
    for _ in 0..PROGRESS_FRAMES {
        level.frame();
    }
    assert!(
        !level.armed(gate),
        "one of two closet monsters is not a clear"
    );
    assert_eq!(level.lamp(), LAMP_DISARMED);
    level.frame_with(false, &residents[1..]);
    assert!(
        level.residents().is_empty(),
        "the authored policy removed both"
    );
    assert!(!level.armed(gate), "progress waits for the removal drain");
    assert_eq!(
        level.app.session.as_ref().unwrap().pending_death_events,
        ["gate.open"],
        "the frame-end removal completed `progress` over `closet`"
    );
    level.frame();
    assert!(!level.armed(gate), "the next frame's drain queues the arm");
    assert_eq!(
        level.ctx.pending_teleporter_mutations.borrow().as_slice(),
        &[(gate, true)]
    );
    level.frame();
    assert!(
        level.armed(gate),
        "armed again when the closet's monsters die"
    );
    assert_eq!(level.lamp(), LAMP_ARMED, "`gate.open` brightens the lamp");

    // Using it teleports the player to `vault_in` and disarms it.
    let (min, max) = level.region(gate);
    level.place((min + max) * 0.5, height);
    let teleported = level.frame();
    assert_eq!(teleported.len(), 1, "the gate fired once");
    let arrival = Vec3::from(level.record(gate).destination_origin);
    assert!((teleported[0].1.arrival - arrival).length() < 1.0e-4);
    assert!(
        (level.position() - arrival).length() < 1.0e-4,
        "the tick ends at the arrival"
    );
    level.frame();
    assert!(!level.armed(gate), "using the gate disarms it");
    assert_eq!(level.lamp(), LAMP_DISARMED, "`gate.used` dims the lamp");
    level.place((min + max) * 0.5, height);
    for _ in 0..SETTLE {
        assert!(level.frame().is_empty(), "the one-way gate stays shut");
    }

    // Room A: stand on the era plate inside the central region. Touching it arms
    // both directions; the next tick shifts the standing player into room B.
    let region_a = level.region(era_a_to_b);
    let plate = level.trigger_center("era_plate", Some(region_a));
    level.place(plate, height);
    let before = level.movement().teleport;
    assert!(
        level.frame().is_empty(),
        "the touch arms on the next tick, not this one"
    );
    assert!(!level.armed(era_a_to_b) && !level.armed(era_b_to_a));
    assert_eq!(
        level.ctx.pending_teleporter_mutations.borrow().as_slice(),
        &[(era_a_to_b, true), (era_b_to_a, true)],
        "era.shift queued both arms in member order"
    );
    let origin = level.position();
    let shifted = level.frame();
    assert!(
        level.armed(era_a_to_b) && level.armed(era_b_to_a),
        "era.shift armed both"
    );
    assert_eq!(
        shifted.len(),
        1,
        "arming era_shift shifts the player standing in it"
    );
    assert_eq!(shifted[0].1.teleporter, level.component(era_a_to_b).index);
    let record = level.record(era_a_to_b);
    let delta = record.destination_yaw - record.landmark_yaw;
    // The authored landmark pair (`angles` 0 and 90) is a quarter turn to the
    // left, as the TrenchBroom arrows show it.
    assert!((shifted[0].1.yaw_delta - delta).abs() < 1.0e-6);
    assert!(
        (delta - std::f32::consts::FRAC_PI_2).abs() < 1.0e-4,
        "the landmark pair turns the player 90° left: {delta}"
    );
    // No horizontal input, so the step moved the pawn only by one tick of fall:
    // the horizontal offset is exact and the vertical one within that fall.
    let expected = Vec3::from(record.destination_origin)
        + Quat::from_rotation_y(delta) * (origin - Vec3::from(record.landmark_origin));
    let arrival = shifted[0].1.arrival;
    assert!(
        Vec3::new(arrival.x - expected.x, 0.0, arrival.z - expected.z).length() < 1.0e-3,
        "offset rotated onto the target: {arrival} vs {expected}"
    );
    assert!(
        (arrival.y - expected.y).abs() < 0.05,
        "vertical offset kept"
    );
    let after = level.movement().teleport;
    assert_eq!(after.generation, before.generation.wrapping_add(1));
    let (min_b, max_b) = level.region(era_b_to_a);
    assert!(
        arrival.cmpge(min_b).all() && arrival.cmple(max_b).all(),
        "the shift lands inside room B's copy of the region"
    );
    // The authored layout: room B's plate is room A's plate carried by the pair,
    // so a player standing on A's plate lands on B's (TrenchBroom (8192, 64)).
    let plate_b = level.trigger_center("era_plate", Some((min_b, max_b)));
    assert!(
        Vec3::new(arrival.x - plate_b.x, 0.0, arrival.z - plate_b.z).length() < 1.0e-3,
        "the shift lands on room B's plate: {arrival} vs {plate_b}"
    );
    assert_eq!(
        after.arrival_latches,
        [level.component(era_b_to_a).index],
        "arrival latches the armed return direction"
    );
    for _ in 0..SETTLE {
        assert!(
            level.frame().is_empty(),
            "the arrival latch holds; no ping-pong"
        );
    }
    assert_eq!(
        level.lamp(),
        LAMP_DISARMED,
        "the seamless rooms leave it alone"
    );
}

// The matched baseline keeps the gate lamp and drops the gate. Its script binds
// nothing, so the lamp starts dim and the hub's button leaves it dim.
#[test]
fn teleport_baseline_gate_lamp_stays_dim_without_a_gate() {
    let out = tempfile::tempdir().unwrap();
    let mut level = Level::install(compile("teleporters-baseline", out.path()));
    assert_eq!(
        level.lamp(),
        LAMP_DISARMED,
        "`levelLoad` starts the lamp dim"
    );
    level.settle();
    let height = level.position().y;
    level.place(level.trigger_center("vault_button", None), height);
    level.settle();
    level.frame_with(true, &[]);
    level.frame();
    assert_eq!(
        level.lamp(),
        LAMP_DISARMED,
        "no gate, so the button binds nothing"
    );
}

// Hot-reload recompose rebinds teleporter events only. Armed state is live
// entity state and a queued arm/disarm is an already-dispatched effect: the
// composed reaction set is a derived view (scripting.md §12) and a staged commit
// recomposes "without re-rolling the live level" (boot_sequence.md), as trigger
// volumes keep their armed state across it.
#[test]
fn teleport_recompose_keeps_armed_state_and_queued_mutations() {
    let out = tempfile::tempdir().unwrap();
    let mut level = Level::install(compile("teleporters", out.path()));
    let gate = level.teleporter("vault_gate");
    level.settle();
    let height = level.position().y;

    // The button queues the arm in-tick; it is still pending when the mod reloads.
    level.place(level.trigger_center("vault_button", None), height);
    level.settle();
    level.frame_with(true, &[]);
    assert!(!level.armed(gate));
    assert_eq!(
        level.ctx.pending_teleporter_mutations.borrow().as_slice(),
        &[(gate, true)],
        "the button's arm waits for the next tick"
    );
    level.app.rebuild_reactions_after_staged_commit();
    assert_eq!(
        level.ctx.pending_teleporter_mutations.borrow().as_slice(),
        &[(gate, true)],
        "recompose keeps the queued arm"
    );
    level.frame();
    assert!(level.armed(gate), "the queued arm applies on the next tick");

    // A second recompose keeps the armed state, and the rebuilt binding still
    // runs the one-way gate for this teleporter.
    level.app.rebuild_reactions_after_staged_commit();
    assert!(level.armed(gate), "recompose keeps armed state");
    let (min, max) = level.region(gate);
    level.place((min + max) * 0.5, height);
    assert_eq!(
        level.frame().len(),
        1,
        "the armed gate fires after recompose"
    );
    level.frame();
    assert!(
        !level.armed(gate),
        "the rebound `teleport` event disarmed it"
    );
}
