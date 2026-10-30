//! Shared builders for the level lifecycle tests: the test `App`, level worlds,
//! install fixtures, and the source-guard readers.
//! See: context/lib/testing_guide.md §4

use super::*;

use crate::frame_timing::{FrameRateMeter, FrameTiming, InterpolableState};
use crate::input::InputFocus;
use crate::scripting;
use crate::scripting::primitives::register_all;
use crate::{input, options, scripting_systems, view_feel};
use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use postretro_entities::{EntityTypeDescriptor, TriggerPoolDescriptor};
use postretro_entities::{ScriptCtx, SlotRecord};
use postretro_foundation::ModMapEntry;

use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_scripting_core::reaction_dispatch::ProgressTracker;
use postretro_scripting_core::runtime::{ScriptRuntime, ScriptRuntimeConfig};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

use postretro_scripting_core::state_crossings::CrossingDetector;

pub(super) const FIXTURE_MAP_A: &str = "fixture_map_a_reactor_room";

fn test_runtime(ctx: &ScriptCtx) -> ScriptRuntime {
    let mut registry = PrimitiveRegistry::new();
    register_all(&mut registry, ctx.clone());
    ScriptRuntime::new(&registry, &ScriptRuntimeConfig::default(), ctx).unwrap()
}

/// Gives the test app a client endpoint, as `--connect` would.
pub(crate) fn connect_test_app_as_client(app: &mut App) {
    app.session.as_mut().unwrap().net_endpoint = Some(
        crate::netcode::NetEndpoint::from_role(
            &crate::netcode::NetRole::Connect {
                addr: std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 1)),
            },
            None,
        )
        .expect("client endpoint constructs")
        .expect("connect role yields an endpoint"),
    );
}

