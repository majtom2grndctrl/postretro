# Loading Screen and Startup Timing

## Goal

Get a window on screen as soon as wgpu is initialized, displaying a baked PNG of the README ASCII-art splash. Move PRL parsing, texture decode, mod init, and level install off the pre-event-loop critical path so the user sees the engine respond immediately. Instrument every startup stage with `Instant`-based timers so future perf work has data instead of guesses. Codify a sync/async model for the boot sequence so future phases (mod scan, mod browser, level reload) follow one template instead of diverging.

## Boot-phase concurrency model

Pinned here for this plan; promoted to `context/lib/boot_sequence.md` when the plan moves to `ready/`.

- **Main thread owns** the winit event loop, wgpu (device, queue, all GPU work), audio mixer ownership, and all script-VM execution. `ScriptRuntime` and `Renderer` are not `Send`; this is enforced by the types, not by convention.
- **Worker threads own** file I/O, parsing, and decoding. Outputs must be plain `Send` data — no engine handles, no GPU resources. Today: PRL parse, PNG decode. Future: audio decode, mod-manifest scan.
- **Handoff** is `mpsc` channels carrying POD. One worker per kicked-off task; no thread pool until measurement demands one.
- **Phases are sequential at the architecture level; intra-phase work is parallel.** The engine does not advance from phase N to phase N+1 until phase N's worker outputs are consumed and any main-thread follow-up (GPU upload, script run, registry populate) has completed. This is what keeps the boot sequence linear and reasonable while still letting individual phases parallelize internally.
- **No async runtime.** `std::thread` + `mpsc`. Revisit only if a future phase produces evidence the model is insufficient.

This plan is the first application of the model. The level-load phase parses on a worker, uploads on the main thread, and runs scripts on the main thread — and the phase boundary is the splash → first level frame transition.

## Scope

### In scope

- Refactor `main()` so `EventLoop::run_app` is reached without first loading a level.
- Add a splash render pass: fullscreen-triangle pipeline sampling a single 2D texture with nearest-neighbor — matches the engine's "blocky pixelated textures" aesthetic.
- Bundle a baked PNG of the README ASCII art as the splash asset.
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

## Acceptance criteria

- [ ] On a debug build of `cargo run -p postretro`, the OS-decorated window with the splash visible appears before PRL parse begins, as evidenced by stage timestamps in the log.
- [ ] The splash texture covers the entire window at any aspect ratio, sampled with nearest-neighbor (no bilinear blur on letterbox bars or stretched pixels).
- [ ] When the worker delivers a successful level load, the next presented frame is the rendered level — no flash of black, no second splash frame after install.
- [ ] When the worker delivers `None` (file-not-found path that today logs and continues), the splash remains visible until the user closes the window; closing exits cleanly.
- [ ] One log line at info level, emitted when the renderer is ready, lists durations for every stage between process entry and first splash frame.
- [ ] One log line at info level, emitted when `levelLoad` fires, lists durations for every stage between worker dispatch and level installed.
- [ ] Closing the window with `Esc` or the close button during loading joins or detaches the worker without panic and `App::exit_result` is `Ok(())`.
- [ ] Existing tests in `crates/postretro/src/main.rs` pass unchanged.

## Tasks

### Task 1: Startup timing scaffold

Add a small `StartupTimings` type that holds an ordered list of `(stage_name, Duration)` entries and an internal `Instant` cursor. Provides `record(&mut self, stage: &'static str)` to capture the delta since the last record, and `summary(&self) -> String` that formats stages on one line. One instance owned by `App` (main-thread stages); the worker thread owns its own and ships it back through the load channel for merging.

Stages to record, in order:

| Phase | Stage |
|---|---|
| main thread | `args_parsed`, `script_runtime_ctor`, `event_loop_created`, `window_created`, `wgpu_init`, `splash_uploaded`, `first_splash_frame` |
| worker thread | `prl_parse`, `texture_decode`, `uv_normalize` |
| post-deliver | `mod_init`, `geometry_upload`, `texture_upload`, `bridges_populated`, `classname_dispatch`, `data_script`, `archetype_sweep`, `level_load_event`, `first_level_frame` |

`mod_init` runs on the main thread because `ScriptRuntime` is not `Send`; it is recorded once it interleaves between splash frames after the window is up.

### Task 2: Splash asset capture

Capture the README ASCII art as a PNG screenshot. Commit at `content/base/textures/splash/postretro.png`. RGBA8, solid background, no transparency. Resolution chosen so nearest-neighbor sampling at 1280×720 stays crisp — 1920×1080 is fine. One-time asset commit, no code dependency.

The splash is loaded by a one-shot helper distinct from `texture::load_textures` — it does not participate in the BSP texture-name resolver. The helper lives next to the splash render pass.

### Task 3: Splash render pass

Add a renderer pass that draws a single fullscreen triangle sampling a 2D texture with `FilterMode::Nearest` on both min and mag. Pass is opt-in: when no splash texture is bound, the pass is skipped and the present frame falls through to the existing clear-color path.

Two new methods on `Renderer`: one to upload the splash texture from a `LoadedTexture`, one to clear it. The render-frame entry point gains a branch: if a splash texture is bound and `is_ready()` is false, the present frame issues only the splash pass against the swapchain view. Otherwise existing behavior is unchanged.

