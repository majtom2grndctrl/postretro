// The area-light Scripting-surface example run as a real content/dev level:
// the shipped `area-lights-demo.map` compiled by `prl-build`, its data script
// (and the Luau twin) installed through `install_world_cpu` under the real dev
// mod. Both ceiling skylights are `light_area` panels with no `_animated` key;
// the script's `levelLoad` pulse reserves their animated bake.
// See: context/lib/scripting.md §12 (Entity addressing) · build_pipeline.md §PRL section IDs

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use postretro_entities::components::light::{LightComponent, LightKind};
use postretro_entities::{ScriptCtx, ScriptError};
use postretro_level_loader::{LevelWorld, LightType};
use postretro_scripting_core::primitive_adapters::StoreSchemaJson;
use postretro_scripting_core::primitives_registry::{ContextScope, PrimitiveRegistry};
use serde_json::Value;

use super::teleporters_surface_tests::{canonical, compile, workspace};
use crate::coop_fixture_tests::initialize_dev_mod;
use crate::startup::StartupTimings;
use crate::startup::lifecycle::tests::{script_ctx, test_app};
use crate::startup::lifecycle::{
    WorldInstallHandles, install_world_cpu, start_installed_presentation_animations,
};

const MAP: &str = "area-lights-demo";
/// Quake map units to engine metres.
const UNIT: f64 = 0.0254;

const JS_CAPTURE: &str = "\n;(function () {\n  const original = globalThis.setupLevel;\n  globalThis.setupLevel = function (ctx) {\n    const result = original(ctx);\n    __areaFixtureCapture(result);\n    return result;\n  };\n})();\n";
const LUAU_CAPTURE: &str = "\ndo\n  local original = setupLevel\n  setupLevel = function(ctx)\n    local result = original(ctx)\n    __areaFixtureCapture(result)\n    return result\n  end\nend\n";

/// One installed demo level on a single-player host.
struct Level {
    app: crate::App,
    ctx: ScriptCtx,
    world: LevelWorld,
    /// The raw value `setupLevel` returned, as canonical JSON.
    wire: String,
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
            .register("__areaFixtureCapture", {
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
            // The renderer populates map lights before the data script runs,
            // as level load does; `install_world_cpu` leaves them to it.
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
        Self {
            app,
            ctx,
            world,
            wire,
        }
    }

    /// The `skylight` panels as `getMapEntities("light", { tag: "skylight" })`
    /// reads them: the raw query's snapshots, in authored order.
    fn snapshots(&self) -> Vec<Value> {
        let handles = postretro_lighting::script_primitives::collect_light_handles(
            &self.ctx,
            Some("skylight"),
        );
        match postretro_lighting::script_primitives::handles_to_json(handles) {
            Value::Array(items) => items,
            other => panic!("the query returns an array: {other}"),
        }
    }

    fn light(&self, map_index: usize) -> LightComponent {
        let id = self
            .app
            .session
            .as_ref()
            .unwrap()
            .light_bridge
            .entity_for_map_index(map_index)
            .expect("the map light has an entity");
        self.ctx
            .registry
            .borrow()
            .get_component::<LightComponent>(id)
            .unwrap()
            .clone()
    }
}

fn level(luau: bool) -> (tempfile::TempDir, Level) {
    let out = tempfile::tempdir().unwrap();
    let world = compile(MAP, out.path());
    let level = Level::install(world, luau);
    (out, level)
}

/// An engine-space point from Quake map units, as `f32`.
fn engine(x: f64, y: f64, z: f64) -> [f32; 3] {
    [(-y * UNIT) as f32, (z * UNIT) as f32, (-x * UNIT) as f32]
}

// Acceptance row 41: the Scripting-surface example as shipped. Neither panel
// has an `_animated` key, yet each gains an animated-bake reservation from the
// script; its glass's glow entry names that reserved compose slot; `lightType`
// reads "Area"; `position` is the glass's area-weighted centroid; and the
// pulse runs on both from load.
#[test]
fn area_light_surface_example_reserves_pulses_and_glows_each_skylight() {
    let (_out, level) = level(false);
    let lights = &level.world.lights;
    assert_eq!(lights.len(), 2, "two skylights and no other light");
    let mut slots = Vec::new();
    for light in lights {
        assert_eq!(light.light_type, LightType::Area);
        assert!(!light.is_dynamic);
        slots.push(
            light
                .animated_slot
                .expect("the script's pulse reserved an animated slot"),
        );
    }
    assert_ne!(slots[0], slots[1]);

    let glow: Vec<_> = level.world.area_glow.lights.iter().collect();
    assert_eq!(glow.len(), 2, "one glow record per skylight");
    for (light, slot) in glow.iter().zip(&slots) {
        assert_eq!(
            light.compose_slot,
            Some(*slot),
            "each glass glows on its own light's reserved slot"
        );
        assert!(!light.faces.is_empty(), "the glass has emitting faces");
    }

    let snapshots = level.snapshots();
    assert_eq!(snapshots.len(), 2);
    for (index, (snapshot, centre_x)) in snapshots.iter().zip([128.0, 384.0]).enumerate() {
        assert_eq!(snapshot["component"]["lightType"], "Area");
        let centroid = engine(centre_x, 128.0, 120.0);
        for (axis, name) in ["x", "y", "z"].into_iter().enumerate() {
            let got = snapshot["position"][name].as_f64().unwrap() as f32;
            assert!(
                (got - centroid[axis]).abs() < 1.0e-5,
                "panel {index} position.{name}: {got} vs centroid {}",
                centroid[axis]
            );
        }
        let pulse = level
            .light(index)
            .animation
            .expect("the levelLoad pulse is installed");
        let brightness = pulse.brightness.expect("a brightness pulse");
        assert_eq!(pulse.period_ms, 900.0);
        assert!(pulse.direction.is_none());
        let (low, high) = brightness
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &b| (lo.min(b), hi.max(b)));
        assert!(
            (low - 0.3).abs() < 1.0e-3 && (high - 1.0).abs() < 1.0e-3,
            "pulse within [0.3, 1]: {low}..{high}"
        );
        assert_eq!(level.light(index).light_type, LightKind::Area);
    }
}

