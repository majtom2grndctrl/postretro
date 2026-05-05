# Fog SH Color

## Goal

Replace the fixed per-volume fog color with full L2 SH irradiance sampled at each raymarch position. Fog inherits ambient environment color from the same SH volume that lights world geometry, eliminating per-volume color authoring and ensuring visual coherence with the surrounding scene.

## Scope

### In scope

- Fog ambient scatter driven by full L2 SH reconstruction at each sample position (all 9 bands, fixed neutral normal)
- Spot beam scatter loses the per-volume color tint; beams render at their natural `spot.color`
- `color` KVP removed from FGD fog entities (`fog_volume`, `fog_lamp`, `fog_tube`)
- `color` field removed end-to-end: PRL wire format, `FogVolumeRecord`, GPU struct, CPU packing code, and shader `VolumeSample` accumulation

### Out of scope

- Per-volume color tint multiplier layered on top of SH (not needed; mappers control hue via light placement)
- Changes to how dynamic spot/point light intensities or falloff work in the fog pass
- SH volume baking changes (no new probe channels needed)
- Animated SH compose pass changes

## Acceptance criteria

- [ ] In a level with colored SH lighting, fog visually reflects the local ambient color at its position — warm rooms produce warm fog, cool rooms produce cool fog, without any `color` KVP set on the fog entity.
- [ ] Fog in a scene with no SH volume (`has_sh_volume == 0`) produces no ambient scatter contribution (same fallback as the current L0 path when the SH volume is absent).
- [ ] Spot beam scatter through fog shows the beam's natural color, unmodulated by a per-volume tint.
- [ ] Point light scatter through fog is unchanged. (The point-light accumulation uses `pt.color` directly, not `vs.color`; no code change required.)
- [ ] `color` field is absent from `FogVolumeRecord`, the PRL wire format, the GPU `FogVolume` WGSL struct, and the CPU packing code; existing PRL files must be recompiled.
- [ ] `fog_volume`, `fog_lamp`, and `fog_tube` FGD entities no longer expose a `color` KVP.
- [ ] Maps that previously set `color` on fog entities recompile without error (the compiler ignores unrecognised KVPs).

## Tasks

### Task 1: Fog shader — SH-driven ambient scatter

Replace the L0-only `sample_sh_ambient()` in `fog_volume.wgsl` with a full L2 reconstruction helper. The new helper mirrors `sample_sh_indirect_fast()` from `forward.wgsl` but fixes the evaluation normal to `vec3(0, 1, 0)` — fog is directionally isotropic, so a stable neutral normal avoids ray-direction artifacts and produces an "ambient from above" reading consistent with the artistic intent. No surface-normal offset (the 0.1 m wall-bleed mitigation in the forward path has no meaning here). When `sh_grid.has_sh_volume == 0u` the helper returns `vec3<f32>(0.0)`, preserving the no-volume fallback.

Change the raymarch accumulation to use the SH color directly instead of `vs.color * sh_amb`. Also remove the `vs.color` multiplier from the spot beam accumulation line so beams render at their natural color.

Task 1 leaves the `VolumeSample.color` field and its write sites in `sample_fog_volumes()` intact — only the consumption of `vs.color` in the accumulation lines is removed. Task 3 deletes the field and its five write/normalization sites.

Remove `sample_sh_ambient()` and `sh_reconstruct_l0()` — both become dead code.

### Task 2: FGD — retire `color` KVP from fog entities

Remove the `color(color255)` KVP definition from `fog_volume`, `fog_lamp`, and `fog_tube`. Update any inline documentation in the FGD block that references the color attribute. Add a brief comment on each entity noting that ambient fog color is derived from the scene's SH irradiance volume. The compiler ignores unrecognised KVPs; no compiler change is needed. Also update the mapper-pattern example comment in the FGD block that lists `color` as a tuned default attribute.

### Task 3: Remove `color` field end-to-end

With the shader no longer reading `color`, remove it from every layer:

