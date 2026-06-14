# movement--slide

## Goal

Add a speed-preserving `Sliding` movement state (Titanfall/Apex model) on the shipped state-machine seam: crouching while moving fast banks the player's horizontal momentum as a decaying boost, holds a crouched capsule, and — crucially — a slide → jump (or slide → stand) **keeps that speed**. Slide is the first state to consume the momentum-conservation policy in a gameplay-meaningful way (`movement.md` §6) and the first reuse of crouch's capsule-resize substrate helpers.

Fifth in the `movement--*` series under Milestone 11 (Advanced Movement), after crouch and the cross-cutting policies. See `movement.md` §2/§4/§5/§6.

## Scope

### In scope
- A `Sliding` variant on `MovementState` (`scripting/components/player_movement.rs`), plugging into the existing intent/substrate/transition seam, carrying its own live data (elapsed-time guard, the decaying boost vector, the eye-smoothing source).
- Entry from `Normal`: crouch-intent while grounded and moving at/above an authored speed gate banks the current horizontal velocity into the slide boost and shrinks the capsule. Below the gate, the existing `Normal` → `Crouching` path fires instead (slide takes priority over crouch at speed).
- Per-tick slide physics on the D4 base+boost model: a constant-linear boost decay (`slideDrag`), limited input steering (`steerControl`), eye-height smoothing to the crouched eye, and the same boost↔realized-velocity reconciliation `Dash` uses (so sliding into a wall produces no phantom kick).
- The momentum-conservation payoff via `KEEP_ALL` carries: slide → jump preserves horizontal speed + boost; slide → stand (crouch released, headroom clear) preserves speed (the "slide cancel"); slide off a ledge keeps speed into the air.
- Reuse of crouch's substrate helpers unchanged — `resize_capsule` (Feet anchor on grounded entry), `standup_clearance_probe`, `stand_up_resize` — and the crouched capsule dimensions from `CrouchParams` (slide requires crouch enabled).
- An optional `SlideParams` descriptor (present-then-all-required, like `dash`/`crouch`): dual-path JS + Luau parse/validate, `register_type` emission, type-name maps, `EXPECTED_TS`/`EXPECTED_LUAU` (+ `_WITH_DOCS`) updates, drift-test coverage, regenerated committed `.d.ts`/`.d.luau`. Materialize `slide: Option<SlideParams>` on `PlayerMovementComponent`.
- A `SLIDE_MAX_MS` seamed engine constant bounding the state (mirrors `DASH_MAX_MS`).
- Correcting the `carry.rs` doc-comment that earmarks slide as the first `Zero`/`Scale` consumer (Decision D4).

### Out of scope (non-goals)
- **Slope-aware slide** — continuous downhill acceleration. It needs the floor-normal contact data the substrate forwards, deferred to `movement--wall-run` (cross-cutting-policies D8). Slide v1 banks *current* speed at entry (whether earned from sprint or a prior downhill run) but does not accelerate from slope during the slide.
- **New input action.** Slide reuses the resolved `crouch_intent` bit and the derived jump edges — no `Action::Slide`, no `MovementInput` field, no input-layer changes.
- **IR/expression-form slide fields.** `SlideParams` values are plain literals. Expression adoption (the `Dash`/`MovementScope` path) is deferred to a future adopter, consistent with dash's dedicated M14 spec.
- **Full direction-steer of the committed slide.** v1 steering nudges the *base* layer only (the boost stays committed in the entry direction, decaying); steering the whole velocity vector is deferred (Open questions).
- **An entry speed kick / `entryBoost`.** v1 is purely speed-*preserving* (≈1:1). A tunable entry boost is deferred (Open questions).
- A minimum slide duration floor. Slide exits purely on speed decay / input / the time guard.
- Map-overridable slide tuning (movement physics is descriptor-owned, never FGD KVPs — `movement.md` §7).
- Per-tick script-authored slide (the surface is declarative — `movement.md` §2, §7).
- Splitting the oversized `movement/mod.rs` (Open questions).

## Decisions

