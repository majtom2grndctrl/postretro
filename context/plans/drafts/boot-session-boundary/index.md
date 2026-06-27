# Boot/Session Boundary

## Goal

Make the boot/session boundary a type, not a convention. Extract a `Session` container that owns every session-lifetime field, held on `App` as `Option<Session>`, built entirely after the first visible frame. Boot-phase code then physically cannot name a session field, and the script runtime — the heaviest startup init — moves behind first pixels.

This completes the boundary work `early-boot-solo-splash` deliberately deferred. That plan staged boot construction with a `PendingSessionInit`/`SessionServices` seam but kept ~40 session-lifetime fields living directly on the `App` god-struct, guarded only by control-flow convention. Two costs followed: the boundary is unenforced (a future boot path can read an uninitialized session field with no compile error), and the script runtime (QuickJS + Luau VMs, primitive registry, SDK typedef emit) is still constructed pre-window, which forced `early-boot-solo-splash` AC #1 to be weakened.

## Scope

### In scope

- Introduce a `Session` type owning the session-lifetime fields; store it as `App.session: Option<Session>`.
- Build the entire `Session` in the existing post-first-pixel install path; absorb the current `SessionServices` build and the `install_post_splash_services` / `install_pending_session` steps into it.
- Move `ScriptCtx`/`ScriptRuntime`/registry/SDK-typedef construction from pre-window into the post-first-pixel `Session` build. Restore `early-boot-solo-splash` AC #1 in full.
- Define post-first-pixel session-build failure behavior (fallible `ScriptRuntime::new` can no longer bail before the window exists).
- Convert gameplay/frontend handlers to a single session destructure at entry.
- Re-site `impl App` methods that touch both session and boot state.
- Behavior-preserving throughout. Update durable docs.

### Out of scope

- A full typestate `enum App { Booting(BootApp), Running(SessionApp) }`. Considered and rejected: it fights winit's `ApplicationHandler` single-`&mut self` event API and over-models a single-window engine. `Option<Session>` gives the same compile-time guarantee with far less churn. Revisit only if the implementer finds `Option<Session>` insufficient.
- Changing the steady-state gameplay frame order (Input → Game logic → Audio → Render → Present).
- The renderer boot/full split (`Renderer::new` → `finish_full_init`). Already shipped and correct.
- A wholesale `main.rs` module split. This refactor moves session code into a `session` module and shrinks `main.rs` as a side effect; it does not chase a line target.
- Mod-supplied splash overrides, animated splash, or any `early-boot-solo-splash` non-goal.

## Acceptance criteria