- `FogVolumeRecord` in `crates/level-format/src/fog_volumes.rs`: drop the `color` field, remove it from `to_bytes()` / `from_bytes()`, update the `MIN_RECORD_SIZE` constant, and update all round-trip tests. Also update the struct-level doc-comment that currently states 'colour is linear 0–1 (no sRGB curve applied)' and the on-disk layout doc-comment block listing `f32 color_r, color_g, color_b`. Update the `MIN_RECORD_SIZE` local constant inside `from_bytes` from `92` to `80` and its explanatory comment from '21 × f32 + 2 × u32 = 92 bytes' to '18 × f32 + 2 × u32 = 80 bytes'.
- Bump `CURRENT_VERSION` in `crates/level-format/src/lib.rs` (currently `1`). The loader rejects files with a mismatched version — without a bump, stale PRL files would be parsed with the wrong field layout and silently corrupt data.
- WGSL `FogVolume` struct in `fog_volume.wgsl`: drop `color: vec3<f32>`. After removal, the struct is 80 bytes:
  ```
  min: vec3<f32> / density: f32              → 16 bytes
  max_v: vec3<f32> / edge_softness: f32      → 16 bytes
  center: vec3<f32> / half_diag: f32         → 16 bytes
  inv_half_ext: vec3<f32> / inv_height_extent: f32 → 16 bytes
  radial_falloff: f32 / scatter: f32 / plane_offset: u32 / plane_count: u32 → 16 bytes
  ```
  `scatter` re-pairs with `radial_falloff` in the final 16-byte block. Update the shader size comment to 80 bytes.
- CPU packing in `crates/postretro/src/fx/fog_volume.rs`: remove the code that writes the color bytes into the GPU fog-volume buffer; drop the `color: [f32; 3]` field on the Rust `FogVolume` struct and update the `FOG_VOLUME_SIZE` stride constant from 96 to 80.
- `VolumeSample` struct in `fog_volume.wgsl`: drop `color: vec3<f32>` and the accumulation lines in `sample_fog_volumes()` that computed it.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — independent.
**Phase 2 (sequential):** Task 3 — depends on Task 1 having removed all shader reads of `color`; depends on Task 2 so the FGD and wire format land together.

## Rough sketch

`fog_volume.wgsl` changes:

```wgsl
// Proposed design — remove after implementation

// Replace sample_sh_ambient (L0 only) with:
fn sample_sh_fog(world_pos: vec3<f32>) -> vec3<f32> {
    if sh_grid.has_sh_volume == 0u { return vec3<f32>(0.0); }
    let gdims_f = max(vec3<f32>(sh_grid.grid_dimensions), vec3<f32>(1.0));
    let cell_coord = (world_pos - sh_grid.grid_origin) /
        max(sh_grid.cell_size, vec3<f32>(1.0e-6));
    let gf = clamp(cell_coord, vec3<f32>(0.0), gdims_f - vec3<f32>(1.0));
    let gi = vec3<u32>(floor(gf));
    let gfrac = fract(gf);
    return sample_sh_indirect_fast(vec3<f32>(0.0, 1.0, 0.0), gi, gfrac);
}

// Raymarch accumulation:
// Before: accum = accum + transmittance * weight * vs.color * sh_amb;
// After:
let sh_color = sample_sh_fog(pos);
accum = accum + transmittance * weight * sh_color;

// Spot beam line — drop vs.color:
// Before: accum = accum + transmittance * weight * vs.color * spot.color * atten * lit;
// After:
accum = accum + transmittance * weight * spot.color * atten * lit;
```

`sample_sh_indirect_fast()` and `sh_irradiance()` are defined in `forward.wgsl`. The fog shader is a separate WGSL compilation unit; duplicate them into `fog_volume.wgsl`. The rendering pipeline doc §8 explicitly endorses string-concatenation composition for shared helpers. Extraction to a shared `sh_sample.wgsl` module is a follow-up, not a prerequisite.

## Open questions

1. _(resolved)_ **Neutral normal choice.** Use `vec3(0, 1, 0)` (world up). Rationale: averaging `sh_irradiance` symmetrically over any set of normals that spans the sphere collapses to L0 (the L1/L2 terms cancel exactly — verified analytically). A single asymmetric fixed normal is therefore the only way to get directional hue from a single SH sample without additional data structures. World up captures overhead/ceiling ambient, the dominant hue in indoor scenes, and is stable across all camera orientations.

2. _(resolved)_ **`color` KVP in legacy maps.** The compiler ignores unrecognised KVPs. No backward-compatibility work needed; existing maps recompile cleanly after the FGD change. Pre-release policy: no compat shims.
