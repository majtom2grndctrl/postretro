// Postretro engine entry point and level-load orchestration.
// See: context/lib/index.md (routes to rendering_pipeline.md, scripting.md, etc.)

mod camera;
mod compute_cull;
mod frame_timing;
mod fx;
mod geometry;
mod input;
mod lighting;
mod material;

mod portal_vis;
mod prl;
mod render;
mod scripting;
mod texture;
mod visibility;

// Rooted here (not under `scripting/`) so `gen_script_types.rs` can reuse the
// `scripting` tree via `#[path]` without pulling in wgpu/engine-dependent code.
#[path = "scripting/systems/mod.rs"]
mod scripting_systems;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use glam::Vec3;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey, PhysicalKey};
use winit::window::{Window, WindowAttributes};

use crate::camera::Camera;
use crate::frame_timing::{FrameRateMeter, FrameTiming, InterpolableState};
use crate::input::{Action, DiagnosticAction};
use crate::render::Renderer;
use crate::scripting::builtins::{
    ClassnameDispatch, apply_classname_dispatch, apply_data_archetype_dispatch,
    register_builtins as register_builtin_classnames,
};
use crate::scripting::call_context::ScriptCallContext;
use crate::scripting::ctx::ScriptCtx;
use crate::scripting::primitives::register_all;
use crate::scripting::primitives_light::register_sequenced_light_primitives;
use crate::scripting::primitives_registry::PrimitiveRegistry;
use crate::scripting::reaction_dispatch::{
    ProgressTracker, fire_named_event_with_sequences, validate_sequence_primitives,
};
use crate::scripting::reactions::registry::{
    ReactionPrimitiveRegistry, register_emitter_reaction_primitives,
};
use crate::scripting::runtime::{ScriptRuntime, ScriptRuntimeConfig, Which as ScriptWhich};
use crate::scripting::sequence::SequencedPrimitiveRegistry;
use crate::texture::TextureSet;
use crate::visibility::{VisibilityPath, VisibilityStats, VisibleCells};

const DEFAULT_MAP_PATH: &str = "content/tests/maps/test-3.prl";

fn resolve_map_path(args: &[String]) -> String {
    args.iter()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| DEFAULT_MAP_PATH.to_string())
}

fn content_root_from_map(map_path: &str) -> PathBuf {
    // `Path::new("maps/test.prl").parent()` returns `Some("maps")`, and
    // `"maps".parent()` returns `Some("")` — an empty path, not `None`. Filter
    // out the empty case so the `unwrap_or` fallback to `"."` actually fires.
    Path::new(map_path)
        .parent()
        .and_then(|maps_dir| maps_dir.parent())
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf()
}

fn load_level(path: &str) -> Result<Option<prl::LevelWorld>> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "prl" => match prl::load_prl(path) {
            Ok(world) => {
                log::info!("[Engine] PRL loaded successfully from {path}");
                Ok(Some(world))
            }
            Err(prl::PrlLoadError::FileNotFound(p)) => {
                log::warn!("[Engine] PRL file not found: {p} — starting without map");
                Ok(None)
            }
            Err(err) => anyhow::bail!("failed to load PRL: {err}"),
        },
        _ => {
            log::error!(
                "[Engine] Unknown file extension '.{ext}' for {path} — only .prl is supported"
            );
            Ok(None)
        }
    }
}

fn main() -> Result<()> {
    env_logger::init();
    log::info!("[Engine] Postretro starting");

    let args: Vec<String> = std::env::args().collect();

    let map_path = resolve_map_path(&args);
    let content_root = content_root_from_map(&map_path);
    log::info!("[Engine] Content root: {}", content_root.display());
    let mut level = load_level(&map_path)?;

    let texture_set = match &level {
        Some(world) if !world.texture_names.is_empty() => {
            let texture_root = content_root.join("textures");
            log::info!(
                "[Engine] Loading PRL textures from {}",
                texture_root.display()
            );
            let texture_names: Vec<Option<String>> = world
                .texture_names
                .iter()
                .map(|n| Some(n.clone()))
                .collect();
            Some(texture::load_textures(&texture_names, &texture_root))
        }
        _ => None,
    };

    // Normalize PRL UVs after texture dimensions are known.
    if let (Some(world), Some(tex_set)) = (&mut level, &texture_set) {
        normalize_prl_uvs(world, tex_set);
    }

    let initial_camera_pos = match &level {
        Some(world) => world.spawn_position(),
        None => Vec3::new(0.0, 200.0, 500.0),
    };

    let event_loop = EventLoop::new().context("failed to create event loop")?;

    let initial_state = InterpolableState::new(initial_camera_pos);

    // Scripting bootstrap. Behavior scripts load lexicographically so
    // cross-file `registerHandler` order is deterministic. `fire_level_load`
    // runs after world population but before the first frame; `fire_tick`
    // each frame after game logic. See: context/lib/scripting.md
    let script_ctx = ScriptCtx::new();
    let mut script_registry = PrimitiveRegistry::new();
    register_all(&mut script_registry, script_ctx.clone());
    let mut script_runtime = ScriptRuntime::new(
        &script_registry,
        &ScriptRuntimeConfig::default(),
        &script_ctx,
    )
    .context("failed to construct script runtime")?;

    // Rust-only handlers on the sequence-dispatch path — distinct from the
    // script-facing primitive registry (these never run inside QuickJS/Luau).
    let mut sequence_registry = SequencedPrimitiveRegistry::new();
    register_sequenced_light_primitives(&mut sequence_registry, script_ctx.clone());

    // Reaction-primitive handlers invoked by name when a `Primitive` reaction
    // fires. Populated once at startup; survives level reloads.
    let mut reaction_registry = ReactionPrimitiveRegistry::new();
    register_emitter_reaction_primitives(&mut reaction_registry);

    // Built-in classname dispatch — survives level unload because handlers
    // describe engine types, not per-level state. See: context/lib/scripting.md
    let mut classname_dispatch = ClassnameDispatch::new();
    register_builtin_classnames(&mut classname_dispatch);

    // Failure to start the watcher is logged and swallowed — a missing or
    // unwatchable scripts directory must not prevent engine startup.
    let scripts_root = content_root.join("scripts");
    if let Err(err) = script_runtime.start_watcher(&scripts_root) {
        log::warn!(
            "[Scripting] failed to start hot-reload watcher on `{}`: {err}",
            scripts_root.display(),
        );
    }

    let mut app = App {
        renderer: None,
        window_state: None,
        level,
        texture_set,
        content_root,
        exit_result: Ok(()),
        camera: Camera::new(initial_camera_pos, 0.0, 0.0),
        input_system: input::InputSystem::new(input::default_bindings()),
        gamepad_system: input::gamepad::GamepadSystem::new(),
        frame_timing: FrameTiming::new(initial_state),
        diagnostic_inputs: input::DiagnosticInputs::new(input::default_diagnostic_chords()),
        capture_portal_walk_next_frame: false,
        scratch_cells: Vec::new(),
        frame_rate_meter: FrameRateMeter::new(),
        title_buffer: String::with_capacity(256),
        last_title_update: Instant::now(),
        script_runtime,
        script_ctx,
        sequence_registry,
        reaction_registry,
        progress_tracker: ProgressTracker::new(),
        classname_dispatch,
        light_bridge: scripting_systems::light_bridge::LightBridge::new(),
        fog_volume_bridge: scripting_systems::fog_volume_bridge::FogVolumeBridge::new(),
        emitter_bridge: scripting_systems::emitter_bridge::EmitterBridge::new(),
        particle_render: scripting_systems::particle_render::ParticleRenderCollector::new(),
        level_load_fired: false,
        builtin_handled: None,
        script_time: 0.0,
    };

    event_loop
        .run_app(&mut app)
        .context("event loop terminated with error")?;

    app.exit_result
}

