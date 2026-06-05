# movement--crouch

## Goal

Add a `Crouching` movement state on the shipped state-machine seam: the player shrinks the collision capsule and drops the eye, moves at a reduced crouched speed, and cannot stand back up while a ceiling blocks the standing capsule. Factor the capsule resize as a reusable mechanism (not crouch-only) because `movement--slide` consumes it. Toggle-vs-hold is resolved in the input layer from a new `PlayerOptions.crouch_mode`, not inside the movement intent.

Independent thin slice under Milestone 11 (Advanced Movement), in the `movement--*` series. Draftable early. See `movement.md`.

## Scope

### In scope
- A `Crouching` variant on `MovementState` (`scripting/components/player_movement.rs`), plugging into the existing intent/substrate/transition seam (`movement.md` §4).
- A capsule-resize mechanism on the substrate: the active state sets the target `capsule.half_height`/`eye_height`; the substrate already rebuilds the parry `Capsule` every tick from those fields, so resize is mutation. Factored so slide reuses it (Decision D8).
- A stand-up ceiling probe: an upward `cast_capsule` with the STANDING capsule. Blocked → stay crouched, re-check each tick (Decision D7).
- A crouched horizontal-speed model (Decision D5).
- Eye-height smoothing for feel; collision capsule resizes promptly (Decision D3).
- An optional `CrouchParams` descriptor (present-then-all-required, like `dash`) with the crouch tunables; dual-path JS + Luau parse/validate, `register_type` emission, type-name maps, drift-test coverage, regenerated committed `.d.ts`/`.d.luau`.
- An `Action::Crouch` input action + default keyboard/gamepad bindings; a `crouch_intent: bool` field on `MovementInput` threaded from `main.rs`.
- The `crouch_mode` (toggle | hold, default hold) field on `PlayerOptions` plus the input-layer edge derivation that resolves it into the per-tick `crouch_intent` bit. `player-options` (done) designates the crouch spec as this field's owner — its out-of-scope section reads "the crouch spec adds that field as a consumer." The resolution lives in input, never in the movement intent.

### Out of scope (non-goals)
- The M13 settings-menu UI for `crouch_mode`. `player-options` ships the store and this spec adds the field (see In scope); rendering a control to change it at runtime is a separate M13 deliverable (`player_options.md` §4).
- Slide (`movement--slide`) — it consumes this spec's capsule-resize mechanism but is its own draft.
- Map-overridable crouch tuning (movement physics is descriptor-owned, never FGD KVPs — `movement.md` §7).
- Per-tick script-authored crouch (the surface is declarative — `movement.md` §2, §7).
- Crouch's eye drop is one camera contribution; bob/sway/tilt from `movement--view-feel` are independent contributions that SUM at the view chokepoint (Decision D9). No view-feel work here.

## Decisions

**D1 — `Crouching` is a state on the existing seam.** Mirrors `Dash`: a `MovementState` variant with per-state velocity intent, dispatched in `dispatch_state_intent`, transitions returned to the tick. The capsule/eye target is the state's distinguishing effect; the substrate honors it because `integrate_collision` rebuilds the parry `Capsule` every tick from `component.capsule.half_height`/`radius`.

**D2 — Resize anchoring: feet planted, head/eye drops (FPS norm).** The pawn `Transform.position` is the capsule center; shrinking `half_height` with the feet fixed means the center moves DOWN by the half-height delta so the lowest point (`position.y - (half_height + radius)`) stays put. The intent writes both the reduced `half_height` and the adjusted center; `eye_height` drops correspondingly. Standing up reverses it (center rises) only after the stand-up probe clears (D7).

