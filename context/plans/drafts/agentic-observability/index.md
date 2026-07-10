# Agentic Observability — Headless Batch Mode

> "Agent" in this plan means an **AI coding agent** working on the engine — not the NPC
> nav-agent subsystem (`AgentComponent`, `agent_steering`). Code added by this plan avoids
> the word "agent" entirely; the module and feature are named `observability`.

## Goal

Give AI coding agents a way to exercise the engine and inspect resulting game state
without a window, GPU, or display server: load a real `.prl` map, run N fixed ticks with
scripted player commands, dump world state as JSON, exit. This is the first slice of a
larger observability effort (live socket channel and frame capture follow as separate
plans); the command/query vocabulary born here is the shared substrate. Second audience:
mod and map authors — this runner is the runtime sibling of `prl-build` for content CI,
so the runspec/output format is designed as a stable tool-facing surface, not a
throwaway debug format.

## Scope

### In scope

- A `--headless <runspec.json>` mode of the `postretro` binary: no winit, no wgpu, no kira.
- A run-spec input format: map path, tick count, per-tick player commands (movement,
  aim, fire, reload), dump filters.
- A JSON output document on stdout: filtered entity dump, tick events, player pawn
  summary, explicit out-of-frame notice.
- Extraction of the CPU-side world install (collision, nav, movers, map-entity dispatch,
  data script) into a seam shared by the windowed and headless paths.
- A reduced headless session construction path (scripting core only — no audio, input,
  UI, net, or window), sharing construction code with `Session::build`.
- A new `observability` cargo feature on `postretro` (independent of `dev-tools`; pulls
  no egui) and an xtask subcommand to build + run headless in one command.

### Out of scope

- Live socket/IPC channel into a running windowed session (follow-up plan; reuses this
  vocabulary).
- Screenshot / frame capture (follow-up plan; renderer-owned readback).
- Streaming telemetry, MCP wrapper, log ring buffer.
- A new crate. The sim seam (`simulate_tick`, `SimCommand`) is `pub(crate)` inside
  `postretro`; headless mode lives there as a module. A shared protocol crate is deferred
  until a second consumer needs typed access (xtask passes JSON through untyped).
- Renderer-derived world state headless: light-bridge entities are populated from
  renderer-computed values (`renderer.level_lights()`) in the windowed path and are
  absent headless. Fog-volume entities are populated headless (`fog_volume_bridge`
  reads PRL data and needs only the registry — only fog pixel scale / cell masks are
  renderer-side). The output document declares the light-only absence (see AC) rather
  than faking it.
- Multiplayer/net roles headless. Single-player sim only; the future dedicated-server
  entry point (Epic 15 Phase 4) consumes the same session/world-install seams but is its
  own plan.
- Protocol versioning. Engine and runspec live in one repo and build in lockstep.
- End-to-end reload verification. The runspec `reload` field maps to
  `SimCommand.reload` and is unit-tested as vocabulary passthrough; asserting reload
  deliveries end-to-end waits for the ammo-resource work (E16).

## Acceptance criteria

- [ ] `postretro --headless <runspec>` targeting `content/dev/maps/campaign-test.prl`
      (compiled first from `content/dev/maps/campaign-test.map` via `prl-build` if
      absent) loads the map, runs the requested tick count, writes a single JSON
      document to stdout, and exits 0 — on a machine with no display server and no GPU
      adapter. All logging stays on stderr; stdout carries only the JSON document.
- [ ] Two identical headless runs produce byte-identical stdout.
- [ ] A runspec commanding forward movement for 60 ticks reports a player pawn position
      displaced from spawn; a runspec with no commands reports the pawn settled at spawn.
- [ ] A runspec commanding a jump surfaces a movement tick event in the output.
- [ ] A runspec commanding weapon fire with valid aim surfaces a weapon tick event in
      the output; the same runspec with `fire` false surfaces none.
- [ ] Entity dump honors component-kind filter, tag filter, and an entry cap; a
      truncated dump says so explicitly (count of omitted entries), never silently.
- [ ] The output document carries an out-of-frame declaration split into two honest
      categories: entities not present headless (map lights only — fog volumes are
      populated headless) and state present but not serialized in the entity dump
      (collision geometry, mover geometry, hit zones — side tables the sim uses but the
      dump omits), so a reader can distinguish "absent" from "not dumped."
- [ ] `cargo build -p postretro` (no features) compiles without the observability module
      and pulls no new dependencies; the full existing test suite passes unchanged.
