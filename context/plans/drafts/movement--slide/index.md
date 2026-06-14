# movement--slide

## Goal

Add a speed-preserving `Sliding` movement state (Titanfall/Apex model) on the shipped state-machine seam: crouching while moving fast banks the player's horizontal momentum as a decaying boost, holds a crouched capsule, **accelerates downhill**, lets the player steer the slide, and — crucially — a slide → jump (or slide → stand) **keeps that speed**. Slide is the first state to consume the momentum-conservation policy in a gameplay-meaningful way (`movement.md` §6) and the first reuse of crouch's capsule-resize substrate helpers.

This spec ships slide **complete**, not as a flat-ground thin slice: it pulls the cross-cutting-policies D8 substrate contact-forwarding forward (slide is its first consumer, ahead of wall-run) and prefaces the feature with a behavior-preserving split of the per-state intents into a submodule, before wall-run + vault pile onto `movement/mod.rs`.

Fifth in the `movement--*` series under Milestone 11 (Advanced Movement). See `movement.md` §2/§4/§5/§6.

## Scope

### In scope
- **Intents-submodule split (Task 1, behavior-preserving):** move the per-state velocity intents and their entry helpers out of `movement/mod.rs` into a `movement/states/` submodule, before slide is built. The dispatch, substrate, carry, scope, and `tick` stay in `mod.rs`. Regression-gated, no behavior change.
- **Substrate floor-normal forwarding (Task 2):** the cross-cutting-policies D8 seam — the substrate already computes the walkable floor normal each tick and discards it; record it on the `SubstrateResult` and carry it forward onto the component for the next tick's intent to read. No existing-state behavior change; slide is the first consumer.
- A `Sliding` variant on `MovementState`, carrying its live data (elapsed-time guard, the decaying boost vector, the eye-smoothing source).
- Entry from `Normal`: crouch-intent while grounded and moving at/above an authored speed gate banks current horizontal velocity (plus an `entryBoost` kick) into the slide boost and shrinks the capsule. Below the gate, the existing `Normal` → `Crouching` path fires (slide takes priority over crouch at speed).
- Per-tick slide physics on the D4 base+boost model: **slope-aware acceleration** (downhill gravity projection scaled by `slopeAssist`, read from the forwarded floor normal), a constant-linear boost decay (`slideDrag`), **direction-steer** (rotate the boost toward input, rate-scaled by `steerControl`), eye-height smoothing to the crouched eye, and the boost↔realized-velocity reconciliation `Dash` uses (no phantom wall kick).
- The momentum-conservation payoff via `KEEP_ALL` carries: slide → jump preserves horizontal speed + boost; slide → stand (crouch released, headroom clear) preserves speed (the "slide cancel"); slide off a ledge keeps speed into the air.
- A `minDurationMs` floor so a quick tap still yields a satisfying slide, composed with `slideDrag` per D9.
- Reuse of crouch's substrate helpers unchanged — `resize_capsule` (Feet anchor on grounded entry), `standup_clearance_probe`, `stand_up_resize` — and the crouched capsule dimensions from `CrouchParams` (slide requires crouch enabled).
- An optional `SlideParams` descriptor (present-then-all-required, like `dash`/`crouch`): six literal fields, dual-path JS + Luau parse/validate, `register_type` emission, type-name maps, `EXPECTED_TS`/`EXPECTED_LUAU` (+ `_WITH_DOCS`) updates, drift-test coverage, regenerated committed `.d.ts`/`.d.luau`. Materialize `slide: Option<SlideParams>` on `PlayerMovementComponent`.
- A `SLIDE_MAX_MS` seamed engine constant (hard cap; mirrors `DASH_MAX_MS`).
- Correcting the `carry.rs` doc-comment that earmarks slide as the first `Zero`/`Scale` consumer (Decision D4).

