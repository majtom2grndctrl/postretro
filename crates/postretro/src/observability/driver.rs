// Headless batch-mode driver: parse a runspec, load a real `.prl` map, run the
// requested fixed ticks with scripted commands, and emit a deterministic JSON
// state dump to stdout — no window, GPU, or display server. This is the runtime
// sibling of `prl-build` for content CI and the agent-facing engine harness.
//
// Contract: stdout carries ONLY the JSON document; every log line and every
// diagnostic goes to stderr. On any failure the process exits non-zero and no
// partial JSON reaches stdout (the document is fully built in memory, then
// printed once).
// See: context/lib/boot_sequence.md §3, context/plans/done/agentic-observability

use std::borrow::Cow;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context, Result, anyhow, bail};
use glam::{Vec2, Vec3};

use postretro_entities::components::health::HealthComponent;
use postretro_entities::{EntityId, EntityRegistry, Transform};

use crate::collision::CollisionWorld;
use crate::movement::MovementInput;
use crate::scripting_systems::fog_volume_bridge::FogVolumeBridge;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::scripting_systems::mesh_anim::MeshClipTables;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::session::{HeadlessSession, require_headless_mod_manifest};
use crate::sim::{PostMovementCommand, SimCommand, simulate_tick};
use crate::startup::StartupTimings;
use crate::startup::lifecycle::{
    WorldInstallHandles, install_world_cpu, install_world_gravity_and_nav,
};
use crate::trigger_pools::TriggerPoolSeedPolicy;
use crate::weapon::FireButtonState;
use postretro_scripting_core::reaction_dispatch::ProgressTracker;
use postretro_scripting_core::state_crossings::CrossingDetector;

use super::{
    AimCommand, CommandEntry, PawnHealth, PlayerPawnSummary, TickEventRecord,
    build_output_document, parse_runspec, to_deterministic_json,
};

/// Fixed game-logic tick length. Pinned to `1/60` s exactly (NOT
/// `TICK_DURATION.as_secs_f32()`, whose rounded value diverges) so headless runs
/// match the determinism-test reference `DT`.
const TICK_DT: f32 = 1.0 / 60.0;

/// Entry point wired from `startup::build_session` behind `--headless`. Runs the
/// whole batch synchronously and terminates the process with an exit code — it
/// never returns a `BootSession`, so `main` never drives a windowless event loop.
/// Success prints the JSON document to stdout and exits 0; any failure prints a
/// diagnostic to stderr and exits non-zero with no stdout output.
pub(crate) fn run_headless(
    runspec_arg: Option<&str>,
    trigger_pool_policy: TriggerPoolSeedPolicy,
) -> ! {
    match run_headless_inner(runspec_arg, trigger_pool_policy) {
        Ok(json) => {
            // stdout carries ONLY the document; a single write after the full
            // build guarantees no partial JSON on any earlier failure.
            println!("{json}");
            std::process::exit(0);
        }
        Err(err) => {
            eprintln!("[Headless] {err:#}");
            std::process::exit(1);
        }
    }
}

