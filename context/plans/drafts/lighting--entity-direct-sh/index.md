# Baked Static Direct SH for Entities + Billboards

> **Status:** draft.
> **Track:** Lighting — closes the half-built dynamic-object lighting model. Follows M9 (octahedral SH GI) and rides on M10's mesh pass.
> **Related:** `context/lib/rendering_pipeline.md` §4 (lighting), §8 (shader composition), §9 (skinned model pipeline) · `context/plans/in-progress/M10--mesh-render-pass/` · sibling `research.md` (code-grounding citations).

## Goal

Give moving objects (mesh entities, billboards) the room's baked direct brightness they currently lack. The engine bakes lighting in two disjoint parts: lightmaps carry DIRECT light on static surfaces; the octahedral SH volume carries INDIRECT (bounced) light only. Entities and billboards can't use lightmaps, so they are lit by indirect SH alone and read much dimmer than the lightmapped geometry around them. Add a SECOND, dense, baked SH layer carrying the DIRECT light arriving at each probe from STATIC lights (per-light, shadow-ray visibility, projected into L2 SH). Dynamic shaders sample indirect + direct and finally match the room — at no runtime light-loop cost, because it is baked.

Net runtime symmetry after this lands: static surface = lightmap-direct + SH-indirect; entity/billboard = SH-direct + SH-indirect.

## Scope

### In scope

- **Dense direct-at-probe SH bake.** For each probe, project the direct radiance from every STATIC light (the same `StaticBakedLights` set/order the lightmap baker uses), shadow-ray-visibility-tested, into L2 SH using the existing projection + cosine-lobe convolution. Dense (all probes), no animation.
- **A SEPARATE direct atlas.** The direct data lives in its own octahedral atlas, tile geometry IDENTICAL to the indirect atlas. ONLY the dynamic shaders sample it. The shared indirect total atlas (`sh_total_atlas`) is unchanged.
- **New PRL section.** A new section id (mirroring `OctahedralShVolume`/`DeltaShVolumes`) carries the direct atlas. No `SH_VOLUME_VERSION` bump — existing v7 maps stay loadable (they simply lack the new section).
- **Entity + billboard shaders sample indirect + direct.** `skinned_mesh.wgsl` and `billboard.wgsl` add a direct atlas sample alongside the existing indirect sample and SUM the two irradiance terms before the albedo multiply.
- **Debug toolbar control.** A direct-scale slider + an isolation toggle (direct-only / indirect-only / combined) for the dynamic path, to verify brightness parity against lightmapped surfaces. Includes wiring a scale uniform into the mesh/billboard path (which today read no such field).
- **Bake caching + determinism parity.** The direct bake participates in the SAME build-stage cache and determinism contract as the indirect bake: byte-identical output, index-derived jitter, no RNG, no order-dependent reduction, cache-keyed on geometry + static-light set + probe layout.
- **Storage footprint reporting.** The dense direct atlas roughly doubles dense SH atlas bytes; this is measured and logged.

### Out of scope (non-goals)

- **Runtime group-2 dynamic-direct light loop.** The reserved mesh-pipeline group 2 ("dynamic-direct") for genuinely moving / script-spawned lights is a SEPARATE, later feature. This spec handles ONLY the baked static direct that entities are missing. Group 2 stays unallocated.
- **Sharp per-mover shadows.** Baked direct is intentionally soft and probe-resolution — that is the correct ceiling for movers under a baked term. Crisp dynamic shadows are the dynamic-direct/shadow-map track's job.
- **Animated static lights' direct.** Animated direct already lives in `lm_anim` (the animated weight-map bake). Folding animated lights into the direct-at-probe bake would double-count — animated lights are excluded here.
- **`forward.wgsl` / static-surface lighting changes.** Static surfaces keep lightmap-direct + SH-indirect, byte-for-byte. The static path is not touched.
- **A version-bump rebake of existing content.** No `SH_VOLUME_VERSION` change; old maps load unchanged.

## Decisions

**D1 — Direct lives in a SEPARATE atlas; the indirect total is untouched (LOAD-BEARING).** `sh_total_atlas` is the composed indirect total (base + animated delta) and is sampled by BOTH `forward.wgsl` (static) and the dynamic shaders. Folding direct into it would leak direct into the static path, double-counting against the lightmap — the exact failure the architecture forbids (`sh_bake.rs:34`, `delta_sh_bake.rs:8-12`). Direct gets its own atlas, sampled ONLY by the dynamic shaders. `forward.wgsl` is not modified; it stays indirect-only by not being pointed at the new data. "Static path unchanged / no double-count" is an acceptance criterion.