**D3 — Collision capsule resizes promptly; eye-height smooths (DECIDED).** The collision `half_height` snaps to the crouched value on entry (and back on stand-up when clear) so the hitbox is honest immediately. The camera `eye_height` interpolates toward its target by an exponential approach at a tunable per-second rate (`transitionRate`) so the view glides rather than teleporting. Rationale: a smoothly-shrinking hitbox would let geometry the player visually cleared still block them mid-transition; a snapped eye reads as a jarring jump. Splitting them keeps collision honest and the camera smooth; exponential approach is the conventional framerate-independent smoothing form and `transitionRate` is tunable, so the exact feel is dialed in-engine. The eye smoothing runs against the same camera-follow read that applies `eye_height` each tick (`main.rs` camera-follow).

**D4 — Airborne crouch: allowed, HEAD-anchored (DECIDED).** Allow crouch midair; shrink the hitbox with the HEAD anchored (feet rise toward center), so an airborne crouch tucks the legs up rather than dropping the camera through the floor. Stand-up midair needs no ceiling probe unless a surface is within the standing extent. Decided for slide-jump continuity — `movement--slide` inherits this resize, and the M11 slide/bhop flow (Titanfall lineage, Neon White flexibility yardstick) depends on midair crouch composing cleanly. Disallowing it would fight the downstream slide spec.

**D5 — Crouched speed: extend `SpeedParams` with a `crouch` tier.** Add `crouch: f32` to `SpeedParams` (alongside `walk`/`run`), selected by the `Crouching` intent as the omnidirectional horizontal speed target. Chosen over a `crouchSpeedScale` on `CrouchParams` because speed already lives in `SpeedParams` as a tier set (walk/run); a third tier is the symmetric, discoverable home, and a flat speed (not a scale) matches how walk/run are authored. Slide, which converts speed rather than capping it, is unaffected.

**D6 — `CrouchParams` is an optional descriptor (present-then-all-required, like `dash`).** Absent ⇒ crouch disabled: the `Normal`→`Crouching` transition never fires, no `Action::Crouch` effect. Present ⇒ all fields required and validated, dual-path JS/Luau. Holds the descriptor-owned tuning invariant.

**D7 — Stand-up blocked: ceiling probe with the STANDING capsule; if blocked stay crouched, re-check each tick.** When crouch-intent releases (D-dependency), the intent attempts to stand: it builds the STANDING-size capsule and `cast_capsule`s UPWARD by the head-clearance delta (standing top minus crouched top). A hit within that distance ⇒ headroom blocked ⇒ remain `Crouching`, keep the crouched capsule, retry next tick. Clear ⇒ snap the collision capsule back to standing (raising the center, D2) and transition `Crouching`→`Normal`. The probe runs every tick crouch-intent is inactive, so the player auto-stands the first clear tick.

**D8 — LOAD-BEARING: capsule resize is a reusable mechanism, not crouch-only.** Factor entry/exit resize (target half-height + center adjustment + eye target) and the stand-up clearance probe as substrate-level helpers the `Crouching` intent calls — NOT inlined in crouch. `movement--slide` consumes the same helpers (roadmap: slide "depends on crouch's capsule resize"). This is a contract slide will hold us to: the helpers take a target capsule size and an anchor mode (feet/head), so slide reuses them unchanged.

**D9 — Camera composition: crouch eye-drop and view-feel offsets SUM at the chokepoint.** Crouch contributes a smoothed `eye_height` reduction (D3); `movement--view-feel` contributes bob/sway/tilt offsets at the view transform. They are independent additive contributions at the camera-follow/view chokepoint — no conflict, no ordering dependency. Stated so neither slice assumes exclusive ownership of the eye offset.

**D10 — Crouch-jump: never suppress (DECIDED).** Pressing jump while crouched first attempts to stand (the D7 stand-up probe): headroom clear ⇒ stand and jump (transition to `Normal`, apply the jump); blocked by a ceiling ⇒ the jump still fires while the player stays crouched (lower arc, crouched capsule retained). The jump is never swallowed. Chosen against auto-stand-then-suppress because a dead jump input under a ceiling is the "engine-fighting" feel the product explicitly rejects, and the Titanfall slide lineage (which owns the downstream slide model) treats crouch and jump as coexisting tech, not mutually gating. This is the mutual-exclusion contract: `Crouching` does not block the jump/air-jump branch or the `Dash` transition — it reshapes them (stand-if-clear, else act crouched).

