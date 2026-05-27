# SDF Static-Occluder Shadows

## Goal

Deliver a shadow blend Quake's static `.bsp`/lightmap architecture could never reach: baked indirect (SH/DDGI) + runtime SDF shadows from **static occluders** (soft, off-screen, point-light, not frozen into the bake) + shadow-map shadows from **dynamic occluders** (enemies and other moving meshes), all on a retro aesthetic and a 2020-MacBook-Pro-class GPU budget.

The split is by **occluder, not by light**:

- **Static geometry casts via SDF** — for static *and* dynamic lights, point *and* spot. A dynamic spot light throwing a static wall's shadow is an SDF query, not a shadow-map render. This buys the Quake-impossible wins: off-screen static occluders (monster closets, scripted reveals), point-light static shadows (the engine has none today), soft penumbra, and shadows that are no longer frozen into the bake.
- **Dynamic geometry casts via shadow maps** — enemies and moving meshes render into the existing 12-slot `SpotShadowPool`. With SDF owning static-occluder shadows, the shadow-map pass renders only *dynamic* meshes into its maps, not the whole static world — cheaper than today.

Built on a dedicated git branch and compared by switching branches: per-pass GPU time via `POSTRETRO_GPU_TIMING`, visuals side by side. The branch cleanly isolates the **static-occluder shadow technique** (`main` = shadows baked into the lightmap; branch = unshadowed lightmap × runtime SDF visibility). Dynamic/enemy shadows stay on shadow maps on **both** branches, so they are neither part of the A/B nor broken by it.

This is an **experimental** feature: a correct, minimal foundation with room to grow, not a complete shadow system.

### Key approximation

The unshadowed lightmap fuses all static lights into one irradiance value plus one dominant incoming direction per texel (`rendering_pipeline.md` §4). The SDF therefore traces **once** toward that dominant direction for the entire static-light term — **O(1) in static-light count per pixel**, not per-light. The hard, pixelated retro shadow aesthetic absorbs this approximation; reasoning in `research.md`. Per-light-exact static shadows (move static lights into the runtime loop) are an escape hatch only — they multiply trace cost per light and break the budget.

### Terminology

| Term | Meaning |
|---|---|
| Shadowed lightmap | `main`'s bake: static-light shadows folded into irradiance (occluded texels darkened). |
| Unshadowed lightmap | Branch bake: full static irradiance/bounce, **no** visibility term. Task 2a. |
| Static-occluder shadow | A shadow cast by baked world geometry. SDF owns these. |
| Dynamic-occluder shadow | A shadow cast by a moving mesh (enemy). Shadow maps own these. |
| Shadow factor | `[0,1]` visibility scalar the SDF pass writes; 1 = lit, 0 = occluded. |
| `SdfShadowMode` | Debug selector: SDF on / off / visualize. |

### Data flow

`unshadowed lightmap irradiance` (baked) → forward. `SDF atlas` (baked, static geometry only) → half-res shadow pass → `shadow-factor target` → bilateral upsample in forward → multiplies the static-light term (one dominant-direction trace) and each dynamic-light's-vs-static-occluder term. DDGI `E[d]` moment feeds the pass's open-space skip. Enemy shadow-map results are untouched by the SDF factor. Legacy PRL (shadowed lightmap, no SDF section): forward skips the static-term multiply and the pass is disabled — degrades to `main`-equivalent lighting.

## Scope

### In scope