/// A windowless `App` with an installed session, for App-level tests.
pub(crate) fn test_app() -> App {
    let script_ctx = ScriptCtx::new();
    let script_runtime = test_runtime(&script_ctx);
    let initial_state = InterpolableState::new(Vec3::ZERO);
    App {
        renderer: None,
        window_state: None,
        window_modes: crate::app::window_modes::WindowModes::new(false),
        level: None,
        nav_graph: None,
        map_path: None,
        content_root: PathBuf::from("content/dev"),
        baked_root: None,
        core_root: postretro_ui::CoreRoot::working_directory(),
        exit_result: Ok(()),
        camera: Camera::new(Vec3::ZERO, 0.0, 0.0),
        // Tests exercise level load/unload in the Running state, which touches
        // the session-owned modal stack and the whole script tranche; construct
        // a minimal `Session` inline. The registries (`classname_dispatch`,
        // `scripting.sequence_registry`, `scripting.reaction_registry`,
        // `scripting.system_registry`) are intentionally minimal/empty — these
        // lifecycle tests exercise level load/unload plumbing, not
        // reaction/classname dispatch; the real `Session::build` populates them.
        session: Some(crate::session::Session {
            input_system: input::InputSystem::new(input::default_bindings()),
            bindings: input::BindingState::default(),
            glyph_art: Default::default(),
            mod_ui_images: Default::default(),
            loading_screen: Default::default(),
            device_family: Default::default(),
            controls: Default::default(),
            pending_slider_steps: Vec::new(),
            gameplay_input_latch: input::GameplayInputLatch::new(),
            ui_dispatch: input::UiDispatch::new(),
            gamepad_system: None,
            input_focus: InputFocus::Gameplay,
            ui_focus: input::UiFocusEngine::new(),
            ui_focus_rects: None,
            ui_input_mode: input::InputMode::default(),
            modal_stack: postretro_ui::modal_stack::ModalStack::new(),
            font_system: postretro_ui::text::build_font_system(),
            scripting: crate::session::ScriptingCore {
                command_diagnostics: Default::default(),
                auto_close_timers: Default::default(),
                mover_auto_close_ms: crate::runtime_movers::ENGINE_AUTO_CLOSE_MS,
                descriptor_sounds: Default::default(),
                spawn_context: Default::default(),
                script_runtime,
                script_ctx: script_ctx.clone(),
                impact_policy_runtime: crate::impact_policy::ImpactPolicyRuntime::new(
                    script_ctx.clone(),
                ),
                sequence_registry: SequencedPrimitiveRegistry::new(),
                reaction_registry: scripting::reactions::registry::ReactionPrimitiveRegistry::new(),
                system_registry: scripting::reactions::system_commands::SystemReactionRegistry::new(
                ),
                system_reaction_ir_bindings: Default::default(),
                slot_accumulator_bindings: Default::default(),
                scheduler: Default::default(),
                player_hud_state: scripting_systems::ui_proxy::PlayerHudStatePublisher::new(
                    script_ctx.clone(),
                ),
                flash_decay: scripting_systems::flash_decay::FlashDecay::new(script_ctx.clone()),
                vignette_decay: scripting_systems::vignette_decay::VignetteDecay::new(
                    script_ctx.clone(),
                ),
                shake_decay: scripting_systems::shake_decay::ShakeDecay::new(script_ctx.clone()),
                input_mode_tracker: scripting_systems::input_mode::InputModeTracker::new(
                    script_ctx.clone(),
                ),
                client_level_verb_warnings: Default::default(),
            },
            presentation_cells: scripting_systems::presentation_cells::PresentationCellStore::new(),
            presentation_pool: crate::presentation_pool::PresentationPool::default(),
            client_overlay_facts: crate::netcode::ClientOverlayFactState::default(),
            host_overlay_fact_tracker: crate::netcode::HostOverlayFactTracker::default(),
            state_store_lifecycle: Default::default(),
            persisted_state: None,
            per_owner_save_timer: Default::default(),
            progress_tracker: ProgressTracker::new(),
            pending_death_events: Vec::new(),
            crossing_detector: CrossingDetector::new(),
            classname_dispatch: scripting::builtins::ClassnameDispatch::new(),
            light_bridge: scripting_systems::light_bridge::LightBridge::new(),
            fog_volume_bridge: scripting_systems::fog_volume_bridge::FogVolumeBridge::new(),
            trigger_volume_bridge:
                scripting_systems::trigger_volume_bridge::TriggerVolumeBridge::new(),
            trigger_system: crate::trigger_system::TriggerSystem::default(),
            touch_system: crate::sim::touch::TouchSystem::default(),
            emitter_bridge: scripting_systems::emitter_bridge::EmitterBridge::new(),
            particle_pool: postretro_sim::particle_pool::ParticlePool::new(),
            impact_particle_requests: Vec::new(),
            particle_render: scripting_systems::particle_render::ParticleRenderCollector::new(),
            mesh_render: scripting_systems::mesh_render::MeshRenderCollector::new(),
            mesh_clip_tables: scripting_systems::mesh_anim::MeshClipTables::new(),
            hit_zone_store: scripting_systems::hit_zones::HitZoneStore::new(),
            sh_streaming: None,
            level_streaming: crate::session::level_streaming::LevelStreaming::default(),
            options_bridge: options::OptionsBridge::new(),
            os_preferences: crate::os_preferences::OsPreferenceFeed::fake().0,
            player_options: options::PlayerOptions::default(),
            settings_path: None,
            data_dir: None,
            frontend: None,
            accessibility_entry_check: Default::default(),
            net_endpoint: None,
            seat_table: None,
            audio: None,
            #[cfg(feature = "dev-tools")]
            debug_ui: None,
        }),
        #[cfg(feature = "observe-live")]
        observe_live: None,
        remote_player_presentation: crate::netcode::ClientPresentationInputs::default(),
        crouch_toggle_active: false,
        sprint_toggle_active: false,
        ai_runtime: postretro_ai::AiRuntime::new(),
        cursor_pos: None,
        ui_wheel: None,
        nav_stick_tracker: input::StickNavTrackers::new(),
        frame_timing: FrameTiming::new(initial_state),
        view_feel_state: view_feel::ViewFeelState::default(),
        view_feel_followed_pawn: None,
        view_feel_descriptor: None,
        diagnostic_inputs: input::DiagnosticInputs::new(input::default_diagnostic_chords()),
        capture_portal_walk_next_frame: false,
        portal_walk_scratch: Default::default(),
        blocked_portals: Vec::new(),
        frame_rate_meter: FrameRateMeter::new(),
        cpu_timer: crate::cpu_timing::CpuFrameTimer::new(crate::cpu_timing::TimingGate::OFF),
        title_buffer: String::new(),
        last_title_update: Instant::now(),
        mod_theme_override: Default::default(),
        switching: Default::default(),
        pending_mode_signal: None,
        pending_menu_toggle: false,
        accessibility_panel_was_open: false,
        pending_exit_to_desktop: false,
        ui_focused_id: None,
        collision_world: crate::collision::CollisionWorld::new(),
        environment_volumes: postretro_collision::EnvironmentVolumes::empty(),
        environment_fluids: Default::default(),
        environment_fluid_source: Default::default(),
        fluid_surface_drift: Vec::new(),
        kinematic_mover_colliders: Vec::new(),
        kinematic_mover_tick_states: crate::kinematic_mover::MoverTickStateTable::default(),
        mover_yaw_carry_ground: postretro_foundation::GroundRef::Airborne,
        kinematic_mover_render: crate::runtime_movers::KinematicMoverRenderCollector::new(),
        fluid_surface_render: postretro_render_cpu::fluid_surface::FluidSurfaceRenderCollector::new(
        ),
        fluid_eye: Default::default(),
        planar_render: Default::default(),
        trigger_bindings: crate::trigger_bindings::TriggerBindingTable::default(),
        teleporter_bindings: Default::default(),
        ghost_links: Default::default(),
        ghost_bus: Default::default(),
        ghost_render: Default::default(),
        mover_ghost_render: Default::default(),
        billboard_ghost_render: Default::default(),
        player_events: Default::default(),
        trigger_pool_report: TriggerPoolInstallReport::default(),
        client_fire_resolutions: Vec::new(),
        client_weapon: Default::default(),
        active_slot_resync: Default::default(),
        observer_weapon_cues: Vec::new(),
        pending_teleport_sounds: Vec::new(),
        client_predicted_shots: crate::weapon::ClientPredictedShots::new(),
        client_reload_edges: Default::default(),
        client_overheat_edge: Default::default(),
        boot_state: BootState::Running,
        splash_frame: 0,
        os_wait_from: None,
        last_resolve_at: None,
        boot_destination: None,
        pending_level_log: false,
        settle: None,
        settle_summary: None,
        published_level_identity: None,
        pending_splash_override: None,
        host_spawn_points: Vec::new(),
        script_time: 0.0,
        anim_time: 0.0,
        anim_time_scale: 1.0,
        boot_timings: StartupTimings::new(),
        session_boot_config: crate::startup::session::SessionBootConfig::default(),
        mod_timings: StartupTimings::new(),
        level_timings: StartupTimings::new(),
        active_level_tags: Vec::new(),
        active_level_source: None,
        level_load: None,
        level_rx: None,
        level_worker: None,
        level_requests: VecDeque::new(),
        boot_load: false,
        pending_session: None,
        #[cfg(feature = "dev-tools")]
        debug_chase_agent: None,
    }
}

