// The ghost-link Scripting surface run as a real content/dev level: the shipped
// `ghost-links.map` and its Luau twin compiled by `prl-build`, loaded from the PRL,
// installed under the real dev mod (its `echo` style, Ghost bus profile and
// descriptor opt-ins), then driven through the authoritative fixed tick, the
// frame-end drains and the per-frame ghost update production runs.
// See: context/lib/entity_model.md §7d · context/lib/scripting.md §12 (Entity addressing)

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use glam::Vec3;
use log::Level as LogLevel;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::{
    ComponentKind, EntityId, GhostLinkComponent, KinematicMoverComponent, ReactionDescriptor,
    ScriptCtx, ScriptError, SequenceTarget, TeleporterComponent, Transform,
};
use postretro_foundation::{GhostStyle, PlayerMovementComponent};
use postretro_level_format::ghost_links::GhostMembers;
use postretro_level_loader::LevelWorld;
use postretro_physics::movement::{MovementInput, TeleportEvent};
use postretro_scripting_core::primitive_adapters::StoreSchemaJson;
use postretro_scripting_core::primitives_registry::{ContextScope, PrimitiveRegistry};
use postretro_scripting_core::reaction_dispatch::dispatch_deferred_named_events_with_sequences;
use postretro_scripting_core::runtime::{ModGhostAudio, ModGhostTransparency, ModManifestResult};
use postretro_sim::ai_host::AiTickResult;
use postretro_sim::residual_drain::ResidualRegistries;
use postretro_sim::sim::{PostMovementCommand, SimCommand, TriggerTickContext, simulate_tick};
use postretro_sim::trigger_system::PlayerId;
use postretro_test_log_capture::LogCapture;
use serde_json::Value;

use crate::coop_fixture_tests::initialize_dev_mod;
use crate::frame_loop::named_drains::{NamedFrameEvents, drain_frame_named_events};
use crate::ghost_links::{GhostCategory, GhostMember};
use crate::startup::StartupTimings;
use crate::startup::lifecycle::tests::{script_ctx, test_app};
use crate::startup::lifecycle::{WorldInstallHandles, install_world_cpu};
use crate::startup::teleporters_surface_tests::run_prl_build_on;

const DT: f32 = 1.0 / 60.0;
/// Frames that let placed pawns settle onto the floor before they act.
const SETTLE: usize = 30;
/// Metres above the bare floor's standing height that clear a 4-unit floor marker.
const PAD_CLEARANCE: f32 = 0.2;
/// Quake units per metre (`build_pipeline.md` §Supported Map Formats).
const UNITS_PER_METRE: f32 = 39.37;
/// Copy B sits this far along Quake +x from copy A, which is engine −z.
const COPY_OFFSET: Vec3 = Vec3::new(0.0, 0.0, -4096.0 / UNITS_PER_METRE);

/// The dev mod's `echo` style (`content/dev/start-script.ts`).
const ECHO: GhostStyle = GhostStyle {
    tint: [0.6, 0.8, 1.0],
    opacity: 0.45,
    saturation: 0.2,
    audio: true,
    impact_audio: false,
};
/// The dev mod's `audio.ghost` profile.
const DEV_BUS: ModGhostAudio = ModGhostAudio {
    volume: 0.7,
    high_pass_hz: 250.0,
    low_pass_hz: 3000.0,
    drive: 0.3,
};

/// Wraps `setupLevel` so its raw return, before Rust-side parsing, reaches the capture.
const JS_CAPTURE: &str = "\n;(function () {\n  const original = globalThis.setupLevel;\n  globalThis.setupLevel = function (ctx) {\n    const result = original(ctx);\n    __ghostFixtureCapture(result);\n    return result;\n  };\n})();\n";
const LUAU_CAPTURE: &str = "\ndo\n  local original = setupLevel\n  setupLevel = function(ctx)\n    local result = original(ctx)\n    __ghostFixtureCapture(result)\n    return result\n  end\nend\n";

