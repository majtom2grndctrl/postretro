# Boot Sequence

> **Read this when:** wiring startup, splash, level-load, or shutdown code; or reasoning about where new init/teardown belongs.
> **Key invariant:** the engine owns the schedule; mod code runs only inside phases the engine grants. The boot path is single-window; level load/unload is repeatable at runtime.
> **Related:** [Architecture Index](./index.md) · [Scripting](./scripting.md) · [Entity Model](./entity_model.md) · [Build Pipeline](./build_pipeline.md)

Boot code lives in `crates/postretro/src/main.rs` and `crates/postretro/src/startup/`.

---

## 1. Boot Order

Boot runs in two stages: a setup pass that builds session-lifetime state, then a winit-driven state machine (Booting → Splash → Frontend/Loading/Running). Mod init and first level work are deliberately deferred out of the setup pass so the first splash frame paints before any user-authored work runs.

| # | Stage | Notes |
|---|-------|-------|
| 1 | Init logging and boot clock, parse args, resolve map path + content root | |
| 2 | Build the script runtime: `ScriptCtx`, primitive registry (`register_all`), `ScriptRuntime` | Constructed ONCE, before the window. Primitive closures capture `ScriptCtx` clones. Held for the whole session, never recreated. |
| 3 | Build the event loop and Rust-side registries (sequenced/reaction primitives, classname dispatch) | These survive level unload — they describe engine types, not per-level state. |
| 4 | Construct the app in the Booting state | Mod init, hot-reload watcher, and level worker spawn are NOT done here. |
| 5 | On resume: create window, init the renderer (wgpu), build audio, enter Splash, request first redraw | Fires once on desktop; guarded against re-entry. Adapter requirement checks live in renderer init and fail fast with a named message (see `rendering_pipeline.md` §4). Audio init is fault-tolerant — failure logs and runs silent. No mod or level work. |
| 6 | Redraw drives the splash state machine (below) | |
| 7 | Enter Frontend or enqueue the boot map load | No CLI map → Frontend. CLI map → Loading. |
| 8 | Loading polls worker, then installs the requested level (§3) | PRL parse off-thread; install on main thread. Repeats for runtime load requests. |
| 9 | Steady per-frame loop: Input → Game logic → Audio → Render → Present | Running draws the level. Frontend draws a world-less frame and frontend-safe UI. |

### Splash state machine

The splash advances one frame per redraw, deferring all CPU-heavy work behind visible pixels:

- **First frame.** Paint a black frame so the OS window appears immediately (no splash texture bound yet). After present: decode and upload the splash image. No mod or level work.
- **Second frame.** Paint the splash so it is visible before any user-authored work. After paint: recompile stale scripts (debug-only; release no-ops), then run mod init. On success the engine commits the validated entity descriptors, transactional mod-state declarations, and the mod map catalog from `ModManifest.maps`. Persistence overlays declared defaults only on the first successful commit in the process. Store initialization and catalog commit complete before level work and the first gameplay frame. Start the hot-reload watcher (debug-only). With no CLI map, clear splash and enter Frontend. With a CLI map, enqueue a raw-path load request and enter Loading.
- **Loading frames.** Non-blocking poll of the worker channel. Keep painting the splash while it runs. On a non-empty payload: install the level, clear the splash, enter Running. Runtime load failures log and return to Frontend. A failed CLI boot-map load exits non-zero.

The two-frame delay is causal: pixels reach the user before any mod-supplied or level-load CPU work runs.

---

## 2. Worker vs. Main Thread

The level worker parses the PRL only. Texture decode, GPU upload, and UV normalization run on the **main thread** during level install — they need the renderer, which is not `Send`.

Textures are decoded from baked `.prm` mip sidecars, not from the PRL. The worker derives the texture cache root and ships it in the payload so the main thread can locate sidecars without re-deriving the layout. Missing or unusable sidecars degrade per-texture to placeholders with a warning — not a startup failure.

| Owner | Work |
|-------|------|
| Main thread | winit event loop, wgpu (device, queue, all GPU work), audio mixer, all script-VM execution, texture decode/upload, UV normalize, geometry upload. The script runtime and renderer are not `Send` — enforced by the types. |
| Worker thread | PRL parse only. Output is plain `Send` POD — no engine handles, no GPU resources. |

