# Fog Volume Reactions

## Goal

Expose runtime control of fog volume parameters (`density`, `scatter`, `edge_softness`, `falloff`) for all fog entity types (`fog_volume`, `fog_lamp`, `fog_tube`, `fog_ellipsoid`) via named reaction primitives and the reactions API. Replace the existing `setComponent`-based fog mutation path so the scripting VM is not live at runtime. All behavior executes in Rust; scripts declare intent at load time.

This is the companion to `fog-ellipsoid-entity`. That spec lands the new shape and gets fog ellipsoids compiling and rendering at level load. This spec lands the runtime-control surface that was originally the dropped Task 5 of that spec, redesigned around the same reactions pattern that drives light animations and emitter rate/spin (`setLightAnimation`, `setEmitterRate`, `setSpinRate`).

## Scope

### In scope

- New named reaction primitives `setFogDensity`, `setFogScatter`, `setFogEdgeSoftness`, `setFogFalloff`. Each is tag-targeted and applies to every fog entity matching the reaction's tag, regardless of subtype (`fog_volume`, `fog_lamp`, `fog_tube`, `fog_ellipsoid`).
- A combined `setFogParams` primitive accepting any subset of the four fields in one args object. Mirrors the partial-update ergonomic that the dropped `setComponent` plan offered, expressed as a single reaction call instead of a live mutation.
- A new `falloff` field on `FogVolumeComponent` (the only mutable field added by this spec), surfaced through wire load and the reaction primitives.
- Removal of `FogVolumeComponent` from any future `setComponent` dispatch surface — there is no live-VM path for fog mutation. The `scripting/primitives/mod.rs` "forbidden primitives" test already guards against `setComponent` regressing into the registry; this spec's contribution is to ensure the reaction-primitive path is the only documented mutation surface for fog.
- SDK fog vocabulary file additions: extend `sdk/lib/entities/fog_volumes.{ts,luau}` with a read-only `FogVolumeHandle` wrapper and pure animation constructors (`fogPulse`, `fogFade`) that build step-list descriptors for `registerReaction` — matching the sequence step shape from `arena-lights.ts`.

### Out of scope

- Changing the fog volume's geometry (AABB, planes, half-extent) at runtime. Only the four scalar parameters are mutable.
- A keyframed `FogAnimation` type built into `FogVolumeComponent` analogous to `LightAnimation`. Fog uses sequenced reactions (the same path emitter rate uses) rather than a per-component animation channel — fog parameters do not need GPU-side phase evaluation.
- Per-classname enforcement at the script boundary. `falloff` is accepted on every fog entity; the shader path it reaches depends on the volume type. Documented; no error.
- Per-entity fog color (settled elsewhere: ambient comes from the SH irradiance volume).
- Anything in `fog-ellipsoid-entity` (FGD, compiler resolver, shader branch, `shape_mode` discriminant). Those land independently.

## Acceptance criteria

- [ ] A data script can register a sequenced reaction that calls `setFogDensity` against a tag and the visible fog density on every tagged fog entity changes on the frame the reaction fires, with no scripting VM running on the tick path.
- [ ] The same script pattern works for `setFogScatter`, `setFogEdgeSoftness`, `setFogFalloff`, and `setFogParams` (partial object).
- [ ] `setFogParams` accepts any subset of `{density, scatter, edgeSoftness, falloff}`. Absent fields leave the corresponding component value at its prior value (wire-loaded at level start; the most recent reaction-applied value thereafter). Negative or non-finite numeric inputs are handled per the per-field rules in the validation table; `log::warn!` records each violation. For `setFogParams`: each field is validated independently; invalid fields are skipped, valid fields applied, component written once per target.
- [ ] `setFogFalloff` is accepted on every fog entity type. For `fog_volume` (plane-sweep) it updates the stored value but is not consulted by the shader. For `fog_lamp` and `fog_tube` it changes the radial exponent. For `fog_ellipsoid` it drives the ellipsoid path. No error is raised on any subtype.
- [ ] Tag-targeting matches `setEmitterRate`'s semantics: the reaction's `tag` filter resolves to a target list at dispatch time; entities lacking a `FogVolumeComponent` are skipped with `log::warn!` (typo guard); empty target sets are a debug-log no-op.
- [ ] The primitive registry's "forbidden primitives" test (`scripting/primitives/mod.rs`) continues to assert `setComponent` is absent. No new dispatch arm is added that would re-introduce a live mutation path for fog.
- [ ] SDK type generation (`cargo run -p postretro --bin gen-script-types`) emits typed argument shapes for the new primitives. The drift-detection test in `cargo test` passes after regeneration.
- [ ] The fog volume bridge cache (`FogVolumeAabb`) carries no `Option<f32>` override fields and is not extended with per-component override slots — the bridge reads `FogVolumeComponent` directly the same way it does today, with `falloff` joining the existing `density`/`scatter`/`edge_softness` set.
- [ ] `fogFade(id, from, to)` constructor exists in both `fog_volumes.ts` and `fog_volumes.luau`, returns a step array with linearly-interpolated `setFogDensity` steps, and is exercised by a unit test. (No timing parameter: the sequence dispatcher fires every step on the same frame; pacing is not a constructor input.)
- [ ] An author-visible script using the new vocabulary (one fog-driven scene in `content/tests/scripts/`) demonstrates a `fogPulse` against a tag and runs cleanly in the QuickJS runtime. (No `.luau` mirror of the demo script is required — `content/tests/scripts/` has no `.luau` analogs to the `.ts` scripts; the prelude integration is verified by the Luau round-trip test in Task 4.)