**D2 — The per-frame delta compose path is NOT the vehicle.** The delta layer is a sparse CSR-by-affinity-cell structure for ANIMATED lights, scaled by per-frame time curves in `sh_compose.wgsl`. The direct layer is STATIC (no curve) and DENSE (all probes). It does not belong in the per-frame delta CSR loop. Bake a standalone dense direct atlas and sample it directly — two atlas samples in the dynamic fragment (indirect + direct). (Alternative considered: a compose-time pre-sum into a dynamics-only "incident" atlas. Rejected as default — see Open questions; recommend the simpler two-sample approach unless a measured reason appears.)

**D3 — Probe-point direct projection convention (CENTRAL design decision).** A probe is a point in air — there is no surface normal at bake time. "Direct at a probe" is therefore defined as: for each reaching static light, compute its incident radiance at the probe (the `light_contribution_lambert` falloff/intensity form, receiver = probe point) × `soft_visibility` shadow factor, then `accumulate_sh_rgb` that RGB value along the light's INCIDENT DIRECTION (probe→light unit vector) as a delta/cosine lobe; after all lights, `apply_cosine_lobe_rgb` so the stored coefficients are irradiance-convolved EXACTLY as the indirect bake stores them. At runtime the existing cosine-lobe convolution + the receiver fragment's own normal produce the per-fragment response — same sampler (`sample_sh_indirect_corners_depth_aware`), same math. Rationale: this is what makes the direct term reconstruct as cosine-weighted irradiance the receiver normal can read; getting the lobe direction or the normalization wrong is exactly what breaks brightness parity with the lightmapped room (see Open questions — owner must confirm the lobe form).

**D4 — Same static-light set/order as the lightmap baker; static-only.** The direct-at-probe bake iterates the SAME `StaticBakedLights::from_lights` set in the SAME order the lightmap and indirect bakes consume, and ONLY static (`shadow_type == StaticLightMap`, non-animated, non-dynamic) lights. This keeps direct-at-probe parity with the lightmapped room and avoids re-introducing the animated double-count (`lm_anim` already owns animated direct).

**D5 — New section id, no version bump; identical tile geometry.** Add a NEW `SectionId` mirroring `OctahedralShVolume = 34` / `DeltaShVolumes = 27`, rather than bumping `SH_VOLUME_VERSION` (7) — a version bump hard-rejects existing v7 `.prl` files and forces a full content rebake. The direct atlas uses `DEFAULT_IRRADIANCE_TILE_DIMENSION`/`DEFAULT_IRRADIANCE_TILE_BORDER` (the indirect tile geometry) so `sh_sample.wgsl`'s sampler math is reused unchanged. A map without the section degrades to direct = 0 (entities fall back to indirect-only, today's behavior).

**D6 — Cache + determinism are LOCKED contracts.** The direct bake honors the indirect bake's determinism rules: byte-identical output for identical inputs; soft-visibility jitter index-derived (`soft_visibility_seed`-style, perturbing sampling never geometry); no RNG; order-preserving fan-out (no float `reduce`, no HashMap-iteration to assemble output). It cache-keys on the same input kinds (geometry + static-light set + probe layout) and slots into the existing warm/cold driver paths.

## Acceptance criteria

### Visual / behavioral (eyeball in-engine, dev-tools)
- [ ] A mesh entity and a billboard placed in a directly-lit room read at a brightness that visibly MATCHES the lightmapped static geometry around them — not the dim indirect-only baseline they show today.
- [ ] An entity in a directly-lit cell is brighter than the same entity in a shadowed cell behind an occluder (baked shadow-ray visibility is present in the direct term).
- [ ] The debug toggle cycles direct-only / indirect-only / combined for the dynamic path; direct-only on a dynamic object reads as the room's direct brightness, indirect-only reads as today's dim baseline, combined sums them.

### Correctness / no-regression (test-gated where possible)
- [ ] `forward.wgsl` is unmodified and static-surface output is byte-identical to before this change (no direct double-count on lightmapped surfaces). A static surface's rendered color does not change.
- [ ] The shared indirect total atlas (`sh_total_atlas`) content is unchanged — the direct data is in a separate atlas/section.
- [ ] A map compiled WITHOUT the new section (legacy v7 `.prl`) loads and renders; dynamic objects fall back to indirect-only with no error. The loader does not reject it (no `SH_VOLUME_VERSION` change).
- [ ] Only static lights contribute to the direct atlas: a map whose only lights are animated produces a zero (or absent) direct section; no animated light's direct double-counts against `lm_anim`.

