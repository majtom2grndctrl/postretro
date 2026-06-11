# movement--view-feel

## Goal

Add first-person view feel — head **bob** (step-cycle oscillation), strafe **tilt** (velocity-driven view roll), and ambient **sway** (continuous noise) — as a declarative `viewFeel` sub-descriptor on the player movement descriptor. View feel is a render-only camera effect; it never touches gameplay state, collision, or the movement tick. Per-class authoring (a heavy tank vs. a nimble scout vs. an alien) falls out of descriptor tuning, with a spring `tension` knob as the primary character-weight control.

Part of the `movement--*` series. Not a roadmap milestone on its own — a self-contained, shippable increment under Milestone 11. See `movement.md`.

## Scope

### In scope
- A `viewFeel` optional sub-descriptor on `PlayerMovementDescriptor`, with three optional sub-objects: `bob`, `tilt`, `sway`. Each absent → that motion disabled.
- Bob: distance-phased vertical + lateral oscillation, self-gating (zero at rest), with a speed deadzone.
- Tilt: view roll driven by lateral (strafe) velocity, settled through a frame-rate-independent spring whose stiffness is the author-facing `tension` knob (the lead-and-settle / heavy-vs-snappy control).
- Sway: continuous, non-periodic yaw/pitch/roll wander from summed incommensurate sines, base amplitude scaled up by movement speed.
- Render-rate evaluation: a pure view-feel evaluator plus engine-owned integrator state (spring + phase) advanced by `frame_dt` each frame, reading the followed pawn's velocity and grounded flag.
- Extension of the view-transform chokepoint (`InterpolableState::view_matrix` / `view_projection`) to apply a roll angle and a world-space eye offset; sway angle offsets fold into the existing yaw/pitch arguments.
- Wiring the existing `PlayerOptions.view_feel_scale` (`[0,1]`, default 1.0, clamped on load — `player_options.md` §5) as the global multiplier on all view-feel output; the M13 settings menu later drives the same field.
- Descriptor + SDK surface: dual-path (JS + Luau) parsing with matching validation and error text, `register_type` emission, type-name maps, drift-test coverage, regenerated committed `.d.ts`/`.d.luau`.

### Out of scope
- Movement-mechanics tuning (ground friction / `pm_stopspeed`-style direction-change deceleration). The *mechanical* floaty-strafe fix is a separate `Normal`-tuning spec; this spec only addresses the *visual* read of direction change. See `research.md`.
- Weapon/viewmodel sway and a separate weapon FOV camera. No viewmodel exists yet (Milestone 10+). View feel here is the world-view transform only.
- Screen-fixed crosshair / HUD compositing. No crosshair exists (debug egui only); when the M13 HUD lands it must render screen-fixed, independent of the view roll — noted as an M13 constraint, built here by leaving the HUD layer untouched by view feel.
- A player-facing accessibility settings UI. Only the runtime scale seam ships here; its slider is M13.
- Perlin/value-noise sway (summed sines ship instead — see D4). Tick-state interpolation of view feel (it is render-rate — see D5).
- View feel for the free-fly (no-pawn) camera. View feel requires a player pawn's velocity; without one, the view transform is unchanged.

## Decisions

**D1 — `viewFeel` is render-only, pawn-driven, and never enters the movement tick.** The evaluator reads the followed pawn's `velocity` and `is_grounded` and outputs a camera roll, a world-space eye offset, and yaw/pitch angle offsets. It writes nothing back to the component or the tick state. This holds the movement invariant (engine-internal kinematic motion) and keeps view feel off the hot game-logic path. When no pawn drives the camera, or the pawn carries no `viewFeel`, the view transform is bit-identical to today.

**D2 — Three motions, three mechanisms — not unified.** Bob is a distance-phased oscillator (self-gating: phase advances with horizontal distance travelled, so it stops when the player stops). Tilt is a spring settling toward a velocity-derived target. Sway is a noise source. Folding them into one mechanism is the documented mistake (`research.md`); they stay separate functions sharing only the global scale and the chokepoint.