## Acceptance criteria

### Automated (test-gated)
- [ ] With crouch-intent active and on the ground, the player enters `Crouching`: the collision `half_height` equals the crouched value and the capsule's lowest point (`position.y - (half_height + radius)`) is unchanged from standing (feet planted, D2).
- [ ] Crouched horizontal speed targets `ground.speed.crouch`: with crouch-intent held and full movement input, steady-state horizontal speed settles at the crouch tier, below `walk`.
- [ ] Stand-up CLEAR: crouch-intent released with open headroom transitions `Crouching`→`Normal`, restores the standing `half_height`, and the capsule lowest point is unchanged (feet planted, center rises).
- [ ] Stand-up BLOCKED: crouch-intent released under a ceiling within the head-clearance delta keeps the player `Crouching` (standing capsule stays unmaterialized); on a later tick with the ceiling removed the player auto-stands (transition fires the first clear tick) — verified by the upward STANDING-capsule `cast_capsule` (D7).
- [ ] Crouch-jump never suppresses (D10): pressing jump while crouched under open headroom stands then jumps (transition to `Normal`, jump applied); pressing jump while crouched under a blocking ceiling still applies the jump with the crouched capsule retained (no dead input). The `Dash` transition is likewise available from `Crouching`.
- [ ] Airborne crouch (D4): entering `Crouching` midair anchors the HEAD — the capsule's highest point (`position.y + half_height + radius`) is unchanged and the feet rise; no stand-up ceiling probe is required midair unless a surface lies within the standing extent.
- [ ] Capsule-resize helper is reusable (D8): a unit test drives the resize/stand-up helpers directly with a target size and anchor mode, independent of the `Crouching` intent, proving slide can call them.
- [ ] Absent `crouch` descriptor disables crouch: the `Normal`→`Crouching` transition never fires regardless of crouch-intent; no capsule resize occurs.
- [ ] Present `crouch` requires all fields: an absent inner field is rejected in BOTH the JS and Luau paths (present-then-all-required, like `dash`).
- [ ] Descriptor parsers reject invalid `crouch` fields symmetrically in JS and Luau (each path names the offending field; wording/granularity differ per path as `dash` already does): the crouched-height/eye fields reject missing/non-finite/non-positive (zero rejected); `transitionRate` rejects missing/non-finite/non-positive; the crouched speed tier (`ground.speed.crouch`) rejects missing/non-finite/negative (zero allowed).
- [ ] `ground.speed.crouch` is required when `ground.speed` is present, validated non-negative finite, symmetric JS/Luau (mirrors `walk`/`run`).
- [ ] The SDK type-drift test (`committed_sdk_types_match_current_registry`) passes with `CrouchParams` present in `sdk/types/postretro.d.ts` and `.d.luau`, `crouch?` on `PlayerMovementDescriptor`, and the new `crouch` field on `SpeedParams`.
- [ ] `Action::Crouch` is in the gameplay action set with default keyboard AND gamepad bindings; `input/defaults.rs`'s `all_actions()` list and per-Action coverage tests include it.
- [ ] `crouch_mode` resolves toggle vs. hold in the input layer (Task 5): in `hold` mode `crouch_intent` tracks `Action::Crouch` level (active while held); in `toggle` mode one press latches `crouch_intent` on and a second press latches it off (rising-edge derived). The `Crouching` intent is identical in both modes — it sees only the resolved per-tick bit.
- [ ] `PlayerOptions.crouch_mode` round-trips through TOML with default `hold`; an absent field loads as `hold` (`serde(default)`), per the player-options schema-evolution invariant.

