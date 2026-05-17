# SDK Animation Capability Interfaces

> **Status:** ready
> **Related:** `context/lib/scripting.md` §7 (SDK library globals) · `sdk/lib/entities/lights.ts` · `sdk/lib/entities/fog_volumes.ts` · `crates/postretro/src/scripting/typedef.rs` (`TS_SDK_LIB_BLOCK`)

---

## Goal

Replace the SDK's free-function animation constructors (`flicker`, `pulse`, `fogPulse`, `fogFade`, `colorShift`, `sweep`) with **methods on entity handles**, declared via reusable **capability interfaces** parametrized by channel name. The handle is the natural subject of an animation call; lifting it out of the argument list (no more `fogPulse(fog.id, ...)`) collapses redundancy and decouples curve construction from any specific entity type.

Authors get one mental model — "I have a fog, animate it" — and the SDK gets a shared algorithm surface that any future animatable entity inherits without redeclaration.

Engine primitives stay per-entity. This is an SDK-layer refactor only; the engine reaction dispatcher and primitive registry are untouched.

---

## Background

Today the SDK exposes animation constructors as free functions that take an `EntityId` as their first positional argument:

```ts
const steps = fogPulse(fog.id, 0.2, 1.0, 1500);
const steps = pulse(light.id, 0.2, 1.0, 1500); // (hypothetical — see note)
```

This couples curve-construction vocabulary to entity types: every new animatable entity gets its own `<entity>Pulse`, `<entity>Fade`, etc., reimplementing the same curve generation logic against a different step descriptor. Authors hold a handle, then dig out its `id` to pass to a function — when the function could just be a method on the handle.

A second problem sits adjacent: `LightEntityHandle.setAnimation(anim)` is the only handle method that *mutates* live state. Every other animation flows through reaction descriptors. Keeping `setAnimation` on the handle leaves the SDK with a split discipline — "handles mostly construct descriptors, but lights also mutate." This plan resolves both at once.

Naming note: `pulse` and `flicker` currently exist for lights but are *free functions that take no entity id* — they return a curve to pass to `setAnimation`. They're already once-removed from the entity. The fog constructors `fogPulse` / `fogFade` were added later under a different pattern (id-passing). The refactor unifies both patterns under "handle methods."

---

## Settled decisions

- **Move fast, break APIs.** Pre-release; no compat shims, no deprecation period. Existing content scripts migrate in the same pass. (See memory: `feedback_api_stability`.)
- **Capabilities live at the SDK layer only.** Engine primitives (`setLightAnimation`, `setFogAnimation`, etc.) stay per-entity by name. The capability abstraction is for author vocabulary, not for engine dispatch.
- **`setAnimation` is dropped from handles.** Handles construct descriptors; they do not mutate. Replaced by handle methods that return `SequenceStep[]`.
- **No sub-package imports (`postretro/animation`).** Discussed and deferred. Flat namespace stays until usage proves the surface is crowded. Splitting later is cheap; un-splitting is expensive.
- **Channel names are type-level only.** The `Channel` type parameter documents which channel the interface targets at the definition site — it does not affect runtime dispatch or method resolution in TypeScript. The handle method's body knows which primitive descriptor to emit; the channel literal is not threaded into a generic primitive.
- **`LightEntity` renamed `LightEntityHandle` in `lights.ts`.** Nothing else extends it; the typedef.rs two-level structure collapses. `LightEntityHandle` extends `GeneratedLightEntity` directly.
- **Vec3 channel methods match today's free-function names.** `colorShift` and `sweep` on `LightEntityHandle`. `AnimatableVec3<Channel>` names the generic algorithm; the handle declares the method per channel.
- **`FogVolumeHandle` includes `flicker`.** Same 8-sample curve as lights. No interface split.

---

## Capability interface design

Two interfaces cover the current animation surface:

```ts
/** Capability for entities with a scalar animation channel. */
export interface AnimatableScalar<Channel extends string> {
  /** Sine pulse oscillating between `min` and `max` over `periodMs`. Loops forever. */
  pulse(opts: { min: number; max: number; periodMs: number }): SequenceStep[];

  /** One-shot linear ramp from `from` to `to` over `periodMs`. Plays exactly once. */
  fade(opts: { from: number; to: number; periodMs: number }): SequenceStep[];

  /** Irregular flicker between `min` and `max` at `rate` Hz. Loops forever. */
  flicker(opts: { min: number; max: number; rate: number }): SequenceStep[];
}

/** Capability for entities with a vec3 animation channel. */
export interface AnimatableVec3<Channel extends string> {
  /** Uniform cycle through the given vectors over `periodMs`. */
  cycle(opts: { values: Vec3[]; periodMs: number }): SequenceStep[];
}
```

