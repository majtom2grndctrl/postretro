# movement--state-machine

## Goal

Split the monolithic player-movement tick into a shared physics substrate plus pluggable, mutually-exclusive movement states. Ship the velocity-impulse states (dash/dodge, air-dash, double-jump) on the new seam. Establish the architecture every later `movement--*` spec (crouch, slide, wall-run, vault) plugs into, and decide how mod authors tune movement.

First in a planned `movement--*` series. Not a roadmap milestone — a self-contained, shippable increment.

## Scope

### In scope
- A movement state machine: an explicit current-state value on the player movement component, plus per-state "velocity intent" logic and declarative transitions.
- Extracting the existing collision/integration half of `crate::movement::tick` into a shared substrate that runs regardless of state — moved intact, not redesigned.
- Refactoring today's walk/run/jump/air-control into a `Normal` state expressed on the seam, behavior-identical to current movement.
- Dash/dodge: a directional velocity impulse with a cooldown.
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

**D3 — Dash spans a rigid↔fluid range via orthogonal knobs, not a single fixed behavior.** A dedicated `Dash` state, parameterized by three orthogonal behavioral knobs, lets an author place the dash anywhere from fully fluid (momentum-chaining, Ultrakill-style) to fully rigid (deterministic, frame-perfect repeatable, Neon-White-style) — while staying FPS-shaped.

Rationale: the three knobs are precisely the three sources of nondeterminism in a dash. `momentumRetention` controls **entry composition** (dependence on entry velocity): at 0 the dash replaces prior horizontal velocity, at 1 it is fully additive. `steerControl` controls **in-dash input authority** (dependence on mid-dash input): at 0 the dash is committed, at 1 full steering. `dashDrag` controls the **decay source** (dependence on friction context): at 0 the added velocity decays through `Normal`'s contextual ground/air friction, above 0 it decays at its own constant rate. The two reference corners: fluid (Ultrakill) is `momentumRetention ≈ 1`, `steerControl ≈ 0.3`, `dashDrag = 0`; rigid/deterministic (Neon White) is `momentumRetention = 0`, `steerControl = 0`, `dashDrag > 0` — bit-exact repeatable regardless of entry state. The deterministic corner is the conjunction of the three rigid extremes; nothing forces an author there, and points in between are valid. This is FPS-shaped, not a general physics sandbox: gravity still runs during the dash, the jump/air-jump branch stays omitted, and the state stays bounded by the hardcoded `DASH_MAX_MS` engine constant so it cannot linger. A dedicated state (over folding the knobs into `Normal`) is chosen so a future dodge can hang an i-frame / control-lockout window on the `Dash` variant without reshaping callers. parry3d resolves the resulting velocity against geometry only (sweep-and-slide) — no dynamics solver; Rapier is not a dependency and physics-engine integration is a non-goal (`entity_model.md` §9).

## Acceptance criteria

### Automated (test-gated)
- [ ] The full existing movement regression suite passes unchanged: walk-advances, jump-launch, step-up traversal, wall-slide, no-orbital-jitter, run-vs-walk steady-state, airborne run cap. (Regression gate proving the substrate was lifted cleanly.)
- [ ] Fluid corner: with `momentumRetention=1` and `dashDrag=0`, a dash while already running stacks — peak horizontal speed exceeds a standing dash (additive momentum) — and then decays through `Normal`'s friction back into its steady band, at which point control returns to `Normal`.
- [ ] Rigid corner: with `momentumRetention=0`, `steerControl=0`, and `dashDrag>0`, the dash outcome (peak speed and decay curve) is identical regardless of entry velocity — bit-exact repeatability across differing entry states.
- [ ] `steerControl`: at 0 input does not alter the dash trajectory mid-dash (committed); at >0 input steers it (one AC capturing the committed-vs-steerable contrast).
- [ ] `momentumRetention`: at 0 the dash replaces prior horizontal velocity (outcome independent of entry velocity); at 1 it adds to prior horizontal velocity.
- [ ] The `Dash` state cannot persist past the `DASH_MAX_MS` guard even if momentum stays high.
- [ ] A dash requested while the cooldown is active is suppressed (no second impulse until cooldown elapses).
- [ ] Air-dash fires while airborne up to the configured air-dash budget; the budget is exhausted after that many airborne dashes and is restored on landing.
- [ ] Double-jump: with an air-jump budget of ≥1, a second jump fires while airborne under the existing ceiling rule; the air-jump budget is restored on landing.
- [ ] An absent `dash` sub-object is valid and disables dash (no impulse; the `Normal`→`Dash` transition never fires). When `dash` is present, all its fields are required (present-then-all-required, like `ground`/`air`/`fall`).
- [ ] Descriptor parsers reject invalid dash fields symmetrically in both the JS and Luau paths, with matching field-path error text: `boostSpeed` rejects missing/non-finite/non-positive (zero and negative both rejected); `momentumRetention` and `steerControl` reject missing/non-finite/out-of-`[0,1]`; `dashDrag` and `cooldownMs` reject missing/non-finite/negative (zero allowed); `airDashes` rejects negative/non-integer; `preserveVertical` rejects non-bool.
- [ ] The SDK type-drift test passes with the new dash descriptor type present in `sdk/types/postretro.d.ts` and `.d.luau`.
- [ ] The `Dash` action is registered in the gameplay action set with default keyboard and gamepad bindings, and `input/defaults.rs`'s exhaustive `all_actions()` list and per-Action coverage tests are updated to include it.
- [ ] On dash entry, `preserveVertical=false` zeroes vertical velocity and `preserveVertical=true` retains it (gravity then resumes normally) — verified for an airborne dash.

