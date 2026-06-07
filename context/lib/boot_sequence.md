# Boot Sequence

> **Read this when:** wiring startup, splash, level-load, or shutdown code; or reasoning about where new init/teardown belongs.
> **Key invariant:** the engine owns the schedule; mod code runs only inside phases the engine grants. The boot path is single-window, single-level-per-process — there is no runtime level-to-level transition.
> **Related:** [Architecture Index](./index.md) · [Scripting](./scripting.md) · [Entity Model](./entity_model.md) · [Build Pipeline](./build_pipeline.md)

Boot code lives in `crates/postretro/src/main.rs` and `crates/postretro/src/startup/`.

---

## 1. Boot Order

Boot runs in two stages: a setup pass that builds session-lifetime state, then a winit-driven state machine (Booting → Splash → Running) that opens the window and loads the level. Mod init and level load are deliberately deferred out of the setup pass so the first splash frame paints before any user-authored work runs.

| # | Stage | Notes |
|---|-------|-------|
| 1 | Init logging and boot clock, parse args, resolve map path + content root | |
| 2 | Build the script runtime: `ScriptCtx`, primitive registry (`register_all`), `ScriptRuntime` | Constructed ONCE, before the window. Primitive closures capture `ScriptCtx` clones. Held for the whole session, never recreated. |
| 3 | Build the event loop and Rust-side registries (sequenced/reaction primitives, classname dispatch) | These survive level unload — they describe engine types, not per-level state. |
| 4 | Construct the app in the Booting state | Mod init, hot-reload watcher, and worker spawn are NOT done here. |
| 5 | On resume: create window, init the renderer (wgpu), build audio, enter Splash, request first redraw | Fires once on desktop; guarded against re-entry. Adapter requirement checks live in renderer init and fail fast with a named message (see `rendering_pipeline.md` §4). Audio init is fault-tolerant — failure logs and runs silent. No mod or level work. |
| 6 | Redraw drives the splash state machine (below) | |
| 7 | Install the level (§3) | Runs ONCE per process, on the main thread, on worker delivery. |
| 8 | Steady per-frame loop: Input → Game logic → Audio → Render → Present | |

### Splash state machine

The splash advances one frame per redraw, deferring all CPU-heavy work behind visible pixels:

- **First frame.** Paint a black frame so the OS window appears immediately (no splash texture bound yet). After present: decode and upload the splash image. No mod or level work.
- **Second frame.** Paint the splash so it is visible before any user-authored work. After paint: recompile stale scripts (debug-only; release no-ops), then run mod init. On success the engine drains the validated `setupMod()` return's entity-type descriptors into the engine-global `data_registry`. Start the hot-reload watcher (debug-only). Then spawn the level worker — **PRL parse only** (§2).
- **Remaining frames.** Non-blocking poll of the worker channel. Keep painting the splash while it runs. On a non-empty payload: install the level, clear the splash, enter Running. A delivered-but-empty payload or a worker error stays in Splash.

The two-frame delay is causal: pixels reach the user before any mod-supplied or level-load CPU work runs.

---

## 2. Worker vs. Main Thread

The level worker parses the PRL only. Texture decode, GPU upload, and UV normalization run on the **main thread** during level install — they need the renderer, which is not `Send`.

Textures are decoded from baked `.prm` mip sidecars, not from the PRL. The worker derives the texture cache root and ships it in the payload so the main thread can locate sidecars without re-deriving the layout. Missing or unusable sidecars degrade per-texture to placeholders with a warning — not a startup failure.

| Owner | Work |
|-------|------|
| Main thread | winit event loop, wgpu (device, queue, all GPU work), audio mixer, all script-VM execution, texture decode/upload, UV normalize, geometry upload. The script runtime and renderer are not `Send` — enforced by the types. |
| Worker thread | PRL parse only. Output is plain `Send` POD — no engine handles, no GPU resources. |

Handoff is an `mpsc` channel. One worker per load; no thread pool, no async runtime (`std::thread` + `mpsc`).

---

## 3. Level Install Order

Level install runs once, on the main thread, after worker delivery. Texture upload precedes UV normalize: `.prm` slot dimensions drive UV normalization, so the renderer must produce loaded textures before texel-space UVs convert to `[0,1]`.