fn normalize_prl_uvs(world: &mut prl::LevelWorld, texture_set: &TextureSet) {
    let mut normalized = vec![false; world.vertices.len()];

    for leaf in &world.bvh.leaves {
        let tex_idx = leaf.material_bucket_id as usize;
        let (w, h) = match texture_set.textures.get(tex_idx) {
            Some(tex) => (tex.width, tex.height),
            None => continue,
        };
        if w == 0 || h == 0 {
            continue;
        }

        let start = leaf.index_offset as usize;
        let count = leaf.index_count as usize;
        for i in start..start + count {
            if let Some(&idx) = world.indices.get(i) {
                let vi = idx as usize;
                // The compiler emits a fresh vertex copy per face, so sharing
                // across leaves is not expected — this guard is defensive
                // against future vertex deduplication.
                if vi < normalized.len() && !normalized[vi] {
                    if let Some(vert) = world.vertices.get_mut(vi) {
                        vert.base_uv[0] /= w as f32;
                        vert.base_uv[1] /= h as f32;
                        normalized[vi] = true;
                    }
                }
            }
        }
    }
}

fn load_behavior_scripts(runtime: &ScriptRuntime, content_root: &Path) {
    let root_buf = content_root.join("scripts");
    let root = root_buf.as_path();
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(err) => {
            log::debug!(
                "[Scripting] `{}` not found ({err}); no behavior scripts loaded",
                root.display(),
            );
            return;
        }
    };
    let all_paths: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && matches!(
                    p.extension().and_then(|s| s.to_str()),
                    Some("ts") | Some("js") | Some("luau")
                )
        })
        .collect();

    // Build the set of stems that have a `.ts` source so we can skip
    // same-stem `.js` files (compiler artifacts). All paths share the same
    // root so direct stem comparison (no canonicalize — the bare stem path
    // does not exist on disk and would always fail to resolve) is sufficient.
    let ts_stems: std::collections::HashSet<std::path::PathBuf> = all_paths
        .iter()
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("ts"))
        .map(|p| p.with_extension(""))
        .collect();

    let mut paths: Vec<std::path::PathBuf> = all_paths
        .into_iter()
        .filter(|p| {
            if p.extension().and_then(|s| s.to_str()) == Some("js") {
                // Skip `.js` files that are compiler artifacts — identified by
                // the presence of a same-stem `.ts` sibling.
                if ts_stems.contains(&p.with_extension("")) {
                    return false;
                }
            }
            true
        })
        .collect();

    // Sort by UTF-8 byte order — see context/lib/scripting.md §8 for the
    // ordering contract.
    paths.sort();

    // In debug builds, compile `.ts` → `.js` before loading. The watcher
    // module (and TsCompilerPath) is only compiled under debug_assertions.
    #[cfg(debug_assertions)]
    let ts_compiler = crate::scripting::watcher::TsCompilerPath::detect();

    #[cfg(debug_assertions)]
    if ts_compiler.is_none() {
        log::warn!(
            "[Scripting] `scripts-build` not found. `.ts` files will be passed to \
             QuickJS as-is and will likely fail with \"Unexpected token\" errors. \
             Install scripts-build or ship it next to the engine binary."
        );
    }

    for path in &paths {
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");

        // In debug builds, compile `.ts` to `.js` first.
        #[cfg(debug_assertions)]
        if ext == "ts" {
            match &ts_compiler {
                Some(compiler) => {
                    let out_path = crate::scripting::watcher::compiled_output_for(path);
                    match crate::scripting::watcher::run_ts_compiler(compiler, path, &out_path) {
                        Ok(()) => {
                            if let Err(err) =
                                runtime.run_script_file(ScriptWhich::Behavior, &out_path)
                            {
                                log::error!(
                                    "[Scripting] failed to load compiled `{}`: {err}",
                                    out_path.display(),
                                );
                            }
                        }
                        Err(msg) => {
                            log::error!(
                                "[Scripting] TS compile failed for `{}`: {msg}",
                                path.display(),
                            );
                        }
                    }
                    continue;
                }
                None => {
                    // No compiler — fall through to loading the raw `.ts`
                    // (preserves prior behavior; warning already logged above).
                }
            }
        }

        // `.js`, `.luau`, or `.ts` with no compiler available.
        if let Err(err) = runtime.run_script_file(ScriptWhich::Behavior, path) {
            log::error!("[Scripting] failed to load `{}`: {err}", path.display(),);
        }
    }
}

fn window_attributes() -> WindowAttributes {
    Window::default_attributes()
        .with_title("Postretro")
        .with_inner_size(winit::dpi::LogicalSize::new(1280, 720))
}