### Manual-visual (no automated verification — eyeball in-engine)
- [ ] Dash reads as a fast burst, not a teleport; the camera tracks it without a visible hitch.
- [ ] Sprint → dash → jump and dash → air-dash chains feel continuous, with no dead frame on state hand-off.

## Tasks

### Task 1: Extract the shared integration substrate
Lift the collision/integration half of `tick` — the sweep-and-slide loop, step-up probe, floor-push budget, stuck-stop corner-wedge mitigation, ground-stick snap, and ground-state/landing resolution — into a standalone function that takes a desired velocity and current state and returns the new position plus contact/landing results. Move it intact: same constants, same ordering, same outputs. No behavior change. This is pure extraction and is the foundation everything else builds on.

### Task 2: Introduce the state enum and the `Normal` state
Add a movement-state value to the player movement component (default `Normal`). Move the velocity-intent half of `tick` (gravity, jump/air-jump, `pm_accelerate`, ground friction, air cap) into a `Normal`-state intent step. Rewire `tick` to: read current state → run that state's intent against `component.velocity` using last tick's grounded flag → call the Task 1 substrate → emit events → apply any transition. The regression suite is the gate (D2). Note: transition gating reads the same last-tick grounded flag the intent uses; the one-tick staleness is consistent with how `Normal` jump/air-jump already gate on `is_grounded` carried from last tick — no fresh ground probe before applying a transition.

Also establish a single landing-refresh point on the component that resets every ability budget on landing — today only `air_jumps_remaining` (the existing reset moves here, unchanged). Later budgets (the air-dash charge in Task 4) hook the same point rather than adding parallel reset code, so all ability budgets are refreshed uniformly.

### Task 3: Dash descriptor + SDK surface
Add a dash tuning sub-descriptor to the player movement descriptor with the seven-field set: impulse magnitude (`boostSpeed`), entry composition (`momentumRetention`), in-dash steering (`steerControl`), decay rate (`dashDrag`), cooldown (`cooldownMs`), air-dash count (`airDashes`), and whether vertical velocity is preserved on dash entry (`preserveVertical`). `movement.dash` is OPTIONAL on `PlayerMovementDescriptor`: when absent, dash is disabled (no `DashParams` materialized on the component; the `Normal`→`Dash` transition never fires). When present, all its fields are required — the same present-then-all-required discipline `ground`/`air`/`fall` follow. Parse it in both the JS and Luau descriptor paths with the same validation discipline as existing movement fields (finite, required-when-present). Pin the float validators per field: `boostSpeed` uses the strict positive validator (`validate_positive_finite`, the "must be a finite value > 0.0" helper near line 800 in `data_descriptors.rs`) — a zero impulse is semantically meaningless, exactly as `falloff`/`lightRange` are `(0, +∞)` in the fog validation table; `momentumRetention` and `steerControl` use the `[0,1]` range validator (`validate_in_range_finite` with `min=0.0, max=1.0`, the "finite value in [min, max]" helper near line 818 in `data_descriptors.rs`); `dashDrag` and `cooldownMs` use `validate_non_negative_finite` (near line 809), since `0` is a legitimate value for each (`dashDrag = 0` selects `Normal`-friction decay, `cooldownMs = 0` is a legitimate no-cooldown choice). For `airDashes` specifically: use `get_required_u32_js` on the JS side; use the inline float→fract→u32 check mirroring `movement.air.jumps` (lines 700–709 in `data_descriptors.rs`) on the Luau side. The shared error-text format for both paths is: `` `movement.dash.airDashes` must be a non-negative integer ``. Register the new type for SDK emission and update the committed `.d.ts`/`.d.luau` so the drift test passes. Full steps for the SDK surface: (a) add the `DashParams` type body (all seven fields) and `dash` field on `PlayerMovementDescriptor` to the verbatim TS static block and verbatim Luau static block in `typedef.rs` (source: `sdk/lib/data_script.ts` / `.luau`); (b) add `"DashParams" => "DashParams".to_string()` to both the TS and Luau type-name maps in `typedef.rs` (as `SpeedParams` is), or the generator emits it unresolved; (c) update the `EXPECTED_TS` and `EXPECTED_LUAU` constants in `typedef.rs`'s test module to match; (d) regenerate the committed `.d.ts`/`.d.luau` and confirm the `committed_sdk_types_match_current_registry` drift test passes. Pure data-surface work; see Boundary inventory for naming.

