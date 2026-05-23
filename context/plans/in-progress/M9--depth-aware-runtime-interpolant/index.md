# Depth-Aware Runtime Interpolant

## Goal

Replace the current validity/backface-only SH probe blend with a DDGI-style
depth-aware runtime interpolant. The runtime consumes the baked per-probe depth
moments already stored in the `ShVolumeSection` payload for
`SectionId::ShVolume` and weights each probe corner by Chebyshev visibility,
reducing indirect-light bleed through walls for static world surfaces and
dynamic billboard entities.

Milestone 9 spec #3. Depends on shipped M9 #1 (manual corner blend and validity
weighting) and M9 #2 (baked probe depth moments).

## Scope

### In scope

- Upload the existing `ShProbe.mean_distance` / `mean_sq_distance` values into
  a renderer-owned GPU resource that shares the SH probe grid.
- Extend the shared SH WGSL helper to include a Chebyshev visibility term in
  the existing 8-corner weight.
- Apply the depth-aware path to forward world shading and billboard sprite
  shading through the single shared helper.
- Preserve missing-SH behavior: no SH section still binds dummy resources and
  returns zero indirect SH.
- Keep the current validity and forward-only backface weighting. Chebyshev
  multiplies those weights; it does not replace them.
- Add regression coverage for packing, binding layout, WGSL validation, and
  visibility-weight math.
- Record a before/after visual check on a leak-prone map using the existing
  StaticSHOnly diagnostics mode.

### Out of scope

- New PRL sections or ShVolume wire-format changes. The moments are already in
  section 20.
- New bake data, directional probe depth maps, octahedral visibility maps, or
  a re-bake of SH coefficients.
- Directional fog. Fog must keep rendering, but its directional scattering
  model is the next M9 slot.
- Probe streaming or brick splitting.
- Replacing the SH compose pass or animated-lightmap compose pass.
- Runtime ray tracing, screen-space GI, or clustered light binning.

## Acceptance criteria

- [ ] Static world surfaces in StaticSHOnly mode show visibly reduced
      through-wall indirect light bleed on the chosen leak-prone test map.
- [ ] Billboard sprite particles use the same depth-aware SH visibility path
      as world geometry.
- [ ] Missing SH volume still renders without validation errors and contributes
      zero SH indirect, as before.
- [ ] Invalid probes still contribute zero weight even if their depth moments
      are zero.
- [ ] A corner whose sample point is closer than the probe's mean distance
      remains fully visible except for existing validity/backface weighting.
- [ ] A corner whose sample point is beyond the probe's mean distance is
      smoothly attenuated by the moment variance. No hard popping at cell
      boundaries.
- [ ] Any minimum visibility floor is low enough to prevent f16 moment-noise
      blackouts without masking the intended through-wall leak reduction.
- [ ] The runtime has no plain-trilinear SH sampling path and no parallel
      non-depth-aware path for forward or billboard shading. A fog-only
      compatibility path may keep SH depth visibility disabled until the
      directional fog plan.
- [ ] Shader validation passes for forward, billboard, and fog shader modules.
- [ ] A before/after note records map, camera pose, diagnostics mode, commit,
      and qualitative residual leak in
      `context/plans/drafts/M9--depth-aware-runtime-interpolant/measurements/static-sh-only.md`.

## Tasks

### Task 1: Upload probe depth moments to group 3

Add a depth-moment texture to `ShVolumeResources`. Build it from the existing
`ShVolumeSection.probes` array at level load and reload. Use the same
dimensions, origin, cell size, and z-major/y/x probe order as the SH bands.
Pack `mean_distance` and `mean_sq_distance` into the red and green channels of
an `Rgba16Float` 3D texture; leave the remaining channels zero. Missing or
zero-dimension SH sections bind a 1x1x1 zero texture. Add the texture view to
the group-3 bind group and bind group layout after the current scripted-light
descriptor binding. Keep visibility `FRAGMENT | COMPUTE`: forward and
billboard consume group 3 from fragment stages, while fog shares the same
group-3 layout from its compute raymarch.