/// The fallible body of the headless run. Returns the serialized JSON document on
/// success; every error path bubbles up here so `run_headless` can map it to a
/// stderr diagnostic and a non-zero exit without ever touching stdout.
fn run_headless_inner(
    runspec_arg: Option<&str>,
    trigger_pool_policy: TriggerPoolSeedPolicy,
) -> Result<String> {
    let runspec_path =
        runspec_arg.ok_or_else(|| anyhow!("`--headless` requires a runspec JSON path argument"))?;

    // 1. Parse + validate the runspec. Malformed JSON, unknown fields, and a
    //    missing map all surface as a non-zero exit with a stderr diagnostic.
    let text = std::fs::read_to_string(runspec_path)
        .with_context(|| format!("failed to read runspec `{runspec_path}`"))?;
    let runspec = parse_runspec(&text)?;

    let map_path = PathBuf::from(&runspec.map);
    if !map_path.is_file() {
        bail!("map not found: `{}`", runspec.map);
    }
    // Resolve the component filter up front so a bad `dump.component` value fails
    // before the expensive load/tick work rather than after it.
    let _ = runspec.dump.resolve_component()?;
    // A command authored at a tick the run never reaches is a silent no-op —
    // warn so an author notices the dead command rather than assuming it fired.
    warn_unreachable_commands(&runspec.commands, runspec.ticks);

    // 2. Load the PRL synchronously (no worker thread; headless has no event
    //    loop). The loader owns file IO and validation.
    let map_path_str = map_path
        .to_str()
        .ok_or_else(|| anyhow!("map path is not valid UTF-8: `{}`", runspec.map))?;
    let world = postretro_level_loader::load_prl(map_path_str)
        .with_context(|| format!("failed to load `{}`", runspec.map))?;

    // 3. Build the reduced headless session (scripting core + classname dispatch;
    //    no audio/input/UI/net/window). This checks the `scripts-build` sidecar
    //    and derives the content root from the map path.
    let mut session = HeadlessSession::build(&map_path)?;
    let content_root = session.content_root.clone();

    // 4. Mod-init, then the manifest-to-DataRegistry drain. `run_mod_init` alone
    //    only parses/stores the manifest — without the drain the archetype sweep
    //    sees an empty entity-type registry and no player pawn spawns. A `None`
    //    manifest (missing start-script) is rejected here rather than silently
    //    producing a pawn-less world.
    session
        .scripting
        .script_runtime
        .run_mod_init(&content_root)
        .context("mod-init failed")?;
    require_headless_mod_manifest(session.scripting.script_runtime.mod_manifest().is_some())?;
    session.scripting.drain_manifest_registrations();

    // 5. World install, segments A then B back-to-back (headless creates no light
    //    entities, so the documented fog-first entity-id shape holds). The mesh
    //    upload hook is a no-op — headless has no renderer, so clip tables stay
    //    empty. Every session-owned bridge/store the sweep touches is a fresh
    //    local instance here.
    let script_ctx = session.scripting.script_ctx.clone();
    let nav_graph = install_world_gravity_and_nav(&world, &script_ctx);

    let mut collision_world = CollisionWorld::new();
    let mut fog_volume_bridge = FogVolumeBridge::new();
    // Trigger volumes are populated for parity with the windowed install, but the
    // tick loop passes no `TriggerTickContext` to `simulate_tick`, so the trigger
    // system never evaluates them headless (declared out-of-frame in the dump).
    let mut trigger_volume_bridge = TriggerVolumeBridge::new();
    // The sweep plumbing requires a modal-stack handle even though headless has no
    // UI; the level-scope tree registrations it receives are simply never rendered.
    let mut modal_stack = postretro_ui::modal_stack::ModalStack::new();
    // `progress_tracker` persists across ticks (it drives the death sweep), so it
    // is owned here for the whole run, not per-tick.
    let mut progress_tracker = ProgressTracker::new();
    let mut crossing_detector = CrossingDetector::new();
    let mut mesh_clip_tables = MeshClipTables::new();
    let mut hit_zone_store = HitZoneStore::new();
    // Runspecs address raw map paths, not catalog ids. Preserve the raw-path
    // contract: direct `.prl` loads never inherit catalog classification tags.
    let active_level_tags = active_level_tags_for_headless_install();
    let mut timings = StartupTimings::new();

    let products = {
        let handles = WorldInstallHandles {
            world: &world,
            script_ctx: &script_ctx,
            command_diagnostics: session.scripting.command_diagnostics.clone(),
            mover_auto_close_ms: session.scripting.mover_auto_close_ms,
            spawn_context: session.scripting.spawn_context.clone(),
            content_root: content_root.as_path(),
            active_level_tags: &active_level_tags,
            nav_graph: nav_graph.as_ref(),
            collision_world: &mut collision_world,
            fog_volume_bridge: &mut fog_volume_bridge,
            trigger_volume_bridge: &mut trigger_volume_bridge,
            classname_dispatch: &session.classname_dispatch,
            script_runtime: &session.scripting.script_runtime,
            sequence_registry: &session.scripting.sequence_registry,
            reaction_registry: &session.scripting.reaction_registry,
            system_registry: &session.scripting.system_registry,
            modal_stack: &mut modal_stack,
            progress_tracker: &mut progress_tracker,
            crossing_detector: &mut crossing_detector,
            slot_accumulator_bindings: &mut session.scripting.slot_accumulator_bindings,
            impact_policy_runtime: &mut session.scripting.impact_policy_runtime,
            mesh_clip_tables: &mut mesh_clip_tables,
            hit_zone_store: &mut hit_zone_store,
            trigger_pool_policy,
            suppress_ai_enemies: false,
            suppress_boot_pawn: false,
            local_carried_loadout: None,
        };
        install_world_cpu(
            handles,
            &mut timings,
            |_models, _clip_tables| {
                crate::scripting_systems::hit_zones::ModelLoadWarningOwner::GameSide
            },
            |_spawn_points| {},
        )
    };

    let mover_colliders = products.mover_colliders;
    let mut mover_tick_states = products.mover_tick_states;

    // 6. Tick loop. Persistent per-run state: the registry handle, the progress
    //    tracker (above), the AI-warning set, the mover tick states, and the
    //    animation clock advanced by dt each tick.
    let registry = script_ctx.registry.clone();
    let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
    let mut anim_time: f64 = 0.0;
    let mut prev_fire_active = false;
    // Seeded from tick 0's effective aim (not just `0.0`) so a `ticks: 0` run —
    // which never enters the loop below — still reports the authored aim
    // instead of an always-neutral facing.
    let mut last_facing_yaw = effective_aim_at(&runspec.commands, 0)
        .map(|aim| yaw_from_direction(aim.direction))
        .unwrap_or(0.0);
    let mut events: Vec<TickEventRecord> = Vec::new();
    // Deferred removals report after a tick's game-logic event collection, so
    // their progress events become observable in the following tick's dump.
    let mut pending_death_events: Vec<String> = Vec::new();

    for tick in 0..runspec.ticks {
        // Headless has no frame concept, so the scheduler's monotonic frame
        // counter advances once per tick. Install-time enrollment stamps counter
        // 0, so a 1-tick wait first advances on this (the first) tick — a defined
        // offset, not incidental. This driver has no frame-end drain, so it only
        // proves the counter/evaluate half; landings are exercised by SimHarness.
        session.scripting.scheduler.begin_frame();
        let mut death_events_for_tick = std::mem::take(&mut pending_death_events);
        // Sparse command timeline: the active entry is the last one whose tick has
        // arrived; the effective aim is the most recent aim among arrived entries
        // (aim carries no neutral — it persists until overridden).
        let active = active_command_at(&runspec.commands, tick);
        let effective_aim = effective_aim_at(&runspec.commands, tick);
        // Re-read every tick (a `Cell<f32>`, cheap) so a mid-run `world.setGravity`
        // reaction is observed the same tick as the windowed loop (`main.rs`),
        // rather than only at level-load time.
        let gravity = script_ctx.gravity.get();

        let facing_yaw = effective_aim
            .map(|aim| yaw_from_direction(aim.direction))
            .unwrap_or(0.0);
        last_facing_yaw = facing_yaw;

        let movement = match active {
            Some(entry) => entry.movement_input(facing_yaw),
            None => neutral_movement(facing_yaw),
        };
        // `fire` is a held/level signal: `active` is this tick's level, `pressed`
        // is the rising edge across consecutive ticks.
        let fire_active = active.map(|entry| entry.fire).unwrap_or(false);
        let fire_button = fire_button_state(fire_active, prev_fire_active);
        prev_fire_active = fire_active;
        let reload = active.map(|entry| entry.reload).unwrap_or(false);

        let command = SimCommand {
            movement,
            fire_button,
            reload,
            firing_slot: 0,
            select_slot: None,
            // No "use" verb in the runspec yet; headless drives no trigger stage.
            use_pressed: false,
            drop_pressed: false,
        };

        // The post-movement closure returns the runspec's aim (origin + a
        // normalized direction); `simulate_tick` re-normalizes before measuring
        // weapon range, but we normalize here too to honor the documented shape.
        let aim = effective_aim.cloned();
        let post_movement = move |_registry: &Rc<RefCell<EntityRegistry>>| match &aim {
            Some(aim) => PostMovementCommand {
                aim_origin: Vec3::from_array(aim.origin),
                aim_direction: Vec3::from_array(aim.direction).normalize_or_zero(),
            },
            None => PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
        };

        let tick_events = simulate_tick(
            registry.clone(),
            &collision_world,
            &hit_zone_store,
            nav_graph.as_ref(),
            gravity,
            None,
            anim_time,
            &mut progress_tracker,
            &mut ai_runtime,
            &mover_colliders,
            &mut mover_tick_states,
            &[], // no remote pawns headless
            &command,
            post_movement,
            TICK_DT,
            // No trigger context headless: trigger volumes are populated but the
            // host-authoritative trigger stage is not driven (no use/overlap
            // routing), so triggers stay inert — declared out-of-frame in the dump.
            None,
            |registry| session.scripting.evaluate_pending_in_tick_impacts(registry),
        );
        // No trigger context here, so no Exit fires are ever produced; the empty
        // set means nothing is cancelled and the countdown simply advances.
        session.scripting.scheduler.evaluate(&[]);
        crate::scripting_systems::slot_accumulators::evaluate_slot_accumulators(
            &mut session.scripting.slot_accumulator_bindings,
            TICK_DT,
        );
        run_headless_frame_end_removals(
            &registry,
            &mut progress_tracker,
            &mut pending_death_events,
        );
        death_events_for_tick.extend(tick_events.death.iter().cloned());

        // Skip building/pushing the owned-string record entirely when the dump
        // won't emit events — `build_output_document` discards this vec wholesale
        // for `dump.events == false`, so buffering it per tick is pure wasted
        // allocation (and an OOM vector on a large `ticks` with `events: false`).
        if runspec.dump.events {
            let mut weapon_events = to_owned_strings(&tick_events.weapon);
            weapon_events.extend(
                tick_events
                    .reload_deliveries
                    .iter()
                    .map(|delivery| delivery.outcome.event_name().to_string()),
            );
            events.push(TickEventRecord {
                tick,
                movement: to_owned_strings(&tick_events.movement),
                ai: to_owned_cow_strings(&tick_events.ai),
                weapon: weapon_events,
                death: death_events_for_tick,
            });
        }

        anim_time += TICK_DT as f64;
    }

    // 7. Serialize. Build the player summary and full document, then emit through
    //    the deterministic serializer so two identical runs are byte-identical.
    let player = {
        let registry_ref = registry.borrow();
        build_player_summary(&registry_ref, last_facing_yaw)
    };
    let doc = {
        let registry_ref = registry.borrow();
        build_output_document(
            runspec.map.clone(),
            runspec.ticks,
            &registry_ref,
            &runspec.dump,
            &world,
            events,
            player,
        )?
    };
    Ok(to_deterministic_json(&doc)?)
}