- A baked **SDF atlas** PRL section: sparse brick grid of quantized `i16` distances + top-level index + coarse per-brick `f32` fallback, computed from **static world geometry** at compile time. Revives the old brick/coarse design (`research.md`).
- A `prl-build` bake stage producing the section, gated on a CLI flag, deterministic, integrated with the build cache.
- **Unshadowed-irradiance lightmap bake** (branch-only behavior): the static lightmap bakes full static-light irradiance and bounce with the **visibility term removed**, so runtime SDF supplies visibility without double-counting. The section records that its irradiance is unshadowed.
- Runtime SDF resources: upload atlas to 3D textures + top-level storage buffer + meta uniform, owned by the renderer, bound only in the shadow pass's own layout.
- A new **half-resolution SDF shadow pass** (compute) after the depth pre-pass: reconstruct world position from depth, sphere-trace the SDF, and write a packed shadow factor to a half-res target. One trace toward the **dominant baked direction** covers the whole static-light term; each dynamic light also traces toward its own direction (against static occluders). Uses the baked DDGI `E[d]` moment as an open-space skip and the closest-passing-distance estimate for soft penumbra.
- Forward integration: depth-aware bilateral upsample of the shadow-factor buffer, multiply the **static-light term** (gated on the unshadowed-mode flag) and each **dynamic light's** direct contribution by its factor. Enemy shadow-map results are **not** multiplied by it.
- Point-light static-occluder shadows via SDF (new capability — point lights cast no dynamic shadow today).
- A debug **`SdfShadowMode`** selector (SDF on / off / visualize): panel dropdown + frame-uniform field. "Off" applies no SDF factor (shadow maps still run); "visualize" renders the shadow factor for artifact-spotting.
- Debug-panel **quality sliders** (dev-tools) for live-tuning SDF and fog quality — per-knob feasibility split in *Quality sliders*.

### Out of scope (non-goals)

- **Indirect / SH visibility via SDF.** Leak suppression stays on the DDGI depth-moment path. The shadow pass never traces the SDF for indirect lighting.
- **Removing or bypassing shadow maps.** The 12-slot `SpotShadowPool` stays — it now renders only dynamic meshes (enemies, M10). Enemy shadows come from shadow maps, never SDF, in v1.
- **Dynamic occluders in the SDF** (capsule/mesh insertion for enemies). The baked field is static geometry only; enemy shadows are the shadow-map path's job. Room to grow.
- **Per-light-exact static shadows.** v1 fuses static lights into one dominant-direction trace. The per-light alternative is the named escape hatch only.
- **Merging the branch to `main`.** This draft delivers the comparison branch. Promotion is a later decision driven by the comparison.
- **More than 4 simultaneously SDF-shadowed terms.** v1 packs factors into one `Rgba8Unorm` half-res target. Channel budget is an open question (below). Excess terms render unshadowed. Room to grow (texture array).
- **Temporal accumulation of the shadow buffer.** Single-frame half-res + bilateral upsample only.
- **Fog knobs beyond `step_size` and `fog_pixel_scale`.** The §7.5 `ACCUM_ALPHA` temporal-resolve knob does not exist in current source — not exposed (`research.md`).

## Acceptance criteria

Automated (test- or tooling-gated):

- [ ] The SDF atlas section round-trips byte-identically through serialize → deserialize, including the empty-geometry case (empty section, no panic). [T1]
- [ ] Re-running the SDF bake on identical input produces byte-identical section bytes. [T2]
- [ ] Bumping the SDF bake stage version invalidates the prior cache entry: first build after the bump is a miss and rebakes; the second is a hit. [T2]
- [ ] For a known map, a brick straddling a wall stores a near-zero distance at the surface and growing distances away; an all-open brick is marked empty/interior in the top-level index. Distinguishable in the baked data. [T2]
- [ ] The unshadowed bake omits the visibility term: for a map with a static light fully occluded from a surface, the lightmap texel is **non-zero** (lit), whereas the shadowed bake leaves it dark. A bake-mode flag selects the behavior; the section records which mode produced it. [T2a]
- [ ] With neither new `prl-build` flag set, the compiled output is byte-identical to `main`'s for the same map. [T2, T2a]
- [ ] An old `.prl` without the SDF section loads without error; the renderer reports "no SDF atlas" and the shadow pass is skipped (degradation path, not a failure). [T3]
- [ ] With SDF mode active and `POSTRETRO_GPU_TIMING=1`, a per-pass timing line for the SDF shadow pass is logged alongside the existing passes. [T4]
- [ ] The shadow-mode selector cycles through all modes via the debug-UI dropdown; the selected mode round-trips into the frame uniform. [T6]
- [ ] The SDF quality sliders (max march steps, open-space skip threshold, penumbra `k`) write through to the shadow-pass uniform; the fog `step_size` slider writes through to `FogParams` per frame, and the `fog_pixel_scale` slider triggers a scatter-target rebuild rather than a per-frame uniform write. [T7]

