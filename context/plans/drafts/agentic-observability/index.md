# Agentic Observability — Headless Batch Mode

> "Agent" in this plan means an **AI coding agent** working on the engine — not the NPC
> nav-agent subsystem (`AgentComponent`, `agent_steering`). Code added by this plan avoids
> the word "agent" entirely; the module and feature are named `observability`.

## Goal

Give AI coding agents a way to exercise the engine and inspect resulting game state
without a window, GPU, or display server: load a real `.prl` map, run N fixed ticks with
scripted player commands, dump world state as JSON, exit. This is the first slice of a
larger observability effort (live socket channel and frame capture follow as separate
plans); the command/query vocabulary born here is the shared substrate.

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
- Renderer-derived world state headless: light-bridge and fog-bridge entities are
  populated from renderer-computed values in the windowed path and are absent headless.
  The output document declares this (see AC) rather than faking it.
- Multiplayer/net roles headless. Single-player sim only; the future dedicated-server
  entry point (Epic 15 Phase 4) consumes the same session/world-install seams but is its
  own plan.
- Protocol versioning. Engine and runspec live in one repo and build in lockstep.

## Acceptance criteria

- [ ] `postretro --headless <runspec>` targeting `content/dev/maps/campaign-test.prl`
      loads the map, runs the requested tick count, writes a single JSON document to
      stdout, and exits 0 — on a machine with no display server and no GPU adapter.
      All logging stays on stderr; stdout carries only the JSON document.
- [ ] Two identical headless runs produce byte-identical stdout.
- [ ] A runspec commanding forward movement for 60 ticks reports a player pawn position
      displaced from spawn; a runspec with no commands reports the pawn settled at spawn.
- [ ] A runspec commanding a jump surfaces a movement tick event in the output.
- [ ] Entity dump honors component-kind filter, tag filter, and an entry cap; a
      truncated dump says so explicitly (count of omitted entries), never silently.
- [ ] The output document carries an out-of-frame declaration: renderer-derived entities
      (map lights, fog volumes) and side-table state (collision geometry, mover geometry,
      hit zones) are named as absent, so a reader can distinguish "absent" from "empty."
- [ ] `cargo build -p postretro` (no features) compiles without the observability module
      and pulls no new dependencies; the full existing test suite passes unchanged.
- [ ] Windowed level load behavior is unchanged after the world-install extraction
      (existing lifecycle tests pass; dev launch of campaign-test.prl plays normally).
- [ ] `cargo run -p xtask -- observe <runspec>` builds the scripts sidecar, builds
      `postretro` with the `observability` feature, runs headless, and forwards stdout —
      one command, non-TTY friendly.
- [ ] An invalid runspec (missing map, malformed JSON, unknown field) exits non-zero
      with a diagnostic on stderr and no partial JSON on stdout.

## Tasks

### Task 1: World-install extraction

Split the CPU-side world install out of `install_level_payload`
(`crates/postretro/src/startup/lifecycle.rs`) into a function callable without a
renderer. CPU side: gravity set from the loaded level, collision-world populate,
nav-graph build from the baked navmesh section, kinematic-mover collider build and mover
entity spawns, spawn-point partition (player-start classname), classname dispatch of map
entities, and the data-script archetype sweep. Renderer/session-visual steps stay in the
windowed path: texture install, UV normalize, geometry install, light-bridge populate
(fed by renderer light data), fog pixel scale and cell masks, smoke-collection
registration, audio level-sound load, debug-UI reseed. The extracted function takes its
dependencies as parameters (registry handle, classname dispatch table, scripting
handles) rather than reading `self`, so a headless caller without `App` can drive it.
Behavior-preserving for the windowed path: same install order, same logs. This is the
split-before-extend step for `lifecycle.rs` (2,602 lines); do not add headless logic here.

### Task 2: Headless session construction

Add a reduced session-construction path beside `Session::build`
(`crates/postretro/src/startup/session.rs`): scripting core only (script runtime,
registries, script ctx, classname dispatch, data-script runner, mod-init execution) —
no audio, input, UI, modal stack, net endpoint, options I/O, or window. Extract the
scripting-core construction shared between `Session::build` and the headless path into
one function so the two cannot drift; `Session::build` output is unchanged. The headless
path requires the `scripts-build` sidecar exactly as the windowed engine does — surface a
clear error naming the xtask launch when it is missing.

