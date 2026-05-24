# Directional Fog

## Goal

Give the fog pass's SH ambient in-scatter a view-direction-dependent term. Today
`sample_sh_fog` collapses the full L2 SH radiance to a single fixed world-up
read (`§7.5`: "fog is directionally isotropic … without view-direction
dependence"). The baked SH already encodes directional radiance; this plan
weights that radiance by a Henyey-Greenstein phase function along the view ray
so fog brightens when the camera looks toward regions of strong baked indirect
light. Per-volume `scatter_bias` KVP (float, 0..100, mapped to HG `g`
at compile time) controls the effect. This is M9 spec #4.

## Scope

### In scope

- New per-volume `scatter_bias` authored KVP (0..100) on `fog_volume`, `fog_lamp`, and `fog_tube`: FGD → compiler
  (translate to HG `g = clamp(authored / 100.0 * 0.9, 0.0, 0.9)`) → PRL
  `FogVolumeRecord.anisotropy: f32` wire format → GPU `FogVolume` → WGSL.
- New per-volume `ambient_scatter` authored KVP (0.0..1.0, default 1.0):
  scales the SH ambient contribution; 0.0 = fog invisible except where
  dynamic lights shine through it. Same pipeline as `scatter_bias`.
- Henyey-Greenstein phase weighting of the fog SH ambient term in
  `fog_volume.wgsl`, using the view ray direction and the per-step
  density-weighted blended `g` (derived from `scatter_bias`).
- `scatter_bias = 0` and `ambient_scatter = 1.0` reproduce the current output
  within float tolerance, so existing maps are visually unchanged.
- Preserve the existing distance-based SH cache schedule and its frame-stability
  guarantees.
- Tests: wire round-trip, GPU-record layout/stride, WGSL parse + naga validation
  for the fog module, and a CPU reference for the HG phase weight.
- A before/after directional-fog measurement note in this plan's
  `measurements/` folder.

### Out of scope

- Phase weighting of the dynamic spot-beam and point-light scatter — those stay
  isotropic. Only the SH ambient term gains directionality.
- Feeding the directional/sun light (type 2) into fog. Fog still marches only
  spot + point lights for direct scatter.
- Adding `scatter_bias`, `ambient_scatter`, or the derived runtime `g` value
  to the script-visible `FogVolumeComponent`, the scripting SDK surface, or
  fog animation curves. Both parameters are static/authored only;
  runtime-animated variants are a possible follow-up.
- Changes to the depth-aware SH path. Fog continues to use the shared helper
  (`sample_sh_indirect_corners`, called with `reject_backface = false`); Chebyshev visibility
  stays off for fog.
- **Back-scatter** (signed/negative `scatter_bias`, `g < 0`). Forward-only ships
  in this plan. Back-scatter needs a third cached directional SH read and is
  deferred as an optional Milestone 9 goal. The wire format stores `anisotropy`
  as a signed `f32`, so widening the authored range later is a runtime/shader-
  only change — no PRL format break.
- New PRL sections, height/gradient fog, or a true radiance-SH re-bake (see
  Open questions on the cosine-convolution approximation).

## Acceptance criteria

The directional effect has no automated image test — visual output is verified
manually by the author via A/B frame comparison and recorded in
`measurements/directional.md`. The criteria are split accordingly: the
**Automated** group is test-gated and must be green; the **Manual visual** group
is confirmed by eye. The automated group is the real correctness gate for the
wiring; the manual group confirms the look and the parity that the math alone
cannot fully prove.

### Automated (test-gated)

- [ ] `scatter_bias` translates to `g` via `clamp(authored / 100.0 * 0.9, 0.0,
      0.9)`; a value outside 0..100 logs a warning and clamps. Asserted in the
      compiler round-trip test, including the clamp endpoints `0` and `0.9`.
- [ ] `scatter_bias` and `ambient_scatter` round-trip through the
      `FogVolumeRecord` wire format (`to_bytes` / `from_bytes`); an unset
      `scatter_bias` defaults to `0`, an unset `ambient_scatter` to `1.0`.
      Asserted in the wire round-trip test.
- [ ] The GPU `FogVolume` packs `anisotropy` at offset 104 and `ambient_scatter`
      at offset 108, stride 112. Asserted by the `pack_fog_volumes` round-trip test
      (which reads bytes 104..108 as `anisotropy` and 108..112 as `ambient_scatter`,
      extending its existing offset spot-checks) and the `FOG_VOLUME_SIZE == 112` guard.
- [ ] Maps recompiled against the extended `FogVolumeRecord` load without format
      errors; the layout doc-comment and `MIN_RECORD_SIZE` match the new record.
- [ ] Fog raymarch WGSL parses and passes naga validation (forward and billboard
      modules unaffected). Asserted by the `fog_pass.rs` tests.
- [ ] CPU reference for the HG phase weight: peak toward the lobe direction,
      symmetric falloff, **`g = 0` is uniform** (a property of the phase function itself; runtime `g = 0`
      parity comes separately from the blend weight collapsing to 0, asserted below), and finite — no
      NaN — at the clamp endpoints `0` and `0.9`.
- [ ] CPU reference for the iso/dir blend asserts **no NaN and no blow-out, by
      construction**: the blend is a convex combination of the two SH reads
      (weight `∈ [0, 1]`; the phase steers the weight, never a multiplicative
      gain on irradiance), so for arbitrary coefficients and any `g ∈ [0.0, 0.9]`
      the output is finite and bounded componentwise by `[min(iso, dir), max(iso,
      dir)]` (so it inherits the sign of the underlying SH reads — the blend adds
      no negativity and no blow-out of its own). Brightness can exceed
      today's world-up read (the feature) but never exceeds the baked field's
      legitimate directional range.

### Manual visual (author A/B, recorded in `measurements/directional.md`)

- [ ] A `fog_volume` / `fog_lamp` / `fog_tube` with `scatter_bias > 0` looks
      brighter when the camera faces toward strong baked indirect light than when
      facing away; the effect scales with `scatter_bias`.
- [ ] With `scatter_bias = 0`, fog looks unchanged vs. the pre-change build on a
      representative map. (The construction guarantee is covered by the CPU test
      above; this confirms the rendered frame matches.)
- [ ] Overlapping fog volumes with different `scatter_bias` blend smoothly by
      density, consistent with how `tint` / `saturation` blend today.
- [ ] A `fog_volume` with `ambient_scatter = 0` shows no SH ambient contribution
      but still scatters dynamic spot and point light; a volume with unset
      `ambient_scatter` looks unchanged.
- [ ] The before/after note records map, camera poses (toward/away from light),
      `scatter_bias` value used, and commit.

## Tasks

### Task 1: Author surface + PRL wire format

Add a `scatter_bias(float)` KVP to `fog_volume`, `fog_lamp`, and `fog_tube` in
`sdk/TrenchBroom/postretro.fgd`, default `"0"`, with a modder-readable
description: "Directional scatter bias, 0 to 100. Higher values make fog glow
more strongly toward light. 0 = flat haze (default)." Parse it in all four resolvers
in `crates/level-compiler/src/parse.rs` (`resolve_fog_volume`,
`resolve_fog_ellipsoid`, `resolve_fog_lamp`, `resolve_fog_tube`) via
`props.get("scatter_bias")` with a `0.0` fallback, alongside the existing
`density` / `glow` / `tint` / `saturation` reads. (The single `fog_volume` FGD classname is backed by two resolvers — `resolve_fog_volume` for the plane-bounded brush variant and `resolve_fog_ellipsoid` for the axis-aligned ellipsoid — so the three authored entities map to four resolvers; the `scatter_bias` read is added to each of the four independently.) Translate the parsed value to a HG `g` coefficient:
`g = clamp(authored / 100.0 * 0.9, 0.0, 0.9)`. Log a warning when the
authored value falls outside 0..100 (matching existing invalid-KVP
behavior); the round-trip test asserts the translated and clamped `g` value.
Store this `g` as `anisotropy` — the internal name throughout the pipeline;
`scatter_bias` is the FGD-only surface. The field must be added in three places:
(1) `MapFogVolume` struct in `crates/level-compiler/src/map_data.rs`; (2) each
resolver's `MapFogVolume { … }` literal (the four resolvers in `parse.rs` build
`MapFogVolume`, not `FogVolumeRecord`); (3) the `FogVolumeRecord { … }` literal
in `encode_fog_volumes` in `crates/level-compiler/src/pack.rs`, which copies
field-by-field from `MapFogVolume` — this mirrors the plain-`f32` three-hop path that `tint` / `saturation` / `min_brightness` / `light_range` follow — a plain `f32` in `MapFogVolume`, the resolver literals, and `FogVolumeRecord` alike (unlike `is_ellipsoid: bool`, which converts to `shape_mode: f32` in `pack.rs`). The serde edits
(`to_bytes` / `from_bytes`, one little-endian `f32`), `MIN_RECORD_SIZE` update,
layout doc-comment, and test fixture fixes all apply to `FogVolumeRecord` in
`crates/level-format/src/fog_volumes.rs`.

