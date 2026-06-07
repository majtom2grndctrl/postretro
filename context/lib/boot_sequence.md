# Boot Sequence

> **Read this when:** wiring startup, splash, level-load, or shutdown code; or reasoning about where new init/teardown belongs.
> **Key invariant:** the engine owns the schedule; mod code runs only inside phases the engine grants. The boot path is single-window, single-level-per-process — there is no runtime level-to-level transition.
> **Related:** [Architecture Index](./index.md) · [Scripting](./scripting.md) · [Entity Model](./entity_model.md) · [Build Pipeline](./build_pipeline.md)

All `file:line` citations point at `crates/postretro/src/`. They are navigation aids, not contracts — trust the code over a stale line number.

---

## 1. Boot Order

Boot runs in two stages: a `main()` setup pass that builds session-lifetime state, then a winit-driven state machine (`BootState` Booting → Splash → Running) that opens the window and loads the level. Mod init and level load are deliberately deferred out of `main` so the first splash frame paints before any user-authored work runs (`main.rs:221-225`).

| # | Stage | Where | Notes |
|---|-------|-------|-------|
| 1 | env logger, boot clock, parse args, resolve map path + content root | `main.rs:162` | |
| 2 | Build `ScriptCtx`, `PrimitiveRegistry` + `register_all`, `ScriptRuntime` | `main.rs:187` | Constructed ONCE, before the window. Primitive closures capture `ScriptCtx` clones. Held by `App` the whole session, never recreated. |
| 3 | Build `EventLoop` and Rust-side registries (sequenced/reaction primitives, classname dispatch) | `main.rs:201-219` | These survive level unload — they describe engine types, not per-level state. |
| 4 | Construct `App` with `boot_state: Booting` | `main.rs:227` | Mod init, hot-reload watcher, and worker spawn are NOT done here. |
| 5 | `resumed()` — create window, `Renderer::new` (wgpu init), build audio, `boot_state = Splash`, request first redraw | `main.rs:497` | Fires once on desktop; guarded against re-entry. Adapter requirement checks live in `Renderer::new` and fail fast with a named message (see `rendering_pipeline.md` §4). Audio init is fault-tolerant — failure logs and runs silent (`main.rs:539`). No mod or level work. |
| 6 | `RedrawRequested` drives the splash state machine (below) | `main.rs:1452` | |
| 7 | `install_level_payload` — install the level (§3) | `main.rs:1707` | Runs ONCE per process, on the main thread, on worker delivery. |
| 8 | Steady per-frame loop: Input → Game logic → Audio → Render → Present | `main.rs:767-1389` | |

### Splash state machine

`run_splash_frame` (`main.rs:1452`) advances one frame per redraw:

- **Frame 0** (`main.rs:1454`). Paint a black frame so the OS window appears immediately (no splash texture bound yet). After present: decode the splash PNG synchronously and upload it. No mod or level work.
- **Frame 1** (`main.rs:1494`). Paint the splash so it is visible before any user-authored work. After paint: `compile_stale_scripts` (debug-only; release no-ops), then `run_mod_init` (`main.rs:1515`). On success the boot caller drains the validated `setupMod()` return's entity-type descriptors into the engine-global `data_registry` (`main.rs:1517-1527`). Start the hot-reload watcher (debug-only). Then spawn the level worker — **PRL parse only** (`startup/worker.rs:48-84`).
- **Frames 2+** (`main.rs:1577`). Non-blocking poll of the worker channel. Keep painting the splash while it runs. On `Ok(level=Some)`: call `install_level_payload`, clear the splash, set `boot_state = Running` (`main.rs:1614-1627`). A delivered-but-empty payload or a worker error stays in Splash.

The two-frame delay is causal: pixels reach the user before any mod-supplied or level-load CPU work runs.

---

## 2. Worker vs. Main Thread

The level worker parses the PRL only. Texture decode, GPU upload, and UV normalization run on the **main thread** in `install_level_payload` — they need the renderer, which is not `Send`.

Textures are decoded from baked `.prm` mip sidecars, not from the PRL. The worker derives the cache root (`<workspace>/.build-caches/prm-cache/`, `worker.rs:16-25,103`) and ships it in the payload so the main thread can locate `<hex(blake3)>.prm` sidecars without re-deriving the layout. Missing or unusable sidecars degrade per-texture to placeholders with a warning — not a startup failure.

| Owner | Work |
|-------|------|
| Main thread | winit event loop, wgpu (device, queue, all GPU work), audio mixer, all script-VM execution, texture decode/upload, UV normalize, geometry upload. `ScriptRuntime` and `Renderer` are not `Send` — enforced by the types. |
| Worker thread | PRL parse only. Output is plain `Send` POD (`LevelPayload`) — no engine handles, no GPU resources. |

Handoff is an `mpsc` channel. One worker per load; no thread pool, no async runtime (`std::thread` + `mpsc`).

