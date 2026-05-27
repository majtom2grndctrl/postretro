# SDF Direct Shadows

## Goal

On a dedicated git branch, make a static signed-distance-field (SDF) of world geometry the single runtime source of **direct** shadows — for static and dynamic lights, point and spot. The SDF is baked into the PRL and consumed in a decoupled half-resolution shadow pass (not inline per-fragment as the removed `sdf-shadows` version was), accelerated by the existing DDGI probe depth moments. To make the static-light path correct, the branch's lightmap bake produces **unshadowed irradiance**; SDF supplies the runtime visibility term that multiplies into it.

The branch is the comparison surface. `main` keeps baked-lightmap static shadows + spot-map dynamic shadows; the SDF branch replaces both. A human compares the two builds by switching branches — visuals side by side, per-pass GPU time via `POSTRETRO_GPU_TIMING`. No runtime A/B toggle is needed for the comparison, which removes the coexistence constraint that previously forced static shadows out of scope.

This is an **experimental** feature: a correct, minimal foundation with room to grow, not a complete shadow system.

## Background

The old SDF (`sdf-shadows` tag) was slow because it did two per-pixel jobs: direct shadows *and* indirect-visibility cone tracing (~96 marches/fragment) to fight SH light leak. Milestone 9's DDGI depth moments + Chebyshev interpolant now own leak suppression, so the indirect job is gone. SDF returns for direct shadows only, decoupled and half-res. Full reasoning, the branch-comparison rationale, external research, and old data structures: `research.md`.

### Terminology

| Term | Meaning |
|---|---|
| Shadowed lightmap | `main`'s bake: static-light shadows folded into irradiance (occluded texels darkened). |
| Unshadowed lightmap | Branch bake: full static irradiance/bounce, **no** visibility term. Task 2a. |
| Shadow factor | Per-light `[0,1]` visibility scalar the SDF pass writes; 1 = lit, 0 = occluded. |
| `SdfShadowMode` | Debug selector: SDF on / off / visualize. Replaces the old 3-way `ShadowMode` (`research.md`). |

### Data flow

`unshadowed lightmap irradiance` (baked) → forward. `SDF atlas` (baked) → half-res shadow pass → `shadow-factor target` → bilateral upsample in forward → multiplies **every** shadowed light's term (static + dynamic). DDGI `E[d]` moment feeds the pass's open-space skip. Legacy PRL (shadowed lightmap, no SDF section): forward skips the static-term multiply and the pass is disabled — degrades to `main`-equivalent lighting.

## Scope

### In scope

- A baked **SDF atlas** PRL section: sparse brick grid of quantized distances + top-level index + coarse per-brick fallback distances, computed from static world geometry at compile time. Revives the old brick/coarse design (`research.md`).
- A `prl-build` bake stage producing the section, gated on a CLI flag, deterministic, integrated with the build cache.
- **Unshadowed-irradiance lightmap bake** (branch-only behavior): the static lightmap bakes full static-light irradiance and bounce with the **visibility/shadow term removed**, so runtime SDF can supply visibility without double-counting. The section signals "irradiance is unshadowed" so the runtime knows to apply SDF visibility to the static term.
- Runtime SDF resources: upload atlas to 3D textures + top-level storage buffer + meta uniform, owned by the renderer.
- A new **half-resolution SDF shadow pass** (compute) that runs after the depth pre-pass: reconstructs world position from depth, sphere-traces the SDF toward each shadow-casting light, and writes a packed per-light shadow factor to a half-res target. Uses the baked DDGI `E[d]` moment as an open-space skip early-out and the closest-passing-distance penumbra estimate for soft edges.
- Forward integration: sample the shadow-factor buffer with a depth-aware bilateral upsample, multiply **each light's** direct contribution by its factor — the static (lightmap) term and dynamic terms alike, because the lightmap is now unshadowed.
- Point-light direct shadows via SDF (new capability — point lights cast no dynamic shadow today).
- A debug **shadow on/off + visualize** toggle: an `SdfShadowMode` selector (SDF on / SDF off / SDF-visualize) for debugging, exposed as a debug-UI dropdown and a frame-uniform field. "Off" falls back to unshadowed lighting (no spot-map path on the branch); "visualize" renders the shadow factor for artifact-spotting.
- Debug-panel **quality sliders** (dev-tools) for live-tuning SDF and fog quality — see *Quality sliders* in the rough sketch for the per-knob feasibility split.