**D1 — `Sliding` is a state on the existing seam, shaped as a `Dash`/`Crouching` hybrid.** A `MovementState::Sliding { elapsed_ms, boost, eye_current }` variant: `elapsed_ms` + `boost` mirror `Dash` (time guard + the D4 decaying boost the `slideDrag` bleeds); `eye_current` mirrors `Crouching` (the eye-smoothing source). Dispatched in `dispatch_state_intent`, with an `outgoing_boost` arm returning the live boost. Per-state data borrowed in place by the dispatch (D7 of cross-cutting-policies) — no widening of `tick`.

**D2 — Entry banks current momentum as the boost; carry is `KEEP_ALL`.** `Normal` → `Sliding` seeds `boost = current horizontal velocity` and leaves `component.velocity` unchanged (speed-preserving). Because `Normal` carries no boost, the entry blend is authored entirely in the intent (like `try_enter_dash`) and the dispatch carry is the `KEEP_ALL` no-op. Per the D4 layering, `base = velocity − boost ≈ 0` at entry; the base then builds from steering while the boost decays, so total velocity transitions smoothly from banked momentum to crouch-speed locomotion.

**D3 — Slide requires crouch.** Slide reuses the crouched capsule dimensions (`CrouchParams.half_height`/`eye_height`) and crouch's resize/probe helpers, exactly as the roadmap states ("depends on crouch's capsule resize"). When `slide` is present but `crouch` is absent, slide is disabled — `from_descriptor` warns once and the `Normal` → `Sliding` transition never fires (graceful degradation, mirroring the dash bind-failure path). No separate slide capsule height field.

**D4 — Slide consumes `KEEP_ALL`, not `Zero`/`Scale`; correct the `carry.rs` comment.** `movement.md` §6 frames slide's non-trivial carry as "slide→jump keeps slide speed" — a *gameplay-meaningful* `KEEP_ALL` (dash's `KEEP_ALL` was a parity no-op; slide's preserves real momentum across a state change). A pure speed-preserving slide, on the boost-heavy velocity model, has no natural use for `Zero`/`Scale` — and `Scale` acts on the base layer (`velocity − boost`), which is ≈0 during a slide, so it is ill-suited regardless. The `carry.rs` doc-comment currently predicts slide as "the first non-trivial consumer of `Zero` or `Scale`"; re-home that prediction to the environment-probe states (wall-run's wall rules; a vault may `Zero` horizontal on a mantle) and record that slide is the first gameplay-meaningful `KEEP_ALL` consumer.

**D5 — Going airborne hands off to `Crouching`.** Slide is a grounded mechanic. The moment the player leaves the ground mid-slide (walks off a ledge, no jump), the slide ends with a `KEEP_ALL` transition to `Crouching` (capsule already crouched, `eye_current` handed over) — preserving speed into the air. `Crouching` already owns airborne-crouch and stand-up-on-release, so slide does not duplicate that logic.

**D6 — Jump is never suppressed (slide-jump), mirroring crouch D10.** A jump edge during a slide runs the stand-up probe first: clear headroom ⇒ stand (resize to standing) and exit to `Normal` with the jump applied; blocked ⇒ apply the jump and exit to `Crouching` with the crouched capsule retained. Either path uses `KEEP_ALL`, so the launch keeps the slide's horizontal speed — the headline momentum tech.

**D7 — `slideDrag` is a constant linear boost decay (no Normal-friction fold).** Unlike `dashDrag`'s dual path, `slideDrag` always linearly decelerates the boost (world-units/sec²); `0` is a legitimate frictionless ("ice") slide, bounded only by `SLIDE_MAX_MS` and the input/stand exits. Slide wants its own low, designed friction distinct from `Normal`'s contextual decay, so the base layer is left un-frictioned during the slide.

## Acceptance criteria