fn run_headless_frame_end_removals(
    registry: &Rc<RefCell<EntityRegistry>>,
    progress_tracker: &mut ProgressTracker,
    next_tick_death_events: &mut Vec<String>,
) {
    crate::impact_effects::run_end_of_frame_removal_pass(
        &mut registry.borrow_mut(),
        |_, pending_kill_credit| {
            let Some(pending_kill_credit) = pending_kill_credit else {
                return;
            };
            next_tick_death_events
                .extend(progress_tracker.on_entity_killed(&pending_kill_credit.tags));
        },
    );
}

fn active_level_tags_for_headless_install() -> Vec<String> {
    Vec::new()
}

/// The active command for `tick`: the last entry whose tick has arrived. `None`
/// for ticks before the first entry (neutral input). Commands are authored in
/// ascending tick order and apply from their tick until the next entry.
fn active_command_at(commands: &[CommandEntry], tick: u32) -> Option<&CommandEntry> {
    commands.iter().rev().find(|entry| entry.tick <= tick)
}

/// The effective aim for `tick`: the most recent aim among arrived entries. Aim
/// carries no neutral, so it persists across later entries that omit `aim`.
fn effective_aim_at(commands: &[CommandEntry], tick: u32) -> Option<&AimCommand> {
    commands
        .iter()
        .filter(|entry| entry.tick <= tick)
        .rev()
        .find_map(|entry| entry.aim.as_ref())
}

