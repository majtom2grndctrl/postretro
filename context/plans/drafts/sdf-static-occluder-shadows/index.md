# SDF Static-Occluder Shadows

## Goal

Deliver a shadow blend Quake's static `.bsp`/lightmap architecture could never reach: baked indirect (SH/DDGI) + runtime SDF shadows from **static occluders** (soft, off-screen, point-light, not frozen into the bake) + shadow-map shadows from **dynamic occluders** (enemies and other moving meshes), all on a retro aesthetic and a 2020-MacBook-Pro-class GPU budget.

**Every light casts static-occluder shadows via the SDF** — through one of three trace classes, and the partition is what keeps the pass cheap. The classification is on the **geometry axis**: a light is *dynamic* only when its position/aim **moves**; intensity-only animation (brightness/color pulse) is a **static** light with animated intensity.

- **Static-lightmap aggregate term** — all baked static lights fuse into one irradiance + one dominant incoming direction per texel; one SDF trace toward that direction shadows the whole static-light term. **O(1) in static-light count.**
- **Animated-baked aggregate term** — lights whose *geometry* is fixed but whose *intensity* animates (brightness/color pulses, including script-driven sweeps) live on the animated-lightmap bake. The compose pass already loops per-texel over contributing lights to sum radiance; it also fuses a dominant direction — each light's direction weighted by its current radiance, normalized, written to a per-frame direction atlas. One SDF trace shadows the whole animated-baked term toward that fused direction, so the shadow tracks the sweep frame to frame. **Also O(1)** — one trace per texel, the added fusion work living in the per-frame compose pass at atlas resolution, not per screen pixel. This is the perf lever (see below).
- **Geometry-moving lights** — lights whose *geometry* moves (position/aim) trace the static SDF once each toward their own direction, on the per-pixel runtime loop. **No light in the engine moves yet**, so this bucket is empty in v1. It stays named in the architecture as the seam where direction-animated lights plug in later; v1 implements only the two aggregate traces.

So v1 SDF cost per pixel is `static-aggregate + animated-baked-aggregate` — two O(1) traces, not one trace per light affecting the pixel. The geometry-moving term is built when the first moving light lands.

**Dynamic-occluder (enemy) shadows stay on shadow maps**, and are now **opt-in per light** via an authoring flag — enemies render into the existing 12-slot `SpotShadowPool`. With SDF owning static-occluder shadows, the shadow-map pass renders only *dynamic* meshes into its maps, not the whole static world — cheaper than today. Enemy shadow-map results are never multiplied by the SDF factor.

**Definition of "dynamic" — the geometry axis.** A light is *dynamic* when its **geometry moves** (position/aim), **not** when its **intensity pulses** (brightness/color). Intensity-only animation is a static light with animated intensity, on a baked path. The old code conflated the two and forced every scripted-brightness light onto the fully-dynamic runtime path. Every current `_dynamic` light is therefore misclassified: none physically moves. Correcting it re-tags all of them static and empties the geometry-moving bucket, which dissolves the channel budget (`research.md`).

Built on a dedicated git branch and compared by switching branches: per-pass GPU time via `POSTRETRO_GPU_TIMING`, visuals side by side. The branch cleanly isolates the **static-occluder shadow technique** (`main` = shadows baked into the lightmap; branch = unshadowed lightmap × runtime SDF visibility). Dynamic/enemy shadows stay on shadow maps on **both** branches, so they are neither part of the A/B nor broken by it.

This is an **experimental** feature: a correct, minimal foundation with room to grow, not a complete shadow system.

### Key approximation

The unshadowed lightmap fuses all static lights into one irradiance value plus one dominant incoming direction per texel (`rendering_pipeline.md` §4). The SDF therefore traces **once** toward that dominant direction for the entire static-light term — **O(1) in static-light count per pixel**, not per-light. This single mean-direction shadow for overlapping static lights is the stated approximation: a texel lit by two static lights from different directions gets one shadow toward their luminance-weighted mean. The hard, pixelated retro shadow aesthetic absorbs it (`research.md`).

The animated-baked term is **not** an approximation of direction. The compose pass fuses a *per-frame* dominant direction from the lights' current radiances and the SDF traces toward it, so the shadow tracks the brightness sweep — still O(1), one trace per texel. The chunky retro look licenses **coarseness** (low resolution, hard edges) but not **incorrectness** (a frozen shadow pointing the wrong way while the lights sweep). A static baked direction would be the latter; the per-frame fusion avoids it at no extra trace cost.

