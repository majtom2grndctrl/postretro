# Boot Sequence

> **Read this when:** wiring startup, splash, level-load, or shutdown code; or reasoning about where new init/teardown belongs.
> **Key invariant:** the engine owns the schedule; mod code runs only inside phases the engine grants. The boot path is single-window; level load/unload is repeatable at runtime.
> **Related:** [Architecture Index](./index.md) · [Scripting](./scripting.md) · [Entity Model](./entity_model.md) · [Build Pipeline](./build_pipeline.md)

Boot code lives in `crates/postretro/src/main.rs` and `crates/postretro/src/startup/`.

---

## 1. Boot Order

Boot is staged so first pixels reach the window at the earliest practical point, then heavier subsystems initialize behind the visible splash. No session-lifetime work happens pre-window — the pre-window pass only assembles the boot-phase `App` (args, content root, camera, frame timing, `session: None`, the `PendingSessionInit` owner) and creates the event loop. The winit-driven splash state machine (Booting → Splash → Frontend/Loading/Running) paints a black frame, then the logo, and only **after** the first visible frame does it install the whole `Session` via `Session::build` (player options I/O, fault-tolerant audio, scripting bootstrap + registries, input/UI/modal group, net endpoint — the heaviest startup work, built behind first pixels), then lazy-init the dev-tools debug UI, recompile stale scripts if the `scripts-build` sidecar is already available (debug-only), run mod init, start the hot-reload watcher (debug-only), and complete full renderer initialization. The canonical development launcher (`cargo run -p xtask -- run ...`) prepares `scripts-build` before the engine process starts; the running engine does not invoke Cargo to build that sidecar.

The renderer initializes in two phases (`rendering_pipeline.md` §7.8): **boot-ready** (surface/device/queue/surface-config + the renderer-owned boot splash pass) gates splash painting; **full-ready** (pipelines, passes, lighting/shadow/UI/fog resources) gates Frontend, Loading completion, Running, the UI pass, and scene rendering. Boot-ready is enough to present black and logo frames.