Manual / visual (observed by a human running the engine — not machine-verified):

- [ ] In SDF mode, a dynamic point light near a **static wall** casts a visible soft shadow (a capability absent on `main`). [T4, T5]
- [ ] A static light casts a static-occluder shadow via SDF, matching the look of `main`'s baked-lightmap shadow for the same scene (branch-vs-branch). [T2a, T4, T5]
- [ ] An off-screen static occluder shadows a visible surface (Quake-impossible win). [T4]
- [ ] Toggling `SdfShadowMode` off leaves the static + dynamic-light terms fully lit with no SDF factor; shadow-map (enemy) shadows are unaffected. [T6, T5]
- [ ] SDF edges soften with distance from the occluder. [T4]
- [ ] SDF-visualize mode renders an interpretable shadow-factor (or march-step) view usable to spot artifacts. [T6]
- [ ] No light-leak regression in indirect lighting (indirect is independent of this feature). [T5]
- [ ] Dragging the SDF and fog quality sliders visibly changes shadow/fog quality and per-pass GPU time without artifacts or crashes; the `fog_pixel_scale` slider re-blocks/un-blocks fog resolution live. [T7]

## Tasks

### Task 1: SDF atlas PRL section (level-format)

Revive `crates/level-format/src/sdf_atlas.rs`: the section struct, brick/top-level/coarse layout, slot sentinels, and serialize/deserialize mirroring the old design (`research.md`). Register `SectionId::SdfAtlas = 33` in `crates/level-format/src/lib.rs` and wire the section read/write paths. Add round-trip and empty-geometry tests. Format foundation for both the bake and the runtime parse.

### Task 2: SDF bake stage (level-compiler)

Add `crates/level-compiler/src/sdf_bake.rs`: compute per-voxel signed distances to nearest **static** geometry using the compiler's existing geometry/BVH (the same data the lightmap and SH bakers traverse), pack surface bricks, fill the top-level index with empty/interior/surface slots, compute coarse per-brick fallback distances. Gate on a new `prl-build` CLI flag. Make it a build-cache stage with its own version constant; keep accumulation order-stable for deterministic output. Log a coarse bake-stats line. Add a wall-straddling-brick structural assertion.

### Task 2a: Unshadowed-irradiance lightmap bake (level-compiler)

On this branch, make the lightmap bake (`crates/level-compiler/src/lightmap_bake.rs`) skip the visibility gate so the atlas holds full static-light irradiance and bounce with **no** baked shadows. The bake already separates per-texel light contribution from the visibility test; unshadowed mode treats every light as visible. Add a bake-mode flag (CLI-driven, in the cache key) selecting shadowed (`main`) vs. unshadowed. Record the mode in the lightmap section. Bump the lightmap stage version. Add a test: an occluded surface dark under the shadowed bake is lit under the unshadowed bake.

### Task 3: Runtime SDF resources (postretro/render)

Parse the SDF section at load and upload: distances to a 3D atlas texture, top-level to a storage buffer, coarse distances to a 3D texture, and a meta uniform. Own these in a renderer-side struct with its own bind-group layout, used only by the shadow pass (Task 4) — not added to forward bind groups. Section absence yields a "no atlas" state that disables the pass. Read the lightmap's shadowed/unshadowed mode flag (Task 2a) and expose it so the forward pass (Task 5) knows whether to multiply SDF visibility into the static term.

### Task 4: Half-resolution SDF shadow pass + shader (postretro)

Add a compute pass scheduled **after the depth pre-pass** and before the forward pass. Per half-res pixel: reconstruct world position from depth via an inverse view-projection (threaded into the pass uniform from the camera the renderer already has). Trace **once toward the baked dominant light direction** sampled from the lightmap to produce the aggregate static-light shadow factor. For each shadow-casting **dynamic** light affecting the pixel (point and spot), trace toward that light against the static SDF for its static-occluder factor. Accumulate occlusion with the closest-passing-distance penumbra estimate. Early-out open regions via the baked DDGI `E[d]` moment sampled at the pixel. Bind the light array in the pass's own layout. Write factors into one `Rgba8Unorm` half-res target (channel budget — open question). Expose march-step cap, open-space skip threshold, and penumbra `k` as uniform fields (Task 7). Add a GPU-timing pair.