These two aggregate traces are the whole v1 cost — the geometry-moving bucket is empty, so no per-light trace runs. Per-light-exact static shadows (move static lights into the runtime loop) are an escape hatch only — they multiply trace cost per light and break the budget.

### Terminology

| Term | Meaning |
|---|---|
| Shadowed lightmap | `main`'s bake: static-light shadows folded into irradiance (occluded texels darkened). |
| Unshadowed lightmap | Branch bake: full static irradiance/bounce, **no** visibility term. Task 2a. |
| Static-occluder shadow | A shadow cast by baked world geometry. SDF owns these — for *all* lights. |
| Dynamic-occluder shadow | A shadow cast by a moving mesh (enemy). Shadow maps own these; opt-in per light. |
| Animated-baked light | A light with **static geometry** but **animated intensity** (brightness/color pulse, incl. script-driven). Lives on the animated-lightmap bake; its static-occluder shadow folds into one aggregate trace toward a per-frame fused direction. |
| Geometry-moving light | A light whose **position/aim moves** — the only true "dynamic" light. Per-pixel runtime trace. **Empty in v1** (no light moves yet); the named future seam. |
| Shadow factor | `[0,1]` visibility scalar the SDF pass writes; 1 = lit, 0 = occluded. |
| `casts_dynamic_shadows` | Per-light authoring flag (FGD `_dynamic_shadows`) gating shadow-map-pool eligibility. |
| `SdfShadowMode` | Debug selector: SDF on / off / visualize. |

### Data flow

`unshadowed lightmap irradiance` + per-texel static dominant direction (baked) → forward. `animated-baked lightmap irradiance` + per-texel **per-frame** dominant direction — both fused in the compose pass each frame, the direction written to a direction atlas → forward + shadow pass. `SDF atlas` (baked, static geometry only) → half-res shadow pass → `shadow-factor target` → bilateral upsample in forward → multiplies the static-lightmap term (one dominant-direction trace) and the animated-baked term (one per-frame-direction trace). DDGI `E[d]` moment feeds the pass's open-space skip. Enemy shadow-map results are untouched by the SDF factor. The geometry-moving term is a documented seam — no consumer in v1. Legacy PRL (shadowed lightmap, no SDF section): forward skips the static-term multiply and the pass is disabled — degrades to `main`-equivalent lighting.

## Scope

### In scope

- A baked **SDF atlas** PRL section: sparse brick grid of quantized `i16` distances + top-level index + coarse per-brick `f32` fallback, computed from **static world geometry** at compile time. Revives the old brick/coarse design (`research.md`).
- A `prl-build` bake stage producing the section, gated on a CLI flag, deterministic, integrated with the build cache.
- **Unshadowed-irradiance lightmap bake** (branch-only behavior): the static lightmap bakes full static-light irradiance and bounce with the **visibility term removed**, so runtime SDF supplies visibility without double-counting. The section records that its irradiance is unshadowed.
- **Light model:** a per-light `casts_dynamic_shadows` authoring flag (FGD `_dynamic_shadows`) gating shadow-map-pool eligibility; a redefined `is_dynamic` so intensity-only animation no longer forces the runtime-dynamic path (geometry-moving lights only); updated `rank_lights` eligibility. (Task 1b.)
- **Animated-baked dominant direction:** a per-texel **per-frame** dominant incoming direction fused in the compose pass — each contributing light's direction weighted by its current radiance, normalized, written to a per-frame direction atlas alongside the irradiance output. The SDF traces toward it, so the animated-baked aggregate shadow tracks the brightness sweep at no extra trace cost. (Task 2b.)
- **Scripting→animated-baked bridge:** a declarative light property (e.g. `_animated`) meaning "static geometry, intensity arrives at runtime; reserve a baked weight map." The compiler bakes a weight map for the light; at runtime the script-driven brightness/color curve routes into the animated-compose descriptor (format already shared with `scripted_light_descriptors`). No compile-time auto-detection magic — modder-friendly favors an explicit flag. Includes re-tagging **all** current `_dynamic` campaign-test lights to static: the arena sweep lights onto this animated-baked path, fixed-intensity lights to pure-baked. (Task 2c.)
- Runtime SDF resources: upload atlas to 3D textures + top-level storage buffer + meta uniform, owned by the renderer, bound only in the shadow pass's own layout.
- A new **half-resolution SDF shadow pass** (compute) after the depth pre-pass: reconstruct world position from depth, sphere-trace the SDF, and write a packed shadow factor to a half-res target. One trace toward the static-lightmap **dominant baked direction** covers the whole static-light term; one trace toward the animated-atlas **per-frame dominant direction** covers the whole animated-baked term. Uses the baked DDGI `E[d]` moment as an open-space skip and the closest-passing-distance estimate for soft penumbra. The geometry-moving per-light trace is a named seam — coded when the first moving light lands, not in v1.
- Forward integration: depth-aware bilateral upsample of the shadow-factor buffer, multiply the **static-lightmap term** and the **animated-baked term** by their factors. Enemy shadow-map results are **not** multiplied by it.
- Point-light static-occluder shadows via SDF (new capability — point lights cast no dynamic shadow today). Delivered through the aggregate, for a **static** point light: a stationary point light casting a static-occluder shadow, which the engine cannot do today.
- A debug **`SdfShadowMode`** selector (SDF on / off / visualize): panel dropdown + frame-uniform field. "Off" applies no SDF factor (shadow maps still run); "visualize" renders the shadow factor for artifact-spotting.
- Debug-panel **quality sliders** (dev-tools) for live-tuning SDF and fog quality — per-knob feasibility split in *Quality sliders*.

