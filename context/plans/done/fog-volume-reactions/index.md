# Fog Volume Reactions

## Goal

Expose runtime control of fog volume parameters (`density`, `scatter`, `edge_softness`, `falloff`) for all fog entity types (`fog_volume` — plane-bounded and axis-aligned ellipsoid forms, `fog_lamp`, `fog_tube`) via named reaction primitives and the reactions API. Replace the existing `setComponent`-based fog mutation path so the scripting VM is not live at runtime. All behavior executes in Rust; scripts declare intent at load time.

This is the companion to `fog-ellipsoid-entity`. That spec lands the new shape and gets fog ellipsoids compiling and rendering at level load. This spec lands the runtime-control surface that was originally the dropped Task 5 of that spec, redesigned around the same reactions pattern that drives light animations and emitter rate/spin (`setLightAnimation`, `setEmitterRate`, `setSpinRate`).

The surface has two parts: (1) tag-targeted scalar reactions (`setFogDensity`, `setFogScatter`, `setFogEdgeSoftness`, `setFogFalloff`, `setFogParams`) for one-shot story-event changes; (2) a `FogAnimation` channel on `FogVolumeComponent` plus a `setFogAnimation` primitive for time-varying density and/or saturation curves, sampled by a Rust-side per-frame evaluator (analogous to `LightAnimation` / `setLightAnimation`, but evaluated on CPU and written back to the component fields rather than on GPU). Scatter, edge_softness, and falloff stay set-once via their respective scalar primitives.

## Scope

### In scope

- New named reaction primitives `setFogDensity`, `setFogScatter`, `setFogEdgeSoftness`, `setFogFalloff`. Each is tag-targeted and applies to every fog entity matching the reaction's tag, regardless of subtype (`fog_volume` plane-bounded or axis-aligned, `fog_lamp`, `fog_tube`).
- A combined `setFogParams` primitive accepting any subset of the four fields in one args object. Mirrors the partial-update ergonomic that the dropped `setComponent` plan offered, expressed as a single reaction call instead of a live mutation.
- A new `falloff` field on `FogVolumeComponent` (the only mutable field added by this spec), surfaced through wire load and the reaction primitives.
- Removal of `FogVolumeComponent` from any future `setComponent` dispatch surface — there is no live-VM path for fog mutation. The `scripting/primitives/mod.rs` "forbidden primitives" test already guards against `setComponent` regressing into the registry; this spec's contribution is to ensure the reaction-primitive path is the only documented mutation surface for fog.
- SDK fog vocabulary file additions: extend `sdk/lib/entities/fog_volumes.{ts,luau}` with a read-only `FogVolumeHandle` wrapper and pure animation constructors (`fogPulse`, `fogFade`) that build step-list descriptors for `registerReaction` — matching the sequence step shape from `arena-lights.ts`. The constructors return `SetFogAnimationStep[]` (one step carrying a curve), not per-step density values: the sequence dispatcher fires every step on the same frame, so a 16-step `setFogDensity` array collapses to the last value with no time-varying playback. `fogPulse` / `fogFade` are curve-definition helpers for `setFogAnimation`.
- A new optional `animation: FogAnimation` field on `FogVolumeComponent`, analogous to `LightComponent.animation`. Carries per-frame density and/or saturation curves sampled by a Rust-side evaluator. Either channel may be `None`; both being `None` with a finite `play_count` is rejected. `scatter`, `edge_softness`, and `falloff` remain set-once via the existing primitives. The evaluator writes sampled values into `FogVolumeComponent` each frame before the fog bridge's `update_volumes` runs, so the existing GPU path is unchanged.
- New `setFogAnimation` reaction primitive — tag-targeted, same dispatch semantics as the four scalar fog primitives. Args shape mirrors `FogAnimation`. Registers (or clears, when `null`) a `FogAnimation` on every matching `FogVolumeComponent`. The named-event use case (one-shot density change at a story beat) stays on `setFogDensity` / `setFogParams`; `setFogAnimation` is the time-varying channel.

### Out of scope

- Changing the fog volume's geometry (AABB, planes, half-extent) at runtime. Only the four scalar parameters are mutable.
- Animated `scatter`, `edge_softness`, or `falloff`. The `FogAnimation` channel drives `density` and `saturation`; the other three fields stay set-once via their respective reaction primitives.
- GPU-side phase evaluation for `FogAnimation`. Lights walk their curve in WGSL because the GPU evaluator was already on the critical path; fog curves are sampled on the CPU each frame inside the bridge, before `update_volumes` packs the GPU buffer. Per-frame evaluator cost is one sample per channel per fog volume per frame — negligible at expected volume counts (well under 100). If a future scene drives fog volumes into the thousands, revisit; the design intentionally avoids inventing a fog-specific GPU evaluator until that need exists.
- Per-classname enforcement at the script boundary. `falloff` is accepted on every fog entity; the shader path it reaches depends on the volume type. Documented; no error.
- Per-entity fog color (settled elsewhere: ambient comes from the SH irradiance volume).
- Anything in `fog-ellipsoid-entity` (FGD, compiler resolver, shader branch, `shape_mode` discriminant). Those land independently.

## Acceptance criteria

