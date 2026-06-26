# Early Boot Solo Splash

## Goal

Restore the splash screen as a lean renderer-owned boot feature. Show first pixels at the earliest practical point in process startup, then use that visible splash while the heavier engine subsystems initialize.

This keeps the near-instant boot northstar honest. The splash should no longer depend on the UI system, UI JSON, glyphon, taffy, mod themes, or gameplay UI snapshots.

## Scope

### In scope

- Replace the UI-authored splash path with a small renderer-owned boot splash path.
- Reorder startup so `winit` event loop creation stays before Rust-only handler registries, options, input seeding, and built-in UI JSON registration, and also moves before session-lifetime scripting bootstrap and network endpoint construction.
- Split renderer initialization so the first black frame and logo frame need only window, surface, adapter, device, queue, surface config, and the boot splash pass.
- Add no new non-render wgpu usage. Direct boot-splash GPU work lives under `render/`.
- Preserve the causal boot contract: visible pixels before mod init, level worker spawn, or user-authored script work.
- Add startup timing/log seams that prove the new order.
- Update durable context docs after implementation.

### Out of scope

- Mod-supplied splash overrides.
- Animated splash screens.
- Splash text rendering through glyphon or the UI system.
- A new loading-screen framework.
- Multiple windows or async runtime startup.
- Changing the steady-state frame order for gameplay.

## Acceptance criteria

- [ ] On launch, the first presented frame is black and is recorded before script runtime construction, built-in UI asset loading, network socket construction, audio init, debug UI init, mod init, and level worker dispatch. Splash rendering returns a presented/not-presented outcome; `first_black_frame` is recorded only after command submission and a successful swapchain present path.
- [ ] With a valid built-in splash asset, the second presented boot frame shows the built-in splash logo without using the gameplay/UI pass, UI descriptor JSON, UI image registry, or a `UiReadSnapshot` version line.
- [ ] Missing or malformed base splash assets degrade to black boot frames plus a warning; they do not abort startup. This is the fallback path for the logo-frame criterion.
- [ ] `winit` event loop creation happens after only logging, boot timing setup, and minimal argument parsing needed to choose the boot target. Net role parsing is deferred unless `winit` forces an earlier decision.
- [ ] Window creation and minimal GPU surface/device/queue setup happen before full renderer pipeline/resource setup.
- [ ] With a valid splash asset, mod init, hot-reload watcher startup, frontend presentation, and boot-map loading happen only after at least one visible splash-logo frame. With a missing or malformed splash asset, they happen only after the warning and fallback black boot frame present.
- [ ] This work adds no new non-render imports or storage of wgpu types. Direct boot-splash texture creation, upload, bind groups, pipelines, and draw calls live under `render/`.
- [ ] Full renderer initialization completes before clearing splash and before Frontend, Loading completion, Running, UI pass, or scene render path executes.
- [ ] Launch with no CLI map reaches the frontend after the splash.
- [ ] Launch with a CLI map keeps painting the splash while the level worker runs, then clears it before the first gameplay frame.
- [ ] Platform suspend/resume handles each boot phase without corrupting boot state: black-frame phase re-presents black; logo phase drops GPU splash state and re-presents black then logo; deferred-session phase commits session state once; session-installed/full-renderer-pending phase keeps the session bundle and reruns renderer completion; full-renderer-complete phase resumes through the normal renderer/window rebuild path.
- [ ] Stale-script compile and reload drain do not run before the first visible logo frame and an initialized script runtime. They may run after that point and before mod init/session-dependent work observes script state.
- [ ] `cargo check -p postretro` passes.
- [ ] Targeted verification passes: unit tests cover startup-order/timing seams, malformed splash fallback, deferred-session guard before runtime exists, and suspend/resume boot-state transitions; review/grep gates prove no boot `UiReadSnapshot`/UI splash path and no new non-render wgpu usage; manual engine runs verify black/logo frames and CLI/no-CLI transitions.

## Tasks

### Task 1: Split Boot Construction Out Of `main.rs`

Do a behavior-preserving split before changing startup order. `crates/postretro/src/main.rs` is already far past the source-size smell threshold, and this work will otherwise add more boot code to it. Extract the current pre-`run_app` construction flow from `main()` into a startup-owned module, such as `crates/postretro/src/startup/session.rs`, while keeping the current order and behavior intact. The extracted code owns the current argument parsing, content-root selection, script runtime construction, Rust registries, player options load/save, input seeding, modal-stack built-in registration, network endpoint construction, and `App` construction. `main()` should become orchestration glue: initialize logging/timing, call the startup builder, run the event loop, and return the app exit result.