### Task 5: Forward shadow-factor integration (postretro)

In the forward fragment shader, multiply by the upsampled shadow factor: the **static lightmap term** (gated on the unshadowed-mode flag from Task 2a/3 — skip the multiply for a shadowed/legacy lightmap to avoid double shadows) and each **dynamic light's** direct contribution (its static-occluder factor). Do **not** apply the factor to shadow-map (enemy) results — those already carry their dynamic-occluder shadow. Upsample inline with a depth-aware 2×2 bilateral filter (no extra full-res target). The `fog_composite.wgsl` bilateral upsample was reverted in `f50314d` for perf — **re-derive** the filter, don't reuse it (`research.md`). Sample the shadow-factor buffer via a new binding in the shadows bind group (group 5); update the shared group-5 BGL and every pipeline using it together with the new binding's `visibility`. Gate application on the `SdfShadowMode` uniform (off → no factor applied).

### Task 6: Debug shadow-mode toggle (postretro)

Add an `SdfShadowMode` enum (SDF on / off / visualize) mirroring the `LightingIsolation` pattern (`ALL_VARIANTS`, `label()`) in `render/mod.rs`; add a field to `FrameUniforms` and the forward `Uniforms` (keep the uniform 16-byte aligned). Add a dropdown in `debug_ui/mod.rs` `draw_diagnostics_panel()`, panel-only (no keyboard chord), matching `LightingIsolation`. Visualize mode outputs the shadow factor (or march-step heatmap) instead of shaded color.

### Task 7: Debug quality sliders (postretro, dev-tools)

Add sliders to `draw_diagnostics_panel()` in a new collapsing section. SDF knobs (pure uniform scalars, per-frame write, no rebuild): max march steps, open-space skip threshold, penumbra/cone softness `k` — write through to the shadow-pass uniform via renderer setters. Fog knobs: `step_size` (uniform scalar — add a renderer setter that updates `FogParams.step_size` and re-uploads per frame; none exists today) and `fog_pixel_scale` (resolution knob — drive `Renderer::set_fog_pixel_scale`, which rebuilds the scatter target and bind group; a resource rebuild on change, not a per-frame uniform write). Seed slider state from live renderer values on first draw, matching the ambient-floor/indirect-scale pattern.

## Sequencing

**Phase 1 (sequential):** Task 1 — the section format blocks both the bake and the runtime parse.
**Phase 2 (concurrent):** Task 2, Task 2a, Task 3 — independent once the format exists. Task 2a touches only `lightmap_bake.rs`; Task 2 and Task 3 share no files with it.
**Phase 3 (sequential):** Task 4 — consumes the runtime resources from Task 3.
**Phase 4 (sequential):** Task 5 — consumes the shadow-factor buffer from Task 4 and the unshadowed-mode flag from Tasks 2a/3.
**Phase 5 (concurrent):** Task 6 and Task 7 — both edit `debug_ui` + `render/mod.rs`; coordinate on those files but otherwise independent. Both consume the pass/uniforms from Tasks 4–5.

## Wire format

New PRL section, `SectionId::SdfAtlas = 33`. Little-endian throughout, mirroring existing section conventions in `crates/level-format`. Pin during implementation: header field order (world min/max, voxel size, brick size, grid dims, atlas bricks-per-axis, surface brick count), top-level index (`u32` per brick cell, `EMPTY`/`INTERIOR` sentinels = `u32::MAX` / `u32::MAX-1`), quantized `i16` atlas distances (unit = `voxel_size_m / 256`), `f32` coarse per-brick distances. Empty-geometry encoding: zero grid dims and empty arrays, matching how `ShVolumeSection` signals an empty volume. Section is optional — absence is a valid no-SDF load, not an error. Mirror the old `sdf_atlas.rs` layout (`research.md`) unless a field is demonstrably better changed.

