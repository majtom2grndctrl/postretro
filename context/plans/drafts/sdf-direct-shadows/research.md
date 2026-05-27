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

**Cost caveat.** Tracing the static term every frame is strictly more runtime work than baked-into-lightmap shadows (free at runtime). The dominant-direction fusion holds it to one trace, but the comparison still weighs fidelity/flexibility vs. that cost. If too expensive, the fallback (static lights stay baked-shadowed; SDF shadows only dynamic-light-vs-static-occluder terms) needs the shadowed bake and loses the clean branch split — flagged as an open question, not designed here.

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