### Out of scope (non-goals)

- **Indirect / SH visibility via SDF.** Leak suppression stays on the DDGI depth-moment path. The shadow pass never traces the SDF for indirect lighting.
- **Spot shadow maps on the SDF branch.** The branch replaces the spot-map path with SDF for all direct shadows; `SpotShadowPool` machinery is bypassed, not deleted (it remains on `main`). No within-build spot-maps-vs-SDF runtime A/B — the branch is the A/B.
- **Merging the branch to `main`.** This draft delivers the comparison branch. Promotion to `main` (and any migration of the unshadowed bake to a default) is a later decision driven by the comparison result.
- **Dynamic occluders casting shadows** (animated enemies via capsule/mesh SDF insertion). The baked field contains static geometry only. Room to grow.
- **More than 4 simultaneously SDF-shadowed lights.** v1 packs factors into one `Rgba8Unorm` half-res target (4 channels). Lights beyond the cap render unshadowed. Room to grow (texture array).
- **Soft-shadow quality tuning passes / temporal accumulation of the shadow buffer.** Single-frame half-res + bilateral upsample only.
- **Removing the inline-SDF "naive" path for pedagogy.** Only the decoupled path is built; perf comparison uses the branch + `POSTRETRO_GPU_TIMING`.
- **Fog half-space-clip / step-quality knobs beyond `step_size` and `fog_pixel_scale`.** Only those two fog knobs are slider-exposed (see *Quality sliders*). The fog temporal-resolve `ACCUM_ALPHA` knob described in `rendering_pipeline.md` §7.5 does not exist in current source — not exposed (`research.md`).

## Acceptance criteria

Automated (test- or tooling-gated):

- [ ] The SDF atlas section round-trips byte-identically through serialize → deserialize, including the empty-geometry case (empty section, no panic). [T1]
- [ ] Re-running the SDF bake on identical input produces byte-identical section bytes (determinism contract holds). [T2]
- [ ] Bumping the SDF bake stage version invalidates the prior cache entry: first build after the bump is a miss and rebakes; the second is a hit. [T2]
- [ ] For a known map, a brick straddling a wall stores a near-zero distance at the surface and growing distances away from it; an all-open brick is marked empty/interior in the top-level index. Distinguishable in the baked data. [T2]
- [ ] The unshadowed bake omits the visibility term: for a map with a static light fully occluded from a surface, the corresponding lightmap texel is **non-zero** (lit), whereas the shadowed bake leaves it dark. A bake-mode flag selects the behavior; the section records which mode produced it. [T2a]
- [ ] With neither new `prl-build` flag set, the compiled output is byte-identical to `main`'s for the same map (the new bakes are opt-in; default builds are unchanged). [T2, T2a]
- [ ] An old `.prl` without the SDF section loads without error; the renderer reports "no SDF atlas" and the shadow pass is skipped (degradation path, not a failure). [T3]
- [ ] With SDF mode active and `POSTRETRO_GPU_TIMING=1`, a per-pass timing line for the SDF shadow pass is logged alongside the existing passes. [T4]
- [ ] The shadow-mode selector cycles through all modes via the debug-UI dropdown; the selected mode round-trips into the frame uniform. [T6]
- [ ] The SDF quality sliders (max march steps, open-space skip threshold, penumbra `k`) write through to the shadow-pass uniform; the fog `step_size` slider writes through to `FogParams` per frame, and the `fog_pixel_scale` slider triggers a scatter-target rebuild rather than a per-frame uniform write. [T7]

Manual / visual (observed by a human running the engine — not machine-verified):

