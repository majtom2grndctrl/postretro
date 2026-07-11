# Agentic Observability — Research Notes

Session research (three parallel codebase surveys + one adversarial critique pass)
behind `index.md`. Findings only; decisions live in the spec.

## What exists today

All debug infrastructure is human-in-the-loop. No IPC, sockets, HTTP, stdin command
reader, quake-style console, cvar system, headless engine mode, screenshot capability,
or replay/demo system exists.

| Substrate | Where | Relevance |
|---|---|---|
| Headless sim seam | `crates/postretro/src/sim/mod.rs` (`simulate_tick`, `pub(crate)`) | The core reusable asset. Deterministic (proptest coverage in `sim/determinism_tests.rs`), window/GPU/audio-free. Wide parameter surface — caller assembles world state. |
| Per-frame command drains | `SystemCommandQueue` drained by `App::dispatch_system_commands` (post-tick); `level_requests: VecDeque<LevelRequest>` drained at redraw boundary (pre-gameplay) | Shipped closed-vocabulary command precedents. Note they sit at opposite ends of the frame — control wants pre-tick, queries want post-tick settled state. |
| Cross-thread ingress | level worker: `thread::spawn` + `mpsc` + compile-time `Send` guard (`startup/worker.rs`) | Pattern for any future transport thread. |
| Serde on game state | `EntityId`, `ComponentValue` (tag `"kind"`, snake_case), `ComponentKind`, `Transform` all derive | Entity dump is nearly free. Partial picture: collision AABBs, mover geometry, hit zones live in side tables outside `ComponentValue`. |
| egui debug panel | `crates/renderer/src/render/debug_ui/` behind `dev-tools` | Human-facing; egui slated for retirement (Epic 13 BIS). Do not build on it. |
| Frame/render stats | `FrameRateMeter`, `VisibilityStats`, `CameraCullDiagnostics`, `FrameTimingSnapshot` (`POSTRETRO_GPU_TIMING=1`) | Gathered per frame on main thread; export is a copy. Live-channel material, not headless. |
| Diagnostic chords | `DiagnosticAction` enum, `crates/postretro/src/input/diagnostics.rs` | Closed action-enum precedent. |
| GPU readback | `read_texture_rgba8` in `render/ui/gpu_test_harness.rs` — `#[cfg(test)]`, `pub(crate)`, offscreen textures only | NOT reusable as-is for screenshots. Live frame renders into the swapchain (no `COPY_SRC`); present is an opaque `PresentHandle` post-Epic-19. Frame capture = new renderer feature (offscreen target + blit or surface-usage change). |
| Log capture | thread-local `CaptureLogger`, `#[cfg(test)]`-shaped | Logging is `env_logger` stderr text; no structured sink. |

## Hard constraints (from context/lib + source)

- Registry, script runtime, renderer are `!Send` (`Rc<RefCell<..>>`); all engine-state
  access on the main thread. Any future transport thread marshals via mpsc + frame drain.
- Scripting is not a control channel: VM drops after load; live-VM primitives
  (`spawnEntity`, `getComponent`, ...) were deliberately removed
  (`scripting/primitives/mod.rs` asserts their absence).
- Renderer owns GPU; no `unsafe` without approval; no speculative abstraction (traits
  need 2+ impls); `thiserror` at boundaries, `anyhow` at top level.
- ≤3 concurrent build worktrees (QuickJS C build); `main.rs` is 7,602 lines — at most
  one task touching it per wave.
- Netcode is host-authoritative; any future live mutating verbs need a
  single-player/host gate or reconciliation fights the write.

## Why headless-first (critique outcome)

Original phasing led with a live socket into a windowed session. The critique inverted
it: AI agents frequently run where a window cannot open (no display, no adapter — true
of this very container), the existing sim seam already answers state questions in tests,
and the genuinely missing capabilities are (1) map load + N ticks + state dump *outside*
the test harness, (2) pixels. Headless batch mode works in every environment, has no
liveness/backpressure problem, and proves the vocabulary before transport engineering.