- [ ] A data script can register a sequenced reaction that calls `setFogDensity` against a tag and the visible fog density on every tagged fog entity changes on the frame the reaction fires, with no scripting VM running on the tick path.
- [ ] The same script pattern works for `setFogScatter`, `setFogEdgeSoftness`, `setFogFalloff`, and `setFogParams` (partial object).
- [ ] `setFogParams` accepts any subset of `{density, scatter, edgeSoftness, falloff}`. Absent fields leave the corresponding component value at its prior value (wire-loaded at level start; the most recent reaction-applied value thereafter). Negative or non-finite numeric inputs are handled per the per-field rules in the validation table; `log::warn!` records each violation. For `setFogParams`: each field is validated independently; invalid fields are skipped, valid fields applied, component written once per target.
- [ ] `setFogFalloff` is accepted on every fog entity type. For plane-bounded `fog_volume` (plane-sweep) it updates the stored value but is not consulted by the shader. For `fog_lamp` and `fog_tube` it changes the radial exponent. For axis-aligned `fog_volume` it drives the ellipsoid path. No error is raised on any subtype.
- [ ] Tag-targeting matches `setEmitterRate`'s semantics: the reaction's `tag` filter resolves to a target list at dispatch time; entities lacking a `FogVolumeComponent` are skipped with `log::warn!` (typo guard); empty target sets are a debug-log no-op. Exception: `setFogFalloff` early-returns after a single invalid-arg warn when `falloff` is non-positive or non-finite, so missing-component (typo) warns only fire when the falloff value itself is valid. One invalid-arg warn is enough signal in that case.
- [ ] The primitive registry's "forbidden primitives" test (`scripting/primitives/mod.rs`) continues to assert `setComponent` is absent. No new dispatch arm is added that would re-introduce a live mutation path for fog.
- [ ] SDK type generation (`cargo run -p postretro --bin gen-script-types`) emits typed argument shapes for the new primitives. The drift-detection test in `cargo test` passes after regeneration.
- [ ] The fog volume bridge cache (`FogVolumeAabb`) carries no `Option<f32>` override fields and is not extended with per-component override slots — the bridge reads `FogVolumeComponent` directly the same way it does today, with `falloff` joining the existing `density`/`scatter`/`edge_softness` set.
- [ ] `fogPulse(id, min, max, periodMs)` and `fogFade(id, from, to, periodMs)` exist in both `fog_volumes.ts` and `fog_volumes.luau`, return a single-element `SetFogAnimationStep[]` whose `args` carry a 16-sample curve plus the supplied `periodMs`, and are exercised by unit tests. (Timing now matters because the curve is played back over wall-clock time by the per-frame evaluator, not the sequence dispatcher.)
- [ ] `setFogAnimation` is registered as a tag-targeted sequenced reaction primitive. Dispatching it against a tag installs the supplied `FogAnimation` (or clears it when `args` is `null`) on every matching `FogVolumeComponent`. Targets lacking a `FogVolumeComponent` are skipped with `log::warn!`, identical to the four scalar fog primitives.
- [ ] The `FogAnimation` evaluator runs each frame in `fog_volume_bridge::tick` — a new method invoked as a separate call from `main.rs` immediately before `update_volumes`, in the same per-frame block (not bundled inside `update_volumes`, so the existing `update_volumes_packs_*` tests stay isolated). For every entity with `FogVolumeComponent.animation = Some(_)`, the evaluator samples the curve at the current period-relative phase and writes the result into `FogVolumeComponent.density`. `play_count = Some(n)` settles after `n` periods — the bridge writes the final-sample density back as the static value and clears `animation`, mirroring the light bridge's `play_count`-bounded settle path. `play_count = None` loops forever.
- [ ] An author-visible script using the new vocabulary (one fog-driven scene in `content/tests/scripts/`) demonstrates a `fogPulse` against a tag, registered via `setFogAnimation`, and runs cleanly in the QuickJS runtime — visible density modulation over time confirms the per-frame evaluator wired through correctly. (No `.luau` mirror of the demo script is required — `content/tests/scripts/` has no `.luau` analogs to the `.ts` scripts; the prelude integration is verified by the Luau round-trip test in Task 4.)

## Tasks

### Task 0: Add `animation` field and `FogAnimation` type to `FogVolumeComponent`

This task lands the data shape the evaluator and the new primitive both depend on. It is a prerequisite for Task 2 (the new `setFogAnimation` primitive) and Task 5 (the per-frame evaluator).

- New module `crates/postretro/src/scripting/components/fog_volume.rs` (or extend the existing fog component module if one exists alongside `components/light.rs`). Define `FogAnimation` mirroring `LightAnimation`'s shape, with dual animated channels (`density` and `saturation`):

  ```
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase")]
  pub(crate) struct FogAnimation {
      pub(crate) period_ms: f32,
      #[serde(default)]
      pub(crate) phase: Option<f32>,            // None == 0.0, rem_euclid'd into [0, 1)
      #[serde(default)]
      pub(crate) play_count: Option<u32>,       // None == loop forever
      #[serde(default)]
      pub(crate) density: Option<Vec<f32>>,     // None == hold static density
      #[serde(default)]
      pub(crate) saturation: Option<Vec<f32>>,  // None == hold static saturation
  }
  ```

  **Why no `start_active`.** `LightAnimation` carries `start_active` because the GPU evaluator needs a flag to mark a descriptor inactive without unbinding it. Fog has no GPU descriptor for the curve — the bridge writes density directly into `FogVolumeComponent.density` each frame. There is no activation event in the surface (`setFogAnimation null` clears the channel; reinstalling reactivates it), so `start_active` would be a no-op field. Drop it to keep the API minimal; if a future "pause" event lands, add it then with a defined semantic.

