# Movement sustained FOV feel

## Goal

Replace the snap-like slide FOV effect with smooth, sustained FOV targets for
running and sliding. Let mod authors tune an absolute camera FOV and independent
enter/exit settle rates without changing movement simulation, reactions, or wire
state.

## Scope

### In scope

- Optional `movement.viewFeel.sustainedFov` data for run and slide targets.
- Absolute horizontal FOV targets in degrees, bounded by the engine camera band.
- Critically damped, monotonic target tracking with separate enter and exit
  settle rates.
- Render-local run activity derived from held sprint input, baseline movement
  state, and horizontal locomotion; retain it through a sprint-jump while those
  conditions hold.
- Slide target priority over run. Targets never add together.
- Add sustained FOV to the existing optional transition impulse before the world
  camera's final FOV clamp.
- Parser, SDK, docs, lifecycle, dev sample, automated, and manual coverage.
- Repair the existing own-key versus inherited-key mismatch in `impulse.states`
  while introducing the new closed state table.

### Out of scope

- A `Running` movement-state variant.
- Gameplay speed, collision, input, prediction, or wire-format changes.
- New reaction addresses or script callbacks.
- FOV changes for dash, crouch, or arbitrary movement speed.
- Viewmodel projection zoom.

## Decisions

| Topic | Decision |
|---|---|
| Author unit | `targetFov` is an absolute horizontal FOV in degrees. Authors choose the resulting view, not an offset relative to an engine default. |
| Active run | Baseline `Normal`, held Sprint, and horizontal speed above an engine-owned near-zero locomotion epsilon. Releasing Sprint removes the target even while momentum persists. |
| Airborne run | A sprint-jump retains run FOV while Sprint stays held and horizontal locomotion remains above the epsilon. |
| Arbitration | Slide overrides run. A slide exit retargets to run if run is active; targets never sum. |
| Timing | A target row's `enterTension` applies when selecting or retargeting to that row. The departing row's `exitTension` applies while returning to neutral. |
| Accents | Existing `impulse` remains optional and additive. Dev slide tuning removes its large FOV kick so the sustained target carries the body-state cue. |

## Acceptance criteria

- [ ] A descriptor may omit `sustainedFov`; omitted run/slide rows contribute no sustained FOV.
- [ ] A present row accepts only finite `targetFov` values in the shared 60°–130° horizontal-FOV band and finite enter/exit tensions in the existing 0.1–240/sec band. Both script runtimes reject the same malformed shape, range, unknown key, and inherited/non-own-key cases with an authored path.
- [ ] Held Sprint while stationary does not widen FOV. Baseline moving Sprint selects the run target; walking, crouching, and dashing do not.
- [ ] A sprint-jump remains at the run target while Sprint is held and horizontal locomotion continues; releasing Sprint returns toward neutral.
- [ ] Sliding selects only the slide target even while Sprint is held. On slide exit, the target moves directly to run when its activation condition holds, otherwise toward neutral.
- [ ] Entering, exiting, and retargeting are monotonic and frame-rate independent. Higher tension settles sooner and never overshoots the selected target.
- [ ] An optional transition impulse composes once with the sustained target before the final camera clamp. World projection changes; viewmodel projection remains unchanged.
- [ ] `view_feel_scale` scales sustained FOV presentation at 1.0/0.5/0.0; scale zero continues integrator advancement and restores without a frozen-state snap.
- [ ] Descriptor refresh, followed-pawn switch, absent view-feel driver, and level install discard sustained FOV state. No movement edge or reaction fires from that reset.
- [ ] The closed-key validation used by both new sustained rows and existing `impulse.states` sees only own table keys; prototype/`__index` entries cannot influence a descriptor.
- [ ] TypeScript and Luau SDK output, scripting reference, movement context, player-options context, and dev player descriptor describe the sustained target and final camera safety band.
- [ ] Manual trial confirms run and slide enter/exit read as body-state changes rather than FOV snaps, including sprint-jump continuity and scales 1.0/0.5/0.0.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Sustained FOV is local presentation only | Task 2 | input/prediction and render assembly | AC 3, 4, 9 |
| Exactly one sustained target is active | Task 2 | run/slide arbitration and retargeting | AC 5, 6 |
| FOV offset reaches camera once | Task 2 | impulse composition and camera assembly | AC 7 |
| Transient presentation state never outlives its driver or descriptor | Task 2 | staged refresh, pawn selection, level install | AC 9 |
| Closed author tables have no inherited behavior | Task 1 | JS and Luau parser lookup | AC 2, 10 |

