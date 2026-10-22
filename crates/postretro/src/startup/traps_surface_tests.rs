// Projectile traps as compiled content/dev levels (`trap-volley`, `trap-wave` twins), host-ticked.
// See: context/lib/entity_model.md §Components (ProjectileTrap) · context/lib/scripting.md §12

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use glam::Vec3;
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::components::projectile_trap::{ProjectileTrapComponent, TrapRequest};
use postretro_entities::{ComponentKind, Emitter, EntityId, ScriptCtx, ScriptError, Transform};
use postretro_level_loader::LevelWorld;
use postretro_physics::movement::MovementInput;
use postretro_scripting_core::primitive_adapters::StoreSchemaJson;
use postretro_scripting_core::primitives_registry::{ContextScope, PrimitiveRegistry};
use postretro_scripting_core::reaction_dispatch::dispatch_deferred_named_events_with_sequences;
use postretro_sim::ai_host::AiTickResult;
use postretro_sim::residual_drain::ResidualRegistries;
use postretro_sim::sim::{PostMovementCommand, SimCommand, TriggerTickContext, simulate_tick};
use postretro_sim::trigger_system::PlayerId;
use serde_json::Value;

use super::teleporters_surface_tests::{
    JS_CAPTURE, LUAU_CAPTURE, canonical, compile, register_teleporter_and_trigger_primitives,
};
use crate::coop_fixture_tests::initialize_dev_mod;
use crate::frame_loop::named_drains::{NamedFrameEvents, drain_frame_named_events};
use crate::startup::StartupTimings;
use crate::startup::lifecycle::tests::{script_ctx, test_app};
use crate::startup::lifecycle::{WorldInstallHandles, install_world_cpu};

const DT: f32 = 1.0 / 60.0;
/// Frames that let a placed pawn settle onto the floor before it acts.
const SETTLE: usize = 30;
/// Metres above the standing height a placed pawn drops from.
const PAD_CLEARANCE: f32 = 0.5;
const VOLLEY_WEAPON: &str = "enemy_rifle";

/// One trap shot a frame produced: the tick it launched on, its trap, and the
/// presentation class its mirror carries.
#[derive(Debug, Clone, PartialEq)]
struct Shot {
    tick: u32,
    trap: EntityId,
    projectile: EntityId,
    class: String,
    /// The projectile as launched, before it flies or expires.
    launched: ProjectileComponent,
}

/// One installed trap level on a single-player host.
struct Level {
    app: crate::App,
    ctx: ScriptCtx,
    pawn: EntityId,
    /// The raw value `setupLevel` returned, as canonical JSON, for a level that
    /// carries a data script.
    wire: Option<String>,
    captured: Rc<RefCell<Option<Value>>>,
    /// Authoritative ticks run so far; the first frame runs tick 0.
    tick: u32,
    anim_time: f64,
    shots: Vec<Shot>,
    /// `activate` emissions on trap entities, per tick.
    fire_cues: Vec<(u32, EntityId, Option<postretro_foundation::ShotId>)>,
}

impl Level {
    fn install(world: LevelWorld) -> Self {
        Self::install_as(world, false)
    }

    /// Install `world` as a host, or as a connected client's install does
    /// when `client` is set.
    fn install_as(world: LevelWorld, client: bool) -> Self {
        let mut app = test_app();
        let ctx = script_ctx(&app);
        let captured: Rc<RefCell<Option<Value>>> = Rc::new(RefCell::new(None));
        Self::prepare(&mut app, &ctx, &captured);
        let wire = Self::load(&mut app, &ctx, &captured, world, client);
        let pawn = Self::boot_pawn(&ctx);
        Self {
            app,
            ctx,
            pawn,
            wire,
            captured,
            tick: 0,
            anim_time: 0.0,
            shots: Vec::new(),
            fire_cues: Vec::new(),
        }
    }

    /// Unload this level the way a level change does, then install `world`
    /// into the same session.
    fn reinstall(&mut self, world: LevelWorld) {
        self.app.unload_level();
        self.wire = Self::load(&mut self.app, &self.ctx, &self.captured, world, false);
        self.pawn = Self::boot_pawn(&self.ctx);
    }