/// Derive the weapon fire button from the held/level `fire` signal: `active` is
/// this tick's level, `pressed` is the rising edge relative to the prior tick.
fn fire_button_state(fire_active: bool, prev_fire_active: bool) -> FireButtonState {
    FireButtonState {
        pressed: fire_active && !prev_fire_active,
        active: fire_active,
    }
}

/// Neutral movement intent for ticks before the first command (or gaps the
/// vocabulary treats as neutral), carrying the driver-derived facing yaw.
fn neutral_movement(facing_yaw: f32) -> MovementInput {
    MovementInput {
        wish_dir: Vec2::ZERO,
        jump_pressed: false,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        use_pressed: false,
        drop_pressed: false,
        facing_yaw,
    }
}

/// Derive the engine facing yaw from an aim direction's horizontal projection.
/// Inverts the camera's `forward = (-sin(yaw), 0, -cos(yaw))` mapping, so a
/// direction of `-Z` yields yaw `0`. Purely horizontal — pitch is ignored.
/// A zero-length or non-finite direction carries no aim at all; treated as
/// neutral (yaw `0.0`) rather than `atan2`'s arbitrary `-π` for `(-0.0, -0.0)`.
fn yaw_from_direction(direction: [f32; 3]) -> f32 {
    let dir = Vec3::from_array(direction);
    if dir == Vec3::ZERO || !dir.is_finite() {
        return 0.0;
    }
    (-direction[0]).atan2(-direction[2])
}

