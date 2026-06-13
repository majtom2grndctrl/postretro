# Player Descriptor Composition

## Goal

Reorganize the player movement descriptor from bolted-on flat siblings into a composed surface with three load-bearing ideas: **states are the primary axis**, **modifiers compose under states**, and **viewFeel is a layered override stack**. This is the direction document for the descriptor API — new specs (slide, wall-run, weapons, AI) extend this shape instead of adding more top-level keys. A companion file (`m10-example.md`) demonstrates the target surface across the full M10 scripting vocabulary.

## Background — what's wrong today

Today's `PlayerMovementDescriptor` grew one feature at a time:

- `ground`, `air`, `crouch`, `dash` are flat siblings with mixed roles — `ground` and `air` are states, `crouch` is a state whose speed lives in `ground.speed.crouch`, and `dash` is a modifier pretending to be a state.
- Dash cannot express "ground dashes differ from air dashes" except through IR expressions branching on `grounded` — a workaround, not a shape. `airDashes` is a dash field rather than an air-state fact.
- `viewFeel` differentiates walk vs run only implicitly (speed feeds amplitude). Per-channel `groundedOnly` booleans are a one-off override mechanism that doesn't extend to crouch, dash, slide, or carrying a heavy weapon.
- `fall` is a one-field top-level block that is really an air-state parameter.

## The composition model

Three rules, applied everywhere the descriptor grows:

### 1. States are the primary axis

Movement tuning lives under `states`, keyed by the engine's closed state vocabulary (`movement.md` §2): `ground`, `air`, `crouch` today; `slide`, `wallRun`, `vault` as they land. A state block carries that state's velocity-intent tuning and nothing else's. Crouch speed lives on crouch, terminal velocity lives on air.

### 2. Modifiers compose under states

A **modifier** is a time-bounded velocity overlay entered *from* a state (dash first; future candidates follow the same shape). A modifier declares **shared defaults** once in a top-level `modifiers` block; each state opts in by carrying a child block of the same name, whose fields sparsely override the defaults.

- **Presence = availability.** `states.ground.dash: {}` enables ground dash with pure defaults; omitting `dash` under a state disables dashing from it.
- **Per-state specialization is sparse.** `states.air.dash.momentumRetention` overrides only that field; everything else resolves from `modifiers.dash`.
- Budget fields live where their lifecycle lives: `charges` on `states.air.dash` (refreshed by the landing-refresh point), shared `cooldownMs` on the defaults.
- Expression-valued fields (`number | RuntimeValue`) keep working at either level, with the same per-field engine-pinned evaluation moment.

### 3. viewFeel is a layered override stack

`viewFeel` becomes `base` plus sparse `layers` keyed by a closed set of tiers and states. Resolution: deep sparse merge per channel, in fixed precedence order; `channel: null` disables a channel at that layer.

```
base  <  tier (walk | run)  <  state (crouch | air | slide | wallRun)  <  modifier (dash)  <  wieldable overlay  <  future (status effects)
```

- **Tiers are input-selected** (the run modifier key), not speed-measured — no hysteresis. Exactly one of `walk`/`run` is active while grounded and moving.
- **States and modifiers** activate their layer while active; movement states are exclusive, so at most one state layer plus the dash layer apply at once.
- **`groundedOnly` is deleted.** `layers.air: { bob: null, tilt: null }` says the same thing in the general mechanism.
- **Wieldable overlay** is the extension point for weapons: a weapon descriptor may carry its own sparse `viewFeel` layer (sway amplitude up, tilt tension down for a heavy gun), applied while wielded. Direction only — lands with wieldable-instance work, not this plan.
- viewFeel stays render-side, read-only, integrator-state engine-owned (`movement.md` §1).

The closed-vocabulary rule holds throughout: layer keys, state keys, and modifier names are engine vocabulary; authors compose data, never register states or callbacks.

## Proposed descriptor shape