### Out of scope (non-goals)
- **IR/expression-form slide fields.** `SlideParams` values are plain literals. Expression adoption (the `Dash`/`MovementScope` path) is M14's concern — only *computed/conditional/derived* behavior consolidates onto the command buffer (`scripting.md` §11); slide's six knobs are static tuning. Dash got IR as a dedicated adopter spec; slide does not pull it in.
- **Wall-relative carry rules + wall-normal forwarding.** `projectOntoWallPlane`/`reflect` and the forwarded *wall* normal belong to `movement--wall-run` — slide does not consume them. Building them now is speculation. (Slide forwards only the *floor* normal it actually uses.)
- **New input action.** Slide reuses the resolved `crouch_intent` bit and the derived jump edges — no `Action::Slide`, no `MovementInput` field, no input-layer changes.
- Map-overridable slide tuning (movement physics is descriptor-owned, never FGD KVPs — `movement.md` §7).
- Per-tick script-authored slide (the surface is declarative — `movement.md` §2, §7).

## Decisions

**D1 — `Sliding` is a `Dash`/`Crouching` hybrid on the existing seam.** `MovementState::Sliding { elapsed_ms, boost, eye_current }`: `elapsed_ms` + `boost` mirror `Dash` (time guard + the D4 decaying boost); `eye_current` mirrors `Crouching` (eye-smoothing source). Dispatched in `dispatch_state_intent`, with an `outgoing_boost` arm returning the live boost; per-state data borrowed in place. The boost machinery (reconcile → decay → recombine) and the eye/resize/probe machinery are both reused — little net-new math.

**D2 — Entry banks current momentum (+ `entryBoost` kick); carry is `KEEP_ALL`.** `Normal` → `Sliding` seeds `boost = current horizontal velocity + entryBoost·slide_dir` and leaves the resulting `component.velocity` consistent (the kick is an additive bump along the slide direction). The entry blend is authored entirely in the intent (like `try_enter_dash`), so the dispatch carry is the `KEEP_ALL` no-op. `entryBoost = 0` is pure speed-preservation.

**D3 — Slide requires crouch.** Slide reuses the crouched capsule dimensions (`CrouchParams.half_height`/`eye_height`) and crouch's resize/probe helpers, per the roadmap ("depends on crouch's capsule resize"). When `slide` is present but `crouch` is absent, slide is disabled — `from_descriptor` warns once and the `Normal` → `Sliding` transition never fires (graceful, mirroring the dash bind-failure path). No separate slide capsule height field.

**D4 — Slide consumes `KEEP_ALL`, not `Zero`/`Scale`; correct the `carry.rs` comment.** `movement.md` §6 frames slide's non-trivial carry as "slide→jump keeps slide speed" — a *gameplay-meaningful* `KEEP_ALL` (dash's was a parity no-op; slide's preserves real momentum across a state change). A pure speed-preserving slide on the boost-heavy velocity model has no natural use for `Zero`/`Scale` — `Scale` acts on the base layer (`velocity − boost`), ≈0 during a slide. Re-home the `carry.rs` "first `Zero`/`Scale` consumer" prediction to the environment-probe states (wall-run's wall rules; a vault may `Zero` horizontal on a mantle); record slide as the first gameplay-meaningful `KEEP_ALL` consumer.

**D5 — Going airborne hands off to `Crouching`.** Slide is a grounded mechanic. Leaving the ground mid-slide (no jump) ends the slide with a `KEEP_ALL` transition to `Crouching` (capsule already crouched, `eye_current` handed over), preserving speed into the air. `Crouching` already owns airborne-crouch and stand-up-on-release, so slide does not duplicate that logic.

**D6 — Jump is never suppressed (slide-jump), mirroring crouch D10.** A jump edge during a slide runs the stand-up probe first: clear headroom ⇒ stand (resize to standing) and exit to `Normal` with the jump applied; blocked ⇒ apply the jump and exit to `Crouching` with the crouched capsule retained. Either path uses `KEEP_ALL`, so the launch keeps the slide's horizontal speed — the headline momentum tech. Jump overrides `minDurationMs` (D9).