// --- Application state ---

struct App {
    renderer: Option<Renderer>,
    window_state: Option<WindowState>,
    level: Option<prl::LevelWorld>,
    texture_set: Option<TextureSet>,

    /// Derived from the map path at startup. `textures/` and `scripts/`
    /// sibling directories are resolved relative to this root.
    content_root: PathBuf,

    exit_result: Result<()>,

    camera: Camera,
    input_system: input::InputSystem,
    gamepad_system: Option<input::gamepad::GamepadSystem>,
    frame_timing: FrameTiming,

    /// Parallel to `input_system`; same key events, debug actions only.
    /// See: context/lib/input.md §7
    diagnostic_inputs: input::DiagnosticInputs,

    /// One-shot flag: set by `DumpPortalWalk`, consumed and cleared on the
    /// next redraw. Visibility emits per-portal traces under
    /// `postretro::portal_trace` for that one frame only.
    capture_portal_walk_next_frame: bool,

    scratch_cells: Vec<u32>,

    /// Ring buffer of per-frame CPU durations. Reports min/avg/max so
    /// hitches don't vanish into the average.
    frame_rate_meter: FrameRateMeter,

    /// Reused across frames to avoid a per-frame `format!` allocation.
    title_buffer: String,

    /// Rate-limits title writes to ~4Hz — at 60fps rapid `set_title` is
    /// unreadable and the OS may throttle it.
    last_title_update: Instant,

    script_runtime: ScriptRuntime,

    /// Holds the entity registry shared by the light bridge and the script
    /// runtime. Outlives the renderer so device resets preserve scripted
    /// light state. See: context/lib/scripting.md
    script_ctx: ScriptCtx,

    /// Consulted by `fire_named_event_with_sequences` for `Sequence` steps.
    /// No per-level state — entity lookups go through `ScriptCtx`, which the
    /// level-unload path clears separately. See: context/lib/scripting.md §2
    sequence_registry: SequencedPrimitiveRegistry,

    /// Resolved by name when a `Primitive` reaction fires.
    /// See: context/lib/scripting.md §2
    reaction_registry: ReactionPrimitiveRegistry,

    /// Per-tag kill-count subscriptions. Cleared on level unload
    /// independently of the behavior `HandlerTable`.
    /// See: context/lib/scripting.md §2
    progress_tracker: ProgressTracker,

    /// Maps `classname` strings to engine spawn handlers. Survives level
    /// unload — built-in handlers carry no per-level state.
    /// See: context/lib/scripting.md
    classname_dispatch: ClassnameDispatch,

    /// Runs between Game Logic and Render; uploads repacked GpuLight bytes
    /// when any `LightComponent` is dirty. See: context/lib/scripting.md
    light_bridge: scripting_systems::light_bridge::LightBridge,

    /// Per-level fog-volume registry side-table; packs `FogVolume` GPU bytes
    /// each frame from `FogVolumeComponent` mutations. See:
    /// context/lib/rendering_pipeline.md §7.5
    fog_volume_bridge: scripting_systems::fog_volume_bridge::FogVolumeBridge,

    /// Walks every `BillboardEmitterComponent` after script `tick` handler and
    /// before particle sim. See: context/lib/scripting.md
    emitter_bridge: scripting_systems::emitter_bridge::EmitterBridge,

    /// Packs `SpriteInstance` bytes per collection in the Render stage;
    /// never touches wgpu directly. See: context/lib/scripting.md
    particle_render: scripting_systems::particle_render::ParticleRenderCollector,

    /// Gates first-frame work: ensures `levelLoad` handlers run before the
    /// first `tick` and before the first render.
    level_load_fired: bool,

    /// Classnames the built-in dispatch handled at level open. Captured in
    /// `resumed()` and consumed by the data-archetype sweep during the
    /// `!level_load_fired` cold path on the first redraw frame. `None` before
    /// level load and after the sweep consumes it.
    builtin_handled: Option<std::collections::HashSet<String>>,

    /// Seconds since level load, not wall clock. Resets to zero on level
    /// unload; fed into `ScriptCallContext::time` each tick.
    script_time: f32,
}

struct WindowState {
    window: Arc<Window>,
}