## Tasks

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

Per-target behavior matches `set_emitter_rate.rs`:

- Missing `FogVolumeComponent` → `log::warn!`, skip (tag matched a non-fog entity — most likely a tag typo).
- Out-of-range numeric input → per-field clamp + `log::warn!` once per field. See validation table below.
- Empty target set → no-op, debug log.

For `set_fog_params.rs`: each field is validated independently. A field that fails validation is dropped with `log::warn!`; valid fields are still written. The component is mutated once per target with the merged result. If all fields fail validation for a target, the component is not written for that target — no dirty flag is set.

Register the five primitives in `reactions/registry.rs` alongside the existing `register_emitter_reaction_primitives` via a new `register_fog_reaction_primitives(&mut ReactionPrimitiveRegistry)`. Wire it into the same call site that wires the emitter primitives (see Rough sketch for the call site file and location).

Each `Set*Args` struct derives `Deserialize` with `#[serde(rename_all = "camelCase")]`, matching `SetEmitterRateArgs`. All five carry the attribute uniformly for consistency, even where the rename is a no-op for a given field.

After registering the five primitives, run `cargo run -p postretro --bin gen-script-types` and commit the updated `sdk/types/postretro.d.{ts,luau}`.

### Task 3: SDK fog vocabulary

Extend `sdk/lib/entities/fog_volumes.{ts,luau}` to mirror the `lights.{ts,luau}` shape:

- Replace the existing pass-through `FogVolumeHandle` type alias with a proper wrapper that exposes typed read access to the fog component fields. No mutation methods — authors build sequenced reactions via `registerReaction` and the fog primitives directly.
- Pure animation constructors:
  - `fogPulse(id, min, max)` — returns a `{ id, primitive, args }` step array whose steps emit `setFogDensity` calls at density values sampled along a full sine cycle (`mid + amp * sin(2π·i/N)`) between `min` and `max`. Step count and `id` shape mirror the `pulse` constructor in `sdk/lib/entities/lights.ts`. No timing parameter — the sequence dispatcher fires every step on the same frame, so pacing is not a constructor input.
  - `fogFade(id, from, to)` — returns a `{ id, primitive, args }` step array that linearly interpolates `density` from `from` to `to` across N evenly-spaced steps. No timing parameter, same reason as `fogPulse`.

Both return `{ id, primitive, args }` step arrays matching the sequence step shape in `arena-lights.ts`. Authors wrap in `registerReaction`.

Wire the new file into both preludes — `sdk/lib/prelude.js` regenerated via the prelude-bundler (see `context/lib/scripting.md` §8) for TypeScript. For Luau: `fog_volumes.luau` is already in the prelude eval order; no new eval-order entry in `luau.rs` is needed. Any new bare globals exported from the updated file (e.g., `fogPulse`, `fogFade`) must be added to the `FOG_VOLUMES_LUAU_FIELDS` slice in `luau.rs` (line 72), which drives the publics-lifting loop at lines 148–160. Currently defined as `&[]`; extend it to match the new exports, e.g. `const FOG_VOLUMES_LUAU_FIELDS: &[&str] = &["fogPulse", "fogFade"];`. The loop reads each name from the table returned by `fog_volumes.luau` and installs it as a bare global — the same pattern `LIGHTS_LUAU_FIELDS` uses for `flicker`, `pulse`, `colorShift`, `sweep`.

After editing `fog_volumes.ts`, regenerate `sdk/lib/prelude.js` via `cargo run -p postretro-script-compiler -- --prelude --sdk-root sdk/lib --out sdk/lib/prelude.js` and commit the result.

### Task 4: Reference test scene and round-trip tests