**D3 — `tension` is the spring knob, exposed only on `tilt`.** Tilt is the motion that reacts to direction *changes*, so its settle behavior is where character weight reads. `tension` maps to the spring's natural frequency; damping is fixed internally at a slight under-damp so the roll *leads and overcorrects* (the Destiny boxing-reference feel) rather than snapping. Low tension = heavy/loose/slow-settle (tank); high tension = tight/snappy (scout). One knob, character-legible — no exposed damping ratio. Bob conveys weight through amplitude + frequency instead (the "dinosaur/ogre" pattern, `research.md`); sway is noise and needs no spring. The spring step must be frame-rate-independent (analytic/semi-implicit, not naive Euler) so `tension` feels identical at 60 and 144 Hz.

**D4 — Sway is summed incommensurate sines, not Perlin.** Two-to-three sines per axis at irrational frequency ratios are deterministic, table-free, dependency-free, and effectively non-repeating over gameplay timescales — enough for v1 organic wander. Perlin/value noise is a future refinement behind the same `sway` field set, no descriptor break. Sway perturbs yaw, pitch, and roll — three independent sine channels under one shared `amplitude`/`frequency`/`speedScale` set (no per-axis fields). The authored `frequency` is the base rate; each channel's 2–3 sines run at fixed engine-internal incommensurate multiples of it — the ratios are constants, not authored fields. Sway's roll is a small *ambient* term summed with tilt's velocity-driven roll at the chokepoint: tilt owns the strafe response, sway adds the organic wander. Bob phase is distance-driven (`frequency` is cycles per metre of horizontal travel), so bob auto-scales with speed and halts at rest. Amplitude eases in over a small speed band above `speedThreshold`; at speed it holds the authored metres-at-peak value.

**D5 — View feel is evaluated at render rate, not tick rate.** Spring and bob phase advance by `frame_dt` (in scope at the render-assembly site), reading the pawn's latest post-tick velocity as input — mirroring how yaw/pitch are already applied render-rate (`frame_timing.rs`, `research.md`). Nothing is added to `InterpolableState` or `push_state`; the tick state stays position-only. Integrator state (current roll + roll velocity, bob phase, sway clock) is engine-owned alongside the camera, not on the component.

**D6 — Accessibility scale is the existing `PlayerOptions.view_feel_scale`, not a descriptor field.** Per-class feel is the author's job (descriptor); the player's comfort override multiplies every motion's output. The field is `PlayerOptions.view_feel_scale` (`crates/postretro/src/options/mod.rs`), not a descriptor field and not a new engine constant. It already exists in `PlayerOptions` with no UI; the M13 settings menu drives it later; this spec wires it into the render path. `scale = 0` zeroes all view feel — the off switch.

**D7 — Two-level present-then-all-required.** `viewFeel` is optional on the descriptor. Within a present `viewFeel`, each of `bob`/`tilt`/`sway` is independently optional. Within a present sub-object, all its fields are required — the same `contains_key`-gated discipline `ground`/`air`/`fall` follow, applied at two nesting levels.

## Acceptance criteria

