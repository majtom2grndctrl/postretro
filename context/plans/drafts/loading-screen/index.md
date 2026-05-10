# Loading Screen and Startup Timing

## Goal

Get a window on screen as soon as wgpu is initialized, displaying a baked PNG of the README ASCII-art splash. Move PRL parsing, texture decode, mod init, and level install off the pre-event-loop critical path so the user sees the engine respond immediately. Keep the splash rendering across mod init, worker wait, and install — the splash hides every startup stall until the first level frame replaces it. Make the splash mod-overridable so total conversions own their front door. Instrument every startup stage with `Instant`-based timers so future perf work has data instead of guesses. Codify a sync/async model for the boot sequence so future phases (mod scan, mod browser, level reload) follow one template instead of diverging.

## Boot-phase concurrency model

Pinned here for this plan; promoted to `context/lib/boot_sequence.md` as a new §8 appended after §7 Non-Goals when the plan moves to `ready/`.

- **Main thread owns** the winit event loop, wgpu (device, queue, all GPU work), audio mixer ownership, and all script-VM execution. `ScriptRuntime` and `Renderer` are not `Send`; this is enforced by the types, not by convention.
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
- Mod-overridable splash via a `SplashSource` notion: base PNG is the default; if mod init registers an override, the renderer re-binds before worker delivery. (Mod system itself is out of scope — only the override hook lands here.)
- Move PRL parse + texture decode + UV normalize to a worker thread; deliver the result to the main thread over an `mpsc` channel.
- Boot state machine on `App` driving the transition splash → first level frame.
- Per-stage startup timing logged in one structured line at engine-ready and at level-ready.
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
- [ ] Splash remains visible across mod init, worker wait, and install. Exactly two distinct frames are observable: a splash frame and the first level frame; no flash of black between them.
- [ ] If mod init registers a `SplashSource` override, the new texture re-binds before worker delivery and is the splash visible until level-ready.
- [ ] When the worker delivers `None` (file-not-found path that today logs and continues), the splash remains visible until the user closes the window; closing exits cleanly.
- [ ] One log line at info level, emitted when the renderer is ready, lists durations for every stage between process entry and first splash frame.
- [ ] One log line at info level, emitted when `levelLoad` fires, lists durations for every stage between worker dispatch and level installed.
- [ ] Closing the window with `Esc` or the close button during loading joins or detaches the worker without panic and `App::exit_result` is `Ok(())`. (Esc-to-close currency to be confirmed against current `App` event handling before promotion.)
- [ ] Existing tests in `crates/postretro/src/main.rs` pass (updated for renamed/relocated APIs as needed).

## Tasks

### Task 1: Startup timing scaffold

Add a small `StartupTimings` type that holds an ordered list of `(stage_name, Duration)` entries and an internal `Instant` cursor. Provides `record(&mut self, stage: &'static str)` to capture the delta since the last record, and `summary(&self) -> String` that formats stages on one line.

App holds two `StartupTimings` instances: one for the engine-up line (main-thread stages through `first_splash_frame`, plus splash-phase stages), one composed from the worker's shipped instance plus main-thread install stages for the level-load line.

Stages to record, in order:

| Phase | Stage |
|---|---|
| main thread | `args_parsed`, `script_runtime_ctor`, `event_loop_created`, `window_created`, `wgpu_init`, `splash_decoded`, `splash_uploaded`, `first_splash_frame` |
| splash phase (main thread) | `mod_init`, `mod_splash_swap` (recorded only if a `SplashSource` override fires) |
| worker thread | `prl_parse`, `texture_decode`, `uv_normalize` |
| install (main thread) | `geometry_upload`, `texture_upload`, `bridges_populated`, `classname_dispatch`, `data_script`, `archetype_sweep`, `level_load_event`, `first_level_frame` |

Main thread + splash phase stages roll up into log line A (engine-ready). Worker thread + install stages roll up into log line B (level-ready).

### Task 2: Splash asset and source

Capture the README ASCII art as a PNG screenshot. Commit at `content/base/textures/splash/postretro.png`. RGBA8 source, alpha=255 throughout (no transparency). The asset must have at least 1px of solid background color on all four edges so that `AddressMode::ClampToEdge` letterbox bars sample a uniform color. Resolution chosen so nearest-neighbor sampling at 1280×720 stays crisp — 1920×1080 is fine.

Introduce a `SplashSource` enum with two variants: `Base` (the path above) and `Mod(PathBuf)` (a mod-supplied override). The splash is loaded by a one-shot helper distinct from `texture::load_textures` — it does not participate in the BSP texture-name resolver. The helper takes a `SplashSource`, decodes the PNG, returns a `LoadedTexture`. The helper lives next to the splash render pass.

`SplashSource::Base` is loaded eagerly during engine init, before the event loop. After mod init, if any mod has registered an override, the helper runs again and the renderer re-binds the splash texture in place. The override registration surface lives on the engine side of the scripting boundary; mod-side wiring is deferred until the mod system lands. Today only `Base` is reachable.

### Task 3: Splash render pass

Add a renderer pass that draws a single fullscreen triangle sampling a 2D texture with `FilterMode::Nearest` on both min and mag. Pass is opt-in: when no splash texture is bound, the pass is skipped and the present frame falls through to the existing clear-color path.

