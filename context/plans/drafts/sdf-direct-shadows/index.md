# SDF Direct Shadows

## Goal

Reintroduce a static signed-distance-field (SDF) of world geometry, baked into the PRL, and use it at runtime to cast **direct** shadows for dynamic lights — including point lights, which cast no dynamic shadows today. The SDF is consumed in a decoupled half-resolution shadow pass (not inline per-fragment as the removed `sdf-shadows` version was), accelerated by the existing DDGI probe depth moments. A debug shadow-mode toggle switches the direct-shadow source live so the technique can be compared against the current spot shadow maps on both visuals and per-pass GPU time.

This is an **experimental** feature: a correct, minimal foundation with room to grow, not a complete shadow system.

## Background

The old SDF (`sdf-shadows` tag) was slow because it did two per-pixel jobs: direct shadows *and* indirect-visibility cone tracing (~96 marches/fragment) to fight SH light leak. Milestone 9's DDGI depth moments + Chebyshev interpolant now own leak suppression, so the indirect job is gone. SDF returns for direct shadows only, decoupled and half-res. Full reasoning, external research, and old data structures: `research.md`.

## Scope

### In scope

- A baked **SDF atlas** PRL section: sparse brick grid of quantized distances + top-level index + coarse per-brick fallback distances, computed from static world geometry at compile time. Revives the old brick/coarse design (`research.md`).
- A `prl-build` bake stage producing the section, gated on a CLI flag, deterministic, integrated with the build cache.
- Runtime SDF resources: upload atlas to 3D textures + top-level storage buffer + meta uniform, owned by the renderer.
- A new **half-resolution SDF shadow pass** (compute) that runs after the depth pre-pass: reconstructs world position from depth, sphere-traces the SDF toward each shadow-casting dynamic light, and writes a packed per-light shadow factor to a half-res target. Uses the baked DDGI `E[d]` moment as an open-space skip early-out and the closest-passing-distance penumbra estimate for soft edges.
- Forward integration: sample the shadow-factor buffer with a depth-aware bilateral upsample, multiply each light's direct contribution by its factor.
- A debug **shadow-mode toggle**: a `ShadowMode` cycle (baseline spot maps / SDF / SDF-visualize) on a debug chord, a debug-UI dropdown, a uniform field, and a GPU-timing pair for the new pass.
- Point-light direct shadows via SDF (new capability — point lights cast no dynamic shadow today).

### Out of scope (non-goals)

- **Indirect / SH visibility via SDF.** Leak suppression stays on the DDGI depth-moment path. The shadow pass never traces the SDF for indirect lighting.
- **Lightmap-vs-SDF *static-light* shadow A/B.** The lightmap bakes static-light shadows into irradiance; a clean static A/B needs an unshadowed-irradiance bake variant. Deferred — this draft compares SDF against the *dynamic* spot-shadow path, which needs no bake surgery.
- **Dynamic occluders casting shadows** (animated enemies via capsule/mesh SDF insertion). The baked field contains static geometry only. Room to grow.
- **Replacing spot shadow maps.** Both paths coexist behind the toggle; neither is deleted.
- **More than 4 simultaneously SDF-shadowed lights.** v1 packs factors into one `Rgba8Unorm` half-res target (4 channels). Lights beyond the cap keep their existing shadow behavior. Room to grow (texture array).
- **Soft-shadow quality tuning passes / temporal accumulation of the shadow buffer.** Single-frame half-res + bilateral upsample only.
- **Removing the inline-SDF "naive" path for pedagogy.** Only the decoupled path is built; perf comparison uses the toggle + `POSTRETRO_GPU_TIMING`.

## Acceptance criteria

Automated (test- or tooling-gated):