- Extend `FogVolumeComponent` in `registry.rs` with `#[serde(default)] pub(crate) animation: Option<FogAnimation>`. `FogVolumeComponent` loses `Copy` (the `Vec<f32>` curve is heap-backed). The bridge cache (`FogVolumeAabb`) still copies its scalar mirror; only the component itself loses `Copy`.

  **`Copy`-loss blast radius (enumerated).** Dropping `Copy` from `FogVolumeComponent` forces a knock-on change on `ComponentValue` and every site that bitwise-copies it:

  - `ComponentValue` in `registry.rs:112` derives `Copy` and includes `FogVolume(FogVolumeComponent)`. That derive must drop `Copy` too — leave `Clone`/`PartialEq`/`Debug`/`Serialize`/`Deserialize` in place.
  - The five already-shipped reaction files all use `*c` copy semantics at their dispatch sites; switch each one from `*c` to `c.clone()`:
    - `crates/postretro/src/scripting/reactions/set_fog_density.rs` (line 47)
    - `crates/postretro/src/scripting/reactions/set_fog_scatter.rs` (line 55)
    - `crates/postretro/src/scripting/reactions/set_fog_edge_softness.rs` (line 47)
    - `crates/postretro/src/scripting/reactions/set_fog_falloff.rs` (line 50)
    - `crates/postretro/src/scripting/reactions/set_fog_params.rs` (line 145)
  - Audit every `match` and pattern-bind on `ComponentValue` engine-wide (`primitives/mod.rs:196`, `conv.rs` arms, etc.). Sites that previously moved a `ComponentValue` by copy now move by value or borrow; sites that need an owned copy switch to `.clone()`. Where possible, prefer reference borrows (`ComponentValue::FogVolume(ref f)`) over a clone.
- `conv.rs` `from_js` / `from_lua` for `ComponentValue::FogVolume` parse the new optional `animation` field via `serde_json` (same path `LightAnimation` takes — see lines ~739–768). `into_js` / `into_lua` emit it.
- `typedef.rs` adds a `FogAnimation` type definition next to `LightAnimation` (TS at ~L490, Luau at ~L850), and the `FogVolumeComponent` declaration gains `animation: FogAnimation | null`.

### Task 1: Add `falloff` to `FogVolumeComponent`

In `crates/postretro/src/scripting/registry.rs`, extend `FogVolumeComponent` with a fourth field, `falloff: f32`. Wire it through:

- `populate_from_level` in `fog_volume_bridge.rs` reads `entry.radial_falloff` and stores it as `FogVolumeComponent.falloff` at level load. The `FogVolumeAabb` cache no longer needs `radial_falloff` — drop that field from the cache. (Consumers in `update_volumes` switch to reading `falloff` from the component.)
- `update_volumes` passes `component.falloff` into `FogVolume.radial_falloff` at GPU pack time. The wire field `MapFogVolume.radial_falloff` and the wire/GPU/WGSL field name `radial_falloff` are unchanged — same asymmetry as documented in `fog-ellipsoid-entity` (script-facing name `falloff`, wire/Rust-internal name `radial_falloff`).
- `conv.rs` `FromJs` and `FromLua` for `ComponentValue::FogVolume` parse the new `falloff` field. Since this spec drops the live `setComponent` path, conv.rs's role for fog narrows to the world-query handle's read-only snapshot — `into_js`/`into_lua` emit `falloff`; `from_js`/`from_lua` parse it.
- `typedef.rs` hand-written `FogVolumeComponent` adds `falloff: number` and renames `edge_softness` → `edgeSoftness` to match the camelCase boundary inventory. Update `conv.rs` `into_js`/`into_lua` and the round-trip tests to emit/assert `edgeSoftness`. The handle type `FogVolumeEntity` already wraps the component snapshot, so the new field surfaces through `world.query` automatically.
- `sdk/types/postretro.d.{ts,luau}` regenerated.
- `docs/scripting-reference.md` `FogVolumeComponent` row gains `falloff`.

### Task 2: Add fog reaction primitives

In `crates/postretro/src/scripting/reactions/`, mirror the layout of `set_emitter_rate.rs` and `set_spin_rate.rs`:

- New file `set_fog_density.rs` with `SetFogDensityArgs { density: f32 }` and a `dispatch` that applies the new value to every target's `FogVolumeComponent.density`.
- New file `set_fog_scatter.rs` with `SetFogScatterArgs { scatter: f32 }`.
- New file `set_fog_edge_softness.rs` with `SetFogEdgeSoftnessArgs { edge_softness: f32 }` (script-facing camelCase: `edgeSoftness`).
- New file `set_fog_falloff.rs` with `SetFogFalloffArgs { falloff: f32 }`.
- New file `set_fog_params.rs` with `SetFogParamsArgs { density: Option<f32>, scatter: Option<f32>, edge_softness: Option<f32>, falloff: Option<f32> }`. Absent fields preserve the target's current component value. The dispatch reads-modify-writes the component once per target.
- New file `set_fog_animation.rs` with `SetFogAnimationArgs(Option<FogAnimation>)` (newtype around the optional payload, so `null` and `{...}` both deserialize cleanly — same shape as `setLightAnimation`'s arg). Dispatch installs the animation onto every target's `FogVolumeComponent.animation` (or clears it when `None`). Validation: `period_ms > 0` and finite; each `density[i]` is `>= 0` and finite (clamp to `0.0` with `log::warn!` once on violation); `phase` (when `Some`) is `rem_euclid(1.0)`'d into `[0, 1)` (not `.fract()` — `fract` returns a negative value for negative inputs, while `rem_euclid` always lands in `[0, 1)`); `play_count == Some(0)` is bumped to `Some(1)` with `log::warn!` — `Some(0)` has no defensible meaning under fog's CPU evaluator (the curve has nothing to settle to), so we coerce it to a one-shot rather than silently looping. (This deliberately differs from `light_bridge`'s `play_count == 0` → "never completes" behavior, which is a GPU-evaluator-specific accommodation.) An empty `density` curve is a hard reject — `log::warn!`, no install. A length-1 curve is accepted as a constant (the evaluator skips interpolation). `phase` is `rem_euclid(1.0)`'d into `[0, 1)` at install time, so the per-frame evaluator can assume a normalized phase. A second `setFogAnimation` call against the same target resets `animation_start_time` when the new payload differs from the cached snapshot (value comparison in the bridge's `tick`); a bitwise-identical reinstall does not restart the clock. Detecting the change in the bridge avoids plumbing reset-on-install through the reaction dispatch (which would couple the reaction layer to the bridge's side-table) or adding a transient flag on the component. The test `setFogAnimation_resets_start_time` (Task 5) asserts the value-change case. Per-target behavior matches the four scalar primitives (missing component → warn-skip; empty target set → debug no-op).

Per-target behavior matches `set_emitter_rate.rs`:

- Missing `FogVolumeComponent` → `log::warn!`, skip (tag matched a non-fog entity — most likely a tag typo).
- Out-of-range numeric input → per-field clamp + `log::warn!` once per field. See validation table below.
- Empty target set → no-op, debug log.

For `set_fog_params.rs`: each field is validated independently. A field that fails validation is dropped with `log::warn!`; valid fields are still written. The component is mutated once per target with the merged result. If all fields fail validation for a target, the component is not written for that target — no dirty flag is set.

Register the six primitives in `reactions/registry.rs` alongside the existing `register_emitter_reaction_primitives` via a new `register_fog_reaction_primitives(&mut ReactionPrimitiveRegistry)`. Wire it into the same call site that wires the emitter primitives (see Rough sketch for the call site file and location).

Each `Set*Args` struct derives `Deserialize` with `#[serde(rename_all = "camelCase")]`, matching `SetEmitterRateArgs`. All six carry the attribute uniformly for consistency, even where the rename is a no-op for a given field.

After registering the six primitives, run `cargo run -p postretro --bin gen-script-types` and commit the updated `sdk/types/postretro.d.{ts,luau}`.

### Task 3: SDK fog vocabulary

Extend `sdk/lib/entities/fog_volumes.{ts,luau}` to mirror the `lights.{ts,luau}` shape:

- Replace the existing pass-through `FogVolumeHandle` type alias with a proper wrapper that exposes typed read access to the fog component fields. No mutation methods — authors build sequenced reactions via `registerReaction` and the fog primitives directly.
- Pure animation constructors. Both return a single-element `SetFogAnimationStep[]` — the curve lives inside one step's `args`, not spread across N density steps. The previous "16 `setFogDensity` steps" shape was wrong: the sequence dispatcher fires every step on the same frame, so the density just landed on the last sample. `setFogAnimation` plus the per-frame evaluator is the channel that produces actual time-varying playback.
  - `fogPulse(id, min, max, periodMs)` — builds a 16-sample sine curve (`mid + amp * sin(2π·i/N)`) sampled between `min` and `max`, packs it as `FogAnimation.density`, and emits one `{ id, primitive: "setFogAnimation", args: { periodMs, density: [...], phase: null, playCount: null } }` step. Step count, sampling formula, and `id` shape mirror the `pulse` constructor in `sdk/lib/entities/lights.ts`. (`startActive` is intentionally absent — see Task 0.)
  - `fogFade(id, from, to, periodMs)` — builds a 16-sample linearly-interpolated curve from `from` to `to` (first sample == `from`, last sample == `to`), packs it as `FogAnimation.density`, and emits one `{ id, primitive: "setFogAnimation", args: { periodMs, density: [...], phase: null, playCount: 1 } }` step. `playCount: 1` because a fade is a one-shot — looping a fade replays the snap from `to` back to `from` every period, which is rarely what authors want. (Authors who want a continuously-fading effect can pass their own `LightAnimation`-style curve directly.)

Both return single-element `SetFogAnimationStep[]` arrays matching the sequence step shape in `arena-lights.ts`. Authors wrap in `registerReaction`. Doc comments inside both functions call out the change from the prior "fan out into 16 `setFogDensity` steps" shape so readers grep'ing for the old pattern find the explanation.

Wire the new file into both preludes — `sdk/lib/prelude.js` regenerated via the prelude-bundler (see `context/lib/scripting.md` §8) for TypeScript. For Luau: `fog_volumes.luau` is already in the prelude eval order; no new eval-order entry in `luau.rs` is needed. Any new bare globals exported from the updated file (e.g., `fogPulse`, `fogFade`) must be added to the `FOG_VOLUMES_LUAU_FIELDS` slice in `luau.rs` (line 72), which drives the publics-lifting loop at lines 148–160. Currently defined as `&[]`; extend it to match the new exports, e.g. `const FOG_VOLUMES_LUAU_FIELDS: &[&str] = &["fogPulse", "fogFade"];`. The loop reads each name from the table returned by `fog_volumes.luau` and installs it as a bare global — the same pattern `LIGHTS_LUAU_FIELDS` uses for `flicker`, `pulse`, `colorShift`, `sweep`.

After editing `fog_volumes.ts`, regenerate `sdk/lib/prelude.js` via `cargo run -p postretro-script-compiler -- --prelude --sdk-root sdk/lib --out sdk/lib/prelude.js` and commit the result.

### Task 4: Reference test scene and round-trip tests

- One new test script in `content/tests/scripts/` (e.g. `fog-pulse-demo.ts`) that calls `world.query({ component: "fog_volume", tag: ... })`, builds a `fogPulse` descriptor, and registers it via `registerReaction("levelLoad", { sequence: [...] })`. Mirrors `arena-lights.ts`'s shape exactly.
- Round-trip tests in `reactions/registry.rs` (`registers_all_fog_primitives_under_expected_names`) and per-primitive dispatch tests in each new file (analog to the `setEmitterRate` tests that assert clamp behavior, missing-component skip, empty-target no-op).
- Cross-runtime parity test: same `setFogParams` JSON arg dispatched through the sequenced primitive registry from a QuickJS-shaped JSON value and a Luau-shaped JSON value produces identical post-mutation `FogVolumeComponent` state. See `set_light_animation_quickjs_and_luau_produce_identical_output` (in `crates/postretro/src/scripting/primitives/light.rs`) for the exact two input shapes this test mirrors.
- Unit tests for `fogPulse` and `fogFade` (TypeScript and Luau) asserting the returned single-step arrays carry a 16-sample curve in `args.density`, the supplied `periodMs`, the expected `playCount` (`null` for `fogPulse`, `1` for `fogFade`), and the `{ id, primitive: "setFogAnimation", args }` shape.

### Task 5: `FogAnimation` per-frame evaluator

Rust-side per-frame system that walks `FogVolumeComponent.animation` and writes the sampled density into `FogVolumeComponent.density` before the existing `update_volumes` runs. Mirrors the `play_count`-bounded settle path in `light_bridge::tick` (lines ~269–340) but writes the result back to the component instead of into a GPU descriptor — the existing `update_volumes` then packs the (now updated) density into the GPU buffer the same way it does for static fog.

- New method `FogVolumeBridge::tick(&mut self, registry: &mut EntityRegistry, time_seconds: f32)` (or `tick_animations`, matching whichever name the light bridge uses). Iterates the registry's `FogVolumeComponent`s; for each one with `animation = Some(_)`:
  - Compute period-relative phase: `t = ((time_seconds * 1000.0 - start_ms) / period_ms + phase).rem_euclid(1.0)`. Use `rem_euclid(1.0)` (not `.fract()`): in Rust, `(-0.25_f32).fract() == -0.25`, which would index the curve negatively if `phase` were ever negative or if a clock skew produced a negative numerator. `rem_euclid(1.0)` always returns a value in `[0, 1)`. The bridge tracks `animation_start_time: Option<f32>` per fog entity in a side table (parallel to `LightSlot.animation_start_time`) so `play_count` settles deterministically and `setFogAnimation` calls reset the start time.
  - Sample the curve at `t` with linear interpolation across uniformly-spaced samples. This deliberately differs from the GPU-side Catmull-Rom path that lights use: fog density is a single scalar per frame, the sampling cadence is once per fog volume per frame on CPU, and the visual difference at curve boundaries is imperceptible. As a consequence, `fogPulse` / `fogFade` produce results that are *similar but not mathematically identical* to `pulse` on lights at keyframe boundaries — the SDK-side curve definitions match, but fog is sampled with linear interpolation on CPU and lights with Catmull-Rom on GPU.
  - Write the sampled value into `component.density`. Subsequent `update_volumes` reads it the same way it reads any other density. No new override slot on the bridge, no `Option<f32>` shadow field — the component is the single source of truth.
- `play_count`-bounded settle: when `Some(n)` is reached, sample the final keyframe, write it as `density`, clear `animation`, and clear the side-table start time. Identical control flow to `light_bridge`'s settle pass.
- Wire `tick` into the engine update sequence in `main.rs` immediately before the existing `update_volumes` invocation, as a separate call (not bundled inside `update_volumes`). Keeping it as its own call preserves the test isolation that the existing `update_volumes_packs_*` tests rely on — those tests construct a registry, call `update_volumes` directly, and assert GPU bytes; bundling the evaluator into `update_volumes` would force every such test to set up time and animation state. The actual call site (`main.rs:924`) sits in the same per-frame block that runs after the light bridge and before `render_frame_indirect`, between the Game-logic and Render phases. The acceptance criterion is positional ("immediately before `update_volumes` in the same frame block"), not phase-named.
- Tests in `fog_volume_bridge.rs`:
  - `evaluator_writes_curve_sample_into_component_density` — install a known curve via `setFogAnimation`, advance time by half a period, assert `component.density == curve.sample(0.5)` within an epsilon.
  - `evaluator_settles_play_count_bounded_animation_and_clears_field` — install an animation with `play_count: Some(1)`, advance past one period, assert `animation` is now `None` and `density` equals the final keyframe.
  - `evaluator_skips_components_without_animation` — sanity check that components with `animation: None` are untouched.
  - `setFogAnimation_resets_start_time` — install one animation, advance time, install a second whose payload differs from the first; assert the second animation's `start_time` is the call moment, not the first install's. Reset is value-change-driven: a bitwise-identical reinstall does not restart the clock.

## Sequencing

**Phase 1 (concurrent):** Task 0 (FogAnimation type + `animation` field on `FogVolumeComponent`) and Task 1 (`falloff` field on `FogVolumeComponent` + bridge rewire). Both extend the component shape; they touch overlapping files (`registry.rs`, `conv.rs`, `typedef.rs`) but on independent fields, so they can be done in parallel and merged in either order. Every later task depends on this phase landing.

**Phase 2 (concurrent):** Task 2 (six reaction primitives, including `setFogAnimation`), Task 3 (SDK vocabulary), Task 5 (per-frame evaluator). Independent surfaces:
- Task 2 lands the Rust dispatch surface.
- Task 3 lands the script-facing builders (depends on `setFogAnimation` being registered before round-trip tests can run, but the constructor source is independent and can be written in parallel).
- Task 5 lands the evaluator that consumes `FogVolumeComponent.animation` each frame.

**Phase 3 (sequential, depends on Phase 2):** Task 4 — reference scene + round-trip + cross-runtime tests. Cannot be written until the primitive surface, the SDK helpers, and the evaluator all exist (the demo script only produces visible motion when Task 5 has wired the evaluator into the per-frame path).

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | Reaction primitive |
|---|---|---|---|---|---|
| Density | `FogVolumeComponent.density: f32`, `FogVolume.density` | `density: f32` (existing) | `FogVolumeComponent.density: number` | `FogVolumeComponent.density: number` | `setFogDensity { density }`; also `setFogParams.density?` |
| Scatter | `FogVolumeComponent.scatter: f32`, `FogVolume.scatter` | `scatter: f32` (existing) | `FogVolumeComponent.scatter: number` | `FogVolumeComponent.scatter: number` | `setFogScatter { scatter }`; also `setFogParams.scatter?` |
| Edge softness | `FogVolumeComponent.edge_softness: f32`, `FogVolume.edge_softness` | `edge_softness: f32` (existing) | `FogVolumeComponent.edgeSoftness: number` | `FogVolumeComponent.edgeSoftness: number` | `setFogEdgeSoftness { edgeSoftness }`; also `setFogParams.edgeSoftness?` |
| Falloff | `FogVolumeComponent.falloff: f32` (new), `FogVolume.radial_falloff` | `radial_falloff: f32` (existing wire field, no rename) | `FogVolumeComponent.falloff: number` | `FogVolumeComponent.falloff: number` | `setFogFalloff { falloff }`; also `setFogParams.falloff?` |
| Animation | `FogVolumeComponent.animation: Option<FogAnimation>` (new) — see `FogAnimation` row below | `animation?: FogAnimation` (default `null`) | `FogVolumeComponent.animation: FogAnimation \| null` | `FogVolumeComponent.animation: FogAnimation?` | `setFogAnimation { animation: FogAnimation \| null }` (newtype-deserialized; `null` clears the channel) |
| FogAnimation.periodMs | `FogAnimation.period_ms: f32` | `periodMs: f32` | `FogAnimation.periodMs: number` | `FogAnimation.periodMs: number` | (carried inside `setFogAnimation` args) |
| FogAnimation.density | `FogAnimation.density: Option<Vec<f32>>` | `density?: Vec<f32>` | `FogAnimation.density: number[] \| null` | `FogAnimation.density: {number}?` | (carried inside `setFogAnimation` args) |
| FogAnimation.phase | `FogAnimation.phase: Option<f32>` | `phase?: f32` | `FogAnimation.phase: number \| null` | `FogAnimation.phase: number?` | (carried inside `setFogAnimation` args) |
| FogAnimation.playCount | `FogAnimation.play_count: Option<u32>` | `playCount?: u32` | `FogAnimation.playCount: number \| null` | `FogAnimation.playCount: number?` | (carried inside `setFogAnimation` args) |

**Asymmetry note (carried from `fog-ellipsoid-entity`).** Script-facing field is `falloff`; wire and Rust-internal field stays `radial_falloff` for the same reason: avoiding a much larger rename across `MapFogVolume`, `FogVolumeRecord`, the GPU struct, the WGSL struct, and the existing PRL wire format. The bridge maps `FogVolumeComponent.falloff` ↔ `FogVolume.radial_falloff` at the existing copy site.

### Validation table

| Field | Valid range | On out-of-range | Default at level load |
|---|---|---|---|
| `density` | `[0.0, +∞)`, finite | clamp to `0.0`; `log::warn!` once | wire-loaded `density` (FGD default `0.5`) |
| `scatter` | `[0.0, 1.0]`, finite | clamp into range; `log::warn!` once | wire-loaded `scatter` (FGD default `0.6`) |
| `edge_softness` | `[0.0, +∞)`, finite | clamp to `0.0`; `log::warn!` once | wire-loaded `edge_softness` (FGD default per entity type) |
| `falloff` | `(0.0, +∞)`, finite | skip field with `log::warn!`; component `falloff` unchanged for this target (applies in both `setFogFalloff` and `setFogParams`) | wire-loaded `radial_falloff` (FGD default per entity type: `fog_lamp` = 2.0, `fog_tube` = 1.5) |
| `FogAnimation.periodMs` | `(0.0, +∞)`, finite | reject the entire animation install with `log::warn!`; pre-existing `animation` is unchanged | n/a (must be supplied) |
| `FogAnimation.density[i]` | `[0.0, +∞)`, finite | clamp the offending sample to `0.0`; `log::warn!` once per install | n/a |
| `FogAnimation.density` (length) | length `>= 2` when `Some`; length `1` is accepted as a constant-density curve (no interpolation, no warn); empty rejects | reject the entire animation install with `log::warn!`; pre-existing `animation` unchanged | `None` (no curve) |
| `FogAnimation.phase` | finite when `Some` | `rem_euclid(1.0)` to `[0, 1)` (handles negative inputs cleanly; `.fract()` would return a negative value for negative inputs); non-finite → treat as `None` with `log::warn!` | `None` (== 0.0) |
| `FogAnimation.playCount` | `Some(n)` where `n >= 1`, or `None` | `Some(0)` is bumped to `Some(1)` with `log::warn!` (one-shot; intentionally differs from `light_bridge`'s GPU-evaluator behavior — see Task 2 note) | `None` (loop forever) |

**Note:** `density` has no upper clamp; arbitrarily large values are accepted and saturate the shader. If a future FGD cap is introduced, clamp to match it.

## Rough sketch

Implementation pivot points in source:

- `crates/postretro/src/scripting/components/fog_volume.rs` — new file (or new module within an existing fog component file). Defines `FogAnimation` mirroring `LightAnimation`'s shape (period_ms, phase, play_count, start_active, density curve). Exported from `components::mod` for `registry.rs` and the new primitive file to import.
- `crates/postretro/src/scripting/registry.rs` — `FogVolumeComponent` gains `falloff: f32` and `animation: Option<FogAnimation>`. The struct keeps `Clone`/`PartialEq`/`Serialize`/`Deserialize` but loses `Copy` (the `Vec<f32>` density curve is heap-backed). Audit existing `Copy` consumers and switch them to `.clone()`. Serde field rename (`radial_falloff` → `falloff`) is *not* applied — the script-facing rename lives on the typedef and conv layer, not on the wire. (`falloff` is already camelCase; no `#[serde(rename)]` attribute is needed for that field. The struct's existing serde derives suffice.)
- `crates/postretro/src/scripting/systems/fog_volume_bridge.rs` — drop `radial_falloff` from `FogVolumeAabb`. `populate_from_level` writes `entry.radial_falloff` into the spawned `FogVolumeComponent.falloff`. `update_volumes` reads `component.falloff` and copies it to `FogVolume.radial_falloff`. New `FogVolumeBridge::tick` (or `tick_animations`) method evaluates `FogVolumeComponent.animation` each frame and writes the sampled density back into `component.density` before `update_volumes` is called. Add a `HashMap<EntityId, FogAnimSlot>` side-table for `animation_start_time` (parallels `LightSlot`). Existing tests `update_volumes_packs_density_and_edge_softness_from_component` and `populate_from_level_spawns_one_entity_per_record_with_component` get a fourth field check.
- `crates/postretro/src/scripting/conv.rs` — extend the `"fog_volume"` arm in both `from_js` and `from_lua` (lines ~375 and ~469) to parse `falloff`. Update `into_js`/`into_lua` to emit it. Update the `fog_volume_component_round_trips_through_quickjs` and `…_through_luau` tests at ~L797 and ~L840 to set and assert `falloff`.
- `crates/postretro/src/scripting/typedef.rs` — `FogVolumeComponent` declaration (TS at ~L968, Luau at ~L1058) gains `falloff: number` and `animation: FogAnimation | null`. New `FogAnimation` type definition next to `LightAnimation` (TS at ~L490, Luau at ~L850), and a new `SetFogAnimationStep` entry next to the other fog step definitions, added to the `SequenceStep` union.
- `crates/postretro/src/scripting/reactions/` — six new files (one per primitive, including `set_fog_animation.rs`); `registry.rs` gains `register_fog_reaction_primitives` and a wiring call.
- `crates/postretro/src/scripting/reactions/mod.rs` — `pub(crate) mod set_fog_density;` and five siblings (including `set_fog_animation`).
- Per-frame fog tick wiring — wherever the engine currently calls `fog_volume_bridge.update_volumes` each frame, prepend `fog_volume_bridge.tick(&mut registry, time_seconds)`. (Likely `main.rs` or a per-frame system module; identify the exact call site during Task 5.)
- Call site that wires reaction primitives at engine init — `main.rs` around line 172 (alongside the `register_emitter_reaction_primitives` call). Extend with `register_fog_reaction_primitives(&mut reactions)`.
- `sdk/lib/entities/fog_volumes.ts` — replace the pass-through `wrapFogVolumeEntity` with a read-only `FogVolumeHandle` wrapper; rewrite `fogPulse` / `fogFade` to return single-element `SetFogAnimationStep[]` (curve packed into `args.density`, `periodMs` carried at the args root). Drop the previous "fan out into 16 `setFogDensity` steps" body.
- `sdk/lib/entities/fog_volumes.luau` — same shape as the TS file; install `FogVolumeHandle` type and `wrapFogVolumeEntity`, `fogPulse`, `fogFade` in the returned table; `luau.rs` prelude evaluation already includes this file.
- `sdk/lib/prelude.js` — regenerate via `cargo run -p postretro-script-compiler -- --prelude --sdk-root sdk/lib --out sdk/lib/prelude.js`.
- `sdk/types/postretro.d.{ts,luau}` — regenerated; the new primitives surface as global functions on the script API.
- `docs/scripting-reference.md` — new section under "Reaction primitives" listing `setFogDensity` / `setFogScatter` / `setFogEdgeSoftness` / `setFogFalloff` / `setFogParams`; `FogVolumeComponent` row in the components table gains `falloff`.
- `content/tests/scripts/fog-pulse-demo.ts` — reference example; mirrors `arena-lights.ts`.

## Open questions

1. **Combined `setFogParams` vs. four single-field primitives.** Resolved: ship both. The four single-field primitives are simpler to call from a sequence step and minimize the per-step JSON payload. `setFogParams` is the right call when an author wants to change two or more fields atomically — without it, two single-field calls in adjacent steps would briefly observe a partial update on the GPU. Cost is one extra primitive registration; benefit is the partial-update ergonomic that the dropped `setComponent` plan promised.
2. **Per-classname enforcement on `falloff`.** Resolved: no enforcement. Accept `falloff` on every fog entity. For `fog_volume` plane-sweep volumes the value is stored on the component but the shader's plane-sweep path doesn't read `radial_falloff`; documented in `docs/scripting-reference.md`. Same posture as the dropped Task 5 of `fog-ellipsoid-entity`.
3. **Should `falloff` be `Option<f32>` to preserve the wire-loaded value?** Resolved: no. The original `setComponent` plan needed `Option<f32>` because partial JSON inputs deserialized absent fields as `None`. With reaction primitives, partial updates are expressed by *which primitive is called*, not by which fields are present in a serde payload. `setFogDensity` only touches `density`; the other three fields keep their current component value because the dispatch reads-modifies-writes the full component. `setFogParams` makes the same distinction explicit by typing absent fields as `Option`. The component itself stays plain `f32` for all four fields — same shape as `LightComponent.intensity`.
4. **Keyframed `FogAnimation` channel inside `FogVolumeComponent`?** Resolved: in scope (Task 0 + Task 5). The original out-of-scope verdict assumed sequenced `setFogDensity` calls could substitute for a curve channel, but the sequence dispatcher fires every step on the same frame — a 16-step `fogPulse` collapsed to its last sample with no time-varying playback. `FogAnimation` is the right channel for time-varying density; the existing scalar primitives stay the right tool for one-shot story-event changes. The fog evaluator runs in CPU each frame inside the bridge (not GPU like lights) — fog volume counts are low enough that the CPU cost is trivial, and the data flows back through `FogVolumeComponent.density` so the existing GPU pack path is unchanged.
5. **Density is animated; perception is non-linear — does the API need to compensate?** Resolved: no API change for v1; document the caveat. Fog density drives Beer-Lambert-style accumulation along a view ray (`transmittance ≈ exp(-density · distance)`), so a uniform sine on density does not produce a uniform-feeling change in visibility — at high density the same delta is far more visible than at low density. This is real, but it isn't unique to fog: light intensity has a similar non-linearity in eye response, and the project ships `pulse` on light without compensation. Two specific consequences for authors using `fogPulse` / `fogFade`:

   - Pick conservative `min`/`max` bounds. A pulse from `0.05` to `0.5` reads as gentle breathing; a pulse from `0.5` to `2.0` looks like the world is being repeatedly snapped to opaque because the upper end saturates.
   - `fogFade` from `0` to a target density reads as "fog rolls in" with a perceptually slow start and fast end. That is usually what authors want for a fade-in. A `fogFade` from a high density back to `0` will feel abrupt at the high end and crawl near zero — author with that asymmetry in mind, or sample two fades back-to-back.

   These are doc-comment notes on the constructors and a paragraph in `docs/scripting-reference.md` — not API changes. We considered exposing a perceptual-curve flag (e.g., animate `1 - exp(-density)` instead of `density`), but the value of a "perceptually linear pulse" doesn't justify a second curve interpretation in the evaluator, and authors who really want one can pre-bake the inverse into their `density` array. Revisit only if a scene's authors hit this concretely.

   `scatter` and `edge_softness` remain non-animated for v1 (already in Out of scope). `scatter` is the most plausible second channel to animate — small periodic `scatter` modulation produces shimmering halos around in-fog lights, which is a different effect from density breathing — but it is additive on top of density animation, not a replacement, and ships when a scene needs it.

6. **Removing `setComponent` dispatch surface entirely.** The `mod.rs` "forbidden primitives" test already asserts `setComponent` is absent from the registry — so this spec is purely additive on the reactions side, with no removal work. If a future spec re-introduces a live-mutation primitive for any subsystem, the fog parameters must continue to flow through reactions; the rationale (no live VM at runtime) is engine-wide.
