# Loading Screen and Startup Timing

## Goal

Get a window on screen as soon as wgpu is initialized, displaying a baked PNG of the README ASCII-art splash. Move PRL parsing, texture decode, mod init, and level install off the pre-event-loop critical path so the user sees the engine respond immediately. Keep the splash rendering across mod init, worker wait, and install — the splash hides every startup stall until the first level frame replaces it. Make the splash mod-overridable so total conversions own their front door. Instrument every startup stage with `Instant`-based timers so future perf work has data instead of guesses. Codify a sync/async model for the boot sequence so future phases (mod scan, mod browser, level reload) follow one template instead of diverging.

## Boot-phase concurrency model

Pinned here for this plan; promoted to `context/lib/boot_sequence.md` as a new §8 appended after §7 Non-Goals when the plan moves to `ready/`.

- **Main thread owns** the winit event loop, wgpu (device, queue, all GPU work), the audio mixer, and all script-VM execution. (Audio is documented for the appendix; this plan does not touch audio threading.) `ScriptRuntime` and `Renderer` are not `Send`; this is enforced by the types, not by convention.
- **Worker threads own** file I/O, parsing, and decoding. Outputs must be plain `Send` data — no engine handles, no GPU resources. Today: PRL parse, PNG decode. Future: audio decode, mod-manifest scan.
- **Handoff** is `mpsc` channels carrying POD. One worker per kicked-off task; no thread pool until measurement demands one.
- **Phases are sequential at the architecture level; intra-phase work is parallel.** The engine does not advance from phase N to phase N+1 until phase N's worker outputs are consumed and any main-thread follow-up (GPU upload, script run, registry populate) has completed. This is what keeps the boot sequence linear and reasonable while still letting individual phases parallelize internally.
- **No async runtime.** `std::thread` + `mpsc`. Revisit only if a future phase produces evidence the model is insufficient.

This plan is the first application of the model. The level-load phase parses on a worker, uploads on the main thread, and runs scripts on the main thread. The splash render pass keeps painting through every stall in between, so the visible boot is two frames: last splash frame, first level frame.

## Scope

### In scope

- Refactor `main()` so `EventLoop::run_app` is reached without first loading a level.
- Add a splash render pass: fullscreen-triangle pipeline sampling a single 2D texture with nearest-neighbor — matches the engine's "blocky pixelated textures" aesthetic.
- Ship a baked PNG of the README ASCII art as the base splash asset, loaded from disk at boot.
- Mod-overridable splash via a `SplashSource` notion: base PNG is the default. If a mod registers an override during `mod_init`, the renderer re-binds before the level-load worker is spawned. If no override is registered, Base remains in place — most mods inherit Base; only total conversions or splash-conscious mods opt out. (Mod system itself is out of scope — only the override hook lands here.)
- Move PRL parse + texture decode + UV normalize to a worker thread; deliver the result to the main thread over an `mpsc` channel.
- Boot state machine on `App` driving the transition splash → first level frame.
- Per-stage startup timing logged in three structured lines: one at first splash frame (engine boot), one after mod init completes, one at first level frame (level load).
- Clean shutdown when the user closes the window mid-load.

### Out of scope

- Progress text, percent indicator, or any dynamic text on the splash (would force a text-rendering library decision; defer).
- Animated splash, fades, transitions — splash is replaced by the first real frame instantly.
- Adoption of `tokio` or any async runtime; `std::thread` + `mpsc` is sufficient.
- Migration from `log` to `tracing`.
- Persisting timings to a JSON / CSV file or external profiler integration.
- Mod browser, main menu, or any phase from `boot_sequence.md` §3 beyond phase 0–4.
- Cancellation of in-flight load (close-during-load lets the worker finish into a dropped receiver).
- A `--no-splash` flag.
- Mod manifest format and mod activation. The splash override hook is path-driven: today only the base path is wired; future mod-system work supplies additional paths.

## Acceptance criteria