- [ ] Session-lifetime state is accessible only through `App.session: Option<Session>`. No session field is a direct `App` field. Verifiable by grep + the type checker: boot-phase code cannot name a session field.
- [ ] The first presented black frame is recorded before script runtime construction. A startup-order test asserts `first_black_frame` precedes the session-build / script-runtime mark. (This restores the original `early-boot-solo-splash` AC #1.)
- [ ] The entire `Session` is constructed in the post-first-pixel install path, not before the window. Net endpoint, audio, debug UI, and script runtime all build there.
- [ ] Session build failure after first pixels logs the error and exits cleanly via the event loop with a non-zero result; it never leaves a half-installed `Session`. No path observes `App.session` as partially built.
- [ ] Each `ApplicationHandler` event method and frame handler obtains session state through one destructure at entry (`let Some(session) = self.session.as_mut() else { … }`); no co-borrow expression calls a `self.session_mut()`-style accessor repeatedly. Review/grep gate.
- [ ] Suspend/resume still commits session install exactly once; resume re-presents black/logo and reruns renderer completion without rebuilding an already-installed `Session`.
- [ ] Behavior preserved: full `cargo test` workspace suite green; manual boot with a CLI map and with no map reach the same states as before; no new `unsafe`; renderer remains the sole GPU owner.

## Tasks

### Task 1: Session container, install path, and the access discipline

Define `Session` (owning, initially holding the field group migrated in this task) and add `App.session: Option<Session>`. Expand `PendingSessionInit` to carry every raw input the full session build needs (content root, raw argv, settings inputs), not just `raw_args`. Add `Session::build(/* pending inputs */) -> Result<Session>` and install it from `run_splash_frame_one`, replacing the net-only `install_pending_session` and folding in `install_post_splash_services`. Define the post-first-pixel build-failure path: on `Err`, store the error in `exit_result`, log it, and `event_loop.exit()` — mirroring `finish_renderer_full_init`. Establish the destructure-at-entry pattern in every `ApplicationHandler` method and the redraw/frame handlers. Migrate the first, smallest field group to prove the pattern end to end: the input/UI/modal group (`input_system`, `gameplay_input_latch`, `ui_dispatch`, `gamepad_system`, `ui_focus`, `ui_focus_rects`, `ui_input_mode`, `input_focus`, `modal_stack`). Boot-phase pre-session event handling keeps passing through one boundary (close, suspend/resume, boot-surface resize, splash redraw), ignoring gameplay/UI input until `Session` is installed.

### Task 2: Migrate the scripting core (restores AC #1)

Move `script_ctx`, `script_runtime`, the four registries (`sequence_registry`, `reaction_registry`, `system_registry`, `classname_dispatch`), and every system that holds a cloned `ScriptCtx` or a registry reference into `Session`: `player_hud_state`, `flash_decay`, `vignette_decay`, `shake_decay`, `input_mode_tracker`, the bridges (`light_bridge`, `fog_volume_bridge`, `emitter_bridge`), the collectors (`particle_render`, `mesh_render`, `mesh_clip_tables`, `hit_zone_store`), `presentation_cells`, `crossing_detector`, `progress_tracker`, and `state_store_lifecycle`. `ScriptCtx` is cloned into eight subsystems at construction, so they form one indivisible tranche. This moves `ScriptCtx::new` / `register_all` / `ScriptRuntime::new` / `emit_sdk_types_in_debug` from the pre-window `build_session` into `Session::build` (post-first-pixel) — the change that restores AC #1. Re-site the `impl App` methods that read both session and boot state (e.g. `net_poll_and_apply`, `dispatch_system_commands`, the redraw block's bridge/collector calls): pass the needed pieces as parameters, or move the method onto `Session`. This is the largest tranche (~65 `script_ctx` sites).

### Task 3: Migrate remaining session state; retire `SessionServices`

Move the last session fields into `Session`: `player_options`, `settings_path`, the frontend declaration state (`frontend` and related UI-focus fields), `net_endpoint`, `audio`, and the `dev-tools` `debug_ui` (several already `Option` — fold them in, dropping their now-redundant per-field `Option`). Remove the now-empty `SessionServices` (its role is wholly subsumed by `Session`). After this task, `App` holds only boot-lifetime fields plus `session: Option<Session>`.

### Task 4: Verify and document

Add startup-order/timing tests asserting `first_black_frame` precedes the session/script-runtime build mark, and a test for the post-first-pixel build-failure exit path. Add the grep gates: no session field on `App`, no repeated `session_mut()` in co-borrow expressions. Run the full workspace suite. Update `context/lib/boot_sequence.md` to describe the type-enforced boundary and the post-first-pixel session build; close out the `early-boot-solo-splash` AC #1 caveat (the script-runtime-pre-window note) now that it no longer holds.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes `Session`, the install path, the failure mode, and the access discipline; everything else builds on it.
**Phase 2 (sequential):** Task 2 — consumes Task 1's container; shares `main.rs` heavily, so it cannot overlap Task 1 or Task 3.
**Phase 3 (sequential):** Task 3 — consumes Task 2's container; finishes the migration and removes `SessionServices`.
**Phase 4 (sequential):** Task 4 — verifies and documents the completed boundary.

All phases are sequential: every task moves a field group through the same `main.rs` handlers and the shared `Session` container, so concurrent edits would collide. Each task must leave the tree compiling and the focused suite green — an intermediate `Session` is partially populated (migrated groups live in `Session`, not-yet-migrated groups stay on `App`), which is sound because the install builds whichever fields `Session` currently owns, whole, at once.

## Rough sketch

`Session` lives in its own module (expand `startup/session.rs`, or a new `src/session/`). It is the live runtime container, not a transient construction bundle — the opposite of today's `SessionServices`, which `build_session` destructures into the `App` literal and discards.

Install flow (post-first-pixel, in `run_splash_frame_one`, after the logo/fallback-black frame presents):

```
PendingSessionInit::install(self, app)         // already runs here
  └─ Session::build(pending_inputs) -> Result<Session>
       └─ on Ok:  app.session = Some(session)
       └─ on Err: app.exit_result = Err(e); log; event_loop.exit()
```

Access discipline — one destructure at handler entry, then disjoint field borrows:

```rust
// Proposed pattern
let Some(session) = self.session.as_mut() else { return };
let Some(renderer) = self.renderer.as_mut() else { return };
// session.* and renderer/camera/window_state borrow disjoint App fields
let mut reg = session.script_ctx.registry.borrow_mut();
session.emitter_bridge.update(&mut reg, dt, …);
renderer.upload_bridge_lights(&bytes);
```

The borrow-checker cost is bounded because the borrow of `self.session` and the borrow of `self.renderer` are disjoint field paths. The repeated-`self.session_mut()`-inside-an-expression anti-pattern is what produces borrow conflicts; the spec forbids it. Most call sites become a mechanical `self.X` → `session.X` rename. The real work is the handful of `&mut self` methods that touch both groups (Task 2): give them the pieces as parameters or move them onto `Session`.

Why post-first-pixel build is behavior-preserving: session systems are unused during the boot splash, so constructing them after the first frame changes startup *timing*, not observable behavior — and strictly improves boot latency by moving script-VM construction off the critical path to first pixels.

## Decisions

- **`App.session: Option<Session>`, not a phase enum.** The codebase already models boot phase once (`BootState`) and already uses `Option<T>` for a not-yet-built subsystem in `pending_session` and `renderer.full`. A second `BootPhase` enum would be a third phase representation that can desync from `BootState`; the single install path already guarantees the only invariant the enum would add. Reuse the established idiom.
- **Session-build failure logs and exits; no painted error frame.** A legible error frame needs text, and the boot splash is deliberately text-free (`early-boot-solo-splash` removed boot-path text rendering on purpose). The failure — script-runtime construction failing — is developer/modder-facing; its channel is the log + non-zero exit, matching the engine's init-failure convention (`development_guide.md` §6.2, `finish_renderer_full_init`). The boot splash paints a black frame and the process exits.

Both follow the same rule, which the implementer should apply to any fork this spec didn't foresee: prefer the leaner option that reuses an idiom already in the tree over a new representation, absent a measured reason otherwise.