---

## 3. Level Install Order

`install_level_payload` (`main.rs:1707`) runs once, on the main thread, after worker delivery. Texture upload precedes UV normalize: `.prm` slot dimensions drive UV normalization, so the renderer must produce `LoadedTexture`s before texel-space UVs convert to `[0,1]` (`main.rs:1700-1703,1735-1749`).

| Order | Stage | Where |
|-------|-------|-------|
| 1 | Seed world gravity from the level's authored value (before scripts run) | `main.rs:1711` |
| 2 | Texture upload from `.prm` sidecars | `main.rs:1737` |
| 3 | UV normalize using uploaded texture dimensions | `main.rs:1748` |
| 4 | Geometry upload (vertex/index buffers) | `main.rs:1753` |
| 5 | Hardcoded mesh-spawn seam: load + spawn one `MeshComponent` (provisional; a future classname handler replaces this) | `main.rs:1756` |
| 6 | Light bridge: one `LightComponent` entity per map-authored light | `main.rs:1854` |
| 7 | Fog bridge: fog-volume entities + renderer pixel-scale / cell masks | `main.rs:1865` |
| 8 | Collision world populated from static geometry (separate from BSP) | `main.rs:1877` |
| 9 | `load_level_sounds` from `sounds/` (fault-tolerant; silent if audio init failed) | `main.rs:1886` |
| 10 | Built-in classname dispatch (`player_spawn` partitioned out; remainder dispatched, handled set stashed) | `main.rs:1894` |
| 11 | Sprite-collection registration for map-spawned emitters | `main.rs:1916` |
| 12 | Data script: `run_data_script` → per-level reactions into `data_registry`; progress tracker init | `main.rs:1948` |
| 13 | Data-archetype sweep (match map placements against `data_registry.entities` not already handled), player spawn, camera teleport to first `player_spawn` (or geometry center) | `main.rs:1969` |
| 14 | Second sprite pass for descriptor-spawned emitters | `main.rs:2040` |
| 15 | `fire_named_event("levelLoad")` | `main.rs:2086` |

Lights come from PRL data via the light bridge, not classname dispatch. Entity types are engine-global and arrive at mod-init via `setupMod` — `setupLevel`/the data script contributes only per-level reactions (see `scripting.md` §2).

---

## 4. Shutdown

There is no in-engine level unload distinct from process exit: this engine runs one level per lifetime, so unload coincides with exit.

- `CloseRequested` (`main.rs:638`) and Escape (`main.rs:646`) call `event_loop.exit()`.
- `exiting()` (`main.rs:1423`) is the clean teardown hook: `audio.release_level_sounds()` (mirrors texture release on unload), then drop renderer and window.
- There is NO `Drop` impl on `App` and NO save-on-exit precedent. `App` holds `script_ctx`/`data_registry` until `run_app` returns and `main` ends.

`suspended()` (`main.rs:572`) is a separate path (platform suspend): it clears renderer/window/fog/collision and the in-flight worker, and resets `boot_state` to `Booting` so `resumed()` rebuilds the surface. It does NOT clear `script_ctx`/`data_registry`.

---

## 5. Lifetimes

| Scope | Cleared on |
|-------|-----------|
| Engine init (preludes, primitive registry, `ScriptCtx`, `ScriptRuntime`, Rust-side registries) | Process exit only — built once in `main`, never recreated. |
| `data_registry.entities` (entity-type descriptors from `setupMod` return) | Engine-global. Survives level unload; survives `suspended()` (`data_registry.rs:13-26,116`). |
| Per-level reactions (`data_registry.reactions`) | Level unload (`data_registry.rs:118`). |
| Renderer, window, audio, collision world, fog bridge | Dropped on `exiting()`; cleared on `suspended()` and rebuilt on `resumed()`. |

`install_level_payload` runs exactly once per process — there is no runtime path to swap levels. Hot reload (debug only) recompiles changed script files via `replace_entity_types`; definition-context changes still require a restart.

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

- **Mod discovery / `content/mods/`.** A scan of `content/base/` and `content/mods/*/` for manifests, a mod manifest format, and per-mod load-order resolution. Today there is a single content root derived from the map path; `run_mod_init` runs against it directly.
- **Mod browser UI and main menu.** Engine-native mod selection (`--mods` flag, persisted selection) and a mod-contributed main menu / level selector. Today the map is a CLI argument and the engine boots straight into it.
- **Mod-supplied splash override.** The frame-1 override hook exists (`pending_splash_override`, `main.rs:1540`) but is always `None` until the mod system ships.
- **Domain folder convention.** Fixed `start-script` entry, `actors/`, `weapons/`, `levels/<name>/<name>.{ts,luau}` auto-discovery, and moving the data script out of the PRL. Today the data script is bundled in the PRL (`main.rs:1951`).
