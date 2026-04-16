# Sub-plan 7 — SH Volume Sampling (Indirect Lighting)

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals and the BVH dependency.
> **Scope:** Runtime loading and sampling of the SH irradiance volume baked in sub-plan 2. 3D texture creation and upload, trilinear SH sampling in the fragment shader, SH L2 irradiance reconstruction. Replaces flat ambient floor as the indirect lighting term (ambient floor remains as a minimum beneath the indirect contribution).
> **Crates touched:** `postretro` only.
> **Depends on:** sub-plan 2 (SH PRL section must exist in compiled maps) **and** sub-plan 3 (ambient floor and direct lighting must be working — indirect is additive to the direct term).
> **Blocks:** sub-plan 8 (animated SH layers extend the base SH sampling path built here).

---

## Description

Parse the `ShVolume` PRL section (section ID 20, written by the baker in sub-plan 2), upload SH probe data to 3D GPU textures, and sample them trilinearly in the fragment shader to reconstruct per-fragment irradiance. This replaces flat ambient as the indirect lighting contribution. The ambient floor (sub-plan 3) remains as a minimum beneath the indirect term — it prevents pitch-black areas where neither direct lights nor indirect probes contribute. This sub-plan provides the foundation that sub-plan 8 (animated SH) builds upon.

Missing SH section degrades cleanly to the pre-Milestone-5 behavior: the shader skips SH sampling and uses the ambient floor alone.

---

## 3D texture layout

27 f32 per probe (9 SH L2 coefficients × 3 color channels) do not fit in a single texel. The data is split across multiple 3D textures, each sized to the probe grid dimensions (`grid_dimensions.x × grid_dimensions.y × grid_dimensions.z`).

### Texture slab approach

Group the 9 SH coefficients into 3 slabs of 3 coefficients each. Each slab stores 3 coefficients × 3 color channels = 9 scalars, packed into `Rgba16Float` texels:

| Slab | SH bands | Texels per probe | Textures |
|------|---------|-----------------|----------|
| 0 | L0 + L1 (bands 0–3) | 3 texels (12 f32 → 3 × rgba16f) | 3 `texture_3d<f32>` |
| 1 | L1 + L2 (bands 4–6) | 3 texels (9 f32 → 3 × rgba16f, 3 channels unused) | ... |
| 2 | L2 (bands 7–8) | 2 texels (6 f32 → 2 × rgba16f, 2 channels unused) | ... |

**Simplified alternative:** 7 `Rgba16Float` 3D textures total, each storing 4 scalars (28 scalars with 1 wasted). Each texture holds one "slice" of the 27 coefficients. This is simpler to upload and index — the implementation chooses between the two layouts based on what's easier to manage. The shader math is the same either way.

**Why `Rgba16Float` and not `Rgba32Float`?** Half-float precision is sufficient for SH coefficients (irradiance values, not positions). Halves memory and bandwidth. If banding is visible on test maps, upgrade to `Rgba32Float`.

### Upload

At level load:
1. Parse the `ShVolume` section header (grid origin, cell size, dimensions, probe stride, animated light count).
2. Read base probe records. Skip invalid probes (validity = 0) — upload zeroed SH coefficients for invalid probes so trilinear filtering blends them away.
3. Create 3D textures sized to grid dimensions. Upload SH coefficient slices via `queue.write_texture()`.

---

## Validity mask handling

Invalid probes (inside solid geometry) store zeroed SH coefficients. Hardware trilinear filtering naturally blends towards zero near walls, which produces a darkening effect at solid boundaries. This is acceptable for the initial cut — it's geometrically correct (less light reaches corners near walls) and avoids shader-side validity branching.

If the darkening is too aggressive on test maps (shadowed corners become unnaturally dark), a follow-up can implement nearest-valid-probe fallback in the shader.

---

## Bind group changes

Add **group 3 (SH volume)**:

```
@group(3) @binding(0) var sh_sampler: sampler;              // trilinear (linear filter, clamp-to-edge)
@group(3) @binding(1) var sh_texture_0: texture_3d<f32>;    // SH coefficients slice 0
@group(3) @binding(2) var sh_texture_1: texture_3d<f32>;    // slice 1
...                                                         // one binding per texture
@group(3) @binding(N) var<uniform> sh_grid: ShGridInfo;     // grid origin, cell size, dimensions
```

```
struct ShGridInfo {
    grid_origin: vec3<f32>,
    _pad0: f32,
    cell_size: vec3<f32>,
    _pad1: f32,
    grid_dimensions: vec3<u32>,
    _pad2: u32,
}
```

Group 3 is created once at level load if the SH section exists. If absent, group 3 is not bound and the shader skips SH sampling (controlled by a `has_sh_volume: u32` flag in the per-frame uniforms or by binding a dummy 1×1×1 texture).

---

## Fragment shader: SH sampling and irradiance reconstruction