### Task 4: Dash + air-dash state
Add a `Dash` variant to the state enum carrying an elapsed-time guard against `DASH_MAX_MS`. The `Dash` intent reads all four behavioral knobs from the materialized `DashParams`. On entry, blend horizontal velocity per `momentum_retention`: horizontal `velocity = momentum_retention × horizontal_v_prior + dash_direction × boost_speed`, where `dash_direction` is the player's input `wish_dir` when it is non-zero, falling back to the `facing_yaw` direction when there is no movement input (modern-shooter convention, feel-first). At `momentum_retention = 0` the dash replaces prior horizontal velocity (outcome independent of entry velocity); at `1` it is fully additive (stacks on current momentum). `preserve_vertical` is applied ONCE on entry: when `preserve_vertical == false`, zero `component.velocity.y`; when `true`, keep the entering `velocity.y`. After entry, the `Dash` intent runs gravity NORMALLY and OMITS the jump/air-jump branch. It applies `pm_accelerate` (input steering) scaled by `steer_control` — at `steer_control = 0` the term is omitted entirely (a committed dash, no steering), at `1` it is `Normal`'s full `pm_accelerate`. Horizontal decay is selected by `dash_drag`: when `dash_drag == 0`, the added velocity decays through `Normal`'s existing ground/air friction/drag step (the same one `Normal` uses — fast on ground, slow in air, contextual); when `dash_drag > 0`, the dash's added velocity decays at that constant rate instead, overriding `Normal`'s friction for the added velocity during the `Dash` state (decoupled from friction context, deterministic). Cooldown + air-dash budget cap the stacking. Transitions: `Normal`→`Dash` on the dash input edge when cooldown is ready and — if airborne — an air-dash charge remains; `Dash`→`Normal` when the added velocity has decayed back into `Normal`'s steady band — horizontal speed at or below `Normal`'s steady cap for the current grounded state (run speed when grounded, air cap when airborne) — OR when the `DASH_MAX_MS` elapsed-time guard fires, whichever comes first. `DASH_MAX_MS` is a hardcoded-but-seamed engine `const` (it only bounds the state so it cannot linger), NOT a descriptor field. When no `DashParams` is materialized on the component (descriptor omitted `movement.dash`, Task 3), dash is disabled: the `Normal`→`Dash` transition never fires and no impulse occurs, regardless of input. Consume-rule: a grounded dash is gated by cooldown only and consumes no air-dash charge; an airborne dash requires both cooldown ready AND a remaining air-dash charge, and consumes one charge; cooldown applies to every dash. The grounded/airborne classification for the consume-rule uses the same last-tick `is_grounded` flag as the intent step (consistent with Task 2's stated staleness); this one-tick margin is acceptable for air-charge accounting — the same tradeoff as jump's ceiling gate. Timer ownership: the per-dash elapsed-time guard lives in the `Dash` variant (scoped to the active dash); the cooldown timer and air-dash charge counter (`air_dashes_remaining: u32`, distinct from the descriptor max `air_dashes: u32`) live on the component (they persist across states and reset on landing). Add cooldown + air-dash-charge timers to the component; hook the air-dash charge into the Task 2 landing-refresh point (do not add a parallel reset); on landing, when `dash` is `Some`, set `air_dashes_remaining = dash.air_dashes`, and when `None` the counter is irrelevant (dash disabled). The cooldown timer decrements each tick. Add a `Dash` action to the input action set with default keyboard/gamepad bindings. Note: adding `Action::Dash` requires updating `input/defaults.rs`'s `all_actions()` list and binding-coverage tests, or they will fail. Adding `dash_pressed: bool` to `MovementInput` will break every `MovementInput { .. }` struct literal in `movement/mod.rs`'s test module (~20 sites); update all of them to include `dash_pressed: false`. "Passes unchanged" (D2) means behavior-unchanged, not source-unchanged — test source may be mechanically updated to compile.