**D7 — `slideDrag` is a constant linear boost decay (no Normal-friction fold).** Unlike `dashDrag`'s dual path, `slideDrag` always linearly decelerates the boost (world-units/sec²); `0` is a legitimate frictionless ("ice") slide, bounded only by `SLIDE_MAX_MS`, slope, and the input/stand exits. Slide wants its own low, designed friction distinct from `Normal`'s contextual decay, so the base layer is left un-frictioned during the slide.

**D8 — Slope-aware via the substrate floor-normal forwarding (pulls cross-cutting-policies D8 forward).** Slide is the first consumer of the forwarded floor normal. Per the intent/substrate split (`movement.md` §4: "contact flows forward, not sideways"), the slide intent never casts — it reads last tick's floor normal carried on the component. Each tick it projects world gravity onto the floor plane (the downhill-along-slope vector, magnitude `g·sin θ`) and adds `slopeAssist ×` that vector to the boost: downhill accelerates, uphill bleeds faster. Flat ground (`sin θ = 0`) and no forwarded normal (airborne / first tick) degenerate to no slope effect. `slopeAssist` is the authored knob (flexibility band — Neon White vs. Titanfall want different downhill feel).

**D9 — `minDurationMs` and `slideDrag` are complementary, not redundant.** `slideDrag` shapes the *high-speed* decay curve; `minDurationMs` fixes the *low-entry-speed* case (a tap just above `minSpeed` would otherwise hit the speed-decay exit almost immediately). Drag cannot selectively fix the tap case — slowing it lengthens every slide. They compose by `minDurationMs` gating only the **automatic** exits (the speed-decay exit and the crouch-release stand-up); **jump (D6) and ledge handoff (D5) and `SLIDE_MAX_MS` always win**. This is the "committed but jump-cancelable" Titanfall feel.

**D10 — Direction-steer rotates the boost toward input, rate-scaled by `steerControl`.** Unlike dash's short burst (base-only steer is fine for 200 ms), a multi-second slide must be steerable. `steerControl ∈ [0,1]` scales a `SLIDE_MAX_TURN_RATE` engine constant; each tick the boost's horizontal direction rotates toward the input direction by at most that angular rate (magnitude preserved). `0` = committed (no steer), `1` = full turn rate. Keeps `[0,1]` cohesion with dash's `steerControl`.

**D11 — Intents split to a submodule before slide.** The `movement/mod.rs` production code (~1900 lines) is past the split smell, and wall-run + vault are next. Extract the per-state intents first as a behavior-preserving refactor (regression-gated), so slide builds into the clean structure rather than deepening the monolith.

## Acceptance criteria