```ts
// Proposed design — replaces today's PlayerMovementDescriptor wholesale (pre-stable, no dual-shape support).
movement: {
  capsule: { radius, halfHeight, eyeHeight },          // unchanged — cross-state
  states: {
    ground: {
      speed: { walk, run },                            // crouch speed moved out
      accel, stepHeight, maxSlope,
      dash?: { /* sparse override of modifiers.dash */ },
    },
    air: {
      forwardSteer, accel, maxControlSpeed, bunnyHop,
      terminalVelocity,                                // was top-level fall.terminalVelocity
      jump: { velocity, ceiling, airCount },           // was jumpVelocity / jumpCeiling / jumps
      dash?: { charges, preserveVertical, /* … */ },
    },
    crouch?: { speed, halfHeight, eyeHeight, transitionRate },
  },
  modifiers?: {
    dash?: { boostSpeed, momentumRetention, steerControl, drag, cooldownMs, preserveVertical },
  },
  viewFeel?: {
    base: { bob?, tilt?, sway? },                      // channel params unchanged, minus groundedOnly
    layers?: { walk?, run?, crouch?, air?, dash? },    // sparse per-channel overrides; null disables
  },
  forgiveness?: { coyoteMs?, jumpBufferMs? },          // unchanged
  stuckStopEnabled?, stuckStopThreshold?,              // unchanged
}
```

## Migration mapping

| Today | Proposed |
|---|---|
| `ground.speed.crouch` | `states.crouch.speed` |
| `fall.terminalVelocity` | `states.air.terminalVelocity` |
| `air.jumps` / `jumpVelocity` / `jumpCeiling` | `states.air.jump.airCount` / `.velocity` / `.ceiling` |
| `dash` (flat block) | `modifiers.dash` defaults + per-state `states.<s>.dash` enable/override |
| `dash.airDashes` | `states.air.dash.charges` |
| `dash.dashDrag` | `modifiers.dash.drag` |
| `viewFeel.{bob,tilt,sway}` | `viewFeel.base.{bob,tilt,sway}` |
| `bob/tilt.groundedOnly: true` (defaults) | explicit `viewFeel.layers.air: { bob: null, tilt: null }` — no implicit default |
| `sway.groundedOnly` | `viewFeel.layers.air.sway: null` when wanted |

## Scope

### In scope

- The reshaped `PlayerMovementDescriptor` (states / modifiers / dash-per-state) on both script surfaces (TS + Luau), parsing, validation, component materialization.
- viewFeel `base` + `layers` with tier (walk/run), state (crouch/air), and dash layers; `groundedOnly` removed.
- Behavior preservation: a migrated descriptor produces identical tick output for mapped fields.
- SDK typedef regeneration, example content (`content/dev/scripts/player.ts`) migration, modder docs update.
- A `data_descriptors.rs` split (5,581 lines) ahead of the reshape.

### Out of scope

- Wieldable viewFeel overlay (precedence slot reserved here; lands with wieldable-instance work — `research/weapon-model.md`).
- Slide / wall-run / vault states and their layers (each lands with its movement spec, extending this shape).
- AI behavior graph and hit zones (M10 plans; the example file shows the direction only).
- Expression-field expansion beyond dash's current fields.
- Dual-shape support or deprecation path — old shape rejects with a clear validation error (pre-stable).

## Acceptance criteria

- [ ] A descriptor migrated per the mapping table produces tick-identical movement to its old-shape equivalent (same positions over a scripted input sequence).
- [ ] Ground and air dash tune independently: differing `momentumRetention` under `states.ground.dash` vs `states.air.dash` yields differing velocity carry depending on the state the dash was entered from.
- [ ] Dash availability is per-state: omitting `dash` under `ground` disables ground dash while air dash still fires, and vice versa.
- [ ] A state-level dash field absent from the state block resolves from `modifiers.dash`; absent from both is a validation error naming the field.
- [ ] `viewFeel.layers.run` overrides apply only while the run tier is input-active; `layers.crouch` only while crouched; `layers.air: { bob: null }` suppresses bob airborne (reproducing old `groundedOnly: true`).
- [ ] Old-shape descriptors (top-level `dash`, `fall`, `ground.speed.crouch`, `groundedOnly`) fail validation with an error naming the moved key and its new location.
- [ ] `gen-script-types` output reflects the new shape; the drift-detection test passes; `content/dev/scripts/player.ts` loads and plays under the new shape.
- [ ] Expression-valued dash fields validate and evaluate at both the defaults and state-override levels.