- One new test script in `content/tests/scripts/` (e.g. `fog-pulse-demo.ts`) that calls `world.query({ component: "fog_volume", tag: ... })`, builds a `fogPulse` descriptor, and registers it via `registerReaction("levelLoad", { sequence: [...] })`. Mirrors `arena-lights.ts`'s shape exactly.
- Round-trip tests in `reactions/registry.rs` (`registers_all_fog_primitives_under_expected_names`) and per-primitive dispatch tests in each new file (analog to the `setEmitterRate` tests that assert clamp behavior, missing-component skip, empty-target no-op).
- Cross-runtime parity test: same `setFogParams` JSON arg dispatched through the sequenced primitive registry from a QuickJS-shaped JSON value and a Luau-shaped JSON value produces identical post-mutation `FogVolumeComponent` state. See `set_light_animation_quickjs_and_luau_produce_identical_output` (in `crates/postretro/src/scripting/primitives/light.rs`) for the exact two input shapes this test mirrors.
- Unit tests for `fogPulse` and `fogFade` (TypeScript and Luau) asserting the returned step arrays have the expected step count, linearly interpolated or cosine-sampled density values, and `{ id, primitive, args }` shape.

## Sequencing

**Phase 1 (sequential):** Task 1 — extends `FogVolumeComponent` with `falloff` and rewires the bridge to read it from the component. Every reaction primitive depends on the component carrying all four fields.

**Phase 2 (concurrent):** Task 2 (reaction primitives), Task 3 (SDK vocabulary). Independent files; Task 2 lands the Rust dispatch surface, Task 3 lands the script-facing builders.

**Phase 3 (sequential, depends on Phase 2):** Task 4 — reference scene + round-trip tests. Cannot be written until both the primitive surface and the SDK helpers exist.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | Reaction primitive |
|---|---|---|---|---|---|
| Density | `FogVolumeComponent.density: f32`, `FogVolume.density` | `density: f32` (existing) | `FogVolumeComponent.density: number` | `FogVolumeComponent.density: number` | `setFogDensity { density }`; also `setFogParams.density?` |
| Scatter | `FogVolumeComponent.scatter: f32`, `FogVolume.scatter` | `scatter: f32` (existing) | `FogVolumeComponent.scatter: number` | `FogVolumeComponent.scatter: number` | `setFogScatter { scatter }`; also `setFogParams.scatter?` |
| Edge softness | `FogVolumeComponent.edge_softness: f32`, `FogVolume.edge_softness` | `edge_softness: f32` (existing) | `FogVolumeComponent.edgeSoftness: number` | `FogVolumeComponent.edgeSoftness: number` | `setFogEdgeSoftness { edgeSoftness }`; also `setFogParams.edgeSoftness?` |
| Falloff | `FogVolumeComponent.falloff: f32` (new), `FogVolume.radial_falloff` | `radial_falloff: f32` (existing wire field, no rename) | `FogVolumeComponent.falloff: number` | `FogVolumeComponent.falloff: number` | `setFogFalloff { falloff }`; also `setFogParams.falloff?` |

**Asymmetry note (carried from `fog-ellipsoid-entity`).** Script-facing field is `falloff`; wire and Rust-internal field stays `radial_falloff` for the same reason: avoiding a much larger rename across `MapFogVolume`, `FogVolumeRecord`, the GPU struct, the WGSL struct, and the existing PRL wire format. The bridge maps `FogVolumeComponent.falloff` ↔ `FogVolume.radial_falloff` at the existing copy site.

### Validation table

| Field | Valid range | On out-of-range | Default at level load |
|---|---|---|---|
| `density` | `[0.0, +∞)`, finite | clamp to `0.0`; `log::warn!` once | wire-loaded `density` (FGD default `0.5`) |
| `scatter` | `[0.0, 1.0]`, finite | clamp into range; `log::warn!` once | wire-loaded `scatter` (FGD default `0.6`) |
| `edge_softness` | `[0.0, +∞)`, finite | clamp to `0.0`; `log::warn!` once | wire-loaded `edge_softness` (FGD default per entity type) |
| `falloff` | `(0.0, +∞)`, finite | skip field with `log::warn!`; component `falloff` unchanged for this target (applies in both `setFogFalloff` and `setFogParams`) | wire-loaded `radial_falloff` (FGD default per entity type: `fog_lamp` = 2.0, `fog_tube` = 1.5) |

**Note:** `density` has no upper clamp; arbitrarily large values are accepted and saturate the shader. If a future FGD cap is introduced, clamp to match it.

## Rough sketch

Implementation pivot points in source:

- `crates/postretro/src/scripting/registry.rs` — `FogVolumeComponent` gains `falloff: f32`. The struct stays `Copy`/`Clone`/`PartialEq`/`Serialize`/`Deserialize`; serde field rename (`radial_falloff` → `falloff`) is *not* applied — the script-facing rename lives on the typedef and conv layer, not on the wire. (`falloff` is already camelCase; no `#[serde(rename)]` attribute is needed for that field. The struct's existing serde derives suffice.)
- `crates/postretro/src/scripting/systems/fog_volume_bridge.rs` — drop `radial_falloff` from `FogVolumeAabb`. `populate_from_level` writes `entry.radial_falloff` into the spawned `FogVolumeComponent.falloff`. `update_volumes` reads `component.falloff` and copies it to `FogVolume.radial_falloff`. Existing tests `update_volumes_packs_density_and_edge_softness_from_component` and `populate_from_level_spawns_one_entity_per_record_with_component` get a fourth field check.
- `crates/postretro/src/scripting/conv.rs` — extend the `"fog_volume"` arm in both `from_js` and `from_lua` (lines ~375 and ~469) to parse `falloff`. Update `into_js`/`into_lua` to emit it. Update the `fog_volume_component_round_trips_through_quickjs` and `…_through_luau` tests at ~L797 and ~L840 to set and assert `falloff`.
- `crates/postretro/src/scripting/typedef.rs` — `FogVolumeComponent` declaration (TS at ~L968, Luau at ~L1058) gains `falloff: number`.
- `crates/postretro/src/scripting/reactions/` — five new files (one per primitive); `registry.rs` gains `register_fog_reaction_primitives` and a wiring call.
- `crates/postretro/src/scripting/reactions/mod.rs` — `pub(crate) mod set_fog_density;` and four siblings.
- Call site that wires reaction primitives at engine init — `main.rs` around line 172 (alongside the `register_emitter_reaction_primitives` call). Extend with `register_fog_reaction_primitives(&mut reactions)`.
- `sdk/lib/entities/fog_volumes.ts` — replace the pass-through `wrapFogVolumeEntity` with a read-only `FogVolumeHandle` wrapper; add `fogPulse`, `fogFade` constructors.
- `sdk/lib/entities/fog_volumes.luau` — same shape as the TS file; install `FogVolumeHandle` type and `wrapFogVolumeEntity`, `fogPulse`, `fogFade` in the returned table; `luau.rs` prelude evaluation already includes this file.
- `sdk/lib/prelude.js` — regenerate via `cargo run -p postretro-script-compiler -- --prelude --sdk-root sdk/lib --out sdk/lib/prelude.js`.
- `sdk/types/postretro.d.{ts,luau}` — regenerated; the new primitives surface as global functions on the script API.
- `docs/scripting-reference.md` — new section under "Reaction primitives" listing `setFogDensity` / `setFogScatter` / `setFogEdgeSoftness` / `setFogFalloff` / `setFogParams`; `FogVolumeComponent` row in the components table gains `falloff`.
- `content/tests/scripts/fog-pulse-demo.ts` — reference example; mirrors `arena-lights.ts`.

## Open questions

1. **Combined `setFogParams` vs. four single-field primitives.** Resolved: ship both. The four single-field primitives are simpler to call from a sequence step and minimize the per-step JSON payload. `setFogParams` is the right call when an author wants to change two or more fields atomically — without it, two single-field calls in adjacent steps would briefly observe a partial update on the GPU. Cost is one extra primitive registration; benefit is the partial-update ergonomic that the dropped `setComponent` plan promised.
2. **Per-classname enforcement on `falloff`.** Resolved: no enforcement. Accept `falloff` on every fog entity. For `fog_volume` plane-sweep volumes the value is stored on the component but the shader's plane-sweep path doesn't read `radial_falloff`; documented in `docs/scripting-reference.md`. Same posture as the dropped Task 5 of `fog-ellipsoid-entity`.
3. **Should `falloff` be `Option<f32>` to preserve the wire-loaded value?** Resolved: no. The original `setComponent` plan needed `Option<f32>` because partial JSON inputs deserialized absent fields as `None`. With reaction primitives, partial updates are expressed by *which primitive is called*, not by which fields are present in a serde payload. `setFogDensity` only touches `density`; the other three fields keep their current component value because the dispatch reads-modifies-writes the full component. `setFogParams` makes the same distinction explicit by typing absent fields as `Option`. The component itself stays plain `f32` for all four fields — same shape as `LightComponent.intensity`.
4. **Keyframed `FogAnimation` channel inside `FogVolumeComponent`?** Out of scope. Lights have a baked-in `LightAnimation` because the GPU evaluator phase-walks the curve every frame. Fog parameters are set by sequenced reactions; the engine has no per-frame phase-walker for fog. If a future scene needs sub-millisecond-accurate density curves, revisit — but the bar is "what `setLightAnimation` provides cannot be matched by sequenced `setFogDensity` steps at the chosen step granularity."
5. **Removing `setComponent` dispatch surface entirely.** The `mod.rs` "forbidden primitives" test already asserts `setComponent` is absent from the registry — so this spec is purely additive on the reactions side, with no removal work. If a future spec re-introduces a live-mutation primitive for any subsystem, the fog parameters must continue to flow through reactions; the rationale (no live VM at runtime) is engine-wide.