### Automated (test-gated)
- [ ] **Split is behavior-preserving:** after the intents move to `movement/states/`, the full existing movement + dash + crouch regression suite passes unchanged.
- [ ] **Substrate forwarding is additive:** with the floor-normal now recorded on `SubstrateResult` and forwarded to the component, all existing-state regression tests pass unchanged (no `Normal`/`Dash`/`Crouching` behavior delta); a test asserts the forwarded normal matches the walkable contact on a grounded tick and is absent/zero when airborne.
- [ ] Entry banks speed (+ kick): with `slide` + `crouch` present, grounded, moving at horizontal speed ≥ `slide.minSpeed`, the `crouch_intent` bit enters `Sliding` — collision `half_height` is the crouched value, the capsule's lowest point is unchanged (feet planted), and entry horizontal speed equals pre-entry speed + `entryBoost` (no cap applied).
- [ ] Entry gate: crouch-intent while grounded and moving BELOW `slide.minSpeed` enters `Crouching`, not `Sliding`.
- [ ] Slope acceleration (D8): on a downhill slope, total horizontal speed during a slide increases per `slopeAssist`; on flat ground the slope term is zero (slide bleeds only by `slideDrag`); on an uphill it bleeds faster. `slopeAssist = 0` reproduces the flat (no-slope) curve on any slope.
- [ ] Boost decays: with `slideDrag > 0` on flat ground, total horizontal speed bleeds toward the crouch tier at the constant linear `slideDrag` rate; with `slideDrag = 0` it holds until a slope/input/stand/time-guard exit.
- [ ] Direction-steer (D10): with `steerControl > 0`, sustained input perpendicular to the slide rotates the velocity toward the input direction over time (magnitude preserved within decay); with `steerControl = 0` the slide direction is committed (input does not rotate it).
- [ ] Min-duration floor (D9): with `minDurationMs > 0`, a slide entered just above `minSpeed` (or with crouch released immediately) persists at least `minDurationMs` — the speed-decay and crouch-release exits are suppressed within the floor; a jump within the floor still exits immediately (D6).
- [ ] Natural exit: after the floor, when total horizontal speed decays to ≤ `ground.speed.crouch`, the slide ends — to `Crouching` if `crouch_intent` is still held, else via the stand-up probe to `Normal` (clear) or `Crouching` (blocked).
- [ ] Slide-jump preserves speed (D6): a jump during a slide launches the arc and exits with horizontal speed preserved — to `Normal` when headroom is clear (resize to standing), to `Crouching` when blocked. The jump is never swallowed.
- [ ] Slide-cancel preserves speed (D2/D6): releasing `crouch_intent` after the floor with clear headroom exits to `Normal` keeping horizontal speed; blocked headroom exits to `Crouching`.
- [ ] Ledge handoff (D5): leaving the ground mid-slide (no jump) transitions to `Crouching` with horizontal speed preserved into the air.
- [ ] Time guard: a slide cannot persist past `SLIDE_MAX_MS` even with `slideDrag = 0` on a downhill.
- [ ] Wall reconciliation: a slide driven head-on into a wall projects velocity along the contact (substrate) with no phantom backward kick on the following tick.
- [ ] Slide requires crouch (D3): `slide` present + `crouch` absent disables slide — the `Normal` → `Sliding` transition never fires; materialization warns once.
- [ ] Absent `slide` disables slide: crouch-intent at any speed enters `Crouching` (or nothing if crouch also absent).
- [ ] Present `slide` requires all six fields: an absent inner field is rejected in BOTH JS and Luau (present-then-all-required, like `dash`/`crouch`).
- [ ] Descriptor parsers reject invalid `slide` fields symmetrically in JS and Luau (each path names the offending field; wording/granularity differ per path as `dash`/`crouch` do): `minSpeed` rejects missing/non-finite/non-positive (zero rejected); `slideDrag`, `slopeAssist`, `entryBoost`, `minDurationMs` reject missing/non-finite/negative (zero allowed); `steerControl` rejects missing/non-finite/out-of-`[0,1]`.
- [ ] The SDK type-drift test (`committed_sdk_types_match_current_registry`) passes with `SlideParams` present in `sdk/types/postretro.d.ts` and `.d.luau`, and `slide?` on `PlayerMovementDescriptor`.

### Manual-visual (no automated verification — eyeball in-engine)
- [ ] Sprint → crouch reads as a smooth slide that carries speed; the camera dips to the crouched eye; speed visibly bleeds on flat and builds downhill.
- [ ] Slide → jump chains continuously, launching with the slide's speed (no dead frame, no reset); the slide can be steered around a corner.

## Tasks

### Task 1: Split per-state intents into `movement/states/`
Behavior-preserving refactor, regression-gated, landing BEFORE any slide code (D11). Move the per-state velocity intents (`normal_intent`, `dash_intent`, `crouching_intent`) and their entry helpers (`try_enter_dash`, the dash `resolve_number`/`resolve_bool` helpers) out of `movement/mod.rs` into a `movement/states/` submodule (e.g. `states/normal.rs`, `states/dash.rs`, `states/crouching.rs`, `states/mod.rs`). Keep in `mod.rs`: `tick`, `dispatch_state_intent`, `integrate_collision` and the substrate helpers (`resize_capsule`, `standup_clearance_probe`, `stand_up_resize`, `step_up_lift`), `apply_carry`/`outgoing_boost`, `pm_accelerate`, `wish_dir_from_input`, `air_jump_ready`, `apply_normal_horizontal_decay`, the `JumpEdges`/`derive_jump_edges`/`advance_forgiveness` forgiveness layer, and the engine consts. Promote the shared items the moved intents call to `pub(super)` (or a small `movement/shared.rs`); the intents themselves become `pub(super)`. The large `#[cfg(test)]` module in `mod.rs` stays put (it tests `tick`) — the diff is production-code only. No behavior change: the existing suite is the gate.

