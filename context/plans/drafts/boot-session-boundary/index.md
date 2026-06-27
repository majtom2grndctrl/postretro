# Boot/Session Boundary

## Goal

Make the boot/session boundary a type, not a convention. Extract a `Session` container that owns every session-lifetime field, held on `App` as `Option<Session>`, built entirely after the first visible frame. Boot-phase code then physically cannot name a session field, and the script runtime — the heaviest startup init — moves behind first pixels.

This completes the boundary work `early-boot-solo-splash` deliberately deferred. That plan staged boot construction with a `PendingSessionInit`/`SessionServices` seam but kept ~40 session-lifetime fields living directly on the `App` god-struct, guarded only by control-flow convention. Two costs followed: the boundary is unenforced (a future boot path can read an uninitialized session field with no compile error), and the script runtime (QuickJS + Luau VMs, primitive registry, SDK typedef emit) is still constructed pre-window. `early-boot-solo-splash` accepted the second cost on purpose: its amended AC #1 names the script runtime "the one session-lifetime system constructed pre-window" and records `first_black_frame` *after* `script_runtime_ctor`. This spec **reverses** that deliberate decision — it does not restore a weakened state.

This plan runs after `early-boot-solo-splash` ships and moves to `done/`. Per the documentation lifecycle, a `done/` spec is frozen historical record (it may describe stale state), so this spec supersedes the predecessor's AC #1 in the live docs (`boot_sequence.md`) and tests — it does not edit the archived predecessor.

## Scope

### In scope

- Introduce a `Session` type owning the session-lifetime fields; store it as `App.session: Option<Session>`.
- Build the entire `Session` in the existing post-first-pixel install path; absorb the current `SessionServices` build and the `install_post_splash_services` / `install_pending_session` steps into it.
- Move `ScriptCtx`/`ScriptRuntime`/registry/SDK-typedef construction from pre-window into the post-first-pixel `Session` build. This reverses `early-boot-solo-splash`'s pre-window-script decision so `first_black_frame` precedes `script_runtime_ctor`.
- Define post-first-pixel session-build failure behavior (fallible `ScriptRuntime::new` can no longer bail before the window exists).
- Convert gameplay/frontend handlers to a single session destructure at entry.
- Re-site `impl App` methods that touch both session and boot state.
- Steady-state observable behavior preserved; boot timing/order changes by design (script runtime now constructed after first pixels). Update durable docs, including the predecessor's now-superseded boot-order tests and `boot_sequence.md` claims.

### Out of scope

