# M15 Phase 0 — Headless simulation seam + determinism harness + spike

> Milestone 15 (multiplayer netcode), Phase 0. Design reference:
> `context/research/netcode/` (`index.md` Phase 0, `research.md` §6). Grounded code map:
> sibling `research.md`.

## Goal

Extract PostRetro's per-tick **core game-state advance** out of the render-interleaved
frame loop into a headless `simulate_tick` seam — one tick of `snapshot-transforms →
movement → weapon-fire → death-sweep`, returning the tick's event names — that runs with
**no wgpu/winit/renderer/audio/input dependency**. The seam is the per-tick body a server
and a client will both call (Phases 1+); the *caller* owns the multi-tick loop and the
render/host work around it. Prove it with a **determinism harness** (the honesty gate)
and, as a build-to-learn spike, **measure the cross-architecture `f32` divergence** that
sets the reconciliation tolerance and get an empirical **feel** read on basic
predict/reconcile. This is the foundation every later netcode phase rides; a leaky seam
here poisons reconciliation forever.

## Scope

### In scope
- **Behavior-preserving split** of `movement/mod.rs` (substrate / intent / dispatch /
  public API) and of `main.rs`'s game-tick assembly — *before* extending either.
- A headless `simulate_tick` seam covering the **core per-tick advance**: order 0
  transform-snapshot → order 1 movement → order 3 weapon-fire → death-sweep, returning the
  tick's **event names**. The seam neither fires events nor dispatches commands nor pushes
  interpolation state — those stay with the caller.
- **Severing the render/window reaches** from the seam: the *caller* owns the
  `for _ in 0..ticks` loop and runs camera-follow + `frame_timing.push_state` **per tick**;
  it accumulates the seam's returned event names and, **post-loop**, fires them and runs
  the existing `dispatch_system_commands` **unchanged** (preserving the accumulate-then-
  drain timing and the post-crossing re-dispatch).
- A **determinism harness + test written first** (recorded input stream → deterministic
  tick) with teeth: pinned N/pawns/epsilon, run-to-run identical, plus spawn-order
  invariance and a `proptest` over input streams.
- **Spike (build-to-learn):** measured `f32` divergence over N ticks under forced-
  divergence conditions; a throwaway predict/reconcile feel-prototype.

### Out of scope
- Any transport, wire format, serialization, or networking (Phase 1+).
- The **scripting-bridge stage** (order 4: emitter / particle sim / light / fog bridges,
  `entity_model.md` §5) — those run after the core tick and are partly render-collectors;
  bringing them under a seam is a later phase. A server's handling of order 4 is deferred.
- **Camera-follow (order 2)** stays render/caller-side; it is not in the seam.
- The **headless gating partition** of `dispatch_system_commands` (which command arms a
  no-renderer server may apply) — Phase 0 keeps the existing dispatch host-side, unchanged;
  partitioning it is a server-phase concern.