| # | Stage | Notes |
|---|-------|-------|
| 1 | Init logging and boot clock, collect raw args, resolve selected mod/content root + optional map path | Minimal pre-window work. Net role is NOT parsed here — it defers past first pixels. `--mod <path>` / `--mod=<path>` is the shipped single-mod boot handle; a bare map argument is the raw-path dev bypass under that root. |
| 2 | Create the event loop; no session-lifetime work pre-window | No player options, audio, scripting, or net work happens here. The event loop is created, and the `PendingSessionInit` owner is set up to carry deferred inputs. All session construction (`Session::build`: player options I/O, scripting bootstrap + registries + `ScriptCtx`-clone systems + built-in UI registration, audio, input/UI/modal group, net endpoint) runs post-first-pixel on the logo frame. Primitive closures capture `ScriptCtx` clones; held for the whole session, never recreated. |
| 3 | Construct the app in the Booting state with a `PendingSessionInit` owner holding the raw net args | Mod init, hot-reload watcher, audio, debug UI, net endpoint, and level worker spawn are NOT done here. The pending owner carries the deferred net-setup inputs. |
| 4 | On resume: create window (**visible**), init the **boot-ready** renderer (surface/device/queue/config + boot splash pass), enter Splash, request first redraw | Fires once on desktop; guarded against re-entry. The window is created visible (winit default): the redraw-driven splash loop depends on the OS delivering `RedrawRequested`, which on Windows only happens for a visible window — a hidden window receives no `WM_PAINT`, so a "create hidden, reveal after first present" scheme hangs boot (see § "Window visibility (Windows white-flash)" below). Device creation already requests the FULL feature/limit set (wgpu features can't be added later). Adapter checks the boot splash needs run before the first splash draw; hard renderer requirements fail fast with a named message (`rendering_pipeline.md` §4). No audio, debug UI, net, mod, or level work in this path. |
| 5 | Redraw drives the splash state machine (below) | |
| 6 | Present the frontend or enqueue the boot map load | No CLI map → frontend menu. If the committed frontend declares a background level, the menu stays open while that catalog id loads behind it. CLI map → Loading. |
| 7 | Loading polls worker, then installs the requested level (§3) | PRL parse off-thread; install on main thread. Repeats for runtime load requests. |
| 8 | Steady per-frame loop: Input → Game logic → Audio → Render → Present | Running draws the level. Frontend draws a world-less frame and frontend-safe UI. A frontend menu over a loaded backdrop runs through Running with UI capture and the menu camera pose held. |

### Splash state machine

The boot splash is a **renderer-owned** path — a direct boot splash pass that clears the swapchain and draws the decoded logo as a textured quad. It does NOT use the UI system, UI JSON, glyphon, taffy, mod themes, the UI image registry, or `UiReadSnapshot`. The splash advances one frame per redraw, deferring all CPU-heavy work behind visible pixels. Splash painting returns a present outcome: a frame's stage marks and schedule advance happen only after command submission **and** a successful present; a transient surface failure re-requests a redraw without advancing.

- **First frame (splash-color clear).** Paint the first frame (no logo bound — a `SPLASH_CLEAR_COLOR` clear) into the visible window. After it presents: record `first_black_frame`, then decode and upload the splash image (bounded CPU work, single-threaded). A missing or malformed splash asset warns and keeps the frame at the splash-color clear — never aborts boot. No mod, session, audio, net, or level work yet.
- **Second frame (logo / fallback black).** Paint the splash so it is visible before any user-authored work; record `first_splash_frame` after present. Then, behind the now-visible pixels (or behind the fallback black frame when the asset was unavailable): install the whole `Session` via `Session::build` — player options I/O (seeds input), fault-tolerant audio, scripting bootstrap + registries + `ScriptCtx`-clone systems + input/UI/modal group, then the net endpoint; a hard script-runtime construction failure logs, stores the error, and exits boot. After session install: lazy-init the dev-tools debug UI (window-derived, separate from session install), then complete full renderer initialization, then recompile stale scripts (debug-only), run mod init, and start the hot-reload watcher (debug-only). Full renderer init runs **before** mod init because mod init installs the manifest's UI theme and font faces through full-ready renderer paths (`set_ui_theme` / `register_ui_font`, which touch `Renderer::full`) — those paths panic on the full-ready guard if `Renderer::full` is not yet built. Session install stays ahead of full renderer init: `Session::build` is CPU-side and its failure must early-return before any full-ready work runs. Mod init commits the validated entity descriptors, mod-state declarations, UI/theme data, frontend declaration, and map catalog from `ModManifest`; persistence overlays declared defaults only on the first successful commit. Full renderer init completes **before** splash clears and before any Frontend/Loading-completion/Running/UI/scene path runs. With no CLI map, clear splash and present the frontend menu. With a CLI map, enqueue a raw-path load request and enter Loading.
- **Loading frames.** Non-blocking poll of the worker channel. Keep painting the splash while it runs. On a non-empty payload: install the level, clear the splash, enter Running. Runtime load failures log and return to Frontend. A failed CLI boot-map load exits non-zero.

The two-frame delay is causal: pixels reach the user before any deferred session, audio, net, mod-supplied, or level-load CPU work runs.

### Window visibility (Windows white-flash)

The window is created **visible** (winit default). On Windows the OS paints a freshly-created window with its class background brush (white) before the app's first wgpu frame is on screen, so boot shows a brief pre-first-present white flash, then the splash-color frame. This is an accepted cosmetic artifact.

A "create the window hidden (`with_visible(false)`), then `set_visible(true)` after the first present" scheme was tried to suppress the flash and **hung boot on Windows** — it was reverted. The cause is a winit-0.30/Win32 interaction confirmed in the winit source: `Window::request_redraw()` calls `RedrawWindow(hwnd, NULL, NULL, RDW_INTERNALPAINT)` (`platform_impl/windows/window.rs`), and Windows does not deliver `WM_PAINT` (hence no `RedrawRequested`) to an **invisible** window. The splash state machine is driven entirely by `RedrawRequested` under the default `ControlFlow::Wait` (no `set_control_flow` anywhere; the event loop blocks in `MsgWaitForMultipleObjectsEx(.., INFINITE, ..)`), and `about_to_wait`'s `request_redraw()` produces no paint for a hidden window either. So with a hidden window, frame 0 never receives its redraw → `run_splash_frame_zero` never runs → the window is never revealed → permanent hang (symptom: `[Engine] Window ready` then silence).

A proper flash fix must not gate the first frame on an OS paint event delivered to a hidden window. Viable directions (not implemented): register the window class with a background brush matching `SPLASH_CLEAR_COLOR` so the pre-first-present paint is the splash color rather than white (Win32, would need platform-specific window creation); or drive the very first frame proactively (render + present while hidden, then `set_visible(true)`) **without** depending on `RedrawRequested` — but DXGI present-to-hidden-window behavior (whether it returns `Occluded`) is not guaranteed across drivers, so this was not adopted. A booting engine with a brief cosmetic flash is strictly better than a hang.

### Deferred-session boundary and single commit

Session-dependent systems (script runtime/context, registries, options, input, modal stack/UI registrations, frontend/focus state, net endpoint) are grouped so boot paths before install touch only pending inputs plus boot window/renderer state. The pending session owner is consumed exactly once (take-on-install), so a suspend/resume re-entering the splash loop never re-runs deferred init. The redraw-path stale-script reload drain is gated behind that install — it never runs before the script runtime exists and the first logo frame paints.

### Session boundary (type-enforced)

All session-lifetime state — the scripting core (context, runtime, primitive/reaction/system/classname registries, every system holding a `ScriptCtx` clone or registry reference), the input/UI/modal group, player options + settings path, the committed frontend declaration, the net endpoint, audio, and (dev-tools) the debug UI — is owned by one `Session`, held as `App.session: Option<Session>`. Boot-lifetime fields (renderer, window, camera, boot timings, level worker, the pending-session owner) stay on `App`.

The boundary is **type-enforced**: while `App.session` is `None` (Booting/Splash before install), boot-phase code physically cannot name a session field. There is no `session_mut()` accessor; handlers reach session state through a scoped `self.session.as_ref()/as_mut()` borrow disjoint from the renderer/window field paths.

`Session::build` is the **sole** construction site: whole-or-nothing, synchronous (no `await`, no yield), post-first-pixel, run inside the single install redraw on the logo frame. It builds in boot order: player options I/O (seeds input), fault-tolerant audio, the scripting bootstrap + registries + `ScriptCtx`-clone systems + input/UI/modal group, then the net endpoint. The only fatal step is script-runtime construction; audio, net, and built-in UI-tree disk loads degrade in place.

**Install / failure flow.** The pending-session owner is consumed once via `take_once(pending_session)` (single-commit guard). On success the built `Session` is stored and boot proceeds. On a `Session::build` `Err` the install stores the error in `exit_result`, requests `event_loop.exit()`, and early-returns from the install frame so no later step runs against a `None` session — mirroring the renderer full-init failure path. The build-result → action decision is a pure classifier, tested without a window or GPU.

**Suspend/resume.** Suspend keeps an installed `Session` (and an unconsumed pending owner) alive; the single-commit `take_once` guard means a resume re-entering the splash loop never re-runs deferred init. Only window-derived session state (the dev-tools debug UI) is dropped on suspend and lazily rebuilt on the resumed logo frame.

### Startup timing vocabulary

`StartupTimings` records ordered named stage marks (logged as `[Startup] …`). Boot marks in order: `args_parsed`, `event_loop_created`, `window_created`, `wgpu_init` (boot-ready renderer), `first_black_frame`, `splash_decoded`, `splash_uploaded`, `first_splash_frame`, `audio_init_complete`, `script_runtime_ctor` (post-first-pixel, inside `Session::build`), `net_endpoint_complete`, `session_init_complete`, `renderer_full_init_complete`, and (CLI-map boot) `boot_worker_dispatch`. `first_black_frame` / `first_splash_frame` are recorded only on the presented branch (after command submission + successful present); the window is created visible (see "Window visibility (Windows white-flash)"). `first_black_frame` precedes the net/audio/debug-UI/mod/level-worker marks **and** `script_runtime_ctor` — the script runtime is now built behind first pixels in `Session::build`.

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
| Return to frontend | Clear per-level state, enter Frontend; if the frontend declares a background level, enqueue that catalog id so it loads behind the menu. |
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

The mod frontend hub is the player-facing menu flow. A committed `frontend` manifest block names the menu tree, required static menu camera pose, and optional background level catalog id. With no background level, the menu sits over the world-less Frontend state. With one, the menu is pushed first, then the background catalog id loads as the only active level; UI capture suppresses gameplay controls and the menu camera pose is held every frame. If the named menu tree is missing, the engine fallback frontend menu is pushed before any backdrop load starts. Starting a catalog map clears pushed menus, unloads any backdrop, and loads the selected map. `returnToFrontend()` and `ui.quitToMenu` share the return path: present the frontend menu, unload active level, then reload the declared backdrop if present.

## 5. Shutdown

- A close request or Escape exits the event loop.
- On clean exit, teardown saves persistent mod-state slots best-effort, releases level sounds (mirrors texture release on unload), then drops renderer and window.
- Abnormal termination may lose writes made since the last successful save.
- `App` holds `App.session` (which owns `script_ctx`, the scripting registries, audio, and all session-lifetime state) until the event loop returns and the process ends.

Platform suspend is a separate path: it clears renderer/window/fog/collision and the in-flight worker, and resets state to Booting so resume rebuilds the surface and re-drives the splash loop from frame 0. It does NOT clear the installed `Session` (which owns `script_ctx`, the scripting registries, audio, etc.) or an unconsumed pending-session owner. The single-commit guards keep deferred session init from re-running and let renderer full-init rerun idempotently. Suspend during the black phase re-presents black; during the logo phase it drops GPU splash state and re-presents black then logo; an already-installed session and a completed full renderer survive into the resumed loop.

---

## 6. Lifetimes

| Scope | Cleared on |
|-------|-----------|
| Session-lifetime core (primitive registry, `ScriptCtx`, `ScriptRuntime`, Rust-side registries, input/UI/modal group, options, frontend, net endpoint, audio) | Process exit only. Owned by `Session`, built once post-first-pixel by `Session::build` (not pre-window), then held for the whole run via `App.session`; never recreated. Survives platform suspend. |
| `data_registry.entities` (entity-type descriptors from `ModManifest.entities`) | Engine-global. Survives level unload; survives platform suspend. |
| `data_registry.maps` (mod map catalog from `ModManifest.maps`) | Engine-global. Survives level unload; survives platform suspend. |
| `data_registry.global_reactions` / `global_crossings` (definitions from `ModManifest.reactions` / `crossings`) | Engine-global. Survive level unload; survive platform suspend. |
| Active per-level reactions/crossings (`data_registry.reactions` / `crossings`) and level-scope UI trees | Level unload. Active sets recompose on the next level load from current globals plus that level's catalog tags. |
| Level world, collision world, fog bridge, light bridge, level sounds, sprite collections, per-level GPU resources | Level unload; also cleared/dropped by suspend or exit as applicable. |
| Renderer device/queue, window | Dropped on exit; cleared on suspend and rebuilt on resume. (Audio is session-owned — see the session-lifetime row; it survives suspend.) |

Hot reload (debug only) stages entity descriptors, store declarations, the map catalog, and mod-global reaction/crossing definitions off-thread, then reconciles them on the main thread. Compatible store schemas preserve live values; incompatible changes reject the staged result and preserve the previous descriptors/catalog/globals. A successful staged commit atomically replaces the committed catalog and global reaction/crossing snapshots, then recomposes the active sets for the current level tags. Stale or failed staged results leave committed globals and active sets unchanged. Removed store declarations never clear committed stores.

---

## 7. Manual Lifecycle Checklist

- Launch with no map argument: engine reaches Frontend, paints a world-less frame, no panic.
- Launch with `--mod <campaign>` and no map: frontend menu appears. Confirm declared camera pose, optional background level, and suppressed player controls.
- From that menu: start a catalog map, play, quit to menu, then start again. Confirm no stale backdrop/world residue.
- Bind death to `restartLevel()` in test content: death reloads the active map.
- Launch with no mod frontend: engine fallback frontend menu appears over the world-less Frontend state.
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

- **Mod discovery / `content/mods/`.** A scan of `content/base/` and `content/mods/*/` plus per-mod load-order resolution. Today one active mod root is selected explicitly with singular `--mod`, by `--content-root`, by the CLI map path, or by the default dev root.
- **Mod browser and persisted mod selection.** Engine-native mod browsing, multi-mod selection (`--mods`), and remembering the last selected mod. Singular `--mod` is already the shipped single-mod boot handle.
- **Mod-supplied splash override.** No override hook is active until a splash-override feature ships. The renderer-owned boot splash (§1) consumes only the built-in base asset today; the mod-override consume path is wired but unreachable until the mod system sets it.
- **Classname-driven world mesh spawn.** A classname handler replaces the hardwired single-mesh seam (§3 stage 5).
- **Domain folder convention.** Fixed `start-script` entry, `actors/`, `weapons/`, `levels/<name>/<name>.{ts,luau}` auto-discovery, and moving the data script out of the PRL. Today the data script is bundled in the PRL.