- [ ] On a debug build of `cargo run -p postretro`, the OS-decorated window with the base splash visible appears before PRL parse begins, as evidenced by stage timestamps in the log.
- [ ] The splash texture covers the entire window at any aspect ratio, sampled with nearest-neighbor (no bilinear blur on letterbox bars or stretched pixels).
- [ ] Splash remains visible across mod init, worker wait, and install. Exactly two visually distinct frame contents are observable across the boot — the splash image and the first level frame — with no flash of black between them. (Many splash frames may be presented; their pixel contents are identical until the level frame replaces them.)
- [ ] The splash render pass accepts a rebind from the engine-side override helper and presents the new texture on the next frame with no flash of the previous splash. (End-to-end mod-driven override is verified by the mod-system plan; this plan delivers the renderer-side capability and the engine-side hook only.)
- [ ] If `mod_init` registers no override, Base remains in place across the entire `Splash` phase — no rebind, no `mod_splash_swap` recorded.
- [ ] When the worker delivers `None` (file-not-found path that today logs and continues), the splash remains visible until the user closes the window; closing exits cleanly.
- [ ] Log line A (info, emitted at first splash frame): lists durations for all engine-boot stages from process entry through `first_splash_frame`.
- [ ] Log line B (info, emitted after mod init completes): lists durations for `mod_init` and, if applicable, `mod_splash_swap`.
- [ ] Log line C (info, emitted at start of first level frame): lists durations for all level-load stages from `worker_dispatch` through `first_level_frame`.
- [ ] Closing the window with `Esc` or the close button during `Splash` detaches the worker `JoinHandle` without panic. `App::exit_result` is `Ok(())`. (Esc handling exists today at `crates/postretro/src/main.rs:607-620`; Task 7 only ensures the exit path interacts correctly with the worker handle.)
- [ ] Existing tests in `crates/postretro/src/main.rs` pass (updated for renamed/relocated APIs as needed).

## Tasks

### Task 1: Startup timing scaffold

Add a small `StartupTimings` type that holds an ordered list of `(stage_name, Duration)` entries and an internal `Instant` cursor. Provides `record(&mut self, stage: &'static str)` to capture the delta since the last record, and `summary(&self) -> String` that formats stages on one line.

App holds three `StartupTimings` instances, one per log line.

Worker → main thread merge: the worker ships its `Vec<(name, Duration)>` (the cursor stays thread-local). The main thread records `worker_dispatch` before spawning and `worker_delivered` on `try_recv` success, then concatenates the worker's entries between those two markers in the level-load timing instance before recording install stages.

Stages to record, grouped by log line:

| Log line | Phase | Stage |
|---|---|---|
| A — engine boot | main thread | `args_parsed`, `script_runtime_ctor`, `event_loop_created`, `window_created`, `wgpu_init`, `splash_decoded`, `splash_uploaded`, `first_splash_frame` |
| B — mod init | splash phase (main thread) | `mod_init`, `mod_splash_swap` (recorded only if a `SplashSource` override fires) |
| C — level load | main thread (pre-worker) | `worker_dispatch` |
| C — level load | worker thread | `prl_parse`, `texture_decode`, `uv_normalize` |
| C — level load | main thread (post-worker) | `worker_delivered`, `geometry_upload`, `texture_upload`, `bridges_populated`, `classname_dispatch`, `data_script`, `archetype_sweep`, `level_load_event`, `first_level_frame` |

Line A emits at `first_splash_frame`. Line B emits after `mod_init` (and `mod_splash_swap` if it fires) and before worker spawn. Line C emits at the start of the first level frame. Each line answers one independent perf question: cold-start engine cost, mod-set script cost, and map load cost.

`worker_dispatch` and `worker_delivered` belong to the level-load (C) timing instance, never the engine-boot (A) instance. The worker's internal entries (`prl_parse`, `texture_decode`, `uv_normalize`) are independent deltas from the worker's thread-local cursor; their wall-clock spans overlap with `worker_delivered` (main-thread wait) by design — line C reports both the main-thread wait and the worker's internal breakdown.

