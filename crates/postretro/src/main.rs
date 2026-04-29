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

// Per-frame systems that bridge the scripting surface to other engine
// subsystems. Intentionally rooted at the main-binary crate level (not under
// `scripting/`) so that `src/bin/gen_script_types.rs` — which re-uses the
// `scripting` module tree via `#[path]` without the engine's renderer/prl
// modules — does not pull in wgpu/engine-dependent code.
//
// See: context/lib/scripting.md
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
use crate::scripting::builtins::{ClassnameDispatch, register_builtins as register_builtin_classnames};
use crate::scripting::call_context::ScriptCallContext;
use crate::scripting::ctx::ScriptCtx;
use crate::scripting::data_registry::DataRegistry;
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

/// Derive the content root directory from a map file path. The content root
/// is the parent of the `maps/` directory containing the map; sibling
/// directories such as `textures/` and `scripts/` live alongside `maps/`
/// under this root.
///
/// For `content/tests/maps/test-3.prl`, returns `content/tests/`.
/// For `content/base/maps/e1m1.prl`, returns `content/base/`.
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
    let spawn_demo_smoke = args.iter().any(|a| a == "--demo-smoke");

    // Load textures for PRL levels.
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

    // Position camera inside the level geometry.
    let initial_camera_pos = match &level {
        Some(world) => world.spawn_position(),
        None => Vec3::new(0.0, 200.0, 500.0),
    };

    let event_loop = EventLoop::new().context("failed to create event loop")?;

    let initial_state = InterpolableState::new(initial_camera_pos);

    // --- Scripting bootstrap.
    //
    // One `ScriptRuntime` per engine instance. Behavior scripts load from
    // `<content_root>/scripts/` (if present) sorted lexicographically by
    // UTF-8 byte order — this fixes `registerHandler` invocation order
    // across files.
    // `fire_level_load` runs after world population but before the first
    // frame renders; `fire_tick` runs each frame after game logic. See
    // context/lib/scripting.md.
    let script_ctx = ScriptCtx::new();
    let mut script_registry = PrimitiveRegistry::new();
    register_all(&mut script_registry, script_ctx.clone());
    let mut script_runtime = ScriptRuntime::new(
        &script_registry,
        &ScriptRuntimeConfig::default(),
        &script_ctx,
    )
    .context("failed to construct script runtime")?;

    // Sequenced-primitive table: Rust-only handlers consulted by
    // `fire_named_event_with_sequences` when a `Sequence` reaction step fires.
    // Distinct from the script-facing primitive registry — these handlers run
    // on the dispatch path, not from inside QuickJS/Luau. Populated once at
    // startup; survives level reloads and behavior hot-reloads.
    let mut sequence_registry = SequencedPrimitiveRegistry::new();
    register_sequenced_light_primitives(&mut sequence_registry, script_ctx.clone());

    // Tag-targeted reaction-primitive table: handlers invoked by `Primitive`
    // reactions whose `primitive` field matches a registered name. Populated
    // once at startup; survives level reloads. See:
    // context/plans/in-progress/scripting-foundation/plan-3-emitter-entity.md §Sub-plan 5
    let mut reaction_registry = ReactionPrimitiveRegistry::new();
    register_emitter_reaction_primitives(&mut reaction_registry);

    // Built-in FGD-classname dispatch table. Engine-init-once: handlers
    // survive level unload because they describe engine types, not per-level
    // state. Sub-plan 8 will wire the level loader to consult this table.
    // See: context/plans/in-progress/scripting-foundation/plan-3-emitter-entity.md §Sub-plan 6
    let mut classname_dispatch = ClassnameDispatch::new();
    register_builtin_classnames(&mut classname_dispatch);

    // Start the dev-mode hot-reload watcher rooted at the same `scripts/`
    // directory `load_behavior_scripts` reads from. No-op in release builds.
    // Failure is logged and swallowed — a missing or unwatchable directory
    // must not prevent engine startup.
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
        smoke_emitters: build_demo_emitters(spawn_demo_smoke, initial_camera_pos),
        script_runtime,
        script_ctx,
        data_registry: DataRegistry::new(),
        sequence_registry,
        reaction_registry,
        progress_tracker: ProgressTracker::new(),
        classname_dispatch,
        light_bridge: scripting_systems::light_bridge::LightBridge::new(),
        emitter_bridge: scripting_systems::emitter_bridge::EmitterBridge::new(),
        particle_render: scripting_systems::particle_render::ParticleRenderCollector::new(),
        level_load_fired: false,
        script_time: 0.0,
    };

    event_loop
        .run_app(&mut app)
        .context("event loop terminated with error")?;

    app.exit_result
}