### Task 2: Substrate floor-normal forwarding (cross-cutting-policies D8 seam)
Add a forwarded floor-normal field to `SubstrateResult` (e.g. `floor_normal: Option<Vec3>`), populated in `integrate_collision` from the walkable contact it already computes (the slide-loop floor branch and the ground-stick down-cast — record the last walkable normal). Add a `last_floor_normal: Option<Vec3>` field to `PlayerMovementComponent`, written by `tick` from the `SubstrateResult` each tick so the NEXT tick's intent reads last-tick contact (the D8 forward-not-sideways rule). Initialize it `None` in `from_descriptor`. No existing-state behavior change — this only records and forwards data no current state reads. Regression-gated. (Wall-normal forwarding stays out — wall-run's job.)

### Task 3: `SlideParams` data surface — struct, parse/validate, SDK emit, component field
Add a `SlideParams` struct (`data_descriptors.rs`) with six literal fields: `min_speed` (`minSpeed`, `validate_positive_finite`), `slide_drag` (`slideDrag`, `validate_non_negative_finite`), `slope_assist` (`slopeAssist`, `validate_non_negative_finite`), `steer_control` (`steerControl`, `validate_in_range_finite` `[0,1]`), `entry_boost` (`entryBoost`, `validate_non_negative_finite`), `min_duration_ms` (`minDurationMs`, `validate_non_negative_finite`). Parse `slide` as an OPTIONAL sub-object on `PlayerMovementDescriptor` with the present-then-all-required `contains_key`/null-guard discipline `dash`/`crouch` use, via `slide_params_from_js`/`slide_params_from_lua` mirroring `crouch_params_from_*`. Materialize `slide: Option<SlideParams>` onto `PlayerMovementComponent` in `from_descriptor` (clone, mirrors `crouch`); when `slide.is_some() && crouch.is_none()`, warn once and store `slide = None` (D3). Emit the SDK type: `register_type("SlideParams").field(...)` in `primitives/mod.rs` alongside `DashParams`/`CrouchParams`; add an optional `slide?: SlideParams` field to the `PlayerMovementDescriptor` chain (mirror the optional `crouch?` field); add `"SlideParams" => "SlideParams".to_string()` to BOTH type-name maps in `typedef.rs`; update `EXPECTED_TS`/`EXPECTED_LUAU` (and `_WITH_DOCS` — reword the `PlayerMovementDescriptor` `.doc()` to name `slide` as another optional sub-object); regenerate the committed `.d.ts`/`.d.luau` and confirm the drift test passes. Adding `slide: Option<SlideParams>` to `PlayerMovementDescriptor` breaks every `PlayerMovementDescriptor { .. }` literal (the `mod.rs`, `player_movement.rs`, and `scope.rs` test modules) — add `slide: None` to each, exactly as the `crouch` field did. Pure data-surface work; see Boundary inventory.