Handle types compose them by channel:

```ts
// Proposed design
export interface LightEntityHandle
  extends GeneratedLightEntity,
    AnimatableScalar<"brightness"> {
  colorShift(opts: { values: Vec3[]; periodMs: number }): SequenceStep[];
  sweep(opts: { values: Vec3[]; periodMs: number }): SequenceStep[];
}

export interface FogVolumeHandle
  extends FogVolumeEntity,
    AnimatableScalar<"density"> {
  pulseSaturation(opts: { min: number; max: number; periodMs: number }): SequenceStep[];
  fadeSaturation(opts: { from: number; to: number; periodMs: number }): SequenceStep[];
}
```

Scalar channels compose cleanly: `AnimatableScalar<"brightness">` gives `LightEntityHandle` `pulse`, `fade`, and `flicker`, with the channel name visible at the interface definition site. Vec3 channels don't compose via multiple `AnimatableVec3` extensions — TypeScript collapses duplicate method names. `LightEntityHandle` declares `colorShift` and `sweep` directly, matching today's free-function vocabulary. `AnimatableVec3<Channel>` names the generic algorithm; the handle names the method per channel.

Secondary scalar channels (`FogVolumeHandle.saturation`) follow the same pattern: declared directly on the handle with suffixed names, not via a second `AnimatableScalar` extension.

---

## Method-to-descriptor mapping

| Handle | Method | Emits |
|--------|--------|-------|
| `LightEntityHandle` | `pulse`, `fade`, `flicker` | `setLightAnimation` step (brightness channel of `LightAnimation`) |
| `LightEntityHandle` | `colorShift` | `setLightAnimation` step (color channel) |
| `LightEntityHandle` | `sweep` | `setLightAnimation` step (direction channel) |
| `FogVolumeHandle` | `pulse`, `fade`, `flicker` | `setFogAnimation` step (density channel of `FogAnimation`) |
| `FogVolumeHandle` | `pulseSaturation`, `fadeSaturation` | `setFogAnimation` step (saturation channel) |

The engine sees no change. The step descriptors emitted are exactly what today's free functions return. The handle method is sugar over the existing step shape.

---

## Sub-plans

### 1. Define capability interfaces

**Scope.** Add `AnimatableScalar<Channel>` and `AnimatableVec3<Channel>` to the SDK type surface. Update `LightEntityHandle` and `FogVolumeHandle` to compose them.

**Files**
- `sdk/lib/entities/lights.ts` — rename `LightEntity` → `LightEntityHandle`, change it to extend `GeneratedLightEntity` directly, drop free `flicker`/`pulse`/`colorShift`/`sweep` exports, remove `setAnimation`
- `sdk/lib/entities/fog_volumes.ts` — change local `FogVolumeHandle` interface to `extends FogVolumeEntity`, drop free `fogPulse`/`fogFade` exports
- `sdk/lib/index.ts` — remove the dropped re-exports
- `crates/postretro/src/scripting/typedef.rs` — update `TS_SDK_LIB_BLOCK` and `LUAU_SDK_LIB_BLOCK` (both in this file) to declare the interfaces and remove the free-function declarations
- `sdk/types/postretro.d.ts` + `.d.luau` — regenerated via `gen-script-types`

**Acceptance criteria**
- [ ] `AnimatableScalar` and `AnimatableVec3` interfaces exist in the SDK type surface (both `.d.ts` and `.d.luau`).
- [ ] `LightEntityHandle` and `FogVolumeHandle` declare capability inheritance in both type definition files.
- [ ] No free-function animation curve constructors (`flicker`, `pulse`, `colorShift`, `sweep`, `fogPulse`, `fogFade`) remain exported from the SDK prelude. Keyframe utilities (`timeline`, `sequence`) are unaffected.
- [ ] `cargo run -p postretro --bin gen-script-types` produces output matching the committed `.d.ts` / `.d.luau`.
- [ ] `setAnimation` is no longer present on `LightEntityHandle` in any type definition file or handle implementation.

---

### 2. Implement handle methods (TypeScript + Luau)

**Scope.** Move the curve-construction bodies from the dropped free functions into handle wrapper methods. `wrapLightEntity` and `wrapFogVolumeEntity` install the methods on the returned object. Note: the existing free `pulse` and `flicker` for lights return a bare `LightAnimation`, not a `SequenceStep[]`. The new handle methods wrap the curve in a `setLightAnimation` step — this is a return-type change, not just a signature change.