Plumbing: `ShVolumeResources::new` already receives the decoded
`ShVolumeSection`, creates the SH band textures, and installs the group-3 bind
group. Extend that constructor rather than adding a second owner. The forward,
billboard, and fog pipeline layouts already consume
`sh_volume_resources.bind_group_layout`; updating the layout in one place
updates all three consumers. The SH compose pass does not need this texture
because depth moments are static per probe and are not affected by animated
SH deltas.

### Task 2: Add Chebyshev visibility to the shared SH helper

Extend `sh_sample.wgsl` so `sample_sh_indirect_corners` loads the moment texture
for each corner. Reconstruct the probe position from the clamped corner index,
`sh_grid.grid_origin`, and `sh_grid.cell_size`. Compute distance from the
probe to the same world-space sample point that produced `gi` and `gfrac`.
Then evaluate one-tailed Chebyshev visibility:

```wgsl
// Proposed design.
let mean = moments.r;
let mean2 = moments.g;
let variance = max(mean2 - mean * mean, MIN_VARIANCE);
let delta = max(distance - mean - DEPTH_BIAS, 0.0);
let visibility = select(1.0, variance / (variance + delta * delta), delta > 0.0);
```

Multiply the existing corner weight by this visibility:
`trilinear * validity * backface * visibility`. Keep the current
renormalization over surviving weights. Clamp the final visibility into
`[MIN_VISIBILITY, 1.0]` only for valid corners so narrow geometry does not
collapse to black from f16 moment noise. Start with conservative constants:
small variance floor in squared meters, a depth bias proportional to the
smallest probe cell axis, and a low minimum visibility floor. Treat these as
implementation constants with tests that pin behavior, not user-facing
settings.

Plumbing: today callers pass only `gi` and `gfrac`. Change the helper signature
to also receive the world-space point being sampled for SH. Forward already
computes `offset_world`; pass that value. Billboard passes its current
`in.world_position` / sprite-center sample point, preserving the existing
per-particle SH sampling model. Fog must use an explicit helper entry point or
boolean that leaves Chebyshev depth visibility disabled for this plan. The
helper still clamps probe indices only for texture loads and probe position
reconstruction; it does not clamp the SH sample point before computing
probe-to-sample distance. Directional fog remains out of scope.

### Task 3: Adopt the updated helper in forward, billboard, and fog safely

Update group-3 declarations in `forward.wgsl`, `billboard.wgsl`, and
`fog_volume.wgsl` to declare the new moment texture at the chosen binding.
Update the local wrapper functions to pass the world-space sample position into
`sample_sh_indirect_corners`. Forward keeps normal offset and backface
rejection. Billboard keeps no normal offset and no backface rejection. Fog work
is compatibility-only: keep the fog shader compiling against the shared group-3
layout and helper contract, preserve its current fixed world-up SH evaluation
and no-normal-offset behavior, and route it through the explicit depth-disabled
fog compatibility path. No directional fog term is added.

Plumbing: `render/mod.rs`, `render/smoke.rs`, and `render/fog_pass.rs` already
append `sh_sample.wgsl` to the consumer shader source. Do not duplicate the
helper in individual shaders. If the fog call makes item #3 too broad during
implementation, add a helper entry point or boolean that preserves current fog
behavior without adding directional fog.

### Task 4: Tests, diagnostics, and measurement

Add Rust-side tests for the CPU moment packing helper before GPU upload: valid
probes preserve the f16 moment bits in RG, invalid probes pack zero moments,
and dummy resources exist when the SH section is absent. Extend group-3 layout
tests so shader bindings and Rust bindings agree. Add a CPU reference test for
the Chebyshev visibility function: fully visible before the mean distance,
smoothly lower past the mean, stable under zero variance, and zero contribution
for invalid probes. Keep WGSL parse/validation tests green for concatenated
forward, billboard, and fog sources.

