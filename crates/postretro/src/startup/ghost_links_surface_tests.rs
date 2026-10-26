// The ghost-link Scripting surface run as a real content/dev level: the shipped
// `ghost-links.map` and its Luau twin compiled by `prl-build`, loaded from the PRL,
// installed under the real dev mod (its `echo` style, Ghost bus profile and
// descriptor opt-ins), then driven through the authoritative fixed tick, the
// frame-end drains and the per-frame ghost update production runs.
// See: context/lib/entity_model.md §7d · context/lib/scripting.md §12 (Entity addressing)

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use glam::Vec3;
use log::Level as LogLevel;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{
    ComponentKind, EntityId, GhostLinkComponent, KinematicMoverComponent, ReactionDescriptor,
    ScriptCtx, ScriptError, SequenceTarget, TeleporterComponent, Transform, TriggerVolumeComponent,
};
use postretro_foundation::{GhostStyle, PlayerMovementComponent};
use postretro_level_format::ghost_links::GhostMembers;
use postretro_level_loader::LevelWorld;
use postretro_physics::movement::{MovementInput, TeleportEvent};
use postretro_scripting_core::primitive_adapters::StoreSchemaJson;
use postretro_scripting_core::primitives_registry::{ContextScope, PrimitiveRegistry};
use postretro_scripting_core::reaction_dispatch::dispatch_deferred_named_events_with_sequences;
use postretro_scripting_core::runtime::{ModGhostAudio, ModGhostTransparency, ModManifestResult};
use postretro_sim::residual_drain::ResidualRegistries;
use postretro_sim::sim::{
    PostMovementCommand, RemotePawnCommand, SimCommand, TriggerTickContext,
    simulate_tick_with_presentation_aim,
};
use postretro_sim::trigger_system::PlayerId;
use postretro_test_log_capture::LogCapture;
use serde_json::Value;

use crate::coop_fixture_tests::initialize_dev_mod;
use crate::frame_loop::named_drains::{NamedFrameEvents, drain_frame_named_events};
use crate::ghost_links::{GhostCategory, GhostMember};
use crate::netcode::frame_order::ReplicatedStateFrame;
use crate::startup::StartupTimings;
use crate::startup::lifecycle::tests::{script_ctx, test_app};
use crate::startup::lifecycle::{WorldInstallHandles, install_world_cpu};
use crate::startup::teleporters_surface_tests::run_prl_build_on;

const DT: f32 = 1.0 / 60.0;
/// Frames that let placed pawns settle onto the floor before they act.
const SETTLE: usize = 30;
/// Fixed ticks a fight runs once its monsters wake: ten seconds.
const BATTLE_FRAMES: usize = 600;
/// Metres above the bare floor's standing height that clear a 4-unit floor marker.
const PAD_CLEARANCE: f32 = 0.2;
/// Quake units per metre (`build_pipeline.md` §Supported Map Formats).
const UNITS_PER_METRE: f32 = 39.37;
/// Copy B sits this far along Quake +x from copy A, which is engine −z.
const COPY_OFFSET: Vec3 = Vec3::new(0.0, 0.0, -4096.0 / UNITS_PER_METRE);
/// The closets' open faces, Quake y ±208, on engine x: past it is a closet.
const CLOSET_LINE: f32 = 208.0 / UNITS_PER_METRE;
/// Metres the two spawns' capsules keep clear of each other, beyond touching.
const SPAWN_MARGIN: f32 = 0.5;
/// Quake x of the westmost closet mouth in a room copy: copy A's south closet.
const CLOSET_MOUTH_WEST_X: f32 = -64.0;
/// The partner's client id: its seat, and its `PlayerId::Remote` while live.
const PARTNER_CLIENT: u64 = 7;
/// The north closet's timer, counted from level load.
const NORTH_TIMER_SECS: f64 = 15.0;
/// The beat between the split crossing and copy B's closets opening.
const LIMITATOR_BEAT_SECS: f64 = 3.0;
/// The final wave's beat between its fire and the closet doors starting.
const WAVE_BEAT_SECS: f64 = 3.0;
/// Slack around a beat's end: a few ticks either side of it.
const WAVE_BEAT_SLACK: f64 = 0.1;
/// Frames a pedestal takes to rise its 56 units at 1 m/s, with margin.
const RISE_FRAMES: usize = 120;
/// The return pedestal's half height: its brush is 48 Quake units tall.
const PEDESTAL_HALF_HEIGHT: f32 = 24.0 / UNITS_PER_METRE;
/// Where the players stand for the final wave: in room A just inside the
/// doorway, east of the cover wall, in sight of the final closet. From farther
/// back in room A the baked navmesh offers the closet's knights no route (a
/// `find_path` limit, not this map's geometry), so they would wait in it.
const HOST_STAND: (f32, f32) = (160.0, -32.0);
const PARTNER_STAND: (f32, f32) = (160.0, 32.0);
/// Quake x of the final closet's open face: past it is the closet.
const FINAL_CLOSET_X: f32 = 448.0;

/// A Quake map point in engine metres (`build_pipeline.md` §Supported Map Formats).
fn quake(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(-y, z, -x) / UNITS_PER_METRE
}

/// Horizontal distance: pawns settle onto floors, so heights differ a little.
fn flat_distance(a: Vec3, b: Vec3) -> f32 {
    Vec3::new(a.x - b.x, 0.0, a.z - b.z).length()
}

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
/// the ghost-link manifest or scripts. The shared `reference_enemy`
/// descriptor declares zone multipliers its model has no zones for
/// (`[HitZones]`) and a behavior activity the lint calls unreachable; the
/// dev maps' debug textures carry no material prefix; and this install's
/// no-op upload leaves animation caches empty (`[Model]`), which a real
/// install fills, so the AI tick cannot switch a monster's animation state
/// either (`[AI] enemy animation state`). The teleporter fixture's install
/// logs the same set.
const UNRELATED_WARNINGS: [&str; 5] = [
    "[HitZones] archetype",
    "[Model] mesh entity references uncached model",
    "[Material] Unknown prefix 'debug'",
    "components.behavior: activities [startle] are unreachable",
    "[AI] enemy animation state",
];

