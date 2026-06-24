# Early Boot Solo Splash

## Goal

Restore the splash screen as a lean renderer-owned boot feature. Show first pixels at the earliest practical point in process startup, then use that visible splash while the heavier engine subsystems initialize.

This keeps the near-instant boot northstar honest. The splash should no longer depend on the UI system, UI JSON, glyphon, taffy, mod themes, or gameplay UI snapshots.

## Scope

### In scope

- Replace the UI-authored splash path with a small renderer-owned boot splash path.
- Reorder startup so `winit` event loop creation and window creation happen before session-lifetime scripting, registry, options, network endpoint, UI asset, audio, and debug UI initialization.
- Split renderer initialization so the first black frame and logo frame need only window, surface, adapter, device, queue, surface config, and the boot splash pass.
- Keep all wgpu calls inside `render/`.
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

- [ ] On launch, the first presented frame is black and is recorded before script runtime construction, built-in UI asset loading, network socket construction, audio init, debug UI init, mod init, and level worker dispatch.
- [ ] The second presented boot frame shows the built-in splash logo without using the gameplay/UI pass, UI descriptor JSON, UI image registry, or a `UiReadSnapshot` version line.
- [ ] `winit` event loop creation happens after only logging, boot timing setup, and minimal argument parsing needed to choose the boot target.
- [ ] Window creation and minimal GPU surface/device/queue setup happen before full renderer pipeline/resource setup.
- [ ] Mod init, hot-reload watcher startup, frontend presentation, and boot-map loading still happen only after at least one visible splash-logo frame.
- [ ] No module outside `render/` imports or stores wgpu types.
- [ ] Launch with no CLI map reaches the frontend after the splash.
- [ ] Launch with a CLI map keeps painting the splash while the level worker runs, then clears it before the first gameplay frame.
- [ ] Missing or malformed base splash assets degrade to black boot frames plus a warning; they do not abort startup.
- [ ] Platform suspend/resume still drops and recreates renderer/window state without corrupting boot state.
- [ ] `cargo check -p postretro` passes.
- [ ] Targeted splash/startup tests pass.

## Tasks

### Task 1: Split Boot Construction Out Of `main.rs`

Do a behavior-preserving split before changing startup order. `crates/postretro/src/main.rs` is already far past the source-size smell threshold, and this work will otherwise add more boot code to it. Extract the current pre-`run_app` construction flow from `main()` into a startup-owned module, such as `crates/postretro/src/startup/session.rs`, while keeping the current order and behavior intact. The extracted code owns the current argument parsing, content-root selection, script runtime construction, Rust registries, player options load/save, input seeding, modal-stack built-in registration, network endpoint construction, and `App` construction. `main()` should become orchestration glue: initialize logging/timing, call the startup builder, run the event loop, and return the app exit result.

### Task 2: Split Splash Lifecycle Out Of `startup/lifecycle.rs`

Do a behavior-preserving split before extending the splash state machine. `crates/postretro/src/startup/lifecycle.rs` is also past the source-size smell threshold. Move splash-specific frame driving, splash decode/upload handoff, splash clearing, and boot-map/frontend transition helpers into a focused startup module, such as `crates/postretro/src/startup/splash_lifecycle.rs`. Keep `App::drive_boot_state_for_redraw` as the dispatcher, but make the splash branch call the extracted splash code. This gives the later staged-startup work a narrow place to add post-first-present initialization.

### Task 3: Restore A Direct Renderer Boot Splash

Replace the current UI splash renderer path with a dedicated renderer-owned splash path. Current splash install/render lives in `crates/postretro/src/render/renderer_splash.rs`; CPU decode and texture upload helpers live in `crates/postretro/src/render/splash.rs`; UI-authored splash descriptor code lives in `crates/postretro/src/render/ui/splash.rs`. Add a small boot splash pass under `render/` that clears the swapchain and draws the decoded logo as a textured quad. The pass owns its pipeline, bind group layout, sampler, uploaded logo texture, and sizing math. It does not call `UiPass`, `UiImageRegistry`, `UiReadSnapshot`, `render::ui::splash`, or read `content/base/ui/splash.json`.

The app-facing renderer API should stay small: install decoded splash pixels, render a black/logo splash frame, clear splash resources. `App::paint_splash` should stop publishing a UI snapshot and stop querying splash capture mode. The input-dispatch seam can remain passthrough during boot without asking the renderer.

### Task 4: Defer Session-Lifetime Heavy Startup Until After First Pixels

Change startup from eager full app construction to staged boot construction. Move `EventLoop::new` ahead of script runtime construction, Rust registry construction, options disk I/O, input preference seeding, built-in UI JSON registration, and network endpoint construction. The minimal pre-event-loop work should be logging, boot timing setup, raw argument collection, enough argument parsing to identify the content root and optional boot map, and construction of a minimal app state that can enter `ApplicationHandler::resumed`.

Introduce an explicit deferred-startup owner, such as a `PendingSessionInit` or similar startup module type. It should carry the raw inputs needed to build the current session-lifetime systems after first pixels. It must write initialized state into `App` before mod init, level install, frontend UI logic, or hot reload can observe those systems. The current unconditional `script_runtime.drain_reload_requests()` call in the redraw path must be guarded or moved so it runs only after the script runtime exists.

### Task 5: Split Renderer Initialization Into Boot And Full Phases