- [ ] The SDF atlas section round-trips byte-identically through serialize → deserialize, including the empty-geometry case (empty section, no panic).
- [ ] Re-running the bake on identical input produces byte-identical section bytes (determinism contract holds).
- [ ] Bumping the SDF bake stage version invalidates the prior cache entry: first build after the bump is a miss and rebakes; the second is a hit.
- [ ] An old `.prl` without the SDF section loads without error; the renderer reports "no SDF atlas" and the shadow pass is skipped (degradation path, not a failure).
- [ ] For a known map, a brick straddling a wall stores a near-zero distance at the surface and growing distances away from it; an all-open brick is marked empty/interior in the top-level index. Distinguishable in the baked data.
- [ ] With SDF mode active and `POSTRETRO_GPU_TIMING=1`, a per-pass timing line for the SDF shadow pass is logged alongside the existing passes.
- [ ] The shadow-mode toggle cycles through all modes via the debug chord and the debug-UI dropdown; the selected mode round-trips into the frame uniform.

Manual / visual (observed by a human running the engine — not machine-verified):

- [ ] In SDF mode, a dynamic point light near static geometry casts a visible soft shadow (a capability absent in baseline mode).
- [ ] Switching baseline ↔ SDF on a dynamic spot light shows shadows from the same static occluders, with SDF edges softening with distance from the occluder.
- [ ] SDF-visualize mode renders an interpretable shadow-factor (or march-step) view usable to spot artifacts.
- [ ] No light leak regression in indirect lighting when toggling shadow modes (indirect is independent of this feature).

## Tasks

### Task 1: SDF atlas PRL section (level-format)

Revive `crates/level-format/src/sdf_atlas.rs`: the `SdfAtlasSection` struct, brick/top-level/coarse layout, slot sentinels, and serialize/deserialize mirroring the old design (`research.md`). Register a new `SectionId::SdfAtlas` at value 33 in `crates/level-format/src/lib.rs` and wire it through the section read/write paths. Add round-trip and empty-geometry tests. This is the format foundation both the bake and the runtime parse against.

### Task 2: SDF bake stage (level-compiler)

Add `crates/level-compiler/src/sdf_bake.rs`: compute per-voxel signed distances to nearest static geometry using the compiler's existing geometry/BVH (the same data the SH baker traverses), pack surface bricks into the atlas, fill the top-level index with empty/interior/surface slots, and compute coarse per-brick fallback distances. Gate the stage on a new CLI flag on `prl-build`. Make it a build-cache stage with its own version constant; keep accumulation order-stable for deterministic output. Log a coarse bake-stats line (brick counts, atlas size).

### Task 3: Runtime SDF resources (postretro/render)

Parse the SDF section at load and upload: distances to a 3D atlas texture, top-level to a storage buffer, coarse distances to a 3D texture, and a `SdfMeta`-equivalent uniform. Own these in a renderer-side resource struct with its own bind-group layout, used only by the shadow pass (Task 4) — not added to the forward bind groups. Absence of the section yields a "no atlas" state that disables the pass.

### Task 4: Half-resolution SDF shadow pass + shader (postretro)

Add a compute pass scheduled **after the depth pre-pass** (depth is the input) and before the forward pass. Per half-res pixel: reconstruct world position from the depth buffer via an inverse view-projection (thread the inverse matrix into the pass uniform from the camera the renderer already has), then for each shadow-casting dynamic light affecting the pixel, sphere-trace the SDF toward the light, accumulating occlusion with the closest-passing-distance penumbra estimate. Early-out open regions using the baked DDGI `E[d]` moment sampled at the pixel (skip the march when the region is open toward the light past a skip distance). Bind the dynamic light array (same storage buffer the forward path uses) in the pass's own layout. Write up to 4 lights' factors into one `Rgba8Unorm` half-res target. Add a GPU-timing pair.

### Task 5: Forward shadow-factor integration (postretro)

In the forward fragment shader's dynamic-light loop, for lights assigned an SDF shadow channel, multiply the direct contribution by the upsampled shadow factor. Upsample inline with a depth-aware 2×2 bilateral filter reusing the `fog_composite.wgsl` pattern (no extra full-res target). Sample the shadow-factor buffer via a new binding in the shadows bind group (group 5); the shared group-5 BGL and every pipeline that uses it must be updated together with the new binding's `visibility`. Gate application on the shadow-mode uniform.

### Task 6: Debug shadow-mode toggle (postretro)