- Changing any movement, weapon, or death-sweep **behavior** — the splits and the seam are
  behavior-preserving (the spike's feel-prototype is throwaway, not shipped).
- A standalone headless-server binary / dedicated-server entry point (Phase 7).
- Hardening the divergence result into a pass/fail threshold — it is a recorded finding
  (`experimental_spikes.md`), not a gate.

## Acceptance criteria

- [ ] `movement/mod.rs` logic is split into cohesive substrate / intent / dispatch /
  public-API modules (alongside the existing `movement/{carry,scope}.rs`); the **movement
  test suite passes unchanged** and `movement::tick`'s public signature
  (`-> (Vec3, MovementEvents)`) is unchanged. (Task 1)
- [ ] The game-tick assembly is extracted from `main.rs` into a dedicated module the frame
  loop calls; the engine plays with **identical gameplay** including across a **multi-tick
  catch-up frame** (a frame hitch must not change interpolation or reaction timing), and
  the build + existing tests are green. (Task 2)
- [ ] **Honesty gate:** `simulate_tick` advances one tick (transform-snapshot → movement →
  weapon → death) and returns the tick's event names, **constructed and called with no
  renderer, audio, window, input-system, wgpu, or winit in scope** — demonstrated by a
  harness that builds the game state + a resolved input command and ticks it with no
  window/GPU context. Event firing, command dispatch, `push_state`, and camera-follow are
  not inside the seam. (Task 3)
- [ ] **Honesty gate (determinism, with teeth):** a test ticks a fixed pawn count from a
  recorded input stream over a pinned N ticks and asserts the outcome (positions,
  velocities, event names) is **identical within epsilon** (a) run-to-run, (b) when the
  same entities are spawned in a different order, and (c) across a `proptest` of randomized
  input streams. Green and stays green. (Task 3)
- [ ] **Measured finding:** same-input divergence over N ticks is **recorded** (a logged
  value) under forced-divergence conditions (forced-rounding baseline; a second
  architecture if a runner is available), with a recommended Phase 3 reconciliation
  tolerance. Not a pass/fail gate. (Task 4)
- [ ] **Measured finding:** a throwaway predict/reconcile feel-prototype runs and yields a
  **recorded read** on whether basic reconciliation feels acceptable; it is clearly marked
  throwaway and ships nothing into the engine path. (Task 5)

## Tasks

### Task 1: Split `movement/mod.rs` along the §4 seam
`movement/` is already a directory (`mod.rs` 6,055 lines incl. ~72 co-located tests,
`carry.rs`, `scope.rs`). Split `mod.rs`'s ~1,875 lines of logic into new sibling modules
following the `movement.md` §4 boundary: **substrate** (`pm_accelerate`,
`wish_dir_from_input`, `step_up_lift`, `integrate_collision`, `resize_capsule`,
`standup_clearance_probe`, the forgiveness/jump-edge helpers — the collide-and-slide
physics), **intents** (`normal_intent`, `dash_intent`, `crouching_intent`,
`try_enter_dash`, the IR `resolve_*` helpers), and **dispatch** (`dispatch_state_intent`,
boost/carry glue, stand-up helpers). `mod.rs` keeps the public API: `MovementInput`,
`MovementEvents`, `SubstrateResult`, `Transition`, and `pub(crate) fn tick` (re-using
`MovementState` from `scripting::components::player_movement`, where it is defined — do not
relocate it). Leave `carry.rs`/`scope.rs` as-is. Behavior-preserving — the public
`movement::tick` signature is untouched; the test block stays co-located or moves to a
sibling test module (exempt from the size rule). This is the split-before-extend for the
file Phase 3 will extend.

### Task 2: Extract the game-tick assembly from `main.rs`
Move the per-tick logic out of the `RedrawRequested` handler and the `run_movement_tick`
/ `run_weapon_fire_tick` / `run_death_sweep` `impl App` methods into a new engine module
(working name `sim`). The extracted per-tick function takes **explicit game-state
parameters** (the `Rc<RefCell<EntityRegistry>>` + `ScriptCtx` handles, `&CollisionWorld`,
`&HitZoneStore`, gravity, `active_wieldable`, `anim_time`, `&mut ProgressTracker`) and a
resolved per-tick **input command** — never `&mut App`. The inline input-intent resolution
(axes, jump/dash/crouch edges, `MovementInput` construction) moves with it. **The frame
loop keeps ownership of the `for _ in 0..ticks` loop** and, per tick, runs: build command →
`sim::simulate_tick` → camera-follow → `frame_timing.push_state`, accumulating the returned
event names; **post-loop** it fires the accumulated events and runs `dispatch_system_commands`
unchanged (the accumulate-then-drain timing and the post-crossing re-dispatch are
preserved). The **no-pawn fly-cam branch stays host-side**, run per tick before the seam
call when no pawn exists; its `up_axis`/`Action::MoveUp` input is host-only (fly-cam) and is
**not** part of the resolved command. Crouch-toggle resolution stays in the input layer
(`resolve_crouch_intent`, `main.rs:662`), producing the `crouch_intent` bit. For Phase 0,
the command carries the inputs `weapon::tick` needs (the `ActionSnapshot` and the
camera-derived aim ray) so `weapon::tick`'s signature is **unchanged** — resolving a
networked fire-intent is Phase 3's job, not this extraction's.

### Task 3: Define the headless `simulate_tick` seam + determinism harness (test first)
Define a single entry — working name `sim::simulate_tick` (no existing symbol collides) —
that advances exactly **one** game-logic tick from `(game-state bundle, resolved input
command, dt)` and **returns the tick's event names** (the union of what `run_movement_tick`
/ `run_weapon_fire_tick` / `run_death_sweep` return today). It runs `snapshot_transforms` →
movement → weapon-fire → death-sweep and touches **no** renderer/audio/window/input-system,
and **no** render/window types appear in its signature. It does not fire events, dispatch
commands, push interpolation, or follow the camera — those are the caller's (Task 2).
**Write the determinism test first** (extending the movement-test harness pattern — build a
registry, spawn a player + weapon, a `CollisionWorld` from a parry3d `TriMesh`, a
`HitZoneStore`, and a recorded input-command stream at `DT = 1.0/60.0`) and do not call the
seam done until the determinism AC holds: pinned N/pawns/epsilon, run-to-run identical,
spawn-order invariant, and a `proptest` over input streams — green and staying green. The
seam's exact bundle/struct shapes are the implementer's call (the constraint: no
render/window types in the signature; the command is the shape Phases 2–3 will network).

