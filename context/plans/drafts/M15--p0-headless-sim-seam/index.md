# M15 Phase 0 — Headless simulation seam + determinism harness + spike

> Milestone 15 (multiplayer netcode), Phase 0. Design reference:
> `context/research/netcode/` (`index.md` Phase 0, `research.md` §6). Grounded code map:
> sibling `research.md`.

## Goal

Extract PostRetro's fixed-tick game logic out of the render-interleaved frame loop into
a **headless `simulate` seam** that advances one game-logic tick from explicit game state
+ a resolved input command, with **no wgpu/winit/renderer/audio/input dependency** — the
single tick path a server and a client will both call (Phases 1+). Prove it with a
**determinism harness** (the honesty gate) and, as a build-to-learn spike, **measure the
cross-architecture `f32` divergence** that sets the reconciliation tolerance and get an
empirical **feel** read on basic predict/reconcile. This is the foundation every later
netcode phase rides; a leaky seam here poisons reconciliation forever.

## Scope

### In scope
- **Behavior-preserving split** of `movement/mod.rs` (substrate / intent / dispatch /
  public API) and of `main.rs`'s game-tick assembly — *before* extending either.
- A headless seam covering the design-reference tick order: **transform-snapshot →
  movement → weapon-fire → death-sweep → event firing → system-command enqueue**.
- **Severing the render/window leaks** from the tick path: `frame_timing.push_state`,
  camera-follow/fly-cam, and the audio/gamepad/UI arms of `dispatch_system_commands`
  move to the host caller as post-tick steps; the seam only enqueues system commands.
- A **determinism harness + test written first** (recorded input stream → deterministic
  tick), green-and-stays-green.
- **Spike (build-to-learn):** measured cross-arch `f32` divergence over N ticks under
  forced-divergence conditions; a throwaway predict/reconcile feel-prototype.

### Out of scope
- Any transport, wire format, serialization, or networking (Phase 1+).
- Extending the seam to the **scripting-bridge stage** (emitter / particle sim / light /
  fog bridges, `entity_model.md` §5 order 4) — those run after the core tick and are
  partly render-collectors; broadening the seam to cover them is a later phase.