- [ ] In SDF mode, a dynamic point light near static geometry casts a visible soft shadow (a capability absent on `main`).
- [ ] A static light casts shadows from static occluders via SDF, matching the look of `main`'s baked-lightmap shadows for the same scene (the branch-vs-branch comparison).
- [ ] Toggling the SDF shadow mode off leaves the scene fully lit with no shadow term (confirms the unshadowed lightmap + multiplied visibility split).
- [ ] SDF edges soften with distance from the occluder.
- [ ] SDF-visualize mode renders an interpretable shadow-factor (or march-step) view usable to spot artifacts.
- [ ] No light leak regression in indirect lighting (indirect is independent of this feature).
- [ ] Dragging the SDF and fog quality sliders visibly changes shadow/fog quality and per-pass GPU time without artifacts or crashes; the `fog_pixel_scale` slider re-blocks/un-blocks fog resolution live.

## Tasks

### Task 1: SDF atlas PRL section (level-format)

Revive `crates/level-format/src/sdf_atlas.rs`: the `SdfAtlasSection` struct, brick/top-level/coarse layout, slot sentinels, and serialize/deserialize mirroring the old design (`research.md`). Register a new `SectionId::SdfAtlas` at value 33 in `crates/level-format/src/lib.rs` and wire it through the section read/write paths. Add round-trip and empty-geometry tests. This is the format foundation both the bake and the runtime parse against.

### Task 2: SDF bake stage (level-compiler)

Add `crates/level-compiler/src/sdf_bake.rs`: compute per-voxel signed distances to nearest static geometry using the compiler's existing geometry/BVH (the same data the lightmap and SH bakers traverse), pack surface bricks into the atlas, fill the top-level index with empty/interior/surface slots, and compute coarse per-brick fallback distances. Gate the stage on a new CLI flag on `prl-build`. Make it a build-cache stage with its own version constant; keep accumulation order-stable for deterministic output. Log a coarse bake-stats line (brick counts, atlas size). Add a "wall-straddling brick" structural assertion.

### Task 2a: Unshadowed-irradiance lightmap bake (level-compiler)

On this branch, make the lightmap bake (`crates/level-compiler/src/lightmap_bake.rs`) skip the visibility term so the atlas holds full static-light irradiance and bounce with **no** baked shadows. The bake already separates a per-texel light contribution from a visibility test; the unshadowed mode treats every light as visible (no occlusion ray). Add a bake-mode flag (CLI-driven, in the cache key) that selects shadowed (current `main` behavior) vs. unshadowed. Record the mode in the lightmap section so the runtime knows whether to apply SDF visibility to the static term. Bump the lightmap stage version. Add a test: an occluded surface that is dark under the shadowed bake is lit under the unshadowed bake.

### Task 3: Runtime SDF resources (postretro/render)

Parse the SDF section at load and upload: distances to a 3D atlas texture, top-level to a storage buffer, coarse distances to a 3D texture, and a `SdfMeta`-equivalent uniform. Own these in a renderer-side resource struct with its own bind-group layout, used only by the shadow pass (Task 4) — not added to the forward bind groups. Absence of the section yields a "no atlas" state that disables the pass. Read the lightmap's shadowed/unshadowed mode flag (Task 2a) and expose it so the forward pass (Task 5) knows whether to multiply SDF visibility into the static term.

### Task 4: Half-resolution SDF shadow pass + shader (postretro)

Add a compute pass scheduled **after the depth pre-pass** (depth is the input) and before the forward pass. Per half-res pixel: reconstruct world position from the depth buffer via an inverse view-projection (thread the inverse matrix into the pass uniform from the camera the renderer already has), then for each shadow-casting light affecting the pixel — static and dynamic, point and spot — sphere-trace the SDF toward the light, accumulating occlusion with the closest-passing-distance penumbra estimate. Early-out open regions using the baked DDGI `E[d]` moment sampled at the pixel (skip the march when the region is open toward the light past a skip distance). Bind the light array in the pass's own layout. Write up to 4 lights' factors into one `Rgba8Unorm` half-res target. Expose the march-step cap, open-space skip threshold, and penumbra `k` as uniform fields (consumed by Task 7 sliders). Add a GPU-timing pair.