    fn boot_pawn(ctx: &ScriptCtx) -> EntityId {
        ctx.registry
            .borrow()
            .local_player_pawn()
            .expect("the boot pawn spawned at player_spawn")
    }

    /// The production dev mod and the session registrars, once per app.
    fn prepare(app: &mut crate::App, ctx: &ScriptCtx, captured: &Rc<RefCell<Option<Value>>>) {
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
        initialize_dev_mod(app, &primitives);
        register_teleporter_and_trigger_primitives(app, ctx);
        // `test_app` builds empty registries; `Session::build` adds this one.
        postretro_sim::traps::commands::register_sequenced_trap_primitives(
            &mut app.session.as_mut().unwrap().scripting.sequence_registry,
            ctx.clone(),
        );
    }

    /// Install `world` as level load does; returns the captured `setupLevel` wire.
    fn load(
        app: &mut crate::App,
        ctx: &ScriptCtx,
        captured: &Rc<RefCell<Option<Value>>>,
        world: LevelWorld,
        client: bool,
    ) -> Option<String> {
        let mut world = world;
        if let Some(script) = world.data_script.as_mut() {
            let luau = script.source_path.ends_with(".luau");
            script
                .compiled_bytes
                .extend_from_slice(if luau { LUAU_CAPTURE } else { JS_CAPTURE }.as_bytes());
        }

        let content_root = super::teleporters_surface_tests::workspace().join("content/dev");
        let nav_graph = crate::startup::lifecycle::install_world_gravity_and_nav(&world, ctx);
        let products = {
            let session = app.session.as_mut().unwrap();
            session.light_bridge.populate_from_level(
                &world.lights,
                &mut ctx.registry.borrow_mut(),
                0,
            );
            let handles = WorldInstallHandles {
                world: &world,
                script_ctx: ctx,
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
                suppress_host_replicated: client,
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

        captured
            .borrow_mut()
            .take()
            .map(|raw| serde_json::to_string(&canonical(raw)).unwrap())
    }

    fn traps(&self, tag: &str) -> Vec<EntityId> {
        let registry = self.ctx.registry.borrow();
        let mut found: Vec<EntityId> = registry
            .query_by_component_and_tag(ComponentKind::ProjectileTrap, Some(tag))
            .map(|(id, _)| id)
            .collect();
        found.sort_unstable();
        found
    }

    fn trap(&self, tag: &str) -> EntityId {
        let found = self.traps(tag);
        assert_eq!(found.len(), 1, "one `{tag}` trap");
        found[0]
    }

    fn trigger(&self, name: &str) -> (EntityId, Vec3, Vec3) {
        let registry = self.ctx.registry.borrow();
        let bridge = &self.app.session.as_ref().unwrap().trigger_volume_bridge;
        let found: Vec<_> = registry
            .iter_with_kind(ComponentKind::TriggerVolume)
            .filter(|(id, _)| bridge.name(*id) == Some(name))
            .map(|(id, _)| {
                let (min, max) = bridge.aabb(id).expect("trigger volume is bridged");
                (id, min, max)
            })
            .collect();
        assert_eq!(found.len(), 1, "one trigger named `{name}`");
        found[0]
    }

    fn entered(&self, trigger: EntityId) -> bool {
        self.app
            .session
            .as_ref()
            .unwrap()
            .trigger_system
            .paired_enters()
            .contains(&(trigger, PlayerId::Local(self.pawn)))
    }

    fn position(&self) -> Vec3 {
        self.ctx
            .registry
            .borrow()
            .get_component::<Transform>(self.pawn)
            .unwrap()
            .position
    }

    /// Put the pawn at rest above `at`, from where it drops onto the floor.
    fn place(&self, at: Vec3, grounded: f32) {
        let mut registry = self.ctx.registry.borrow_mut();
        let mut transform = *registry.get_component::<Transform>(self.pawn).unwrap();
        transform.position = Vec3::new(at.x, grounded + PAD_CLEARANCE, at.z);
        registry.set_component(self.pawn, transform).unwrap();
        let mut movement = registry
            .get_component::<postretro_foundation::PlayerMovementComponent>(self.pawn)
            .unwrap()
            .clone();
        movement.velocity = Vec3::ZERO;
        registry.set_component(self.pawn, movement).unwrap();
    }

    fn component(&self, trap: EntityId) -> ProjectileTrapComponent {
        self.ctx
            .registry
            .borrow()
            .get_component::<ProjectileTrapComponent>(trap)
            .unwrap()
            .clone()
    }

    fn shot_ticks(&self, trap: EntityId) -> Vec<u32> {
        self.shots
            .iter()
            .filter(|shot| shot.trap == trap)
            .map(|shot| shot.tick)
            .collect()
    }

    /// One single-player host frame of one fixed tick: the authoritative tick,
    /// then the frame-end drains in `frame_loop` order. Returns this tick's shots.
    fn frame(&mut self) -> Vec<Shot> {
        let ctx = self.ctx.clone();
        let app = &mut self.app;
        let session = app.session.as_mut().unwrap();
        session.scripting.scheduler.begin_frame();
        let mut deaths = std::mem::take(&mut session.pending_death_events);
        let use_edges: HashMap<PlayerId, bool> = HashMap::new();
        let command = SimCommand {
            input_tick: self.tick,
            movement: MovementInput {
                wish_dir: glam::Vec2::ZERO,
                jump_pressed: false,
                dash_pressed: false,
                running: false,
                crouch_intent: false,
                use_pressed: false,
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
            use_pressed: false,
            drop_pressed: false,
        };
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
        let shots: Vec<Shot> = tick
            .trap_projectile_spawns
            .iter()
            .map(|spawn| {
                let launched = ctx
                    .registry
                    .borrow()
                    .get_component::<ProjectileComponent>(spawn.projectile)
                    .unwrap()
                    .clone();
                Shot {
                    tick: self.tick,
                    trap: launched.owner_pawn,
                    projectile: spawn.projectile,
                    class: spawn.descriptor_class.clone(),
                    launched,
                }
            })
            .collect();
        for emission in tick.weapon.iter().filter(|e| e.address == "activate") {
            if let Emitter::Entity { id, .. } = emission.emitter {
                self.fire_cues.push((self.tick, id, emission.shot_id));
            }
        }
        self.shots.extend(shots.iter().cloned());
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
        self.tick += 1;
        shots
    }

    fn run(&mut self, frames: usize) {
        for _ in 0..frames {
            self.frame();
        }
    }
}

fn ms_ticks(ms: f32) -> u32 {
    (f64::from(ms) * 1000.0 / 16_667.0).ceil().max(1.0) as u32
}

// First slice: a `start_on_spawn` trap in a level with no script fires on its
// own from install, every shot credits no attacker, and every shot surfaces a
// presentation mirror classed by its weapon plus an `activate` cue on the trap.
#[test]
fn volley_sentry_starts_on_spawn_with_attackerless_mirrored_shots() {
    let out = tempfile::tempdir().unwrap();
    let mut level = Level::install(compile("trap-volley", out.path()));
    assert!(level.wire.is_none(), "trap-volley carries no data script");
    let sentry = level.trap("sentry");
    assert!(
        level.component(sentry).is_armed(),
        "enemy_rifle arms the trap"
    );
    let interval = ms_ticks(2000.0);
    let phase = ms_ticks(500.0);
    level.run((phase + 2 * interval + 1) as usize);
    assert_eq!(
        level.shot_ticks(sentry),
        vec![phase, phase + interval, phase + 2 * interval],
        "its first execution lands `phase` after the first authoritative tick"
    );
    for shot in level.shots.iter().filter(|shot| shot.trap == sentry) {
        assert_eq!(shot.class, VOLLEY_WEAPON);
        let projectile = &shot.launched;
        assert!(projectile.attackerless);
        assert_eq!(projectile.credit_source, "enemy.rifle");
        assert!(
            level
                .fire_cues
                .iter()
                .any(|(tick, trap, id)| *tick == shot.tick
                    && *trap == sentry
                    && *id == projectile.source_shot),
            "an activate cue carries the shot's id"
        );
    }
    for tag in ["volley", "updown"] {
        for trap in level.traps(tag) {
            assert!(
                level.shot_ticks(trap).is_empty(),
                "`{tag}` waits for its plate"
            );
        }
    }
}

// A connected client installs the same armed traps but starts none:
// `start_on_spawn` is a host request, and the host's shots reach the client
// as mirrored projectiles.
#[test]
fn client_install_arms_traps_without_starting_them() {
    let out = tempfile::tempdir().unwrap();
    let level = Level::install_as(compile("trap-volley", out.path()), true);
    let sentry = level.component(level.trap("sentry"));
    assert!(sentry.is_armed(), "the client resolves the same weapon");
    assert_eq!(sentry.pending, Default::default(), "nothing latched");
    assert!(
        sentry.runtime.unwrap().schedule.is_none(),
        "no schedule on the client"
    );
    let host = Level::install(compile("trap-volley", out.path()));
    assert!(
        host.component(host.trap("sentry")).pending.start.is_some(),
        "the host latches the sentry's start"
    );
}

// Every map spelling of the facing: `angle`, `angles`, `mangle`, and the
// legacy up (-1) and down (-2) sentinels.
#[test]
fn volley_traps_fire_along_their_authored_facing() {
    let out = tempfile::tempdir().unwrap();
    let mut level = Level::install(compile("trap-volley", out.path()));
    let volley = level.traps("volley");
    assert_eq!(volley.len(), 4);
    let updown = level.traps("updown");
    assert_eq!(updown.len(), 2);
    for &trap in volley.iter().chain(&updown) {
        postretro_sim::traps::issue_trap_request(
            &mut level.ctx.registry.borrow_mut(),
            trap,
            TrapRequest::Fire,
        );
    }
    let shots = level.frame();
    assert_eq!(shots.len(), 6);
    for shot in shots {
        let projectile = &shot.launched;
        let origin = level
            .ctx
            .registry
            .borrow()
            .get_component::<Transform>(shot.trap)
            .unwrap()
            .position;
        let launched = level
            .ctx
            .registry
            .borrow()
            .get_component::<Transform>(shot.projectile)
            .unwrap()
            .position;
        assert!(
            launched.abs_diff_eq(origin, 1e-5),
            "it fires from its origin"
        );
        // Quake yaw 180 faces engine +Z; -1 is up (+Y), -2 down (-Y).
        let expected = if volley.contains(&shot.trap) {
            Vec3::Z
        } else if origin.y < 1.0 {
            Vec3::Y
        } else {
            Vec3::NEG_Y
        };
        assert!(
            Vec3::from(projectile.direction).abs_diff_eq(expected, 1e-5),
            "trap {:?} fires along {:?}, expected {expected}",
            shot.trap,
            projectile.direction
        );
        assert_eq!(projectile.speed, 20.0, "enemy_rifle's speed");
        assert_eq!(projectile.damage, 10.0, "enemy_rifle's damage");
    }
}

// The no-script fixture, with a player entering each plate, fires every
// tagged trap on the Enter tick.
#[test]
fn volley_plates_fire_their_tagged_traps_on_the_enter_tick() {
    let out = tempfile::tempdir().unwrap();
    let mut level = Level::install(compile("trap-volley", out.path()));
    level.run(SETTLE);
    let height = level.position().y;
    for (plate, tag) in [("volley_plate", "volley"), ("updown_plate", "updown")] {
        let (trigger, min, max) = level.trigger(plate);
        level.place((min + max) * 0.5, height);
        let traps = level.traps(tag);
        let mut enter_tick = None;
        for _ in 0..SETTLE {
            let shots = level.frame();
            if level.entered(trigger) {
                let fired: Vec<EntityId> = shots
                    .iter()
                    .filter(|shot| traps.contains(&shot.trap))
                    .map(|shot| shot.trap)
                    .collect();
                assert_eq!(
                    fired.len(),
                    traps.len(),
                    "every `{tag}` trap fires on the Enter tick"
                );
                enter_tick = Some(level.tick - 1);
                break;
            }
        }
        let enter_tick = enter_tick.expect("the pawn entered the plate");
        for &trap in &traps {
            assert_eq!(level.shot_ticks(trap), vec![enter_tick]);
        }
    }
}

const STEP_MS: f32 = 250.0;
const GAP_MS: f32 = 1500.0;
const PASSES: usize = 3;

impl Level {
    /// The `wave` traps in the scripts' sweep order: ascending engine x.
    fn wave(&self) -> Vec<EntityId> {
        let mut traps = self.traps("wave");
        let registry = self.ctx.registry.borrow();
        traps.sort_by(|a, b| {
            let x = |id: &EntityId| registry.get_component::<Transform>(*id).unwrap().position.x;
            x(a).total_cmp(&x(b))
        });
        traps
    }

    /// Settle, then step onto `plate` and run frames until its Enter fires.
    /// Returns the Enter tick.
    fn enter(&mut self, plate: &str) -> u32 {
        self.run(SETTLE);
        let height = self.position().y;
        let (trigger, min, max) = self.trigger(plate);
        self.place((min + max) * 0.5, height);
        for _ in 0..SETTLE {
            self.frame();
            if self.entered(trigger) {
                return self.tick - 1;
            }
        }
        panic!("the pawn never entered `{plate}`");
    }
}

/// The sweep's shot ticks per trap under the timing rule: the first step
/// acts on the Enter tick; every later step one tick after its summed waits.
fn expected_sweep(enter: u32, traps: usize) -> Vec<Vec<u32>> {
    let (step, gap) = (ms_ticks(STEP_MS), ms_ticks(GAP_MS));
    let mut per_trap = vec![Vec::new(); traps];
    let mut waited = 0;
    for pass in 0..PASSES {
        for (index, ticks) in per_trap.iter_mut().enumerate() {
            if pass > 0 && index == 0 {
                waited += gap;
            } else if index > 0 {
                waited += step;
            }
            ticks.push(if waited == 0 {
                enter
            } else {
                enter + waited + 1
            });
        }
    }
    per_trap
}

fn sweep_shots(map: &'static str) -> (Vec<Vec<u32>>, u32, String) {
    let out = tempfile::tempdir().unwrap();
    let mut level = Level::install(compile(map, out.path()));
    let wave = level.wave();
    assert_eq!(wave.len(), 5, "five `wave` traps");
    let enter = level.enter("sweep_plate");
    level.run(600);
    let shots = wave.iter().map(|&trap| level.shot_ticks(trap)).collect();
    (
        shots,
        enter,
        level.wire.clone().expect("the wave level has a script"),
    )
}

// The `trap-wave` twins fire their traps in sweep order on the shot ticks
// their constants imply, and the two runtimes record the same ticks and wire.
#[test]
fn wave_sweep_fires_in_order_on_its_constant_ticks_in_both_runtimes() {
    let (ts, ts_enter, ts_wire) = sweep_shots("trap-wave");
    let (luau, luau_enter, luau_wire) = sweep_shots("trap-wave-luau");
    assert_eq!(ts_wire, luau_wire, "TS and Luau setupLevel wire diverged");
    assert_eq!(ts, expected_sweep(ts_enter, 5), "TypeScript sweep ticks");
    assert_eq!(
        luau.iter()
            .map(|ticks| ticks
                .iter()
                .map(|tick| tick - luau_enter)
                .collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        ts.iter()
            .map(|ticks| ticks.iter().map(|tick| tick - ts_enter).collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        "the runtimes record the same shot ticks"
    );
}

fn loop_shots(map: &'static str) {
    let out = tempfile::tempdir().unwrap();
    let mut level = Level::install(compile(map, out.path()));
    let wave = level.wave();
    let enter = level.enter("loop_zone");
    let period = ms_ticks(wave.len() as f32 * STEP_MS + GAP_MS);
    let phase = |index: usize| {
        if index == 0 {
            0
        } else {
            ms_ticks(index as f32 * STEP_MS)
        }
    };
    // Leave on the tick trap 0's third repeat is due.
    let exit = enter + 2 * period;
    level.run((exit - level.tick) as usize);
    level.place(Vec3::new(1000.0, level.position().y, 1000.0), 0.0);
    level.run(2 * period as usize);
    for (index, &trap) in wave.iter().enumerate() {
        let expected: Vec<u32> = (0..3)
            .map(|repeat| enter + phase(index) + repeat * period)
            .filter(|&tick| tick < exit)
            .collect();
        assert_eq!(level.shot_ticks(trap), expected, "{map}: wave trap {index}");
    }
    for &trap in &wave {
        let component = level.component(trap);
        assert!(
            component.runtime.unwrap().schedule.is_none(),
            "leaving the zone stopped every trap"
        );
    }
}

// Entering `loop_zone` fires trap i at its rounded phase, then once
// per rounded period; leaving on a due tick fires nothing on that tick or after.
#[test]
fn wave_loop_zone_staggers_repeats_and_stops_on_exit_in_both_runtimes() {
    loop_shots("trap-wave");
    loop_shots("trap-wave-luau");
}

// A level reinstall drops every trap request, schedule and parked step
// from the old install.
#[test]
fn reinstall_drops_old_trap_work() {
    let out = tempfile::tempdir().unwrap();
    let mut level = Level::install(compile("trap-wave", out.path()));
    let wave = level.wave();
    level.enter("sweep_plate");
    assert_eq!(level.shots.len(), 1, "the sweep's first step fired");
    // A latched request and a started schedule, both still owed.
    for &trap in &wave {
        postretro_sim::traps::issue_trap_request(
            &mut level.ctx.registry.borrow_mut(),
            trap,
            TrapRequest::Start {
                interval_ms: Some(100.0),
                phase_ms: Some(400.0),
            },
        );
    }
    level.reinstall(compile("trap-wave", out.path()));
    let before = level.shots.len();
    level.run(600);
    assert_eq!(
        level.shots.len(),
        before,
        "no old trap work fires in the new install"
    );
}

// A mod hot reload keeps a schedule already running, on its pre-reload
// anchor, and a latched request, with their install-time weapon; it drops the
// sweep's parked tail. The replaced descriptor reaches traps at the next
// install.
#[test]
fn mod_hot_reload_keeps_started_traps_and_latched_requests() {
    let out = tempfile::tempdir().unwrap();
    let mut level = Level::install(compile("trap-wave", out.path()));
    let wave = level.wave();
    level.enter("sweep_plate");
    // Start the last trap and let its schedule run partway into an interval.
    let started = wave[4];
    postretro_sim::traps::issue_trap_request(
        &mut level.ctx.registry.borrow_mut(),
        started,
        TrapRequest::Start {
            interval_ms: Some(1000.0),
            phase_ms: Some(0.0),
        },
    );
    let anchor = level.tick;
    level.run(5);
    // Latch a fire on the second trap, as a frame-end drain would.
    postretro_sim::traps::issue_trap_request(
        &mut level.ctx.registry.borrow_mut(),
        wave[1],
        TrapRequest::Fire,
    );
    // The mod reloads with a replaced weapon descriptor.
    {
        let mut data = level.ctx.data_registry.borrow_mut();
        let mut entities = data.entities.clone();
        for descriptor in &mut entities {
            if descriptor.canonical_name.as_deref() == Some(VOLLEY_WEAPON) {
                descriptor.weapon.as_mut().unwrap().damage = 99.0;
            }
        }
        data.replace_entity_types(entities);
    }
    level.app.rebuild_reactions_after_staged_commit();
    let reloaded_at = level.tick;
    level.run(200);
    assert_eq!(
        level.shot_ticks(wave[1]),
        vec![reloaded_at],
        "the latched fire survives and acts on the next tick; the parked sweep does not"
    );
    for &trap in &wave[2..4] {
        assert!(
            level.shot_ticks(trap).is_empty(),
            "the sweep's tail was dropped"
        );
    }
    let interval = ms_ticks(1000.0);
    assert_eq!(
        level.shot_ticks(started)[..4],
        [
            anchor,
            anchor + interval,
            anchor + 2 * interval,
            anchor + 3 * interval
        ],
        "the started trap keeps its schedule across the reload"
    );
    for shot in level.shots.iter().filter(|shot| shot.tick >= reloaded_at) {
        assert_eq!(shot.launched.damage, 10.0, "the install-time weapon fires");
    }
    level.reinstall(compile("trap-wave", out.path()));
    let wave = level.wave();
    postretro_sim::traps::issue_trap_request(
        &mut level.ctx.registry.borrow_mut(),
        wave[0],
        TrapRequest::Fire,
    );
    let shots = level.frame();
    assert_eq!(shots.len(), 1);
    assert_eq!(
        shots[0].launched.damage, 99.0,
        "the next install resolves the replacement"
    );
}