- Changing any movement, weapon, or death-sweep **behavior** — the splits and the seam
  are behavior-preserving (the spike's feel-prototype is throwaway, not shipped).
- A standalone headless-server binary / dedicated-server entry point (Phase 7).
- Hardening the cross-arch result into a pass/fail threshold — it is a recorded finding
  (`experimental_spikes.md`), not a gate.

## Acceptance criteria

- [ ] `movement/mod.rs` logic is split into cohesive substrate / intent / dispatch /
  public-API modules; the existing ~145 movement tests pass **unchanged** and
  `movement::tick`'s public signature is unchanged (behavior-preserving). (Task 1)
- [ ] The game-tick assembly is extracted from `main.rs` into a dedicated module the
  frame loop calls; the engine plays with **identical gameplay** (movement, weapon fire,
  death all behave as before) and the build + existing tests are green. (Task 2)
- [ ] **Honesty gate:** the seam advances one full tick (transform-snapshot → movement →
  weapon → death → events) constructed and called with **no renderer, audio, window,
  input-system, wgpu, or winit in scope** — demonstrated by a harness that builds the
  game state + a recorded input command and ticks it with no window/GPU context. The
  interpolation push, camera-follow, and audio/gamepad/UI command dispatch are no longer
  inside the seam. (Task 3)
- [ ] **Honesty gate:** the determinism test ticks N pawns from a recorded input stream
  and is **green and stays green** — the same recorded input from the same initial state
  produces the same output (positions, velocities, fired events) within epsilon,
  run-to-run, on the build machine. (Task 3)
- [ ] **Measured finding:** same-input simulation divergence over N ticks is **recorded**
  (a logged value) under forced-divergence conditions, and the recommendation states the
  reconciliation tolerance it implies for Phase 3. (Task 4)
- [ ] **Measured finding:** a throwaway predict/reconcile feel-prototype runs and yields a
  **recorded read** on whether basic reconciliation feels acceptable; it is clearly marked
  throwaway and ships nothing into the engine path. (Task 5)

## Tasks

### Task 1: Split `movement/mod.rs` along the §4 seam
Break the ~1,875 lines of logic into a `movement/` module set following the
`movement.md` §4 boundary: a **substrate** module (`pm_accelerate`, `wish_dir_from_input`,
`step_up_lift`, `integrate_collision`, `resize_capsule`, `standup_clearance_probe`, the
forgiveness/jump-edge helpers — the collide-and-slide physics), an **intents** module
(`normal_intent`, `dash_intent`, `crouching_intent`, `try_enter_dash`, the IR
`resolve_*` helpers), a **dispatch** module (`dispatch_state_intent`, boost/carry,
stand-up helpers), and the **public API** in `mod.rs` (`MovementInput`, `MovementEvents`,
`SubstrateResult`, `Transition`, `MovementState`, `pub(crate) fn tick`). Behavior-
preserving — the public `movement::tick` signature is untouched; the test block stays
(co-located or a sibling test module, exempt from the size rule). This is the
split-before-extend for the file Phase 3 will extend.

### Task 2: Extract the game-tick assembly from `main.rs`
Move the per-tick logic out of the `RedrawRequested` handler and the `run_movement_tick`
/ `run_weapon_fire_tick` / `run_death_sweep` `impl App` methods into a new engine module
(working name `sim`). The extracted functions take **explicit game-state parameters**
(the registry/`ScriptCtx` handles, `&CollisionWorld`, `&HitZoneStore`, gravity,
`active_wieldable`, `anim_time`, `&mut ProgressTracker`, the reaction/sequence/system
registries) and a resolved per-tick **input command** — never `&mut App`. The inline
input-intent resolution (axes, jump/dash/crouch edges, `MovementInput` construction) and
the event-drain/system-command-enqueue move with it. Behavior-preserving: the frame loop
now builds the command from its local snapshot + camera and calls `sim::*`; the
render-side steps (camera-follow, `frame_timing.push_state`, the audio/gamepad/UI command
arms) stay in the frame loop. Crouch-toggle resolution stays in the input layer
(`resolve_crouch_intent`), producing the `crouch_intent` bit in the command.

### Task 3: Define the headless `simulate` seam + determinism harness (test first)
Define a single entry — working name `sim::simulate_tick` — that advances exactly one
game-logic tick from `(game state bundle, resolved input command, dt)` and returns the
tick outcome (fired-event names + the enqueued `system_commands` for the host to drain).
It runs: `snapshot_transforms` → movement → weapon-fire → death-sweep → event firing →
system-command **enqueue** (never dispatch). It touches **no** renderer/audio/window/
input-system. The host caller does the render-side post-tick work (camera-follow,
`frame_timing.push_state`) and drains the returned `system_commands` with the
audio/gamepad/UI arms gated; a future headless server applies only the game-state arms.
**Write the determinism test first** (extending the existing movement-test harness
pattern — build a registry, spawn a player + weapon, a `CollisionWorld` from a parry3d
`TriMesh`, a `HitZoneStore`, and a recorded input-command stream) and do not call the
seam done until it is green and stays green across repeated runs (same input + initial
state → identical output within epsilon). The seam's exact bundle/struct shapes are the
implementer's call (state the constraint: no render/window types in the signature).

### Task 4 (spike): cross-arch divergence measurement
Run the seam over a recorded input stream twice under conditions that force `f32`
divergence — forced intermediate rounding (always available, single-machine) and/or a
second CI architecture (e.g. an ARM runner) if one is available — and **record** the
per-axis / total position divergence over N ticks. Report it as a measured finding with a
recommended Phase 3 reconciliation tolerance. Not a pass/fail gate. Forced-rounding is the
baseline method; a real second arch strengthens the result if a runner exists.

### Task 5 (spike): throwaway predict/reconcile feel-prototype
Stand up a minimal in-process two-sim loop (a "client" that predicts locally from the
input stream and a "server" that is authoritative, both calling the seam — no transport)
that applies a basic rewind-to-acked-tick + replay reconciliation, and record an
empirical read on whether basic reconciliation feels acceptable (and how the dash case
behaves). **Throwaway** — a dev-tools-gated harness or an `#[ignore]`-able test, not wired
into the engine path; its value is the recorded read, deleted or parked after.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — independent files (`movement/` vs. `main.rs`);
both behavior-preserving splits. Task 2 calls the unchanged public `movement::tick`.
**Phase 2 (sequential):** Task 3 — consumes Task 2's extracted `sim` module; defines the
headless seam and its determinism gate.
**Phase 3 (concurrent):** Task 4, Task 5 — both consume the Task 3 seam; independent spikes.

## Rough sketch

**Module layout.** `movement/` becomes a directory (`mod.rs` + `substrate.rs` +
`intents.rs` + `dispatch.rs`, tests co-located or sibling). The tick assembly lands in a
new `sim` module (`crates/postretro/src/sim/` or `sim.rs`); `simulate_tick` is its entry.
The frame loop in `main.rs` shrinks to: build the input command → call `simulate_tick` →
apply render-side post-tick steps (camera-follow, `push_state`) → drain returned
`system_commands` with audio/gamepad/UI gated.

**The seam contract.** Input: a game-state bundle (`Rc<RefCell<EntityRegistry>>` +
`ScriptCtx` handles, `&CollisionWorld`, `&HitZoneStore`, gravity, `active_wieldable`,
`anim_time`, `&mut ProgressTracker`, the reaction/sequence/system registries) + a resolved
per-tick command (the existing `MovementInput` — `wish_dir`, jump, dash edge, running,
`crouch_intent`, `facing_yaw` — plus the weapon fire intent and aim ray) + `dt`. Output:
fired-event names + the enqueued `system_commands`. The command is exactly what Phases 2–3
will network; building it as a first-class struct now (sampled from the local snapshot +
camera on the host) is the seam's forward-looking shape. `Camera` is already GPU-free
(`research.md`), so aim/facing enter as data — this is plumbing, not a GPU decoupling.

**Determinism test.** Extend the movement-test harness pattern (no App/window/GPU): a
recorded input-command stream + a fixed `DT = 1.0/60.0` + a small parry3d `TriMesh` world,
ticked N times; assert run-to-run identical outcome within epsilon (never exact-float
equality — `testing_guide.md` §3). A `proptest` variant ("same input + state → same output
for any valid stream") guards the determinism contract.

**Cross-arch spike.** Forced intermediate rounding (quantize intermediate `f32`s to a
coarser grid between sub-steps to simulate cross-ISA rounding divergence) is the portable
method; record the divergence the same harness produces with and without it. If an ARM CI
runner exists, run the identical recorded stream on both and diff the final state.

**Feel spike.** Two `simulate_tick` instances over one input stream, client predicting +
reconciling against the server's delayed authoritative state, with an injected RTT. The
output is a recorded judgment + the dash-correction observation, feeding Phase 3.

## Open questions

- **System-command return vs. capability flag.** The seam enqueues `system_commands` and
  the host drains-with-gating (recommended — keeps audio/UI ownership host-side) rather
  than the seam taking an `is_headless` flag. Confirm at implementation.
- **`sim` module vs. `main.rs`-local.** Whether the extracted assembly is a sibling module
  (`sim/`) or stays a `main.rs` submodule; sibling is cleaner for the eventual headless
  server but is an implementer call.
- **Cross-arch method availability.** Whether CI offers a second architecture; if not,
  forced-rounding alone carries the finding (and the spec accepts that).
- **Feel-prototype lifetime.** Parked behind `#[ignore]` vs. deleted after the read —
  decide at the spike; it must not ship into the engine path either way.