/// Clone the session-owned `ScriptCtx` handle (cheap `Rc` bump) for tests.
/// The scripting core lives on `Session`; this keeps the many test reads of
/// the shared registries one short call away without a borrow fight against
/// the non-`Clone` session subsystems.
pub(crate) fn script_ctx(app: &App) -> ScriptCtx {
    app.session
        .as_ref()
        .expect("test app session installed")
        .scripting
        .script_ctx
        .clone()
}

pub(super) fn slot_snapshot(app: &App) -> BTreeMap<String, SlotRecord> {
    let ctx = script_ctx(app);
    let slots = ctx.slot_table.borrow();
    let first_name = slots
        .iter()
        .next()
        .map(|(name, _)| name.to_string())
        .expect("slot table should carry engine slots");
    assert!(
        slots.get(&first_name).is_some(),
        "slot snapshot must exercise SlotTable::get"
    );
    slots
        .iter()
        .map(|(name, record)| (name.to_string(), record.clone()))
        .collect()
}

pub(super) fn descriptor(name: &str) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        ghost: false,
        faction: None,
        tolerance: None,
        canonical_name: Some(name.to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
    }
}

pub(super) fn catalog_map(id: &str, path: &str, name: &str, tags: &[&str]) -> ModMapEntry {
    ModMapEntry {
        id: id.to_string(),
        path: path.to_string(),
        name: name.to_string(),
        tags: tags.iter().map(|tag| tag.to_string()).collect(),
        loading_tree: Vec::new(),
    }
}