The splash uses an aspect-correct UV mapping — UVs computed in the vertex shader from the swapchain dimensions and the splash texture's dimensions to avoid stretching. Letterbox bars (when aspect mismatches) sample the splash's edge texels via `AddressMode::ClampToEdge`; effectively the splash centers and scales to fit. No additional border fill needed because the ASCII art will already have a solid background.

### Task 4: Worker thread for asset load

Replace the synchronous `load_level` / `texture::load_textures` / `normalize_prl_uvs` calls in `main()` with a worker spawn. The worker takes the resolved map path and content root, performs all three steps with timing, and sends a `LoadOutcome` over an `mpsc::Sender`. `LoadOutcome` carries the `Option<LevelWorld>`, `Option<TextureSet>`, and `StartupTimings` from the worker — all `Send` POD per the concurrency model.

The `Receiver` lives on `App`. On every redraw, before any per-frame work, `try_recv` is checked once. On receipt, transition the boot state to `Installing` and run the install steps inline that frame.

The worker is `std::thread::spawn` — no rayon, no tokio. Its `JoinHandle` is owned by `App` and detached on window-close-during-load: PRL parse and PNG decode are bounded CPU work, and the worker's send into a dropped receiver returns an error which the worker ignores. Phase cancellation as a general capability is not part of this plan.

### Task 5: Boot state machine and install path

Introduce a `BootState` enum on `App`:

| State | Meaning |
|---|---|
| `Booting` | Pre-event-loop or pre-`resumed`; window not yet created |
| `Splash` | Window + renderer up; splash visible; worker may still be running |
| `Installing` | Worker delivered; install steps run this frame |
| `Running` | Level installed, normal frame loop |

`resumed()` shrinks to: create window, create renderer (no level), upload splash texture, kick off worker if not already started, transition to `Splash`. The light-bridge populate, fog populate, collision populate, classname dispatch, and the existing first-redraw work (data script, archetype sweep, `levelLoad` fire) all move to a single `install_level` routine called once on the `Installing` → `Running` transition. The current `level_load_fired` flag is subsumed by the state.

`run_mod_init` moves out of pre-event-loop main() and into the first frame of `Splash` state, so it does not delay window-up. Existing `script_runtime.start_watcher` follows it.

### Task 6: Shutdown safety

Window-close during `Splash` or `Installing` must not panic. The worker's `JoinHandle` is dropped (detached) on exit; the worker's `Sender` write into a dropped `Receiver` returns an error which the worker ignores. PRL parse and PNG decode are bounded CPU work — no risk of unbounded background activity.

### Task 7: Timing log emission

Two info-level log lines:

- One when the renderer is ready and the first splash frame has presented, summarizing all stages from process entry to that point.
- One when `levelLoad` has fired, summarizing all stages from worker dispatch through first level frame.

Single-line format, stage name and duration in milliseconds, comma-separated. Format pinned by Task 1's `summary()` method.

## Sequencing

**Phase 1 (sequential):** Task 1 — timing scaffold. Every other task records through it.
**Phase 2 (concurrent):** Task 2 (asset capture), Task 3 (splash render pass), Task 4 (worker thread). All three are independent and only meet at Task 5.
**Phase 3 (sequential):** Task 5 — boot state machine. Consumes the splash pass, the worker channel, and the timing scaffold.
**Phase 4 (sequential):** Task 6 (shutdown safety) and Task 7 (timing log emission). Both depend on the state machine being in place.

## Rough sketch

- New module `crates/postretro/src/startup.rs` holds `StartupTimings` and `BootState`. Owned by `App`.
- New module `crates/postretro/src/render/splash.rs` holds the splash pipeline (vertex shader emits fullscreen triangle, fragment shader samples a single texture). The pipeline is created during `Renderer::new` regardless of whether a splash texture is bound — cost is one pipeline object, negligible.
- Splash texture upload reuses the existing `LoadedTexture` shape and the existing texture-creation helper inside the renderer (sampler config differs from world textures: nearest-neighbor, clamp-to-edge).
- Splash PNG path: `content/base/textures/splash/postretro.png`. Decoded eagerly on the main thread before the worker is dispatched — splash is part of Phase 0 (engine init), which is main-thread by the concurrency model, and one PNG decode (~ms) is negligible. Removes any phase-internal dependency between worker output and splash visibility.
- Worker channel: `std::sync::mpsc::channel::<LoadOutcome>()`. `App` polls the receiver at the top of each `RedrawRequested`.
- Stage timer log target: `[Startup]` prefix, info level. One line per phase.

Existing `Renderer::new` accepts `Option<&LevelGeometry>` and `Option<&TextureSet>` (see `crates/postretro/src/render/mod.rs:478`), but the `Some` branches inline the GPU uploads. The concurrency model — worker parses, main thread uploads — requires those uploads to be addressable as a separate step. Split them out into `Renderer::install_level_geometry` and `Renderer::install_textures`, called from the `Installing → Running` transition in `App`. `Renderer::new` is then called with `(None, None)` from `resumed()` and never with `Some` payloads.

## Open questions

- **Promoting the concurrency model to `context/lib/boot_sequence.md`.** The five-bullet model is durable. Move it at promotion-time, not now, per the documented draft → ready lifecycle. Flag for the reviewer to confirm.
- **Phase cancellation as a future capability.** Detached worker on close-mid-load is fine for PRL parse + PNG decode. Mid-mod-swap or mid-level-reload cancellation will need a real story — out of scope here, but the model should not preclude it. Worth a sentence in `boot_sequence.md` at promotion.