## Tasks

### Task 1: Split `data_descriptors.rs`

Behavior-preserving split of the 5,581-line file along its existing seams (movement, weapon, light/emitter, mesh/health, IR validation — final seams chosen from the file's actual structure) into a `data_descriptors/` module directory; `mod.rs` re-exports the current public surface so call sites are untouched. No semantic change; drift test and existing tests stay green.

### Task 2: Descriptor reshape — states and modifiers

Reshape parsing/validation (both JS and Lua paths in the new movement descriptor module) and `PlayerMovementComponent::from_descriptor` to the proposed shape: `states` block, `modifiers.dash` defaults with per-state sparse override resolution (resolved at materialization into per-state effective dash params on the component), per-state dash availability, validation errors that name moved keys. Migrate `content/dev/scripts/player.ts` and regenerate typedefs in the same pass (drift test forces this).

### Task 3: viewFeel layers

Replace flat `viewFeel` with `base` + `layers` in descriptor parsing and the render-side view-feel evaluator: layer activation from the followed pawn's input tier and movement state, fixed precedence merge, per-channel `null` disable, `groundedOnly` removed. The evaluator already reads pawn velocity and grounded flag; it additionally needs the active state and input tier — exposed the same read-only render-side way, never widening script access to the movement component.

### Task 4: Docs sweep

Update `docs/scripting-reference.md` movement/viewFeel sections and the migration notes; confirm typedef doc comments read correctly for the new nesting.

## Sequencing

**Phase 1 (sequential):** Task 1 — the split lands before anything extends the file.
**Phase 2 (sequential):** Task 2 — Task 3 consumes its state vocabulary and descriptor plumbing.
**Phase 3 (sequential):** Task 3.
**Phase 4 (sequential):** Task 4 — documents the settled surface.

## Boundary inventory

All script-surface keys are camelCase on both runtimes per the existing convention; Rust internals stay snake_case. New keys introduced by this plan:

| Name | Rust (internal) | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|
| states block | `states` field on descriptor struct | `states` | `states` | n/a |
| dash defaults | `modifiers.dash` | `modifiers.dash` | `modifiers.dash` | n/a |
| air dash charges | `charges` | `charges` | `charges` | n/a |
| dash drag | `drag` | `drag` | `drag` | n/a |
| air jumps | `jump.air_count` | `jump.airCount` | `jump.airCount` | n/a |
| viewFeel layers | `view_feel.layers` | `viewFeel.layers` | `viewFeel.layers` | n/a |

Movement is descriptor-owned, never FGD (`movement.md` §7) — no FGD column entries.

## Open questions

- **Layer-transition smoothing.** Switching layers mid-motion (crouch enter, weapon swap later) steps parameters discretely; the oscillator phase carries but amplitude/frequency pop. Engine-pinned crossfade on resolved parameters (one duration, not authored per layer) is the lean candidate — decide during Task 3.
- **States vs modifiers boundary.** Working rule: a state owns the tick's velocity intent; a modifier is a time-bounded overlay entered from a state. Slide is a state; dash is a modifier. Future cases (a charged jump?) get judged against this rule plus the flexibility band (`movement.md` §3).
- **Ground dash charges.** `charges` validates on `states.air.dash` only for now (landing-refresh owns the budget). Allowing it on other states needs a refresh rule — defer until a use case lands.
- **Validation strictness on unknown layer keys.** Reject (closed vocabulary, catches typos) — but confirm this matches existing descriptor unknown-key policy during Task 2.
