# Fog SH Color

## Goal

Replace the fixed per-volume fog color with full L2 SH irradiance sampled at each raymarch position. Fog inherits ambient environment color from the same SH volume that lights world geometry, eliminating per-volume color authoring and ensuring visual coherence with the surrounding scene.

## Scope

### In scope

- Fog ambient scatter driven by full L2 SH reconstruction at each sample position (all 9 bands, fixed neutral normal)
- Spot beam scatter loses the per-volume color tint; beams render at their natural `spot.color`
- `color` KVP removed from FGD fog entities (`env_fog_volume`, `fog_lamp`, `fog_tube`)
- Per-volume `color` field retained in PRL wire format and Rust types — parsed but ignored by the shader

### Out of scope

- Per-volume color tint multiplier layered on top of SH (not needed; mappers control hue via light placement)
- Changes to how dynamic spot/point light intensities or falloff work in the fog pass
- SH volume baking changes
- Animated SH compose pass changes

## Acceptance criteria

- [ ] In a level with colored SH lighting, fog visually reflects the local ambient color at its position — warm rooms produce warm fog, cool rooms produce cool fog, without any `color` KVP set on the fog entity.
- [ ] Fog in a scene with no SH volume (`has_sh_volume == 0`) produces no ambient scatter contribution (same fallback as the current L0 path when the SH volume is absent).
- [ ] Spot beam scatter through fog shows the beam's natural color, unmodulated by a per-volume tint.
- [ ] Point light scatter through fog is unchanged.
- [ ] Existing PRL files load without error; the `color` field present in their wire format is parsed and silently ignored.
- [ ] `env_fog_volume`, `fog_lamp`, and `fog_tube` FGD entities no longer expose a `color` KVP.
- [ ] TrenchBroom accepts maps that previously set `color` on fog entities (unrecognised KVPs are passed through; the compiler may warn but must not hard-error on a now-dropped KVP).

## Tasks

### Task 1: Fog shader — SH-driven ambient scatter

Replace the L0-only `sample_sh_ambient()` in `fog_volume.wgsl` with a full L2 reconstruction helper. The new helper mirrors `sample_sh_indirect_fast()` from `forward.wgsl` but fixes the evaluation normal to `vec3(0, 1, 0)` — fog is directionally isotropic, so a stable neutral normal avoids ray-direction artifacts and produces an "ambient from above" reading consistent with the artistic intent. No surface-normal offset (the 0.1 m wall-bleed mitigation in the forward path has no meaning here).

Change the raymarch accumulation to use the SH color directly instead of `vs.color * sh_amb`. Also remove the `vs.color` multiplier from the spot beam accumulation line so beams render at their natural color.

Remove `sample_sh_ambient()` and `sh_reconstruct_l0()` — both become dead code.

### Task 2: FGD — retire `color` KVP from fog entities

Remove the `color(color255)` KVP definition from `env_fog_volume`, `fog_lamp`, and `fog_tube`. Update any inline documentation in the FGD block that references the color attribute. Add a brief comment on each entity noting that ambient fog color is derived from the scene's SH irradiance volume.

The compiler's map-data parsing reads KVPs generically and ignores unrecognised keys; no compiler change is needed beyond verifying that a `color` key present in an old `.map` file does not produce a hard error.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — fully independent, no shared files.

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
    // Reuse sample_sh_indirect_fast from forward.wgsl with neutral up-normal.
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

`sample_sh_indirect_fast()` is defined in `forward.wgsl`. The fog shader is a separate pipeline; it must copy the helper (or the project must introduce a shared WGSL include mechanism — see Open Questions).

## Open questions

1. **Shared WGSL helpers.** `sample_sh_indirect_fast()` and `sh_irradiance()` currently live only in `forward.wgsl`. Task 1 can duplicate them into `fog_volume.wgsl` for now (the rendering pipeline doc §8 explicitly endorses string-concatenation composition). Consider extracting to a `sh_sample.wgsl` helper in a follow-up — but that's not required for this plan to ship.

2. **Neutral normal choice.** `vec3(0, 1, 0)` (world up) is proposed. An alternative is `vec3(0, 0, -1)` (camera forward) for an "ambient facing the player" reading. Up is more stable across all viewing angles and better matches the "what does this space feel like" intent of fog. Decision is for the implementer if the up-normal reads poorly in testing.

3. **`color` KVP in legacy maps.** The compiler's KVP parser should silently ignore the now-dropped key. Verify this before landing — if the compiler hard-errors on unrecognized KVPs, a permissive-unknown-key pass is needed (small change, but a prerequisite).