## Tasks

### Task 1: Define and validate sustained FOV authoring

Add the optional `sustainedFov` descriptor and its sparse closed `run`/`slide`
rows to the foundation descriptor vocabulary. Establish a dependency-safe shared
60°–130° horizontal-FOV band so parsers and camera validation use one contract.
Each present row has `targetFov`, `enterTension`, and `exitTension`. Extend both
descriptor parsers, primitive registry, generated SDK types, committed snapshots,
and parser parity tests. Make table enumeration and lookup own-key-only for the
new rows and repair the same inherited-key defect in existing `impulse.states`.
Update all explicit `ViewFeelParams` construction sites.

### Task 2: Evaluate and compose one sustained target

Add a view-feel responsibility module for a single critically damped FOV target
spring. Feed it a local resolved activity derived from the frame's held Sprint
input and the followed component's `Normal`/`Slide` kind and horizontal velocity;
do not persist it on the movement component or add a wire field. Integrate its
output with existing view-feel FOV impulse output exactly once before
`RenderCamera` assembly. Preserve zero-scale advancement and the independent
viewmodel projection. Generalize descriptor-refresh invalidation to clear both
transient FOV systems while retaining bob, tilt, and sway; keep full reset on
pawn/no-driver/level lifecycle paths. Add pure evaluator, activity-resolution,
camera-composition, and lifecycle regressions.

### Task 3: Publish and trial the feel

Update the dev player descriptor to make sustained run and slide FOV the visual
fixture, removing the large slide-entry FOV impulse while retaining any desired
small non-FOV accent. Publish authoring, accessibility-scale, and camera-limit
semantics in the human and agent-facing docs. Run parser/SDK and focused runtime
checks, then perform the manual trial for run, slide, slide-to-run, sprint-jump,
and scales 1.0/0.5/0.0.

## Sequencing

**Phase 1 (sequential):** Task 1 — defines the authoring and validation contract Task 2 consumes.

**Phase 2 (sequential):** Task 2 — consumes the descriptor contract and establishes runtime behavior.

**Phase 3 (sequential):** Task 3 — consumes the shipped surface for docs, dev content, and manual trial.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Sustained FOV block | `ViewFeelParams::sustained_fov` | `sustainedFov` | `sustainedFov` | `sustainedFov` | n/a |
| Run row | `SustainedFovParams::run` | `run` | `run` | `run` | n/a |
| Slide row | `SustainedFovParams::slide` | `slide` | `slide` | `slide` | n/a |
| Absolute target | `SustainedFovStateParams::target_fov` | `targetFov` | `targetFov` | `targetFov` | n/a |
| Enter rate | `SustainedFovStateParams::enter_tension` | `enterTension` | `enterTension` | `enterTension` | n/a |
| Exit rate | `SustainedFovStateParams::exit_tension` | `exitTension` | `exitTension` | `exitTension` | n/a |

## Rough sketch

- `MovementInput.running` remains a simulation input. Render derives activity
  from the current local gameplay snapshot plus the followed component; it does
  not create `MovementStateKind::Running`.
- `ViewFeelState` gains only render-owned target-spring state. Existing
  transition impulses remain edge-driven and separate.
- Existing camera FOV bounds currently live with `RenderCamera`; Task 1 moves or
  exposes the band from a lower dependency layer before descriptor validation
  references it.
- The FOV target is converted to the camera's offset representation at render
  assembly. The camera remains authoritative for final world-projection clamp.

## Open questions

- None. The engine-owned locomotion epsilon is an implementation detail; tests
  pin stationary exclusion and ordinary run activation rather than its numeric
  spelling.