### Task 4: `Sliding` state + intent + transitions
Add `MovementState::Sliding { elapsed_ms: f32, boost: Vec3, eye_current: f32 }` (`player_movement.rs`); dispatch it in `dispatch_state_intent`; add a `Sliding` arm to `outgoing_boost` returning the live `boost`. Add `try_enter_slide` (mirroring `try_enter_dash`) and wire it into `normal_intent`'s crouch branch: when `crouch_intent` is active, grounded, `slide` + `crouch` both present, and horizontal speed ≥ `slide.min_speed`, enter `Sliding`; otherwise fall through to `Normal` → `Crouching`. `try_enter_slide` seeds `boost = horizontal velocity + entry_boost·slide_dir` (D2), resizes the capsule to the crouched dimensions via `resize_capsule` (Feet anchor) applying the center delta to `position`, seeds `eye_current` at the standing eye, returns `Transition { Sliding{..}, KEEP_ALL }`. Add `sliding_intent` in `states/sliding.rs` (mirror `dash_intent`): gravity (airborne); **slope accel** from `component.last_floor_normal` scaled by `slope_assist` (D8); reconcile the tracked `boost` against realized velocity (the dash clamp); **direction-steer** rotating the boost toward input at `steer_control × SLIDE_MAX_TURN_RATE` (D10); constant-linear `slide_drag` decay of the boost (D7); recombine into `component.velocity`; eye smoothing toward the crouched eye via the crouch exponential-approach using `CrouchParams.transition_rate`, written to `capsule.eye_height`; accumulate `elapsed_ms`. Exit order: jump edge (D6) and ledge/airborne handoff (D5) ALWAYS checked first; then, only when `elapsed_ms ≥ min_duration_ms` (D9), the crouch-release stand-up and the speed-decay exit (total horizontal speed ≤ `ground.speed.crouch`); `elapsed_ms ≥ SLIDE_MAX_MS` always forces exit. Exits route → `Normal` (stand, clear) / `Crouching` (blocked or crouch-held), all `KEEP_ALL`. Add `SLIDE_MAX_MS` and `SLIDE_MAX_TURN_RATE` engine consts near `DASH_MAX_MS`. Update the `carry.rs` doc-comments per D4. No `apply_carry`/`tick` carry changes — the seam already routes `KEEP_ALL`.

## Sequencing

**Phase 1 (sequential):** Task 1 — the intents-submodule split reshapes where slide's intent lands; everything builds on the new structure.
**Phase 2 (concurrent):** Task 2 (substrate forwarding) ∥ Task 3 (data surface) — Task 2 touches `mod.rs`'s substrate + a component field; Task 3 touches the descriptor/SDK layer + a component field. Both add a `PlayerMovementComponent` field — coordinate (or sequence) the two `player_movement.rs` edits to avoid a collision.
**Phase 3 (sequential):** Task 4 — consumes the new submodule (Task 1), the forwarded floor normal (Task 2), and `SlideParams` + the component `slide` field (Task 3).

## Rough sketch

`Sliding` is a `Dash`/`Crouching` hybrid behind the same seam — boost machinery from `dash_intent`, eye/resize/probe from `crouching_intent` + the substrate helpers.