### Task 5: Formalize double-jump under the budget model
Double-jump already functions via the air-jump path (air-jump count + ceiling rule, budget reset on landing). Consolidate it as a named ability inside the `Normal` airborne intent whose air-jump budget refreshes via the Task 2 landing-refresh point, so it and the air-dash budget (Task 4) reset through one uniform mechanism. Net-new behavior is minimal; the work is consolidation plus explicit test coverage. The canonical test descriptor has `air.jumps = 0` and no existing test exercises the air-jump path; a new test fixture with `air.jumps ≥ 1` and a finite `air.jump_ceiling` must be authored as part of this task. Do not change the existing ceiling/velocity semantics.

## Sequencing

**Phase 1 (sequential):** Task 1 — substrate extraction blocks everything; the seam can't exist until the integration half is callable on its own.
**Phase 2 (sequential):** Task 2 — consumes the Task 1 substrate; establishes the enum and the regression gate before any new state is added.
**Phase 3 (concurrent):** Task 3 (descriptor/SDK data surface) ∥ Task 5 (double-jump consolidation inside `Normal`) — independent: one touches the wire/SDK layer, the other touches `Normal` airborne intent.
**Phase 4 (sequential):** Task 4 — consumes the dash descriptor from Task 3 and the state seam from Task 2.

## Rough sketch

Current `crate::movement::tick` (`crates/postretro/src/movement/mod.rs`) already partitions cleanly:
- **Intent half** (steps 1–6): gravity, jump/air-jump (`air.jumps`, `air.jump_ceiling`, `air_jumps_remaining`), `pm_accelerate`, ground friction, airborne cap. Operates on `component.velocity` and reads `component.is_grounded` carried from last tick. → becomes `Normal` intent.
- **Substrate half** (steps 7–8): the `for _ in 0..4` sweep-and-slide loop, `step_up_lift`, `floor_push_remaining`, stuck-stop, ground-stick down-cast, `is_grounded`/`air_ticks` reset and landing/contact resolution (results, not event emission). → becomes the shared substrate (Task 1). The tick maps those results to `MovementEvents` after calling the substrate.

Seam shape: a `MovementState` enum on `PlayerMovementComponent` (`Normal`, `Dash { .. }`; later `Crouching`/`Sliding`/`WallRunning`/`Vaulting`). `tick` dispatches to a per-state intent fn, then calls the substrate. Transitions are returned by the intent step as an optional next-state, applied after integration. Keep all state in Rust on the component.

`MovementInput` (today: `wish_dir`, `jump_pressed`, `facing_yaw`, `running`) gains `dash_pressed: bool`, a true rising-edge signal: `run_movement_tick` sets it via `matches!(snapshot.button(Action::Dash), ButtonState::Pressed)` — only `Pressed` fires, not `Held`. This intentionally differs from `jump_pressed`, which uses `snapshot.button(Action::Jump).is_active()` (a level signal, `Pressed|Held`); jump self-gates via the ceiling rule so a held button is harmless, but a held dash button would re-fire every tick the cooldown is ready, making a true rising edge mandatory. `MovementEvents` (today: `landed`, `jumped`) may gain a `dashed` flag (deferred — see Open questions); the `main.rs` `run_movement_tick` event mapping (`landed`/`jumped` → reaction event strings) extends the same way.