### Task 5: Forward shadow-factor integration (postretro)

In the forward fragment shader, multiply each shadowed light's direct contribution by the upsampled shadow factor — including the **static lightmap term**, gated on the unshadowed-mode flag from Task 2a/3 (when the lightmap is shadowed, e.g. a legacy PRL, skip multiplying the static term to avoid double shadows). Upsample inline with a depth-aware 2×2 bilateral filter reusing the `fog_composite.wgsl` pattern (no extra full-res target). Sample the shadow-factor buffer via a new binding in the shadows bind group (group 5); the shared group-5 BGL and every pipeline that uses it must be updated together with the new binding's `visibility`. Gate application on the `SdfShadowMode` uniform (off → no shadow factor applied).

### Task 6: Debug shadow-mode toggle (postretro)

Add an `SdfShadowMode` enum (SDF on / SDF off / SDF-visualize) mirroring the `LightingIsolation` pattern (`ALL_VARIANTS`, `label()`) in `render/mod.rs`; add a field to `FrameUniforms` and the forward `Uniforms` (the uniform layout must stay 16-byte aligned — current size is exactly full, so the constant grows). Add a dropdown in `debug_ui/mod.rs` `draw_diagnostics_panel()`, matching how `LightingIsolation` is panel-only (no keyboard chord — the prior `LightingIsolation` cycle chord was removed). SDF-visualize mode outputs the shadow factor (or march-step heatmap) instead of shaded color.

### Task 7: Debug quality sliders (postretro, dev-tools)

Add sliders to `draw_diagnostics_panel()` for live quality tuning, in a new collapsing section. SDF knobs (pure uniform scalars, per-frame write, no rebuild): max march steps, open-space skip threshold, penumbra/cone softness `k` — write through to the shadow-pass uniform fields from Task 4 via renderer setters. Fog knobs: `step_size` (uniform scalar — add a renderer setter that updates `FogParams.step_size` and re-uploads per frame; no setter exists today) and `fog_pixel_scale` (resolution knob — drive `Renderer::set_fog_pixel_scale`, which rebuilds the scatter target and bind group via `FogPass::set_pixel_scale`; this is a resource rebuild on change, not a per-frame uniform write). Seed slider state from live renderer values on first draw, matching the existing ambient-floor/indirect-scale pattern.

## Sequencing

**Phase 1 (sequential):** Task 1 — the section format blocks both the bake and the runtime parse.
**Phase 2 (concurrent):** Task 2 (SDF bake), Task 2a (unshadowed lightmap bake), and Task 3 (runtime resources) — independent once the format exists. Task 2a touches only `lightmap_bake.rs`; Task 2 and Task 3 share no files with it.
**Phase 3 (sequential):** Task 4 — consumes the runtime resources from Task 3.
**Phase 4 (sequential):** Task 5 — consumes the shadow-factor buffer from Task 4 and the unshadowed-mode flag from Tasks 2a/3.
**Phase 5 (concurrent):** Task 6 (mode toggle) and Task 7 (quality sliders) — both edit `debug_ui` + `render/mod.rs`; coordinate on those files but otherwise independent. Both consume the pass/uniforms from Tasks 4–5.

## Wire format

New PRL section, `SectionId::SdfAtlas = 33`. Little-endian throughout, mirroring the existing section conventions in `crates/level-format`. Pin during implementation: header field order (world min/max, voxel size, brick size, grid dims, atlas bricks-per-axis, surface brick count), then top-level index (`u32` per brick cell, with `EMPTY`/`INTERIOR` sentinels = `u32::MAX` / `u32::MAX-1`), then quantized `i16` atlas distances (unit = `voxel_size_m / 256`), then `f32` coarse per-brick distances. Empty-geometry encoding: zero grid dims and empty arrays, matching how `ShVolumeSection` signals an empty volume. Section is optional — absence is a valid lower-fidelity (no-SDF) load, not an error. Mirror the old `sdf_atlas.rs` layout (`research.md`) unless a field is demonstrably better changed.