- Any phase-enum representation of session presence. Two alternatives to `Option<Session>` are rejected: a full typestate `enum App { Booting(BootApp), Running(SessionApp) }` (fights winit's `ApplicationHandler` single-`&mut self` event API, over-models a single-window engine), and a smaller `enum BootPhase { Booting, Session(Session) }` on `App` (a third phase representation that can desync from the existing `BootState`). `Option<Session>` gives the same compile-time guarantee, reuses the idiom already in the tree for `pending_session` and `renderer.full`, and keeps a single source of truth for phase. Revisit only if `Option<Session>` proves ergonomically insufficient.
- A painted boot-path error indicator for session-build failure. The boot splash is deliberately text-free, so no legible error could be drawn; a script-runtime construction failure is developer/modder-facing and belongs in the log + non-zero exit (`development_guide.md` §6.2), not on screen.
- Changing the steady-state gameplay frame order (Input → Game logic → Audio → Render → Present).
- The renderer boot/full split (`Renderer::new` → `finish_full_init`). Already shipped and correct.
- A wholesale `main.rs` module split. This refactor moves session code into a `session` module and shrinks `main.rs` as a side effect; it does not chase a line target.
- Mod-supplied splash overrides, animated splash, or any `early-boot-solo-splash` non-goal.

## Acceptance criteria

- [ ] Session-lifetime state is accessible only through `App.session: Option<Session>`. No session field is a direct `App` field. Verifiable by grep + the type checker: boot-phase code cannot name a session field.
- [ ] The first presented black frame is recorded before script runtime construction. A startup-order test asserts `first_black_frame` precedes `script_runtime_ctor`. This **supersedes** `early-boot-solo-splash` AC #1, which deliberately ordered them the other way; the predecessor's order test is inverted, not merely supplemented.
- [ ] The entire `Session` is constructed in the post-first-pixel install path, not before the window. Net endpoint, audio, and debug UI already build post-first-pixel (predecessor) — this AC newly requires they build *inside `Session`*; the script runtime additionally moves to post-first-pixel for the first time. The audio/net/debug-UI marks' order relative to `first_black_frame` is unchanged.
- [ ] Session build failure after first pixels logs the error, stores it in `exit_result`, requests `event_loop.exit()`, and the install frame early-returns so no later boot step runs against a `None` session. `Session::build` returns whole-or-nothing, so `App.session` is never observed partially built.
- [ ] Each `ApplicationHandler` event method and frame handler obtains session state through at most one `self.session.as_mut()` (or `as_ref()`) destructure at entry. No `session_mut()`-style accessor is introduced. Review gate (grep target: zero `session_mut(` tokens; no handler body with multiple `self.session.as_` destructures).
- [ ] Suspend/resume commits session install exactly once: the install consumes `pending_session` via `take_once`, so a resumed boot finds it `None` and does not rebuild an already-installed `Session`; resume re-presents black/logo and reruns renderer completion only.
- [ ] Steady-state behavior preserved: full `cargo test` workspace suite green; manual boot with a CLI map and with no map reach the same states as before; no new `unsafe`; renderer remains the sole GPU owner. Boot timing changes by design (script runtime now after first pixels) — see the inverted order test above.

## Tasks

### Task 1: Session container, install path, and the access discipline

Define `Session` (owning, initially holding the field group migrated in this task) and add `App.session: Option<Session>`. Expand `PendingSessionInit` (today: only `raw_args: Vec<String>`) to carry every raw input the migrated fields' build needs (content root, raw argv, settings inputs). Add `Session::build(/* pending inputs */) -> Result<Session>`, invoked from the retained `PendingSessionInit::install` entry point (reached via `App::install_pending_session`, which already `take_once`s `pending_session`); `Session::build` absorbs the migrated-field work of both `install_pending_session` and `install_post_splash_services`. It runs to completion synchronously within the single install redraw — no `await`, no yield — so no suspend can interleave a partial build; the only resume-relevant states are "not yet installed" and "installed".

**Dual-construction intermediate.** Until Task 3 finishes the migration, two construction sites coexist: the residual pre-window `build_session` still builds the not-yet-migrated `App` fields (e.g. `script_ctx`/`script_runtime` until Task 2, `net_endpoint`/`audio` until Task 3 — it does not migrate them), while `Session::build` builds the migrated group post-first-pixel. This is sound only if no migrated field depends at construction on a not-yet-migrated field, and vice versa. The Task-1 input/UI/modal group (`input_system`, `gameplay_input_latch`, `ui_dispatch`, `gamepad_system`, `ui_focus`, `ui_focus_rects`, `ui_input_mode`, `input_focus`, `modal_stack`) holds no `ScriptCtx` clone or registry handle — confirm this against source before migrating — so it is severable from the script tranche and buildable in isolation. `PendingSessionInit` carries inputs for both sites until Task 3 collapses them.

**Failure path.** On `Err` from `Session::build`: store the error in `exit_result`, log it, request `event_loop.exit()`, and early-return from `run_splash_frame_one` so no later step that frame (renderer full-init, frontend/loading transition) runs against a `None` session — mirroring `finish_renderer_full_init`'s failure handling. Subsequent redraws observe `BootState` unchanged until the loop exits.

**Single-commit guard.** The one-shot guard is the existing `take_once(&mut self.pending_session)`: once consumed, `pending_session` is `None`, so a resumed boot finds nothing to install and reruns only renderer completion. `app.session.is_some()` is the resulting observable state, not a second guard. A failed build also consumes `pending_session` and exits, so there is no retry-on-resume. This is the producer for the suspend/resume AC.

**Access discipline.** Establish the destructure-at-entry pattern in every `ApplicationHandler` method and the redraw/frame handlers. Boot-phase pre-session event handling keeps passing through one boundary (close, suspend/resume, boot-surface resize, splash redraw), ignoring gameplay/UI input until `Session` is installed. Note already-`Option` fields in the migrated group (`gamepad_system`, `ui_focus_rects`): their `Option` encodes runtime absence, distinct from "not yet installed" — keep it inside `Session`.

### Task 2: Migrate the scripting core (moves script init behind first pixels)

Move `script_ctx`, `script_runtime`, the four registries (`sequence_registry`, `reaction_registry`, `system_registry`, `classname_dispatch`), and every system that holds a cloned `ScriptCtx` or a registry reference into `Session`: `player_hud_state`, `flash_decay`, `vignette_decay`, `shake_decay`, `input_mode_tracker`, the bridges (`light_bridge`, `fog_volume_bridge`, `emitter_bridge`), the collectors (`particle_render`, `mesh_render`, `mesh_clip_tables`, `hit_zone_store`), `presentation_cells`, `crossing_detector`, `progress_tracker`, and `state_store_lifecycle`. `ScriptCtx` is `Clone` (an `Rc`-backed handle) and is cloned into eight sites at construction, so they form one indivisible tranche; `Session::build` must absorb **both** current construction sites — the three registrar clones inside `SessionServices::build` and the five subsystem clones built inline in the `App` literal in `build_session` (`player_hud_state`, `flash_decay`, `vignette_decay`, `shake_decay`, `input_mode_tracker`). This moves `ScriptCtx::new` / `register_all` / `ScriptRuntime::new` / `emit_sdk_types_in_debug` from pre-window into `Session::build` (post-first-pixel) — the change that makes `first_black_frame` precede `script_runtime_ctor`.

Re-site the `impl App` methods that read both session and boot state (e.g. `net_poll_and_apply`, `dispatch_system_commands`, the redraw block's bridge/collector calls). Rule: a session-dominant method moves onto `Session` and takes the few boot pieces (`renderer`, `camera`) as parameters; a boot-dominant method stays on `App` and takes `&mut Session` (or the needed session fields) as parameters. Apply per method by which side it touches more. This is the largest tranche (~67 `script_ctx` sites in `main.rs`).

### Task 3: Migrate remaining session state; retire `SessionServices`

Move the last session fields into `Session`: `player_options`, `settings_path` (already `Option`), the frontend declaration state (`frontend: Option<Frontend>`), `net_endpoint`, `audio`, and the `dev-tools` `debug_ui`. `net_endpoint` and `audio` stay on `App`, built by the residual pre-window / post-splash path, only until this task moves them — preserve their fallback behavior unbroken (net parse degrades to single-player, audio runs silent). Their existing per-field `Option` collapses into "present once `Session` is installed," so drop it where the `Option` only meant "not yet built"; keep it where it encodes a genuine runtime-absence state. `debug_ui` stays `#[cfg(feature = "dev-tools")]`-gated on `Session` — `Session::build` and `Session` must compile in both feature configurations; only the runtime-init `Option` is dropped, not the cfg gate. Remove the now-empty `SessionServices` (its role is wholly subsumed by `Session`). After this task, `App` holds only boot-lifetime fields plus `session: Option<Session>`.

### Task 4: Verify and document

Invert (not merely supplement) the predecessor's startup-order test so it asserts `first_black_frame` precedes `script_runtime_ctor`, and add a test for the post-first-pixel build-failure exit path. Add the gates: grep for zero session fields on `App` and zero `session_mut(` tokens; review-check the at-most-one-destructure discipline. Run the full workspace suite. Rewrite the affected `boot_sequence.md` claims, not just one caveat: the §1 boot-order stages that say the script runtime is "constructed before the window," the lifetimes table entry, the startup-timing-vocabulary ordering line (`first_black_frame` vs `script_runtime_ctor`), and the suspend/resume note about `script_ctx` retention — all now describe a post-first-pixel, type-enforced `Session`. Add `Session` and the boundary to the durable boot-sequence description.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes `Session`, the install path, the failure mode, and the access discipline; everything else builds on it.
**Phase 2 (sequential):** Task 2 — consumes Task 1's container; shares `main.rs` heavily, so it cannot overlap Task 1 or Task 3.
**Phase 3 (sequential):** Task 3 — consumes Task 2's container; finishes the migration and removes `SessionServices`.
**Phase 4 (sequential):** Task 4 — verifies and documents the completed boundary.

All phases are sequential: every task moves a field group through the same `main.rs` handlers and the shared `Session` container, so concurrent edits would collide. Each task must leave the tree compiling and the focused suite green. The intermediate states use the dual-construction topology described in Task 1: the residual pre-window `build_session` keeps constructing not-yet-migrated `App` fields while `Session::build` constructs the migrated group post-first-pixel, with no construction dependency crossing the boundary in either direction. Task 3 collapses the two sites and removes the pre-window session build.

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

Decision rule for forks this spec didn't foresee: prefer the leaner option that reuses an idiom already in the tree over a new representation, absent a measured reason otherwise. Both scope decisions above (`Option<Session>` over a phase enum; log-and-exit over a painted error frame) follow from it.