Run a visual check on a leak-prone map in StaticSHOnly mode. Record the
before/after result in
`context/plans/drafts/M9--depth-aware-runtime-interpolant/measurements/static-sh-only.md`
with map, camera pose, diagnostics mode, commit, and qualitative residual leak.
GPU timing is useful when available, but visual correctness is the gate for this
spec.

## Sequencing

**Phase 1 (sequential):** Task 1 — creates the GPU data and binding all shader work consumes.
**Phase 2 (sequential):** Task 2 — changes the shared helper contract and visibility math.
**Phase 3 (sequential):** Task 3 — updates all current helper consumers to the new contract.
**Phase 4 (sequential):** Task 4 — verifies layout, math, shader validation, and visual result.

## Rough sketch

- Format source: `crates/level-format/src/sh_volume.rs::ShProbe` already carries
  `mean_distance` and `mean_sq_distance` as f16 bits. Do not change
  `SH_VOLUME_VERSION` or `PROBE_STRIDE`.
- Runtime owner: `crates/postretro/src/render/sh_volume.rs::ShVolumeResources`
  owns group 3. Add a moment texture/view field there. The constructor already
  handles real and dummy SH volume resources.
- Packing helper: add a small sibling to `pack_probes_to_band_slices` that emits
  `[mean_distance, mean_sq_distance, 0, 0]` per probe as `u16` RGBA texels.
  Invalid probes should pack all zeroes, matching the bake contract.
- Binding choice: use the next group-3 binding after
  `BIND_SCRIPTED_LIGHT_DESCRIPTORS`. Keep binding 0 vacant.
- Shader helper: `crates/postretro/src/shaders/sh_sample.wgsl` is the only place
  to implement the depth-aware blend. It already owns `sh_irradiance` and
  `sample_sh_indirect_corners`.
- Forward wrapper: `crates/postretro/src/shaders/forward.wgsl::sample_sh_indirect`
  computes `offset_world`; pass it into the helper.
- Billboard wrapper: `crates/postretro/src/shaders/billboard.wgsl::sample_sh_indirect`
  samples at the existing entity/particle center world position with no
  surface-normal offset; do not change billboard interpolation to per-corner
  world positions in this plan.
- Fog wrapper: `crates/postretro/src/shaders/fog_volume.wgsl::sample_sh_fog`
  remains ambient SH sampling only. Update only what is needed for group-3
  binding/helper compatibility. Do not add directional scattering.
- Pipeline composition: `SHADER_SOURCE`, `BILLBOARD_SHADER_SOURCE`, and
  `FOG_SHADER_SOURCE` already append `sh_sample.wgsl`; keep that pattern.

## Boundary inventory

| Name | Rust / PRL source | GPU upload | WGSL |
|---|---|---|---|
| Probe validity | `ShProbe.validity` | band-0 alpha in SH band texture | `t0.a >= 0.5` |
| Mean distance | `ShProbe.mean_distance` f16 bits | depth-moment texture R channel | moment load `.r` |
| Mean squared distance | `ShProbe.mean_sq_distance` f16 bits | depth-moment texture G channel | moment load `.g` |
| Probe grid | `ShVolumeSection.grid_origin`, `cell_size`, `grid_dimensions` | `ShGridInfo` uniform | `sh_grid` |
| SH coefficients | `ShProbe.sh_coefficients` | 9 `Rgba16Float` band textures | `sh_band0..sh_band8` |

No JS, Luau, FGD, or PRL wire names are added.

## Open questions

- **Tuning constants.** Pick exact variance floor, depth bias, and minimum
  visibility during implementation. They are renderer constants, not scripting
  API.
- **Fog compatibility.** Fog shares `sh_sample.wgsl`. Treat fog as
  compatibility-only in this plan: keep validation and rendering stable while
  deferring the directional fog lighting model to the next M9 plan.
- **Measurement map.** Prefer `content/dev/maps/occlusion-test.map` if it
  still shows residual smear after latest code. Use
  `content/dev/maps/campaign-test.map` as fallback if the original pose no
  longer demonstrates the leak.