### Out of scope (non-goals)

- **Indirect / SH visibility via SDF.** Leak suppression stays on the DDGI depth-moment path. The shadow pass never traces the SDF for indirect lighting.
- **Removing or bypassing shadow maps.** The 12-slot `SpotShadowPool` stays — it now renders only dynamic meshes (enemies, M10). Enemy shadows come from shadow maps, never SDF, in v1.
- **Dynamic occluders in the SDF** (capsule/mesh insertion for enemies). The baked field is static geometry only; enemy shadows are the shadow-map path's job, now opt-in per light. Room to grow.
- **Per-light-exact static shadows.** v1 fuses static lights (and, separately, animated-baked lights) into one dominant-direction trace each. The per-light alternative is the named escape hatch only.
- **The geometry-moving per-light trace.** No light moves yet, so this bucket is empty. The trace class stays named in the architecture (the seam for direction-animated lights) but is not implemented in v1 — it ships with the first moving light.
- **Merging the branch to `main`.** This draft delivers the comparison branch. Promotion is a later decision driven by the comparison.
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
- [ ] A light authored with `_dynamic_shadows 0` is excluded from `rank_lights` pool eligibility; one with `_dynamic_shadows` absent or `1` is eligible — preserving today's behavior (dynamic spots cast). Asserted on parsed `MapLight`. [T1b]
- [ ] `is_dynamic` is set by **geometry motion**, not intensity animation: a light that only pulses brightness (no position/aim animation) parses with `is_dynamic == false`. [T1b]
- [ ] The compose pass writes a per-frame direction atlas: for a texel where two animated-baked lights pulse out of phase, the written dominant direction at the peak of one light differs measurably from its value at the peak of the other (the direction tracks the radiance-weighted sweep, not a frozen value). Asserted on the compose output. [T2b]
- [ ] No campaign-test light parses with `is_dynamic == true` after reclassification: every previously-`_dynamic` light is now static (pure-baked or animated-baked). Asserted on parsed PRL. [T2c]
- [ ] For the reclassified arena sweep lights, the compiled output carries an animated-baked weight map (not a `_dynamic` runtime entry), and the runtime intensity curve still drives a phased brightness sweep equivalent to the script's. Asserted on parsed PRL + a runtime curve-eval test. [T2c]
- [ ] An old `.prl` without the SDF section loads without error; the renderer reports "no SDF atlas" and the shadow pass is skipped (degradation path, not a failure). [T3]
- [ ] With SDF mode active and `POSTRETRO_GPU_TIMING=1`, a per-pass timing line for the SDF shadow pass is logged alongside the existing passes. [T4]
- [ ] The shadow-mode selector cycles through all modes via the debug-UI dropdown; the selected mode round-trips into the frame uniform. [T6]
- [ ] The SDF quality sliders (max march steps, open-space skip threshold, penumbra `k`) write through to the shadow-pass uniform; the fog `step_size` slider writes through to `FogParams` per frame, and the `fog_pixel_scale` slider triggers a scatter-target rebuild rather than a per-frame uniform write. [T7]

Manual / visual (observed by a human running the engine — not machine-verified):