/// Normalize PRL texel-space UVs by dividing by texture dimensions. Called
/// after `load_textures()` provides actual dimensions. BVH leaves own the
/// index ranges now, so we walk the leaf array and use each leaf's
/// `material_bucket_id` — which is the texture index for this face — to
/// pick the correct texture's (w, h).
///
/// Invariant: the compiler emits a fresh copy of every face's vertices
/// (`extract_geometry` appends to `vertices` at the start of each face's
/// emit loop), so no vertex is shared between any two leaf `index_offset`
/// ranges at all. That makes the one-pass `normalized[vi]` guard a pure
/// defensive check — it would only trip on future pipeline changes that
/// begin deduplicating vertices across faces, at which point sharing
/// between different textures would become possible and this function
/// would need revisiting.
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

/// Build the initial smoke-emitter list. When `spawn_demo` is true (enabled
/// via the `--demo-smoke` CLI flag), a single emitter is placed a few units
/// in front of the camera spawn so the billboard pipeline has something to
/// render without requiring a map with `env_smoke_emitter` entities yet.
///
/// Once the PRL entity-flow wire format carries `env_smoke_emitter`
/// instances, this helper also folds in the level-derived emitters. Until
/// then the `--demo-smoke` path is the only way to exercise the pass end
/// to end.
fn build_demo_emitters(spawn_demo: bool, camera_pos: Vec3) -> Vec<fx::smoke::SmokeEmitter> {
    if !spawn_demo {
        return Vec::new();
    }
    let origin = camera_pos + Vec3::new(0.0, 0.0, -3.0);
    vec![fx::smoke::SmokeEmitter::new(
        fx::smoke::SmokeEmitterParams {
            origin,
            rate: 4.0,
            lifetime: 3.0,
            size: 0.5,
            speed: 0.3,
            collection: "smoke".to_string(),
            spec_intensity: 0.3,
        },
    )]
}