### Task 4 (spike): cross-arch divergence measurement
Run the seam over a recorded input stream twice under conditions that force `f32`
divergence — forced intermediate rounding (always available, single-machine) and, if a
runner exists, a second CI architecture (e.g. ARM) — and **record** the per-axis / total
position divergence over N ticks. Report it as a measured finding with a recommended Phase
3 reconciliation tolerance. Not a pass/fail gate; forced-rounding is the baseline method, a
real second arch strengthens it.

### Task 5 (spike): throwaway predict/reconcile feel-prototype
Stand up a minimal in-process two-sim loop (a "client" that predicts locally from the input
stream and a "server" that is authoritative, both calling the seam — no transport) that
applies a basic rewind-to-acked-tick + replay reconciliation, and record an empirical read
on whether basic reconciliation feels acceptable (and how the dash case behaves).
**Throwaway** — a dev-tools-gated harness or an `#[ignore]`-able test, not wired into the
engine path; its value is the recorded read, deleted or parked after.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — independent files (`movement/` vs. `main.rs`);
both behavior-preserving splits. Task 2 calls the unchanged public `movement::tick`.
**Phase 2 (sequential):** Task 3 — consumes Task 2's extracted `sim` module; defines the
headless seam and its determinism gate.
**Phase 3 (concurrent):** Task 4, Task 5 — both consume the Task 3 seam; independent spikes.

## Rough sketch

**Module layout.** `movement/` gains `substrate.rs` + `intents.rs` + `dispatch.rs` beside
the existing `mod.rs`/`carry.rs`/`scope.rs`. The tick assembly lands in a new `sim` module
(`crates/postretro/src/sim/` or `sim.rs`); `simulate_tick` is its entry. The frame loop in
`main.rs` keeps the `for _ in 0..ticks` loop, now: `[no pawn → host fly-cam]` build command
→ `simulate_tick` → camera-follow → `push_state` (per tick); then post-loop fire events +
`dispatch_system_commands` (unchanged).

**The seam contract.** Input: a game-state bundle (`Rc<RefCell<EntityRegistry>>` +
`ScriptCtx` handles, `&CollisionWorld`, `&HitZoneStore`, gravity, `active_wieldable`,
`anim_time`, `&mut ProgressTracker`) + a resolved per-tick command (the existing
`MovementInput` — `wish_dir`, jump, dash edge, running, `crouch_intent`, `facing_yaw` —
plus the `ActionSnapshot` and camera-derived aim ray weapon fire needs) + `dt`. Output: the
tick's event names. The command is exactly what Phases 2–3 will network; building it as a
first-class value now (sampled from the local snapshot + camera on the host) is the seam's
forward-looking shape. `Camera` is already GPU-free (`research.md`), so aim/facing enter as
data — this is plumbing, not a GPU decoupling.

**Why the caller owns the loop.** `push_state` (`frame_timing.rs:102`) shifts
current→previous **every tick**; camera-follow feeds the position it snapshots; events are
**accumulated across ticks and drained once** so reactions observe the settled post-tick
world (`main.rs` comment ~1891). Keeping all three per-tick/post-loop in the caller makes
the extraction behavior-identical on multi-tick catch-up frames — the determinism the seam
itself must not break.

**Determinism test.** Extend the movement-test harness pattern (no App/window/GPU): a
recorded input-command stream + fixed `DT = 1.0/60.0` + a small parry3d `TriMesh` world,
ticked N times. Assert outcome identical within epsilon (never exact-float equality —
`testing_guide.md` §3) run-to-run, **and** under a permuted spawn order (catches
iteration-order / `HashMap`-order nondeterminism, the real netcode risk a single-machine
re-run misses), **and** via a `proptest` over input streams.

**Cross-arch spike.** Forced intermediate rounding (quantize intermediate `f32`s to a
coarser grid between sub-steps to mimic cross-ISA rounding) is the portable method; record
the divergence with vs. without it. If an ARM CI runner exists, run the identical recorded
stream on both and diff the final state.

**Feel spike.** Two `simulate_tick` instances over one input stream, client predicting +
reconciling against the server's delayed authoritative state, with an injected RTT. Output:
a recorded judgment + the dash-correction observation, feeding Phase 3.

## Open questions

- **`sim` module vs. `main.rs`-local.** Sibling `sim/` is cleaner for the eventual headless
  server; an implementer call.
- **Cross-arch method availability.** Whether CI offers a second architecture; if not,
  forced-rounding alone carries the finding (the AC accepts that).
- **Feel-prototype lifetime.** Parked behind `#[ignore]` vs. deleted after the read — decide
  at the spike; it must not ship into the engine path either way.
