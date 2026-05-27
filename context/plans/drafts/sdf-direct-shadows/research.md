# SDF Direct Shadows — Research Notes

Background and findings behind `index.md`. Not a spec. Decisions live in the spec; this is the reasoning and the source material.

## Why SDF was removed, and what changed

The `sdf-shadows` git tag (commit `2297684`, April 2026) ran SDF as a per-fragment, inline workload in the forward pass. It did **two** distinct sphere-trace jobs per pixel:

1. **Direct shadows** — `sample_sdf_shadow`, up to 32 march steps per shadow-casting light per fragment.
2. **Indirect visibility** — the SH irradiance volume traced 8 corner cones (`SH_VIS_MAX_STEPS = 12` each → ~96 marches/fragment) solely to suppress light leak through walls.

Each `sample_sdf` step is a *dependent* 3D-texture fetch (each fetch waits on the previous), the worst case for GPU latency hiding, and 3D textures cache poorly. On the dev machine (2020 MBP, Radeon Pro) the combined ~96 + 32×N marches per pixel, full-res, in the forward pass with overdraw, was the bottleneck. A depth pre-pass was added late as mitigation.

**What changed since:** Milestone 9 shipped the DDGI visibility term — per-probe depth moments (`E[d]`, `E[d²]`) baked into `ShProbe` and a runtime Chebyshev interpolant (`sh_corner_depth_visibility` in `sh_sample.wgsl`) that suppresses indirect leak with a couple of texture fetches and a statistical test. **This subsumes job #2 entirely.** The ~96 indirect marches no longer have a reason to exist.