### Manual-visual (no automated verification — eyeball in-engine)
- [ ] Crouch entry reads as a smooth camera dip (eye glides, hitbox snaps) — no view teleport, no clipping into geometry the player visually cleared.
- [ ] Holding crouch under a low ceiling keeps the player crouched; the player pops up the instant they clear it.

## Tasks

### Task 1: `crouch` data surface — `SpeedParams.crouch`, `CrouchParams`, parse/validate/emit
Add `crouch: f32` to `SpeedParams` (`data_descriptors.rs`), parsed and validated `validate_non_negative_finite` in BOTH the JS (`movement_descriptor_from_js`) and Luau paths, exactly as `walk`/`run` are. Add a `CrouchParams` struct (`data_descriptors.rs`) holding the crouched capsule height, crouched eye height, and `transition_rate` (camelCase `transitionRate`); the crouched-height/eye validators use `validate_positive_finite`, `transition_rate` uses `validate_positive_finite`. Parse `crouch` as an OPTIONAL sub-object on `PlayerMovementDescriptor` with the present-then-all-required `contains_key`/null-guard discipline `dash` uses (`data_descriptors.rs` dash block), via a `crouch_params_from_js`/`_from_lua` mirroring `dash_params_from_js`. Materialize `crouch: Option<CrouchParams>` onto `PlayerMovementComponent` in `from_descriptor` (clone, mirrors `dash`). Emit the SDK type: `register_type("CrouchParams").field(...)` in `primitives/mod.rs` alongside `DashParams`; add an optional `crouch?: CrouchParams` field to the `PlayerMovementDescriptor` chain (mirror the optional `dash?` field) and the new `crouch` field to the `SpeedParams` chain; add `"CrouchParams" => "CrouchParams".to_string()` to BOTH the TS and Luau type-name maps in `typedef.rs`; update `EXPECTED_TS`/`EXPECTED_LUAU` (and the `_WITH_DOCS` constants if the doc string changes); regenerate the committed `.d.ts`/`.d.luau` and confirm the drift test passes. Pure data-surface work; see Boundary inventory.

### Task 2: Capsule-resize + stand-up-probe substrate helpers (reusable — D8)
Add to `movement/mod.rs` two helpers the `Crouching` intent calls and `movement--slide` will reuse: (a) a resize helper taking a target `half_height`/`eye_height` and an anchor mode (`Feet` for grounded, `Head` for airborne — D2/D4), which mutates `component.capsule.half_height`/`eye_height` and adjusts the pawn center so the anchored extreme stays fixed (it returns or applies the center delta the tick must add to `position`); (b) a stand-up clearance probe building the STANDING-size parry `Capsule` and `cast_capsule`-ing UPWARD by the head-clearance delta, returning whether headroom is clear (D7). Both are state-agnostic — they take sizes/anchors, not a crouch flag — so slide calls them unchanged. The resize honors the substrate's per-tick `Capsule` rebuild (`integrate_collision` reads `component.capsule.half_height`/`radius`), so no second capsule cache exists. Unit-test the helpers directly (reusability AC).

### Task 3: `Crouching` state + intent + transitions
Add `Crouching { eye_current: f32 }` (or equivalent per-state live data: the smoothing source for D3) to `MovementState` and dispatch it in `dispatch_state_intent` alongside `Normal`/`Dash`. The `Crouching` intent: applies gravity/locomotion like `Normal` but targets the crouch speed tier `ground.speed.crouch` as its horizontal speed (D5); on entry calls the Task 2 resize helper to the crouched size with the grounded/airborne anchor (D2/D4); advances `eye_current` toward the crouched eye target at `transitionRate` for smoothing (D3); while crouch-intent is inactive, runs the Task 2 stand-up probe and, when clear, resizes back to standing and returns a `Crouching`→`Normal` `Transition` (carry `CarryRule::KEEP_ALL` — crouch preserves momentum, it is a resize not a velocity reset). `Normal`→`Crouching` transition fires on the crouch-intent bit when `crouch` is `Some`. When `crouch` is `None`, the transition never fires (crouch disabled). Crouch-jump follows D10 (never suppress): the jump/air-jump branch and `Dash` transition stay available from `Crouching` — stand-if-clear, else act crouched — rather than being gated off. The camera-follow eye read (`main.rs`) consumes the smoothed eye value rather than the raw `capsule.eye_height` when crouching/transitioning (D3/D9) — thread the smoothed value to the follow read.