pub(super) fn drop_in_flight_worker(app: &mut App) {
    app.level_rx = None;
    if let Some(handle) = app.level_worker.take() {
        handle
            .join()
            .expect("level worker should not panic during lifecycle test");
    }
    app.level_load = None;
}

pub(super) fn map_light(tag: &str, origin: [f64; 3]) -> postretro_level_loader::MapLight {
    postretro_level_loader::MapLight {
        origin,
        light_type: postretro_level_loader::LightType::Point,
        intensity: 1.0,
        color: [1.0, 0.8, 0.6],
        falloff_model: postretro_level_loader::FalloffModel::InverseSquared,
        falloff_range: 16.0,
        cone_angle_inner: 0.0,
        cone_angle_outer: 0.0,
        cone_direction: [0.0, 0.0, 0.0],
        is_dynamic: false,
        casts_entity_shadows: false,
        animated_slot: None,
        tags: vec![tag.to_string()],
        cell_index: 0,
        shadow_type: postretro_level_loader::ShadowType::StaticLightMap,
    }
}

pub(super) fn fog_record(
    tag: &str,
    center_x: f32,
) -> postretro_level_format::fog_volumes::FogVolumeRecord {
    postretro_level_format::fog_volumes::FogVolumeRecord {
        min: [center_x - 1.0, 0.0, -1.0],
        density: 0.5,
        max: [center_x + 1.0, 2.0, 1.0],
        edge_softness: 0.25,
        glow: 0.0,
        radial_falloff: 1.0,
        center: [center_x, 1.0, 0.0],
        inv_half_ext: [1.0, 1.0, 1.0],
        half_diag: 2.0,
        shape_mode: 0.0,
        tint: [1.0, 1.0, 1.0],
        saturation: 1.0,
        min_brightness: 0.0,
        light_range: 1.0,
        anisotropy: 0.0,
        ambient_scatter: 1.0,
        plane_count: 0,
        planes: vec![],
        tags: vec![tag.to_string()],
    }
}

fn vertex(position: [f32; 3]) -> postretro_render_data::geometry::WorldVertex {
    postretro_render_data::geometry::WorldVertex {
        position,
        base_uv: [0.0, 0.0],
        normal_oct: [0, 0],
        tangent_packed: [0, 0],
        lightmap_uv: [0, 0],
        lightmap_block: 0,
        animated_block: 0,
    }
}