`mod_splash_swap` spans from override registration (the moment a mod's `mod_init` resolves the registration hook) to renderer-rebind-complete (`Renderer::install_splash` returns).

### Task 2: Splash asset and source

Capture the README ASCII art as a PNG screenshot. Commit at `content/base/textures/splash/postretro-ascii-art.png`. RGBA8 source, alpha=255 throughout (no transparency). The asset must have at least 1px of solid background color on all four edges so that `AddressMode::ClampToEdge` letterbox bars sample a uniform color. Resolution chosen so nearest-neighbor sampling at 1280×720 stays crisp — 1920×1080 is fine.

Introduce a `SplashSource` enum with two variants: `Base` (the path above) and `Mod(PathBuf)` (a mod-supplied override). The splash is loaded by a one-shot helper distinct from `texture::load_textures` — it does not participate in the BSP texture-name resolver. The helper takes a `SplashSource`, decodes the PNG, returns a `LoadedTexture`. The helper lives next to the splash render pass.

`SplashSource::Base` is decoded to a CPU `LoadedTexture` eagerly during engine init, before the event loop (`splash_decoded`). GPU upload happens in `App::resumed()` after `Renderer::new` returns (`splash_uploaded`). After `mod_init` returns, the engine inspects whether any mod registered an override. If yes, the helper runs again and the renderer re-binds the splash texture in place. If no, Base remains in place — no rebind, no `mod_splash_swap` recorded. The override registration surface lives on the engine side of the scripting boundary; mod-side wiring is deferred until the mod system lands. Today only `Base` is reachable.

### Task 3: Splash render pass

Add a renderer pass that draws a single fullscreen triangle sampling a 2D texture with `FilterMode::Nearest` on both min and mag. Pass is opt-in: when no splash texture is bound, the pass is skipped and the present frame falls through to the existing clear-color path.

Three new methods on `Renderer`: bind a splash texture from a `LoadedTexture` (used at first install and at mod-override swap), clear the bound splash, and check whether one is bound. The render-frame entry point gains a branch: while `BootState` is `Splash`, the present frame issues only the splash pass against the swapchain view. Otherwise existing behavior is unchanged. Texture format: `Rgba8UnormSrgb` (matches world textures). Sampled as `Rgba8UnormSrgb` and written to the existing sRGB swapchain view; no manual gamma in the fragment shader. The splash render pass uses `LoadOp::Clear` with a black clear color and writes to the swapchain view.

The splash uses an aspect-correct UV mapping — UVs computed in the vertex shader from the swapchain dimensions and the splash texture's dimensions to avoid stretching. Letterbox bars (when aspect mismatches) sample the splash's edge texels via `AddressMode::ClampToEdge`; effectively the splash centers and scales to fit. Swapchain dimensions and texture dimensions are delivered to the vertex shader via a small uniform buffer. `Renderer` writes the UBO on splash bind. `Renderer::resize` rewrites the UBO whenever a splash texture is currently bound; if no splash is bound, the splash UBO write is skipped.

### Task 4: Worker thread for asset load

Move the synchronous `load_level` / `texture::load_textures` / `normalize_prl_uvs` work out of `main()`. Spawn the worker after `mod_init` completes — see Task 6 for exact placement. (Deferring worker spawn until post-`mod_init` aligns with the eventual main-menu-first boot sequence: in the full product, level selection happens after the menu, so the worker fundamentally cannot be kicked off in `resumed()`. Today's CLI-arg path becomes "menu auto-confirms the supplied map" in the future flow. The defer also makes the splash override ordering causal rather than racy — an override registered during `mod_init` is guaranteed to land before worker delivery.)

The worker takes the resolved map path and content root, performs all three steps with timing, and sends a `LoadOutcome` over an `mpsc::Sender`. `LoadOutcome` is shaped as:

```
type LoadOutcome = Result<LevelPayload, anyhow::Error>;

struct LevelPayload {
    level: Option<LevelWorld>,
    textures: Option<TextureSet>,
    timings: Vec<(&'static str, Duration)>,
}
```

`Ok(LevelPayload { level: None, .. })` is the file-not-found path (today logs and continues — engine boots to idle window, a valid state in the engine's value system). `Err(e)` covers parse and IO errors that previously would have been swallowed by the inline path; `App` logs and remains in `Splash` indefinitely. All fields are `Send` per the concurrency model.

The `Receiver` lives on `App`. On every redraw, before any per-frame work, `try_recv` is checked once. On receipt, run `install_level` inline that frame; transition `Splash → Running` at end of install. The frame that runs install presents one final splash frame (because the splash pass already encoded for that frame); the next frame presents the installed level. `window.request_redraw()` is called every frame while in `Splash` so the redraw loop drives worker-delivery polling regardless of OS redraw suppression.

The worker is `std::thread::spawn`. Its `JoinHandle` is owned by `App` and detached on window-close-during-load: PRL parse and PNG decode are bounded CPU work, and the worker's send into a dropped receiver returns an error which the worker ignores. Phase cancellation as a general capability is not part of this plan.

### Task 5: Renderer install method split

`Renderer::new` today (`crates/postretro/src/render/mod.rs:479`) accepts `Option<&LevelGeometry>` and `Option<&TextureSet>` and inlines GPU upload in the `Some` branches. The concurrency model — worker parses, main thread uploads — requires uploads to be addressable as a separate step.

Split the upload paths into two new public methods: `Renderer::install_level_geometry(&LevelGeometry)` and `Renderer::install_textures(&TextureSet)`. Update `Renderer::new` to take no geometry/texture arguments. Callers that previously passed `Some(...)` now invoke `install_level_geometry` / `install_textures` after construction; callers that passed `None` / `None` simply omit the install calls. Update the level compiler integration tests and any other callers in the same pass — pre-release, no compat shim.

Note: the worker's `LevelWorld` (parsed PRL aggregate, `crates/postretro/src/prl.rs:163`) is not the same type as `LevelGeometry` (renderer-side, `crates/postretro/src/render/mod.rs:336`). Conversion from `LevelWorld` to `LevelGeometry` happens on the main thread, between `try_recv` and `install_level_geometry`. No conversion function exists today (today's flow constructs `LevelGeometry` inline in `resumed()`); introduce one as part of this task — name and module location are the implementer's choice.

### Task 6: Boot state machine and install path

Introduce a `BootState` enum on `App`:

| State | Meaning |
|---|---|
| `Booting` | Pre-event-loop or pre-`resumed`; window not yet created |
| `Splash` | Window + renderer up; splash visible; mod init, worker wait, and install all happen here |
| `Running` | Level installed and presented; normal frame loop |

`resumed()` shrinks to: create window, create renderer, upload base splash texture, transition `Booting → Splash`. Worker spawn is deferred to the second `Splash` frame (after `mod_init`); see step ordering below.

In `Splash`, the canonical install order is `install_level_geometry` → `install_textures` → bridges populate → fog populate → collision populate → classname dispatch → level data script → archetype sweep → `levelLoad` fire. This matches today's `resumed()` flow but expands `boot_sequence.md` §4, which currently elides the geometry/texture upload and the bridge/fog/collision populate steps. Update §4 in the same PR that promotes this plan.

Per-Splash-frame order:

1. **First Splash frame.** Paint only — splash visible. After paint, emit log line A. (`run_mod_init` is deferred to the second frame so the first paint is guaranteed before mod scripts touch the engine.)
2. **Second Splash frame, before paint:**
   - Run `ScriptRuntime::run_mod_init` (`crates/postretro/src/scripting/runtime.rs:219`) and `ScriptRuntime::start_watcher` (`crates/postretro/src/scripting/runtime.rs:108`). Record `mod_init`.
   - If `mod_init` registered a `SplashSource` override, run the helper from Task 2 and rebind via the renderer's splash-install method. Record `mod_splash_swap`.
   - Emit log line B.
   - Record `worker_dispatch` into the level-load (C) timing instance.
   - Spawn the level-load worker (Task 4).
3. **Third Splash frame onward, before paint:** `try_recv` the worker channel.
   - On `Ok(LevelPayload { level: Some(_), textures: Some(_), .. })`: record `worker_delivered`, concatenate the worker's `timings` into the C instance, run install in canonical order recording each install stage, transition `Splash → Running`. The current frame still presents the splash; the next frame presents the installed level. The current `level_load_fired` flag (`crates/postretro/src/main.rs:374`) is subsumed by the state.
   - On `Ok(LevelPayload { level: None, .. })` (file-not-found path): record `worker_delivered`, log the no-level state, remain in `Splash` indefinitely.
   - On `Err(e)`: log the error, remain in `Splash` indefinitely.

### Task 7: Shutdown safety

Window-close during `Splash` must not panic. The worker's `JoinHandle` is dropped (detached) on exit; the worker's `Sender` write into a dropped `Receiver` returns an error which the worker ignores. PRL parse and PNG decode are bounded CPU work — no risk of unbounded background activity.

### Task 8: Timing log emission

Three info-level log lines, one per boot phase:

- **Line A** — emitted at `first_splash_frame`. Summarizes engine-boot stages from process entry through `first_splash_frame`. Cost varies with binary, GPU init, and splash asset decode; independent of mod set and map.
- **Line B** — emitted after mod init completes (after `mod_init`, after `mod_splash_swap` if it fired). Summarizes splash-phase stages. Cost varies with mod set; independent of map.
- **Line C** — emitted at the start of the first level frame (after the present that displays it). Summarizes level-load stages from `worker_dispatch` through `first_level_frame`. Cost varies with map size and texture count.

Single-line format, stage name and duration in milliseconds, comma-separated. Format pinned by Task 1's `summary()` method. Example: `[Startup] args_parsed=0.4ms, script_runtime_ctor=12.1ms, ...` (one decimal, comma-separated, ms suffix).

## Sequencing

**Phase 1 (sequential):** Task 1 — timing scaffold. Every other task records through it.
**Phase 2 (concurrent):** Task 2 (asset + `SplashSource`), Task 3 (splash render pass), Task 4 (worker thread), Task 5 (Renderer install split). Independent; meet at Task 6. Task 3 can be implemented against a placeholder texture; final visual verification needs Task 2 done.
**Phase 3 (sequential):** Task 6 — boot state machine and install path. Consumes the splash pass, the worker channel, the install methods, and the timing scaffold.
**Phase 4 (sequential):** Task 7 (shutdown safety) and Task 8 (timing log emission). Both depend on the state machine being in place.

## Rough sketch

- New module `crates/postretro/src/startup.rs` holds `StartupTimings`, `BootState`, and `SplashSource`. Owned by `App`.
- New module `crates/postretro/src/render/splash.rs` holds the splash pipeline (vertex shader emits fullscreen triangle, fragment shader samples a single texture). The pipeline is created during `Renderer::new` regardless of whether a splash texture is bound — cost is one pipeline object, negligible.
- Splash texture upload reuses the existing `LoadedTexture` shape and the existing texture-creation helper inside the renderer (sampler config differs from world textures: nearest-neighbor, clamp-to-edge).
- Base splash PNG path: `content/base/textures/splash/postretro-ascii-art.png`. Decoded eagerly on the main thread before the worker is dispatched.
- Worker channel: `std::sync::mpsc::channel::<LoadOutcome>()`. `App` polls the receiver at the top of each `RedrawRequested`.
- Stage timer log target: `[Startup]` prefix, info level. One line per phase.

## Open questions

- **Mod splash registration surface.** Where on the scripting primitive surface does a mod register a `SplashSource` override? Pin once the broader mod system plan establishes the start-script API. For now, the engine-side hook exists; mod-side wiring is deferred.