### Task 2: Split Splash Lifecycle Out Of `startup/lifecycle.rs`

Do a behavior-preserving split before extending the splash state machine. `crates/postretro/src/startup/lifecycle.rs` is also past the source-size smell threshold. Move splash-specific frame driving, splash decode/upload handoff, splash clearing, and boot-map/frontend transition helpers into a focused startup module, such as `crates/postretro/src/startup/splash_lifecycle.rs`. Keep `App::drive_boot_state_for_redraw` as the dispatcher, but make the splash branch call the extracted splash code. This gives the later staged-startup work a narrow place to add post-first-present initialization.

### Task 3: Restore A Direct Renderer Boot Splash

Replace the current UI splash renderer path with a dedicated renderer-owned splash path. Current splash install/render lives in `crates/postretro/src/render/renderer_splash.rs`; CPU decode and texture upload helpers live in `crates/postretro/src/render/splash.rs`; UI-authored splash descriptor code lives in `crates/postretro/src/render/ui/splash.rs`. Add a small boot splash pass under `render/` that clears the swapchain and draws the decoded logo as a textured quad. The built-in logo is `content/base/textures/splash/postretro-ascii-art.png`. Runtime resolution follows the cwd-relative content asset convention; tests may use `CARGO_MANIFEST_DIR` only under `cfg(test)`. The pass owns its pipeline, bind group layout, sampler, uploaded logo texture, and sizing math. It does not call `UiPass`, `UiImageRegistry`, `UiReadSnapshot`, `render::ui::splash`, or read `content/base/ui/splash.json`.

The app-facing renderer API should stay small: install decoded splash pixels, render a black/logo splash frame, clear splash resources. Splash render returns a `PresentOutcome`-style result or boolean so startup records `first_black_frame` / `first_splash_frame` only after a submitted frame presents; non-present surface outcomes request another redraw without advancing the splash state. `App::paint_splash` should stop publishing a UI snapshot and stop querying splash capture mode. The input-dispatch seam can remain passthrough during boot without asking the renderer. Remove `render/ui/splash.rs` and its dead tests (the inline `#[test]`s inside `render/ui/splash.rs` and the two sibling files `render/ui/splash_layout_test.rs` and `render/ui/splash_golden_test.rs`, all of which `use super::splash::…` and exist only to exercise the UI-authored descriptor path), plus the boot-only UI splash helper call sites — or prove they are no longer reachable from boot. The asset `content/base/ui/splash.json` was already deleted when this spec was promoted, so only the now-dangling loader and tests that still reference it remain to remove; one inline test (`splash_json_carries_logo_asset_version_sentinel_and_envelope`) currently `panic!`s under `cargo test` because that JSON is gone, and removing the UI splash path resolves it. `UiReadSnapshot` remains for gameplay/frontend UI only.

### Task 4: Defer Session-Lifetime Heavy Startup Until After First Pixels

Change startup from eager full app construction to staged boot construction. Keep the already-current ordering where `EventLoop::new` precedes Rust-only handler registries, options disk I/O, input preference seeding, and built-in UI JSON registration. Move it ahead of `NetEndpoint::from_role` and session-lifetime scripting bootstrap, including the script primitive registry. The minimal pre-event-loop work should be logging, boot timing setup, raw argument collection, and enough argument parsing to identify the content root and optional boot map. After `EventLoop::new`, construct minimal app state that can enter `ApplicationHandler::resumed`. Do not parse net role before the event loop unless `winit` or platform setup explicitly requires it; prefer deferring that parse into the deferred-startup owner.

Introduce an explicit deferred-startup owner, such as a `PendingSessionInit` or similar startup module type. It should carry the raw inputs needed to build the current session-lifetime systems after first pixels. This task introduces the boundary; first-pixel ordering is satisfied only once Task 5 splits renderer initialization. The owner installs one session bundle, such as `SessionServices`, rather than scattering `Option` checks through gameplay paths. That bundle owns the current session-dependent `App` field groups: `ScriptCtx`, `ScriptRuntime`, script primitive registry outputs, Rust-only handler registries, `PlayerOptions` and settings path, `InputSystem` and gameplay latches, HUD publisher, flash/vignette/shake/input-mode systems, presentation cells, modal stack and built-in UI registrations, frontend declaration state, UI focus state, and net endpoint state. Boot paths before session install may use only pending inputs plus boot window/renderer state.