// Acceptance row 44, runtime half (research.md O18): the east panel carries
// `delay 0`, `_light_size`, `_cone` and `mangle`. The runtime snapshot reads
// lightType "Area", falloffModel "InverseSquared", falloffRange the resolved
// reach, position the centroid, and no cone fields, each equal to the
// AlphaLights record it was built from; the build half
// (`level-compiler` `inert_keyed_area_light_snapshot_matches_its_alpha_lights_record`)
// holds the bake-time snapshot to the same record. Apart from position, the
// inert-keyed panel's snapshot equals the clean one's.
#[test]
fn inert_keyed_area_light_runtime_snapshot_matches_its_record() {
    let (_out, level) = level(false);
    let snapshots = level.snapshots();
    // A 64 × 64-unit panel at `light 300`: R = √(256 · 1 · A).
    let area_m2 = 64.0 * 64.0 * UNIT * UNIT;
    let reach = (256.0 * area_m2).sqrt();
    for (snapshot, record) in snapshots.iter().zip(&level.world.lights) {
        let component = &snapshot["component"];
        assert_eq!(component["lightType"], "Area");
        assert_eq!(component["falloffModel"], "InverseSquared");
        for cone in ["coneAngleInner", "coneAngleOuter", "coneDirection"] {
            assert!(component[cone].is_null(), "{cone}: {}", component[cone]);
        }
        let range = component["falloffRange"].as_f64().unwrap() as f32;
        assert_eq!(range, record.falloff_range, "the record's resolved reach");
        assert!(
            ((range as f64) - reach).abs() < 1.0e-4 * reach,
            "{range} vs {reach}"
        );
        let origin: Vec<f32> = ["x", "y", "z"]
            .into_iter()
            .map(|axis| snapshot["position"][axis].as_f64().unwrap() as f32)
            .collect();
        assert_eq!(
            origin,
            record.origin.map(|v| v as f32),
            "position is the record's centroid"
        );
        assert_eq!(
            component["intensity"].as_f64().unwrap() as f32,
            record.intensity
        );
    }
    let strip_position = |snapshot: &Value| {
        let mut component = snapshot["component"].clone();
        component.as_object_mut().unwrap().remove("origin");
        component
    };
    assert_eq!(
        strip_position(&snapshots[1]),
        strip_position(&snapshots[0]),
        "the inert keys change nothing but position"
    );
}

// Acceptance row 41, TS and Luau: the Luau twin returns byte-identical wire
// and installs the same pulse on both panels.
#[test]
fn area_light_surface_example_twins_emit_identical_wire_and_animations() {
    let (_ts_out, ts) = level(false);
    let (_luau_out, luau) = level(true);
    assert_eq!(ts.wire, luau.wire, "TS and Luau wire diverged");
    for index in 0..2 {
        assert_eq!(ts.light(index).animation, luau.light(index).animation);
        assert!(ts.light(index).animation.is_some());
    }
    assert!(
        Path::new(&workspace().join(format!("content/dev/scripts/{MAP}.luau"))).exists(),
        "the Luau twin ships beside the TS script"
    );
}