// --- ApplicationHandler ---

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = match event_loop.create_window(window_attributes()) {
            Ok(w) => Arc::new(w),
            Err(err) => {
                self.exit_result = Err(anyhow::anyhow!("failed to create window: {err}"));
                event_loop.exit();
                return;
            }
        };

        // Derive material properties from texture names once so the renderer
        // can populate per-material uniforms (shininess) without re-parsing.
        let texture_materials: Vec<crate::material::Material> = self
            .level
            .as_ref()
            .map(|world| {
                let mut warned = std::collections::HashSet::new();
                world
                    .texture_names
                    .iter()
                    .map(|n| crate::material::derive_material(n, &mut warned))
                    .collect()
            })
            .unwrap_or_default();

        let geometry = self.level.as_ref().map(|world| render::LevelGeometry {
            vertices: &world.vertices,
            indices: &world.indices,
            bvh: &world.bvh,
            lights: &world.lights,
            light_influences: &world.light_influences,
            sh_volume: world.sh_volume.as_ref(),
            lightmap: world.lightmap.as_ref(),
            chunk_light_list: world.chunk_light_list.as_ref(),
            animated_light_chunks: world.animated_light_chunks.as_ref(),
            animated_light_weight_maps: world.animated_light_weight_maps.as_ref(),
            delta_sh_volumes: world.delta_sh_volumes.as_ref(),
            texture_materials: &texture_materials,
        });

        let mut renderer =
            match Renderer::new(&window, geometry.as_ref(), self.texture_set.as_ref()) {
                Ok(r) => r,
                Err(err) => {
                    self.exit_result = Err(err);
                    event_loop.exit();
                    return;
                }
            };

        let size = window.inner_size();
        self.camera.update_aspect(size.width, size.height);

        input::cursor::capture_cursor(&window);

        // One `LightComponent` entity per map-authored light; stable `EntityId`s
        // the bridge's dirty tracker keys off for the level's lifetime.
        {
            let level_lights = renderer.level_lights().to_vec();
            let fgd_sample_float_count = (renderer.scripted_sample_byte_offset() / 4) as u32;
            let mut registry = self.script_ctx.registry.borrow_mut();
            self.light_bridge.populate_from_level(
                &level_lights,
                &mut registry,
                fgd_sample_float_count,
            );
        }

        // Fog volumes — one entity per record + a renderer-side pixel-scale
        // push. Done after light bridge populate so the registry's first fog
        // entity-id always lands after the light entities.
        if let Some(world) = self.level.as_ref() {
            let mut registry = self.script_ctx.registry.borrow_mut();
            if let Err(err) = self
                .fog_volume_bridge
                .populate_from_level(&mut registry, &world.fog_volumes)
            {
                log::warn!("[FogVolumeBridge] populate failed: {err}");
            }
            renderer.set_fog_pixel_scale(world.fog_pixel_scale);
        }

        // Sweep map entities through classname dispatch. The returned set of
        // handled classnames is stashed and consumed by the data-archetype sweep
        // on the first redraw, after the data script populates
        // `data_registry.entities` via `registerEntity`.
        //
        // No re-entry guard is needed here: this engine targets desktop only,
        // and winit fires `resumed()` exactly once on desktop platforms (it is
        // only called multiple times on Android/iOS during surface re-creation).
        if let Some(world) = self.level.as_ref() {
            let mut registry = self.script_ctx.registry.borrow_mut();
            // Adapt the wire records to the scripting-tree representation at
            // the dispatch boundary. The loader does not depend on scripting
            // types; conversion happens here.
            let map_entities: Vec<crate::scripting::map_entity::MapEntity> =
                world.map_entities.iter().cloned().map(Into::into).collect();
            // Returns classnames claimed by built-in handlers regardless of
            // spawn success; passed to the data-archetype sweep to prevent
            // double-handling even when a built-in handler fails to materialize.
            let handled =
                apply_classname_dispatch(&map_entities, &self.classname_dispatch, &mut registry);
            if !map_entities.is_empty() {
                log::info!(
                    "[Loader] dispatched {total} map entities; {built_in} classname(s) handled by built-in handlers",
                    built_in = handled.len(),
                    total = map_entities.len(),
                );
            }
            self.builtin_handled = Some(handled);
        }

        // Register sprite collections for every distinct `sprite` name in the
        // registry. Covers both map-spawned and future script-spawned emitters.
        // Missing frames register a 1×1 white fallback so the pipeline stays wired.
        let texture_root = self.content_root.join("textures");
        {
            use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;
            use crate::scripting::registry::{ComponentKind, ComponentValue};
            let registry = self.script_ctx.registry.borrow();
            let mut registered: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for (_id, value) in registry.iter_with_kind(ComponentKind::BillboardEmitter) {
                let ComponentValue::BillboardEmitter(c) = value else {
                    continue;
                };
                let _: &BillboardEmitterComponent = c; // type pin
                let collection = c.sprite.clone();
                if collection.is_empty() || !registered.insert(collection.clone()) {
                    continue;
                }
                let frames = fx::smoke::load_collection_frames(&texture_root, &collection)
                    .unwrap_or_else(|| {
                        vec![fx::smoke::SpriteFrame {
                            data: vec![255, 255, 255, 255],
                            width: 1,
                            height: 1,
                        }]
                    });
                // The pass needs a representative `lifetime` for animation-frame
                // stride; `spec_intensity = 0.3` matches the legacy default.
                // Per-emitter binding is future work.
                renderer.register_smoke_collection(&collection, &frames, 0.3, c.lifetime);
                self.particle_render.register_sprite(&collection);
            }
        }

        self.renderer = Some(renderer);
        self.window_state = Some(WindowState { window });
        self.frame_timing.last_frame = Instant::now();

        log::info!("[Engine] Window ready");
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.window_state = None;
        self.renderer = None;
        // Fog-volume entities live in the script registry; clearing the
        // bridge's id table here keeps it from referencing stale slots if a
        // future surface re-creation re-runs `populate_from_level`.
        self.fog_volume_bridge.clear();
        log::info!("[Engine] Suspended");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
                self.camera.update_aspect(size.width, size.height);
            }
            WindowEvent::CloseRequested => {
                if let Some(ws) = self.window_state.as_ref() {
                    input::cursor::release_cursor(&ws.window);
                }
                log::info!("[Engine] Shutting down");
                event_loop.exit();
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        ..
                    },
                ..
            } => {
                if let Some(ws) = self.window_state.as_ref() {
                    input::cursor::release_cursor(&ws.window);
                }
                log::info!("[Engine] Shutting down");
                event_loop.exit();
            }
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                if let PhysicalKey::Code(code) = key_event.physical_key {
                    let pressed = key_event.state.is_pressed();

                    // Chord resolver runs first: owns Alt+Shift+ modifier
                    // tracking and fires only on a clean rising edge.
                    if let Some(action) =
                        self.diagnostic_inputs
                            .handle_key(code, pressed, key_event.repeat)
                    {
                        self.handle_diagnostic_action(action);
                    }

                    self.input_system.handle_keyboard_event(code, pressed);
                }
            }
            WindowEvent::MouseInput { button, state, .. } => {
                self.input_system
                    .handle_mouse_button(button, state.is_pressed());
            }
            WindowEvent::Focused(focused) => {
                if let Some(ws) = self.window_state.as_ref() {
                    input::cursor::handle_focus_change(focused, &ws.window);
                    if !focused {
                        self.input_system.clear_all();
                        self.diagnostic_inputs.clear_modifiers();
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                // Fixed-timestep loop: accumulate wall-clock time, tick at
                // constant rate, interpolate for rendering.
                // See: context/lib/rendering_pipeline.md §1
                let now = Instant::now();
                let frame_result = self.frame_timing.begin_frame(now);
                let tick_dt = self.frame_timing.tick_dt();
                let frame_dt = frame_result.frame_dt;
                let ticks = frame_result.ticks;

                // Hot reload (debug builds only). `level_load_fired` is NOT
                // reset — it gates first-frame init, not subsequent reloads.
                // See: context/lib/scripting.md §8
                match self.script_runtime.drain_reload_requests() {
                    Ok(true) => {
                        self.script_runtime.clear_level_handlers();
                        if let Err(e) = self.script_runtime.reload_behavior_context() {
                            log::error!(
                                "[Scripting] hot reload: failed to rebuild behavior context: {e}",
                            );
                        } else {
                            // Data script runs exactly once per level load (cold path
                            // below), never here. The data registry and progress tracker
                            // carry forward so in-flight subscriptions survive edits.
                            // See: context/lib/scripting.md §2, §8
                            load_behavior_scripts(&self.script_runtime, &self.content_root);
                            if self.level_load_fired {
                                self.script_runtime.fire_level_load();
                            }
                            log::info!(
                                "[Scripting] hot reload finished (check earlier output for per-file errors)"
                            );
                        }
                    }
                    Ok(false) => {}
                    Err(err) => {
                        log::error!("[Scripting] drain_reload_requests failed: {err}");
                    }
                }

                // Fire `levelLoad` once, before the first frame renders. The
                // world is already populated (load_level ran before the event loop).
                // See: context/lib/scripting.md
                if !self.level_load_fired {
                    // Data script fires before behavior handlers register. Errors
                    // surface as an empty manifest so the level still loads.
                    // See: context/lib/scripting.md §2
                    if let Some(world) = &self.level {
                        if let Some(data_script) = &world.data_script {
                            let mut manifest = self.script_runtime.run_data_script(data_script);
                            manifest.reactions = validate_sequence_primitives(
                                manifest.reactions,
                                &self.sequence_registry,
                            );
                            self.script_ctx
                                .data_registry
                                .borrow_mut()
                                .populate_from_manifest(manifest);
                            // Independent of the behavior HandlerTable — a
                            // hot-reload leaves these subscriptions intact.
                            self.progress_tracker.initialize(
                                &self.script_ctx.data_registry.borrow(),
                                &self.script_ctx.registry.borrow(),
                            );
                        }
                    }

                    // Data-archetype sweep: now that `registerEntity` has
                    // populated `data_registry.entities`, materialize every
                    // matching map placement that the built-in dispatch did
                    // not already handle.
                    // See: context/lib/scripting.md §2 · context/lib/build_pipeline.md §Built-in Classname Routing
                    if let Some(world) = self.level.as_ref() {
                        let handled = self.builtin_handled.take().unwrap_or_default();
                        let descriptors = self.script_ctx.data_registry.borrow().entities.clone();
                        let mut registry = self.script_ctx.registry.borrow_mut();
                        // Adapt wire records to the scripting representation
                        // at the dispatch boundary (same pattern as the
                        // built-in sweep above).
                        let map_entities: Vec<crate::scripting::map_entity::MapEntity> =
                            world.map_entities.iter().cloned().map(Into::into).collect();
                        let descriptor_handled = apply_data_archetype_dispatch(
                            &map_entities,
                            &descriptors,
                            &handled,
                            &mut registry,
                        );
                        if !descriptor_handled.is_empty() {
                            log::info!(
                                "[Loader] dispatched {} map entities through descriptor archetypes",
                                descriptor_handled.len(),
                            );
                        }

                        // Pick up any descriptor-spawned `LightComponent`s so
                        // they participate in the per-frame light bridge pack.
                        // `populate_from_level` ran in `resumed()` against the
                        // FGD-sourced `MapLight` list; descriptor-spawn happens
                        // here, after that, so the bridge needs a second pass
                        // to enroll the new dynamic lights.
                        self.light_bridge.absorb_dynamic_lights(&registry);
                    }

                    // Descriptor-spawned emitters may carry sprite collections
                    // not seen during the resumed()-time sweep. Re-register
                    // any new collections so the renderer pass has them ready
                    // before the first frame draws.
                    if let Some(renderer) = self.renderer.as_mut() {
                        use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;
                        use crate::scripting::registry::{ComponentKind, ComponentValue};
                        let texture_root = self.content_root.join("textures");
                        let registry = self.script_ctx.registry.borrow();
                        let mut seen: std::collections::HashSet<String> =
                            std::collections::HashSet::new();
                        for (_id, value) in registry.iter_with_kind(ComponentKind::BillboardEmitter)
                        {
                            let ComponentValue::BillboardEmitter(c) = value else {
                                continue;
                            };
                            let _: &BillboardEmitterComponent = c;
                            let collection = c.sprite.clone();
                            if collection.is_empty() || !seen.insert(collection.clone()) {
                                continue;
                            }
                            let frames =
                                fx::smoke::load_collection_frames(&texture_root, &collection)
                                    .unwrap_or_else(|| {
                                        vec![fx::smoke::SpriteFrame {
                                            data: vec![255, 255, 255, 255],
                                            width: 1,
                                            height: 1,
                                        }]
                                    });
                            renderer.register_smoke_collection(
                                &collection,
                                &frames,
                                0.3,
                                c.lifetime,
                            );
                            self.particle_render.register_sprite(&collection);
                        }
                    }

                    load_behavior_scripts(&self.script_runtime, &self.content_root);
                    self.script_runtime.fire_level_load();
                    fire_named_event_with_sequences(
                        "levelLoad",
                        &self.script_ctx.data_registry.borrow(),
                        &self.sequence_registry,
                        &self.reaction_registry,
                        &self.script_ctx,
                    );
                    self.level_load_fired = true;
                    self.script_time = 0.0;
                }

                if let Some(gp) = &mut self.gamepad_system {
                    gp.update(&mut self.input_system);
                }

                // drain_look_inputs() must precede snapshot(); both touch
                // mouse_axes and look state belongs to the render-rate path.
                let look = self.input_system.drain_look_inputs();
                let snapshot = self.input_system.snapshot();

                // Apply look rotation once at render rate, not once per tick —
                // so zero-tick frames still consume accumulated mouse motion.
                self.camera
                    .rotate(look.yaw_delta(frame_dt), look.pitch_delta(frame_dt));

                // Bump the engine frame counter once per Game logic phase,
                // before any tick handlers fire. `emitEvent` reads this to
                // stamp `GameEvent.frame` so each `game_events` log line
                // carries an ordering key. See: context/lib/scripting.md
                self.script_ctx
                    .frame
                    .set(self.script_ctx.frame.get().wrapping_add(1));

                for _ in 0..ticks {
                    let forward_axis = snapshot.axis_value(Action::MoveForward);
                    let right_axis = snapshot.axis_value(Action::MoveRight);
                    let up_axis = snapshot.axis_value(Action::MoveUp);
                    let sprint = snapshot.button(Action::Sprint).is_active();

                    let speed = if sprint {
                        camera::MOVE_SPEED * camera::SPRINT_MULTIPLIER
                    } else {
                        camera::MOVE_SPEED
                    };

                    let forward = self.camera.forward();
                    let right = self.camera.right();
                    let mut move_dir =
                        forward * forward_axis + right * right_axis + Vec3::Y * up_axis;

                    // Normalize to prevent faster diagonal movement, but only
                    // if there's actual movement input.
                    if move_dir.length_squared() > 0.0 {
                        move_dir = move_dir.normalize();
                    }

                    self.camera.position += move_dir * speed * tick_dt;

                    self.frame_timing
                        .push_state(InterpolableState::new(self.camera.position));
                }

                // Fire `tick` after game logic, before render. `delta` comes
                // from the engine frame timer (not wall clock); `time` is
                // seconds since level load, monotonic within a level.
                self.script_time += frame_dt;
                self.script_runtime.fire_tick(ScriptCallContext {
                    delta: frame_dt,
                    time: self.script_time,
                });

                // Drain the `game_events` ring buffer that `emitEvent` writes
                // to. Each entry surfaces as a single `log::info!` on the
                // `game_events` target so authors can observe emissions with
                // `RUST_LOG=game_events=info`. `payload` is rendered with
                // `Display` so the line is canonical JSON, not Rust Debug.
                {
                    let mut buf = self.script_ctx.game_events.borrow_mut();
                    while let Some(ev) = buf.pop_front() {
                        log::info!(
                            target: "game_events",
                            "kind={} frame={} payload={}",
                            ev.kind,
                            ev.frame,
                            ev.payload,
                        );
                    }
                }

                // Position interpolated from tick-state slots; yaw/pitch from
                // `self.camera` directly so zero-tick frames still see this
                // frame's look rotation.
                let interp = self.frame_timing.interpolated_state();
                let view_proj = interp.view_projection(
                    self.camera.aspect(),
                    self.camera.yaw,
                    self.camera.pitch,
                );

                let capture_portal_walk = std::mem::take(&mut self.capture_portal_walk_next_frame);

                // Portal DFS → cell IDs → visible-cell bitmask → indirect draw buffer.
                let (visible_cells, stats, _frustum) = match self.level.as_ref() {
                    Some(world) => visibility::determine_visible_cells(
                        interp.position,
                        view_proj,
                        world,
                        capture_portal_walk,
                        &mut self.scratch_cells,
                    ),
                    None => (
                        VisibleCells::DrawAll,
                        VisibilityStats {
                            camera_leaf: 0,
                            total_faces: 0,
                            drawn_faces: 0,
                            path: VisibilityPath::EmptyWorldFallback,
                        },
                        visibility::extract_frustum_planes(view_proj),
                    ),
                };

                // Empty slice = DrawAll sentinel: `update_dynamic_light_slots`
                // keeps every leaf-assigned light eligible on that path.
                let visible_leaf_mask: Vec<bool> = match (&visible_cells, self.level.as_ref()) {
                    (VisibleCells::DrawAll, _) | (_, None) => Vec::new(),
                    (VisibleCells::Culled(cell_ids), Some(world)) => {
                        let mut mask = vec![false; world.leaves.len()];
                        for &id in cell_ids {
                            let i = id as usize;
                            if i < mask.len() {
                                mask[i] = true;
                            }
                        }
                        mask
                    }
                };

                if let Some(renderer) = self.renderer.as_mut() {
                    // Emitter bridge — after script `tick` handler, before particle
                    // sim. Spawns new particles; the sim advances them the same
                    // frame so they don't appear stuck at origin.
                    {
                        let mut registry = self.script_ctx.registry.borrow_mut();
                        self.emitter_bridge
                            .update(&mut registry, frame_dt, self.script_time);
                    }

                    // Particle sim — after emitter bridge, before light bridge.
                    // Pure Rust; scripts never observe individual particles.
                    {
                        let mut registry = self.script_ctx.registry.borrow_mut();
                        scripting_systems::particle_sim::tick(&mut registry, frame_dt);
                    }

                    // Light bridge — between Game Logic and Render. Uploads
                    // mutated `LightComponent` data before `render_frame_indirect`
                    // allocates slots, so scripted lights reflect their new state.
                    {
                        let mut registry = self.script_ctx.registry.borrow_mut();
                        if let Some(update) =
                            self.light_bridge.update(&mut registry, self.script_time)
                        {
                            if update.has_dirty_data {
                                renderer.upload_bridge_lights(&update.lights_bytes);
                                renderer.upload_bridge_descriptors(&update.descriptor_bytes);
                                renderer.upload_bridge_samples(&update.samples_bytes);
                            }
                            renderer.set_light_effective_brightness(&update.effective_brightness);
                        }
                    }

                    // Fog volume bridge — alongside the light bridge. Volume
                    // packing reads `FogVolumeComponent`; point-light packing
                    // pre-culls dynamic point lights against fog AABBs. Upload
                    // happens unconditionally so an empty list zeroes the GPU
                    // volume count and skips the pass for the rest of the frame.
                    {
                        let registry = self.script_ctx.registry.borrow();
                        if let Some(bytes) = self.fog_volume_bridge.update_volumes(&registry) {
                            renderer.upload_fog_volumes(bytes);
                        } else {
                            renderer.upload_fog_volumes(&[]);
                        }
                    }
                    let level_lights = renderer.level_lights().to_vec();
                    let point_bytes = self.fog_volume_bridge.update_points(&level_lights);
                    renderer.upload_fog_points(point_bytes);

                    renderer.update_per_frame_uniforms(view_proj, interp.position);

                    if renderer.is_ready() {
                        // Particle render — packs `SpriteInstance` bytes per
                        // collection; the collector never touches wgpu directly.
                        {
                            let registry = self.script_ctx.registry.borrow();
                            self.particle_render.collect(&registry);
                        }
                        let particle_collections: Vec<(&str, &[u8])> =
                            self.particle_render.iter_collections().collect();
                        if let Err(err) = renderer.render_frame_indirect(
                            &visible_cells,
                            &visible_leaf_mask,
                            view_proj,
                            &particle_collections,
                        ) {
                            self.exit_result = Err(err);
                            event_loop.exit();
                        }
                    }
                }

                if let VisibleCells::Culled(mut cells) = visible_cells {
                    cells.clear();
                    self.scratch_cells = cells;
                }

                let pos = interp.position;
                let region_label = "leaf";
                let path_label = match stats.path {
                    VisibilityPath::PrlPortal { .. } => "prl-portal",
                    VisibilityPath::NoPortalsFallback => "no-portals",
                    VisibilityPath::EmptyWorldFallback => "empty",
                    VisibilityPath::SolidLeafFallback => "solid-leaf",
                    VisibilityPath::ExteriorCameraFallback => "exterior",
                };
                let walk_reach_col = match stats.walk_reach() {
                    Some(walk) => format!(" walk:{walk}"),
                    None => String::new(),
                };
                log::debug!(
                    "[Diagnostics] {region_label}:{} path:{path_label} | draw:{} all:{}{walk_reach_col} | pos: ({:.0}, {:.0}, {:.0})",
                    stats.camera_leaf,
                    stats.drawn_faces,
                    stats.total_faces,
                    pos.x,
                    pos.y,
                    pos.z,
                );

                // `vsync:` label always present (not toggled) so it's grep-able
                // and the diagnostic toggle's effect is immediately visible.
                let vsync_label = self
                    .renderer
                    .as_ref()
                    .map(|r| if r.vsync_enabled() { "on" } else { "off" });
                if let Some(ws) = self.window_state.as_ref() {
                    if self.last_title_update.elapsed() >= Duration::from_millis(250) {
                        self.last_title_update = Instant::now();
                        self.title_buffer.clear();
                        let _ = write!(
                            &mut self.title_buffer,
                            "Postretro | {region_label}:{} path:{path_label} | draw:{} all:{}{walk_reach_col} | pos: ({:.0}, {:.0}, {:.0})",
                            stats.camera_leaf,
                            stats.drawn_faces,
                            stats.total_faces,
                            pos.x,
                            pos.y,
                            pos.z,
                        );
                        if let Some(label) = vsync_label {
                            let _ = write!(&mut self.title_buffer, " | vsync:{label}");
                        }
                        if let Some(ft) = self.frame_rate_meter.stats() {
                            let _ = write!(
                                &mut self.title_buffer,
                                " frame: {:.1}/{:.1}/{:.1} ms",
                                ft.min_ms, ft.avg_ms, ft.max_ms,
                            );
                        }
                        ws.window.set_title(&self.title_buffer);
                    }
                }

                // Measure from `now` at handler entry so the sample spans all
                // CPU work. Wall-clock tick-to-tick is useless under vsync
                // (pinned to ~16.6ms); this shows actual load.
                let frame_cpu = Instant::now().duration_since(now);
                self.frame_rate_meter.record(frame_cpu);
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.input_system.handle_mouse_delta(delta.0, delta.1);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(ws) = self.window_state.as_ref() {
            ws.window.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.renderer = None;
        self.window_state = None;
        log::info!("[Engine] Exited");
    }
}

impl App {
    fn handle_diagnostic_action(&mut self, action: DiagnosticAction) {
        match action {
            DiagnosticAction::ToggleWireframe => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.toggle_wireframe();
                }
            }
            DiagnosticAction::DumpPortalWalk => {
                self.capture_portal_walk_next_frame = true;
                log::info!(
                    target: "postretro::portal_trace",
                    "[portal_trace] capture armed for next frame",
                );
            }
            DiagnosticAction::ToggleVsync => {
                if let Some(renderer) = self.renderer.as_mut() {
                    let enabled = renderer.toggle_vsync();
                    // Stale frametime samples would keep the title pinned
                    // to pre-toggle numbers for up to two seconds — exactly
                    // when the user is staring at it to see what changed.
                    self.frame_rate_meter.clear();
                    log::info!("[Renderer] vsync {}", if enabled { "on" } else { "off" },);
                }
            }
            DiagnosticAction::LowerAmbientFloor => {
                if let Some(renderer) = self.renderer.as_mut() {
                    let next = renderer.ambient_floor() - input::AMBIENT_FLOOR_STEP;
                    renderer.set_ambient_floor(next);
                    log::info!("[Renderer] ambient floor: {:.5}", renderer.ambient_floor());
                }
            }
            DiagnosticAction::RaiseAmbientFloor => {
                if let Some(renderer) = self.renderer.as_mut() {
                    let next = renderer.ambient_floor() + input::AMBIENT_FLOOR_STEP;
                    renderer.set_ambient_floor(next);
                    log::info!("[Renderer] ambient floor: {:.5}", renderer.ambient_floor());
                }
            }
            DiagnosticAction::LowerIndirectScale => {
                if let Some(renderer) = self.renderer.as_mut() {
                    let next = renderer.indirect_scale() - input::INDIRECT_SCALE_STEP;
                    renderer.set_indirect_scale(next);
                    log::info!(
                        "[Renderer] indirect scale: {:.2}",
                        renderer.indirect_scale()
                    );
                }
            }
            DiagnosticAction::RaiseIndirectScale => {
                if let Some(renderer) = self.renderer.as_mut() {
                    let next = renderer.indirect_scale() + input::INDIRECT_SCALE_STEP;
                    renderer.set_indirect_scale(next);
                    log::info!(
                        "[Renderer] indirect scale: {:.2}",
                        renderer.indirect_scale()
                    );
                }
            }
            DiagnosticAction::CycleLightingIsolation => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.cycle_lighting_isolation();
                }
            }
        }
    }
}