### Automated (test-gated)
- [ ] Entry banks speed: with `slide` + `crouch` present, grounded, moving at horizontal speed ≥ `slide.minSpeed`, the resolved `crouch_intent` bit enters `Sliding` — the collision `half_height` equals the crouched value, the capsule's lowest point is unchanged (feet planted), and entry horizontal speed is preserved (≈ pre-entry, no cap applied).
- [ ] Entry gate: crouch-intent while grounded and moving BELOW `slide.minSpeed` enters `Crouching`, not `Sliding` (slide priority is speed-gated).
- [ ] Boost decays: during a slide with `slideDrag > 0`, total horizontal speed bleeds toward the crouch tier at the constant linear `slideDrag` rate; with `slideDrag = 0` it holds (no decay) until an input/stand/time-guard exit.
- [ ] Natural exit: when total horizontal speed decays to ≤ `ground.speed.crouch`, the slide ends — to `Crouching` if `crouch_intent` is still held, else via the stand-up probe to `Normal` (clear) or `Crouching` (blocked).
- [ ] Slide-jump preserves speed (D6): a jump during a slide launches the jump arc and exits with horizontal speed preserved — to `Normal` when headroom is clear (resize to standing), to `Crouching` when blocked (crouched capsule retained). The jump is never swallowed.
- [ ] Slide-cancel preserves speed (D2/D6): releasing `crouch_intent` mid-slide with clear headroom exits to `Normal` keeping horizontal speed; blocked headroom exits to `Crouching`.
- [ ] Ledge handoff (D5): leaving the ground mid-slide (no jump) transitions to `Crouching` with horizontal speed preserved into the air.
- [ ] Time guard: a slide cannot persist past `SLIDE_MAX_MS` even if `slideDrag = 0` and speed stays high.
- [ ] Wall reconciliation: driving a slide head-on into a wall projects velocity along the contact (substrate) with no phantom backward kick on the following tick (the boost↔realized-velocity clamp `Dash` uses, applied in `Sliding`).
- [ ] Slide requires crouch (D3): a descriptor with `slide` present but `crouch` absent disables slide — the `Normal` → `Sliding` transition never fires regardless of speed/crouch-intent; materialization warns once.
- [ ] Absent `slide` descriptor disables slide: crouch-intent at any speed enters `Crouching` (or nothing if crouch also absent); no slide ever occurs.
- [ ] Present `slide` requires all fields: an absent inner field is rejected in BOTH the JS and Luau paths (present-then-all-required, like `dash`/`crouch`).
- [ ] Descriptor parsers reject invalid `slide` fields symmetrically in JS and Luau (each path names the offending field; wording/granularity differ per path as `dash`/`crouch` already do): `minSpeed` rejects missing/non-finite/non-positive (zero rejected); `slideDrag` rejects missing/non-finite/negative (zero allowed); `steerControl` rejects missing/non-finite/out-of-`[0,1]`.
- [ ] The SDK type-drift test (`committed_sdk_types_match_current_registry`) passes with `SlideParams` present in `sdk/types/postretro.d.ts` and `.d.luau`, and `slide?` on `PlayerMovementDescriptor`.
- [ ] The full existing movement + dash + crouch regression suite passes unchanged (the slide path is additive; no `Normal`/`Dash`/`Crouching` behavior delta).

### Manual-visual (no automated verification — eyeball in-engine)
- [ ] Sprint → crouch reads as a smooth slide that carries speed, the camera dips to the crouched eye, and speed visibly bleeds over the slide.
- [ ] Slide → jump chains continuously, launching with the slide's speed (no dead frame, no speed reset on hand-off).

## Tasks

### Task 1: `SlideParams` data surface — struct, parse/validate, SDK emit, component field
Add a `SlideParams` struct (`data_descriptors.rs`) with three literal fields: `min_speed` (`minSpeed`, `validate_positive_finite`), `slide_drag` (`slideDrag`, `validate_non_negative_finite`), `steer_control` (`steerControl`, `validate_in_range_finite` `[0,1]`). Parse `slide` as an OPTIONAL sub-object on `PlayerMovementDescriptor` with the present-then-all-required `contains_key`/null-guard discipline `dash`/`crouch` use, via `slide_params_from_js`/`slide_params_from_lua` mirroring `crouch_params_from_*`. Materialize `slide: Option<SlideParams>` onto `PlayerMovementComponent` in `from_descriptor` (clone, mirrors `crouch`); when `slide.is_some() && crouch.is_none()`, warn once and store `slide = None` (D3). Emit the SDK type: `register_type("SlideParams").field(...)` in `primitives/mod.rs` alongside `DashParams`/`CrouchParams`; add an optional `slide?: SlideParams` field to the `PlayerMovementDescriptor` chain (mirror the optional `crouch?` field); add `"SlideParams" => "SlideParams".to_string()` to BOTH type-name maps in `typedef.rs`; update `EXPECTED_TS`/`EXPECTED_LUAU` (and the `_WITH_DOCS` constants — reword the `PlayerMovementDescriptor` `.doc()` to name `slide` as another optional sub-object); regenerate the committed `.d.ts`/`.d.luau` and confirm the drift test passes. Adding `slide: Option<SlideParams>` to `PlayerMovementDescriptor` breaks every `PlayerMovementDescriptor { .. }` literal (the `mod.rs` test module and `player_movement.rs` tests) — add `slide: None` to each, exactly as the `crouch` field did. Pure data-surface work; see Boundary inventory.