/// Warn (stderr) about any command authored at a tick the run never reaches —
/// `tick >= ticks` is a silent no-op the sim never observes. Does not reject:
/// that's the runspec validator's call, out of scope here.
fn warn_unreachable_commands(commands: &[CommandEntry], ticks: u32) {
    let unreachable: Vec<u32> = commands
        .iter()
        .map(|entry| entry.tick)
        .filter(|&tick| tick >= ticks)
        .collect();
    if !unreachable.is_empty() {
        log::warn!(
            "[Headless] command tick(s) {unreachable:?} are >= ticks ({ticks}); these commands never activate"
        );
    }
}

fn to_owned_strings(events: &[&'static str]) -> Vec<String> {
    events.iter().map(|event| (*event).to_string()).collect()
}

/// The AI tick reports `Cow` so an authored `onEnter` address rides alongside
/// the static attack event without cloning the latter every attack tick.
fn to_owned_cow_strings(events: &[Cow<'static, str>]) -> Vec<String> {
    events.iter().map(|event| event.to_string()).collect()
}

/// Resolve the pawn summarized by headless observability.
fn local_pawn(registry: &EntityRegistry) -> Option<EntityId> {
    registry.local_player_movement_pawn()
}

/// Build the curated player-pawn summary from the post-run registry. `None` when
/// no player pawn spawned (a map without a `player_spawn`).
fn build_player_summary(registry: &EntityRegistry, facing_yaw: f32) -> Option<PlayerPawnSummary> {
    let id = local_pawn(registry)?;
    let transform = registry.get_component::<Transform>(id).ok()?;
    let health = registry
        .get_component::<HealthComponent>(id)
        .ok()
        .map(|health| PawnHealth {
            current: health.current,
            max: health.max,
        });
    Some(PlayerPawnSummary {
        entity: id.to_raw(),
        position: transform.position.to_array(),
        facing_yaw,
        health,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::DataRegistry;
    use postretro_entities::components::inventory::Inventory;
    use postretro_entities::components::player_movement::PlayerMovementComponent;
    use postretro_entities::data_descriptors::{
        HealthDescriptor, NamedReaction, ProgressDescriptor, ReactionDescriptor,
    };
    use postretro_scripting_core::data_descriptors::{
        AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementDescriptor, SpeedParams,
    };

    fn test_movement_descriptor() -> PlayerMovementDescriptor {
        PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.35,
                half_height: 0.9,
                eye_height: 1.1,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                    crouch: 3.0,
                },
                accel: 12.0,
                step_height: 0.35,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 0.3,
                accel: 2.0,
                max_control_speed: 4.0,
                bunny_hop: true,
                jumps: 1,
                jump_velocity: 5.0,
                jump_ceiling: 2.0,
            },
            fall: FallParams {
                terminal_velocity: 50.0,
            },
            stuck_stop_enabled: true,
            stuck_stop_threshold: 0.001,
            dash: None,
            forgiveness: None,
            crouch: None,
            slide: None,
            view_feel: None,
        }
    }

    #[test]
    fn headless_install_keeps_direct_prl_paths_untagged() {
        assert!(
            active_level_tags_for_headless_install().is_empty(),
            "headless runspecs use raw .prl paths and cannot match scoped pools",
        );
    }

    #[test]
    fn headless_summary_reads_the_installed_inventory_pawn_via_local_movement_identity() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform {
            position: glam::Vec3::new(3.0, 2.0, 1.0),
            ..Transform::default()
        });
        registry
            .set_component(
                pawn,
                PlayerMovementComponent::from_descriptor(&test_movement_descriptor()),
            )
            .unwrap();
        registry.set_component(pawn, Inventory::default()).unwrap();
        registry.mark_local_player_pawn(pawn).unwrap();

        let summary = build_player_summary(&registry, 0.25).expect("installed pawn is summarized");
        assert_eq!(summary.entity, pawn.to_raw());
        assert_eq!(summary.position, [3.0, 2.0, 1.0]);
        assert!((summary.facing_yaw - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn yaw_from_negative_z_direction_is_zero() {
        assert!(yaw_from_direction([0.0, 0.0, -1.0]).abs() < 1e-6);
    }

    #[test]
    fn yaw_from_negative_x_direction_is_quarter_turn() {
        // Camera forward at yaw = +pi/2 is (-1, 0, 0), so a -X aim yields +pi/2.
        let yaw = yaw_from_direction([-1.0, 0.0, 0.0]);
        assert!(
            (yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6,
            "got {yaw}"
        );
    }

    #[test]
    fn yaw_ignores_pitch_component() {
        // A steeply-pitched aim toward -Z keeps yaw 0 (horizontal projection only).
        let level = yaw_from_direction([0.0, 0.0, -1.0]);
        let pitched = yaw_from_direction([0.0, -5.0, -1.0]);
        assert!((level - pitched).abs() < 1e-6);
    }

    #[test]
    fn yaw_from_zero_direction_is_neutral_not_negative_pi() {
        // Naive atan2(-0.0, -0.0) would yield an arbitrary -π; zero-length aim
        // carries no facing, so this must report neutral (0.0) instead.
        assert_eq!(yaw_from_direction([0.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn yaw_from_non_finite_direction_is_neutral() {
        assert_eq!(yaw_from_direction([f32::NAN, 0.0, -1.0]), 0.0);
        assert_eq!(yaw_from_direction([f32::INFINITY, 0.0, -1.0]), 0.0);
    }

    #[test]
    fn neutral_movement_is_zeroed_but_carries_yaw() {
        let movement = neutral_movement(0.75);
        assert_eq!(movement.wish_dir, Vec2::ZERO);
        assert!(!movement.jump_pressed);
        assert!(!movement.running);
        assert_eq!(movement.facing_yaw, 0.75);
    }

    fn entry(tick: u32, fire: bool, aim: Option<AimCommand>) -> CommandEntry {
        CommandEntry {
            tick,
            movement: super::super::runspec::MovementCommand::default(),
            aim,
            fire,
            reload: false,
        }
    }

    #[test]
    fn active_command_is_none_before_first_entry_then_sticks() {
        let commands = vec![entry(2, false, None), entry(5, true, None)];
        assert!(active_command_at(&commands, 0).is_none());
        assert!(active_command_at(&commands, 1).is_none());
        // Entry applies from its tick through to the next entry.
        assert_eq!(active_command_at(&commands, 2).unwrap().tick, 2);
        assert_eq!(active_command_at(&commands, 4).unwrap().tick, 2);
        assert_eq!(active_command_at(&commands, 5).unwrap().tick, 5);
        assert_eq!(active_command_at(&commands, 99).unwrap().tick, 5);
    }

    #[test]
    fn effective_aim_persists_across_entries_that_omit_it() {
        let aim = AimCommand {
            origin: [0.0, 1.0, 0.0],
            direction: [0.0, 0.0, -1.0],
        };
        // Aim set at tick 0, a later entry at tick 10 omits aim: it must persist.
        let commands = vec![entry(0, false, Some(aim.clone())), entry(10, true, None)];
        assert!(effective_aim_at(&commands, 0).is_some());
        assert_eq!(
            effective_aim_at(&commands, 15).unwrap().direction,
            aim.direction
        );
    }

    #[test]
    fn effective_aim_is_none_until_first_aim_arrives() {
        let commands = vec![entry(0, false, None)];
        assert!(effective_aim_at(&commands, 0).is_none());
    }

    #[test]
    fn fire_button_rising_edge_only_on_first_active_tick() {
        // Held across three ticks: pressed only on the rising edge.
        let t0 = fire_button_state(true, false);
        assert!(t0.active && t0.pressed, "rising edge sets pressed");
        let t1 = fire_button_state(true, true);
        assert!(t1.active && !t1.pressed, "held is active but not pressed");
        let released = fire_button_state(false, true);
        assert!(!released.active && !released.pressed);
        // A fresh press after release is a rising edge again.
        let repressed = fire_button_state(true, false);
        assert!(repressed.active && repressed.pressed);
    }

    #[test]
    fn warn_unreachable_commands_is_silent_when_all_ticks_in_range() {
        let commands = vec![entry(0, false, None), entry(9, true, None)];
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            warn_unreachable_commands(&commands, 10);
        });
        assert!(
            captured.is_empty(),
            "expected no log output, got: {captured:?}"
        );
    }

    #[test]
    fn warn_unreachable_commands_flags_ticks_at_or_past_the_run_length() {
        let commands = vec![
            entry(0, false, None),
            entry(10, true, None),
            entry(15, false, None),
        ];
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            warn_unreachable_commands(&commands, 10);
        });
        assert!(
            captured.iter().any(|(lvl, msg)| *lvl == log::Level::Warn
                && msg.contains("[Headless]")
                && msg.contains("10")
                && msg.contains("15")),
            "expected a warn-level log naming the unreachable ticks, got: {captured:?}"
        );
    }

    #[test]
    fn headless_frame_end_reaps_impact_marked_entities() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let target = registry
            .borrow_mut()
            .spawn(postretro_entities::Transform::default());
        registry
            .borrow_mut()
            .mark_for_end_of_frame_removal(target)
            .unwrap();

        let mut progress_tracker = ProgressTracker::new();
        let mut next_tick_death_events = Vec::new();
        run_headless_frame_end_removals(
            &registry,
            &mut progress_tracker,
            &mut next_tick_death_events,
        );

        assert!(!registry.borrow().exists(target));
    }

    #[test]
    fn headless_removal_queues_kill_progress_for_the_next_tick() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let target = registry.borrow_mut().spawn(Transform::default());
        registry
            .borrow_mut()
            .set_tags(target, vec!["wave".to_string()])
            .unwrap();
        let mut health = HealthComponent::from_descriptor(&HealthDescriptor {
            max: 10.0,
            hitbox: None,
            zone_multipliers: Default::default(),
        });
        health.current = 0.0;
        registry.borrow_mut().set_component(target, health).unwrap();

        let mut data = DataRegistry::new();
        data.populate_level(
            vec![NamedReaction {
                name: "wave_done".to_string(),
                descriptor: ReactionDescriptor::Progress(ProgressDescriptor {
                    tag: "wave".to_string(),
                    at: 1.0,
                    fire: "wave_complete".to_string(),
                }),
            }],
            Vec::new(),
            &[],
        );
        let mut progress_tracker = ProgressTracker::new();
        progress_tracker.initialize(&data, &registry.borrow());

        assert_eq!(
            crate::scripting_systems::health::sweep_deaths(&mut registry.borrow_mut()),
            crate::scripting_systems::health::DeathReport::default(),
            "the zero-HP sweep itself must not queue kill progress",
        );
        crate::impact_effects::despawn(&mut registry.borrow_mut(), target, None);

        let mut next_tick_death_events = Vec::new();
        run_headless_frame_end_removals(
            &registry,
            &mut progress_tracker,
            &mut next_tick_death_events,
        );
        assert_eq!(next_tick_death_events, vec!["wave_complete".to_string()]);

        let death_events_for_next_tick = std::mem::take(&mut next_tick_death_events);
        assert_eq!(
            death_events_for_next_tick,
            vec!["wave_complete".to_string()]
        );
    }
}