### Bake determinism / cache
- [ ] The direct bake produces byte-identical output across two runs on identical inputs (mirrors `sh_volume_bake_produces_byte_identical_output_on_repeated_runs`).
- [ ] A second compile of an unchanged map serves the direct section from the build-stage cache (cache hit); changing geometry, the static-light set, or probe layout invalidates it and rebakes.

### Format / footprint
- [ ] A new `SectionId` variant is registered and round-trips (encode → decode → equal); it does not reuse or collide with existing ids.
- [ ] The compiler logs the direct atlas byte footprint; the figure reflects the dense (all-probe) cost and is reported in the PR alongside the indirect atlas size for comparison.

### Gates
- [ ] `skinned_mesh.wgsl` and `billboard.wgsl` pass naga validation.
- [ ] `cargo test` passes (workspace); `cargo clippy -- -D warnings` clean.

## Tasks

### Task 1: New PRL section — direct octahedral atlas (`level-format`)
Add a new direct-SH section type in `crates/level-format/src` (a sibling to `sh_volume::OctahedralShVolumeSection`) carrying a dense per-probe octahedral irradiance atlas with tile geometry IDENTICAL to the indirect atlas (`DEFAULT_IRRADIANCE_TILE_DIMENSION`/`_BORDER`, same `irradiance_atlas_dimensions`/`tiles_per_row` derivation) plus the grid dims/origin/cell-size needed to bind the same `ShGridInfo`-shaped uniform. Register a new `SectionId` variant in `level-format/src/lib.rs` mirroring `OctahedralShVolume = 34` and `DeltaShVolumes = 27` (next free id; add the decode arm). Do NOT touch `SH_VOLUME_VERSION` (stays `7`). Provide `to_bytes`/`from_bytes` with a round-trip test, mirroring the existing section's encode/decode discipline. The Wire format section pins endianness/ordering. Because the runtime sampler (`sh_sample.wgsl`) is reused verbatim, the per-probe tile byte layout MUST match the indirect atlas tile exactly.

### Task 2: Direct-at-probe SH bake + cache integration (`level-compiler/src/sh_bake.rs`)
Add a direct-at-probe bake that, per probe, iterates the `StaticBakedLights` set (D4) and for each reaching static light: computes incident radiance at the probe (reuse the `light_contribution_lambert` falloff/intensity form with the probe as receiver), multiplies by `soft_visibility` (the SAME shadow-ray routine the lightmap + indirect bakes use, `segment_clear` for the hard case), and `accumulate_sh_rgb`s that RGB along the light's incident direction (probe→light) per the D3 convention; after all lights, `apply_cosine_lobe_rgb`, then `pack_octahedral_irradiance_tile` into the Task 1 atlas. Reuse `ProbeGridLayout` so the direct probes land at byte-identical positions/validity to the indirect probes (no second grid). Honor the determinism contract (D6): index-derived jitter only (`soft_visibility_seed`-style), order-preserving `into_par_iter().map().collect()` fan-out, no RNG, no HashMap-iteration to assemble output — add the byte-identical-output test mirroring `sh_volume_bake_produces_byte_identical_output_on_repeated_runs`. Slot into the build-stage cache the same way the indirect bake does, cache-keyed on geometry + static-light set + probe layout (the warm `sh_group`/cold `sh_bake` split at `main.rs:518-531` is the integration shape — the direct bake either rides the existing cache entry or adds a sibling cache entry keyed on the same inputs; pick one and document it). Excludes animated/dynamic lights at the filter, not downstream.

### Task 3: Driver wiring — invoke the bake, emit the section (`level-compiler` `main.rs` + `pack.rs`)
Invoke the Task 2 direct bake in the compile driver alongside the existing SH/delta bakes (`main.rs` ~518-550), timed and `--verbose`-logged with the byte-footprint line (footprint AC). Thread the new section through `pack.rs` `pack_level` (new param mirroring `sh_volume: &OctahedralShVolumeSection` / `delta_sh_volumes: Option<&...>`) and emit it with the Task 1 `SectionId`, mirroring the `OctahedralShVolume`/`DeltaShVolumes` emit blocks (`pack.rs:436`/`:469`). Empty/absent (no static lights) emits no section — the loader treats absence as direct = 0.