Other critique findings folded into the spec: `SimCommand` has no aim/pitch (aim comes
from camera via `PostMovementCommand` — runspec carries aim); the two drain precedents
sit at opposite frame ends; E17 is an unimplemented spec, cite shipped precedents;
`install_level_payload` (lifecycle.rs:502) interleaves renderer and CPU install — the
light bridge is fed `renderer.level_lights()`, so light/fog entities are out-of-frame
headless; cut log ring buffer, protocol versioning, separate ctl crate.

## Crate placement analysis

Question: new crate or modules in `postretro`?

- The sim seam, collision, movers, scripting systems, startup are all `pub(crate)` in
  the `postretro` binary crate. A new crate cannot reach them without promoting the sim
  seam to a public cross-crate contract — a real API commitment nobody else needs yet.
- The one-way crate graph offers no natural lower home: `entities` can't see collision
  or sim; `net` is transport-specific; a crate above `postretro` is impossible (it's the
  binary root).
- Conclusion: module `crates/postretro/src/observability/` + feature `observability = []`
  (no deps, gates modules only). Independent of `dev-tools` so it never links egui and
  survives Epic 13's egui retirement.
- Future extraction points, when earned: `postretro-observability-protocol` (runspec/
  output types) once a second *typed* consumer exists — xtask v1 passes JSON through
  untyped; renderer frame-capture API lands in `postretro-renderer` (renderer-owns-GPU
  dictates it); Epic 19 already deferred a `postretro-render-diagnostics` crate as a
  named future home for CPU-side render diagnostics.

## Follow-up phases (separate plans, drafted after this ships)

1. **Frame capture** — renderer-owned offscreen readback + PNG; deterministic camera
   placement; honest budget (new render target or surface `COPY_SRC` with per-backend
   caveats; must respect opaque `PresentHandle`). Highest-value live capability: visual
   verification is deliberately absent from the test suite.
2. **Live socket channel** — same vocabulary over a localhost transport thread + mpsc +
   two-point frame drain (pre-tick commands, post-tick queries). Must design: liveness
   when the window is occluded/minimized/suspended or pre-session (transport answers
   "alive, not rendering" without a frame); bounded response channel + dead-client
   policy; host-authority gate on mutating verbs; sync primitives (advance-N-ticks,
   wait-for-predicate with timeout, atomic batches). Task-0 spike: prove windowed
   dev-tools launch in the target agent environment before spending waves.
3. **Later** — streaming telemetry, MCP wrapper over the same protocol.

## Resolved hedges (zoom-out pass)

Two open questions resolved from project goals + source verification:

- **Weapon fire headless: committed v1 scope.** Combat is the engine's core loop (the
  active in-progress plan is E16 client-authoritative-combat), and
  `sim/determinism_tests.rs` already fires weapons headless — spawns a weapon entity,
  passes `Some(active_wieldable)` to `simulate_tick`. The firing proof is
  `simulate_tick_normalizes_callback_aim_direction_before_weapon_fire`
  (`determinism_tests.rs:1125`); `simulate_tick_uses_sim_command_fire_button_with_callback_aim`
  is the negative control (asserts no fire when the button is inactive). Only new driver
  work: resolve the wieldable id after map load.
- **Epic 15 Phase 4: design requirement, not footnote.** Phase 4's testable outcome
  requires a standalone headless server entry point; Tasks 1–2 are its prerequisite
  seams. Constraints moved into the task paragraphs (no caller-lifetime assumptions; do
  not preclude a net endpoint).
- **Modder audience.** "Modder-friendly" is a stated project goal; this runner is
  content-CI tooling (runtime sibling of prl-build). Runspec/output treated as a stable
  tool-facing surface; format docs eventually graduate to human-facing `docs/`.

## Epic 15 Phase 4 coordination

Roadmap: "A headless server entry point runs standalone, confirming dedicated-server
readiness" (open). The dedicated server needs exactly this plan's Tasks 1–2 (world
install without renderer, reduced session). At promotion, note in the roadmap that
Phase 4 consumes these seams — one headless substrate, two entry points (batch runner,
server loop).
