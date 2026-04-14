# Sub-plan 4 — Shadow Maps

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals.
> **Scope:** Shadow map rendering passes and fragment shader shadow sampling. CSM for directional, cube shadow maps for point, single 2D shadow maps for spot. No indirect shadow contribution (bake-time raycast handles that in sub-plan 2).
> **Crates touched:** `postretro` only.
> **Depends on:** sub-plan 3 (direct lighting must be working — shadows modulate the direct term).
> **Blocks:** nothing directly. Sub-plans 5–7 are independent of shadow maps.

---

## Description

Add shadow map passes before the opaque forward pass. Each shadow-casting light renders depth-only geometry into a shadow map texture. The fragment shader samples the shadow map during the light loop (sub-plan 3) to modulate each light's direct contribution by visibility.

Shadow resolution is intentionally modest. Chunky shadow edges with nearest-neighbor sampling are part of the retro aesthetic — not a defect to fix with PCF or VSM.

---

## Shadow pass structure

Shadow passes run **after** the compute BVH cull and **before** the opaque forward pass. Each shadow-casting light gets its own depth-only render pass.

Updated frame order:
1. CPU portal traversal → visible-cell bitmask
2. Compute BVH cull (existing)
3. **Shadow map passes** (new — one per shadow-casting light)
4. Opaque forward pass (existing, now with shadow sampling)
5. Debug wireframe overlay (existing, optional)

### Which lights cast shadows

Not every light casts shadows. A `cast_shadows: bool` field on the runtime `GpuLight` struct gates whether a shadow pass is rendered. Default: `true` for lights authored in the FGD (map lights), `false` for transient gameplay lights (future, Milestone 6+). The flag is set at level load when converting `MapLight` → `GpuLight`.

Shadow-casting lights also store a `shadow_map_index: u32` in the `GpuLight` struct so the fragment shader knows which shadow map to sample.

---

## Directional lights — Cascaded Shadow Maps (CSM)

### Layout

3 cascades (start with 3; add a 4th if coverage is insufficient on test maps). Each cascade is a **1024×1024** depth texture layer in a `texture_depth_2d_array`.

Cascade split scheme: practical split (logarithmic/linear blend, λ ≈ 0.5) based on the camera's near/far range. Each cascade covers a progressively larger frustum slice.

### Per-cascade rendering

For each cascade:
1. Compute the light-space orthographic projection matrix that tightly fits the cascade's frustum slice.
2. Render all world geometry (same vertex buffer, same index buffer) with a depth-only pipeline. No color attachment — only the depth attachment targeting the cascade's array layer.
3. Use the existing BVH leaf index buffer directly. No separate cull pass for shadow geometry in the initial cut — render all leaves. (Shadow-specific culling is a follow-up optimization if shadow pass cost is measurable.)

### Fragment shader sampling

```
// pseudocode
fn sample_csm(frag_world_pos: vec3<f32>, cascade_index: u32) -> f32 {
    let light_space_pos = csm_view_proj[cascade_index] * vec4(frag_world_pos, 1.0);
    let shadow_uv = light_space_pos.xy * 0.5 + 0.5;
    let depth = light_space_pos.z;
    return textureSampleCompareLevel(
        csm_depth_array, shadow_sampler, shadow_uv, cascade_index, depth
    );
}
```

Cascade selection: compare fragment depth against cascade split distances. Use the tightest cascade that contains the fragment.

**No PCF, no filtering.** `nearest` comparison produces hard shadow edges. This is intentional.

---

## Point lights — Cube Shadow Maps

### Layout

Each shadow-casting point light renders into a **512×512** depth texture with 6 array layers (one per cube face). Sampled via `texture_depth_cube` by creating a cube `TextureView` over the 6-layer array.

### Per-face rendering

6 render passes per point light, one per cube face (+X, −X, +Y, −Y, +Z, −Z). Each pass uses a 90° FOV perspective projection and a view matrix oriented along the face's axis. Renders all world geometry depth-only into the corresponding array layer.

### Fragment shader sampling

```
// pseudocode
fn sample_point_shadow(light_pos: vec3<f32>, frag_world_pos: vec3<f32>) -> f32 {
    let to_frag = frag_world_pos - light_pos;
    let depth = length(to_frag) / light_range;  // normalized [0, 1]
    return textureSampleCompareLevel(
        point_shadow_cube, shadow_sampler, normalize(to_frag), depth
    );
}
```

The cube texture sampler takes a 3D direction vector and selects the correct face automatically. Depth is compared against the stored distance.

**Depth encoding:** Store linear depth (distance from light / light range) rather than perspective-Z. Linear depth avoids the non-linear precision distribution of perspective projection, which matters for omnidirectional shadow maps where nearby faces have very different depth ranges.

---

## Spot lights — Single Shadow Map

### Layout

Each shadow-casting spot light renders into a **1024×1024** single `texture_depth_2d`. Uses a perspective projection with the spot's outer cone angle as the FOV.

### Rendering

Single render pass per spot light. View matrix looks along the spot direction from the spot position. Renders all world geometry depth-only.

### Fragment shader sampling

```
// pseudocode
fn sample_spot_shadow(frag_world_pos: vec3<f32>, light_index: u32) -> f32 {
    let light_space_pos = spot_view_proj[light_index] * vec4(frag_world_pos, 1.0);
    let shadow_uv = light_space_pos.xy / light_space_pos.w * 0.5 + 0.5;
    let depth = light_space_pos.z / light_space_pos.w;
    return textureSampleCompareLevel(
        spot_shadow_maps, shadow_sampler, shadow_uv, light_index, depth
    );
}
```

Spot shadow maps stored in a `texture_depth_2d_array`, one layer per shadow-casting spot light.