### Task 4: Renderer loader + bind for the direct atlas (`render/sh_volume.rs`, `render/mod.rs`)
Load the new section at level load and create a GPU texture + view for the direct atlas, mirroring how `ShVolumeResources::new` builds the indirect atlas (`sh_volume.rs:270`). Expose it on a bind group the DYNAMIC pipelines can bind WITHOUT consuming the reserved group-2 dynamic-direct slot. Preferred: add the direct atlas texture as an additional binding inside the EXISTING SH resources group (mesh group 4 / forward+billboard group 3) — `forward.wgsl` simply doesn't declare it, so the static path is unaffected and the group stays shared (BGL may carry entries a shader ignores; `research.md` confirms the precedent). When the section is absent, bind a dummy 1×1 (the `ShVolumeResources::new` `None` path already does this for the indirect atlas) so the dynamic shaders read direct = 0. Update visibility flags on the new BGL entry to cover the consuming stages (`FRAGMENT`, plus `COMPUTE` only if a compose variant is chosen). Honor the §10/§9 group budget — no new group if the shared SH group has room.

### Task 5: Entity + billboard shaders sample indirect + direct (`skinned_mesh.wgsl`, `billboard.wgsl`)
Declare the direct atlas binding (Task 4 slot) and append the SAME `sh_sample.wgsl` helper read against it (the helper is binding-agnostic — §8 shader composition). In `skinned_mesh.wgsl` `fs_main`, compute `direct = sample_sh_direct(world_pos, n, n)` alongside the existing `indirect` and return `base_color.rgb * (indirect + direct)`. Mirror in `billboard.wgsl` `fs_main` — BUT first verify the billboard SH read mirrors `skinned_mesh.wgsl`'s structure (its `sample_sh_indirect` takes one normal arg, `reject_backface = false`; confirm before assuming the same change shape — Open questions). The two atlas samples are the runtime cost (D2). Keep `reject_backface = false` and Chebyshev on for the direct sample, matching the dynamic-object indirect precedent (entities are not static surfaces). `forward.wgsl` is NOT edited.

### Task 6: Debug toolbar control + uniform plumbing (`render/mod.rs`, `render/debug_ui/mod.rs`, shaders)
Add a dynamic-direct scale (0–1, default 1.0) and an isolation toggle (direct-only / indirect-only / combined) for the dynamic path, analogous to the existing `indirect_scale` slider (`debug_ui/mod.rs:170-173`, `set_indirect_scale` `render/mod.rs:3669`) and `LightingIsolation` dropdown (`debug_ui/mod.rs:184-194`). Because the dynamic shaders read a TRIMMED `CameraUniforms { view_proj }` (`skinned_mesh.wgsl:41`) — NOT the full forward `Uniforms` that carries `indirect_scale` — plumb the scale + isolation mode to the dynamic path via a small dedicated uniform (or by extending the mesh/billboard uniform), with setter/getter on the renderer mirroring `set_indirect_scale`. The dynamic fragment selects: combined → `indirect + scale*direct`; direct-only → `scale*direct`; indirect-only → `indirect`. The control is `dev-tools`-gated like the existing knobs. This is the parity-verification instrument for the visual ACs.

### Task 7 (optional, deferred unless justified): compose-time pre-sum variant
Only if Task 5's two-sample read measures as a real cost: a compute pre-sum that adds the dense direct atlas into a dynamics-ONLY "incident" atlas (never the shared indirect total) so the dynamic fragment does ONE sample. Adds a compute pass + a second atlas; the simpler two-sample (D2) is the default. Carries the same "never touch `sh_total_atlas`" constraint. Not built unless the measurement justifies it.

## Sequencing

**Phase 1 (sequential):** Task 1 — the section type all later tasks read/write.
**Phase 2 (sequential):** Task 2 — the bake; consumes Task 1's section type and cache shape.
**Phase 3 (sequential):** Task 3 — driver/pack emit; consumes Task 2's bake + Task 1's `SectionId`.
**Phase 4 (concurrent):** Task 4, Task 5, Task 6 — loader/bind (4), shader sampling (5), debug control (6); all consume the section/atlas but are independent surfaces. (5 and 6 both edit the dynamic shaders — serialize those two edits if they touch the same fragment lines.)
**Phase 5 (optional):** Task 7 — only if Phase 4 measurement justifies the pre-sum.

## Boundary inventory

