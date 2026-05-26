# movement--state-machine

## Goal

Split the monolithic player-movement tick into a shared physics substrate plus pluggable, mutually-exclusive movement states. Ship the velocity-impulse states (dash/dodge, air-dash, double-jump) on the new seam. Establish the architecture every later `movement--*` spec (crouch, slide, wall-run, vault) plugs into, and decide how mod authors tune movement.

First in a planned `movement--*` series. Not a roadmap milestone — a self-contained, shippable increment.

## Scope

### In scope
- A movement state machine: an explicit current-state value on the player movement component, plus per-state "velocity intent" logic and declarative transitions.
- Extracting the existing collision/integration half of `crate::movement::tick` into a shared substrate that runs regardless of state — moved intact, not redesigned.
- Refactoring today's walk/run/jump/air-control into a `Normal` state expressed on the seam, behavior-identical to current movement.
- Dash/dodge: a directional velocity burst with a cooldown.
- Air-dash: dash while airborne, gated by a per-airtime budget that refreshes on landing.
- Double-jump: formalize the already-functional `air.jumps` path as a state-machine ability under the shared budget model.
- A new `Dash` input action + default bindings.
- Descriptor + SDK surface for the new tuning (dash params), including type emission and drift-test coverage.

### Out of scope (later `movement--*` specs)
- Crouch, slide (capsule resize, stand-up ceiling probe).
- Wall-running, vaulting (environment-probe states).
- Grapple (constraint physics + renderer rope + aiming — separate future draft).
- Imperative script-driven movement states (per-tick author callbacks). See Decision D1 — the seam is shaped to allow this later; it is not built here.
- Any change to the collision substrate's behavior. It moves; it does not change.

## Decisions

**D1 — Author surface: declarative descriptor, not per-tick script primitives.** States live natively in Rust; authors tune them through descriptor fields and (future) declarative transitions, not by composing per-tick imperative primitives in script.

Rationale: movement runs every tick in the fixed game-logic step (update order 1, before camera follow — `entity_model.md` §5). Driving state logic through QuickJS/Luau per tick adds FFI cost and determinism risk on the hottest game-logic path. It also contradicts the standing invariant that movement is engine-internal: scripts cannot read or write `PlayerMovement` via `worldQuery` today (`entity_model.md` §7b). A declarative surface keeps movement deterministic and fast while still giving authors real control (which states exist, their tuning, their transition triggers).

The state-machine seam (a state enum + per-state intent functions behind one dispatch point) is shaped so a future script-driven path can resolve behind it without reshaping callers — but that path is explicitly out of scope here. The "first principles / primitives" vision is honored as *declarative* primitives (states + transitions as data), not imperative ones.

**D2 — `Normal` is the behavior baseline.** The existing movement regression suite is the gate: it must pass unchanged after the refactor. No behavior delta is acceptable in `Normal`.

## Acceptance criteria

### Automated (test-gated)
- [ ] The full existing movement regression suite passes unchanged: walk-advances, jump-launch, step-up traversal, wall-slide, no-orbital-jitter, run-vs-walk steady-state, airborne run cap. (Regression gate proving the substrate was lifted cleanly.)
- [ ] Dash from rest produces a burst that reaches the configured dash speed in the input direction within the configured duration, then returns control to `Normal`.
- [ ] A dash requested while the cooldown is active is suppressed (no second burst until cooldown elapses).
- [ ] Air-dash fires while airborne up to the configured air-dash budget; the budget is exhausted after that many airborne dashes and is restored on landing.
- [ ] Double-jump: with an air-jump budget of ≥1, a second jump fires while airborne under the existing ceiling rule; the air-jump budget is restored on landing.
- [ ] Descriptor parsers reject a missing or non-finite/negative dash field symmetrically in both the JS and Luau paths, with matching field-path error text.
- [ ] The SDK type-drift test passes with the new dash descriptor type present in `sdk/types/postretro.d.ts` and `.d.luau`.

### Manual-visual (no automated verification — eyeball in-engine)
- [ ] Dash reads as a fast burst, not a teleport; the camera tracks it without a visible hitch.
- [ ] Sprint → dash → jump and dash → air-dash chains feel continuous, with no dead frame on state hand-off.

## Tasks

### Task 1: Extract the shared integration substrate
Lift the collision/integration half of `tick` — the sweep-and-slide loop, step-up probe, floor-push budget, stuck-stop corner-wedge mitigation, ground-stick snap, and ground-state/landing resolution — into a standalone function that takes a desired velocity and current state and returns the new position plus contact/landing results. Move it intact: same constants, same ordering, same outputs. No behavior change. This is pure extraction and is the foundation everything else builds on.

### Task 2: Introduce the state enum and the `Normal` state
Add a movement-state value to the player movement component (default `Normal`). Move the velocity-intent half of `tick` (gravity, jump/air-jump, `pm_accelerate`, ground friction, air cap) into a `Normal`-state intent step. Rewire `tick` to: read current state → run that state's intent against `component.velocity` using last tick's grounded flag → call the Task 1 substrate → emit events → apply any transition. The regression suite is the gate (D2).

### Task 3: Dash descriptor + SDK surface
Add a dash tuning sub-descriptor to the player movement descriptor (speed, duration, cooldown, air-dash count, and whether vertical velocity is preserved during a dash). Parse it in both the JS and Luau descriptor paths with the same validation discipline as existing movement fields (finite, non-negative, required-when-present). Register the new type for SDK emission and update the committed `.d.ts`/`.d.luau` so the drift test passes. Pure data-surface work; see Boundary inventory for naming.

