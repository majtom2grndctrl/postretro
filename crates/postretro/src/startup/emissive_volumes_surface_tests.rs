// The emissive Scripting-surface example run as a real content/dev level: the
// shipped `emissive-volumes-demo.map` compiled by `prl-build`, its data script
// (and the Luau twin) installed through `install_world_cpu` under the real dev
// mod, the floor pads fired by the authoritative trigger tick and their
// `on_fire` residuals drained, and the emissive bridge driven on the script
// clock as the frame loop drives it.
// See: context/lib/scripting.md §10.2a · §12 (Entity addressing)

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use postretro_entities::{
    ComponentKind, EmissiveAnimation, EmissiveVolumeComponent, EntityId, ScriptCtx, ScriptError,
    Transform,
};
use postretro_level_loader::LevelWorld;
use postretro_scripting_core::primitive_adapters::StoreSchemaJson;
use postretro_scripting_core::primitives_registry::{ContextScope, PrimitiveRegistry};
use postretro_scripting_core::reaction_dispatch::{
    ResidualOrigin, fire_prepartitioned_reactions_with_sequences,
};
use serde_json::Value;

use super::teleporters_surface_tests::{canonical, compile, workspace};
use crate::coop_fixture_tests::initialize_dev_mod;
use crate::scripting_systems::curve_sample::sample_linear_curve;
use crate::startup::StartupTimings;
use crate::startup::lifecycle::tests::{script_ctx, test_app};
use crate::startup::lifecycle::{
    WorldInstallHandles, install_world_cpu, start_installed_presentation_animations,
};
use crate::trigger_commands::TriggerFireContext;
use crate::trigger_system::{
    AuthoritativePlayer, PlayerId, TriggerDispatchInputs, TriggerTickInputs,
};

const MAP: &str = "emissive-volumes-demo";
/// The 60 Hz presented frame the bridge steps at.
const FRAME_S: f64 = 1.0 / 60.0;
const DOOR_LOCKED_RED: [f32; 3] = [1.0, 0.05, 0.02];
/// The lamp's authored `emissive_color` `40 255 60`, channel ÷ 255.
const LAMP_GREEN: [f32; 3] = [40.0 / 255.0, 1.0, 60.0 / 255.0];

const JS_CAPTURE: &str = "\n;(function () {\n  const original = globalThis.setupLevel;\n  globalThis.setupLevel = function (ctx) {\n    const result = original(ctx);\n    __emissiveFixtureCapture(result);\n    return result;\n  };\n})();\n";
const LUAU_CAPTURE: &str = "\ndo\n  local original = setupLevel\n  setupLevel = function(ctx)\n    local result = original(ctx)\n    __emissiveFixtureCapture(result)\n    return result\n  end\nend\n";

/// One installed demo level on a single-player host, at script time 0.
struct Level {
    app: crate::App,
    ctx: ScriptCtx,
    /// The raw value `setupLevel` returned, as canonical JSON.
    wire: String,
    player: AuthoritativePlayer,
    /// The script clock the bridge samples, in seconds.
    script_time: f64,
}