/// Warnings this harness logs for any dev level under the dev mod, none from
/// the ghost-link surface: the model-less test install never loads models
/// (`[HitZones]` zone tags, `[Model]` animation caches), the debug textures
/// carry no material prefix, and a dev descriptor declares an unreachable
/// behavior activity. The teleporter fixture's install logs the same set.
const UNRELATED_WARNINGS: [&str; 4] = [
    "[HitZones] archetype",
    "[Model] mesh entity references uncached model",
    "[Material] Unknown prefix 'debug'",
    "components.behavior: activities [startle] are unreachable",
];

const LEVEL_REACTIONS: [&str; 6] = [
    "echo.reveal",
    "echo.swell",
    "echo.fight",
    "barrier.open",
    "battle.swap",
    "battle.settle",
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Compile `content/dev/maps/<map>.map` once per test process with the real
/// `prl-build` and load a private copy of the PRL from `out`.
fn compile(map: &'static str, out: &Path) -> LevelWorld {
    static COMPILED: OnceLock<Mutex<HashMap<&'static str, Vec<u8>>>> = OnceLock::new();
    let bytes = COMPILED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(map)
        .or_insert_with(|| {
            run_prl_build_on(
                &workspace().join(format!("content/dev/maps/{map}.map")),
                &[],
            )
        })
        .clone();
    let prl = out.join(format!("{map}.prl"));
    std::fs::write(&prl, bytes).expect("write the compiled PRL");
    postretro_level_loader::load_prl(prl.to_str().unwrap()).expect("compiled PRL loads")
}

/// Keys sorted, values untouched.
fn canonical(value: Value) -> Value {
    match value {
        Value::Object(map) => {
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

fn close(a: f32, b: f32, epsilon: f32) -> bool {
    (a - b).abs() <= epsilon
}

/// One installed ghost-link level on a single-player host, plus a partner pawn
/// standing in copy B (a network-slot pawn, as a host materializes a client's).
struct Level {
    app: crate::App,
    ctx: ScriptCtx,
    world: LevelWorld,
    manifest: ModManifestResult,
    /// The raw value `setupLevel` returned, as canonical JSON.
    wire: String,
    pawn: EntityId,
    partner: EntityId,
    input_tick: u32,
    anim_time: f64,
    /// The presented-frame clock the ghost update ramps by, one `DT` per frame.
    start: Instant,
    frames: u32,
}

impl Level {
    /// Load the compiled level and install it the way level load does, under the
    /// production dev mod, including the ghost-link install `lifecycle.rs` runs.
    fn install(world: LevelWorld) -> Self {
        let mut app = test_app();
        let ctx = script_ctx(&app);
        let captured: Rc<RefCell<Option<Value>>> = Rc::new(RefCell::new(None));
        let mut primitives = PrimitiveRegistry::new();
        crate::scripting::primitives::register_all(&mut primitives, ctx.clone());
        primitives
            .register("__ghostFixtureCapture", {
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
        let manifest = initialize_dev_mod(&mut app, &primitives);
        register_level_primitives(&mut app, &ctx);
        // The commit `Session` makes beyond the shared fixture: the style table
        // and the mod's audio profile.
        ctx.data_registry
            .borrow_mut()
            .replace_ghost_styles(manifest.ghost_styles.clone());
        app.apply_mod_audio_profile(manifest.audio);

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
                |_spawns| {},
            )
        };
        // The windowed install's post-install wiring (`lifecycle.rs`).
        app.collision_world =
            crate::collision::CollisionWorld::from_level(&world).expect("static collision builds");
        crate::runtime_teleporters::bind_loaded_teleporters(
            &mut app.collision_world.teleporters,
            &ctx.registry.borrow(),
        );
        app.ghost_links = crate::ghost_links::GhostLinks::install(&world, &ctx.registry.borrow());
        crate::ghost_links::warn_if_links_without_opt_ins(
            world.ghost_links.links.len(),
            &ctx.data_registry.borrow(),
        );
        app.ghost_links
            .bind_styles(&world, &ctx.data_registry.borrow().ghost_styles);
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
        // The partner: the level's second `player_spawn`, in copy B, through the
        // host's network-slot materialization.
        let placement = world
            .map_entities
            .iter()
            .filter(|entity| entity.classname == "player_spawn")
            .nth(1)
            .cloned()
            .expect("a second player_spawn, in copy B");
        let partner =
            postretro_sim::scripting::builtins::net_descriptor::spawn_net_slot_pawn_with_carried_loadout(
                &crate::scripting::map_entity::MapEntity::from(placement),
                &manifest.entities,
                &mut ctx.registry.borrow_mut(),
                None,
                None,
            )
            .expect("the partner pawn spawns from the dev player descriptor");
        Self {
            app,
            ctx,
            world,
            manifest,
            wire,
            pawn,
            partner,
            input_tick: 0,
            anim_time: 0.0,
            start: Instant::now(),
            frames: 0,
        }
    }

    fn entities(&self, kind: ComponentKind, tag: &str) -> Vec<EntityId> {
        self.ctx
            .registry
            .borrow()
            .query_by_component_and_tag(kind, Some(tag))
            .map(|(id, _)| id)
            .collect()
    }

    /// The level's ghost links, in id-56 (map) order.
    fn links(&self) -> Vec<(EntityId, GhostLinkComponent)> {
        let registry = self.ctx.registry.borrow();
        let mut links: Vec<_> = registry
            .query_by_component_and_tag(ComponentKind::GhostLink, None)
            .map(|(id, _)| {
                (
                    id,
                    *registry.get_component::<GhostLinkComponent>(id).unwrap(),
                )
            })
            .collect();
        links.sort_by_key(|(_, link)| link.index);
        links
    }

    fn teleporter(&self, teleporter: EntityId) -> TeleporterComponent {
        self.ctx
            .registry
            .borrow()
            .get_component::<TeleporterComponent>(teleporter)
            .unwrap()
            .clone()
    }

    fn barrier(&self) -> EntityId {
        let barriers = self.entities(ComponentKind::KinematicMover, "vault_barrier");
        assert_eq!(barriers.len(), 1, "one tagged barrier");
        barriers[0]
    }

    fn position(&self, entity: EntityId) -> Vec3 {
        self.ctx
            .registry
            .borrow()
            .get_component::<Transform>(entity)
            .unwrap()
            .position
    }

    /// The `tag` trigger whose box centre lies in copy A (`in_b` false) or B.
    fn trigger_center(&self, tag: &str, in_b: bool) -> Vec3 {
        let registry = self.ctx.registry.borrow();
        let bridge = &self.app.session.as_ref().unwrap().trigger_volume_bridge;
        let found: Vec<Vec3> = registry
            .query_by_component_and_tag(ComponentKind::TriggerVolume, Some(tag))
            .map(|(id, _)| {
                let (min, max) = bridge.aabb(id).expect("trigger volume is bridged");
                (min + max) * 0.5
            })
            .filter(|center| (center.z < COPY_OFFSET.z * 0.5) == in_b)
            .collect();
        assert_eq!(found.len(), 1, "one `{tag}` trigger in that copy");
        found[0]
    }

    /// Put the local pawn at rest above `at`, from where it drops onto the floor.
    fn place(&self, at: Vec3, grounded: f32) {
        let mut registry = self.ctx.registry.borrow_mut();
        let mut transform = *registry.get_component::<Transform>(self.pawn).unwrap();
        transform.position = Vec3::new(at.x, grounded + PAD_CLEARANCE, at.z);
        registry.set_component(self.pawn, transform).unwrap();
        let mut movement = registry
            .get_component::<PlayerMovementComponent>(self.pawn)
            .unwrap()
            .clone();
        movement.velocity = Vec3::ZERO;
        registry.set_component(self.pawn, movement).unwrap();
    }

    /// This frame's ghosts.
    fn ghosts(&self) -> Vec<GhostMember> {
        self.app.ghost_links.members().to_vec()
    }

    fn ghost_of(&self, entity: EntityId) -> Vec<GhostMember> {
        self.ghosts()
            .into_iter()
            .filter(|member| member.entity == entity)
            .collect()
    }

    fn frame(&mut self) -> Vec<(EntityId, TeleportEvent)> {
        self.frame_with(false)
    }

    /// One single-player host frame of one fixed tick, in `frame_loop` order:
    /// the frame's script time, the host-only ghost-link and Ghost bus steps at
    /// the tick's start, the authoritative tick, link history after it, the
    /// frame-end drains, the system-command drain, then the ghost update.
    fn frame_with(&mut self, use_pressed: bool) -> Vec<(EntityId, TeleportEvent)> {
        self.frames += 1;
        self.app.script_time += f64::from(DT);
        self.ctx.presentation_script_time.set(self.app.script_time);
        self.app.apply_pending_ghost_link_commands();
        self.app.apply_pending_ghost_audio_commands();

        let ctx = self.ctx.clone();
        let pawn = self.pawn;
        let app = &mut self.app;
        let session = app.session.as_mut().unwrap();
        session.scripting.scheduler.begin_frame();
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
            |_, _, _, _| AiTickResult {
                events: Vec::new(),
                projectile_spawns: Vec::new(),
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
                deaths: &tick.death,
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
        self.app.record_ghost_link_history();
        // `setGhostAudio` leaves the reaction drain as a system command.
        self.app.dispatch_system_commands();
        let now = self.start + Duration::from_secs_f32(DT * self.frames as f32);
        self.app.update_ghost_links(1.0, now);
        tick.teleports
    }

    fn settle(&mut self) {
        for _ in 0..SETTLE {
            assert!(self.frame().is_empty(), "nothing teleports while settling");
        }
    }
}

/// The registrars `Session::build` adds beyond the shared dev-mod fixture set.
fn register_level_primitives(app: &mut crate::App, ctx: &ScriptCtx) {
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
    postretro_sim::ghost_link_commands::register_sequenced_ghost_link_primitives(
        &mut scripting.sequence_registry,
        ctx.clone(),
    );
    postretro_sim::ghost_link_commands::register_ghost_link_reaction_primitives(
        &mut scripting.reaction_registry,
        ctx.clone(),
    );
}

fn reaction(level: &Level, name: &str) -> ReactionDescriptor {
    level
        .ctx
        .data_registry
        .borrow()
        .reactions
        .iter()
        .find(|reaction| reaction.name == name)
        .unwrap_or_else(|| panic!("`{name}` installed"))
        .descriptor
        .clone()
}

// The dev mod's manifest and both twins' level scripts — the brief's
// Scripting surface examples — install and run with zero warnings, emit
// byte-identical wire, and resolve `setGhostAudio` from `postretro/audio` in
// both runtimes. The `ghostLink` query returns both dev-map links in map order.
#[test]
fn ghost_link_twins_install_the_scripting_surface_with_zero_warnings() {
    let logs = LogCapture::start();
    let out = tempfile::tempdir().unwrap();
    let mut ts = Level::install(compile("ghost-links", out.path()));
    let mut luau = Level::install(compile("ghost-links-luau", out.path()));
    for level in [&mut ts, &mut luau] {
        level.settle();
    }
    let warnings: Vec<_> = logs
        .records()
        .into_iter()
        .filter(|record| record.level <= LogLevel::Warn)
        .filter(|record| {
            !UNRELATED_WARNINGS
                .iter()
                .any(|known| record.message.contains(known))
        })
        .collect();
    assert!(
        warnings.is_empty(),
        "mod init, both installs and their first frames warn nothing: {warnings:#?}"
    );

    // The dev manifest: the sound-carrying style, the bus profile, blend, and
    // the descriptor opt-ins the fixture leans on.
    let manifest = &ts.manifest;
    assert_eq!(manifest.ghost_styles.get("echo"), Some(&ECHO));
    assert_eq!(manifest.audio.ghost, DEV_BUS);
    assert_eq!(manifest.render.ghosts, ModGhostTransparency::Blend);
    let descriptor = |name: &str| {
        manifest
            .entities
            .iter()
            .find(|descriptor| descriptor.canonical_name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("dev descriptor `{name}`"))
    };
    assert!(descriptor("player").ghost, "the dev player opts in");
    assert!(
        !descriptor("reference_enemy").ghost,
        "the fixture's monsters do not, so each player sees only their own"
    );

    assert_eq!(ts.wire, luau.wire, "TS and Luau setupLevel wire diverged");
    assert_eq!(
        ts.world.ghost_links.to_bytes(),
        luau.world.ghost_links.to_bytes(),
        "the twins compile one ghost-link section"
    );
    let records = &ts.world.ghost_links.links;
    assert_eq!(records.len(), 2, "a two-way link pair");
    assert!(records[0].enabled_on_spawn && !records[1].enabled_on_spawn);
    assert_eq!(records[0].ghost_tags, ["vault_barrier"]);
    assert!(records[1].ghost_tags.is_empty());
    for record in records {
        assert_eq!(record.style, "echo");
        assert_eq!(record.members, GhostMembers::all());
    }

    for level in [&ts, &luau] {
        let data = level.ctx.data_registry.borrow();
        let mut names: Vec<&str> = data
            .reactions
            .iter()
            .map(|reaction| reaction.name.as_str())
            .filter(|name| LEVEL_REACTIONS.contains(name))
            .collect();
        names.sort_unstable();
        let mut expected = LEVEL_REACTIONS.to_vec();
        expected.sort_unstable();
        assert_eq!(names, expected, "every level reaction installs");
        // Echo plate and swap pad in each copy, the button in copy B.
        assert_eq!(data.volume_trigger_events().len(), 5);
        assert_eq!(
            data.teleport_events().len(),
            2,
            "both swap teleporters bind"
        );
    }
    let manifest_of = |level: &Level| {
        let data = level.ctx.data_registry.borrow();
        (
            data.reactions
                .iter()
                .filter(|reaction| LEVEL_REACTIONS.contains(&reaction.name.as_str()))
                .cloned()
                .collect::<Vec<_>>(),
            data.teleport_events().to_vec(),
            data.volume_trigger_events().to_vec(),
        )
    };
    assert_eq!(manifest_of(&ts), manifest_of(&luau));

    // `getMapEntities("ghostLink", { tag: "vault_echo" })` returned both
    // links, in map order: `echo.reveal` arms, then filters, each in turn.
    for level in [&ts, &luau] {
        let links: Vec<EntityId> = level.links().into_iter().map(|(id, _)| id).collect();
        let ReactionDescriptor::Sequence(steps) = reaction(level, "echo.reveal") else {
            panic!("echo.reveal is a sequence");
        };
        let targets: Vec<(&str, EntityId)> = steps
            .iter()
            .map(|step| match step.id {
                SequenceTarget::Entity(id) => (step.primitive.as_str(), id),
                ref other => panic!("an entity-targeted step: {other:?}"),
            })
            .collect();
        assert_eq!(
            targets,
            [
                ("armGhostLink", links[0]),
                ("armGhostLink", links[1]),
                ("setGhostLinkColorFilter", links[0]),
                ("setGhostLinkColorFilter", links[1]),
            ]
        );
        // `setGhostAudio` resolved from `postretro/audio`: a target-less
        // primitive body.
        let ReactionDescriptor::Primitive(swell) = reaction(level, "echo.swell") else {
            panic!("echo.swell is a primitive body");
        };
        assert_eq!(swell.primitive, "setGhostAudio");
        assert_eq!(
            (swell.kind, swell.tag.as_deref(), swell.target.as_deref()),
            (None, None, None)
        );
        assert_eq!(
            swell.args,
            serde_json::json!({ "volume": 0.9, "lowPassHz": 5000, "periodMs": 1500 })
        );
    }
}

// The set-pieces drive what the membership, look, category, replication
// and bus rows observe, in both runtimes. The barrier, a tagged mover on the
// link armed on spawn, ghosts into copy B at full membership; the button in
// copy B drops it and its ghost follows. The echo plate arms the second link,
// so the partner in copy B ghosts into copy A, and both links' colour filters
// and the Ghost bus ramp. The swap pad narrows projectiles, rouses the
// monsters, and swaps the player standing in copy A into copy B. Monsters,
// whose descriptor does not opt in, and the viewer's own pawn never ghost.
#[test]
fn ghost_link_dev_level_runs_the_barrier_puzzle_and_battle_swap() {
    let out = tempfile::tempdir().unwrap();
    for map in ["ghost-links", "ghost-links-luau"] {
        run_set_pieces(map, Level::install(compile(map, out.path())));
    }
}

fn run_set_pieces(map: &str, mut level: Level) {
    let links = level.links();
    assert!(links[0].1.armed, "{map}: echo_a armed on spawn");
    assert!(!links[1].1.armed, "{map}: echo_b waits for the reveal");
    for (_, link) in &links {
        assert_eq!(link.members, GhostMembers::all().bits());
    }
    for index in [0, 1] {
        assert_eq!(level.app.ghost_links.link_style(index), ECHO, "{map}");
    }
    let monsters = level.entities(ComponentKind::Health, "echo_monster");
    assert_eq!(monsters.len(), 4, "{map}: two monsters per copy");
    level.settle();
    let height = level.position(level.pawn).y;
    let resolved = level.app.ghost_bus.resolve(level.app.script_time);
    assert_eq!(
        (
            resolved.volume,
            resolved.high_pass_hz,
            resolved.low_pass_hz,
            resolved.drive
        ),
        (
            DEV_BUS.volume,
            DEV_BUS.high_pass_hz,
            DEV_BUS.low_pass_hz,
            DEV_BUS.drive
        ),
        "{map}: the bus presents the dev profile"
    );

    // At rest: only the barrier ghosts, through echo_a, at full membership, at
    // its pose carried into copy B. The viewer, the partner (echo_b is
    // disarmed) and every monster are absent.
    let barrier = level.barrier();
    let ghosts = level.ghosts();
    assert_eq!(
        ghosts.len(),
        1,
        "{map}: only the barrier ghosts: {ghosts:#?}"
    );
    let ghost = &ghosts[0];
    assert_eq!((ghost.entity, ghost.link), (barrier, 0));
    assert_eq!(ghost.category, GhostCategory::Movers);
    assert_eq!(ghost.membership, 1.0, "{map}: armed on spawn");
    let rest = level.position(barrier);
    assert!(
        (ghost.mapped.position - (ghost.source.position + COPY_OFFSET)).length() < 1e-3,
        "{map}: the ghost stands in copy B's doorway"
    );
    assert!((ghost.look.opacity - ECHO.opacity).abs() < 1e-6);

    // Barrier puzzle: the button in copy B drops copy A's barrier, and its
    // ghost sinks with it.
    level.place(level.trigger_center("vault_button", true), height);
    for _ in 0..SETTLE {
        level.frame();
    }
    level.frame_with(true);
    level.frame();
    assert!(
        level
            .ctx
            .registry
            .borrow()
            .get_component::<KinematicMoverComponent>(barrier)
            .unwrap()
            .started,
        "{map}: the button started the barrier"
    );
    for _ in 0..30 {
        level.frame();
    }
    let sinking = level.ghost_of(barrier);
    assert_eq!(sinking.len(), 1);
    let drop = rest.y - level.position(barrier).y;
    assert!(drop > 0.5, "{map}: the barrier sinks: {drop}");
    assert!(
        (sinking[0].mapped.position - (level.position(barrier) + COPY_OFFSET)).length() < 1e-3,
        "{map}: its ghost follows into copy B"
    );
    assert_eq!(sinking[0].membership, 1.0, "{map}: still inside echo_a");

    // The reveal: the echo plate in copy A arms echo_b and sets both links'
    // colour filter; the swell ramps the Ghost bus.
    level.place(level.trigger_center("echo_plate", false), height);
    level.frame();
    level.frame();
    let applied_at = level.app.script_time;
    let links = level.links();
    assert!(links[0].1.armed && links[1].1.armed, "{map}: both armed");
    for (_, link) in &links {
        let tint = link.filter.tint.expect("tint ramps");
        let opacity = link.filter.opacity.expect("opacity ramps");
        assert_eq!((tint.from, tint.to), (ECHO.tint, [1.0, 0.4, 0.3]));
        assert_eq!((opacity.from, opacity.to), (ECHO.opacity, 0.6));
        assert_eq!((tint.duration_ms, opacity.duration_ms), (1500.0, 1500.0));
        assert!(
            link.filter.saturation.is_none(),
            "omitted fields keep the style"
        );
    }
    let fields = *level.app.ghost_bus.script_fields();
    let volume = fields.volume.expect("the swell sets volume");
    let low_pass = fields.low_pass_hz.expect("and the low-pass");
    assert_eq!((volume.from, volume.to), (DEV_BUS.volume, 0.9));
    assert_eq!((low_pass.from, low_pass.to), (DEV_BUS.low_pass_hz, 5000.0));
    assert!(fields.high_pass_hz.is_none() && fields.drive.is_none());
    // The partner, standing in copy B, ramps in through echo_b from 0.
    let partner = level.ghost_of(level.partner);
    assert_eq!(partner.len(), 1, "{map}: the partner ghosts once");
    assert_eq!(
        (partner[0].link, partner[0].category),
        (1, GhostCategory::Players)
    );
    assert_eq!(partner[0].membership, 0.0, "{map}: its first frame is at 0");
    level.frame();
    let partner = level.ghost_of(level.partner);
    assert!(
        partner[0].membership > 0.0 && partner[0].membership < 1.0,
        "{map}: a new ghost ramps in: {}",
        partner[0].membership
    );
    assert!(
        (partner[0].mapped.position - (partner[0].source.position - COPY_OFFSET)).length() < 1e-3,
        "{map}: the partner's ghost stands in copy A"
    );
    // Halfway through the 1500 ms ramps, both present their midpoints.
    while level.app.script_time < applied_at + 0.75 {
        level.frame();
    }
    let elapsed = level.app.script_time - applied_at;
    let progress = (elapsed / 1.5) as f32;
    let look = level.app.ghost_links.link_look(1);
    assert!(
        close(
            look.opacity,
            ECHO.opacity + (0.6 - ECHO.opacity) * progress,
            0.02
        ),
        "{map}: opacity mid-ramp {look:?}"
    );
    let partner = level.ghost_of(level.partner);
    assert_eq!(
        partner[0].membership, 1.0,
        "{map}: the reveal is not limited"
    );
    assert!((partner[0].look.opacity - look.opacity).abs() < 0.02);
    let bus = level.app.ghost_bus.resolve(level.app.script_time);
    assert!(
        close(
            bus.volume,
            DEV_BUS.volume + (0.9 - DEV_BUS.volume) * progress,
            0.02
        ),
        "{map}: the swell mid-ramp {bus:?}"
    );

    // The battle swap: the swap pad in copy A arms both seamless teleporters,
    // rouses the monsters and stops ghosting projectiles; the player standing
    // in copy A lands in copy B, and the first crossing disarms both.
    let swap = level.entities(ComponentKind::Teleporter, "echo_swap");
    assert_eq!(swap.len(), 2, "{map}: a two-way seamless pair");
    for &npc in &monsters {
        assert!(
            !level
                .ctx
                .registry
                .borrow()
                .get_component::<BrainComponent>(npc)
                .unwrap()
                .aggro_armed,
            "{map}: monsters wait for the swap"
        );
    }
    level.place(level.trigger_center("swap_pad", false), height);
    for _ in 0..SETTLE {
        assert!(level.frame().is_empty());
    }
    let before = level.position(level.pawn);
    assert!(
        level.frame_with(true).is_empty(),
        "{map}: the use queues the arm"
    );
    let swapped = level.frame();
    assert_eq!(swapped.len(), 1, "{map}: the player in copy A swaps");
    assert_eq!(swapped[0].0, level.pawn);
    let after = level.position(level.pawn);
    assert!(
        (Vec3::new(after.x, 0.0, after.z) - Vec3::new(before.x, 0.0, before.z + COPY_OFFSET.z))
            .length()
            < 1e-2,
        "{map}: the swap carries the player into copy B: {before} → {after}"
    );
    for (_, link) in level.links() {
        assert_eq!(
            link.members,
            (GhostMembers::all() - GhostMembers::PROJECTILES).bits(),
            "{map}: echo.fight hides projectiles"
        );
    }
    for &npc in &monsters {
        assert!(
            level
                .ctx
                .registry
                .borrow()
                .get_component::<BrainComponent>(npc)
                .unwrap()
                .aggro_armed,
            "{map}: the swap rouses every monster"
        );
    }
    level.frame();
    level.frame();
    for &teleporter in &swap {
        assert!(
            !level.teleporter(teleporter).armed,
            "{map}: the crossing disarms both directions"
        );
    }
    let ghosts = level.ghosts();
    assert!(
        ghosts
            .iter()
            .all(|ghost| ghost.entity != level.pawn && !monsters.contains(&ghost.entity)),
        "{map}: neither the viewer nor a monster ever ghosts: {ghosts:#?}"
    );
}