- [ ] In SDF mode, a **stationary** point light near a **static wall** casts a visible soft static-occluder shadow (a capability absent on `main`, which has no point-light shadows). [T4, T5]
- [ ] A static light casts a static-occluder shadow via SDF, matching the look of `main`'s baked-lightmap shadow for the same scene (branch-vs-branch). [T2a, T4, T5]
- [ ] In the arena, the reclassified animated-baked lights cast static-occluder shadows via the single animated-baked aggregate trace, and the shadow **moves with** the brightness sweep — as the phased sweep shifts which lights are brightest, the shadow swings to track the dominant light rather than sticking in one frozen direction. [T2b, T2c, T4, T5]
- [ ] An off-screen static occluder shadows a visible surface (Quake-impossible win). [T4]
- [ ] Toggling `SdfShadowMode` off leaves the static-lightmap and animated-baked terms fully lit with no SDF factor; shadow-map (enemy) shadows are unaffected. [T6, T5]
- [ ] SDF edges soften with distance from the occluder. [T4]
- [ ] SDF-visualize mode renders an interpretable shadow-factor (or march-step) view usable to spot artifacts. [T6]
- [ ] No light-leak regression in indirect lighting (indirect is independent of this feature). [T5]
- [ ] Dragging the SDF and fog quality sliders visibly changes shadow/fog quality and per-pass GPU time without artifacts or crashes; the `fog_pixel_scale` slider re-blocks/un-blocks fog resolution live. [T7]

### Task ↔ AC cross-check

Every task maps to ≥1 AC; every AC tags its task(s). The other direction:

| Task | Covering ACs |
|---|---|
| T1 | round-trip + empty-geometry |
| T1b | `_dynamic_shadows` pool gating; `is_dynamic` = geometry-motion |
| T2 | re-bake determinism; version-bump cache; wall-straddling brick; no-flag byte-identity; GPU-timing line (via T4) |
| T2a | unshadowed-bake lit-occluded; no-flag byte-identity; static-light-matches-`main` (visual) |
| T2b | compose-written per-frame direction tracks the sweep; arena animated-baked shadow moves with the sweep (visual) |
| T2c | all-lights-static reclassification; arena weight-map + script-curve equivalence; arena animated-baked shadow (visual) |
| T3 | no-SDF-section degrade load |
| T4 | GPU-timing line; stationary-point-light/static-wall, off-screen-occluder, edge-softening, animated-baked (visual) |
| T5 | static/animated-baked terms multiply; off-toggle leaves terms lit; no leak regression (visual) |
| T6 | mode-selector round-trip; off-toggle, visualize-mode (visual) |
| T7 | sliders write through; slider drag changes quality (visual) |

## Tasks

### Task 1: SDF atlas PRL section (level-format)

Revive `crates/level-format/src/sdf_atlas.rs`: the section struct, brick/top-level/coarse layout, slot sentinels, and serialize/deserialize mirroring the old design (`research.md`). Register `SectionId::SdfAtlas = 33` in `crates/level-format/src/lib.rs` and wire the section read/write paths. Add round-trip and empty-geometry tests. Format foundation for both the bake and the runtime parse.

### Task 1b: Light model — geometry-vs-intensity split + dynamic-shadow opt-in (level-compiler/runtime)

Correct the misclassification that forces intensity-only lights onto the runtime-dynamic path (`research.md`). Two changes, threaded through the same data path:

- **Redefine `is_dynamic`** so it is set by **geometry motion** (position/aim animation), not by intensity (brightness/color) animation. Today `is_dynamic` is parsed from FGD `_dynamic` (`crates/level-compiler/src/format/quake_map.rs` ~:245) and the runtime forward loop iterates only `is_dynamic` lights (`filter_dynamic_lights`, `crates/postretro/src/render/mod.rs:3262`). Intensity-only lights move *off* this path (onto the animated-baked path, Task 2c).
- **Add a per-light `casts_dynamic_shadows` flag** (FGD `_dynamic_shadows`) gating shadow-map-pool eligibility. `cast_shadows` currently defaults hardcoded `true` for all lights (`quake_map.rs:407`); the new flag is the *dynamic-occluder* opt-in. Default preserves today's behavior (dynamic spots cast). Update `rank_lights` in `crates/postretro/src/lighting/spot_shadow.rs` (currently gates on `is_dynamic && light_type == Spot`) to gate on the new flag.

Touches `map_data`/`quake_map.rs`/parse → `prl.rs` `MapLight` → `spot_shadow.rs`. Add parse-level assertions for both behaviors.

### Task 2: SDF bake stage (level-compiler)