So the revival is a *subtraction*: SDF comes back only for **direct shadows** (job #1), running alone, never re-adding indirect SDF visibility. That is the core architectural decision and the reason the perf story is now tractable.

## The split is by occluder, not by light

The decisive reframing. An earlier draft had SDF *replace* the shadow-map path entirely — "SpotShadowPool machinery is bypassed," SDF the single direct-shadow source. **That is wrong, and Milestone 10 is why.**

M10 (the active milestone) adds animated enemies and the engine's first per-entity skinned-mesh render path. Enemies cast shadows by rendering into the spot-shadow depth pass — the 12-slot `SpotShadowPool` (`SHADOW_POOL_SIZE = 12`, ranked by projected influence area among influence+frustum-culled lights; `crates/postretro/src/lighting/spot_shadow.rs`). The baked SDF contains **static geometry only** — enemies are not in it and never will be in v1. A branch that bypassed shadow maps and relied on the static SDF would make enemies cast **no shadows** the moment M10 lands. Unacceptable regression.

So split by the **occluder**, not the light:

- **Static geometry casts via SDF** — for static *and* dynamic lights, point *and* spot. A dynamic spot light throwing a static wall's shadow is an SDF query, not a shadow-map render. This is the Quake-impossible win set: off-screen static occluders (on-theme for monster closets and scripted reveals), point-light static shadows (the engine has none today), soft penumbra, and shadows not frozen into the bake.
- **Dynamic geometry casts via shadow maps** — enemies and moving meshes render into the existing 12-slot pool. Crucially this is now *cheaper* than today: if SDF owns static-occluder shadows, the shadow-map pass need only render *dynamic* meshes into its maps, not the whole static world. Budget freed for M10.

This is also how Unreal splits it: movable/dynamic occluders use shadow maps and screen-space traces; large-scale static occlusion uses mesh/global distance fields (see External research). Distance fields for static/large-scale, shadow maps for dynamic detail.

**Net effect on the branch A/B.** The branch cleanly isolates the **static-occluder shadow technique**: `main` = static-occluder shadows baked into the lightmap; branch = unshadowed lightmap × runtime SDF visibility. Dynamic/enemy shadows stay on shadow maps on **both** branches — neither part of the A/B nor broken by it.

## Branch comparison unlocks the unshadowed-lightmap split

The blocker for runtime static-occluder shadows: the lightmap bakes static-light shadows **into** irradiance (`lightmap_bake.rs`: `shadow_visible` zeroes occluded texels before accumulation). A within-build A/B running both the shadowed lightmap and an SDF static-occluder term would double-count — the shadow appears once in the baked irradiance and again in the SDF factor.

The owner's decision: build on a **dedicated git branch** and compare by switching branches. Branch isolation removes the coexistence requirement:

- The branch's lightmap bake produces **unshadowed irradiance** — `shadow_visible` is bypassed so every static light is treated as fully visible during accumulation. The atlas holds the static-light irradiance/bounce integral *without* a visibility term.
- SDF supplies the visibility (shadow) scalar at runtime, multiplied into the static-light term in the forward shader. Standard "baked irradiance × runtime visibility" split — the same factorization DDGI/RTXGI and UE Lumen use for their irradiance-cache + screen-trace occlusion.

**The dominant-direction fusion — O(1) in static-light count.** The lightmap does not store per-static-light irradiance; it fuses *all* static lights into one irradiance value plus one dominant incoming direction per texel (`rendering_pipeline.md` §4: "ray-casts per-texel irradiance and a dominant incoming light direction from all static lights"). So the runtime visibility term is a *single* SDF trace toward that one dominant direction, covering the entire static-light term regardless of how many static lights contributed. This is what keeps the static-occluder cost flat: O(1) per pixel, not O(static lights). The cost is an approximation — a texel lit by two static lights from different directions gets one shadow, traced toward their luminance-weighted mean — but the hard, pixelated retro shadow aesthetic absorbs it (see *Retro aesthetic as a budget* below). The per-light-exact alternative is to move static lights into the runtime dynamic loop, which multiplies the trace cost per light and blows the budget; it is named as an escape hatch only, not designed here.

**Runtime toggle reconsidered.** The prior draft's 3-way `ShadowMode` (baseline spot maps / SDF / SDF-visualize) existed to A/B spot-maps vs. SDF *within a build*. With the branch as the A/B — and with shadow maps staying live on both branches — that purpose is gone. Kept: a debug `SdfShadowMode` (SDF on / off / visualize), where "off" and "visualize" earn their place as debugging aids. Matches the `LightingIsolation` convention — panel-only, no keyboard chord (the chord was removed).

**Cost is a perf gate, not a design fork.** Tracing the static term every frame is strictly more runtime work than baked-into-lightmap shadows (free at runtime). But the project already committed to non-frozen / soft / off-screen / point-light shadows by choosing to build this feature — the free baked alternative structurally cannot do them. So the cost is not a "should we" question; it is the validation the branch A/B exists to clear: the dominant-direction fusion holds the static term to two aggregate traces (static-lightmap + animated-baked, O(1) each), and that must hold framerate on the 2020 MBP. The named retreat, if it does not — static lights stay baked-shadowed, SDF shadows only geometry-moving-light-vs-static-occluder — needs the shadowed bake and loses the clean branch split. The retreat is named, not designed for.

## The channel budget was a symptom: misclassified "dynamic" lights

The earlier draft's top open question was a shadow-factor *channel budget* — one `Rgba8Unorm` (4 channels) holding 1 static aggregate + up to 3 per-dynamic-light factors, flagged as tight. Investigation found the budget is a **symptom of a deeper, largely self-inflicted problem**, and that resolving the cause dissolves the budget.

**Per-pixel dynamic-light SDF tracing is O(lights-affecting-pixel).** Unlike the O(1) static term (all static lights fused into one dominant-direction trace), each *dynamic* light needs its own SDF trace toward its own direction — the same dependent-3D-texture-fetch cost class that got SDF removed originally (see "Why SDF was removed"). The flagship test map `content/dev/maps/campaign-test.map` is the worst case: 12 `light_spot` + 18 `light`, of which **17 carry `_dynamic 1`** — an arena with ~10 dynamic spot lights (wide 45° cones, ~800-unit radius) overlapping **4–8 deep** on the floor, plus a corridor with ~7 overlapping dynamic point lights. Eight overlapping dynamic traces per arena-floor pixel both overflows the 4-channel budget and re-creates the per-fragment march cost the revival was supposed to escape.

**But those lights are misclassified — and so is every other `_dynamic` light.** No light in the engine physically moves yet: nothing animates position or aim. So the geometry-moving bucket is **empty today**, and every current `_dynamic` light is mistagged. Per `content/dev/scripts/arena-lights.ts`, the arena lights animate **brightness only**: `setLightAnimation` is called with `color: null, direction: null` — only an intensity curve, driving a phased sweep across the `arena_1_light` set (plus `arena_wave_2`). Their **geometry — position, aim, cone — is fully static.** They are `_dynamic` only because of a missing bridge: there is **no path to drive a baked-animated light's intensity from a runtime script**, so a script-driven-intensity light is forced onto the fully-dynamic runtime path (`filter_dynamic_lights`, `crates/postretro/src/render/mod.rs:3262`, iterates only `is_dynamic` lights). The corridor points are the same story: fixed or pulsing intensity, static geometry.

So the budget pressure is not intrinsic to the map — it is an artifact of conflating "intensity animates" with "light is dynamic." Re-tagging all of them static (Task 2c) empties the geometry-moving bucket and leaves v1 tracing only the two aggregates.

## The one real blocker: the animated-lightmap atlas stores irradiance only

The animated-lightmap bake (`crates/level-compiler/src/animated_light_chunks.rs` + `animated_light_weight_maps.rs`; runtime upload `crates/postretro/src/lighting/animated_lightmap.rs`) and its compose shader store **irradiance only**: `textureStore(animated_lm_atlas, ..., vec4<f32>(accum, 1.0))` at `crates/postretro/src/shaders/animated_lightmap_compose.wgsl:153`, into an `Rgba16Float` atlas (binding 6). There is **no dominant incoming direction** stored, so if intensity-only lights move to the animated-baked path, the SDF has **nothing to trace toward** for the animated-baked aggregate term.

The static lightmap, by contrast, *does* store a per-texel dominant incoming direction (`rendering_pipeline.md` §4), which is exactly what the static-aggregate SDF trace reads. So the design must **add a per-texel dominant direction to the animated-baked path** (Task 2b).

The wrong fix is to bake one static direction, reasoning "the geometry is static, so the direction is static." It is not. The arena lights sit at distinct positions around the arena and pulse in a *phased sweep*; for a given texel the dominant incoming direction swings substantially as different lights brighten. A single baked direction averages to a wrong, frozen shadow — the exact "frozen into the bake" failure this feature exists to kill.

The right fix fuses the direction **per frame, in the compose pass.** The pass already loops per-texel over contributing lights computing `accum += c*b*weight`. Have it also accumulate each light's incoming direction weighted by its current radiance, normalize, and write it to a per-frame direction atlas. The SDF pass still does exactly **one trace per texel** toward that fused direction, so the shadow tracks the sweep at no extra trace cost. The added work lives in the compose pass — once per frame, at atlas resolution, not per screen pixel. The O(1)-at-trace-time perf lever is preserved.

The one genuinely-open sub-choice: bake per-light per-texel directions into the weight map, or compute them in-compose from light + texel world positions — a load-size-vs-compose-complexity trade.

## The perf lever: fold intensity-only lights into one aggregate trace

The reframe pays off directly. If intensity-only lights move to the animated-**baked** path, their static-occluder shadow folds into **one aggregate SDF trace** (O(1)), exactly like the static-lightmap term — because they too share one baked dominant direction per texel. The arena's per-pixel SDF cost collapses from **~8 traces to 2 aggregate traces** (static-lightmap term + animated-baked term). Since no light moves yet, those two aggregates are the **entire** v1 trace cost — the geometry-moving bucket is empty.

The channel budget then **evaporates**: v1 needs exactly 2 channels — 1 static aggregate + 1 animated-baked aggregate → one `Rgba8Unorm` fits trivially, with room left for the future moving-light factors, no texture array and no per-pixel trace cap. The "accumulate one combined occlusion per pixel" alternative the prior draft worried about (loses per-light attribution, wrong when terms overlap) is moot — terms are aggregated by *baking class*, not crammed per-light. The spare channels and a possible texture array survive only as room-to-grow for the day geometry-moving shadow-casters land.

**The direction is correct per frame, not approximated.** The animated-baked dominant direction is fused per-frame in the compose pass (see "The one real blocker"), so the shadow tracks the sweep — the brightest contributor changing mid-sweep moves the shadow with it. This is deliberate: the chunky retro look licenses **coarseness** (low resolution, hard edges, nearest filtering) but **not incorrectness** (a shadow pointing the wrong way). A frozen baked direction would be an incorrectness error, which the retro budget does not excuse; the per-frame fusion costs no extra trace, so there is no reason to accept the wrong version. The aggregation across overlapping lights into *one* radiance-weighted direction is the coarseness the retro look does absorb — one shadow for the fused term, not one per light.

## The light model: dynamic = geometry moves; dynamic-occluder shadows opt-in

The reframe yields a cleaner light model that maps directly onto the occluder split:

- **"Dynamic" should mean a light whose *geometry* moves** (position/aim), not one whose *intensity* (brightness/color) pulses. Intensity-only animation belongs on the baked path. This redefines `is_dynamic` (today parsed straight from FGD `_dynamic`, `crates/level-compiler/src/format/quake_map.rs` ~:245).
- **Every light casts static-occluder shadows via the SDF** (universal) — through one of three trace classes: static-lightmap aggregate, animated-baked aggregate, or a per-light trace for geometry-moving lights. The third class is **empty in v1** (no light moves yet); it stays named as the future seam, and v1 traces only the two aggregates.
- **Dynamic-occluder (enemy) shadows are opt-in per light**, via a new `casts_dynamic_shadows` authoring flag (FGD `_dynamic_shadows`) gating shadow-map-pool eligibility. Today `cast_shadows` defaults hardcoded `true` for *all* lights (`quake_map.rs:407`) and `rank_lights` (`crates/postretro/src/lighting/spot_shadow.rs`, `SHADOW_POOL_SIZE = 12`) gates on `is_dynamic && light_type == Spot` — point/sun lights cast no dynamic shadow today. The default for the new flag preserves that behavior (dynamic spots cast).
- **Dynamic-occluder shadows stay culled by the existing pool ranking** — the flag is an eligibility gate, not a replacement for the rank-by-projected-influence-area cull.

This is the occluder split (SDF for static occluders, shadow maps for dynamic occluders) made *honest* about which lights are actually dynamic: it stops paying per-pixel SDF cost for lights that never move, while keeping the universal static-occluder coverage SDF gives every light.

## The bridge is plumbing, not invention

A key grounding fact de-risks Task 2c. The animated-compose **descriptor format** — period/phase/brightness/color/direction curves over a shared `anim_samples` buffer — is **the same format** the forward pass's `scripted_light_descriptors` already use. So the existing animated-light infrastructure already separates **baked weight maps** (compile-time, per-texel light contribution) from **runtime intensity curves** (the descriptor), and the runtime curve plumbing is already shared between the baked-animated and scripted paths.

What is missing is narrow: a **compile-time path to bake a weight map for a static-geometry light whose intensity is script-driven** (no `_animation` map keys — the curve arrives from script at runtime). Once that weight map exists, the runtime routes the scripted brightness/color curve into the animated-compose descriptor through the already-shared format. The bridge is wiring two existing halves together, not designing a new subsystem — which is why the arena lights can be reclassified off `_dynamic` onto the animated-baked path **while preserving the scripted-brightness stress test** (the script keeps driving the sweep; the lights are not converted to map-keyed `_animation`).

## Retro aesthetic as a budget

The hard, pixelated, nearest-filtered shadow look is not just a style choice — it is a **budget** that licenses cheaper approximations a photoreal engine could not ship:

- **Dominant-direction static fusion** (above): one trace for the whole static-light term. A photoreal engine would shadow each static light separately; the retro look hides the single-shadow approximation.
- **Half-resolution shadow pass** + depth-aware bilateral upsample: the established production shape (RTSDF, UE), and the chunky aesthetic tolerates the half-res factor better than a soft-shadow photoreal target would.
- **Nearest-ish filtering / low march counts**: `rendering_pipeline.md` §4 already establishes nearest-neighbor lightmap filtering as "arguably more correct on octahedral-encoded directions." Low SDF step counts plus the free closest-passing-distance penumbra estimate (Inigo-Quilez `k·d/t`) carry softness without high sample counts.

Frame every quality/cost tradeoff in this feature through this lens: the question is not "is this physically exact" but "does the retro look absorb the approximation." The dominant-direction fusion is the load-bearing example.

## M10 / enemy budget interaction

M10 brings a net-new per-entity skinned-mesh pass — the engine's first dynamic mesh render path (today only billboards/particles are dynamic). This feature must not break or compete with it:

- **Enemies cast** via shadow maps, not SDF — they render into the 12-slot spot-shadow depth pass. With SDF owning static-occluder shadows, that pass renders *dynamic meshes only*, not the static world, so adding enemies to it costs less than rendering the full world per slot did.
- **Enemies receive** indirect via the SH volume (M9's depth-aware Chebyshev interpolant already handles dynamic entities) plus the runtime dynamic direct loop. The SDF shadow factor is a screen-space static-occluder visibility term; it does not touch enemy shadow-map results.
- **Dynamic occluders in the SDF** (capsule/mesh insertion so enemies cast SDF shadows) stays a non-goal / room-to-grow. In v1, enemy shadows come from shadow maps, full stop.

The two systems are orthogonal by construction: SDF answers "is this pixel occluded from this light by *static* geometry," shadow maps answer "by *dynamic* geometry." They multiply independently into different light terms.

## Quality-slider feasibility and the fog-knob source audit

The grounding brief said to verify fog live-tuning knobs against current source. Findings:

- **`fog_pixel_scale`** — real. `FogPass::set_pixel_scale` (`render/fog_pass.rs`) clamps the scale and, on change, calls `resize`, which recreates the scatter target and rebuilds the group-6 / composite bind groups. So this knob is a **resource rebuild**, not a uniform write — correct to slider-drive but it must run through `set_pixel_scale`, never a per-frame uniform path. Renderer entry point: `Renderer::set_fog_pixel_scale`.
- **`fog.step_size`** — real, a plain uniform scalar (`FogParams.step_size`, defaulted from `DEFAULT_FOG_STEP_SIZE`, read as `fog.step_size` in `fog_volume.wgsl`). Live-tunable per frame. But **no renderer setter exists today** — `set_fog_pixel_scale` exists, a step-size setter does not. Task 7 adds one (write field + re-upload params).
- **`ACCUM_ALPHA` / fog temporal resolve** — **does not exist in current source.** `rendering_pipeline.md` §7.5 documents a `fog_resolve.wgsl` temporal-accumulation pass with an `ACCUM_ALPHA` EMA constant and `prev_view_proj` reprojection; none of it is present (`grep` finds no `ACCUM_ALPHA`, no `fog_resolve.wgsl`, no `prev_view_proj` in `crates/postretro/src`). That doc section is drift, ahead of the code. So there is no const to promote to a uniform and no alpha slider to add — excluded from scope. (Fixing the §7.5 drift is out of scope for this draft, which must not touch `context/lib/`.)

SDF knobs are all pure uniform scalars — max march steps, open-space skip threshold, penumbra `k` — so they are per-frame uniform writes with no rebuild, the cheap class. Half-res shadow scale, if ever exposed, would be the rebuild class (recreate the half-res target), so v1 leaves it fixed.

**Quality-knob norms.** Exposing march-step count, a skip/cull distance, and a penumbra softness `k` matches the live tuning surface UE's Distance Field Shadows expose (per-light shadow softness; project-level mesh-distance-field quality). Step count and softness are the two knobs that dominate the SDF soft-shadow quality/cost trade — RTSDF tunes the same pair. No new external source needed beyond the SDF references below.

## External research

**RTSDF (NUS, 2022)** — soft-shadow SDF technique. Confirms the standard production shape: **half-resolution shadow compute + depth-aware upsample**; cone-trace softness approximated *for free* by tracking the closest passing distance during the march (the Inigo-Quilez `k·d/t` penumbra estimate the old `sample_sdf_shadow` already used). Coarse + fine SDF resolutions (128³ / 256³) with the march preferring fine near surfaces and falling back to coarse in open space — mirrors the old brick atlas + coarse-distance texture split.

**Unreal Distance Field Shadows** — movable lights shadow off per-object/static distance fields; "by tracking the closest distance a ray passed by an occluding object, an approximate cone intersection can be computed with no extra cost… intersections determined with a small number of steps." Near samples use the per-object field; far samples use a camera-clipmap Global Distance Field. Validates: (a) static-baked SDF shadowing dynamic lights is the established use, (b) cone softness is free, (c) low step counts suffice.

**Static/dynamic occluder split (UE).** UE does not use distance fields for everything. Distance-field shadows cover **static/large-scale** occluders; **movable/dynamic** occluders (characters) fall back to conventional shadow maps and screen-space techniques — distance fields are too coarse and too expensive to rebuild per-frame for animated meshes. This is exactly the split this feature adopts: SDF for static-occluder shadows, the existing 12-slot `SpotShadowPool` for dynamic-occluder (enemy) shadows. The retro engine's version is simpler (one baked static field, no per-object fields, no global clipmap) but the partition is the same.

Takeaways folded into the spec: decouple to half-res + depth-aware upsample; keep the brick/coarse split; use the baked DDGI `E[d]` moment as a cheap open-space skip (our analogue to UE's coarse-region fallback); keep step counts low and let the penumbra estimate carry softness.

Sources:
- RTSDF: https://arxiv.org/pdf/2210.06160
- UE Distance Field Soft Shadows: https://dev.epicgames.com/documentation/unreal-engine/distance-field-soft-shadows-in-unreal-engine
- UE Mesh Distance Fields: https://dev.epicgames.com/documentation/unreal-engine/mesh-distance-fields-in-unreal-engine

## Old SDF data structures (from `sdf-shadows` tag — reference for revival)

Old files: `postretro/src/render/sdf.rs`, `postretro-level-format/src/sdf_atlas.rs`, `postretro-level-compiler/src/sdf_bake.rs`.

`SdfAtlasSection` (old `sdf_atlas.rs`): `world_min/max: [f32;3]`, `voxel_size_m: f32`, `brick_size_voxels: u32`, `grid_dims: [u32;3]`, `top_level: Vec<u32>` (one slot per brick cell), `atlas: Vec<i16>` (quantized distances, unit = `voxel_size_m/256`), `coarse_distances: Vec<f32>` (one per brick cell). Slot sentinels: `BRICK_SLOT_EMPTY = u32::MAX`, `BRICK_SLOT_INTERIOR = u32::MAX-1`, surfaces from 0.

`SdfMeta` GPU uniform (64 bytes): `world_min`, `voxel_size_m`, `world_max`, `brick_size_voxels`, `grid_dims`, `has_sdf_atlas`, `atlas_bricks` (bricks-per-axis in the packed 3D atlas), pad.

Old bind layout (group 2, bindings 5–9): `sdf_atlas_tex: texture_3d<f32>`, `sdf_atlas_sampler`, `sdf_top_level: storage array<u32>`, `sdf_meta: uniform`, `sdf_coarse_tex: texture_3d<f32>`. **Those binding slots are no longer free on main** (group 2 binding 5 is now `chunk_indices`). The revived bindings live in the new shadow-pass pipeline's own layout, not the forward group 2 — see spec.

The brick atlas + top-level + coarse-fallback design is sound and worth reviving largely as-is. What changes is *consumption*: not inline in forward, but in a decoupled half-res pass.

## Current main grounding (consumed by the spec)

- **Section registry** — `crates/level-format/src/lib.rs`, `enum SectionId` (`#[repr(u32)]`). Highest used = 32 (`FogCellMasks`). Next free = **33**.
- **DDGI moments** — `crates/level-format/src/sh_volume.rs`: `ShProbe { sh_coefficients:[f32;27], validity:u8, mean_distance:u16, mean_sq_distance:u16 }`, `SH_VOLUME_VERSION = 4`, `PROBE_STRIDE = 116`. Runtime: `sh_corner_depth_visibility` (Chebyshev) in `crates/postretro/src/shaders/sh_sample.wgsl`; `sh_depth_moments: texture_3d<f32>` (RG = E[d], E[d²]) bound at group 3 binding 14 (`render/sh_volume.rs`).
- **Current shadows** — `crates/postretro/src/lighting/spot_shadow.rs`: `SpotShadowPool` (D2 array, 12 slots, 1024², `Depth32Float`, comparison sampler) at group 5 (`spot_shadow_depth`, `spot_shadow_compare`, `light_space_matrices`). Only dynamic spot lights cast; point/sun lights cast no dynamic shadow today. `MapLight.cast_shadows: bool` exists (`crates/postretro/src/prl.rs`).
- **Lightmap** — `SectionId::Lightmap = 22`, `LightmapSection` (`crates/level-format/src/lightmap.rs`), runtime `LightmapResources` at group 4 (`crates/postretro/src/lighting/lightmap.rs`). Bakes irradiance **with static-light hard shadows folded in**: `bake_lightmap` accumulates each light's `light_contribution_and_direction` only when `shadow_visible` passes (`crates/level-compiler/src/lightmap_bake.rs`). The bake already factors contribution from visibility cleanly, so the unshadowed mode is a one-branch change (skip the `shadow_visible` gate) — the basis for Task 2a. `STAGE_VERSION` (currently 1) gates the cache; bump it when the mode flag lands.
- **Debug toggles** — `crates/postretro/src/input/diagnostics.rs`: `DiagnosticAction`, `DiagnosticChord`, `default_diagnostic_chords()` (Alt+Shift namespace; existing: wireframe, portal-walk dump, vsync, debug panel — no cycle actions yet). Uniform: `FrameUniforms` + `LightingIsolation` (10 variants, `cycle()`, `ALL_VARIANTS`) in `crates/postretro/src/render/mod.rs`; forward reads `uniforms.lighting_isolation`. Debug UI dropdown: `crates/postretro/src/render/debug_ui/mod.rs` `draw_diagnostics_panel()`.
- **Pass order** (`render/mod.rs` `draw_frame`): cull (compute) → animated-LM compose (compute) → SH compose (compute) → spot-shadow depth → depth pre-pass → forward → smoke → fog → composite. GPU timing pairs: `TIMING_PAIR_CULL/ANIMATED_LM_COMPOSE/DEPTH_PREPASS/FORWARD`.
- **Bind budget** — `max_bind_groups = 8`. Groups 0–6 used; **group 7 is the one remaining slot**. The decoupled design avoids spending it on forward: the SDF atlas binds only inside the new shadow pass pipeline (its own layout); forward gains a single shadow-factor texture binding in the shadows group (5).
- **Bilateral upsample — no longer a live precedent.** `fog_composite.wgsl` *used* to do depth-aware 2×2 low-res→full-res upsampling, but commit `f50314d` (2026-05-24) reverted the fog quality work — bilateral upsample, animated jitter, and the temporal-resolve pass — because the bundle dropped the 2020 MBP below 60fps with vsync. Current `fog_composite.wgsl` is 93 lines with no bilateral terms; `fog_resolve.wgsl` is deleted. So the shadow pass must **re-derive** a depth-aware 2×2 bilateral upsample, not reuse existing code. (Note: `rendering_pipeline.md` §7.5 still documents the reverted temporal-resolve pass — doc drift, ahead of code. Out of scope to fix here.) The revert message names "a fog quality-settings system once UI/save data exist" as the intended fix — the Task 7 sliders are a step toward it.