impl Level {
    /// Load the compiled level, with the TS data script `prl-build` embedded
    /// or the Luau twin in its place, and install it as level load does.
    fn install(mut world: LevelWorld, luau: bool) -> Self {
        let mut app = test_app();
        let ctx = script_ctx(&app);
        let captured: Rc<RefCell<Option<Value>>> = Rc::new(RefCell::new(None));
        let mut primitives = PrimitiveRegistry::new();
        crate::scripting::primitives::register_all(&mut primitives, ctx.clone());
        primitives
            .register("__emissiveFixtureCapture", {
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

        let script = world
            .data_script
            .as_mut()
            .expect("the level carries its data script");
        if luau {
            let twin = workspace().join(format!("content/dev/scripts/{MAP}.luau"));
            script.compiled_bytes = std::fs::read(&twin).expect("the Luau twin reads");
            script.source_path = twin.to_string_lossy().into_owned();
        }
        script
            .compiled_bytes
            .extend_from_slice(if luau { LUAU_CAPTURE } else { JS_CAPTURE }.as_bytes());

        let content_root = workspace().join("content/dev");
        let nav_graph = crate::startup::lifecycle::install_world_gravity_and_nav(&world, &ctx);
        let products = {
            let session = app.session.as_mut().unwrap();
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
                emissive_volume_bridge: &mut session.emissive_volume_bridge,
                spawn_checkpoint_seam: false,
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
                system_reaction_ir_bindings: &session.scripting.system_reaction_ir_bindings,
                impact_policy_runtime: &mut session.scripting.impact_policy_runtime,
                mesh_clip_tables: &mut session.mesh_clip_tables,
                hit_zone_store: &mut session.hit_zone_store,
                occupancy_families: &mut session.occupancy_families,
                trigger_pool_policy: crate::trigger_pools::TriggerPoolSeedPolicy::ArmAll,
                suppress_host_replicated: false,
                suppress_boot_pawn: false,
                frontend_backdrop: false,
                local_carried_loadout: None,
                spawn_checkpoint: None,
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
        app.trigger_bindings = products.trigger_bindings;
        {
            let session = app.session.as_mut().unwrap();
            start_installed_presentation_animations(
                &ctx.registry.borrow(),
                &mut session.light_bridge,
                &mut session.fog_volume_bridge,
                &mut session.emissive_volume_bridge,
                &mut session.emitter_bridge,
            );
        }

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
        let mut level = Self {
            app,
            ctx,
            wire,
            player: AuthoritativePlayer {
                id: PlayerId::Local(pawn),
                pawn,
            },
            script_time: 0.0,
        };
        // The first running frame samples install-time animations at 0.
        level.present();
        level
    }

    fn volumes(&self, tag: &str) -> Vec<EntityId> {
        self.ctx
            .registry
            .borrow()
            .query_by_component_and_tag(ComponentKind::EmissiveVolume, Some(tag))
            .map(|(id, _)| id)
            .collect()
    }

    fn volume(&self, id: EntityId) -> EmissiveVolumeComponent {
        self.ctx
            .registry
            .borrow()
            .get_component::<EmissiveVolumeComponent>(id)
            .unwrap()
            .clone()
    }

    /// The frame loop's emissive step at the current script time.
    fn present(&mut self) {
        let session = self.app.session.as_mut().unwrap();
        session.emissive_volume_bridge.update(
            &mut self.ctx.registry.borrow_mut(),
            self.script_time,
            None,
        );
    }

    /// Advance the script clock one presented frame and present it.
    fn frame(&mut self) {
        self.script_time += FRAME_S;
        self.present();
    }

    /// Stand the player on the floor pad tagged `pad`, run one authoritative
    /// trigger tick, and drain the fired triggers' `on_fire` residuals as the
    /// frame-end drain does. Returns the triggers that fired.
    fn step_onto(&mut self, pad: &str) -> Vec<EntityId> {
        let ctx = self.ctx.clone();
        let player = self.player;
        let session = self.app.session.as_mut().unwrap();
        let center = {
            let registry = ctx.registry.borrow();
            let pads: Vec<EntityId> = registry
                .query_by_component_and_tag(ComponentKind::TriggerVolume, Some(pad))
                .map(|(id, _)| id)
                .collect();
            assert_eq!(pads.len(), 1, "one `{pad}` trigger");
            let (min, max) = session
                .trigger_volume_bridge
                .aabb(pads[0])
                .expect("the pad is bridged");
            (min + max) * 0.5
        };
        {
            let mut registry = ctx.registry.borrow_mut();
            let mut transform = *registry.get_component::<Transform>(player.pawn).unwrap();
            transform.position = center;
            registry.set_component(player.pawn, transform).unwrap();
        }

        let table = &self.app.trigger_bindings;
        let alive: HashSet<PlayerId> = [player.id].into();
        let mut fired = Vec::new();
        let mut residuals = Vec::new();
        session.trigger_system.run_authoritative_tick_with_dispatch(
            &mut ctx.registry.borrow_mut(),
            &session.trigger_volume_bridge,
            TriggerTickInputs {
                players: &[player],
                use_pressed: &HashMap::new(),
                tick_dt: FRAME_S as f32,
            },
            TriggerDispatchInputs {
                alive_players: &alive,
                bound_edges: table.bound_edges(),
            },
            |event, occupancy, _occupants, registry| {
                fired.push(event.fire.trigger);
                let execution = table.execute_with_script_ctx(
                    event.fire.trigger,
                    event.edge,
                    registry,
                    &ctx,
                    &TriggerFireContext {
                        fired_teleporter: None,
                        fired_seam: None,
                        fired_trigger: Some(event.fire.trigger),
                        activator: Some(player.pawn),
                        occupancy,
                        event_player: None,
                    },
                );
                if let Some(handle) = execution.residual() {
                    residuals.push(handle);
                }
            },
        );
        let scripting = &session.scripting;
        for handle in residuals {
            let follow_ups = fire_prepartitioned_reactions_with_sequences(
                table.residual(handle).unwrap().steps(),
                &scripting.sequence_registry,
                &scripting.reaction_registry,
                &scripting.system_registry,
                &ctx,
                ResidualOrigin::TriggerBinding,
            );
            assert!(follow_ups.is_empty(), "the pads fire no follow-ups");
        }
        fired
    }
}

/// Compile the shipped map once per test into its own directory.
fn level(luau: bool) -> (tempfile::TempDir, Level) {
    let out = tempfile::tempdir().unwrap();
    let world = compile(MAP, out.path());
    let level = Level::install(world, luau);
    (out, level)
}

/// `animation`'s sample `elapsed_s` into a run that started at script time 0.
fn sample_at(animation: &EmissiveAnimation, elapsed_s: f64) -> f32 {
    let periods = elapsed_s * 1000.0 / f64::from(animation.period_ms);
    sample_linear_curve(&animation.brightness, periods.rem_euclid(1.0) as f32)
}

// A10. The example as shipped: both bar signs flicker within [0.3, 1] from
// load; the exit sign's finite fade settles on its last sample; the pad whose
// `on_fire` names `doorLocked` turns the lamp red while its brightness and
// pulse run on; the pad naming `powerCut` holds both bar signs at 0.
#[test]
fn emissive_surface_example_flickers_cuts_the_power_and_locks_the_door() {
    let (_out, mut level) = level(false);
    let signs = level.volumes("bar_sign");
    let [lamp] = level.volumes("status_lamp")[..] else {
        panic!("one status lamp")
    };
    let [exit] = level.volumes("exit_sign")[..] else {
        panic!("one exit sign")
    };
    assert_eq!(signs.len(), 2, "two bar signs");
    let lamp_pulse = level
        .volume(lamp)
        .animation
        .expect("the lamp pulses from load");
    assert_eq!(level.volume(lamp).color, LAMP_GREEN, "authored green");
    let fade = level
        .volume(exit)
        .animation
        .expect("the exit sign fades from load");
    assert_eq!(fade.play_count, Some(1));

    // Flicker: every sample stays in [0.3, 1] and the signs visibly move.
    // 100 frames run past the exit sign's 1.5 s fade.
    let mut seen = Vec::new();
    for _ in 0..100 {
        level.frame();
        for &sign in &signs {
            let volume = level.volume(sign);
            assert!(volume.animation.is_some(), "the flicker loops");
            seen.push(volume.brightness);
        }
    }
    let (low, high) = seen
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &b| (lo.min(b), hi.max(b)));
    assert!(
        (0.3..=1.0).contains(&low) && (0.3..=1.0).contains(&high),
        "flicker leaves [0.3, 1]: {low}..{high}"
    );
    assert!(high - low > 0.4, "the flicker moves: {low}..{high}");

    // The fade (1.5 s) settled on its last sample and cleared, and holds.
    let settled = level.volume(exit);
    assert_eq!((settled.brightness, settled.animation), (1.0, None));

    // doorLocked: red lamp; its pulse keeps its start and samples.
    let before = level.volume(lamp);
    let fired = level.step_onto("door_locked_pad");
    assert_eq!(fired.len(), 1, "the east pad fires");
    let locked = level.volume(lamp);
    assert_eq!(locked.color, DOOR_LOCKED_RED);
    assert_eq!(locked.animation.as_ref(), Some(&lamp_pulse));
    assert_eq!(locked.animation_serial, before.animation_serial);
    assert_eq!(
        locked.brightness, before.brightness,
        "a colour write leaves brightness"
    );
    for _ in 0..20 {
        level.frame();
        let expected = sample_at(&lamp_pulse, level.script_time);
        let lamp_now = level.volume(lamp);
        assert!(
            (lamp_now.brightness - expected).abs() < 1e-4,
            "the pulse runs on from its load start: {} vs {expected}",
            lamp_now.brightness
        );
        assert_eq!(lamp_now.color, DOOR_LOCKED_RED);
    }

    // powerCut: both bar signs hold 0 with no animation, frame after frame.
    let fired = level.step_onto("power_cut_pad");
    assert_eq!(fired.len(), 1, "the west pad fires");
    for _ in 0..30 {
        level.frame();
        for &sign in &signs {
            let volume = level.volume(sign);
            assert_eq!((volume.brightness, volume.animation), (0.0, None));
        }
    }
    let lamp_after = level.volume(lamp);
    assert!(lamp_after.animation.is_some(), "powerCut leaves the lamp");
    assert_eq!(level.volume(exit).brightness, 1.0);
}

// Manual-check support (V1): the lamp pads fire on every entry, so walking
// between them cycles the lamp red and back to its authored green while its
// pulse runs on; the west panel's pulse reaches brightness 0 (its unlit
// diffuse) and 1 within one 3 s period.
#[test]
fn emissive_demo_lamp_cycles_red_and_green_and_the_panel_breathes_to_zero() {
    let (_out, mut level) = level(false);
    let [lamp] = level.volumes("status_lamp")[..] else {
        panic!("one status lamp")
    };
    let [panel] = level.volumes("breathe_panel")[..] else {
        panic!("one breathing panel")
    };
    let pulse = level
        .volume(lamp)
        .animation
        .expect("the lamp pulses from load");

    for (pad, color) in [
        ("door_locked_pad", DOOR_LOCKED_RED),
        ("door_unlocked_pad", LAMP_GREEN),
        ("door_locked_pad", DOOR_LOCKED_RED),
        ("door_unlocked_pad", LAMP_GREEN),
    ] {
        assert_eq!(level.step_onto(pad).len(), 1, "`{pad}` fires on entry");
        level.frame();
        let lamp_now = level.volume(lamp);
        assert_eq!(lamp_now.color, color, "after `{pad}`");
        assert_eq!(
            lamp_now.animation.as_ref(),
            Some(&pulse),
            "a colour write leaves the pulse"
        );
    }

    let mut low = f32::MAX;
    let mut high = f32::MIN;
    for _ in 0..180 {
        level.frame();
        let volume = level.volume(panel);
        assert!(volume.animation.is_some(), "the panel keeps breathing");
        low = low.min(volume.brightness);
        high = high.max(volume.brightness);
    }
    assert!(
        low < 0.01,
        "the panel's trough reaches its unlit diffuse: {low}"
    );
    assert!(high > 0.99, "the panel's crest reaches full glow: {high}");
}

// A10, TS and Luau: the Luau twin returns byte-identical wire and installs
// the same animations on the same volumes.
#[test]
fn emissive_surface_example_twins_emit_identical_wire_and_animations() {
    let (_ts_out, ts) = level(false);
    let (_luau_out, luau) = level(true);
    assert_eq!(ts.wire, luau.wire, "TS and Luau wire diverged");
    for tag in ["bar_sign", "status_lamp", "exit_sign", "breathe_panel"] {
        let animations = |level: &Level| -> Vec<Option<EmissiveAnimation>> {
            level
                .volumes(tag)
                .into_iter()
                .map(|id| level.volume(id).animation)
                .collect()
        };
        assert_eq!(animations(&ts), animations(&luau), "`{tag}` animations");
        assert!(
            animations(&ts).iter().all(Option::is_some),
            "`{tag}` animates"
        );
    }
    assert!(
        Path::new(&workspace().join(format!("content/dev/scripts/{MAP}.luau"))).exists(),
        "the Luau twin ships beside the TS script"
    );
}