Ability budgets: `air_jumps_remaining` already resets on landing in the ground-state branch. Add an air-dash charge counter reset at the same point; add a dash cooldown timer decremented each tick. The materialized `DashParams` are stored on `PlayerMovementComponent` as `Option<DashParams>` alongside the existing capsule/ground/air/fall params — `None` when the descriptor omits `movement.dash`; the `Dash` intent reads them from the component, mirroring how `Normal` reads `ground`/`air`. On entry the `Dash` intent blends horizontal velocity (`momentum_retention × horizontal_v_prior + dash_direction × boost_speed`, vertical zeroed or kept per `preserve_vertical`). During the state it runs gravity normally, applies `pm_accelerate` scaled by `steer_control` (omitted at 0, a committed dash), omits the jump branch, and decays the added velocity through `Normal`'s friction when `dash_drag == 0` else at the constant `dash_drag` rate — exiting when the added velocity returns into `Normal`'s steady band, bounded by the `DASH_MAX_MS` engine const.

double-jump is functional today: set the descriptor air-jump count ≥1 and the airborne jump branch fires while `velocity.y <= air.jump_ceiling`. Task 5 is consolidation, not new mechanics.

## Boundary inventory

Dash tuning crosses Rust ↔ wire (JS/Lua object) ↔ TS ↔ Luau. Field names are camelCase on every script-facing side per the scripting naming convention; Rust uses snake_case. No FGD KVP, no PRL/binary section (descriptor is a script object, not baked data).

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| dash sub-descriptor (optional) | `Option<DashParams>` | optional nested object under `movement` | `dash?: DashParams` | `dash?` | n/a |
| impulse magnitude | `boost_speed: f32` | `boostSpeed` | `boostSpeed` | `boostSpeed` | n/a |
| entry composition | `momentum_retention: f32` | `momentumRetention` | `momentumRetention` | `momentumRetention` | n/a |
| in-dash steering | `steer_control: f32` | `steerControl` | `steerControl` | `steerControl` | n/a |
| decay rate | `dash_drag: f32` | `dashDrag` | `dashDrag` | `dashDrag` | n/a |
| cooldown | `cooldown_ms: f32` | `cooldownMs` | `cooldownMs` | `cooldownMs` | n/a |
| air-dash budget | `air_dashes: u32` | `airDashes` | `airDashes` | `airDashes` | n/a |
| keep vertical vel | `preserve_vertical: bool` | `preserveVertical` | `preserveVertical` | `preserveVertical` | n/a |
| dash input action | `Action::Dash` | n/a (input layer) | n/a | n/a | n/a |
| live air-dash charge (component) | `air_dashes_remaining: u32` | n/a (runtime state) | n/a | n/a | n/a |

The field set is fixed by Task 3 and the table above; this note pins the casing. Any additional field an implementer finds necessary must follow the same casing rule. The `?` on the `dash` row marks the whole sub-object optional (`dash?: DashParams` in the TS body, `dash?` in Luau, `Option<DashParams>` in Rust): absent means dash disabled; present means all inner fields required.

**Wire-casing mechanism:** the movement descriptor sub-structs carry no `#[serde(rename_all)]`; wire camelCase is enforced by the hand-written JS/Luau parsers and verbatim static type blocks in `typedef.rs` — NOT by serde and NOT by `register_type().field()`. Movement descriptor types are defined as verbatim TS/Luau text in `typedef.rs` (sourced from `sdk/lib/data_script.ts` / `.luau`), not generated via `register_type().field()`. Adding `DashParams` requires hand-editing those verbatim blocks, not calling `register_type`. Author each Dash wire key (`boostSpeed`, `momentumRetention`, `steerControl`, `dashDrag`, `cooldownMs`, `airDashes`, `preserveVertical`) literally in: (1) the JS parser in `data_descriptors.rs`, (2) the Luau parser in `data_descriptors.rs`, (3) the verbatim TS static block in `typedef.rs`, (4) the verbatim Luau static block in `typedef.rs`, (5) the `EXPECTED_TS`/`EXPECTED_LUAU` test constants in `typedef.rs`'s test module.

## Open questions
- **How far does declarative composition eventually go?** D1 commits to declarative tuning for v1 and defers imperative per-tick script states. The boundary between "tunable parameters" and "author-defined transition rules as data" will firm up across the later `movement--*` specs — flagged so the seam isn't accidentally narrowed.
- **Dash direction source:** Resolved: dash along input `wish_dir` when it is non-zero, falling back to the `facing_yaw` direction when there is no movement input (modern-shooter convention, feel-first). Stated in Task 4.
- **Should `dashed` become a reaction event?** Resolved: do NOT add `dashed` to `MovementEvents` in this spec. Defer until a consumer exists. Task 4 emits no dash event. If a consumer is added later, `MovementEvents` can gain `dashed` then (same pattern as the deferred option noted in the Rough sketch).