**Lightmap unshadowed flag.** The lightmap section gains a one-value mode marker (shadowed / unshadowed). Constraint: extend the section without breaking the existing layout's parse for legacy PRLs (a missing marker reads as shadowed — `main`'s behavior). Pin the exact placement during implementation.

**`prl-build` CLI surface.** Two new flags: one to emit the SDF atlas section (Task 2), one to bake unshadowed irradiance (Task 2a). On the branch the campaign-test build sets both. Each flag is part of its stage's cache key, so flipping either invalidates only that stage. Default (no flags) reproduces `main`'s output byte-for-byte — the branch's build script opts in.

## Rough sketch

- **Branch comparison, not runtime A/B.** The correct static-SDF design needs unshadowed irradiance, which would double-count if a shadowed lightmap and SDF coexisted in one build. Building on a branch removes coexistence: the branch bakes unshadowed and applies SDF visibility everywhere; `main` stays as-is. Comparison is `git switch`, not a uniform flag. This is why static shadows are now in scope — see `research.md`.
- **Baked irradiance × runtime visibility.** Standard split: the lightmap supplies the static-light irradiance (the expensive bounce integral), SDF supplies the per-frame visibility scalar. Multiplying them at the forward stage reconstructs shadowed static lighting while letting visibility move with dynamic geometry later (room to grow).
- **Atlas vs. coarse split.** Surface bricks carry full-resolution quantized distances; the top-level index routes open/solid bricks to the coarse `f32` texture. The sphere tracer reads the fine atlas near surfaces and the coarse texture in open space — the established two-resolution pattern (`research.md`).
- **DDGI moment as accelerator.** `sh_corner_depth_visibility` already proves the moment texture (`sh_depth_moments`, group 3 binding 14) is sampleable at runtime. The shadow pass samples `E[d]` at the pixel's probe cell; if the open distance toward the light exceeds a cell-scaled skip threshold, the march is skipped (factor = 1). The cheap analogue of UE's coarse-region fallback.
- **Decoupling buys the bind budget.** Only one group slot (7) remains. The decoupled pass keeps the SDF atlas out of the forward bind groups — the atlas lives in the shadow pass's own pipeline layout; forward only gains a single screen-space shadow-factor texture binding in group 5. The last group slot stays free.
- **Quality sliders.** Two feasibility classes. **Uniform scalars** (live, per-frame write, no rebuild): SDF max march steps, open-space skip threshold, penumbra `k`; fog `step_size`. **Resolution/allocation** (slider-driven but triggers a render-target/pipeline rebuild on change, not a uniform write): `fog_pixel_scale` rebuilds the fog scatter target via `set_pixel_scale`. If half-res shadow scale is ever exposed it falls in this second class (shadow target recreate) — not in v1. The `ACCUM_ALPHA` fog knob from the §7.5 doc is absent in source, so it is not exposed; promoting a shader `const` to a uniform would be its prerequisite (`research.md`).

## Open questions

- **Light channel packing vs. accumulation.** v1 packs 4 per-light factors into `Rgba8Unorm`. Alternative: accumulate a single combined occlusion per pixel and lose per-light attribution (cheaper, but wrong when two shadowed lights overlap). Spec assumes per-light channels; confirm 4 is enough for test maps.
- **Static-light SDF-shadow cost.** Shadowing static lights via SDF every frame is strictly more runtime work than baked shadows. The comparison must weigh fidelity/flexibility against that cost; if static SDF shadows are too expensive, the fallback is "static lights stay baked-shadowed, SDF shadows dynamic lights only" — but that needs the shadowed bake, defeating the clean branch split. Flagged for the comparison.
- **Shadow pass: compute vs. fragment.** Compute is assumed (arbitrary half-res dispatch, easy world-pos reconstruction). A full-screen fragment pass is the fallback if the compute path complicates depth sampling.
- **Skip-threshold tuning.** The `E[d]` skip distance trades a few false "lit" pixels near grazing occluders for skipped marches. The Task 7 slider exists to set the cell-scaled multiple by eye; starts conservative.