Entry (`try_enter_slide`, from `normal_intent`'s crouch branch before `Normal` → `Crouching`):
```
// Proposed design
if input.crouch_intent && component.is_grounded {
    if let (Some(slide), Some(crouch)) = (component.slide, component.crouch) {
        let horiz = Vec2::new(component.velocity.x, component.velocity.z);
        if horiz.length() >= slide.min_speed {
            let dir = horiz.normalize_or_zero();
            let boost = Vec3::new(component.velocity.x, 0.0, component.velocity.z)
                      + Vec3::new(dir.x, 0.0, dir.y) * slide.entry_boost;
            component.velocity.x = boost.x; component.velocity.z = boost.z;
            let eye_current = component.capsule.eye_height; // standing eye, smooths down
            let delta = resize_capsule(component, crouch.half_height, crouch.eye_height, ResizeAnchor::Feet);
            position.y += delta;
            component.capsule.eye_height = eye_current; // restore: eye smooths, capsule snaps
            return Some(Transition { next: Sliding { elapsed_ms: 0.0, boost, eye_current }, carry: KEEP_ALL });
        }
    }
}
// else: existing Normal -> Crouching path
```

`sliding_intent` per tick: gravity → slope accel (from `last_floor_normal`) → boost reconcile (dash clamp) → direction-steer (rotate boost) → linear `slide_drag` decay → recombine → eye smooth → `elapsed_ms += dt*1000` → exit checks (jump/ledge always; auto/release gated by `min_duration_ms`; `SLIDE_MAX_MS` always). Total velocity = (`velocity − boost`) base + boost; slope feeds the boost, drag bleeds it, steer rotates it.

**Slope term (D8):** with `n = last_floor_normal`, the downhill-along-plane gravity is `a = gravity_vec − (gravity_vec·n) n` (points downhill, magnitude `g·sin θ`); add `slope_assist · a · dt` to the boost. The implementer pins horizontal-vs-3D projection; the substrate's ground-stick keeps the slide on the floor.

**Interaction to document at promotion (not a bug): airborne speed cap.** `Normal`'s airborne branch caps horizontal speed to the run tier *only when movement input is given* and `air.bunny_hop` is off. A slide-jump / ledge handoff above the run cap keeps that speed in the air only while the player gives no air input, or when `air.bunny_hop` is enabled; steering in air without bunny-hop re-caps. Existing `Normal` behavior, surfaced because slide is the first state that routinely hands off above the run cap — capture in `movement.md` at promotion.

## Boundary inventory

Slide tuning crosses Rust ↔ wire (JS/Lua object) ↔ TS ↔ Luau. camelCase on every script-facing side; snake_case in Rust. No FGD KVP, no PRL/binary section — descriptor is a script object, never map-overridable (`movement.md` §7). No new input action or `MovementInput` field (slide reuses `crouch_intent` + the jump edges). Wire-casing mechanism mirrors `dash`/`crouch` exactly (author each key literally in the JS parser, the Luau parser, the `register_type("SlideParams").field(...)` chain plus the optional `slide?` field on `PlayerMovementDescriptor`, both `typedef.rs` type-name maps, and the `EXPECTED_TS`/`EXPECTED_LUAU` constants).

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| slide sub-descriptor (optional) | `Option<SlideParams>` | optional object under `movement` | `slide?: SlideParams` | `slide?` | n/a |
| entry speed gate | `min_speed: f32` | `minSpeed` | `minSpeed` | `minSpeed` | n/a |
| boost linear decel | `slide_drag: f32` | `slideDrag` | `slideDrag` | `slideDrag` | n/a |
| downhill assist scale | `slope_assist: f32` | `slopeAssist` | `slopeAssist` | `slopeAssist` | n/a |
| in-slide steering | `steer_control: f32` | `steerControl` | `steerControl` | `steerControl` | n/a |
| entry speed kick | `entry_boost: f32` | `entryBoost` | `entryBoost` | `entryBoost` | n/a |
| min slide floor | `min_duration_ms: f32` | `minDurationMs` | `minDurationMs` | `minDurationMs` | n/a |
| forwarded floor normal (runtime) | `last_floor_normal: Option<Vec3>` | n/a (runtime state) | n/a | n/a | n/a |

Units: `minSpeed` world-units/sec (finite > 0), `slideDrag`/`entryBoost` world-units/sec & sec² (finite ≥ 0), `slopeAssist` unitless ≥ 0, `steerControl` unitless `[0,1]`, `minDurationMs` ms (finite ≥ 0). Crouched capsule dimensions come from `CrouchParams` (D3). The `?` marks the whole sub-object optional: absent ⇒ slide disabled; present ⇒ all six fields required.

## Open questions
- **`SLIDE_MAX_TURN_RATE` value.** The engine-const ceiling `steerControl` scales. Pick a feel-friendly rate in-engine (on the order of a half-turn per second); not architectural.
- **Slope projection precision (D8).** Whether the slope term applies in full 3D (let the substrate re-project) or as a pre-projected horizontal vector — pinned at implementation against the ground-stick behavior; both reduce to no-op on flat ground.
- **Slope-aware exit on steep downhill.** A steep slope can keep a slide above the crouch tier indefinitely; `SLIDE_MAX_MS` is the backstop. Confirm in-engine the cap reads as intentional (long fast slides) rather than abrupt.