Add a `ShadowMode` enum (baseline / SDF / SDF-visualize) mirroring the `LightingIsolation` pattern (`cycle()`, `ALL_VARIANTS`) in `render/mod.rs`; add a `shadow_mode` field to `FrameUniforms` and the forward `Uniforms`. Add a `DiagnosticAction` cycle variant and a free Alt+Shift chord in `input/diagnostics.rs`, plus a dropdown in `debug_ui/mod.rs` `draw_diagnostics_panel()`. SDF-visualize mode outputs the shadow factor (or march-step heatmap) instead of shaded color.

## Sequencing

**Phase 1 (sequential):** Task 1 — the section format blocks both the bake and the runtime parse.
**Phase 2 (concurrent):** Task 2 (bake) and Task 3 (runtime resources) — independent once the format exists; they share no files.
**Phase 3 (sequential):** Task 4 — consumes the runtime resources from Task 3.
**Phase 4 (sequential):** Task 5 — consumes the shadow-factor buffer from Task 4.
**Phase 5 (sequential):** Task 6 — consumes the forward uniform from Task 5 and the pass from Task 4 (timing/visualize).

## Wire format

New PRL section, `SectionId::SdfAtlas = 33`. Little-endian throughout, mirroring the existing section conventions in `crates/level-format`. Pin during implementation: header field order (world min/max, voxel size, brick size, grid dims, atlas bricks-per-axis, surface brick count), then top-level index (`u32` per brick cell, with `EMPTY`/`INTERIOR` sentinels = `u32::MAX` / `u32::MAX-1`), then quantized `i16` atlas distances (unit = `voxel_size_m / 256`), then `f32` coarse per-brick distances. Empty-geometry encoding: zero grid dims and empty arrays, matching how `ShVolumeSection` signals an empty volume. Section is optional — absence is a valid lower-fidelity (no-SDF) load, not an error. Mirror the old `sdf_atlas.rs` layout (`research.md`) unless a field is demonstrably better changed.

## Rough sketch

- **Atlas vs. coarse split.** Surface bricks carry full-resolution quantized distances; the top-level index routes open/solid bricks to the coarse `f32` texture. The sphere tracer reads the fine atlas near surfaces and the coarse texture in open space — the established two-resolution pattern (`research.md`).
- **DDGI moment as accelerator.** `sh_corner_depth_visibility` already proves the moment texture (`sh_depth_moments`, group 3 binding 14) is sampleable at runtime. The shadow pass samples `E[d]` at the pixel's probe cell; if the open distance toward the light exceeds a cell-scaled skip threshold, the march is skipped (factor = 1). This is the cheap analogue of UE's coarse-region fallback.
- **Decoupling buys the bind budget.** Only one group slot (7) remains. The decoupled pass keeps the SDF atlas out of the forward bind groups entirely — the atlas lives in the shadow pass's own pipeline layout; forward only gains a single screen-space shadow-factor texture binding in group 5. The last group slot stays free.
- **Light assignment.** Reuse the existing shadow-ranking notion (the spot pool ranks lights into slots); SDF mode ranks shadow-casting lights — now including point lights — into the 4 buffer channels.

## Open questions

- **Light channel packing vs. accumulation.** v1 packs 4 per-light factors into `Rgba8Unorm`. Alternative: accumulate a single combined occlusion per pixel and lose per-light attribution (cheaper, but wrong when two shadowed lights overlap). Spec assumes per-light channels; confirm 4 is enough for test maps.
- **Shadow pass: compute vs. fragment.** Compute is assumed (arbitrary half-res dispatch, easy world-pos reconstruction). A full-screen fragment pass is the fallback if the compute path complicates depth sampling.
- **Skip-threshold tuning.** The `E[d]` skip distance trades a few false "lit" pixels near grazing occluders for skipped marches. Needs a visual pass to set the cell-scaled multiple; starts conservative.
- **Does the lightmap-static-shadow A/B get promoted later?** If the experiment favors SDF, an unshadowed-irradiance lightmap bake becomes the follow-up that enables the full static comparison. Out of scope here, flagged for the roadmap.