const LEVEL_REACTIONS: [&str; 16] = [
    "echo.reveal",
    "echo.swell",
    "echo.fight",
    "barrier.open",
    "closet.a1",
    "levelLoad",
    "split.arm",
    "split.settle",
    "closet.a2",
    "closet.b",
    "swap.arm",
    "swap.settle",
    "wave.start",
    "return.rise",
    "return.arm",
    "return.settle",
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

/// One installed ghost-link level on a co-op host, plus a partner pawn standing
/// in copy B: a joiner's network-slot pawn at the spawn the host's seat table
/// assigns it, as the host materializes a client's.
struct Level {
    app: crate::App,
    ctx: ScriptCtx,
    world: LevelWorld,
    manifest: ModManifestResult,
    /// The raw value `setupLevel` returned, as canonical JSON.
    wire: String,
    pawn: EntityId,
    partner: EntityId,
    /// Player pawns before the partner joined: the boot spawn alone.
    boot_players: usize,
    /// The `player_spawn` index the host's seat table assigned the partner.
    partner_placement: usize,
    /// Whether the partner runs as a connected client's pawn: one neutral
    /// remote command per tick, so triggers and teleporters see it.
    /// Off, the host moves it only where a test puts it.
    partner_live: bool,
    input_tick: u32,
    anim_time: f64,
    /// The presented-frame clock the ghost update ramps by, one `DT` per frame.
    start: Instant,
    frames: u32,
    /// The production AI tick's state, so monsters think as in a real session.
    ai_runtime: postretro_ai::AiRuntime,
    /// The level's baked navmesh, which the AI tick paths monsters over.
    nav_graph: Option<crate::nav::NavGraph>,
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
        let mut seats = crate::netcode::SeatTable::local_only();
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
                // The windowed host's seat binding (`lifecycle.rs`).
                |first_player| {
                    crate::startup::lifecycle::bind_installed_first_player(
                        &mut seats,
                        &mut ctx.registry.borrow_mut(),
                        first_player,
                    )
                },
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
            app.ghost_links.ghosts_movers(),
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
        let boot_players = ctx
            .registry
            .borrow()
            .iter_with_kind(ComponentKind::PlayerMovement)
            .count();
        // The partner joins: the host's seat table assigns its spawn, as the
        // participating-slot path in `main.rs` does, and the network-slot
        // materialization spawns it there.
        let (partner, partner_placement) = {
            let seat = seats
                .admit_or_reclaim(
                    PARTNER_CLIENT,
                    Some(postretro_net::wire::ConnectClaim {
                        player_id: postretro_net::wire::PlayerClaimId([7; 16]),
                        display_name: "Partner".to_string(),
                    }),
                    false,
                )
                .expect("the partner is admitted to a seat")
                .seat;
            let partner_placement = seats
                .assign_joiner_placement(
                    &ctx.registry.borrow(),
                    seat,
                    &crate::scripting::builtins::player_spawn::joinable_spawns(
                        &products.spawn_points,
                        &manifest.entities,
                    ),
                )
                .expect("the partner is assigned a player_spawn");
            let partner =
            postretro_sim::scripting::builtins::net_descriptor::spawn_net_slot_pawn_with_carried_loadout(
                &products.spawn_points[partner_placement],
                &manifest.entities,
                &mut ctx.registry.borrow_mut(),
                None,
                None,
            )
            .expect("the partner pawn spawns from the dev player descriptor");
            (partner, partner_placement)
        };
        Self {
            app,
            ctx,
            world,
            manifest,
            wire,
            pawn,
            partner,
            boot_players,
            partner_placement,
            partner_live: false,
            input_tick: 0,
            anim_time: 0.0,
            start: Instant::now(),
            frames: 0,
            ai_runtime: postretro_ai::AiRuntime::new(),
            nav_graph,
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

    fn trigger_armed(&self, trigger: EntityId) -> bool {
        self.ctx
            .registry
            .borrow()
            .get_component::<TriggerVolumeComponent>(trigger)
            .unwrap()
            .armed
    }

    fn teleporter(&self, teleporter: EntityId) -> TeleporterComponent {
        self.ctx
            .registry
            .borrow()
            .get_component::<TeleporterComponent>(teleporter)
            .unwrap()
            .clone()
    }

    /// The descriptor `entity` was spawned from.
    fn class(&self, entity: EntityId) -> String {
        self.ctx
            .registry
            .borrow()
            .get_component::<DescriptorProvenance>(entity)
            .unwrap()
            .canonical_name
            .clone()
    }

    fn armed(&self, teleporter: EntityId) -> bool {
        self.teleporter(teleporter).armed
    }

    fn started(&self, mover: EntityId) -> bool {
        self.ctx
            .registry
            .borrow()
            .get_component::<KinematicMoverComponent>(mover)
            .unwrap()
            .started
    }

    fn brain(&self, npc: EntityId) -> BrainComponent {
        self.ctx
            .registry
            .borrow()
            .get_component::<BrainComponent>(npc)
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
        self.place_pawn(self.pawn, at, grounded);
    }

    /// Put `pawn` at rest above `at`, from where it drops onto the floor.
    fn place_pawn(&self, pawn: EntityId, at: Vec3, grounded: f32) {
        let mut registry = self.ctx.registry.borrow_mut();
        let mut transform = *registry.get_component::<Transform>(pawn).unwrap();
        transform.position = Vec3::new(at.x, grounded + PAD_CLEARANCE, at.z);
        registry.set_component(pawn, transform).unwrap();
        let mut movement = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone();
        movement.velocity = Vec3::ZERO;
        registry.set_component(pawn, movement).unwrap();
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

    fn frame_with(&mut self, use_pressed: bool) -> Vec<(EntityId, TeleportEvent)> {
        self.frame_full(use_pressed, false)
    }

    /// One frame in which the live partner presses use.
    fn partner_frame_with_use(&mut self) -> Vec<(EntityId, TeleportEvent)> {
        assert!(self.partner_live, "only a live partner presses");
        self.frame_full(false, true)
    }

    /// One host frame of one fixed tick, in `frame_loop` order: the frame's
    /// script time, the host-only ghost-link and Ghost bus steps at the tick's
    /// start, the authoritative tick with the AI (and the live partner's
    /// command), link history after it, the frame-end drains, the crossing
    /// stage, the frame-end removal that reports `progress`, the
    /// system-command drain, then the ghost update.
    fn frame_full(
        &mut self,
        use_pressed: bool,
        partner_use: bool,
    ) -> Vec<(EntityId, TeleportEvent)> {
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
        let mut deaths = std::mem::take(&mut session.pending_death_events);
        let mut use_edges: HashMap<PlayerId, bool> = HashMap::new();
        if use_pressed {
            use_edges.insert(PlayerId::Local(pawn), true);
        }
        if partner_use {
            use_edges.insert(PlayerId::Remote(PARTNER_CLIENT), true);
        }
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
        // A connected client's pawn: one neutral command a tick, pressing use
        // only when the test says so.
        let remote: Vec<RemotePawnCommand> = if self.partner_live {
            let mut partner_command = command.clone();
            partner_command.use_pressed = partner_use;
            partner_command.movement.use_pressed = partner_use;
            vec![RemotePawnCommand {
                reload_press_tick: None,
                equip: Default::default(),
                real_command: true,
                rejected_activation: None,
                pawn: self.partner,
                owner_client_id: PARTNER_CLIENT,
                weapon: None,
                shot_id: None,
                fire_tick: self.input_tick,
                client_tick: self.input_tick,
                aim_pitch: 0.0,
                command: partner_command,
                start_aim: None,
            }]
        } else {
            Vec::new()
        };
        self.input_tick += 1;
        let auto_close = session.scripting.auto_close_timers.clone();
        let scripting = &mut session.scripting;
        // The production tick's inputs: the real AI tick over the level's
        // navmesh, the mod's descriptors (an AI attack resolves its weapon from
        // them) and its factions.
        let data_registry = ctx.data_registry.borrow();
        let tick = simulate_tick_with_presentation_aim(
            ctx.registry.clone(),
            &app.collision_world,
            &session.hit_zone_store,
            self.nav_graph.as_ref(),
            postretro_collision::EnvironmentView::level_only(ctx.gravity.get()),
            false,
            self.anim_time,
            (0.0, 0.0),
            &mut session.progress_tracker,
            postretro_ai::tick_runner!(&mut self.ai_runtime),
            &app.kinematic_mover_colliders,
            &mut app.kinematic_mover_tick_states,
            &remote,
            &command,
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            DT,
            &mut session.touch_system,
            &data_registry.entities,
            data_registry.entity_types_generation(),
            &data_registry.factions,
            ctx.faction_sentiment.as_ref(),
            data_registry.default_weapon_placement.as_ref(),
            &use_edges,
            &HashMap::new(),
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
            |_, _| {},
            |registry| scripting.evaluate_pending_in_tick_impacts(registry),
            postretro_stage_timing::TimingGate::OFF,
        );
        drop(data_registry);
        deaths.extend(tick.death.iter().cloned());
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
        self.app.dispatch_state_crossings();
        crate::impact_effects::run_end_of_frame_removal_pass(
            &mut ctx.registry.borrow_mut(),
            |removal| {
                let session = self.app.session.as_mut().unwrap();
                let fired = removal.report_to_progress(&mut session.progress_tracker);
                session.pending_death_events.extend(fired);
            },
        );
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
    for monster in ["reference_enemy", "limitator"] {
        assert!(
            !descriptor(monster).ghost,
            "the fixture's monsters do not, so each player sees only their own"
        );
    }

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
        // Echo plate, split pad and swap pad in each copy, the button and the
        // return pad in copy B, the barrier approach in copy A.
        assert_eq!(data.volume_trigger_events().len(), 9);
        assert_eq!(
            data.teleport_events().len(),
            6,
            "both split, the A-to-B split's closets, both swap and the return teleporter bind"
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
        // The north closet's timer: a 15 s wait, then a fire of `closet.a1`,
        // so the levelLoad body itself holds no group step.
        let ReactionDescriptor::Sequence(steps) = reaction(level, "levelLoad") else {
            panic!("the levelLoad timer is a sequence");
        };
        let shape: Vec<(&SequenceTarget, &Value)> =
            steps.iter().map(|step| (&step.id, &step.args)).collect();
        assert_eq!(
            shape,
            [
                (
                    &SequenceTarget::Wait,
                    &serde_json::json!({ "durationMs": 15000, "interruptible": false })
                ),
                (
                    &SequenceTarget::Fire,
                    &serde_json::json!({ "event": "closet.a1" })
                ),
            ]
        );
    }
}

// Both players start in copy A's main room, each at their own `player_spawn`,
// the second marked `coop_only`. The host gets one pawn; the second spawn
// stands empty until a joiner takes it through the host's seat table. The two
// stand level, far enough apart that their capsules cannot touch, outside every
// trigger volume and west of every closet mouth.
#[test]
fn ghost_link_seats_both_players_apart_in_copy_a() {
    let out = tempfile::tempdir().unwrap();
    for map in ["ghost-links", "ghost-links-luau"] {
        let level = Level::install(compile(map, out.path()));
        assert_eq!(level.boot_players, 1, "{map}: two spawns, one host pawn");
        assert_eq!(
            level.partner_placement, 1,
            "{map}: the joiner takes the free spawn"
        );
        let (pawn, partner) = (level.position(level.pawn), level.position(level.partner));
        for (who, at) in [("host", pawn), ("joiner", partner)] {
            assert!(
                at.z > COPY_OFFSET.z * 0.5,
                "{map}: the {who} starts in copy A: {at}"
            );
            // Quake x is engine −z; the closets open on Quake x −64 to 160.
            assert!(
                -at.z * UNITS_PER_METRE < CLOSET_MOUTH_WEST_X,
                "{map}: the {who} starts west of every closet mouth: {at}"
            );
        }
        assert!(
            close(pawn.y, partner.y, 1e-4),
            "{map}: both stand on the floor: {pawn} vs {partner}"
        );
        let radius = |pawn: EntityId| {
            level
                .ctx
                .registry
                .borrow()
                .get_component::<PlayerMovementComponent>(pawn)
                .unwrap()
                .capsule
                .radius
        };
        let (host_radius, joiner_radius) = (radius(level.pawn), radius(level.partner));
        let gap = flat_distance(pawn, partner);
        assert!(
            gap > host_radius + joiner_radius + SPAWN_MARGIN,
            "{map}: the spawns stand {gap} m apart, more than two capsule radii \
             ({host_radius} + {joiner_radius} m) plus {SPAWN_MARGIN} m"
        );
        let bridge = &level.app.session.as_ref().unwrap().trigger_volume_bridge;
        for (trigger, _) in level
            .ctx
            .registry
            .borrow()
            .query_by_component_and_tag(ComponentKind::TriggerVolume, None)
        {
            let (min, max) = bridge.aabb(trigger).expect("trigger volume is bridged");
            for (who, at, r) in [
                ("host", pawn, host_radius),
                ("joiner", partner, joiner_radius),
            ] {
                let clear =
                    at.x + r < min.x || at.x - r > max.x || at.z + r < min.z || at.z - r > max.z;
                assert!(
                    clear,
                    "{map}: the {who}'s capsule starts outside trigger {trigger:?}"
                );
            }
        }
    }
}

// The set-pieces drive what the membership, look, category, replication
// and bus rows observe, in both runtimes. The barrier, a tagged mover on the
// link armed on spawn, ghosts into copy B at full membership; the button in
// copy B drops it and its ghost follows. The echo plate arms the second link,
// so the partner in copy B ghosts into copy A, and both links' colour filters
// and the Ghost bus ramp. The first purple-pad press splits the player
// standing on copy A's pad alone into copy B. The crossing opens copy A's south
// closet at once and copy B's closets three seconds later, and arms the swap
// pads. Each later press flips both players between the copies and narrows
// projectiles; the next press flips them again. Monsters, whose descriptor
// does not opt in, and the viewer's own pawn never ghost. Under the real AI
// tick, the south closet's knight then walks out and hits the player in copy
// A, and copy B's limitators shoot the player in copy B.
#[test]
fn ghost_link_dev_level_runs_the_barrier_puzzle_split_and_flip() {
    let out = tempfile::tempdir().unwrap();
    for map in ["ghost-links", "ghost-links-luau"] {
        run_set_pieces(map, Level::install(compile(map, out.path())));
    }
}

/// Each of `tag`'s closet doors has started (`open`) or holds shut, and each
/// of `monsters` is awake or still sealed in.
fn assert_closet(map: &str, level: &Level, doors: &str, monsters: &str, open: bool, why: &str) {
    for &door in &level.entities(ComponentKind::KinematicMover, doors) {
        assert_eq!(level.started(door), open, "{map}: `{doors}` {why}");
    }
    for &npc in &level.entities(ComponentKind::Health, monsters) {
        assert_eq!(
            level.brain(npc).aggro_armed,
            open,
            "{map}: `{monsters}` {why}"
        );
        if !open {
            assert!(
                level.position(npc).x.abs() > CLOSET_LINE,
                "{map}: `{monsters}` stays in its closet: {why}"
            );
        }
    }
}

/// The classes of the monsters tagged `tag`, in registry order.
fn classes(level: &Level, tag: &str) -> Vec<String> {
    level
        .entities(ComponentKind::Health, tag)
        .iter()
        .map(|&npc| level.class(npc))
        .collect()
}

/// Copy A's and copy B's closets, checked as the level installs: two knights
/// behind each of copy A's doors, a limitator behind each of copy B's, every
/// monster in its closet and asleep.
fn closets(map: &str, level: &Level) {
    assert_eq!(
        level.entities(ComponentKind::Health, "echo_monster").len(),
        6,
        "{map}: six closet monsters"
    );
    assert_eq!(
        classes(level, "echo_monster_a1"),
        ["reference_enemy"; 2],
        "{map}: two knights wait in copy A's north closet"
    );
    assert_eq!(
        classes(level, "echo_monster_a2"),
        ["reference_enemy"; 2],
        "{map}: two knights wait in copy A's south closet"
    );
    assert_eq!(
        classes(level, "echo_monster_b"),
        ["limitator"; 2],
        "{map}: two limitators wait in copy B"
    );
    for &npc in &level.entities(ComponentKind::Health, "echo_monster") {
        assert!(
            level.position(npc).x.abs() > CLOSET_LINE,
            "{map}: every monster starts in a closet, out of the room"
        );
    }
    for (tag, count) in [
        ("echo_closet_a1", 1),
        ("echo_closet_a2", 1),
        ("echo_closet_b", 2),
    ] {
        assert_eq!(
            level.entities(ComponentKind::KinematicMover, tag).len(),
            count,
            "{map}: `{tag}` doors"
        );
    }
    for (doors, monsters) in [
        ("echo_closet_a1", "echo_monster_a1"),
        ("echo_closet_a2", "echo_monster_a2"),
        ("echo_closet_b", "echo_monster_b"),
    ] {
        assert_closet(map, level, doors, monsters, false, "starts shut");
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
    closets(map, &level);
    let monsters = level.entities(ComponentKind::Health, "echo_monster");
    // The closets the split opens: copy A's south closet and copy B's two.
    let fight_doors: Vec<EntityId> = ["echo_closet_a2", "echo_closet_b"]
        .iter()
        .flat_map(|tag| level.entities(ComponentKind::KinematicMover, tag))
        .collect();
    let shut: Vec<Vec3> = fight_doors
        .iter()
        .map(|&door| level.position(door))
        .collect();
    // Both players start in copy A. The partner stands in for one who already
    // crossed: it waits in copy B's room, where echo_b will ghost it.
    let joined = level.position(level.partner);
    level.place_pawn(
        level.partner,
        joined + COPY_OFFSET,
        joined.y - PAD_CLEARANCE,
    );
    level.settle();
    let height = level.position(level.pawn).y;
    // Copy A's spawn: off both purple pads and the blue plate.
    let spawn_a = level.position(level.pawn);
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
    // The split: the first press of copy A's purple pad arms both split
    // teleporters and turns both split pads off, so only the presser crosses,
    // into copy B. The press itself opens no closet.
    let swap = level.entities(ComponentKind::Teleporter, "echo_swap");
    assert_eq!(swap.len(), 2, "{map}: a two-way seamless pair");
    let split = level.entities(ComponentKind::Teleporter, "echo_split");
    assert_eq!(split.len(), 2, "{map}: a split teleporter over each pad");
    let split_pads = level.entities(ComponentKind::TriggerVolume, "split_pad");
    let swap_pads = level.entities(ComponentKind::TriggerVolume, "swap_pad");
    assert_eq!((split_pads.len(), swap_pads.len()), (2, 2), "{map}");
    for &pad in &split_pads {
        assert!(level.trigger_armed(pad), "{map}: split pads start on");
    }
    for &pad in &swap_pads {
        assert!(!level.trigger_armed(pad), "{map}: swap pads start off");
    }
    for &npc in &monsters {
        assert!(
            !level.brain(npc).aggro_armed,
            "{map}: monsters wait for their cue"
        );
    }
    level.place(level.trigger_center("split_pad", false), height);
    // The partner joins copy A, off the pad: the split must leave it there.
    level.place_pawn(level.partner, spawn_a, height);
    for _ in 0..SETTLE {
        assert!(level.frame().is_empty());
    }
    let before = level.position(level.pawn);
    let partner_before = level.position(level.partner);
    assert!(
        level.frame_with(true).is_empty(),
        "{map}: the use queues the split"
    );
    assert_closet(
        map,
        &level,
        "echo_closet_a2",
        "echo_monster_a2",
        false,
        "waits for the crossing, not the press",
    );
    let crossed = level.frame();
    let crossed_at = level.app.script_time;
    assert_eq!(crossed.len(), 1, "{map}: the presser splits");
    assert_eq!(crossed[0].0, level.pawn);
    let partner_after = level.position(level.partner);
    assert!(
        (Vec3::new(partner_after.x, 0.0, partner_after.z)
            - Vec3::new(partner_before.x, 0.0, partner_before.z))
        .length()
            < 1e-2,
        "{map}: the partner off the pad stays in copy A: {partner_before} → {partner_after}"
    );
    let after = level.position(level.pawn);
    assert!(
        (Vec3::new(after.x, 0.0, after.z) - Vec3::new(before.x, 0.0, before.z + COPY_OFFSET.z))
            .length()
            < 1e-2,
        "{map}: the split carries the presser into copy B: {before} → {after}"
    );
    // The arrival latch holds the presser on copy B's pad, inside the armed
    // B-to-A split teleporter, until the crossing disarms both directions.
    for _ in 0..SETTLE {
        assert!(
            level.frame().is_empty(),
            "{map}: the presser stays in copy B"
        );
    }
    for &teleporter in &split {
        assert!(
            !level.teleporter(teleporter).armed,
            "{map}: the crossing disarms the split"
        );
    }
    for &pad in &split_pads {
        assert!(!level.trigger_armed(pad), "{map}: the split runs once");
    }
    for &pad in &swap_pads {
        assert!(
            level.trigger_armed(pad),
            "{map}: the crossing arms the swap"
        );
    }
    for &teleporter in &swap {
        assert!(
            !level.teleporter(teleporter).armed,
            "{map}: the split press does not arm the swap"
        );
    }
    for (_, link) in level.links() {
        assert_eq!(
            link.members,
            GhostMembers::all().bits(),
            "{map}: the split press does not run echo.fight"
        );
    }
    // The crossing opens copy A's south closet at once. The north closet keeps
    // its own cue, and copy B's closets hold through their three-second beat.
    assert_closet(
        map,
        &level,
        "echo_closet_a2",
        "echo_monster_a2",
        true,
        "opens on the crossing",
    );
    assert_closet(
        map,
        &level,
        "echo_closet_a1",
        "echo_monster_a1",
        false,
        "ignores the crossing",
    );
    while level.app.script_time < crossed_at + LIMITATOR_BEAT_SECS - WAVE_BEAT_SLACK {
        assert_closet(
            map,
            &level,
            "echo_closet_b",
            "echo_monster_b",
            false,
            "holds through the beat",
        );
        level.frame();
    }
    while level.app.script_time < crossed_at + LIMITATOR_BEAT_SECS + WAVE_BEAT_SLACK {
        level.frame();
    }
    assert_closet(
        map,
        &level,
        "echo_closet_b",
        "echo_monster_b",
        true,
        "opens three seconds after the crossing",
    );
    assert!(
        level.app.script_time < NORTH_TIMER_SECS - 1.0,
        "{map}: the north closet's timer has not run out yet"
    );
    assert_closet(
        map,
        &level,
        "echo_closet_a1",
        "echo_monster_a1",
        false,
        "still waits for its timer",
    );

    // The flip: the partner, now a connected client's pawn, stands in copy A's
    // middle and the presser on copy B's pad. Each press arms both seamless
    // swap teleporters, so the two trade copies; the first crossing disarms
    // both, and the next press arms them again.
    level.partner_live = true;
    level.place_pawn(level.partner, quake(0.0, 0.0, 0.0), height);
    for _ in 0..SETTLE {
        assert!(level.frame().is_empty(), "{map}: nobody flips unpressed");
    }
    for flip in 0..2 {
        let in_b = flip % 2 == 1;
        let (pawn_before, partner_before) =
            (level.position(level.pawn), level.position(level.partner));
        assert!(
            level.frame_with(true).is_empty(),
            "{map}: the use queues the arm"
        );
        let flipped = level.frame();
        let movers: HashSet<EntityId> = flipped.iter().map(|(pawn, _)| *pawn).collect();
        assert_eq!(
            movers,
            HashSet::from([level.pawn, level.partner]),
            "{map}: flip {flip} moves both players"
        );
        // Copy B is engine −z of copy A: the presser leaves copy B on even
        // flips, the partner leaves copy A.
        let toward = if in_b { COPY_OFFSET } else { -COPY_OFFSET };
        for (pawn, from, delta) in [
            (level.pawn, pawn_before, toward),
            (level.partner, partner_before, -toward),
        ] {
            let at = level.position(pawn);
            assert!(
                flat_distance(at, from + delta) < 1e-2,
                "{map}: flip {flip} carries each player into the other copy: {from} → {at}"
            );
        }
        level.frame();
        level.frame();
        for &teleporter in &swap {
            assert!(
                !level.teleporter(teleporter).armed,
                "{map}: flip {flip}'s crossing disarms both directions"
            );
        }
        for &pad in &swap_pads {
            assert!(level.trigger_armed(pad), "{map}: the swap pads stay on");
        }
        for (_, link) in level.links() {
            assert_eq!(
                link.members,
                (GhostMembers::all() - GhostMembers::PROJECTILES).bits(),
                "{map}: echo.fight hides projectiles"
            );
        }
        for _ in 0..SETTLE {
            assert!(level.frame().is_empty(), "{map}: one press flips once");
        }
    }
    let ghosts = level.ghosts();
    assert!(
        ghosts
            .iter()
            .all(|ghost| ghost.entity != level.pawn && !monsters.contains(&ghost.entity)),
        "{map}: neither the viewer nor a monster ever ghosts: {ghosts:#?}"
    );

    // The fights: the south closet's knight walks out and hits the partner,
    // flipped back into copy A, and copy B's limitators shoot the presser from
    // their closet mouths. The sim runs the real AI tick, so this is the whole
    // path from the crossing to a monster's attack.
    assert!(
        level.position(level.partner).z > COPY_OFFSET.z * 0.5
            && level.position(level.pawn).z < COPY_OFFSET.z * 0.5,
        "{map}: two flips leave the partner in copy A and the presser in copy B"
    );
    let health = |level: &Level, pawn: EntityId| {
        level
            .ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(pawn)
            .unwrap()
            .current
    };
    let (pawn_full, partner_full) = (health(&level, level.pawn), health(&level, level.partner));
    let fighters: Vec<EntityId> = ["echo_monster_a2", "echo_monster_b"]
        .iter()
        .flat_map(|tag| level.entities(ComponentKind::Health, tag))
        .collect();
    let south_knight = level.entities(ComponentKind::Health, "echo_monster_a2");
    let mut left_closet: HashSet<EntityId> = HashSet::new();
    let mut engaged: HashSet<EntityId> = HashSet::new();
    for _ in 0..BATTLE_FRAMES {
        level.frame();
        for &npc in &fighters {
            if level.position(npc).x.abs() < CLOSET_LINE {
                left_closet.insert(npc);
            }
            if level.brain(npc).active_path_names().first() == Some(&"engage") {
                engaged.insert(npc);
            }
        }
    }
    for (&door, closed) in fight_doors.iter().zip(&shut) {
        let drop = closed.y - level.position(door).y;
        assert!(drop > 2.5, "{map}: every opened closet door sinks: {drop}");
    }
    for &npc in &south_knight {
        assert!(
            left_closet.contains(&npc),
            "{map}: the south closet's knight walks out"
        );
    }
    for &npc in &fighters {
        assert!(engaged.contains(&npc), "{map}: every woken monster engages");
    }
    assert!(
        health(&level, level.partner) < partner_full,
        "{map}: copy A's knight hits the player in copy A"
    );
    assert!(
        health(&level, level.pawn) < pawn_full,
        "{map}: copy B's limitators hit the player in copy B"
    );
}

// The north closet's timer, in co-op with both players standing at their
// spawns in copy A: with nobody near the barrier, its door holds and its
// knights sleep for fifteen seconds from level load, then the door sinks and
// exactly its two knights wake, walk out and engage. The south closet, which
// waits for the split, stays shut with its two knights asleep inside, and so
// do copy B's closets.
#[test]
fn ghost_link_north_closet_opens_fifteen_seconds_after_level_load() {
    let out = tempfile::tempdir().unwrap();
    for map in ["ghost-links", "ghost-links-luau"] {
        run_north_timer(map, Level::install(compile(map, out.path())));
    }
}

fn run_north_timer(map: &str, mut level: Level) {
    closets(map, &level);
    level.partner_live = true;
    level.settle();
    while level.app.script_time < NORTH_TIMER_SECS - WAVE_BEAT_SLACK {
        assert_closet(
            map,
            &level,
            "echo_closet_a1",
            "echo_monster_a1",
            false,
            "holds until its timer",
        );
        assert!(level.frame().is_empty());
    }
    while level.app.script_time < NORTH_TIMER_SECS + WAVE_BEAT_SLACK {
        level.frame();
    }
    assert_closet(
        map,
        &level,
        "echo_closet_a1",
        "echo_monster_a1",
        true,
        "opens fifteen seconds after level load",
    );
    assert_closet(
        map,
        &level,
        "echo_closet_a2",
        "echo_monster_a2",
        false,
        "ignores the north closet's timer",
    );
    let awake: Vec<EntityId> = level
        .entities(ComponentKind::Health, "echo_monster")
        .into_iter()
        .filter(|&npc| level.brain(npc).aggro_armed)
        .collect();
    assert_eq!(
        awake,
        level.entities(ComponentKind::Health, "echo_monster_a1"),
        "{map}: the timer wakes the north closet's two knights and nobody else"
    );
    assert_north_knights_engage(map, &mut level);
    for (doors, monsters) in [
        ("echo_closet_a2", "echo_monster_a2"),
        ("echo_closet_b", "echo_monster_b"),
    ] {
        assert_closet(map, &level, doors, monsters, false, "waits for a split");
    }
    hidden_return(map, &level, "while nobody presses the barrier button");
}

/// The north closet's door sinks open and both its knights walk out and
/// engage a player, under the real AI tick.
fn assert_north_knights_engage(map: &str, level: &mut Level) {
    let door = only(map, level, ComponentKind::KinematicMover, "echo_closet_a1");
    let shut = level.position(door).y;
    let knights = level.entities(ComponentKind::Health, "echo_monster_a1");
    let mut left_closet: HashSet<EntityId> = HashSet::new();
    let mut engaged: HashSet<EntityId> = HashSet::new();
    for _ in 0..BATTLE_FRAMES {
        level.frame();
        for &npc in &knights {
            if level.position(npc).x.abs() < CLOSET_LINE {
                left_closet.insert(npc);
            }
            if level.brain(npc).active_path_names().first() == Some(&"engage") {
                engaged.insert(npc);
            }
        }
    }
    let drop = shut - level.position(door).y;
    assert!(
        drop > 2.5,
        "{map}: the north closet door sinks open: {drop}"
    );
    for &npc in &knights {
        assert!(
            left_closet.contains(&npc) && engaged.contains(&npc),
            "{map}: every north closet knight walks out and engages"
        );
    }
}

// The barrier approach: the host walking up to copy A's closed barrier opens
// the north closet at once, long before its timer. When the timer runs out
// later, its fire starts nothing again: the open door stays where it stopped.
#[test]
fn ghost_link_barrier_approach_opens_the_north_closet_early() {
    let out = tempfile::tempdir().unwrap();
    for map in ["ghost-links", "ghost-links-luau"] {
        run_barrier_approach(map, Level::install(compile(map, out.path())));
    }
}

fn run_barrier_approach(map: &str, mut level: Level) {
    closets(map, &level);
    let barrier = level.barrier();
    level.settle();
    let height = level.position(level.pawn).y;
    assert_closet(
        map,
        &level,
        "echo_closet_a1",
        "echo_monster_a1",
        false,
        "waits while nobody approaches",
    );
    level.place(level.trigger_center("barrier_approach", false), height);
    level.frame();
    level.frame();
    let opened_at = level.app.script_time;
    assert!(
        !level.started(barrier),
        "{map}: walking up leaves the barrier standing"
    );
    assert_closet(
        map,
        &level,
        "echo_closet_a1",
        "echo_monster_a1",
        true,
        "opens when a player walks up to the barrier",
    );
    assert!(
        opened_at < NORTH_TIMER_SECS - 10.0,
        "{map}: the approach beats the timer by far ({opened_at:.2} s)"
    );
    assert_north_knights_engage(map, &mut level);
    let door = only(map, &level, ComponentKind::KinematicMover, "echo_closet_a1");
    let open = level.position(door);
    while level.app.script_time < NORTH_TIMER_SECS + 1.0 {
        level.frame();
        assert!(
            (level.position(door) - open).length() < 1e-4,
            "{map}: the timer's fire does not move the open door"
        );
    }
    for (doors, monsters) in [
        ("echo_closet_a2", "echo_monster_a2"),
        ("echo_closet_b", "echo_monster_b"),
    ] {
        assert_closet(map, &level, doors, monsters, false, "waits for a split");
    }
    hidden_return(map, &level, "while nobody presses the barrier button");
}

/// The final wave's closet door and monsters, checked as the level installs:
/// one purple door in back room A's east wall, two knights and two limitators
/// behind it, asleep.
fn final_wave(map: &str, level: &Level) -> (Vec<EntityId>, Vec<EntityId>) {
    let doors = level.entities(ComponentKind::KinematicMover, "final_closet_door");
    assert_eq!(doors.len(), 1, "{map}: one final-wave closet");
    let monsters = level.entities(ComponentKind::Health, "final_wave");
    let mut classes: Vec<String> = monsters.iter().map(|&npc| level.class(npc)).collect();
    classes.sort_unstable();
    assert_eq!(
        classes,
        [
            "limitator",
            "limitator",
            "reference_enemy",
            "reference_enemy"
        ],
        "{map}: the final wave mixes knights and limitators"
    );
    for &npc in &monsters {
        assert!(
            !level.brain(npc).aggro_armed,
            "{map}: the final wave sleeps"
        );
    }
    (doors, monsters)
}

/// The one map entity of `kind` tagged `tag`.
fn only(map: &str, level: &Level, kind: ComponentKind, tag: &str) -> EntityId {
    let found = level.entities(kind, tag);
    assert_eq!(found.len(), 1, "{map}: one `{tag}`");
    found[0]
}

/// The host splits from copy A's purple pad into copy B, then presses the pad
/// there and flips back alone: whatever stands outside copy B's room, the
/// partner in back room B included, stays. Returns the split's script time.
fn split_then_flip_back(map: &str, level: &mut Level, height: f32) -> f64 {
    level.place(level.trigger_center("split_pad", false), height);
    for _ in 0..SETTLE {
        assert!(level.frame().is_empty());
    }
    level.frame_with(true);
    let crossed = level.frame();
    let crossed_at = level.app.script_time;
    assert_eq!(crossed.len(), 1, "{map}: the host splits alone");
    assert_eq!(crossed[0].0, level.pawn);
    for _ in 0..SETTLE {
        assert!(level.frame().is_empty());
    }
    level.frame_with(true);
    let flipped = level.frame();
    assert_eq!(flipped.len(), 1, "{map}: the host flips back alone");
    assert_eq!(flipped[0].0, level.pawn);
    level.frame();
    level.frame();
    crossed_at
}

/// Back room B's return pedestal and its pad, while the pedestal is hidden:
/// not started, its top below the floor, its pad disarmed. Returns both.
fn hidden_return(map: &str, level: &Level, why: &str) -> (EntityId, EntityId) {
    let pedestal = only(map, level, ComponentKind::KinematicMover, "return_pedestal");
    let pad = only(map, level, ComponentKind::TriggerVolume, "return_pad");
    let top = level.position(pedestal).y + PEDESTAL_HALF_HEIGHT;
    assert!(
        !level.started(pedestal) && top < 0.0,
        "{map}: the return pedestal stays below the floor {why}: top at {top}"
    );
    assert!(
        !level.trigger_armed(pad),
        "{map}: the return pad stays off {why}"
    );
    (pedestal, pad)
}

/// The barrier button's press raised the pedestal tagged `return_pedestal`
/// and armed its pad at once; the pedestal then stands out of the floor.
fn assert_return_rises(map: &str, level: &mut Level, pedestal: EntityId, pad: EntityId, sunk: f32) {
    assert!(
        level.started(pedestal),
        "{map}: the barrier press raises the return pedestal"
    );
    assert!(
        level.trigger_armed(pad),
        "{map}: the barrier press arms the return pad"
    );
    for _ in 0..RISE_FRAMES {
        level.frame();
    }
    let rise = level.position(pedestal).y - sunk;
    assert!(
        close(rise, 56.0 / UNITS_PER_METRE, 0.02),
        "{map}: the pedestal stands out of the floor: {rise}"
    );
    assert!(
        level.position(pedestal).y - PEDESTAL_HALF_HEIGHT > -1e-3,
        "{map}: the risen pedestal sits on the floor"
    );
}

/// The partner presses the return pad in back room B and alone arrives at
/// `return_a` in room A; returns the script time of the arrival.
fn partner_returns(map: &str, level: &mut Level, height: f32) -> f64 {
    let gate = only(map, level, ComponentKind::Teleporter, "echo_return");
    level.place_pawn(
        level.partner,
        level.trigger_center("return_pad", true),
        height,
    );
    for _ in 0..SETTLE {
        assert!(level.frame().is_empty());
    }
    assert!(!level.armed(gate), "{map}: the return waits for a press");
    let host_before = level.position(level.pawn);
    level.partner_frame_with_use();
    let returned = level.frame();
    let arrived_at = level.app.script_time;
    assert_eq!(returned.len(), 1, "{map}: the press sends one player");
    assert_eq!(returned[0].0, level.partner, "{map}: the presser returns");
    assert!(
        flat_distance(level.position(level.partner), quake(-96.0, 0.0, 0.0)) < 0.05,
        "{map}: the partner lands at return_a: {}",
        level.position(level.partner)
    );
    assert!(
        flat_distance(level.position(level.pawn), host_before) < 1e-2,
        "{map}: the host stays where it is"
    );
    level.frame();
    level.frame();
    assert!(!level.armed(gate), "{map}: the arrival disarms the return");
    arrived_at
}

/// From `fired_at`, the frame the final wave fired, the closet doors hold shut
/// and the monsters sleep through the three-second beat, then the doors start
/// and the monsters wake.
fn assert_wave_beat(
    map: &str,
    level: &mut Level,
    fired_at: f64,
    doors: &[EntityId],
    monsters: &[EntityId],
) {
    while level.app.script_time < fired_at + WAVE_BEAT_SECS - WAVE_BEAT_SLACK {
        for &door in doors {
            assert!(
                !level.started(door),
                "{map}: the closet doors hold through the beat ({:.2} s in)",
                level.app.script_time - fired_at
            );
        }
        for &npc in monsters {
            assert!(
                !level.brain(npc).aggro_armed,
                "{map}: the wave sleeps through the beat"
            );
        }
        level.frame();
    }
    while level.app.script_time < fired_at + WAVE_BEAT_SECS + WAVE_BEAT_SLACK {
        level.frame();
    }
    for &door in doors {
        assert!(level.started(door), "{map}: the doors start after the beat");
    }
    for &npc in monsters {
        assert!(
            level.brain(npc).aggro_armed,
            "{map}: the wave wakes after the beat"
        );
    }
}

/// The final wave fights: the door sinks open, the knights walk out and every
/// monster engages a player in room A, under the real AI tick. Whether a hit
/// lands is left to play: a limitator firing past a knight can start the
/// monsters fighting each other.
fn assert_wave_engages(map: &str, level: &mut Level, doors: &[EntityId], monsters: &[EntityId]) {
    let shut: Vec<f32> = doors.iter().map(|&door| level.position(door).y).collect();
    let players = [level.pawn, level.partner];
    let classes: HashMap<EntityId, String> = monsters
        .iter()
        .map(|&npc| (npc, level.class(npc)))
        .collect();
    let mut engaged: HashSet<EntityId> = HashSet::new();
    let mut left_closet: HashSet<EntityId> = HashSet::new();
    for _ in 0..BATTLE_FRAMES {
        level.frame();
        // A monster can fall to the others' crossfire; the dead are removed.
        let alive: Vec<EntityId> = monsters
            .iter()
            .copied()
            .filter(|&npc| level.ctx.registry.borrow().exists(npc))
            .collect();
        for &npc in &alive {
            let brain = level.brain(npc);
            if brain.active_path_names().first() == Some(&"engage")
                && brain
                    .acquired_target
                    .is_some_and(|target| players.contains(&target))
            {
                engaged.insert(npc);
            }
            // Quake x is engine −z.
            if -level.position(npc).z * UNITS_PER_METRE < FINAL_CLOSET_X {
                left_closet.insert(npc);
            }
        }
    }
    for &npc in monsters {
        if classes[&npc] == "reference_enemy" {
            assert!(
                left_closet.contains(&npc),
                "{map}: every final-wave knight walks out of the closet"
            );
        }
    }
    for (&door, closed) in doors.iter().zip(&shut) {
        let drop = closed - level.position(door).y;
        assert!(
            drop > 2.5,
            "{map}: every final closet door sinks open: {drop}"
        );
    }
    for &npc in monsters {
        assert!(
            engaged.contains(&npc),
            "{map}: every final-wave monster engages a player ({})",
            classes[&npc]
        );
    }
}

// Co-op, the whole run: the host starts in copy A and the partner, a connected
// client's pawn, in back room B. Until the partner presses the button there,
// the return pedestal stays hidden below the floor and its pad off. The press
// drops copy A's barrier and at once raises the pedestal and arms its pad. The
// host splits into copy B and flips back alone, since the partner stands
// outside the swap; copy B's limitators wake three seconds after the split.
// With the limitators still alive, the return pad's press sends only the
// partner to room A. The arrival reunites the players: the flip turns off for
// good, and three seconds later back room A's closet opens and the final wave
// engages. A press of copy A's purple pad afterwards flips nobody.
#[test]
fn ghost_link_coop_return_reunites_and_starts_the_final_wave() {
    let out = tempfile::tempdir().unwrap();
    for map in ["ghost-links", "ghost-links-luau"] {
        run_coop_return(map, Level::install(compile(map, out.path())));
    }
}

fn run_coop_return(map: &str, mut level: Level) {
    let (doors, monsters) = final_wave(map, &level);
    level.partner_live = true;
    let barrier = level.barrier();
    level.settle();
    let height = level.position(level.pawn).y;
    level.place_pawn(
        level.partner,
        level.trigger_center("vault_button", true),
        height,
    );
    for _ in 0..SETTLE {
        assert!(level.frame().is_empty());
    }
    let (pedestal, pad) = hidden_return(map, &level, "before the barrier press");
    let sunk = level.position(pedestal).y;
    level.partner_frame_with_use();
    level.frame();
    assert!(
        level.started(barrier),
        "{map}: the button in back room B drops copy A's barrier"
    );
    assert_return_rises(map, &mut level, pedestal, pad, sunk);
    let crossed_at = split_then_flip_back(map, &mut level, height);
    while level.app.script_time < crossed_at + LIMITATOR_BEAT_SECS + WAVE_BEAT_SLACK {
        level.frame();
    }
    assert_closet(
        map,
        &level,
        "echo_closet_b",
        "echo_monster_b",
        true,
        "opens three seconds after the split",
    );
    level.place(quake(HOST_STAND.0, HOST_STAND.1, 0.0), height);
    assert!(
        !level
            .entities(ComponentKind::Health, "echo_monster_b")
            .is_empty(),
        "{map}: the return does not wait for copy B's clear"
    );
    for &door in &doors {
        assert!(!level.started(door), "{map}: the wave waits for a return");
    }
    let swap = level.entities(ComponentKind::Teleporter, "echo_swap");
    let swap_pads = level.entities(ComponentKind::TriggerVolume, "swap_pad");
    for &pad in &swap_pads {
        assert!(
            level.trigger_armed(pad),
            "{map}: the flip runs until the players reunite"
        );
    }
    let arrived_at = partner_returns(map, &mut level, height);
    for &pad in &swap_pads {
        assert!(
            !level.trigger_armed(pad),
            "{map}: the reunion ends the flip"
        );
    }
    for &teleporter in &swap {
        assert!(!level.armed(teleporter), "{map}: no swap stays armed");
    }
    level.place_pawn(
        level.partner,
        quake(PARTNER_STAND.0, PARTNER_STAND.1, 0.0),
        height,
    );
    assert_wave_beat(map, &mut level, arrived_at, &doors, &monsters);
    assert_wave_engages(map, &mut level, &doors, &monsters);

    // After the reunion, a press of copy A's purple pad flips nobody.
    level.place(level.trigger_center("swap_pad", false), height);
    for _ in 0..SETTLE {
        assert!(level.frame().is_empty());
    }
    assert!(level.frame_with(true).is_empty());
    for _ in 0..SETTLE {
        assert!(
            level.frame().is_empty(),
            "{map}: the swap pad stays off after the reunion"
        );
    }
}