Refactor `Renderer::new` so boot rendering can happen after only the required wgpu setup. The first phase creates the instance, surface, adapter, device, queue, surface configuration, and direct splash pass. The second phase finishes the existing renderer setup: world placeholder buffers, lighting resources, shadow pools, screen effects, mesh pass, UI pass, fog pass, debug lines, and other steady-state resources.

Keep the renderer as the only GPU owner. The app may ask the renderer to finish initialization after the first black or logo frame, but the app must not receive raw wgpu handles. Resize, surface loss, and platform suspend must work in both phases. Adapter fail-fast checks that protect hard renderer requirements should run before gameplay begins; checks required by the boot splash itself must run before the first splash draw.

### Task 6: Move Audio, Debug UI, And Network Endpoint Startup Behind Splash

Move fault-tolerant audio initialization and dev debug UI creation out of `App::resumed`'s pre-redraw path. Build them after the first black frame or after the first logo frame, whichever keeps the implementation simplest while satisfying the acceptance criteria. Construct `NetEndpoint` from the parsed `NetRole` only after first pixels. Preserve the current fallback behavior: audio failures run silent, net endpoint failures degrade to single-player, and the engine continues booting.

### Task 7: Verification And Context Updates

Add focused tests and startup diagnostics for the new order. Keep CPU tests near the logic they verify. GPU behavior is verified by running the engine, per the testing guide. After the implementation is accepted, update `context/lib/boot_sequence.md`, `context/lib/rendering_pipeline.md`, and `context/lib/ui.md` to describe the new boot-splash contract and remove the JSON-authored splash exception.

## Sequencing

**Phase 1 (sequential):** Task 1, then Task 2 — split oversized files before extending boot and splash behavior.

**Phase 2 (sequential):** Task 3 — restores the splash as a lean renderer-owned feature before boot sequencing depends on it.

**Phase 3 (sequential):** Task 4, then Task 5 — staged app boot creates the need for staged renderer boot; renderer staging consumes the app boot boundary.

**Phase 4 (concurrent):** Task 6 and Task 7 — post-splash service moves and verification/docs can proceed after staged boot is in place.

## Rough Sketch

Current startup does meaningful work before the event loop and window. In `main()`, `ScriptCtx::new`, `PrimitiveRegistry::new`, `register_all`, `ScriptRuntime::new`, `emit_sdk_types_in_debug`, `SequencedPrimitiveRegistry::new`, `ReactionPrimitiveRegistry::new`, `SystemReactionRegistry::new`, `ClassnameDispatch::new`, options load/save, `InputSystem::new`, built-in UI registration through `render::ui::tree_asset::register_tree_from_disk`, and `NetEndpoint::from_role` all run before the app enters `run_app`.

Current `ApplicationHandler::resumed` creates the window, calls `Renderer::new`, initializes audio, initializes dev debug UI, then enters splash and requests redraw. `Renderer::new` already creates the wgpu instance, surface, adapter, device, queue, and surface config early, but then continues through full steady-state renderer setup before any pixels are presented.

The intended new shape is:

1. `main()` initializes logging and `StartupTimings`.
2. `main()` collects args and performs minimal parsing for map path, content root, and net role.
3. `main()` creates `EventLoop`.
4. `main()` builds a minimal `App` in `BootState::Booting`, with deferred startup inputs stored in a startup-owned pending object.
5. `App::resumed` creates the window and asks the renderer for boot-phase GPU setup.
6. Splash frame 0 presents black immediately.
7. After frame 0, the app decodes/uploads the base splash and may run deferred session startup.
8. Splash frame 1 presents the logo through the direct renderer splash pass.
9. After the logo frame, remaining full renderer/session services complete, then mod init runs.
10. The boot flow transitions to Frontend or Loading as it does today.

Direct splash rendering can reuse `load_splash` and `upload_splash_texture` from `crates/postretro/src/render/splash.rs`, but the uploaded texture should be stored in splash-owned renderer state rather than registered as a UI image. The old `render/ui/splash.rs` path and `content/base/ui/splash.json` become unused by boot and should be removed if no tests or references remain.

For rendering the logo, use a fixed logical design size or derive a max rectangle from the window dimensions while preserving source aspect ratio. The splash pass only needs a fullscreen clear plus one textured quad. Version text is intentionally removed from scope; it was a UI-system affordance, not a startup requirement.

Startup diagnostics should keep the existing `StartupTimings` style and add stage names that make ordering auditable. Useful stages include `args_parsed`, `event_loop_created`, `window_created`, `boot_wgpu_ready`, `first_black_frame`, `splash_decoded`, `splash_uploaded`, `first_splash_frame`, `session_init_complete`, `renderer_full_init_complete`, `audio_init_complete`, and `net_endpoint_complete`.

`App` currently stores session-lifetime fields directly, including `script_runtime`, `script_ctx`, registries, `player_options`, `input_system`, `modal_stack`, `frontend`, UI focus state, and network endpoint state. The staged design must either make these unavailable until session init completes, or group them into an initialized service bundle that boot code installs before any path can use them. Avoid scattering many unrelated `Option` checks across gameplay paths; prefer one explicit boot/session boundary that proves initialization before leaving the splash phase.

## Open questions

- Whether deferred session startup should run after the black frame or after the logo frame. The fastest user-visible logo favors decoding/uploading first, then session startup. The simplest staged implementation may run some deferred CPU work after black and before logo, as long as the logo still precedes mod init and level work.
- Whether full renderer initialization should complete before or after the first logo frame. The plan permits either, but startup timing should show which choice shipped.
- Whether the old `content/base/ui/splash.json` should be deleted in the implementation diff or left temporarily for rollback. The target architecture does not use it.