Plumbing: `MIN_RECORD_SIZE` (currently 104) is asserted against remaining bytes
in `from_bytes` — bump it by 4 to 108 (anisotropy only, one f32); update the
explanatory comment toward "25 × f32 + 2 × u32 = 108". Task 5 bumps it further.

### Task 2: Thread `anisotropy` (derived g) to the GPU record

Carry the static `anisotropy` from `FogVolumeRecord` to the GPU `FogVolume`
without touching the scripting surface. In
`crates/postretro/src/scripting/systems/fog_volume_bridge.rs::populate_from_level`,
store `anisotropy` as a new field on the static `FogVolumeAabb` side-table (it is authored and baked-only, not runtime-settable — peers are `center` / `inv_half_ext` / `shape_mode`), **not** on `FogVolumeComponent` (where the runtime-settable `edge_softness` lives). In the bridge's GPU-record assembly in `update_volumes` (the
`(Some(component), Some(aabb)) => FogVolume { … }` arm and its `_ => FogVolume`
fallback), set `anisotropy: aabb.anisotropy` in the matched `(Some, Some(aabb))` arm and `anisotropy: 0.0` in the `_ =>` fallback. Add the
field to `crates/postretro/src/fx/fog_volume.rs::FogVolume` by shrinking `_pad6: [f32; 2]` to `[f32; 1]` (anisotropy consumes offset 104; `_pad6` now covers only offset 108) and adding `anisotropy: f32` at byte offset 104, so the stride stays 112 (the `assert!(FOG_VOLUME_SIZE == 112)` guard must still hold). Update both bridge initializers in `fog_volume_bridge.rs` — the `(Some, Some)` arm and the `_ =>` fallback — from `_pad6: [0.0; 2]` to `_pad6: [0.0; 1]`, and update the `pack_fog_volumes` round-trip test literal the same way. Extend that test to read bytes 104..108 and assert they equal the input `anisotropy`. Update the struct's layout doc-comment. Task 5 removes the final `_pad6` slot entirely when `ambient_scatter` consumes offset 108. (This "shrink to `[f32; 1]`, leave `_pad6_b`" is the end-state only if Task 2 lands alone; under the Phase 2 bundle, Task 5b lands in the same edit and `_pad6` is removed entirely — no pad remains, stride 112.) Add the matching field to the `FogVolume` struct in
`crates/postretro/src/shaders/fog_volume.wgsl` by replacing `_pad6_a` (offset
104) with `anisotropy: f32`; leave `_pad6_b` (offset 108) intact — Task 5
owns that replacement. Update the WGSL layout comment accordingly.