/// Load every behavior script under `<content_root>/scripts/` in
/// lexicographic order. The sort order defines cross-file `registerHandler`
/// invocation order per context/lib/scripting.md §8. Missing directory: no-op. Per-file
/// failures are logged and swallowed — one bad script must not kill the
/// engine.
///
/// # TypeScript compilation
///
/// `.ts` files are compiled to `.js` before loading (debug builds only, where
/// the watcher module is available). The compiled `.js` artifact lands next to
/// the source; any bare `.js` that already has a same-stem `.ts` sibling is
/// treated as a compiler artifact and skipped to avoid running the script twice.
///
/// If `scripts-build` is not found at startup, `.ts` files are passed directly
/// to QuickJS, which will fail with "Unexpected token" errors for any
/// TS-specific syntax. A warning is logged in that case.
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
    /// Loaded level data (PRL), held for the lifetime of the app.
    level: Option<prl::LevelWorld>,
    /// CPU-side textures loaded from disk, consumed by renderer during init.
    texture_set: Option<TextureSet>,

    /// Content root for the active level — derived once from the map path at
    /// startup. Sibling directories (`textures/`, `scripts/`) live under this
    /// root; both texture loading and behavior-script discovery read from it.
    content_root: PathBuf,

    exit_result: Result<()>,

    camera: Camera,
    input_system: input::InputSystem,
    gamepad_system: Option<input::gamepad::GamepadSystem>,
    frame_timing: FrameTiming,

    /// Diagnostic chord resolver. Parallel to `input_system`; consumes the
    /// same key events but produces engine debug actions, not gameplay
    /// actions. See: context/lib/input.md §7
    diagnostic_inputs: input::DiagnosticInputs,

    /// One-shot flag set by the `DumpPortalWalk` diagnostic chord. The next
    /// redraw consumes it, passes it into `determine_visible_cells`, and
    /// clears it. The visibility module emits per-portal trace lines under
    /// the `postretro::portal_trace` log target for that one frame only.
    capture_portal_walk_next_frame: bool,

    /// Persistent scratch buffer for per-frame visible cell ID collection.
    scratch_cells: Vec<u32>,

    /// Rolling ring buffer of per-frame CPU work durations. Sampled every
    /// frame, read at title-update cadence. Reports min/avg/max so hitches
    /// don't vanish into the average. See `frame_timing::FrameRateMeter`.
    frame_rate_meter: FrameRateMeter,

    /// Persistent string used to build the window title each update. Cleared
    /// and rewritten with `write!` to avoid the per-frame allocation a
    /// `format!` would do. Owns its capacity across frames.
    title_buffer: String,

    /// Last time the window title was written. Title updates are rate-limited
    /// to ~4Hz — at 60fps the title would otherwise flicker unreadably and
    /// the OS may throttle rapid `set_title` calls.
    last_title_update: Instant,

    /// Live smoke emitters, resolved at level load from `env_smoke_emitter`
    /// point entities. Updated each game-logic tick; the packed instance
    /// buffer is uploaded by the renderer in `render_frame_indirect`.
    ///
    /// See: context/lib/rendering_pipeline.md §7.4
    smoke_emitters: Vec<fx::smoke::SmokeEmitter>,

    /// Script runtime. Holds both QuickJS and Luau subsystems and the
    /// per-level handler table populated by `registerHandler`.
    /// See: context/lib/scripting.md
    script_runtime: ScriptRuntime,

    /// Shared scripting context. Holds the entity registry that the light
    /// bridge and the script runtime both share. Populated at level load;
    /// outlives the renderer so reloads / device resets preserve scripted
    /// light state.
    /// See: context/lib/scripting.md
    script_ctx: ScriptCtx,

    /// Reaction and entity-type registries populated from the level's
    /// `registerLevelManifest()` data script. Cleared on level unload but
    /// independent from the behavior `HandlerTable` — clearing one does not
    /// touch the other.
    /// See: context/lib/scripting.md §2 (Data context lifecycle)
    data_registry: DataRegistry,

    /// Rust-only handler table consulted by
    /// `fire_named_event_with_sequences` when a `Sequence` reaction step
    /// fires. Populated once at engine startup; does not need clearing on
    /// level unload because handlers carry no per-level state — they look up
    /// entities through `ScriptCtx`'s shared `EntityRegistry`, which the
    /// level-unload path clears separately.
    /// See: context/lib/scripting.md §4 (primitives), §5 (shared engine state)
    sequence_registry: SequencedPrimitiveRegistry,

    /// Tag-targeted reaction-primitive handlers (e.g. `setEmitterRate`,
    /// `setSpinRate`). Populated once at startup; resolved by name when a
    /// `Primitive` reaction fires.
    /// See: context/plans/in-progress/scripting-foundation/plan-3-emitter-entity.md §Sub-plan 5
    #[allow(dead_code)]
    reaction_registry: ReactionPrimitiveRegistry,

    /// Per-tag kill-count subscriptions derived from the data script's
    /// `progress` reactions. Initialized at level load from the data registry
    /// and the entity registry; cleared on level unload independently of the
    /// behavior `HandlerTable`.
    /// See: context/lib/scripting.md §2 (Data context lifecycle)
    progress_tracker: ProgressTracker,

    /// Built-in FGD-classname dispatch table: maps `classname` strings (e.g.
    /// `"billboard_emitter"`) to the engine handler that spawns the
    /// corresponding ECS entity from a map entity's KVPs. Built once at engine
    /// init; survives level unload — built-in handlers carry no per-level
    /// state. Sub-plan 8 will wire the level loader to consult this table.
    /// See: context/plans/in-progress/scripting-foundation/plan-3-emitter-entity.md §Sub-plan 6
    // Sub-plan 8 wires the level-loader sweep to consult this table; no read
    // site exists yet, so silence the lint until that lands.
    #[allow(dead_code)]
    classname_dispatch: ClassnameDispatch,

    /// Light bridge state: per-entity dirty tracking and play_count clocks.
    /// Runs once per frame between game logic and render; produces repacked
    /// `GpuLight` bytes which the renderer uploads via `upload_bridge_lights`.
    /// See: context/lib/scripting.md
    light_bridge: scripting_systems::light_bridge::LightBridge,

    /// Emitter bridge state: per-emitter accumulators, spin-tween elapsed
    /// time, and per-emitter LCG. Walks every `BillboardEmitterComponent`
    /// each game-logic tick after script `on_tick` and before particle sim.
    /// See: context/lib/scripting.md
    emitter_bridge: scripting_systems::emitter_bridge::EmitterBridge,

    /// Particle render collector: walks `ParticleState` entities once per
    /// frame in the Render stage, packs `SpriteInstance` bytes per sprite
    /// collection, and hands the byte slices to `SmokePass::record_draw`
    /// alongside the legacy `SmokeEmitter` path.
    /// See: context/plans/in-progress/scripting-foundation/plan-3-emitter-entity.md §Sub-plan 4
    particle_render: scripting_systems::particle_render::ParticleRenderCollector,

    /// Set once the `levelLoad` event has fired. Gates the first-tick
    /// invocation so `levelLoad` handlers are guaranteed to run before the
    /// first `tick` handler and before the first render frame.
    level_load_fired: bool,

    /// Seconds since level load. Fed into `ScriptCallContext::time` each
    /// tick; resets to zero on level unload. Accumulates from the engine
    /// frame timer, not a wall clock.
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

        // Derive per-texture material from texture names so the renderer can
        // populate per-material uniforms (shininess) without re-parsing.
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

        // Build geometry for the renderer.
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

        // Load and register smoke sprite sheets for every collection
        // referenced by this level's emitters. Collections missing frames
        // on disk register a single-frame checkerboard placeholder so the
        // pipeline path is exercised regardless.
        //
        // Entity-resolution note: `env_smoke_emitter` entities do not yet
        // flow through the PRL section wire format; the `smoke_emitters`
        // Vec is populated by developer hook or future PRL section. This
        // keeps the renderer + emitter update pipeline exercised while the
        // cross-crate plumbing is staged. See the task deliverable notes.
        let texture_root = self.content_root.join("textures");
        let mut registered: std::collections::HashSet<String> = std::collections::HashSet::new();
        for emitter in &self.smoke_emitters {
            let collection = emitter.collection().to_string();
            if collection.is_empty() || !registered.insert(collection.clone()) {
                continue;
            }
            let frames = fx::smoke::load_collection_frames(&texture_root, &collection)
                .unwrap_or_else(|| {
                    // Single-frame 1x1 white fallback — the pipeline stays
                    // wired even when the collection has no PNG frames yet.
                    vec![fx::smoke::SpriteFrame {
                        data: vec![255, 255, 255, 255],
                        width: 1,
                        height: 1,
                    }]
                });
            renderer.register_smoke_collection(
                &collection,
                &frames,
                emitter.params.spec_intensity,
                emitter.params.lifetime,
            );
        }

        let size = window.inner_size();
        self.camera.update_aspect(size.width, size.height);

        input::cursor::capture_cursor(&window);

        // Populate the scripting entity registry with one `LightComponent`
        // entity per map-authored light. Mirrors `LevelWorld.lights` one-to-one
        // and assigns stable `EntityId`s for the lifetime of the level; the
        // bridge's dirty tracker hangs its snapshots off those IDs.
        // See: context/lib/scripting.md
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

        self.renderer = Some(renderer);
        self.window_state = Some(WindowState { window });
        self.frame_timing.last_frame = Instant::now();

        log::info!("[Engine] Window ready");
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.window_state = None;
        self.renderer = None;
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

                    // Diagnostic chord resolver runs first. It owns modifier
                    // tracking for the Alt+Shift+ namespace and emits a
                    // diagnostic action only on a clean rising edge.
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

                // Hot reload (debug builds only). Drain pending watcher
                // requests; if any landed, rebuild the behavior surface from
                // disk and re-fire `levelLoad` so newly registered handlers
                // execute immediately. `level_load_fired` is intentionally NOT
                // reset — it gates the first-frame `levelLoad` fire, not
                // subsequent reloads.
                // See: context/lib/scripting.md §8
                match self.script_runtime.drain_reload_requests() {
                    Ok(true) => {
                        self.script_runtime.clear_level_handlers();
                        if let Err(e) = self.script_runtime.reload_behavior_context() {
                            log::error!(
                                "[Scripting] hot reload: failed to rebuild behavior context: {e}",
                            );
                        } else {
                            // INVARIANT: hot reload reruns behavior scripts ONLY. The data
                            // script (`registerLevelManifest`) is called exactly once per
                            // level load, in the cold-load branch below — never here. The
                            // data registry and progress tracker carry forward across
                            // behavior reloads so in-flight progress subscriptions and
                            // entity-type registrations survive script edits. Covered at
                            // the runtime level by `data_script_not_rerun_on_behavior_reload`
                            // (scripting/runtime.rs).
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
                // world is already populated (load_level ran before the event
                // loop started). Script files load from
                // `<content_root>/scripts/` sorted lexicographically — the
                // sort order pins cross-file `registerHandler` registration
                // order.
                // See: context/lib/scripting.md
                if !self.level_load_fired {
                    // Data context fires once per level load, before behavior
                    // handlers register. Errors are logged inside
                    // `run_data_script` and surface here as an empty manifest;
                    // the level loads with empty registries rather than
                    // failing. See: context/lib/scripting.md §2
                    if let Some(world) = &self.level {
                        if let Some(data_script) = &world.data_script {
                            let mut manifest = self.script_runtime.run_data_script(data_script);
                            // Drop sequence reactions that name an unknown primitive before storing.
                            manifest.reactions = validate_sequence_primitives(
                                manifest.reactions,
                                &self.sequence_registry,
                            );
                            self.data_registry.populate_from_manifest(manifest);
                            // Walk progress reactions and seed per-tag kill
                            // counters from the live entity set. Independent
                            // of the behavior HandlerTable — a behavior
                            // hot-reload (which clears handlers) leaves these
                            // subscriptions intact.
                            self.progress_tracker.initialize(
                                &self.data_registry,
                                &self.script_ctx.registry.borrow(),
                            );
                        }
                    }
                    load_behavior_scripts(&self.script_runtime, &self.content_root);
                    self.script_runtime.fire_level_load();
                    // Fire data-script sequence reactions for levelLoad after
                    // behavior handlers run. Entity registry is populated by
                    // this point (level geometry loaded before the event loop).
                    fire_named_event_with_sequences(
                        "levelLoad",
                        &self.data_registry,
                        &self.sequence_registry,
                        &self.script_ctx.registry.borrow(),
                    );
                    self.level_load_fired = true;
                    self.script_time = 0.0;
                }

                // Poll gamepad before taking the snapshot.
                if let Some(gp) = &mut self.gamepad_system {
                    gp.update(&mut self.input_system);
                }

                // drain_look_inputs() must precede snapshot(); both touch
                // mouse_axes and look state belongs to the render-rate path.
                let look = self.input_system.drain_look_inputs();
                let snapshot = self.input_system.snapshot();

                // Apply look rotation once per render frame, before the tick
                // loop. At render rate (not tick rate) mouse motion accumulated
                // on a zero-tick frame is still consumed this frame.
                self.camera
                    .rotate(look.yaw_delta(frame_dt), look.pitch_delta(frame_dt));

                // Run fixed-rate game logic ticks.
                for _ in 0..ticks {
                    // Movement from action snapshot.
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

                    // Push updated camera state for interpolation.
                    self.frame_timing
                        .push_state(InterpolableState::new(self.camera.position));

                    // Advance smoke emitters on the fixed-timestep tick.
                    // See: context/lib/rendering_pipeline.md §7.4
                    for emitter in &mut self.smoke_emitters {
                        emitter.tick(tick_dt);
                    }
                }

                // Fire `tick` after game logic, before render. `delta` comes
                // from the engine frame timer (not wall clock); `time` is
                // seconds since level load, monotonic within a level.
                self.script_time += frame_dt;
                self.script_runtime.fire_tick(ScriptCallContext {
                    delta: frame_dt,
                    time: self.script_time,
                });

                // Interpolate between previous and current state for rendering.
                // Position comes from the tick-state slots; yaw/pitch come from
                // `self.camera` directly so zero-tick frames still reflect this
                // frame's look input.
                let interp = self.frame_timing.interpolated_state();
                let view_proj = interp.view_projection(
                    self.camera.aspect(),
                    self.camera.yaw,
                    self.camera.pitch,
                );

                let capture_portal_walk = std::mem::take(&mut self.capture_portal_walk_next_frame);

                // GPU-driven path: portal DFS produces visible cell IDs; the
                // BVH traversal compute shader consumes them via the
                // visible-cell bitmask and writes the indirect draw buffer.
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

                // Build the per-leaf visibility bitmask the renderer needs to
                // cull dynamic lights against the visible cell set. Empty slice
                // is the DrawAll sentinel (`update_dynamic_light_slots` keeps
                // every leaf-assigned light eligible on that path).
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
                    // Emitter bridge — Game Logic stage, after script
                    // `on_tick` and before particle sim. Walks every
                    // `BillboardEmitterComponent`, handles burst/rate-based
                    // emission, advances any active spin animation, and
                    // spawns new particle entities into the registry. The
                    // sim that runs immediately afterward is what advances
                    // the just-spawned particles' first frame so they don't
                    // appear stuck at origin.
                    {
                        let mut registry = self.script_ctx.registry.borrow_mut();
                        self.emitter_bridge
                            .update(&mut registry, frame_dt, self.script_time);
                    }

                    // Particle simulation — Game Logic stage, after the
                    // emitter bridge (Plan 3 sub-plan 3) and before the
                    // light bridge / render. Integrates velocity, applies
                    // buoyancy/drag, advances curves, and despawns expired
                    // particles. Pure Rust; scripts never observe
                    // individual particles.
                    {
                        let mut registry = self.script_ctx.registry.borrow_mut();
                        scripting_systems::particle_sim::tick(&mut registry, frame_dt);
                    }

                    // Light bridge — between Game Logic and Render. Walks the
                    // scripting entity registry, detects mutated
                    // `LightComponent`s, handles `play_count` completion, and
                    // hands repacked GpuLight bytes to the renderer's upload
                    // seam. `update_dynamic_light_slots` runs later inside
                    // `render_frame_indirect` so scripted lights participate
                    // in slot allocation with their post-mutation state.
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

                    renderer.update_per_frame_uniforms(view_proj, interp.position);

                    if renderer.is_ready() {
                        let emitter_refs: Vec<&fx::smoke::SmokeEmitter> =
                            self.smoke_emitters.iter().collect();
                        // Particle render collector — Render stage. Walks the
                        // entity registry once and packs `SpriteInstance`
                        // bytes per sprite collection. The renderer consumes
                        // byte slices; the collector never touches wgpu.
                        // See: scripting-foundation plan-3 §Sub-plan 4.
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
                            &emitter_refs,
                            &particle_collections,
                        ) {
                            self.exit_result = Err(err);
                            event_loop.exit();
                        }
                    }
                }

                // Reclaim scratch buffer.
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

                // Window title update is rate-limited to ~4Hz; the sample
                // recording below happens every frame. A 60Hz title is
                // unreadable and the OS may throttle rapid `set_title`.
                //
                // The vsync state must always be visible in the title so
                // the diagnostic toggle's effect is self-evident. The
                // `vsync:on|off` segment sits adjacent to `frame:` because
                // they're read together, and in a fixed position so it's
                // grep-able (the label is always present, not only in
                // one state).
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

                // Record the CPU-side frame span. Measured from the `now`
                // captured at the top of the handler to this point — covers
                // input polling, fixed-timestep game logic, visibility
                // determination, title update, render, and scratch reclaim.
                // Wall-clock tick-to-tick is useless under vsync (pinned to
                // ~16.6ms regardless of work); this span shows actual load.
                // See: context/lib/rendering_pipeline.md §1 (frame ordering)
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
    /// Dispatch a diagnostic action emitted by the chord resolver.
    /// See: context/lib/input.md §7
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