Handoff is an `mpsc` channel. One worker per load request; no thread pool, no async runtime (`std::thread` + `mpsc`).

---

## 3. Level Install Order

Level install runs on the main thread after worker delivery. It is repeatable: every load request reaches this path after any active level has been unloaded. Texture upload precedes UV normalize: `.prm` slot dimensions drive UV normalization, so the renderer must produce loaded textures before texel-space UVs convert to `[0,1]`.

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
| 12 | Data script run → compose active reactions/crossings from matching mod-global definitions plus level-local definitions; progress tracker and crossing detector init from the composed active sets |
| 13 | Data-archetype sweep (match map placements against registered entity types not already handled), player spawn, camera teleport to first player spawn (or geometry center) |
| 14 | Second sprite pass for descriptor-spawned emitters |
| 15 | Mesh model sweep: upload each distinct mesh model once, then resolve every animated mesh entity's clip indices (see mesh-sweep note below) |
| 16 | Fire the `levelLoad` named event |

Lights come from PRL data via the light bridge, not classname dispatch. Entity types and mod-global reaction/crossing definitions arrive at mod-init via the mod manifest. `setupLevel`/the data script contributes level-local reactions and crossings only (see `scripting.md` §2). Level install composes active behavior after the data script returns and before progress/crossing initialization, so both systems see the final active set.

**Mesh-spawn seam.** World mesh spawning is its own install stage, distinct from classname dispatch. The durable contract: map geometry becomes renderable mesh entities here, after geometry upload and before the light/fog bridges. (The current implementation hardwires a single world mesh; a classname-driven handler is planned — see §7.)

**Mesh-sweep order.** The single mesh model sweep (model upload + clip-index resolve) runs *after* the data-archetype sweep (stage 13), so it sees both classname-dispatched `prop_mesh` entities and descriptor-spawned animated meshes (`components.mesh`) — every mesh model uploads once and every animated state resolves its `clip_index`. It runs *before* the `levelLoad` fire (stage 16), because a `setAnimationState` reaction in `levelLoad` requires resolved clip indices. Running it earlier (right after classname dispatch) would miss descriptor-spawned meshes entirely, leaving their clips `None` — the bug this ordering fixes.

---

## 4. Runtime Load/Unload

Runtime level requests drain at the redraw boundary before gameplay/world work for that frame.

| Request | State behavior |
|---------|----------------|
| Load from Frontend | Spawn worker, enter Loading, install payload, enter Running. |
| Load from Running | Unload current level first, then spawn worker and enter Loading. |
| Unload from Running | Clear per-level state, enter Frontend. |
| Failed runtime load | Log diagnostic, clear splash if needed, enter Frontend. |
| Failed CLI boot load | Log diagnostic and exit non-zero. |

`LevelSource::Catalog(id)` resolves against the engine-global `DataRegistry.maps` snapshot before a worker is spawned. A found entry contributes `content_root.join(entry.path)` to the worker and stores `{ catalog_id, path, name, tags }` on the in-flight load so catalog metadata is available before the data script runs; omitted catalog tags are stored as `[]`. A missing id logs a diagnostic and no-ops; it must not unload an active level. Raw path loads (`LevelSource::Path`, including the CLI map path and dev tooling) bypass the catalog and synthesize non-catalog metadata: no catalog id, `tags = []`, and `name` from the file stem.

Clear-on-unload contract:

| Cleared on unload | Kept across unload |
|---|---|
| Level world | Renderer device/queue, window |
| Per-level GPU resources: textures, geometry, mesh-pass caches, smoke collections | Script runtime and script context |
| Light bridge, fog bridge, collision world | Slot table |
| Level sounds, sprite collections | Entity-type registry and mod map catalog |
| Active per-level reactions/crossings, level-local reaction/crossing definitions, UI registrations, and presentation cells | Mod-global reaction/crossing definitions and persisted-state save path |
| Progress tracker, active wieldable, camera pose | Rust-side primitive/classname registries |

Frontend is a no-level steady state. Renderer and audio may exist, but world, collision, fog, level sounds, and per-level registries are empty. Frontend rendering uses a world-less clear and skips gameplay/HUD reads that require a level.

## 5. Shutdown