### Automated (test-gated)
- [ ] With `roll = 0` and a zero eye offset, the extended view-transform chokepoint produces a matrix identical to the current `view_projection` output (no-regression pin for the free-fly / no-view-feel path).
- [ ] A nonzero roll angle produces a view matrix that differs from the `roll = 0` matrix, and a nonzero eye offset shifts the camera position in the matrix.
- [ ] Bob output is zero when horizontal speed is at or below `speedThreshold`, and its amplitude increases with horizontal speed through the ease-in band above the threshold, saturating at the authored amplitude (self-gating verified).
- [ ] Tilt sign is opposite for left-strafe vs. right-strafe velocity, and tilt magnitude rises with lateral speed and clamps at `maxAngle` for lateral speed at or above `speedReference`.
- [ ] Tilt spring convergence depends on `tension`: starting from zero roll with a fixed target, a higher `tension` reaches a fixed fraction of the target in fewer simulated frames than a lower `tension` (the settle-speed knob is wired).
- [ ] The tilt spring step is frame-rate-independent: advancing the spring to a fixed wall-clock time in many small `frame_dt` steps and in few large steps converges to the same roll within tolerance.
- [ ] Sway output is bounded by its effective amplitude, is non-zero even at zero speed when `amplitude > 0` (effective amplitude is `amplitude * (1 + speedScale * speed)`, nonzero at rest), and its effective amplitude increases with speed when `speedScale > 0` (and is constant when `speedScale = 0`).
- [ ] The global view-feel scale multiplies all output: at `scale = 0` bob, tilt, and sway all produce zero offset/roll regardless of velocity; at `scale = 1` they produce the unscaled values. The scale is read from `PlayerOptions.view_feel_scale`; clamping/default behavior is owned and tested by the options module, and evaluator tests receive the scale as a plain parameter.
- [ ] Evaluating with `frame_dt = 0` leaves the integrator state unchanged — no bob-phase, spring, or sway-clock advance.
- [ ] Each sub-object absent disables its motion: with `bob`/`tilt`/`sway` individually `None`, that motion contributes zero while the others are unaffected; with `viewFeel` absent entirely, the evaluator produces zero roll and zero offset.
- [ ] An absent `viewFeel` sub-object is valid (no `ViewFeelParams` materialized). When `viewFeel` is present, an absent `bob`/`tilt`/`sway` is valid; when any of those is present, all of its fields are required (two-level present-then-all-required), and a missing required field is rejected in both the JS and Luau paths.
- [ ] Descriptor parsers reject invalid fields symmetrically in both JS and Luau with matching field-path error text: `movement.viewFeel.bob.frequency`, `movement.viewFeel.tilt.speedReference`, `movement.viewFeel.tilt.tension`, and `movement.viewFeel.sway.frequency` reject missing/non-finite/non-positive (zero rejected); `movement.viewFeel.bob.verticalAmplitude`, `movement.viewFeel.bob.lateralAmplitude`, `movement.viewFeel.bob.speedThreshold`, `movement.viewFeel.sway.amplitude`, and `movement.viewFeel.sway.speedScale` reject missing/non-finite/negative (zero allowed); `movement.viewFeel.tilt.maxAngle` rejects missing/non-finite/out-of-`[0, 90]`.
- [ ] The SDK type-drift test (`committed_sdk_types_match_current_registry`) passes with `ViewFeelParams`, `BobParams`, `TiltParams`, and `SwayParams` present in the committed `sdk/types/postretro.d.ts` and `.d.luau`, and `viewFeel?` present on `PlayerMovementDescriptor`.

### Manual-visual (no automated verification — eyeball in-engine)
- [ ] Bob reads as a footstep cadence tied to movement, not a nausea-inducing wobble; it eases in from rest rather than snapping on.
- [ ] At low `tilt.tension` the roll visibly leads into a strafe direction-change and settles with a slight overshoot (heavy feel); at high `tension` it tracks the strafe crisply (agile feel).
- [ ] Sway gives a faint organic drift at rest that grows as the pawn runs faster — an alien/creature-like head motion, not a periodic shake.
- [ ] Two descriptors tuned as a "tank" (low tension, high bob amplitude, low bob frequency) and a "scout" (high tension, low amplitude, high frequency) feel distinctly different in-engine.

## Tasks

### Task 1: `viewFeel` data surface — parse, validate, materialize, emit
Add the Rust param structs (`ViewFeelParams` holding `Option<BobParams>` / `Option<TiltParams>` / `Option<SwayParams>`) in `data_descriptors.rs`, alongside the existing movement sub-descriptors. Parse `viewFeel` in BOTH the JS/QuickJS and Luau descriptor paths with the two-level present-then-all-required discipline (D7): gate each level on `contains_key` (JS) / table presence (Luau); when a sub-object is present, all its fields are required. Pin validators per field using the existing helpers — `validate_positive_finite` for `bob.frequency`, `tilt.speedReference`, `tilt.tension`, `sway.frequency`; `validate_non_negative_finite` for `bob.verticalAmplitude`, `bob.lateralAmplitude`, `bob.speedThreshold`, `sway.amplitude`, `sway.speedScale`; `validate_in_range_finite(min=0.0, max=90.0)` for `tilt.maxAngle`. Field-path error text follows the existing convention (backtick-wrapped dotted path, e.g. `` `movement.viewFeel.tilt.tension` ``). Author every wire key as a camelCase string literal per the Boundary inventory. Materialize `view_feel: Option<ViewFeelParams>` onto `PlayerMovementComponent` in `from_descriptor` (clone, no transform — mirrors `ground`/`air`/`fall`). Emit the SDK types: `register_type("ViewFeelParams"/"BobParams"/"TiltParams"/"SwayParams").field(...).finish()` in `primitives/mod.rs`; add `viewFeel?` to `PlayerMovementDescriptor`'s registration; add all four type names to BOTH the `rust_to_ts` and `rust_to_luau` maps in `typedef.rs` (or they emit unresolved); update the `EXPECTED_TS`/`EXPECTED_LUAU` test constants; regenerate the committed files via `cargo run -p postretro --bin gen-script-types` and confirm the drift test passes.