### Task 3: Directional phase-weighted SH scatter

In `crates/postretro/src/shaders/fog_volume.wgsl`, make the SH ambient term
view-dependent. Add a Henyey-Greenstein phase helper. Compute the per-step
density-weighted blended `g` (via `anisotropy`, the internal field name for
the translated `scatter_bias` value) across overlapping volumes the same way
the `vs_*_accum` family (`vs_tint_accum`, `vs_sat_accum`, etc.) is accumulated
(`vs_aniso_accum += contrib * v.anisotropy` inside the loop, then a post-loop
`let vs_anisotropy = vs_aniso_accum * inv_density` mirroring `vs_tint` /
`vs_saturation`). Replace the single isotropic `cached_sh` contribution with two cached reads —
`cached_sh_iso` (SH toward world-up, the existing read) and `cached_sh_dir` (SH
toward the view-derived direction). Both are refreshed together on the existing
`sh_coverage_dist` schedule: the `dir` evaluation direction derives from
`ray.direction`, which is constant per ray, so it caches on the same distance
schedule as iso. The per-step ALU is a `mix` between the two cached `vec3`s
using a weight derived from the blended `g` — cheap, not a textureLoad. Use
the blended `g` (defensively clamped to `[0.0, 0.9]` in WGSL before the phase
evaluation) and the constant-per-ray `ray.direction`. When `g == 0` the result
must equal the current world-up isotropic read (parity AC). Note this means a
single blended scalar `g` per step: where volumes of differing `anisotropy`
overlap, directionality blends too, so a `g = 0` volume is not locally isotropic
inside an overlap with a directional volume — this is intended, consistent with
how `tint` blends.