Add `crates/level-compiler/src/sdf_bake.rs`: compute per-voxel signed distances to nearest **static** geometry using the compiler's existing geometry/BVH (the same data the lightmap and SH bakers traverse), pack surface bricks, fill the top-level index with empty/interior/surface slots, compute coarse per-brick fallback distances. Gate on a new `prl-build` CLI flag. Make it a build-cache stage with its own version constant; keep accumulation order-stable for deterministic output. Log a coarse bake-stats line. Add a wall-straddling-brick structural assertion.

### Task 2a: Unshadowed-irradiance lightmap bake (level-compiler)

On this branch, make the lightmap bake (`crates/level-compiler/src/lightmap_bake.rs`) skip the visibility gate so the atlas holds full static-light irradiance and bounce with **no** baked shadows. The bake already separates per-texel light contribution from the visibility test; unshadowed mode treats every light as visible. Add a bake-mode flag (CLI-driven, in the cache key) selecting shadowed (`main`) vs. unshadowed. Record the mode in the lightmap section. Bump the lightmap stage version. Add a test: an occluded surface dark under the shadowed bake is lit under the unshadowed bake.

### Task 2b: Animated-baked dominant direction (postretro + level-compiler)

The animated-lightmap atlas stores **irradiance only** — `textureStore(animated_lm_atlas, ..., vec4<f32>(accum, 1.0))` at `crates/postretro/src/shaders/animated_lightmap_compose.wgsl:153`, atlas `Rgba16Float` (binding 6). It carries **no** dominant direction, so the SDF has nothing to trace toward for the animated-baked aggregate term. This is the one real blocker (`research.md`).

Fuse a per-texel dominant direction **in the compose pass, per frame.** The pass already loops per-texel over contributing lights computing `accum += c*b*weight`. Have it also accumulate each light's incoming direction weighted by its current radiance, normalize at the end, and `textureStore` the result into a new per-frame direction atlas. A single baked static direction would be wrong: the arena lights pulse in a phased sweep, so the per-texel dominant direction swings as different lights brighten — a frozen baked direction averages to a wrong, stuck shadow, the exact failure this feature kills (`research.md`). The fused direction stays O(1) at trace time — the SDF still does one trace per texel; the added work lives in the compose pass, which runs once per frame at atlas resolution.

Compose side: `animated_lightmap_compose.wgsl`. Runtime resource: `crates/postretro/src/lighting/animated_lightmap.rs` allocates the direction atlas. Each light's per-texel incoming direction is the input to the radiance weighting.

**Open question — per-light direction source.** The compose pass needs each contributing light's per-texel incoming direction to weight it. Bake a per-light per-texel direction into the weight map (`crates/level-compiler/src/animated_light_chunks.rs` / `animated_light_weight_maps.rs`), or compute it in-compose from the light position and the texel world position. A load-size-vs-compose-complexity trade; settle during implementation.

### Task 2c: Scripting → animated-baked bridge + full reclassification (level-compiler + runtime + content)

Bridge the gap that misclassified the arena lights: there is no path to drive a *baked-animated* light's intensity from a runtime script, so script-driven lights are forced onto the fully-dynamic path. Build it:

- **Declarative authoring property.** A light property (e.g. `_animated`) declaring "static geometry, intensity arrives at runtime; reserve a baked weight map" — *no* `_animation` map keys, the curve comes from script at runtime. The compiler bakes an animated-baked weight map (`animated_light_chunks.rs` / `animated_light_weight_maps.rs`). Explicit, not auto-detected — modder-friendly.
- **Runtime intensity binding.** Route the scripted brightness/color curve into the animated-compose descriptor. The animated-compose descriptor format (period/phase/brightness/color/direction curves over a shared `anim_samples` buffer) is **the same format** the forward pass's `scripted_light_descriptors` already use, so the runtime curve plumbing is shared — the gap is only the compile-time weight-map bake.
- **Full reclassification.** Re-tag **every** `_dynamic` light in `content/dev/maps/campaign-test.map` to static — no light in the engine moves yet, so all of them are misclassified. The arena sweep lights (`arena_1_light`, `arena_wave_2`) go onto the animated-baked path: per `content/dev/scripts/arena-lights.ts` they animate **brightness only** (`setLightAnimation` with `color: null, direction: null`), geometry fully static. **Preserve the scripted-brightness stress test** — keep the script driving the sweep; do **not** convert them to map-keyed `_animation` (the scripting is part of what this exercises). The ~7 corridor point lights tagged `_dynamic`: re-tag pure-baked if their intensity is fixed, animated-baked if it pulses. Pre-release with content we own, this is a straight re-tag — no migration tooling.

**Open question — bridge mechanism.** The declarative property is settled in shape (`_animated` or similar, reserving a baked weight map). The exact compile-marker↔runtime-intensity-binding handshake stays open; settle during implementation.