| Order | Stage |
|-------|-------|
| 1 | Seed world gravity from the level's authored value (before scripts run) |
| 2 | Texture upload from `.prm` sidecars |
| 3 | UV normalize using uploaded texture dimensions |
| 4 | Geometry upload (vertex/index buffers) |
| 5 | World mesh spawn (see seam note below) |
| 6 | Light bridge: one light entity per map-authored light |
| 7 | Fog bridge: fog-volume entities + renderer pixel-scale / cell masks |
| 8 | Collision world populated from static geometry (separate from BSP) |
| 9 | Level sound loading from `sounds/` (fault-tolerant; silent if audio init failed) |
| 10 | Built-in classname dispatch (player spawns partitioned out; remainder dispatched, handled set stashed) |
| 11 | Sprite-collection registration for map-spawned emitters |
| 12 | Data script run → per-level reactions into `data_registry`; progress tracker init |
| 13 | Data-archetype sweep (match map placements against registered entity types not already handled), player spawn, camera teleport to first player spawn (or geometry center) |
| 14 | Second sprite pass for descriptor-spawned emitters |
| 15 | Fire the `levelLoad` named event |

Lights come from PRL data via the light bridge, not classname dispatch. Entity types are engine-global and arrive at mod-init via `setupMod` — `setupLevel`/the data script contributes only per-level reactions (see `scripting.md` §2).

**Mesh-spawn seam.** World mesh spawning is its own install stage, distinct from classname dispatch. The durable contract: map geometry becomes renderable mesh entities here, after geometry upload and before the light/fog bridges. (The current implementation hardwires a single world mesh; a classname-driven handler is planned — see §7.)

---

## 4. Shutdown

There is no in-engine level unload distinct from process exit: this engine runs one level per lifetime, so unload coincides with exit.

- A close request or Escape exits the event loop.
- The exit teardown hook releases level sounds (mirrors texture release on unload), then drops renderer and window.
- There is NO `Drop` impl on the app and NO save-on-exit precedent. The app holds `script_ctx`/`data_registry` until the event loop returns and the process ends.

Platform suspend is a separate path: it clears renderer/window/fog/collision and the in-flight worker, and resets state to Booting so resume rebuilds the surface. It does NOT clear `script_ctx`/`data_registry`.

---

## 5. Lifetimes

| Scope | Cleared on |
|-------|-----------|
| Engine init (preludes, primitive registry, `ScriptCtx`, `ScriptRuntime`, Rust-side registries) | Process exit only — built once at startup, never recreated. |
| `data_registry.entities` (entity-type descriptors from `setupMod` return) | Engine-global. Survives level unload; survives platform suspend. |
| Per-level reactions (`data_registry.reactions`) | Level unload. |
| Renderer, window, audio, collision world, fog bridge | Dropped on exit; cleared on suspend and rebuilt on resume. |

Level install runs exactly once per process — there is no runtime path to swap levels. Hot reload (debug only) recompiles changed script files and replaces engine-global entity types; definition-context changes still require a restart.

---

## 6. Non-Goals

- Per-entity script lifecycle callbacks (see `entity_model.md` §9)
- Runtime level-to-level transition / level swap mid-process
- Networked mod sync; runtime mod hot-swap mid-level
- Sandboxing mods from each other (mods share VM contexts and `data_registry` by design)
- Save-on-exit / persisted session state

---

## 7. Planned (not implemented)

None of the following exists in code. Do not treat any of it as current behavior; it is recorded only to anchor future work.

- **Mod discovery / `content/mods/`.** A scan of `content/base/` and `content/mods/*/` for manifests, a mod manifest format, and per-mod load-order resolution. Today there is a single content root derived from the map path; mod init runs against it directly.
- **Mod browser UI and main menu.** Engine-native mod selection (`--mods` flag, persisted selection) and a mod-contributed main menu / level selector. Today the map is a CLI argument and the engine boots straight into it.
- **Mod-supplied splash override.** The frame-two override hook exists but is always absent until the mod system ships.
- **Classname-driven world mesh spawn.** A classname handler replaces the hardwired single-mesh seam (§3 stage 5).
- **Domain folder convention.** Fixed `start-script` entry, `actors/`, `weapons/`, `levels/<name>/<name>.{ts,luau}` auto-discovery, and moving the data script out of the PRL. Today the data script is bundled in the PRL.
</content>
</invoke>