```wgsl
// Compute probe-grid UV from world position
let grid_uv = (frag_world_pos - sh_grid.grid_origin) / (vec3<f32>(sh_grid.grid_dimensions) * sh_grid.cell_size);

// Sample SH coefficients trilinearly (one sample per texture)
let sh0 = textureSample(sh_texture_0, sh_sampler, grid_uv);
let sh1 = textureSample(sh_texture_1, sh_sampler, grid_uv);
// ... etc, unpack into 9 coefficients per channel

// Reconstruct irradiance from SH L2 in direction of shading normal
fn sh_irradiance(coeffs: array<vec3<f32>, 9>, normal: vec3<f32>) -> vec3<f32> {
    let n = normal;
    return
        coeffs[0] * 0.282095                           // L0
        + coeffs[1] * 0.488603 * n.y                   // L1
        + coeffs[2] * 0.488603 * n.z
        + coeffs[3] * 0.488603 * n.x
        + coeffs[4] * 1.092548 * n.x * n.y             // L2
        + coeffs[5] * 1.092548 * n.y * n.z
        + coeffs[6] * 0.315392 * (3.0 * n.z * n.z - 1.0)
        + coeffs[7] * 1.092548 * n.x * n.z
        + coeffs[8] * 0.546274 * (n.x * n.x - n.y * n.y);
}

let indirect = max(sh_irradiance(sh_coeffs, shading_normal), vec3(0.0));
```

The SH basis constants are the standard real spherical harmonic normalization factors. The `max(..., 0.0)` clamps negative irradiance (possible with L2 ringing) to zero.

### Integration with the lighting equation

The fragment shader output becomes:

```
let rgb = base_color.rgb * (ambient_floor + indirect + direct_sum);
```

Where:
- `ambient_floor` — minimum light level (sub-plan 3, scalar)
- `indirect` — SH irradiance sample (sub-plan 7, vec3)
- `direct_sum` — accumulated direct light contributions (sub-plan 3, vec3)

When no SH volume is loaded, `indirect` is `vec3(0.0)` and the lighting equation degrades to `ambient_floor + direct_sum` — matching the post-sub-plan-3 behavior.

---

## Acceptance criteria

- [ ] SH PRL section parsed into CPU-side probe grid at level load
- [ ] 3D textures created and uploaded with SH coefficient slices
- [ ] Invalid probes uploaded as zeroed coefficients (trilinear blends to dark)
- [ ] `ShGridInfo` uniform uploaded with grid origin, cell size, dimensions
- [ ] Fragment shader computes grid UV from world position and samples SH textures trilinearly
- [ ] SH L2 irradiance reconstruction using standard basis constants and shading normal direction
- [ ] Indirect term replaces flat ambient in the lighting equation (ambient floor remains as minimum)
- [ ] Missing SH section degrades cleanly: no SH textures bound, indirect = 0, lighting = ambient_floor + direct
- [ ] Indirect light visibly bleeds around corners on test maps (validates SH data + sampling)
- [ ] `cargo test -p postretro` passes
- [ ] `cargo clippy -p postretro -- -D warnings` clean

---

## Implementation tasks

1. SH volume loader: parse `ShVolume` PRL section header and base probe records. Map validity flag to zeroed coefficients for invalid probes.

2. Create 3D textures (`Rgba16Float`) sized to grid dimensions. Upload SH coefficient slices via `queue.write_texture()`.

3. Create SH sampler (trilinear, clamp-to-edge) and `ShGridInfo` uniform buffer.

4. Create group 3 bind group with SH textures, sampler, and grid info. Handle missing SH section (skip group creation or bind dummy).

5. Fragment shader: compute grid UV, sample SH textures, unpack into 9-coefficient arrays per channel, evaluate SH L2 irradiance in shading normal direction.

6. Integrate indirect term into the lighting equation: `ambient_floor + indirect + direct_sum`.

7. Validate on test maps: verify indirect bleed around corners, correct color bleeding from colored surfaces, no banding or ringing artifacts.

---

## Notes for implementation

- **SH basis constants.** The constants in the irradiance reconstruction (`0.282095`, `0.488603`, etc.) are the standard real SH normalization factors multiplied by the cosine-lobe convolution coefficients. These are well-established and should not be tuned — if the indirect looks wrong, the bug is in the baker or the texture upload, not the constants.
- **Probe grid UV clamping.** Fragments outside the probe grid (beyond the level's AABB) should clamp to the nearest edge probe. The `clamp-to-edge` sampler mode handles this automatically.
- **Negative irradiance.** SH L2 can produce negative values for sharp transitions (Gibbs phenomenon / ringing). Clamping to zero per channel is standard practice and visually fine.
- **Memory estimate.** 7 `Rgba16Float` textures for a 60×60×20 grid: `7 × 60 × 60 × 20 × 4 × 2 bytes = 4.0 MB`. Well within budget for the target map sizes.