---

## Shadow map texture organization

| Light type | Texture type | Resolution | Layers |
|-----------|-------------|-----------|--------|
| Directional | `texture_depth_2d_array` | 1024² | 3 per directional light (cascades) |
| Point | `texture_depth_2d` × 6 layers → `texture_depth_cube` view | 512² | 6 per point light |
| Spot | `texture_depth_2d_array` | 1024² | 1 per spot light |

All shadow maps use `Depth32Float` format (same as the main depth buffer) with a `comparison` sampler using `CompareFunction::Less`.

Shadow textures are allocated at level load based on the number of shadow-casting lights. No dynamic resizing.

---

## Bind group changes

Extend **group 2 (lighting)** with shadow map bindings:

```
@group(2) @binding(0) var<storage, read> lights: array<GpuLight>;  // existing from sub-plan 3
@group(2) @binding(1) var shadow_sampler: sampler_comparison;
@group(2) @binding(2) var csm_depth_array: texture_depth_2d_array;
@group(2) @binding(3) var<storage, read> csm_view_proj: array<mat4x4<f32>>;
@group(2) @binding(4) var point_shadow_cubes: texture_depth_cube_array;  // or individual bindings if array not supported
@group(2) @binding(5) var spot_shadow_array: texture_depth_2d_array;
@group(2) @binding(6) var<storage, read> spot_view_proj: array<mat4x4<f32>>;
```

Exact binding layout is an implementation detail — may consolidate or split based on what wgpu supports for the target feature set. The key constraint: all shadow data must be accessible in the fragment shader during the light loop.

---

## GpuLight struct extension

Extend the `GpuLight` struct from sub-plan 3:

```
struct GpuLight {                              // 80 bytes → 96 bytes
    position_and_type: vec4<f32>,
    color_and_falloff_model: vec4<f32>,
    direction_and_range: vec4<f32>,
    cone_angles_and_pad: vec4<f32>,
    shadow_info: vec4<f32>,                    // x = bitcast cast_shadows (0 or 1), y = bitcast shadow_map_index, zw = unused
    _padding2: vec4<f32>,                      // align to 96 bytes
}
```

The shadow_map_index maps into the appropriate texture array for the light's type.

---

## Depth-only pipeline

A separate `RenderPipeline` with:
- **Vertex shader:** transforms position only (no UV, no normal needed). Can reuse the world vertex buffer with a simpler vertex shader that reads only position.
- **Fragment shader:** depends on light type:
  - **Directional and spot lights:** none (true depth-only). Standard perspective/orthographic depth is written by the fixed-function depth output — no fragment shader needed.
  - **Point lights:** a minimal fragment shader is required. Linear depth encoding (`length(frag_pos - light_pos) / light_range`) cannot be done with the fixed-function depth output; the fragment shader must write the normalized distance value explicitly.
- **Color attachments:** none
- **Depth attachment:** the shadow map layer
- **Cull mode:** back-face (same as forward pass)
- **Depth bias:** small constant + slope bias to reduce shadow acne. Values tuned during implementation (typical starting point: constant 2, slope 2.0).

---

## Acceptance criteria

- [ ] Depth-only render pipeline created for shadow passes
- [ ] CSM passes render directional lights into cascade array layers
- [ ] Cascade split computed from camera near/far range
- [ ] Cube shadow map passes render point lights (6 faces each)
- [ ] Point shadow maps encode linear depth
- [ ] Single shadow map passes render spot lights
- [ ] Shadow comparison sampler created with `CompareFunction::Less`
- [ ] Fragment shader samples the correct shadow map per light during the light loop
- [ ] `cast_shadows` flag on `GpuLight` gates shadow pass rendering
- [ ] Shadow bias eliminates acne on lit surfaces
- [ ] Shadow edges are hard/chunky (nearest sampling, no PCF)
- [ ] Unshadowed lights still render correctly (skip shadow sample)
- [ ] All test maps render with correct shadow coverage and no light leaks at shadow edges
- [ ] `cargo test -p postretro` passes
- [ ] `cargo clippy -p postretro -- -D warnings` clean

---

## Implementation tasks

1. Create depth-only render pipeline (vertex-only, no color attachment).

2. Implement CSM: cascade split calculation, per-cascade orthographic projection, per-cascade depth render pass, cascade array texture allocation.

3. Implement cube shadow maps: per-face view/projection matrices, 6-pass rendering, cube texture view creation.

4. Implement spot shadow maps: per-light perspective projection from spot position along spot direction, single-pass depth rendering.

5. Allocate all shadow map textures at level load. Create shadow sampler with comparison function.

6. Extend `GpuLight` with shadow info. Upload CSM and spot view-projection matrices to storage buffers.

7. Extend fragment shader light loop: sample shadow map per shadow-casting light, multiply direct contribution by shadow factor.

8. Tune depth bias to eliminate shadow acne across test maps.

---

## Notes for implementation

- **No shadow-specific BVH cull.** The initial cut renders all world geometry into every shadow map. This is simple and correct. If profiling shows shadow passes are expensive, add per-light frustum culling as a follow-up — the BVH is already there.
- **`texture_depth_cube_array`** may not be available on all backends. If wgpu reports the feature as unavailable, fall back to individual `texture_depth_cube` textures per point light with separate bindings. Check `Features::TEXTURE_CUBE_ARRAY_DEPTH`.
- **Shadow map atlas** (packing all shadow maps into one large texture) is a common optimization but adds complexity. Skip it — individual textures per light type are simpler and the target light counts are small.
- **Depth bias values** are notoriously scene-dependent. Start with constant=2, slope=2.0 and adjust. Too much bias causes peter-panning (shadows detach from geometry); too little causes acne (self-shadowing noise).