Three new methods on `Renderer`: bind a splash texture from a `LoadedTexture` (used at first install and at mod-override swap), clear the bound splash, and check whether one is bound. The render-frame entry point gains a branch: while `BootState` is `Splash`, the present frame issues only the splash pass against the swapchain view. Otherwise existing behavior is unchanged. Texture format: `Rgba8UnormSrgb` (matches world textures). The splash render pass uses `LoadOp::Clear` with a black clear color and writes to the swapchain view.

The splash uses an aspect-correct UV mapping — UVs computed in the vertex shader from the swapchain dimensions and the splash texture's dimensions to avoid stretching. Letterbox bars (when aspect mismatches) sample the splash's edge texels via `AddressMode::ClampToEdge`; effectively the splash centers and scales to fit.

### Task 4: Worker thread for asset load

Replace the synchronous `load_level` / `texture::load_textures` / `normalize_prl_uvs` calls in `main()` with a worker spawn. The worker takes the resolved map path and content root, performs all three steps with timing, and sends a `LoadOutcome` over an `mpsc::Sender`. `LoadOutcome` carries the `Option<LevelWorld>`, `Option<TextureSet>`, and `StartupTimings` from the worker — all `Send` POD per the concurrency model.

The `Receiver` lives on `App`. On every redraw, before any per-frame work, `try_recv` is checked once. On receipt, run `install_level` inline that frame; transition `Splash → Running` at end of install. The frame that runs install presents one final splash frame (because the splash pass already encoded for that frame); the next frame presents the installed level. `window.request_redraw()` is called every frame while in `Splash` so the redraw loop drives worker-delivery polling regardless of OS redraw suppression.

The worker is `std::thread::spawn`. Its `JoinHandle` is owned by `App` and detached on window-close-during-load: PRL parse and PNG decode are bounded CPU work, and the worker's send into a dropped receiver returns an error which the worker ignores. Phase cancellation as a general capability is not part of this plan.

### Task 5: Renderer install method split

`Renderer::new` today (`crates/postretro/src/render/mod.rs:479`) accepts `Option<&LevelGeometry>` and `Option<&TextureSet>` and inlines GPU upload in the `Some` branches. The concurrency model — worker parses, main thread uploads — requires uploads to be addressable as a separate step.

Split the upload paths into two new public methods: `Renderer::install_level_geometry(&LevelGeometry)` and `Renderer::install_textures(&TextureSet)`. Update `Renderer::new` to take no level/texture arguments; all current callers pass `None`/`None` paths through the split methods on a separate frame. Update the level compiler integration tests and any other callers in the same pass — pre-release, no compat shim.

### Task 6: Boot state machine and install path

Introduce a `BootState` enum on `App`:

| State | Meaning |
|---|---|
| `Booting` | Pre-event-loop or pre-`resumed`; window not yet created |
| `Splash` | Window + renderer up; splash visible; mod init, worker wait, and install all happen here |
| `Running` | Level installed and presented; normal frame loop |

`resumed()` shrinks to: create window, create renderer, upload base splash texture, kick off worker, transition `Booting → Splash`.

In `Splash`:

1. First frame paints. Then `run_mod_init` runs (deferred to the second `Splash` frame so the first paint is guaranteed before mod scripts touch the engine). `ScriptRuntime::start_watcher` (in `crates/postretro/src/scripting/`) follows it.
2. If `mod_init` registers a `SplashSource` override, the helper from Task 2 reloads and `Renderer` re-binds.
3. Each frame polls the worker channel. On `Some(LoadOutcome { level: Some, textures: Some, .. })`: run `install_level` (Task 5's `install_level_geometry` + `install_textures`, then bridges populate, fog populate, collision populate, classname dispatch → level data script → archetype sweep → `levelLoad` fire), then transition `Splash → Running`. The current frame still presents the splash; the next frame presents the installed level. The current `level_load_fired` flag is subsumed by the state.
4. On `LoadOutcome` with `None` payload (file-not-found path), state remains `Splash` indefinitely; no transition to `Running`.

### Task 7: Shutdown safety

Window-close during `Splash` must not panic. The worker's `JoinHandle` is dropped (detached) on exit; the worker's `Sender` write into a dropped `Receiver` returns an error which the worker ignores. PRL parse and PNG decode are bounded CPU work — no risk of unbounded background activity.

### Task 8: Timing log emission

Two info-level log lines:

- One when the renderer is ready and the first splash frame has presented, summarizing all stages from process entry to that point.
- One when `levelLoad` has fired, summarizing all stages from worker dispatch through first level frame.

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
- Base splash PNG path: `content/base/textures/splash/postretro.png`. Decoded eagerly on the main thread before the worker is dispatched.
- Worker channel: `std::sync::mpsc::channel::<LoadOutcome>()`. `App` polls the receiver at the top of each `RedrawRequested`.
- Stage timer log target: `[Startup]` prefix, info level. One line per phase.

## Open questions

- **`Esc`-to-close coverage.** AC asserts `Esc` exits cleanly during `Splash`. Confirm against current `App` event handling before promotion; if not wired, either add it as a sub-task of Task 7 or drop `Esc` from the AC.
- **Mod splash registration surface.** Where on the scripting primitive surface does a mod register a `SplashSource` override? Pin once the broader mod system plan establishes the start-script API. For now, the engine-side hook exists; mod-side wiring is deferred.