### Task 2: `Sliding` state + intent + entry/exit transitions
Add `MovementState::Sliding { elapsed_ms: f32, boost: Vec3, eye_current: f32 }` (`player_movement.rs`) and dispatch it in `dispatch_state_intent` alongside `Normal`/`Dash`/`Crouching`; add a `Sliding` arm to `outgoing_boost` returning the live `boost`. Add a `try_enter_slide` helper (mirroring `try_enter_dash`) and wire it into `normal_intent`'s crouch branch: when `crouch_intent` is active, grounded, `slide` and `crouch` both present, and horizontal speed ≥ `slide.min_speed`, enter `Sliding`; otherwise fall through to the existing `Normal` → `Crouching` path (slide priority is speed-gated). `try_enter_slide` seeds `boost = current horizontal velocity`, resizes the capsule to the crouched dimensions via `resize_capsule` (Feet anchor, grounded) applying the center delta to `position`, seeds `eye_current` at the standing eye (so it smooths down), and returns `Transition { Sliding{..}, KEEP_ALL }` (D2). Add the `sliding_intent` (mirror `dash_intent`'s structure): gravity (airborne); reconcile the tracked `boost` against realized velocity (the same clamp `dash_intent` applies, to kill phantom wall kicks); steering via `pm_accelerate` toward `ground.speed.crouch` scaled by `steer_control` (omitted at 0); constant-linear `slide_drag` decay of the boost only (D7); recombine into `component.velocity`; eye smoothing toward the crouched eye via the crouch exponential-approach form using `CrouchParams.transition_rate`, written to `capsule.eye_height`; accumulate `elapsed_ms`. Exit order — airborne handoff (D5: → `Crouching{eye_current}`, `KEEP_ALL`); jump edge (D6: stand-up probe → stand + `Normal` if clear, else jump + `Crouching`, `KEEP_ALL`); `crouch_intent` released (stand-up probe → `Normal` if clear, else `Crouching`, `KEEP_ALL`); total horizontal speed ≤ `ground.speed.crouch` OR `elapsed_ms ≥ SLIDE_MAX_MS` (→ `Crouching` if crouch held, else stand-up probe → `Normal`/`Crouching`, `KEEP_ALL`). Add the `SLIDE_MAX_MS` engine `const` near `DASH_MAX_MS`. Update the `carry.rs` doc-comments per D4 (re-home the `Zero`/`Scale` "first consumer" prediction to wall-run/vault; note `KEEP_ALL`'s first gameplay-meaningful consumer is slide). All exits use `KEEP_ALL`; no `apply_carry`/`tick` changes — the seam already routes it.

## Sequencing

**Phase 1 (sequential):** Task 1 — the data surface and the `slide: Option<SlideParams>` component field; Task 2 consumes both.
**Phase 2 (sequential):** Task 2 — the `Sliding` state, intent, entry/exit transitions, and the `carry.rs` comment fix. Depends on Task 1's `SlideParams` type and component field; both tasks edit `player_movement.rs` (Task 1 the field + `from_descriptor`, Task 2 the `MovementState` variant), so they are sequenced rather than concurrent.

## Rough sketch

`Sliding` is a `Dash`/`Crouching` hybrid behind the same seam. The boost machinery (reconcile → decay → recombine), the elapsed-time guard, and the speed-band exit come from `dash_intent`; the eye smoothing, capsule resize, and stand-up probe come from `crouching_intent`/the substrate helpers — both reused, little net-new math.

Entry (`try_enter_slide`, called from `normal_intent`'s crouch branch before `Normal` → `Crouching`):
```
// Proposed design
if input.crouch_intent && component.is_grounded {
    if let (Some(slide), Some(crouch)) = (component.slide, component.crouch) {
        let horiz = Vec2::new(component.velocity.x, component.velocity.z).length();
        if horiz >= slide.min_speed {
            let boost = Vec3::new(component.velocity.x, 0.0, component.velocity.z);
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

`sliding_intent` per tick: gravity → boost reconcile (dash clamp) → steering (`pm_accelerate` × `steer_control`) → linear `slide_drag` boost decay → recombine → eye smooth → exit checks. Total velocity = (`velocity − boost`) base + decayed boost; only the boost decays, so the slide bleeds from banked speed toward crouch-speed locomotion.

**Interaction to document (not a bug): airborne speed cap.** `Normal`'s airborne branch caps horizontal speed to the run tier *only when movement input is given* and `air.bunny_hop` is off. So a slide-jump (or ledge handoff) that exits above run speed keeps that speed in the air only while the player gives no air input, or when `air.bunny_hop` is enabled; steering in air without bunny-hop re-caps to run speed. This is existing `Normal` behavior, surfaced here because slide is the first state that routinely hands off above the run cap. Capture it in `movement.md` at promotion.

## Boundary inventory

Slide tuning crosses Rust ↔ wire (JS/Lua object) ↔ TS ↔ Luau. Field names are camelCase on every script-facing side; Rust uses snake_case. No FGD KVP and no PRL/binary section — the descriptor is a script object, never map-overridable (`movement.md` §7). No new input action or `MovementInput` field (slide reuses `crouch_intent` + the jump edges). Wire-casing mechanism mirrors `dash`/`crouch` exactly (author each key literally in the JS parser, the Luau parser, the `register_type("SlideParams").field(...)` chain plus the optional `slide?` field on `PlayerMovementDescriptor`, both `typedef.rs` type-name maps, and the `EXPECTED_TS`/`EXPECTED_LUAU` test constants).

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| slide sub-descriptor (optional) | `Option<SlideParams>` | optional object under `movement` | `slide?: SlideParams` | `slide?` | n/a |
| entry speed gate | `min_speed: f32` | `minSpeed` | `minSpeed` | `minSpeed` | n/a |
| boost linear decel | `slide_drag: f32` | `slideDrag` | `slideDrag` | `slideDrag` | n/a |
| in-slide steering | `steer_control: f32` | `steerControl` | `steerControl` | `steerControl` | n/a |

Units: `minSpeed` world-units/sec (finite > 0), `slideDrag` world-units/sec² (finite ≥ 0), `steerControl` unitless `[0,1]`. The crouched capsule dimensions come from `CrouchParams` (D3), not `SlideParams`. The `?` marks the whole sub-object optional: absent ⇒ slide disabled; present ⇒ all three fields required.

## Open questions
- **Full direction-steer vs base-only steer.** v1 nudges only the base layer (boost stays committed in the entry direction). Steering the whole velocity vector (rotating the boost) feels better but adds a steer-the-boost model. Deferred; revisit if the committed slide reads too rigid in-engine.
- **Entry speed kick.** Apex gives a small boost entering a slide. v1 is purely speed-preserving. A tunable `entryBoost` field (or a `Scale`-carry entry kick) is deferred; if added later it is a non-breaking new `SlideParams` field.
- **Oversized `movement/mod.rs` (~6000 lines; ~1900 production + ~4100 tests).** Past the ~800-line split-before-extend smell, but the production half is one cohesive state machine and three prior `movement--*` specs extended it in place. Recommend NOT blocking slide on a split (precedent + cohesion + merge risk for the remaining wall-run/vault); flag a dedicated "movement intents → submodule" split as a future spec after the series settles.
- **Slope-aware downhill acceleration.** Confirmed out of scope (needs the deferred floor-normal contact data, cross-cutting-policies D8 / wall-run). When wall-run lands the substrate contact forwarding, a follow-up can add downhill acceleration as a non-breaking `SlideParams` field.