### Task 3: Observability vocabulary module

New module `crates/postretro/src/observability/`, gated on a new `observability` cargo
feature (feature pulls no egui and is independent of `dev-tools`). Serde types, all
snake_case: `RunSpec` (map path, tick count, ordered per-tick command entries carrying
movement input, aim origin/direction, fire, reload), dump filters (component kind, tag,
entity-id list, entry cap), and the output document (map identity, ticks run, filtered
entity list serializing `ComponentValue` through its existing serde derives, per-tick
event lists, player pawn summary, out-of-frame declaration, truncation count). Aim lives
in the runspec because `SimCommand` carries no pitch — aim feeds the post-movement
command, mirroring how the windowed engine derives aim from the camera. Unit tests:
runspec round-trip, unknown-field rejection, filter semantics against an in-memory
registry, truncation reporting.

### Task 4: Headless driver

Wire `--headless <runspec.json>`: detect the flag in early arg handling and branch
before event-loop creation — the only `main.rs` touch, kept to flag detection plus one
call into `observability::run_headless`. The driver: parse and validate the runspec,
load the PRL synchronously via `postretro_level_loader::load_prl`, build the headless
session (Task 2), run the extracted world install (Task 1), then loop the requested
ticks calling `simulate_tick` with the per-tick `SimCommand` and a post-movement closure
returning the runspec's aim; collect tick events; serialize the output document (Task 3)
to stdout; exit 0, or non-zero with stderr diagnostics on any failure. Never constructs
winit, wgpu, or kira types. Determinism guard: no wall-clock values in the document;
iteration orders must be stable (registry column order; no `HashSet`-ordered output).

### Task 5: xtask observe subcommand

Add `observe <runspec.json>` to `crates/xtask/src/main.rs`: ensure the `scripts-build`
sidecar (reuse the existing `run` plumbing), `cargo build -p postretro --features
observability`, execute `postretro --headless <runspec>`, forward stdout/stderr and exit
code untouched. No TTY assumptions. xtask does not parse or interpret the JSON.

## Sequencing

**Phase 1 (concurrent):** Task 1 (lifecycle.rs), Task 2 (session.rs), Task 3 (new
module) — disjoint files, fits the 3-worktree cap.
**Phase 2 (sequential):** Task 4 — consumes all three; sole `main.rs`-touching task.
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
- `simulate_tick` (`crates/postretro/src/sim/mod.rs`) already takes everything as
  parameters; the headless loop assembles the same arguments `App`'s tick loop does:
  registry, collision world, hit-zone store, nav graph, gravity, mover colliders and
  tick states, empty remote-pawn commands, per-tick `SimCommand`, post-movement aim
  closure, fixed `TICK_DURATION` dt.
- Runspec sketch (proposed design — remove after implementation):

  ```json
  {
    "map": "content/dev/maps/campaign-test.prl",
    "ticks": 300,
    "commands": [
      { "tick": 0, "movement": { "wish_dir": [0.0, 1.0], "jump": false },
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
- Weapon-fire verification headless is untested territory: `active_wieldable` is set by
  windowed wieldable install after the data script. The vocabulary carries fire/reload
  from day one; whether firing produces authorized shots headless is a
  decision-during-implementation (see Open questions).

## Boundary inventory

| Name | Rust | Wire / serde |
|---|---|---|
| run-spec / output fields | struct fields | `snake_case` |
| component payloads | `ComponentValue` | existing derive: tag `"kind"`, `snake_case` variants |
| component-kind filter | `ComponentKind` | existing serde derive |
| entity ids | `EntityId` | existing serde derive (packed u32) |

No JS/Luau/FGD surface — the runspec is consumed by tools, not by mods. Scripting SDK
untouched.

## Open questions

- Does weapon fire work headless in v1? `active_wieldable` plumbing may be
  window-session-coupled. Resolve during Task 4: if wiring it is small, do it; if not,
  document fire as inert headless and leave the vocabulary fields in place. Either
  outcome satisfies the AC list (no AC requires fire).
- Epic 15 Phase 4 (dedicated server) will want Tasks 1–2's seams. Coordinate at
  promotion: note in the roadmap that the headless server entry point should consume the
  world-install and headless-session functions rather than growing a parallel path.