**Lightmap unshadowed flag.** The lightmap section gains a one-value mode marker (shadowed / unshadowed). Constraint: extend the section without breaking the existing layout's parse for legacy PRLs (a missing marker reads as shadowed — `main`'s behavior). Pin exact placement during implementation.

**`prl-build` CLI surface.** Two new flags: one to emit the SDF atlas section (Task 2), one to bake unshadowed irradiance (Task 2a). On the branch the campaign-test build sets both. Each flag is part of its stage's cache key, so flipping either invalidates only that stage. Default (no flags) reproduces `main`'s output byte-for-byte.

## Rough sketch

- **Occluder-based split.** SDF owns static-occluder shadows; shadow maps own dynamic-occluder (enemy) shadows. This is how UE splits distance-field vs. shadow-map shadowing (`research.md`). The branch A/B isolates only the static-occluder technique.
- **O(1) static term.** The unshadowed lightmap already fuses all static lights into one irradiance + one dominant direction. One SDF trace toward that direction shadows the entire static-light term, regardless of static-light count.
- **Baked irradiance × runtime visibility.** Standard split (DDGI/RTXGI/Lumen): the lightmap supplies the static irradiance integral, SDF supplies the per-frame visibility scalar. Multiplying at forward reconstructs shadowed static lighting while letting visibility move with dynamic lights.
- **Atlas vs. coarse split.** Surface bricks carry fine quantized distances; the top-level index routes open/solid bricks to the coarse `f32` texture. The tracer reads fine near surfaces, coarse in open space — the established two-resolution pattern (`research.md`).
- **DDGI moment as accelerator.** The shadow pass samples `E[d]` (`sh_depth_moments`, group 3 binding 14) at the pixel's probe cell; if the open distance toward the light exceeds a cell-scaled skip threshold, the march is skipped (factor = 1). The cheap analogue of UE's coarse-region fallback.
- **Cheaper shadow maps.** Because SDF handles static-occluder shadows, the spot-shadow depth pass need render only dynamic meshes (enemies), not the whole static world — net budget freed for M10.
- **Decoupling buys the bind budget.** Only group slot 7 is free. The SDF atlas lives in the shadow pass's own pipeline layout; forward gains only one shadow-factor texture binding in group 5. The last group slot stays free.
- **Quality sliders.** Two feasibility classes. **Uniform scalars** (live, no rebuild): SDF march steps, skip threshold, penumbra `k`; fog `step_size`. **Resolution/allocation** (rebuild on change): `fog_pixel_scale` via `set_pixel_scale`. Half-res shadow scale, if ever exposed, is the second class — not in v1.

## Open questions

- **Shadow-factor channel budget.** v1 packs the one static-aggregate factor plus up to N dynamic-light static-occluder factors into one `Rgba8Unorm` (4 channels). If the static aggregate takes one channel, only 3 dynamic lights fit; if dynamic lights are common on test maps, that may be tight. Resolve at implementation: confirm 4 channels suffice for test maps, else widen to a texture array (room to grow). The alternative — accumulate one combined occlusion per pixel — loses per-light attribution and is wrong when two terms overlap.
- **Static-occluder SDF cost.** Tracing the static term every frame is strictly more runtime work than baked-into-lightmap shadows (free at runtime). The dominant-direction O(1) fusion keeps it to one trace, but the comparison must weigh fidelity/flexibility against the cost; if too expensive, the fallback (static lights stay baked-shadowed, SDF shadows only dynamic-light-vs-static-occluder) needs the shadowed bake and loses the clean branch split. Flagged for the comparison.
- **Shadow pass: compute vs. fragment.** Compute is assumed (arbitrary half-res dispatch, easy world-pos reconstruction). A full-screen fragment pass is the fallback if compute complicates depth sampling.
- **Skip-threshold tuning.** The `E[d]` skip distance trades a few false "lit" pixels near grazing occluders for skipped marches. The Task 7 slider sets the cell-scaled multiple by eye; starts conservative.