pub(crate) fn level_world(
    _name: &str,
    triangle_count: usize,
) -> postretro_level_loader::LevelWorld {
    let mut vertices = vec![
        vertex([0.0, 0.0, 0.0]),
        vertex([1.0, 0.0, 0.0]),
        vertex([0.0, 1.0, 0.0]),
    ];
    let mut indices = vec![0, 1, 2];
    if triangle_count > 1 {
        vertices.extend([
            vertex([2.0, 0.0, 0.0]),
            vertex([3.0, 0.0, 0.0]),
            vertex([2.0, 1.0, 0.0]),
        ]);
        indices.extend([3, 4, 5]);
    }

    postretro_level_loader::LevelWorld {
        vertices,
        indices,
        face_meta: Vec::new(),
        cells: vec![postretro_level_loader::CellData {
            bounds_min: Vec3::ZERO,
            bounds_max: Vec3::ONE,
            face_start: 0,
            face_count: 0,
            portal_ref_start: 0,
            portal_ref_count: 0,
            is_solid: false,
            is_exterior: false,
            is_drawable: false,
        }],
        cell_portal_refs: vec![],
        cell_locator_root: postretro_level_loader::CellLocatorChild::Cell(0),
        cell_locator_nodes: vec![],
        portals: Vec::new(),
        has_portals: false,
        portal_planes: Default::default(),
        cell_visibility: None,
        texture_names: Vec::new(),
        texture_cache_keys: Default::default(),
        bvh: postretro_render_data::geometry::BvhTree {
            nodes: Vec::new(),
            leaves: Vec::new(),
            root_node_index: 0,
        },
        lights: Vec::new(),
        light_influences: Vec::new(),
        sh_volume: None,
        sh_storage: postretro_level_loader::ShStorage::Legacy,
        lightmap: None,
        lightmap_mode: Default::default(),
        sdf_atlas: None,
        chunk_light_list: None,
        animated_light_chunks: None,
        animated_light_weight_maps: None,
        delta_sh_volumes: None,
        direct_sh_volume: None,
        direct_sh_delta_volumes: None,
        animated_direct_sh_delta_volumes: None,
        billboard_direct_scatter_volume: None,
        animated_billboard_direct_scatter_delta_volumes: None,
        entity_shadow_lights: vec![],
        shadowmask_atlas: None,
        vertex_spec_owners: Vec::new(),
        gpu_lighting_payloads: Default::default(),
        lightmap_storage: Default::default(),
        data_script: None,
        map_entities: Vec::new(),
        kinematic_geometry: postretro_level_loader::KinematicGeometry::default(),
        trigger_volumes: Vec::new(),
        fog_volumes: Vec::new(),
        fog_pixel_scale: 4,
        environment_volumes: Default::default(),
        teleporters: Default::default(),
        ghost_links: Default::default(),
        environment_push_forces: Vec::new(),
        reflection_probes: None,
        fog_cell_masks: None,
        navmesh: None,
        cell_draw_index: None,
        cluster_directory: None,
        cell_residency_set: None,
        prl_read_counters: None,
    }
}

pub(crate) struct TriggerPoolWorldInstall {
    pub(super) report: TriggerPoolInstallReport,
    pub(crate) registry: postretro_entities::EntityRegistry,
    pub(super) upload_models: Vec<String>,
    pub(super) hit_zone_store: crate::scripting_systems::hit_zones::HitZoneStore,
}

pub(crate) fn install_descriptor_world(
    world: postretro_level_loader::LevelWorld,
    policy: TriggerPoolSeedPolicy,
    active_level_tags: &[&str],
    global_pools: Vec<TriggerPoolDescriptor>,
    suppress_host_replicated: bool,
    descriptors: Vec<EntityTypeDescriptor>,
) -> TriggerPoolWorldInstall {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    ctx.data_registry.borrow_mut().entities = descriptors;
    let mut upload_models = Vec::new();
    let content_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
    ctx.data_registry
        .borrow_mut()
        .replace_global_trigger_pools(global_pools);
    let active_level_tags: Vec<String> = active_level_tags
        .iter()
        .map(|tag| (*tag).to_string())
        .collect();
    app.collision_world = crate::collision::CollisionWorld::from_level(&world)
        .expect("fixture collision mesh is valid");
    let mut timings = StartupTimings::new();
    let products = {
        let session = app.session.as_mut().expect("test app session installed");
        let handles = WorldInstallHandles {
            command_diagnostics: Default::default(),
            mover_auto_close_ms: crate::runtime_movers::ENGINE_AUTO_CLOSE_MS,
            spawn_context: Default::default(),
            world: &world,
            script_ctx: &ctx,
            content_root: &content_root,
            active_level_tags: &active_level_tags,
            nav_graph: None,
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
            trigger_pool_policy: policy,
            suppress_host_replicated,
            suppress_boot_pawn: suppress_host_replicated,
            local_carried_loadout: None,
        };
        install_world_cpu(
            handles,
            &mut timings,
            |models, _clip_tables| {
                upload_models.extend(models.iter().map(|model| {
                    assert!(model.result.is_ok(), "{} must parse", model.handle);
                    model.handle.clone()
                }));
                crate::scripting_systems::hit_zones::ModelLoadWarningOwner::GameSide
            },
            |_first_player| {},
        )
    };

    let registry = std::mem::take(&mut *ctx.registry.borrow_mut());
    TriggerPoolWorldInstall {
        report: products.trigger_pool_report,
        registry,
        upload_models,
        hit_zone_store: std::mem::take(&mut app.session.as_mut().unwrap().hit_zone_store),
    }
}