Pre-session event handling must pass through one boundary: allow close, suspend/resume, resize needed for the boot surface, and redraw needed for boot splash; ignore or defer gameplay/UI input until `SessionServices` is installed. The current unconditional `script_runtime.drain_reload_requests()` call in the redraw path must be guarded or moved so it runs only after the script runtime exists and after the first visible logo frame. `PendingSessionInit` parses raw net args after first pixels, logs parse/setup failures with the existing single-player fallback behavior, and records `net_endpoint_complete` when endpoint setup has either succeeded or degraded.

### Task 5: Split Renderer Initialization Into Boot And Full Phases

Refactor `Renderer::new` so boot rendering can happen after only the required wgpu setup. The first phase creates the instance, surface, adapter, device, queue, surface configuration, and direct splash pass. Device creation still requests the features and limits required by eventual full renderer initialization; wgpu features cannot be added after the device exists. The second phase finishes the existing renderer setup: world placeholder buffers, lighting resources, shadow pools, screen effects, mesh pass, UI pass, fog pass, debug lines, and other steady-state resources.

Keep the renderer as the only GPU owner. The app may ask the renderer to finish initialization after the first black or logo frame, but the app must not receive raw wgpu handles. Replace the current single readiness meaning with explicit boot-ready vs full-ready state/API: splash paths require boot-ready; Frontend, Loading completion, Running, UI pass, and scene render paths require full-ready. Resize, surface loss, and platform suspend must work in both phases. Adapter fail-fast checks that protect hard renderer requirements should run before gameplay begins; checks required by the boot splash itself must run before the first splash draw. Full initialization must be idempotent/restartable across surface recreation using phase state for boot renderer ready, logo installed, session installed, full renderer pending, and full renderer complete.

### Task 6: Move Audio, Debug UI, And Network Endpoint Startup Behind Splash

Move fault-tolerant audio initialization and dev debug UI creation out of `App::resumed`'s pre-redraw path. Build them after the first visible logo frame, or after the fallback black frame when the splash asset is missing or malformed. Parse net role and construct `NetEndpoint` only after that point, preferably inside the deferred-startup owner. Preserve the current fallback behavior: audio failures run silent, net endpoint failures degrade to single-player, and the engine continues booting.

### Task 7: Verification And Context Updates

Add focused tests and startup diagnostics for the new order. Keep CPU tests near the logic they verify. Runnable tests should cover pure timing/order seams, malformed splash fallback, deferred-session guard before runtime exists, and suspend/resume state transitions. Negative-existence claims are review/grep gates: no boot `UiReadSnapshot` / UI splash path and no new non-render wgpu usage. GPU logo visibility remains manual/run-engine verification per the testing guide. After code and tests for this plan pass locally, update `context/lib/boot_sequence.md`, `context/lib/rendering_pipeline.md`, and `context/lib/ui.md` to describe the new boot-splash contract and remove the JSON-authored splash exception.

## Sequencing

**Phase 1 (sequential):** Task 1, then Task 2 — split oversized files before extending boot and splash behavior.

**Phase 2 (sequential):** Task 3 — restores the splash as a lean renderer-owned feature before boot sequencing depends on it.

**Phase 3 (sequential):** Task 4, then Task 5 — Task 4 creates the deferred session boundary; Task 5 makes first-pixel ordering real by splitting renderer boot from full renderer setup.

**Phase 4 (concurrent):** Task 6 and Task 7 — post-splash service moves and verification/docs can proceed after staged boot is in place.

## Rough Sketch

Current startup does meaningful work before the window and first present. In `main()`, the source already creates `EventLoop` before Rust-only handler registries, options load/save, `InputSystem::new`, and built-in UI registration through `render::ui::tree_asset::register_tree_from_disk`. `ScriptCtx::new`, `PrimitiveRegistry::new`, `register_all`, `ScriptRuntime::new`, `emit_sdk_types_in_debug`, and `NetEndpoint::from_role` still run before first pixels.

Current `ApplicationHandler::resumed` creates the window, calls `Renderer::new`, initializes audio, initializes dev debug UI, then enters splash and requests redraw. `Renderer::new` already creates the wgpu instance, surface, adapter, device, queue, and surface config early, but then continues through full steady-state renderer setup before any pixels are presented.

The intended new shape is:

1. `main()` initializes logging and `StartupTimings`.
2. `main()` collects args and performs minimal parsing for map path and content root. Net role stays raw and is parsed by deferred startup unless explicitly required earlier.
3. `main()` creates `EventLoop` before `NetEndpoint::from_role` and scripting bootstrap, while preserving the already-current later ordering for Rust-only handler registries, options, input seeding, and built-in UI JSON registration.
4. `main()` builds a minimal `App` in `BootState::Booting`, with deferred startup inputs stored in a startup-owned pending object.
5. `App::resumed` creates the window and asks the renderer for boot-phase GPU setup.
6. Splash frame 0 presents black immediately.
7. After frame 0, the app CPU-decodes or requests decode of the base splash, then passes decoded pixels to the renderer. Texture creation and upload remain inside `render/`. No other deferred session/full-renderer work runs between black and the first logo attempt.
8. Splash frame 1 presents the logo through the direct renderer splash pass.
9. After the logo frame, deferred session startup completes. If the splash asset is missing or malformed, deferred session startup runs after the warning and fallback black boot frame present. Stale-script compile and reload drain may run only after the logo is visible, or after the fallback black boot frame, and script runtime exists.
10. Full renderer initialization completes before splash clears and before any Frontend, Loading completion, Running, UI pass, or scene render path executes.
11. The boot flow transitions to Frontend or Loading as it does today.

Direct splash rendering can reuse the existing splash decode shape, but app/startup passes decoded pixels to the renderer rather than receiving GPU handles. The uploaded texture should be stored in splash-owned renderer state rather than registered as a UI image. The old `render/ui/splash.rs` path is unused by boot once the direct splash pass lands (its `content/base/ui/splash.json` asset was already deleted at promotion) and should be removed along with its tests once nothing else references them.

For rendering the logo, use a fixed logical design size or derive a max rectangle from the window dimensions while preserving source aspect ratio. The splash pass only needs a fullscreen clear plus one textured quad. Version text is intentionally removed from scope; it was a UI-system affordance, not a startup requirement.

Startup diagnostics should keep the existing `StartupTimings` style and add stage names that make ordering auditable. Useful stages include `args_parsed`, `event_loop_created`, `window_created`, `boot_wgpu_ready`, `first_black_frame`, `splash_decoded`, `splash_uploaded`, `first_splash_frame`, `session_init_complete`, `renderer_full_init_complete`, `audio_init_complete`, and `net_endpoint_complete`. Mark `first_black_frame` and `first_splash_frame` only after command submission and a successful present path.

`App` currently stores session-lifetime fields directly, including `script_runtime`, `script_ctx`, registries, `player_options`, `input_system`, `modal_stack`, `frontend`, UI focus state, and network endpoint state. The staged design must either make these unavailable until session init completes, or group them into an initialized service bundle that boot code installs before any path can use them. Avoid scattering many unrelated `Option` checks across gameplay paths; prefer one explicit boot/session boundary that proves initialization before leaving the splash phase.

Suspend/resume recovery:

- During black frame: drop window/boot renderer state. Keep pending session inputs. Re-present black, then continue. No deferred session init has run.
- During logo frame: drop window/boot renderer state and decoded/uploaded splash GPU state. Keep pending session inputs and any CPU-decoded splash if convenient. Re-present black, then logo. Prevent duplicate deferred init by leaving pending state unconsumed until install succeeds.
- During deferred session init: drop window/renderer state. Keep pending session owner if init has not committed; keep installed session bundle if commit completed. Re-present black/logo as needed. Guard commit with a consumed flag or state transition.
- After session init, before full renderer init: drop window/renderer state. Keep session bundle. Re-present black/logo before resuming full renderer init. Do not rerun deferred session init.

## Decisions

- Deferred session startup runs after the first visible logo frame. If the splash asset is missing or malformed, it runs after the warning and fallback black boot frame present. Between black and the first logo attempt, do only bounded splash decode/upload work.
- Full renderer initialization runs after the first visible logo frame or fallback black boot frame, then completes before splash clears and before any Frontend, Loading completion, Running, UI pass, or scene render path executes.
- `content/base/ui/splash.json` was already removed when this spec was promoted (it is absent on `main` and on the runtime-cell-spatial-contract merge), so there is no file left to delete. The runtime loader in `render/ui/splash.rs` degrades to an in-code fallback tree, so boot still works, but the inline test that reads the JSON directly (`splash_json_carries_logo_asset_version_sentinel_and_envelope`) now fails under `cargo test` until Task 3 removes the UI splash path and its tests. The target architecture does not use a JSON-authored boot splash.