### Task 2: View-feel evaluator + integrator state
Add a pure evaluation module computing the three motions from `(&ViewFeelParams, horizontal_velocity, lateral_velocity, is_grounded, integrator_state, frame_dt, global_scale)`. The caller owns the camera basis: `lateral_velocity` arrives as the signed projection of pawn velocity onto the camera right vector, `horizontal_velocity` as the horizontal speed magnitude. The evaluator never sees the camera basis; its lateral outputs are scalars the caller maps back onto that basis. Bob: advance phase by horizontal distance travelled this frame (`speed * frame_dt`), gate to zero at or below `speedThreshold`, output a vertical offset (`sin(phase)`) and a lateral offset (`sin(phase * 0.5)` or equivalent half-rate) scaled by `verticalAmplitude` / `lateralAmplitude` and by an ease-in factor that ramps from 0 at `speedThreshold` to 1 over a small engine-internal speed band (amplitudes stay metres-at-peak; the ramp delivers the ease-in-from-rest of the manual AC). Tilt: compute target roll = `maxAngle * clamp(lateral_velocity / speedReference, -1, 1)`, sign carried by the signed `lateral_velocity` input (the caller's right-vector projection); advance a slightly-under-damped spring (current roll + roll velocity in the integrator) toward the target with a frame-rate-independent step (analytic/semi-implicit) whose natural frequency is `tension`. Sway: advance a clock by `frame_dt`, sum 2–3 incommensurate sines per axis (yaw, pitch, roll — independent sine sets), scale by `amplitude * (1 + speedScale * horizontal_speed)`; output the roll channel separately so Task 4 can sum it with the tilt spring roll. Multiply every motion's output by `global_scale`. Return a named output struct — `ViewFeelOutput { bob_vertical: f32, bob_lateral: f32, tilt_roll: f32, sway_roll: f32, sway_yaw: f32, sway_pitch: f32 }` (all post-scale) — so Task 4's per-channel wiring is unambiguous. Define the integrator state struct (current roll, roll velocity, bob phase, sway clock) engine-side; the evaluator reads and updates it. Absent sub-objects (`None`) contribute zero. Unit-test against the automated AC (sign, clamp, gating, spring convergence + frame-rate independence, scale, bounds). Pure functions — no rendering or GPU.

### Task 3: View-transform chokepoint extension
Extend `InterpolableState::view_matrix` and `view_projection` (`crates/postretro/src/frame_timing.rs` — the top-level module, not `render/frame_timing.rs`) to accept a roll angle and a world-space eye offset: roll rotates the up vector around the look direction before `look_at_rh`; the eye offset is added to the position before building the target. Default arguments (`roll = 0`, `offset = ZERO`) must reproduce the current matrix exactly. Update the existing callers: the render assembly call to `InterpolableState::view_projection` in `main.rs` and the `view_projection_*` tests in `frame_timing.rs`. (`InterpolableState::view_matrix` has no external callers — it is internal to `view_projection`. `Camera::view_matrix`/`view_projection` in `camera.rs` are a separate, zero-argument surface and are not touched.) Pure matrix math; unit-test the no-regression pin and the roll/offset-changes-matrix AC. Independent of Task 1/2 — consumes only the *shape* (an angle, an offset).

### Task 4: Render-assembly wiring + accessibility scale seam
Add the integrator state (Task 2) as an engine field alongside `camera`/`frame_timing`. At the render-assembly site, read `self.player_options.view_feel_scale` (the app-struct field already loaded at boot) and pass it as the evaluator's `global_scale` (D6). Clamping and default are owned by the options module — do not re-clamp here. In the per-frame render assembly (`main.rs`, after the pawn-follow position is resolved and before `view_projection` is built), when a pawn drives the camera and carries `view_feel`: read its `velocity` and `is_grounded`, run the Task 2 evaluator with `frame_dt` and the global scale, and feed the resulting roll (tilt spring roll + sway roll, summed) + world-space eye offset (bob vertical along world Y, bob lateral along the camera right vector) into the Task 3 chokepoint, folding the sway yaw/pitch offsets into the `yaw`/`pitch` arguments. When no pawn drives the camera or it carries no `view_feel`, pass `roll = 0` / `offset = ZERO` / no angle offsets (identical-to-today path). Do not advance the integrator on zero-`frame_dt` frames.

## Sequencing

**Phase 1 (concurrent):** Task 1 (data surface) ∥ Task 3 (chokepoint matrix) — independent: one touches the descriptor/SDK layer, the other touches the view-matrix math.
**Phase 2 (sequential):** Task 2 (evaluator + integrator) — consumes `ViewFeelParams` from Task 1.
**Phase 3 (sequential):** Task 4 (render-assembly wiring) — consumes the Task 2 integrator output and the Task 3 chokepoint.

## Boundary inventory

View-feel tuning crosses Rust ↔ wire (JS/Lua object) ↔ TS ↔ Luau. Field names are camelCase on every script-facing side per the scripting naming convention; Rust uses snake_case. No FGD KVP and no PRL/binary section — the descriptor is a script object, not baked data, and movement tuning is never map-overridable (`movement.md` §7).

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| view-feel sub-descriptor (optional) | `Option<ViewFeelParams>` | optional object under `movement` | `viewFeel?: ViewFeelParams` | `viewFeel?` | n/a |
| bob sub-object (optional) | `Option<BobParams>` | optional object under `viewFeel` | `bob?: BobParams` | `bob?` | n/a |
| tilt sub-object (optional) | `Option<TiltParams>` | optional object under `viewFeel` | `tilt?: TiltParams` | `tilt?` | n/a |
| sway sub-object (optional) | `Option<SwayParams>` | optional object under `viewFeel` | `sway?: SwayParams` | `sway?` | n/a |
| bob cycle rate (cycles per metre) | `frequency: f32` | `frequency` | `frequency` | `frequency` | n/a |
| bob vertical amplitude | `vertical_amplitude: f32` | `verticalAmplitude` | `verticalAmplitude` | `verticalAmplitude` | n/a |
| bob lateral amplitude | `lateral_amplitude: f32` | `lateralAmplitude` | `lateralAmplitude` | `lateralAmplitude` | n/a |
| bob speed deadzone | `speed_threshold: f32` | `speedThreshold` | `speedThreshold` | `speedThreshold` | n/a |
| tilt max roll (degrees) | `max_angle: f32` | `maxAngle` | `maxAngle` | `maxAngle` | n/a |
| tilt full-roll lateral speed | `speed_reference: f32` | `speedReference` | `speedReference` | `speedReference` | n/a |
| tilt spring stiffness | `tension: f32` | `tension` | `tension` | `tension` | n/a |
| sway base amplitude | `amplitude: f32` | `amplitude` | `amplitude` | `amplitude` | n/a |
| sway noise rate | `frequency: f32` | `frequency` | `frequency` | `frequency` | n/a |
| sway speed gain | `speed_scale: f32` | `speedScale` | `speedScale` | `speedScale` | n/a |

**Wire-casing mechanism.** Movement descriptor types are emitted via the `register_type().field()` generator in `primitives/mod.rs` (NOT verbatim TS/Luau blocks), and wire keys are read as hand-written camelCase string literals in the JS and Luau parsers in `data_descriptors.rs` — not by serde rename. Author each view-feel wire key literally in: (1) the JS parser, (2) the Luau parser, (3) the `register_type().field(...)` calls, (4) the `rust_to_ts` type-name map, (5) the `rust_to_luau` type-name map, (6) the `EXPECTED_TS`/`EXPECTED_LUAU` test constants. `tension`'s name is the author-facing spring-feel knob (D3); keep it literally `tension` on every side.

## Open questions
- **Global scale → settings UI.** The seam is the existing `PlayerOptions.view_feel_scale` field (no UI yet). The M13 settings menu owns the player-facing slider and any per-motion breakdown (separate bob / tilt / sway sliders, matching DOOM TDA); it wires its slider to that field rather than re-deriving a seam.