Plumbing: `sample_sh_fog` currently calls
`sample_sh_indirect_corners` with a fixed world-up normal. The
directional evaluation reuses that shared helper (binding-agnostic, owns the
8-corner blend) with a view-derived evaluation direction; do not redeclare or
fork the shared SH symbols. Only the fog-local `sample_sh_fog` wrapper changes — parameterized to take an evaluation direction (or split into an iso and a dir wrapper); both call the unchanged shared `sample_sh_indirect_corners`. The `sample_sh_fog_isotropic` / `sample_sh_fog_directional` names in the Rough sketch are illustrative of that split, not real symbols. See Rough sketch for the proposed evaluation and the
caching note.

### Task 4: Verify and measure

Keep the existing fog WGSL parse + naga-validation tests green (`fog_pass.rs`
tests). Add a CPU reference test for the HG phase weight (peak toward the lobe
direction, symmetric falloff, `g = 0` is uniform, stable for the clamp
endpoints) — this is the numeric half of the `g = 0` parity AC. Confirm the
rendered-frame half by visual A/B on a representative map. Author a quick test prop (a `fog_volume` with `scatter_bias` set)
near a strongly-lit region; capture toward-light vs away-from-light frames
and record map, poses, `scatter_bias` value used, and commit in
this plan's `measurements/directional.md`.

### Task 5: Per-volume ambient scatter scale

**(5a)** Add an `ambient_scatter(float)` KVP to `fog_volume`, `fog_lamp`, and
`fog_tube` in `sdk/TrenchBroom/postretro.fgd`, default `"1.0"`, description:
"Ambient light contribution to fog scatter. 1.0 = full ambient (default). 0.0 =
fog only visible where dynamic lights shine through it." Parse it in all four
resolvers via `props.get("ambient_scatter")` with a `1.0` fallback; clamp to
`[0.0, 1.0]` with a warning on out-of-range values. Add `ambient_scatter: f32`
using the same three-hop path as `anisotropy`: (1) `MapFogVolume` struct in
`map_data.rs`; (2) each resolver's `MapFogVolume { … }` literal in `parse.rs`;
(3) the `FogVolumeRecord { … }` literal in `encode_fog_volumes` in `pack.rs`.
The serde edits (`to_bytes` / `from_bytes`), `MIN_RECORD_SIZE` bump (+4, from
108 to 112, final comment "26 × f32 + 2 × u32 = 112"), and fixture fixes apply
to `FogVolumeRecord` in `fog_volumes.rs`.