- [ ] Windowed level load behavior is unchanged after the world-install extraction
      (existing lifecycle tests pass; dev launch of campaign-test.prl plays normally).
- [ ] `cargo run -p xtask -- observe <runspec>` builds the scripts sidecar, builds
      `postretro` with the `observability` feature, runs headless, and forwards stdout —
      one command, non-TTY friendly.
- [ ] An invalid runspec (missing map, malformed JSON, unknown field) exits non-zero
      with a diagnostic on stderr and no partial JSON on stdout.
- [ ] `--headless` on a build compiled without the `observability` feature exits
      non-zero with a diagnostic naming the xtask command.

## Tasks

### Task 1: World-install extraction

Split the CPU-side world install out of `install_level_payload`
(`crates/postretro/src/startup/lifecycle.rs`) into a function callable without a
renderer. CPU side, in source order: gravity set from the loaded level (lifecycle.rs:525),
nav-graph build from the baked navmesh section (:629), fog-volume population (~:651-663;
entity creation only — the renderer-side pixel-scale/cell-mask push stays windowed),
collision-world populate (:670), kinematic-mover collider build and mover entity spawns
(:671-687), spawn-point partition (player-start classname) and classname dispatch of map
entities (~:700-741), and the data-script archetype sweep (lifecycle.rs:835-997) —
including player-pawn spawn (`spawn_from_player_starts`, :927, producing
`active_wieldable` at :971) — plus the mesh model sweep's CPU half (clip-index /
hit-zone-store resolve, ~:1057-1070; `simulate_tick` takes the hit-zone store) and the
`levelLoad` event fire (headless fires it too, so data-script reactions and crossings
compose identically to the windowed path). CPU steps are interleaved with renderer steps
in the current body (texture/UV/geometry upload sits between the gravity set and the
nav-graph build, :581-597; light-bridge populate sits between the nav-graph build and fog
volumes); the extraction pulls the CPU steps into one contiguous function preserving
their relative CPU order, and the windowed path keeps the same observable install
behavior. Renderer/session-visual steps stay in the windowed path: texture install, UV
normalize, geometry install, light-bridge populate (fed by renderer light data), fog
pixel scale and cell masks, mesh-model upload, smoke-collection registration, audio
level-sound load, debug-UI reseed, and — within the archetype-sweep block — the spawn
camera teleport and `light_bridge.absorb_dynamic_lights` (:992). The extracted function
returns/populates everything the tick loop consumes:
populated registry (via the passed handle), collision world, nav graph, mover colliders
and mover tick-state table, gravity, hit-zone store, and the spawn result including
`active_wieldable` and its descriptor — so a caller without `App` can assemble
`simulate_tick`'s arguments from the extracted function's outputs alone. The extracted
function takes its dependencies as parameters (registry handle, classname dispatch table,
scripting handles) rather than reading `self`, so a headless caller without `App` can
drive it. Behavior-preserving for the windowed path: same install order, same logs. This
is the split-before-extend step for `lifecycle.rs` (2,602 lines); do not add headless
logic here. Design constraint: the extracted function is shared substrate with two
committed consumers — this plan's batch runner and Epic 15 Phase 4's dedicated-server
entry point — so it must embed no assumptions about caller lifetime (no "run N ticks then
exit" shape, no windowed-only state).

### Task 2: Headless session construction

Add a reduced session-construction path beside `Session::build`
(`crates/postretro/src/session/mod.rs`, impl at ~line 264): scripting core only (script
runtime, registries, script ctx, classname dispatch, data-script runner) — no audio,
input, UI, modal stack, net endpoint, options I/O, or window. The scripting-core
construction to extract lives in `session/mod.rs` (~lines 322-383). Extract the
scripting-core construction shared between `Session::build` and the headless path into
one function so the two cannot drift; `Session::build` output is unchanged. The shared
extractor covers scripting-core construction only — script runtime, script ctx,
registries, classname dispatch (note `classname_dispatch` is a separate `Session` field,
not inside the `ScriptingCore` struct; the extractor produces both; the screen-effect
decay fields inside `ScriptingCore` come along and sit unused headless). Mod-init is NOT
part of `Session::build` — it runs post-build via `ScriptRuntime::run_mod_init` from
App's deferred logo-frame path. Mod-init execution becomes a headless-driver step (Task 4
calls `run_mod_init` after building the scripting core, before world install, to populate
the data registry for the archetype sweep). The headless path derives its content root
from the runspec map path (windowed derives from argv). The headless path requires the
`scripts-build` sidecar exactly as the windowed engine does — surface a clear error
naming the xtask launch when it is missing. Same two-consumer constraint as Task 1: Epic
15 Phase 4's dedicated server will attach a net endpoint to this session later, so the
construction path must not preclude one (omit it, don't design it out).

### Task 3: Observability vocabulary module

New module `crates/postretro/src/observability/`, gated on a new `observability` cargo
feature (feature pulls no egui and is independent of `dev-tools`). Serde types, all
snake_case: `RunSpec` (map path, tick count, ordered per-tick command entries carrying
movement input, aim origin/direction, fire, reload), dump filters (component kind, tag,
entity-id list, entry cap), and the output document (map identity, ticks run, filtered
entity list serializing `ComponentValue` through its existing serde derives, per-tick
event lists, player pawn summary, out-of-frame declaration split into entities absent
headless (map lights only) and state present-but-not-dumped (collision geometry, mover
geometry, hit zones), truncation count). Aim lives
in the runspec because `SimCommand` carries no pitch — aim feeds the post-movement
command, mirroring how the windowed engine derives aim from the camera. Unit tests:
runspec round-trip, unknown-field rejection, filter semantics against an in-memory
registry, truncation reporting, reload passthrough.

### Task 4: Headless driver

Wire `--headless <runspec.json>`: the branch lands in `startup::build_session`
(`crates/postretro/src/startup/session.rs`), which collects argv and resolves the map
path before `EventLoop::new()` — `main.rs` itself does no arg parsing and is touched at
most trivially. The branch is `#[cfg(feature = "observability")]`-gated; a build without
the feature that receives `--headless` exits non-zero with a diagnostic naming the xtask
observe command. The driver: parse and validate the runspec, load the PRL synchronously
via `postretro_level_loader::load_prl`, build the headless session (Task 2), run the
extracted world install (Task 1), then loop the requested ticks calling `simulate_tick`
with the per-tick `SimCommand` and a post-movement closure returning the runspec's aim;
collect tick events; serialize the output document (Task 3) to stdout; exit 0, or
non-zero with stderr diagnostics on any failure. Weapon fire is in scope: the driver
receives `active_wieldable` from the world-install output (Task 1; windowed source:
`spawn_from_player_starts` return, lifecycle.rs:927/:971) and passes it to
`simulate_tick`. Never constructs winit, wgpu, or kira types. Determinism guard: no
wall-clock values in the document; iteration orders must be stable (registry column
order; no `HashSet`-ordered output).

### Task 5: xtask observe subcommand

Add `observe <runspec.json>` to `crates/xtask/src/main.rs`: ensure the `scripts-build`
sidecar (reuse the existing `run` plumbing), then run a single
`cargo run -p postretro --features observability -- --headless <runspec>` invocation
(path resolution handled by cargo, mirrors existing run plumbing), forwarding
stdout/stderr and exit code untouched. No TTY assumptions. xtask does not parse or
interpret the JSON.

## Sequencing

**Phase 1 (concurrent):** Task 1 (lifecycle.rs), Task 2 (session/mod.rs), Task 3 (new
module) — disjoint files, fits the 3-worktree cap.
**Phase 2 (sequential):** Task 4 — consumes all three; sole task touching the windowed
boot path (`startup/session.rs`).
**Phase 3 (sequential):** Task 5 — consumes Task 4's CLI surface; doubles as the
end-to-end verification of the whole plan.

## Rough sketch

- Precedents to follow (shipped code, not specs): the typed per-frame command drain
  (`SystemReactionCommand` / `SystemCommandQueue`, drained by
  `App::dispatch_system_commands`) for closed-vocabulary command shape;
  `DiagnosticAction` (`crates/postretro/src/input/diagnostics.rs`) for a closed action
  enum; the level worker (`crates/postretro/src/startup/worker.rs`) for `Send`-POD load
  separation. Do not model on `E17--trigger-command-surface` — it is a ready spec, not
  shipped code.
- `simulate_tick` (`crates/postretro/src/sim/mod.rs`) takes 15 parameters; the headless
  loop assembles the same arguments `App`'s tick loop does: registry, collision world,
  hit-zone store, nav graph, gravity, mover colliders and tick states, empty remote-pawn
  commands, per-tick `SimCommand`, post-movement aim closure, `active_wieldable:
  Option<EntityId>`, `anim_time: f64`, `progress_tracker: &mut ProgressTracker`,
  `ai_warned: &mut HashSet<String>`, and `tick_dt: f32`. The driver owns and persists the
  progress tracker and the warned-set across ticks, and advances `anim_time` by dt each
  tick. `TICK_DURATION` is a `Duration` (16_667 µs); its `as_secs_f32()` (0.016667) does
  not equal `1.0 / 60.0`, so the headless loop passes `1.0 / 60.0` f32 directly for
  `tick_dt`, matching the determinism tests' `DT`.
- Fire mapping: runspec `fire` is a held/level signal; the driver derives
  `FireButtonState { pressed, active }` per tick — `active` is the current fire value,
  `pressed` is the rising edge (fire true this tick, false the previous tick). Sparse
  command windows hold the level; the edge derives from consecutive tick values.
- Movement mapping: runspec movement fields map to `MovementInput { wish_dir,
  jump_pressed, dash_pressed, running, crouch_intent, facing_yaw }`. The runspec uses the
  same snake_case field names (`jump_pressed`, etc.); absent fields default to
  false/zero; `facing_yaw` is derived from the runspec aim direction's yaw, not authored
  separately.
- Runspec sketch (proposed design — remove after implementation):

  ```json
  {
    "map": "content/dev/maps/campaign-test.prl",
    "ticks": 300,
    "commands": [
      { "tick": 0, "movement": { "wish_dir": [0.0, 1.0], "jump_pressed": false },
        "aim": { "origin": [0,1.6,0], "direction": [0,0,-1] }, "fire": false }
    ],
    "dump": { "component": "health", "tag": null, "cap": 500, "events": true }
  }
  ```

  Commands are sparse: an entry applies from its tick until the next entry; absent
  fields mean neutral input.
- Crate placement decision: everything lands in `crates/postretro` (module
  `src/observability/`) because the sim seam is `pub(crate)` and widening it into a
  public cross-crate contract is unjustified speculation today. The renderer, when frame
  capture arrives (follow-up plan), gets its own readback API in `postretro-renderer` —
  the renderer-owns-GPU invariant already dictates that split. Extract a protocol crate
  only when a second typed consumer exists.
- Feature placement: `observability = []` on `postretro` — no optional deps today, the
  feature only gates modules. Deliberately not folded into `dev-tools`, which carries
  egui (slated for retirement under Epic 13 BIS); keeping them independent means the
  headless mode survives egui's removal and never links it.
- Weapon fire headless is proven ground: `sim/determinism_tests.rs` spawns a weapon,
  passes `Some(active_wieldable)` to `simulate_tick`, and asserts fire-button + callback-
  aim behavior. `simulate_tick_uses_sim_command_fire_button_with_callback_aim` is the
  negative control (asserts no fire when the button is inactive);
  `simulate_tick_normalizes_callback_aim_direction_before_weapon_fire`
  (`determinism_tests.rs:1125`) is the test that actually fires with callback aim — cite
  that one as the firing proof. The driver's only new work is resolving the wieldable id
  after a map load.

## Boundary inventory

| Name | Rust | Wire / serde |
|---|---|---|
| run-spec / output fields | struct fields | `snake_case` |
| component payloads | `ComponentValue` | existing derive: envelope/variant tag `"kind"` is `snake_case`; embedded payload fields keep per-component casing (some are camelCase, e.g. light/fog components) |
| component-kind filter | `ComponentKind` | existing serde derive has no `snake_case` rename — serializes PascalCase (e.g. `"Health"`). The dump filter accepts the same snake_case strings as `ComponentValue`'s `"kind"` tag, mapped explicitly in the observability module; `ComponentKind`'s derive is untouched |
| entity ids | `EntityId` | existing serde derive (packed u32) |

No JS/Luau/FGD surface — the runspec is consumed by tools, not by mods. Scripting SDK
untouched.

## Promotion notes

No open questions — earlier hedges resolved against source and project goals: weapon
fire is committed v1 scope (the sim seam already fires headless in the determinism
tests — `simulate_tick_normalizes_callback_aim_direction_before_weapon_fire`,
`sim/determinism_tests.rs:1125`, proves firing with callback aim), and Tasks 1–2 are
explicitly shared substrate with Epic 15 Phase 4.

At promotion: add a line to the roadmap's Epic 15 Phase 4 entry stating the dedicated-
server entry point consumes this plan's world-install and headless-session functions —
one headless substrate, two entry points.