// --- Tests ---
//
// Pins for the render-rate look / tick-rate sim split:
//
// - On a frame with `ticks == 0`, mouse delta accumulated this frame must
//   still rotate the camera *and* change the rendered view-projection
//   matrix. Yaw/pitch reach the matrix as arguments to `view_projection`,
//   not as fields of `InterpolableState`, so a yaw assertion alone does
//   not cover the rendering path — a matrix assertion is required.
// - On a multi-tick frame, look rotation applies once at render rate,
//   not once per tick.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_timing::TICK_DURATION;
    use crate::input::{InputSystem, default_bindings};

    /// Epsilon for angle and matrix-element comparisons. Mouse-driven yaw
    /// deltas at default sensitivity land around 1e-1 radians, so 1e-5 is
    /// comfortably tight without being flaky on f32 round-off.
    const EPSILON: f32 = 1e-5;

    /// On a frame with zero ticks, accumulated mouse delta must rotate the
    /// camera *and* change the rendered view-projection matrix. Both checks
    /// are required: `view_projection` takes yaw/pitch as arguments, so an
    /// updated `camera.yaw` alone does not prove rendering sees it.
    #[test]
    fn mouse_delta_applied_on_zero_tick_frame() {
        let mut sys = InputSystem::new(default_bindings());
        let mut camera = Camera::new(Vec3::ZERO, 0.0, 0.0);

        // Accumulate a large horizontal mouse delta. At default sensitivity
        // (0.002 rad/unit) and scale -1.0 this produces yaw_displacement
        // of -0.2 radians — well above EPSILON.
        sys.handle_mouse_delta(100.0, 0.0);
        let look = sys.drain_look_inputs();

        // A 5ms elapsed frame is well below the 16.667ms tick duration, so
        // the accumulator produces zero ticks but still reports a positive
        // frame_dt — the frame shape the look path must handle.
        let initial_state = InterpolableState::new(Vec3::ZERO);
        let mut timing = FrameTiming::new(initial_state);
        let result = timing.accumulate(Duration::from_millis(5));
        assert_eq!(result.ticks, 0, "5ms elapsed must not produce a tick");
        assert!(
            result.frame_dt > 0.0,
            "frame_dt must be positive on a non-zero elapsed frame",
        );

        // Mirror production: rotate once per render frame, before the (here
        // absent) tick loop.
        camera.rotate(
            look.yaw_delta(result.frame_dt),
            look.pitch_delta(result.frame_dt),
        );

        // Camera yaw must reflect the mouse motion.
        assert!(
            camera.yaw.abs() > EPSILON,
            "camera.yaw should have changed from 0.0, got {}",
            camera.yaw,
        );

        // View-projection assertion — the load-bearing check. Build the
        // baseline matrix with yaw/pitch = 0 and the post-rotation matrix
        // with the camera's actual yaw/pitch. Position is identical in both
        // cases, so any element-wise difference must come from the rotation.
        let render_state = InterpolableState::new(Vec3::ZERO);
        let aspect = camera.aspect();
        let baseline = render_state.view_projection(aspect, 0.0, 0.0);
        let rotated = render_state.view_projection(aspect, camera.yaw, camera.pitch);

        let baseline_cols = baseline.to_cols_array();
        let rotated_cols = rotated.to_cols_array();
        let any_differs = baseline_cols
            .iter()
            .zip(rotated_cols.iter())
            .any(|(a, b)| (a - b).abs() > EPSILON);
        assert!(
            any_differs,
            "view_projection must differ after applying mouse-driven yaw; \
             baseline={:?} rotated={:?}",
            baseline_cols, rotated_cols,
        );
    }

    #[test]
    fn content_root_from_map_returns_grandparent_for_standard_path() {
        assert_eq!(
            content_root_from_map("content/tests/maps/test-3.prl"),
            PathBuf::from("content/tests"),
        );
    }

    #[test]
    fn content_root_from_map_returns_grandparent_for_mod_path() {
        assert_eq!(
            content_root_from_map("content/base/maps/e1m1.prl"),
            PathBuf::from("content/base"),
        );
    }

    // Regression: `Path::new("maps/test.prl").parent().and_then(parent)` returns
    // `Some("")` (an empty path), not `None`, so the prior `unwrap_or` fallback
    // was bypassed and the function returned `""` instead of `"."`.
    #[test]
    fn content_root_from_map_returns_dot_for_single_segment_parent() {
        assert_eq!(content_root_from_map("maps/test.prl"), PathBuf::from("."));
    }

    #[test]
    fn content_root_from_map_returns_dot_for_bare_filename() {
        assert_eq!(content_root_from_map("test.prl"), PathBuf::from("."));
    }

    /// Regression: on a multi-tick frame, look rotation must be applied
    /// exactly once (at render rate), not once per tick. Applying it in the
    /// tick loop would multiply the delta by `ticks` and send the view
    /// spinning.
    #[test]
    fn mouse_delta_not_multiplied_on_multi_tick_frame() {
        let mut sys = InputSystem::new(default_bindings());
        let mut camera = Camera::new(Vec3::ZERO, 0.0, 0.0);

        sys.handle_mouse_delta(100.0, 0.0);
        let look = sys.drain_look_inputs();

        // Force exactly 3 ticks by advancing the accumulator by 3 * TICK_DURATION.
        let initial_state = InterpolableState::new(Vec3::ZERO);
        let mut timing = FrameTiming::new(initial_state);
        let result = timing.accumulate(TICK_DURATION * 3);
        assert_eq!(result.ticks, 3, "TICK_DURATION * 3 must produce 3 ticks");

        // Production code rotates once before the tick loop and never inside
        // it. Mirror that: one rotation call, regardless of tick count.
        camera.rotate(
            look.yaw_delta(result.frame_dt),
            look.pitch_delta(result.frame_dt),
        );

        // The expected yaw is the single-application delta. Compute it the
        // same way the production code does on a fresh system to avoid
        // analytic drift from the binding table.
        let mut reference_sys = InputSystem::new(default_bindings());
        reference_sys.handle_mouse_delta(100.0, 0.0);
        let reference_look = reference_sys.drain_look_inputs();
        let expected_yaw = reference_look.yaw_delta(result.frame_dt);

        assert!(
            (camera.yaw - expected_yaw).abs() < EPSILON,
            "camera.yaw should equal single-application delta {} (not 3x), got {}",
            expected_yaw,
            camera.yaw,
        );
    }
}