### Task 3: Runtime SDF resources (postretro/render)

Parse the SDF section at load and upload: distances to a 3D atlas texture, top-level to a storage buffer, coarse distances to a 3D texture, and a meta uniform. Own these in a renderer-side struct with its own bind-group layout, used only by the shadow pass (Task 4) — not added to forward bind groups. Section absence yields a "no atlas" state that disables the pass. Read the lightmap's shadowed/unshadowed mode flag (Task 2a) and expose it so the forward pass (Task 5) knows whether to multiply SDF visibility into the static term.

### Task 4: Half-resolution SDF shadow pass + shader (postretro)

Add a compute pass scheduled **after the depth pre-pass** and before the forward pass. Per half-res pixel: reconstruct world position from depth via an inverse view-projection (threaded into the pass uniform from the camera the renderer already has). Trace **two** term types against the static SDF in v1:

1. **Static-lightmap aggregate** — one trace toward the static-lightmap baked dominant direction sampled at the pixel (`rendering_pipeline.md` §4).
2. **Animated-baked aggregate** — one trace toward the animated-atlas **per-frame** dominant direction (Task 2b) sampled at the pixel.

A third class, **geometry-moving lights** (one trace per moving light toward its own direction, against static occluders), is part of the architecture but has **no consumer in v1** — no light moves yet (Task 2c re-tags all current lights static). It is a documented seam: built when the first geometry-moving light lands, threading a moving-light array into the pass's own layout. Do not code it now.

Accumulate occlusion with the closest-passing-distance penumbra estimate. Early-out open regions via the baked DDGI `E[d]` moment sampled at the pixel. Write factors into one `Rgba8Unorm` half-res target — v1 needs exactly 2 channels (static aggregate + animated-baked aggregate), with room for the future moving-light factors. Expose march-step cap, open-space skip threshold, and penumbra `k` as uniform fields (Task 7). Add a GPU-timing pair.

### Task 5: Forward shadow-factor integration (postretro)

In the forward fragment shader, multiply by the upsampled shadow factor: the **static-lightmap term** (gated on the unshadowed-mode flag from Task 2a/3 — skip the multiply for a shadowed/legacy lightmap to avoid double shadows) and the **animated-baked term** (its animated-baked aggregate factor). These are the only two terms in v1 — the geometry-moving per-light multiply lands with the seam in Task 4, when the first moving light exists. Do **not** apply the factor to shadow-map (enemy) results — those already carry their dynamic-occluder shadow. Upsample inline with a depth-aware 2×2 bilateral filter (no extra full-res target). The `fog_composite.wgsl` bilateral upsample was reverted in `f50314d` for perf — **re-derive** the filter, don't reuse it (`research.md`). Sample the shadow-factor buffer via a new binding in the shadows bind group (group 5); update the shared group-5 BGL and every pipeline using it together with the new binding's `visibility`. Gate application on the `SdfShadowMode` uniform (off → no factor applied).

### Task 6: Debug shadow-mode toggle (postretro)

Add an `SdfShadowMode` enum (SDF on / off / visualize) mirroring the `LightingIsolation` pattern (`ALL_VARIANTS`, `label()`) in `render/mod.rs`; add a field to `FrameUniforms` and the forward `Uniforms` (keep the uniform 16-byte aligned). Add a dropdown in `debug_ui/mod.rs` `draw_diagnostics_panel()`, panel-only (no keyboard chord), matching `LightingIsolation`. Visualize mode outputs the shadow factor (or march-step heatmap) instead of shaded color.

### Task 7: Debug quality sliders (postretro, dev-tools)

Add sliders to `draw_diagnostics_panel()` in a new collapsing section. SDF knobs (pure uniform scalars, per-frame write, no rebuild): max march steps, open-space skip threshold, penumbra/cone softness `k` — write through to the shadow-pass uniform via renderer setters. Fog knobs: `step_size` (uniform scalar — add a renderer setter that updates `FogParams.step_size` and re-uploads per frame; none exists today) and `fog_pixel_scale` (resolution knob — drive `Renderer::set_fog_pixel_scale`, which rebuilds the scatter target and bind group; a resource rebuild on change, not a per-frame uniform write). Seed slider state from live renderer values on first draw, matching the ambient-floor/indirect-scale pattern.

## Sequencing