**(5b)** Thread to `FogVolumeAabb` in `populate_from_level`; set from
`aabb.ambient_scatter` in the `(Some, Some(aabb))` arm and
`ambient_scatter: 1.0` in the `_ =>` fallback arm of `update_volumes` (0.0
would wrongly suppress SH ambient for fallback-arm volumes; 1.0 matches the KVP
default — same pattern as Task 2's `anisotropy: 0.0`). Add the field to
`FogVolume` in `crates/postretro/src/fx/fog_volume.rs` consuming the `_pad6_b`
slot at byte offset 108 (stride stays 112; `_pad6` is now fully consumed, remove
the field); update the layout doc-comment; extend the `pack_fog_volumes`
round-trip test. Read bytes 108..112 and assert they equal the input `ambient_scatter`. In `fog_volume.wgsl`, replace `_pad6_b` (offset 108) with
`ambient_scatter: f32`; update the WGSL layout comment — no trailing pad
remains, stride fully occupied at 112.

**(5c)** Accumulate density-weighted `vs_ambient_scatter_accum` across volumes
(same pattern as `vs_tint_accum`); multiply the SH ambient term by the blended `vs_ambient_scatter` before the
phase weighting is applied (scales only the SH ambient term; the dynamic
spot/point scatter accumulation is not affected, consistent with Out-of-scope). `ambient_scatter` scales only the `cached_sh` term before it is summed into the step scatter; the `min_brightness` floor (applied to the combined result) is unaffected, so the `ambient_scatter = 0` smoke-test must use a volume with `min_brightness = 0` to observe zero ambient contribution.

## Sequencing

**Phase 1 (sequential):** Tasks 1 + 5a — add both `scatter_bias` and
`ambient_scatter` to the FGD, resolvers, and `FogVolumeRecord` wire format
together.
**Phase 2 (sequential):** Tasks 2 + 5b — thread both fields to `FogVolumeAabb`
and the GPU `FogVolume` struct; land the full WGSL struct layout (both pad
slots consumed, stride 112 confirmed).
**Phase 3 (sequential):** Tasks 3 + 5c — add the phase math and the
`vs_ambient_scatter` multiply in the same shader edit.
**Phase 4 (sequential):** Task 4 — verifies layout, math, validation, and the
directional visual result; include an `ambient_scatter = 0` smoke-test.

## Rough sketch

The SH bands encode the directional radiance field; `sh_irradiance(dir)`
reconstructs the value toward `dir`. Today fog reads only `dir = world-up`.

```wgsl
// Proposed design — aesthetic, not physically exact (see Open questions).
fn hg_phase(cos_theta: f32, g: f32) -> f32 {
    let g2 = g * g;
    let denom = 1.0 + g2 - 2.0 * g * cos_theta;
    return (1.0 - g2) / (4.0 * PI * pow(max(denom, 1.0e-4), 1.5));
}

// iso  : current world-up read — the g == 0 baseline (parity).
// dir  : SH evaluated toward the in-scatter direction (light reaching the
//        camera travels along -ray.direction).
// Blend so g == 0 collapses to iso exactly; g > 0 leans on the directional read
// where the HG lobe points along the view ray.
let iso = sample_sh_fog_isotropic(pos);                 // existing evaluation
let dir = sample_sh_fog_directional(pos, -ray.direction);
// hg_phase is exercised by the CPU reference test; the runtime blend weight is saturate(g).
let scatter = mix(iso, dir, saturate(g));               // exact form: implementer's call
```

Forward-only: `g ∈ [0, 0.9]`, so `saturate(g)` is sufficient — no `abs()`
needed. Two firm constraints:
1. The `g == 0` path must reduce to the existing isotropic codepath by
   construction, which guarantees the parity AC.
2. The phase steers the blend *weight* between two real SH reads (`iso`, `dir`);
   it is never a multiplicative gain on the irradiance. With the weight in
   `[0, 1]` the output is a convex combination bounded by `[min(iso, dir),
   max(iso, dir)]` — finite, non-negative, and within the baked field's
   legitimate directional range. This is what makes "no NaN / no blow-out / no
   black" provable rather than visual.

- Caching: cache both the isotropic and directional SH reconstructions on the
  existing `t - t_last_sh_sample >= sh_coverage_dist` schedule (the frame-
  stability reasoning in `fog_volume.wgsl` applies to both). Two cached `vec3`s,
  not one. The phase scalar is recomputed per step (cheap) from the blended `g`.
  Avoid doubling textureLoads on every step.
- Clamp `g` to a stable HG range (`[0.0, 0.9]`); `g → 1` is degenerate.
- Storage: `anisotropy: f32` at offset 104 (replaces Rust `_pad6[0]` / WGSL
  `_pad6_a`); `ambient_scatter: f32` at offset 108 (replaces Rust `_pad6[1]`
  / WGSL `_pad6_b`); both pad slots consumed, stride stays 112.

## Boundary inventory

| Name | Compiler / PRL (`level-format`) | Wire / serde | Runtime (`fx`) | WGSL | FGD KVP |
|---|---|---|---|---|---|
| Fog scatter bias | `FogVolumeRecord.anisotropy: f32` (g after translation) | one LE `f32` per-volume record | `FogVolume.anisotropy` at offset 104 | `FogVolume.anisotropy` + `hg_phase` | `scatter_bias(float)` default `"0"` (0..100 → g via `clamp(v/100.0*0.9,0.0,0.9)`) |
| Fog ambient scale | `FogVolumeRecord.ambient_scatter: f32` | one LE `f32` per-volume record | `FogVolume.ambient_scatter` at offset 108 | `FogVolume.ambient_scatter` (multiplied onto SH ambient before phase) | `ambient_scatter(float)` default `"1.0"` (clamped to [0.0, 1.0]) |

No JS, Luau, or scripting-component name is added — the parameter is static and
renderer-internal past the PRL boundary (stored on `FogVolumeAabb`, not
`FogVolumeComponent`).

## Wire format

`FogVolumesSection` per-volume record (little-endian) gains two `f32` fields:
`anisotropy` (the translated HG `g`) and `ambient_scatter`, appended at the end of the fixed scalar block, immediately before the `plane_count` header — matching how `min_brightness` / `light_range` were added. `to_bytes`, `from_bytes`, the doc-comment layout, and `MIN_RECORD_SIZE` all move together. Mirror the
existing scalar fields (`density`, `saturation`): same endianness, same
per-volume placement convention. `anisotropy` is stored as a signed `f32` even
though the current authored range produces only `[0, 0.9]` — deliberate
headroom so back-scatter (negative `g`) can be enabled later with no format
change. Note: the byte offsets 104 and 108 (and the
stride 112) describe the `fx::FogVolume` GPU struct, which reuses pre-existing
`_pad6` slots. The PRL `FogVolumeRecord` is a separate struct that appends the
two f32 fields; `MIN_RECORD_SIZE` tracks its new total. Both happen to total 112
bytes but the layouts are independent — do not force PRL record fields to byte
offsets 104/108. Empty volume list is
unchanged (count `0`). No section version field exists; old `.prl` files must be
recompiled — consistent with the existing `FogVolume` "must be recompiled" note
for prior field additions.

## Open questions

- **Cosine-convolution approximation.** The baked SH carries cosine-lobe-
  convolved *irradiance*, not raw radiance (`sh_sample.wgsl` notes the
  Ramamoorthi-Hanrahan lobe is folded in at bake time). A physically exact phase
  convolution would need radiance-SH, which the runtime does not have. The
  directional term is therefore an aesthetic approximation on the available data.
  Acceptable for the look; flagged so a reviewer does not read it as physically
  grounded. Resolve the exact evaluation form (which direction, how the HG lobe
  modulates the iso/dir blend) during implementation against the `g = 0` parity
  AC and the visual A/B. Constrained, though: the form must be a convex blend of
  two real SH reads (weight `∈ [0, 1]`), never a multiplicative gain on
  irradiance — see Rough sketch firm constraint 2. That keeps brightness bounded
  by construction, so "no NaN / no blow-out / no black" is a CPU-test assertion,
  not a visual one.
- **g sign convention / default range.** Resolved: forward-only for this plan.
  `scatter_bias` 0..100 maps to `g` ∈ [0, 0.9]. Back-scatter (signed range,
  `g < 0`) is deferred as an optional Milestone 9 goal. The `anisotropy: f32`
  wire field is signed-capable, so widening the range later is a runtime/shader-
  only change — no PRL format break. Default `g = 0`.
- **`anisotropy` / `ambient_scatter` placement in the wire record.** Resolved: append both at the end of the fixed scalar block, immediately before the `plane_count` header, matching the `min_brightness` / `light_range` precedent. `to_bytes`, `from_bytes`, the doc-comment, and `MIN_RECORD_SIZE` move together; the round-trip test is the guard.