- A close request or Escape exits the event loop.
- On clean exit, teardown saves persistent mod-state slots best-effort, releases level sounds (mirrors texture release on unload), then drops renderer and window.
- Abnormal termination may lose writes made since the last successful save.
- The app holds `script_ctx`/engine-global registries until the event loop returns and the process ends.

Platform suspend is a separate path: it clears renderer/window/fog/collision and the in-flight worker, and resets state to Booting so resume rebuilds the surface. It does NOT clear `script_ctx`/`data_registry`.

---

## 6. Lifetimes

| Scope | Cleared on |
|-------|-----------|
| Engine init (preludes, primitive registry, `ScriptCtx`, `ScriptRuntime`, Rust-side registries) | Process exit only — built once at startup, never recreated. |
| `data_registry.entities` (entity-type descriptors from `ModManifest.entities`) | Engine-global. Survives level unload; survives platform suspend. |
| `data_registry.maps` (mod map catalog from `ModManifest.maps`) | Engine-global. Survives level unload; survives platform suspend. |
| `data_registry.global_reactions` / `global_crossings` (definitions from `ModManifest.reactions` / `crossings`) | Engine-global. Survive level unload; survive platform suspend. |
| Active per-level reactions/crossings (`data_registry.reactions` / `crossings`) and level-scope UI trees | Level unload. Active sets recompose on the next level load from current globals plus that level's catalog tags. |
| Level world, collision world, fog bridge, light bridge, level sounds, sprite collections, per-level GPU resources | Level unload; also cleared/dropped by suspend or exit as applicable. |
| Renderer device/queue, window, audio mixer | Dropped on exit; renderer/window cleared on suspend and rebuilt on resume. |

Hot reload (debug only) stages entity descriptors, store declarations, the map catalog, and mod-global reaction/crossing definitions off-thread, then reconciles them on the main thread. Compatible store schemas preserve live values; incompatible changes reject the staged result and preserve the previous descriptors/catalog/globals. A successful staged commit atomically replaces the committed catalog and global reaction/crossing snapshots, then recomposes the active sets for the current level tags. Stale or failed staged results leave committed globals and active sets unchanged. Removed store declarations never clear committed stores.

---

## 7. Manual Lifecycle Checklist

- Launch with no map argument: engine reaches Frontend, paints a world-less frame, no panic.
- Launch with a boot map: Splash → Loading → Running, first gameplay frame appears.
- Fresh checkout setup for the `dev-tools` lifecycle trigger: build the generated second dev map first:
  `cargo run -p postretro-level-compiler -- content/dev/maps/combat-demo.map -o content/dev/maps/combat-demo.prl`
- In a `dev-tools` build, trigger `Alt+Shift+L`: unload current level, load `content/dev/maps/combat-demo.prl`, return to Running.
- Confirm no wgpu validation errors during load → unload → load-different.
- Confirm clean visuals after the second load: no stale world geometry, fog, lights, sprites, or level sounds.

---

## 8. Non-Goals

- Per-entity script lifecycle callbacks (see `entity_model.md` §9)
- Multiple simultaneously resident levels or streaming
- Networked mod sync; runtime mod hot-swap mid-level
- Sandboxing mods from each other (mods share VM contexts and `data_registry` by design)

---

## 9. Planned (not implemented)

None of the following exists in code. Do not treat any of it as current behavior; it is recorded only to anchor future work.

- **Mod discovery / `content/mods/`.** A scan of `content/base/` and `content/mods/*/` for manifests, a mod manifest format, and per-mod load-order resolution. Today there is one active content root: `--content-root` when supplied, otherwise derived from the CLI map path, otherwise the default dev root. Mod init runs against that root directly.
- **Mod browser UI and main menu.** Engine-native mod selection (`--mods` flag, persisted selection) and a mod-contributed main menu / level selector. Today the map argument is optional. No map enters Frontend after Splash. A supplied map enqueues Splash → Loading → Running.
- **Mod-supplied splash override.** The frame-two override hook exists but is always absent until the mod system ships.
- **Classname-driven world mesh spawn.** A classname handler replaces the hardwired single-mesh seam (§3 stage 5).
- **Domain folder convention.** Fixed `start-script` entry, `actors/`, `weapons/`, `levels/<name>/<name>.{ts,luau}` auto-discovery, and moving the data script out of the PRL. Today the data script is bundled in the PRL.
</content>
</invoke>