**Files**
- `sdk/lib/entities/lights.ts` — `wrapLightEntity` returns an object with `pulse`, `fade`, `flicker`, `colorShift`, `sweep`
- `sdk/lib/entities/lights.luau` — mirror of above
- `sdk/lib/entities/fog_volumes.ts` — `wrapFogVolumeEntity` returns an object with `pulse`, `fade`, `flicker`, `pulseSaturation`, `fadeSaturation`
- `sdk/lib/entities/fog_volumes.luau` — mirror of above

Curve algorithms are unchanged — 16-sample sine for `pulse` on lights; fog `pulse` (migrated from `fogPulse`) emits 17 samples (16 + 1 wrap) to avoid period-boundary pop. 16-sample linear ramp for `fade`. 8-sample irregular sequence for `flicker`. Only the function signature and the `id` capture change. In Luau, capability composition is table-merging — methods are installed directly on the returned table, not via interface extension. The `.d.luau` annotations express the capability structure for tooling only.

**Acceptance criteria**
- [ ] Each capability method produces the same `SequenceStep[]` output (modulo `id` capture) as the free function it replaces. Verified by snapshot test against a fixed handle id.
- [ ] Both TypeScript and Luau implementations pass parity tests.
- [ ] `cargo test -p postretro` passes (drift detection + any new snapshot tests).

---

### 3. Migrate content scripts

**Scope.** Update every `.ts` and `.luau` script under `content/` that imports a dropped free function. Rewrite call sites to use handle methods.

| Script | Change |
|--------|--------|
| `content/dev/scripts/fog-pulse-demo.ts` | `fogPulse(fog.id, 0.2, 1.0, 1500)` → `fog.pulse({ min: 0.2, max: 1.0, periodMs: 1500 })` |
| `content/dev/scripts/arena-lights.ts` (and siblings) | `pulse(...)` / `flicker(...)` → `light.pulse(...)` / `light.flicker(...)` |
| (Other content scripts as discovered) | Same pattern |

A pre-step is `grep -rE 'fogPulse|fogFade|\bflicker\b|\bpulse\b|colorShift|sweep' content/` to enumerate every affected file.

**Acceptance criteria**
- [ ] No content script imports the dropped free functions.
- [ ] Every affected script compiles via `scripts-build` without TypeScript errors.
- [ ] Demo levels using fog and light animation visually match their pre-refactor behavior (manual smoke test in-engine).

---

### 4. Update scripting documentation

**Scope.** Document the capability-interface pattern in `context/lib/scripting.md` as a durable design principle. Note the rule for future entity types: scalar/vec3 animatable channels compose the existing capability interfaces rather than introducing new constructors. Also update the module layout list in §7 to remove references to dropped free functions (`flicker`, `pulse`, `colorShift`, `sweep`, `fogPulse`, `fogFade`) and describe handle methods instead.

**Files**
- `context/lib/scripting.md` — new subsection under §7 (SDK library globals), titled "Animation capabilities," describing the interfaces and the rule

**Acceptance criteria**
- [ ] `context/lib/scripting.md` describes `AnimatableScalar` / `AnimatableVec3` and the rule that handle methods (not free functions) are the canonical way to construct animation step descriptors.
- [ ] The doc names no specific method internals — just the principle and the pointer to `sdk/lib/entities/*.ts` as the source of truth.

---

## Non-goals

- **Sub-package imports** (`postretro/animation`). Deferred until namespace pressure justifies it.
- **Generic engine primitives.** No `animateScalar` super-primitive; engine dispatch stays per-entity by name.
- **Removing `setLightAnimation` / `setFogAnimation` step descriptors.** Those are the underlying primitives; handle methods are sugar over them.
- **Reusing capability interfaces in Rust.** This is an SDK / script-surface refactor. The Rust side does not gain trait analogs.
- **Animation for emitters.** Emitters animate via `setEmitterRate` and (potentially) future per-channel primitives. Whether they compose `AnimatableScalar<"rate">` is a follow-up decision; not in scope here.
- **Channel-parameter validation.** The `Channel` type parameter is type-level documentation. No runtime check that a method emits a step matching its declared channel — the implementation closure is the source of truth.
- **Multi-channel light animation via handle methods.** Each handle method emits a single-channel `setLightAnimation` step; `setLightAnimation` is last-write-wins per reaction dispatch. Authors needing simultaneous brightness + color + direction compose a `LightAnimation` directly and schedule a `setLightAnimation` reaction step.

---

## Open questions

- **Where do `timeline` and `sequence` live?** They're not channel-coupled — they're keyframe utilities. Probably stay as free functions on the prelude. Confirm during implementation.