| Name | Rust | Wire / serde | WGSL |
|---|---|---|---|
| Direct SH section | new `SectionId` variant + section struct (`level-format`) | new section id (next free, mirrors `OctahedralShVolume`=34) | n/a |
| Direct atlas texture | renderer texture/view on the SH resources group | dense octahedral atlas bytes (same tile geometry as indirect) | `sh_direct_atlas` (dynamic shaders only) |
| Dynamic direct scale | renderer field + `set_direct_scale`-style setter | n/a (engine-internal uniform) | scale field in the mesh/billboard uniform |
| Dynamic isolation mode | renderer enum (mirrors `LightingIsolation`) | n/a | mode field in the mesh/billboard uniform |

## Wire format

The new direct section mirrors `OctahedralShVolumeSection` exactly except it carries direct (not indirect) coefficients and NO depth moments / animation data (direct is static and dense): little-endian; the same grid header (dims, origin, cell-size); a dense per-probe octahedral tile block in x-fastest linear probe order; per-probe tile byte layout BYTE-IDENTICAL to the indirect atlas tile (the runtime sampler is shared). Empty-list / no-static-lights encoding: the section is omitted entirely (loader treats absence as direct = 0), matching how `DeltaShVolumes` is `Option` and skipped when no animated lights exist. State explicitly in the implementation which existing section the byte layout mirrors.

## Rough sketch

- **Reuse, don't reinvent:** `light_contribution_lambert` (`sh_bake.rs:555`), `soft_visibility` (`lightmap_bake.rs:1227`) / `segment_clear` (`sh_bake.rs:517`), `accumulate_sh_rgb` (`sh_bake.rs:666`), `apply_cosine_lobe_rgb` (`sh_bake.rs:781`), `pack_octahedral_irradiance_tile` (`sh_bake.rs:728`), `ProbeGridLayout` (`sh_bake.rs:108`). The bake is assembly of these.
- **Projection (D3):** per static light reaching probe `p`: `dir = normalize(light_origin - p)`; `radiance = direct_intensity(light, p) * soft_visibility(p, light)`; `accumulate_sh_rgb(&mut acc, dir, radiance, 1.0)`. After the light loop: `apply_cosine_lobe_rgb(&mut acc)` → `pack_octahedral_irradiance_tile(&acc, valid, TILE_DIM, BORDER)`. Stored coeffs reconstruct irradiance directly (cosine-lobe already applied), so `sh_sample.wgsl` needs no per-fragment `A_l`.
- **Group map (mesh):** 0 camera · 1 material · **2 reserved (dynamic-direct, stays empty)** · 3 instance · 4 SH atlas (indirect + NEW direct binding). The direct binding rides the shared SH group, so group 2 is untouched.
- **Determinism:** copy the indirect bake's fan-out shape verbatim — `into_par_iter().map().collect()`, index-derived `soft_visibility_seed`. Add the byte-identical test.

## Open questions

- **Lobe form / normalization (OWNER MUST CONFIRM — highest risk).** D3 projects each light as a cosine lobe along its incident direction then applies `apply_cosine_lobe_rgb`. Is a single delta-along-direction + cosine-lobe convolution the right reconstruction for a point light at a probe, or does the incident radiance need a different normalization (e.g. matching the indirect path's per-ray solid-angle weighting) to hit brightness parity with the lightmap? Getting this wrong is the primary parity failure mode. Recommend a small A/B harness: bake one probe directly under one light, compare reconstructed irradiance at a known normal against the lightmap's value at the same point/normal.
- **Storage doubling.** The dense direct atlas roughly doubles dense SH atlas bytes (no CSR savings — direct reaches all probes). Acceptable for the target maps? The footprint AC measures it; the owner decides whether a compression path (e.g. BC6H at rest, as the lightmap irradiance atlas already does per §4) is in scope now or deferred.
- **Bake-time cost.** Direct adds a per-static-light shadow-ray pass at every probe. On light-dense maps this could dominate SH bake time. Measure and report; decide whether per-probe light-reach culling (à la the delta affinity-cell clip) is needed in v1 or a follow-up.
- **Billboard shader parity.** Verify `billboard.wgsl`'s SH read mirrors `skinned_mesh.wgsl` before assuming the same edit shape — the billboard `sample_sh_indirect` takes one normal arg and uses camera-forward as the normal; confirm the direct sample slots in the same way (Task 5).
- **Where the direct binding lives.** Task 4 prefers a binding inside the shared SH group (so group 2 stays free). Confirm the shared BGL can carry the extra entry without forcing `forward.wgsl` to declare it (it can — bind groups may expose entries a shader ignores), and that visibility flags don't over-broaden.