**Phase 1 (concurrent):** Task 1 (SDF section format — blocks the SDF bake and runtime parse) and Task 1b (light-model split — touches `quake_map.rs`/`prl.rs`/`spot_shadow.rs`, no overlap with the format). Task 2c depends on Task 1b's redefined `is_dynamic`.
**Phase 2 (concurrent):** Task 2, Task 2a, Task 2b, Task 3 — independent once Task 1 exists. Task 2a touches only `lightmap_bake.rs`; Task 2b touches the compose shader + animated-lightmap runtime (and the weight-map bake only if its open direction-source question lands there); Task 2 and Task 3 share no files with either.
**Phase 2b (sequential after 1b + 2b):** Task 2c — the scripting→animated-baked bridge consumes Task 2b's bake path and Task 1b's `is_dynamic` redefinition, and re-tags every `_dynamic` campaign-test light static.
**Phase 3 (sequential):** Task 4 — consumes the runtime resources from Task 3 and the animated dominant direction from Task 2b.
**Phase 4 (sequential):** Task 5 — consumes the shadow-factor buffer from Task 4, the unshadowed-mode flag from Tasks 2a/3, and the animated-baked direction term from Tasks 2b/4.
**Phase 5 (concurrent):** Task 6 and Task 7 — both edit `debug_ui` + `render/mod.rs`; coordinate on those files but otherwise independent. Both consume the pass/uniforms from Tasks 4–5.

## Wire format

New PRL section, `SectionId::SdfAtlas = 33`. Little-endian throughout, mirroring existing section conventions in `crates/level-format`. Pin during implementation: header field order (world min/max, voxel size, brick size, grid dims, atlas bricks-per-axis, surface brick count), top-level index (`u32` per brick cell, `EMPTY`/`INTERIOR` sentinels = `u32::MAX` / `u32::MAX-1`), quantized `i16` atlas distances (unit = `voxel_size_m / 256`), `f32` coarse per-brick distances. Empty-geometry encoding: zero grid dims and empty arrays, matching how `ShVolumeSection` signals an empty volume. Section is optional — absence is a valid no-SDF load, not an error. Mirror the old `sdf_atlas.rs` layout (`research.md`) unless a field is demonstrably better changed.

**Lightmap unshadowed flag.** The lightmap section gains a one-value mode marker (shadowed / unshadowed). Constraint: extend the section without breaking the existing layout's parse for legacy PRLs (a missing marker reads as shadowed — `main`'s behavior). Pin exact placement during implementation.