### Task 4: `Action::Crouch` input plumbing + `MovementInput` wiring
Add `Action::Crouch` to the `Action` enum (`input/types.rs`), with default keyboard (e.g. `ControlLeft` or `KeyC`) and gamepad (e.g. a free button — `LeftThumb` is taken by Sprint) bindings in `input/defaults.rs`, and add it to the test `all_actions()` list so the binding-coverage tests pass. Add `crouch_intent: bool` to `MovementInput` (`movement/mod.rs`); thread it from `main.rs`'s movement call site into `run_movement_tick` as a new `bool` param and onto the `MovementInput { .. }` literal, mirroring how `dash_pressed` is derived and threaded (`main.rs` movement-tick assembly). IMPORTANT: `crouch_intent` is NOT a raw button read — it is the toggle-vs-hold-resolved crouch edge the input layer derives from `PlayerOptions.crouch_mode` (Task 5). The field's contract is "crouch intent active this tick," resolved by input, never by the movement intent. Adding `crouch_intent` to `MovementInput` will break every `MovementInput { .. }` literal in `movement/mod.rs`'s test module and the live construction in `run_movement_tick` — update all of them (`false` in test literals).

### Task 5: `crouch_mode` option field + input-layer toggle/hold derivation
Add a `CrouchMode` enum (`Toggle | Hold`, with `Default` = `Hold`) and a `crouch_mode: CrouchMode` field to `PlayerOptions` (`serde(default)`, snake_case TOML key `crouch_mode`) — `player-options` (done) designates the crouch spec as this field's owner. `PlayerOptions` is engine-internal config: no SDK type, no `register_type`, no `.d.ts`/`.d.luau`, no drift test (`player_options.md` §6). In the input layer (the movement-tick assembly at `main.rs`'s call site, alongside the Task 4 plumbing), derive the per-tick `crouch_intent` bit from `crouch_mode` + `Action::Crouch`: `Hold` ⇒ `snapshot.button(Action::Crouch).is_active()` (level signal); `Toggle` ⇒ a persistent latch flipped on each `ButtonState::Pressed` rising edge. The latch lives with the input consumer (app/input state), never on the movement component. Feed the resolved bit into `MovementInput.crouch_intent` (Task 4). Tests: TOML round-trip + absent-field-defaults-to-`Hold`; toggle latch flips on rising edge; hold tracks level.

## Sequencing

**Phase 1 (concurrent):** Task 1 (data surface) ∥ Task 2 (substrate helpers) — independent: one touches the descriptor/SDK layer, the other the substrate. Task 4's `Action::Crouch`/`MovementInput` plumbing and Task 5's `crouch_mode` field + derivation may also run here (input/options layer, independent of both). Note: Task 2 and Task 4 both edit `movement/mod.rs` — coordinate the two edits (or sequence them) to avoid a collision.
**Phase 2 (sequential):** Task 3 (`Crouching` state) — consumes Task 1's `CrouchParams`/`SpeedParams.crouch`, Task 2's resize/probe helpers, and Task 4's `crouch_intent` field (resolved from `crouch_mode` by Task 5).

## Boundary inventory