### Task 4: Dash + air-dash state
Add a `Dash` variant to the state enum carrying its live timers (remaining duration). Implement its intent (set velocity to the dash burst in the input/facing direction; suspend gravity/accel/friction for the dash window per the descriptor) and its transitions (`Normal`→`Dash` on the dash input edge when cooldown is ready and — if airborne — an air-dash charge remains; `Dash`→`Normal` when the duration expires). Add cooldown + air-dash-charge timers to the component, refresh charges on landing alongside the existing air-jump reset. Add a `Dash` action to the input action set with default keyboard/gamepad bindings.

### Task 5: Formalize double-jump under the budget model
Double-jump already functions via the air-jump path (air-jump count + ceiling rule, budget reset on landing). Consolidate it as a named ability inside the `Normal` airborne intent, sharing the landing-refresh bookkeeping with the air-dash budget so the two ability budgets are managed uniformly. Net-new behavior is minimal; the work is consolidation plus explicit test coverage. Do not change the existing ceiling/velocity semantics.

## Sequencing

**Phase 1 (sequential):** Task 1 — substrate extraction blocks everything; the seam can't exist until the integration half is callable on its own.
**Phase 2 (sequential):** Task 2 — consumes the Task 1 substrate; establishes the enum and the regression gate before any new state is added.
**Phase 3 (concurrent):** Task 3 (descriptor/SDK data surface) ∥ Task 5 (double-jump consolidation inside `Normal`) — independent: one touches the wire/SDK layer, the other touches `Normal` airborne intent.
**Phase 4 (sequential):** Task 4 — consumes the dash descriptor from Task 3 and the state seam from Task 2.

## Rough sketch

Current `crate::movement::tick` (`crates/postretro/src/movement/mod.rs`) already partitions cleanly:
- **Intent half** (steps 1–6): gravity, jump/air-jump (`air.jumps`, `air.jump_ceiling`, `air_jumps_remaining`), `pm_accelerate`, ground friction, airborne cap. Operates on `component.velocity` and reads `component.is_grounded` carried from last tick. → becomes `Normal` intent.
- **Substrate half** (steps 7–8): the `for _ in 0..4` sweep-and-slide loop, `step_up_lift`, `floor_push_remaining`, stuck-stop, ground-stick down-cast, `is_grounded`/`air_ticks` reset and `landed`/`jumped` event resolution. → becomes the shared substrate (Task 1).

Seam shape: a `MovementState` enum on `PlayerMovementComponent` (`Normal`, `Dash { .. }`; later `Crouching`/`Sliding`/`WallRunning`/`Vaulting`). `tick` dispatches to a per-state intent fn, then calls the substrate. Transitions are returned by the intent step as an optional next-state, applied after integration. Keep all state in Rust on the component.

`MovementInput` (today: `wish_dir`, `jump_pressed`, `facing_yaw`, `running`) gains a dash edge input. `MovementEvents` (today: `landed`, `jumped`) may gain a `dashed` flag; the `main.rs` `run_movement_tick` event mapping (`landed`/`jumped` → reaction event strings) extends the same way.

Ability budgets: `air_jumps_remaining` already resets on landing in the ground-state branch. Add an air-dash charge counter reset at the same point; add a dash cooldown timer decremented each tick.

double-jump is functional today: set the descriptor air-jump count ≥1 and the airborne jump branch fires while `velocity.y <= air.jump_ceiling`. Task 5 is consolidation, not new mechanics.

## Boundary inventory

Dash tuning crosses Rust ↔ wire (JS/Lua object) ↔ TS ↔ Luau. Field names are camelCase on every script-facing side per the scripting naming convention; Rust uses snake_case. No FGD KVP, no PRL/binary section (descriptor is a script object, not baked data).

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| dash sub-descriptor | `DashParams` | nested object under `movement` | `dash` | `dash` | n/a |
| burst speed | `speed: f32` | `speed` | `speed` | `speed` | n/a |
| burst duration | `duration_ms: f32` | `durationMs` | `durationMs` | `durationMs` | n/a |
| cooldown | `cooldown_ms: f32` | `cooldownMs` | `cooldownMs` | `cooldownMs` | n/a |
| air-dash budget | `air_dashes: u32` | `airDashes` | `airDashes` | `airDashes` | n/a |
| keep vertical vel | `preserve_vertical: bool` | `preserveVertical` | `preserveVertical` | `preserveVertical` | n/a |
| dash input action | `Action::Dash` | n/a (input layer) | n/a | n/a | n/a |

(Exact `DashParams` field set is a constraint for the implementer to finalize during Task 3; the names above pin the casing once so it is consistent across boundaries.)

## Open questions
- **How far does declarative composition eventually go?** D1 commits to declarative tuning for v1 and defers imperative per-tick script states. The boundary between "tunable parameters" and "author-defined transition rules as data" will firm up across the later `movement--*` specs — flagged so the seam isn't accidentally narrowed.
- **Dash direction source:** input `wish_dir` when present, else facing? Most shooters dash along input and fall back to facing when there is no movement input. Implementer's call during Task 4 unless the manual-visual feel check rejects it.
- **Should `dashed` become a reaction event?** Cheap to add to `MovementEvents`; only worth wiring if a reaction consumer is planned. Defer until a consumer exists (matches the "audio events are illustrative, not yet consumed" stance in `entity_model.md` §5).