**Per-light shadow flags.** `MapLight` (`crates/postretro/src/prl.rs`) gains a `casts_dynamic_shadows: bool` and the redefined `is_dynamic` semantics (Task 1b). Parsed from FGD `_dynamic_shadows` / `_dynamic`. Legacy PRLs: a missing `casts_dynamic_shadows` reads as `true` (preserves today's dynamic-spot casting); `is_dynamic` retains its stored value. Pin the field layout during implementation.

**Animated-lightmap dominant direction.** The per-texel dominant direction is a **per-frame compose-written storage atlas** (Task 2b), not a baked section field — its write lifecycle differs from the irradiance bake. Octahedral-encoded, 8–16 bit (chunky shadows need no more), matching the static lightmap's direction convention. It is **not** a widening of the `Rgba16Float` irradiance atlas. Allocated by the runtime alongside the irradiance atlas, written each frame by the compose pass, read by the forward and SDF passes.

**`prl-build` CLI surface.** Two new flags: one to emit the SDF atlas section (Task 2), one to bake unshadowed irradiance (Task 2a). The animated-baked dominant direction is fused per-frame in the compose pass, so it touches the bake only if the open per-light-direction-source question (Task 2b) lands on baking per-light directions into the weight map — in which case the existing animated-lightmap stage version bumps. On the branch the campaign-test build sets both new flags. Each flag is part of its stage's cache key, so flipping either invalidates only that stage. Default (no flags) reproduces `main`'s output byte-for-byte.

## Rough sketch

- **Occluder-based split.** SDF owns static-occluder shadows; shadow maps own dynamic-occluder (enemy) shadows. This is how UE splits distance-field vs. shadow-map shadowing (`research.md`). The branch A/B isolates only the static-occluder technique.
- **O(1) static term.** The unshadowed lightmap already fuses all static lights into one irradiance + one dominant direction. One SDF trace toward that direction shadows the entire static-light term, regardless of static-light count.
- **The perf lever — fold intensity-only lights into the baked path.** Per-pixel dynamic-light SDF tracing is O(lights-affecting-pixel) — each needs its own dependent-3D-fetch trace, the exact cost class that got SDF removed originally (`research.md`). The campaign-test arena (~10 overlapping wide-cone spots) plus corridor (~7 overlapping points) overflow the channel budget. But those lights are **misclassified**: they animate brightness only, with static geometry, and are `_dynamic` solely because no bridge drove a baked-animated light's intensity from script. **No light in the engine moves at all yet** — so re-tagging them static empties the geometry-moving bucket entirely and folds every per-pixel trace into the two aggregates. The arena collapses from ~8 per-pixel traces to **2 aggregate traces** — the channel budget dissolves.
- **Channel budget is literally 2 in v1.** Static aggregate + animated-baked aggregate → one `Rgba8Unorm` trivially, with room for the future moving-light factors. No texture array, no per-pixel trace cap.
- **Light model — dynamic = geometry moves.** "Dynamic" means position/aim moves, not intensity pulses. Today that bucket is empty. Every light casts static-occluder shadows via SDF (universal); a per-light flag opts a light into dynamic-occluder (enemy) shadows via the shadow-map pool. Dynamic-occluder shadows stay culled by the existing pool ranking.
- **Baked irradiance × runtime visibility.** Standard split (DDGI/RTXGI/Lumen): the lightmap supplies the static irradiance integral, SDF supplies the per-frame visibility scalar. Multiplying at forward reconstructs shadowed static lighting while letting visibility move with dynamic lights.
- **Atlas vs. coarse split.** Surface bricks carry fine quantized distances; the top-level index routes open/solid bricks to the coarse `f32` texture. The tracer reads fine near surfaces, coarse in open space — the established two-resolution pattern (`research.md`).
- **DDGI moment as accelerator.** The shadow pass samples `E[d]` (`sh_depth_moments`, group 3 binding 14) at the pixel's probe cell; if the open distance toward the light exceeds a cell-scaled skip threshold, the march is skipped (factor = 1). The cheap analogue of UE's coarse-region fallback.
- **Cheaper shadow maps.** Because SDF handles static-occluder shadows, the spot-shadow depth pass need render only dynamic meshes (enemies), not the whole static world — net budget freed for M10.
- **Decoupling buys the bind budget.** Only group slot 7 is free. The SDF atlas lives in the shadow pass's own pipeline layout; forward gains only one shadow-factor texture binding in group 5. The last group slot stays free.
- **Quality sliders.** Two feasibility classes. **Uniform scalars** (live, no rebuild): SDF march steps, skip threshold, penumbra `k`; fog `step_size`. **Resolution/allocation** (rebuild on change): `fog_pixel_scale` via `set_pixel_scale`. Half-res shadow scale, if ever exposed, is the second class — not in v1.

## Open questions

- **Light model authoring shape.** Task 1b redefines `is_dynamic` (parsed from FGD `_dynamic`) to mean *geometry moves* and adds `_dynamic_shadows`. One decision pending owner input: the authoring shape — orthogonal flags (`_dynamic` = geometry-moves, `_dynamic_shadows` = enemy-shadow opt-in, `_animated` = script-driven intensity) vs. a few preset light *kinds* a mapper picks from. An authoring-UX call, not a perf one. (Backward compat is moot: pre-release, we own the maps and re-tag — Task 2c.)
- **Animated-baked per-light direction source.** The compose pass weights each contributing light's per-texel incoming direction by its current radiance (Task 2b). Bake that per-light direction into the weight map, or compute it in-compose from light + texel world positions. A load-size-vs-compose-complexity trade; settle during implementation.
- **Scripting→animated-baked bridge handshake.** The declarative property is settled (`_animated`-style, reserve a baked weight map). The exact compile-marker↔runtime-intensity-binding handshake (Task 2c) is unpinned. The descriptor format is already shared with `scripted_light_descriptors`, so this is plumbing, not invention — pin the binding during implementation.
- **Static-occluder SDF cost — perf gate, not a fork.** The project committed to non-frozen / soft / off-screen / point-light static-occluder shadows by choosing to build this feature; the free baked-shadow alternative structurally cannot do them, so this is not a "should we" design choice. It is the performance-validation gate the branch A/B exists to clear: the two SDF aggregate traces (static-lightmap + animated-baked, O(1) each) must hold framerate on the 2020 MBP. If they do not, the named retreat is keeping static lights baked-shadowed, which needs the shadowed bake and loses the clean branch split. The retreat is named, not designed for.
- **Skip-threshold tuning.** The `E[d]` skip distance trades a few false "lit" pixels near grazing occluders for skipped marches. The Task 7 slider sets the cell-scaled multiple by eye; starts conservative.
