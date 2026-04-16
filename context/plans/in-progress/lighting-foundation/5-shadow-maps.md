# Sub-plan 5 — Shadow Maps

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals.
> **Scope:** Shadow map rendering passes and fragment shader shadow sampling. CSM for directional, cube shadow maps for point, single 2D shadow maps for spot. No indirect shadow contribution (bake-time raycast handles that in sub-plan 2).
> **Crates touched:** `postretro` only.
> **Depends on:** sub-plan 3 (direct lighting must be working — shadows modulate the direct term). **Benefits from sub-plan 4** (light influence volumes provide the CPU frustum-visibility test used for shadow-slot allocation; if sub-plan 4 has not shipped, allocate a fixed slot per shadow-casting light instead).
> **Blocks:** nothing directly. Sub-plans 6–8 are independent of shadow maps.

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

Cascade selection: compare the fragment's view-space depth against the cascade split distances and use the tightest cascade that contains the fragment.

**Split distances are passed via the frame uniform buffer (group 0).** Extend `Uniforms` in `forward.wgsl` with a `csm_splits: vec4<f32>` field (xyz = far view-space distance of cascades 0/1/2; w reserved for an optional 4th cascade). This piggybacks on the existing group-0 binding — no new binding slot needed. The CPU writes these values each frame from the cascade split calculation. Note that this also requires passing the view matrix (or view-space Z) into the fragment shader so the fragment can derive its own view-space depth; reuse whatever camera data is already in the uniform buffer rather than adding new plumbing.

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
fn sample_point_shadow(light_pos: vec3<f32>, frag_world_pos: vec3<f32>, light_range: f32) -> f32 {
    let to_frag = frag_world_pos - light_pos;
    let depth = length(to_frag) / light_range;  // normalized [0, 1]
    return textureSampleCompareLevel(
        point_shadow_cube, shadow_sampler, normalize(to_frag), depth
    );
}
```

The cube texture sampler takes a 3D direction vector and selects the correct face automatically. Depth is compared against the stored distance.

`light_range` is read from the existing `direction_and_range.w` slot of the point light's `GpuLight` record (see `postretro/src/lighting.rs` — slot 2). Both the shadow-map fragment shader (write path) and the forward light-loop (read path) MUST use the same `light_range` value, otherwise the comparison is meaningless. The depth-only fragment shader for point lights receives `light_pos` and `light_range` via push constants or a small per-pass uniform buffer.

**Depth encoding:** Store linear depth (distance from light / light range) rather than perspective-Z. Linear depth avoids the non-linear precision distribution of perspective projection, which matters for omnidirectional shadow maps where nearby faces have very different depth ranges.

**Depth write:** The linear-depth fragment shader writes `@builtin(frag_depth)` explicitly. The depth-target pipeline must have `depth_write_enabled: true` and still use `CompareFunction::Less` against the stored value (fragments with smaller normalized distance win). The comparison sampler on the read side uses `CompareFunction::Less` as well — the two compare functions must match.

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

Shadow textures are allocated from a **fixed slot pool** at level load — not one texture per shadow-casting light. The plan targets up to 500 authored lights per level, with potentially hundreds of shadow-casters; only a small subset is visible at any moment (sub-plan 4's `visible_lights` provides the active set). See §Shadow-slot pool sizing for concrete allocations and VRAM budget. No dynamic resizing.

---

## Shadow-slot pool sizing

The plan targets up to **500 authored lights per level**. Most lights in a typical level are point or spot; directional lights (sunlight) are rare (0–2 per level). Not every authored light needs a shadow — fill lights, accent lights, and distant decorative lights can run unshadowed without visual loss. The `cast_shadows` flag lets mappers control this per light.

At any given camera position, sub-plan 4's `visible_lights` frustum test culls the majority of shadow-casting lights. The slot pool only needs to cover the **peak simultaneous visible shadow-casters**, not the total count. Indoor boomer-shooter geometry with portal-isolated rooms means the visible set is typically 10–30 lights even in dense levels.

### Pool allocation

| Pool | Slot count | Per-slot VRAM | Total VRAM |
|------|-----------|--------------|------------|
| CSM (directional) | 2 lights × 3 cascades = 6 layers | 1024² × 4 bytes = 4 MB | 24 MB |
| Cube (point) | 16 lights × 6 faces = 96 layers | 512² × 4 bytes = 1 MB | 16 MB |
| Spot (2D) | 16 lights | 1024² × 4 bytes = 4 MB | 64 MB |
| **Total** | | | **104 MB** |

These are starting values. The pool sizes are engine constants, not per-level — every level allocates the same pool at load time. If 16 point or 16 spot slots prove insufficient for a particular camera position, lights beyond the pool degrade to unshadowed for that frame. The degradation is per-frame and invisible in motion for lights near the frustum edge.

**VRAM note:** 104 MB is the worst-case resident cost for shadow maps. On a 2 GB discrete GPU this is ~5% of VRAM. If the budget is too high for integrated GPUs, reduce spot resolution to 512² (cuts spot pool from 64 MB to 16 MB, total to 56 MB) or reduce slot counts. These are tuning knobs, not architectural changes.

### Slot assignment policy

Each frame, after `visible_lights` returns the active set:

1. Directional lights always get a CSM slot (0–2 lights, pool of 2).
2. Remaining slots are assigned to point and spot lights by **distance from camera** (nearest first). Distance is cheap to compute from the influence record's center.
3. Lights that don't receive a slot have `shadow_kind = 0` in their `GpuLight` upload for that frame — the fragment shader skips the shadow sample and uses unmodulated direct light.

The sort is O(N log N) where N is the visible shadow-caster count (typically 10–30). Sub-microsecond.

---

## Bind group changes

Extend **group 2 (lighting)** with shadow map bindings:

```
@group(2) @binding(0) var<storage, read> lights: array<GpuLight>;             // existing from sub-plan 3
@group(2) @binding(1) var<storage, read> light_influence: array<vec4<f32>>;  // from sub-plan 4
@group(2) @binding(2) var shadow_sampler: sampler_comparison;
@group(2) @binding(3) var csm_depth_array: texture_depth_2d_array;
@group(2) @binding(4) var<storage, read> csm_view_proj: array<mat4x4<f32>>;
@group(2) @binding(5) var point_shadow_cubes: texture_depth_cube_array;  // or individual bindings if array not supported
@group(2) @binding(6) var spot_shadow_array: texture_depth_2d_array;
@group(2) @binding(7) var<storage, read> spot_view_proj: array<mat4x4<f32>>;
```

Exact binding layout is an implementation detail — may consolidate or split based on what wgpu supports for the target feature set. The key constraint: all shadow data must be accessible in the fragment shader during the light loop.

---

## GpuLight struct extension

The `GpuLight` struct from sub-plan 3 already reserves a fifth vec4 slot named `shadow_info` (zero-initialized at upload time). Sub-plan 5 activates that slot — no new fields, no size change. Total size stays at **80 bytes** (5 × vec4<f32>).

```
struct GpuLight {                              // 80 bytes — unchanged
    position_and_type: vec4<f32>,
    color_and_falloff_model: vec4<f32>,
    direction_and_range: vec4<f32>,
    cone_angles_and_pad: vec4<f32>,
    shadow_info: vec4<f32>,                    // x = bitcast<f32>(cast_shadows u32, 0 or 1)
                                               // y = bitcast<f32>(shadow_map_index u32)
                                               // z = bitcast<f32>(shadow_kind u32) — 0 none, 1 CSM, 2 cube, 3 spot-2d
                                               // w = unused (reserved)
}
```

`shadow_map_index` indexes into the appropriate texture array for the light's type (cascade base layer for directionals, cube slot for points, array layer for spots). `shadow_kind` is redundant with `light_type` in the current design but is stored explicitly so that an unshadowed light of any type is a single branch on `shadow_kind == 0` without needing to re-derive which shadow array to consult.

The CPU pack routine in `postretro/src/lighting.rs` (`pack_light`) is updated to write bytes 64..80 according to the resolved shadow assignment instead of zeroing them.

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
- [ ] Fixed slot pool allocated at level load (2 CSM, 16 point cube, 16 spot 2D)
- [ ] Slot assignment uses `visible_lights` from sub-plan 4 (or all shadow-casters if sub-plan 4 absent), sorted by distance from camera
- [ ] Lights beyond the pool degrade to unshadowed for that frame without error
- [ ] Slot cache avoids re-rendering shadow maps for lights that retained their slot from the previous frame
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

## Shadow map update cadence

All map-authored lights in Postretro are static (position, direction, color, and range are baked into the PRL and never mutated at runtime), and world geometry is BSP-static — nothing in a shipped map moves relative to a shadow-casting light. A given light's shadow map content never changes once rendered.

**However, the slot pool is smaller than the total shadow-caster count.** As the camera moves, the set of visible shadow-casters changes, and slots are reassigned to different lights. A slot that held torch A last frame may hold torch B this frame, requiring a re-render of torch B's shadow map into that slot.

**Caching strategy:** maintain a `HashMap<light_index, slot_index>` mapping lights to their current slot assignment. Each frame:

1. Run `visible_lights` (sub-plan 4) to get the active set.
2. Lights that are still visible **and** still hold a slot from last frame keep their slot — no re-render.
3. Lights that lost visibility release their slot.
4. Newly visible lights claim free slots and render their shadow map.
5. If no slots are free, evict the farthest-from-camera light and reassign.

With static lights and static geometry, a shadow map rendered into a slot is valid forever as long as that slot is not reassigned. The cache means that in steady-state (camera not moving), zero shadow passes run per frame. During camera movement, only the newly-visible lights need rendering — typically 1–3 per frame as the player walks through a level.

**CSM exception:** directional light CSMs depend on the camera frustum (cascade splits change with view), so they re-render every frame regardless. With 2 directional lights × 3 cascades × full-scene geometry, this is 6 depth passes per frame — the dominant shadow cost. Shadow-specific BVH culling per cascade (noted in §Notes) becomes more valuable at 500-light levels where the geometry count is likely higher.

If a future milestone introduces dynamic shadow-casting lights or moving shadow-casting geometry, a per-light `dirty` flag forces a re-render of that light's slot without changing the pool or bind group architecture.

---

## Interaction with portal visibility

The forward pass is driven by the CPU portal-traversal visible-cell bitmask. **Shadow passes ignore that bitmask entirely** — shadow maps are rendered from the light's viewpoint, not the camera's, so the camera's visible cells are not a meaningful cull set for a shadow pass. A point light in a room the camera can't currently see still casts shadows that matter the moment the player walks into that room (and with the static-render-once cadence above, the shadow is baked at level load with no camera involved at all).

Do not reuse the camera visibility bitmask to skip shadow geometry. If shadow-pass cost becomes a problem, add per-light frustum culling driven by the light's own bounds, not the camera's portal set.

---

## Notes for implementation

- **No shadow-specific BVH cull.** The initial cut renders all world geometry into every shadow map. This is simple and correct. If profiling shows shadow passes are expensive, add per-light frustum culling as a follow-up — the BVH is already there.
- **`texture_depth_cube_array`** may not be available on all backends. If wgpu reports the feature as unavailable, fall back to individual `texture_depth_cube` textures per point light with separate bindings. Check `Features::TEXTURE_CUBE_ARRAY_DEPTH`.
- **Shadow map atlas** (packing all shadow maps into one large texture) is a common optimization but adds complexity. Skip it — the fixed slot pool keeps the number of textures bounded regardless of how many lights the level contains.
- **Depth bias values** are notoriously scene-dependent. The wgpu `DepthBiasState` has `constant: i32`, `slope_scale: f32`, and `clamp: f32`. For `Depth32Float` the `constant` units are floating-point ULPs scaled by an implementation-defined factor — the "constant=2, slope=2.0" suggestion is a rough GL-era starting point and may need to be **scaled by orders of magnitude** for `Depth32Float`. Tune by observation: start small, double until acne disappears, then check for peter-panning. Document final values in code with a comment linking back to sub-plan 5.
- **Front-face vs. back-face culling in shadow passes.** The spec says back-face culling "same as the forward pass" — this is one of two legitimate choices. Back-face culling pushes self-shadow acne onto the lit front faces where more bias is needed. Front-face culling pushes acne onto back faces the camera can't see (cheaper bias, but causes peter-panning on single-sided or thin geometry like grates). The initial cut uses back-face culling for consistency with the forward pass; if acne is hard to eliminate without causing peter-panning, try flipping to front-face culling as a targeted fix before adding more bias.
- **Reverse-Z is not used.** The forward pipeline uses standard depth (0 near, 1 far) with `CompareFunction::Less` (verified in `postretro/src/render.rs`). Shadow pipelines must match — do not silently introduce reverse-Z in shadow passes, it will make `Less` comparisons meaningless.