/// Production segment-B install and presentation finalization for CPU tests.
pub(crate) fn install_capture_world(app: &mut App, world: &postretro_level_loader::LevelWorld) {
    let ctx = script_ctx(app);
    let content_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
    let session = app.session.as_mut().unwrap();
    session
        .light_bridge
        .populate_from_level(&world.lights, &mut ctx.registry.borrow_mut(), 0);
    let handles = WorldInstallHandles {
        world,
        script_ctx: &ctx,
        command_diagnostics: Default::default(),
        mover_auto_close_ms: crate::runtime_movers::ENGINE_AUTO_CLOSE_MS,
        spawn_context: Default::default(),
        content_root: &content_root,
        active_level_tags: &[],
        nav_graph: None,
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
        suppress_boot_pawn: true,
        local_carried_loadout: None,
    };
    let _ = install_world_cpu(
        handles,
        &mut StartupTimings::new(),
        |_models, _clips| crate::scripting_systems::hit_zones::ModelLoadWarningOwner::GameSide,
        |_first_player| {},
    );
    start_installed_presentation_animations(
        &ctx.registry.borrow(),
        &mut session.light_bridge,
        &mut session.fog_volume_bridge,
        &mut session.emitter_bridge,
    );
    app.script_time = 0.0;
}

/// Parse identical raw descriptors through a bundled TS entry and the Luau
/// data-script runtime. Builder serialization is covered by the SDK tests.
pub(crate) fn runtime_audience_manifest(
    value: &serde_json::Value,
    luau: bool,
) -> postretro_scripting_core::data_descriptors::LevelManifest {
    fn lua(value: &serde_json::Value) -> String {
        use serde_json::Value;
        match value {
            Value::Null => {
                panic!("Lua nil omits a field; parity fixtures use representable values")
            }
            Value::Bool(_) | Value::Number(_) | Value::String(_) => value.to_string(),
            Value::Array(values) => format!(
                "{{{}}}",
                values.iter().map(lua).collect::<Vec<_>>().join(",")
            ),
            Value::Object(values) => format!(
                "{{{}}}",
                values
                    .iter()
                    .map(|(key, value)| format!(
                        "[{}]={}",
                        serde_json::to_string(key).unwrap(),
                        lua(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join(if luau { "audience.luau" } else { "audience.ts" });
    let source = if luau {
        format!("function setupLevel(_ctx) return {} end", lua(value))
    } else {
        format!("export function setupLevel() {{ return {value}; }}")
    };
    std::fs::write(&path, &source).unwrap();
    let source = if luau {
        source
    } else {
        postretro_script_compiler::bundle_entry(&path).expect("raw audience TS fixture bundles")
    };
    let ctx = ScriptCtx::new();
    let runtime = ScriptRuntime::new(
        &PrimitiveRegistry::new(),
        &ScriptRuntimeConfig::default(),
        &ctx,
    )
    .unwrap();
    runtime.run_data_script(
        &postretro_level_format::data_script::DataScriptSection {
            compiled_bytes: source.into_bytes(),
            source_path: path.to_string_lossy().into_owned(),
        },
        dir.path(),
    )
}

/// Shim marker ending `lifecycle.rs`'s production code: the `tests` module
/// that re-exports these fixtures.
const LIFECYCLE_TESTS_MARKER: &str = "#[cfg(test)]\npub(crate) mod tests";

/// The files holding `install_level_payload` and the stages it calls. None
/// of them holds tests, so all of each file is production code.
const INSTALL_SOURCES: [&str; 3] = [
    include_str!("lifecycle_install.rs"),
    include_str!("lifecycle_install_world.rs"),
    include_str!("lifecycle_install_presentation.rs"),
];

/// Production code of every file carved from the one `lifecycle.rs` the
/// older source guards pinned, so a guard that counted a call in that file
/// still counts it across all of them.
pub(in crate::startup) fn lifecycle_production_source() -> String {
    let lifecycle = include_str!("lifecycle.rs")
        .split(LIFECYCLE_TESTS_MARKER)
        .next()
        .expect("split yields the production prefix");
    [
        lifecycle,
        include_str!("lifecycle_finish.rs"),
        include_str!("lifecycle_install.rs"),
        include_str!("lifecycle_install_world.rs"),
        include_str!("lifecycle_install_presentation.rs"),
        include_str!("lifecycle_reaction_rebuild.rs"),
        include_str!("lifecycle_spawners.rs"),
        include_str!("lifecycle_world_handles.rs"),
    ]
    .concat()
}

/// One `impl` method's text, from `fn name(` through its closing brace.
/// Methods sit at four-space indent, so the first `\n    }\n` after the
/// signature closes the method and nothing nested inside it.
pub(in crate::startup) fn method_source<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let start = source.find(&format!("fn {name}("))?;
    let rest = &source[start..];
    let end = rest.find("\n    }\n")? + "\n    }".len();
    Some(&rest[..end])
}

/// Names of the methods called as `self.name(` on one line.
fn self_calls(line: &str) -> Vec<&str> {
    line.match_indices("self.")
        .filter_map(|(at, _)| {
            let rest = &line[at + "self.".len()..];
            let len = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            (len > 0 && rest[len..].starts_with('(')).then(|| &rest[..len])
        })
        .collect()
}

/// Every method defined at four-space indent in the install files.
fn install_methods() -> Vec<&'static str> {
    INSTALL_SOURCES
        .iter()
        .flat_map(|source| source.lines())
        .filter_map(|line| {
            let item = line.strip_prefix("    ")?;
            let item = item.strip_prefix("pub(super) ").unwrap_or(item);
            let name = item.strip_prefix("fn ")?;
            Some(&name[..name.find('(')?])
        })
        .collect()
}

/// `install_level_payload` as one text, for the install-order source guards:
/// the sequencer with each install-file method it calls spliced in, in call
/// order, just before the line that calls it. A statement-order guard then
/// reads every stage where it runs. Every install-file method must be
/// reached from the sequencer, so no stage escapes the guards.
pub(in crate::startup) fn install_level_payload_source() -> String {
    fn expand(text: &str, depth: usize, reached: &mut Vec<&'static str>) -> String {
        assert!(
            depth < 4,
            "install stages nest deeper than the guards expect"
        );
        let mut out = String::new();
        for line in text.split_inclusive('\n') {
            for name in self_calls(line) {
                let Some(stage) = INSTALL_SOURCES
                    .iter()
                    .find_map(|source| method_source(source, name))
                else {
                    continue;
                };
                let name = install_methods()
                    .into_iter()
                    .find(|method| *method == name)
                    .expect("a spliced stage is an install-file method");
                reached.push(name);
                out.push_str(&expand(stage, depth + 1, reached));
                out.push('\n');
            }
            out.push_str(line);
        }
        out
    }
    let sequencer = INSTALL_SOURCES
        .iter()
        .find_map(|source| method_source(source, "install_level_payload"))
        .expect("install_level_payload exists");
    let mut reached = vec!["install_level_payload"];
    let flattened = expand(sequencer, 0, &mut reached);
    for method in install_methods() {
        assert!(
            reached.contains(&method),
            "install-file method `{method}` is not reached from install_level_payload"
        );
    }
    flattened
}