Crouch tuning crosses Rust ↔ wire (JS/Lua object) ↔ TS ↔ Luau. Field names are camelCase on every script-facing side per the scripting naming convention; Rust uses snake_case. No FGD KVP and no PRL/binary section — the descriptor is a script object, not baked data, and movement tuning is never map-overridable (`movement.md` §7). `crouch_mode` is a `PlayerOptions` TOML settings field (snake_case, NOT script-facing) added by this spec (Task 5) — `player-options` (done) designates the crouch spec as its owner. It crosses Rust ↔ TOML only, never the script-facing wire.

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| crouch sub-descriptor (optional) | `Option<CrouchParams>` | optional object under `movement` | `crouch?: CrouchParams` | `crouch?` | n/a |
| crouched capsule half-height | `half_height: f32` | `halfHeight` | `halfHeight` | `halfHeight` | n/a |
| crouched eye height | `eye_height: f32` | `eyeHeight` | `eyeHeight` | `eyeHeight` | n/a |
| eye-smoothing rate | `transition_rate: f32` | `transitionRate` | `transitionRate` | `transitionRate` | n/a |
| crouched speed tier | `crouch: f32` (on `SpeedParams`) | `crouch` | `crouch` | `crouch` | n/a |
| crouch input action | `Action::Crouch` | n/a (input layer) | n/a | n/a | n/a |
| crouch-intent (resolved) | `crouch_intent: bool` (`MovementInput`) | n/a (runtime input) | n/a | n/a | n/a |
| toggle-vs-hold mode | `crouch_mode: CrouchMode` (`PlayerOptions`, Task 5) | `crouch_mode` (TOML) | n/a (not script-facing) | n/a | n/a |

Units: crouched `halfHeight`/`eyeHeight` metres, `transitionRate` units/sec (or per-sec lerp rate — implementer pins against the chosen smoothing form), `crouch` world-units/sec. Crouched `eyeHeight` must lie in `(0, crouched halfHeight + radius]`, the same exclusive-min/inclusive-max bound the standing `eyeHeight` uses (`validate_in_range_finite_exclusive_min` against the crouched extent). **Wire-casing mechanism** mirrors `dash` exactly: author each crouch wire key literally in (1) the JS parser, (2) the Luau parser, (3) the `register_type("CrouchParams").field(...)` chain plus the optional `crouch?` field on `PlayerMovementDescriptor` and the new `crouch` field on `SpeedParams`, (4) the `"CrouchParams"` entries in both type-name maps in `typedef.rs`, (5) the `EXPECTED_TS`/`EXPECTED_LUAU` test constants.

## Open questions

The movement-feel and ownership questions are resolved and folded into Decisions / Scope:

- `crouch_mode` ownership (was #1) → this spec adds the field + input derivation (Scope In-scope, Task 5); `player-options` (done) designates the crouch spec as owner.
- Smoothing form (was #2) → D3: collision snaps, eye smooths via exponential approach at `transitionRate`.
- Airborne crouch (was #3) → D4: allowed, HEAD-anchored.
- Crouch-jump (was #4) → D10: never suppress (stand-if-clear, else act crouched).
- Crouched speed model (was #5) → D5: `SpeedParams.crouch` tier.

Implementation-precision items remain for the implementer (mechanics, not feel calls — surfaced by review, deliberately not over-specified here):

1. **Stand-up probe construction (D7 / Task 2b).** Pin the cast origin frame and exact distance. With feet planted the head must rise `2 × (standing half_height − crouched half_height)`; casting the STANDING capsule already at its final pose risks false positives — prefer an overlap/zero-TOI test at the final pose, or cast the CROUCHED capsule upward by that delta. State which.
2. **Position-center mutation owner (D2 / Task 2a).** `movement.md` §4 says intents author velocity and the substrate returns position; a resize is a position edit. Pin whether the dispatch/tick applies the helper's returned center delta (recommended) versus the intent writing `position` directly.
3. **Smoothed-eye storage (Task 3 / D3 / D9).** Pin a state-agnostic field the `main.rs` follow read consumes each tick rather than pattern-matching the `Crouching` enum payload — writing the smoothed value into `capsule.eye_height` (which collision ignores; `integrate_collision` reads only `half_height`/`radius`) keeps the read uniform across states.
