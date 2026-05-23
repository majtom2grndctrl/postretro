# Probe Weight Correctness (no new data)

## Goal

Fix indirect-light artifacts from the baked SH irradiance volume in the world shader. Reject trilinear corner probes that face away from the surface normal, exclude invalid (zero-packed) probes from the blend, and renormalize the surviving corner weights. Record a residual-smear/leak baseline on a representative map before the depth atlas (spec #2) is built — that before/after delta justifies the atlas. No new baked data.

This is Milestone 9 spec #1. It fixes a latent darkening bug that is independent of the future DDGI work and is a prerequisite that work needs anyway: invalid probes are currently packed as zero and blended in via the hardware trilinear sampler, dragging near-wall surfaces toward black.

## Scope

### In scope

- Replace the hardware-trilinear SH fetch in `forward.wgsl` with a manual 8-corner blend that drops backfacing and invalid corners and renormalizes the rest.
- Decide and apply the same correction (or a justified subset) to the two copy-pasted SH samplers in `billboard.wgsl` and `fog_volume.wgsl`.
- A measurement gate: record a before/after artifact of residual indirect smear/leak on a representative map, in a location the depth-atlas spec (#2) can consume.

### Out of scope

- Probe depth/visibility atlas (spec #2) — no per-probe depth moments, no BVH ray-cast bake.
- Chebyshev / DDGI visibility-weighted interpolant (spec #3).
- Directional fog (spec #4).
- Any new or extended PRL section. The validity-channel option (open question 1) is a packing change to existing band textures, not a new section — but if it requires a PRL format change it falls out of scope and defers to spec #2.
- Consolidating the three SH-sampler copies into one shared WGSL helper (raise as open question 2; default is to leave the copies separate unless trivially shareable).

## Acceptance criteria

- [ ] On the measurement map, a near-wall surface that darkens under the current build no longer darkens versus a captured baseline screenshot, viewed in the StaticSHOnly isolation mode.
- [ ] A surface lit only by an all-zero (invalid) corner probe is no longer pulled toward black by that corner; its indirect value comes only from valid, front-facing corners.
- [ ] When all eight corners are rejected or invalid, the indirect term degrades to a defined fallback (the ambient floor, matching the existing `has_sh_volume == 0` path) — no division-by-zero, no NaN, no black flash.
- [ ] A corner whose direction from the sample point opposes the surface normal contributes zero weight; rotating a test surface's normal changes which corners contribute.
- [ ] Visual output is unchanged from baseline in open areas where all eight corners are valid and front-facing (the fix is a no-op where no corner is rejected).
- [ ] The billboard and fog SH paths render without regression after the Task 2 decision is applied; any intentional divergence from the forward path is documented in the spec, not silently dropped.
- [ ] A before/after residual-smear measurement (screenshot pair and/or scalar metric) is recorded at the agreed location, with the map name, camera pose, and isolation mode noted, such that spec #2 can read the delta without re-deriving it.

## Tasks

### Task 1: Manual 8-corner SH blend in forward.wgsl

Replace the hardware-trilinear fetch with a manual blend. Today `sample_sh_indirect_fast(normal, gi, gfrac)` lands one UVW between eight texel centers and issues nine `textureSampleLevel` calls (one per band); the linear sampler does the 8-corner blend implicitly. To reject or reweight individual corners the hardware sampler cannot be used — switch to manual per-corner fetches. For each of the 8 corners: load all 9 bands at that integer grid index (8 corners x 9 bands = 72 texel fetches vs. today's 9 hardware samples), compute the trilinear corner weight from `gfrac`, then zero the weight if (a) the corner is invalid or (b) the corner direction from the sample point faces away from the surface normal. Sum the surviving weights; divide each by the sum (renormalize); if the sum is ~0, return the ambient-floor fallback. Reconstruct irradiance with the existing `sh_irradiance` reconstruction.

Plumbing: `sample_sh_indirect` already computes `gi` (integer grid index) and `gfrac` and passes the world-space `N_bump` normal — both inputs the corner-rejection math needs are already in hand. Invalid-corner detection has no GPU signal today (see open question 1); the working assumption is "L0/DC band exactly zero ⇒ invalid corner" using `sh_band0`, pending the resolution of question 1. Corner positions for the backface test derive from `gi`, the corner offset, `sh_grid.cell_size`, and `sh_grid.grid_origin` (already in the group-3 uniform). Sampling individual corners requires integer texel loads rather than the linear sampler; the existing `sh_sampler` is linear, so the manual path must fetch by texel coordinate (e.g. `textureLoad`) instead.

### Task 2: Decide and apply across the billboard and fog SH copies

The SH sampler is copy-pasted, not shared: `billboard.wgsl` carries its own `sh_irradiance` + `sample_sh_indirect` (normal `N = V`, camera-forward), and `fog_volume.wgsl` carries `sh_irradiance` + `sample_sh_indirect_fast` with a fixed `vec3(0, 1, 0)` world-up normal and no normal offset (it explicitly notes the forward wall-bleed mitigation has no meaning in fog). Decide per copy: does the invalid-probe exclusion + renormalization apply, and does the backface corner-rejection apply given each path's normal semantics? The invalid-probe exclusion is a correctness fix everywhere (zero-packed corners are wrong everywhere). The backface rejection is meaningful only where the normal is a real surface normal — fog's fixed up-normal and the billboard's camera-forward normal both warrant an explicit decision, not a blind copy. Apply the decided behavior to each copy, or document why a copy is left unchanged.

Plumbing: each copy already binds group 3 and has the same `gi`/`gfrac` derivation inline; whichever corners-and-weights logic Task 1 lands must be transcribed (or the agreed subset of it) into each copy, since there is no shared helper to edit once.

### Task 3: Measurement gate — record residual smear/leak

With the Task 1 fix in place, capture the residual indirect smear/leak baseline before spec #2 exists. Pick the leak-prone map (`content/dev/maps/occlusion-test.map`, compiled to `.prl`; the spec author confirms it exposes a near-wall darkening / through-wall bleed corner — otherwise fall back to `campaign-test.map`). Use a fixed camera pose framing the worst near-wall/through-wall corner. Capture in the StaticSHOnly isolation mode (pure SH indirect, no specular) so the SH contribution is isolated; the IndirectOnly mode is the secondary view if specular interaction matters. Record both a before image (current build) and an after image (with Task 1), plus a residual-smear scalar if one can be cheaply derived from the existing `ShProbeReadback` L0/DC readback or from pixel sampling. Store the artifacts and the metadata (map, camera pose, isolation mode, build/commit) at an agreed location that spec #2 reads — default proposal: a `measurements/` subfolder beside this plan, since spec #2 lives in the same drafts tree (confirm via open question 4).

Plumbing: isolation modes are already wired (StaticSHOnly = mode 6, IndirectOnly = mode 3, set via `LightingIsolation`); the SH debug panel (`show_markers` with `MarkerMode::Validity`, dev-tools feature) and `ShProbeReadback` already exist for inspecting validity and per-probe L0. No new diagnostic plumbing is required for an image-pair capture; a scalar metric may reuse the readback.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the corner-rejection/renormalization logic the other tasks depend on.
**Phase 2 (sequential):** Task 2 — transcribes the agreed logic into the billboard/fog copies; depends on Task 1's final shape.
**Phase 3 (sequential):** Task 3 — measures with the fix in place; consumes Task 1's behavior and needs the before/after delta to be meaningful.

## Rough sketch

- Forward path: `sample_sh_indirect_fast` in `crates/postretro/src/shaders/forward.wgsl` (~L367) becomes a manual blend. Loop 8 corners; per corner load `sh_band0..sh_band8` by integer texel (linear `sh_sampler` cannot be used for per-corner control). Trilinear weight from `gfrac`; gate by validity (L0/DC zero test on `sh_band0`) and by `dot(corner_dir, normal) > 0`; accumulate `Σ w_i * sh_irradiance(corner_bands, normal)` and `Σ w_i`; divide; fall back to ambient floor when `Σ w_i ≈ 0`.
- `sh_irradiance` (~L344) reconstruction is reused unchanged.
- Group-3 bindings (`sh_sampler`@0, `sh_band0..8`@1..9, `sh_grid`@10) are unchanged unless open question 1 rides a validity flag in an existing band's unused alpha channel.
- Baker `crates/level-compiler/src/sh_bake.rs` (~L137-150) sets `validity` and zero-fills invalid probes; packer `crates/postretro/src/render/sh_volume.rs` `pack_probes_to_band_slices` (~L513) drops `validity == 0` probes to zero. Alpha is currently written as 0 and unused — the candidate validity carrier for question 1.
- Diagnostics: `crates/postretro/src/render/sh_diagnostics.rs` (`MarkerMode::Validity`, `ShProbeReadback`); isolation modes in `crates/postretro/src/render/mod.rs` (`LightingIsolation`).

## Open questions

1. **Zero-packed valid-vs-invalid ambiguity.** On the GPU there is no validity channel in the sampled band textures — an invalid probe is indistinguishable from a valid probe with genuinely zero irradiance. "Exclude invalid corners" literally means "treat all-zero-L0 corners as invalid," which would also wrongly drop a legitimately pitch-black-but-valid probe. Options: (i) accept the edge case (dark valid probes are rare and the cost is small); (ii) ride a validity flag in the currently-unused alpha channel of an existing band texture — a packing change to existing textures, not a new PRL section; argue whether this still counts as "no new data"; (iii) other. The L0/DC band (`sh_band0`) being exactly zero is the practical "is this corner real" test under option (i).
2. **Scope across the three shader copies.** Apply to all three (forward, billboard, fog), or only where a real surface normal exists? And consolidate the three copies into one shared WGSL helper (per the §8 string-concat pattern), or leave them separate? Consolidation is possible but may be scope creep.
3. **72-fetch cost.** The manual blend is 8 corners x 9 bands = 72 texel fetches versus today's 9 hardware-trilinear samples. The handoff calls this "pure ALU" — that is inaccurate; the fetch cost is real. Is it acceptable on the target hardware, and should the measurement gate record the per-pass GPU time delta (`POSTRETRO_GPU_TIMING=1`, `forward` pass) alongside the visual delta?
4. **Measurement gate storage.** Exactly what does the gate record — an image pair, a scalar smear metric, or both — and where does it live so spec #2 can consume it without re-deriving? Default proposal: a `measurements/` subfolder beside this plan.
