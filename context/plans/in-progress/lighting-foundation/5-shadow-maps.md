# Sub-plan 5 — Shadow Maps

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals.
> **Scope:** Shadow map rendering passes and fragment shader shadow sampling. CSM for directional, cube shadow maps for point, 2D perspective shadow maps for spot (one logical map per light, physically packed in a texture array). No indirect shadow contribution (bake-time raycast handles that in sub-plan 2).
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
1. Compute the light-space orthographic projection matrix that tightly fits the cascade's frustum slice (algorithm below).
2. Render all world geometry (same vertex buffer, same index buffer) with a depth-only pipeline. No color attachment — only the depth attachment targeting the cascade's array layer.
3. Draw the entire world index buffer in one indexed draw. The shadow pass does not use the per-frame BVH cull state — it renders all static geometry from the light's viewpoint. (Shadow-specific culling is a follow-up optimization if shadow pass cost is measurable.)

### Cascade ortho fitting algorithm

Tightly fitting an orthographic projection to a frustum slice is the most error-prone piece of CSM. The algorithm:

1. **Recover the 8 frustum-slice corners in world space.** The cascade covers view-space depths `[split_near, split_far]`. Map each to NDC Z (wgpu's standard depth is [0, 1]), then unproject the 8 corners `(±1, ±1, ndc_z)` through `inverse(view_proj)` with a w-divide to land in world space.
2. **Build a light-space view matrix.** `Mat4::look_to_rh(Vec3::ZERO, light_dir, up)`. Pick an `up` vector that is not parallel to `light_dir`: `up = Vec3::Z` if `|light_dir.y| > 0.99`, else `up = Vec3::Y`. Using a singular `up` produces a NaN view matrix and invisible shadows.
3. **Transform the 8 corners into light space** and compute the axis-aligned bounding box (min/max on each axis).
4. **Build the orthographic projection** from that light-space AABB. Push the near plane back (toward the light) by a generous margin — typical value 500 world units — so shadow casters that live behind the camera frustum slice but still shadow into it are captured. A too-tight near plane clips off-screen occluders and causes shadow pop-in at cascade boundaries.

**Texel snapping (known gap).** The current implementation does not snap the ortho bounds to texel boundaries, so when the camera rotates, the shadow projection jitters continuously and shadow edges shimmer. The standard fix is to quantize the AABB origin to multiples of `(light_space_extent / shadow_resolution)` before constructing the ortho matrix. Defer until shimmer is observed in motion testing.

**Reference implementation:** `postretro/src/lighting/shadow.rs::cascade_ortho_matrix`.

### Fragment shader sampling

```
// pseudocode
fn sample_csm(frag_world_pos: vec3<f32>, cascade_index: u32) -> f32 {
    let light_space_pos = csm_view_proj[cascade_index] * vec4(frag_world_pos, 1.0);
    // Perspective divide — for orthographic projection w == 1 so this is a
    // no-op, but always divide so the pattern matches the spot pseudocode and
    // is robust if the cascade ever uses a non-ortho matrix.
    let ndc = light_space_pos.xyz / light_space_pos.w;
    let shadow_uv = ndc.xy * 0.5 + 0.5;
    // Y-flip: WebGPU NDC has Y-up but texture V is Y-down. Without the flip,
    // shadows render vertically mirrored.
    let uv = vec2<f32>(shadow_uv.x, 1.0 - shadow_uv.y);
    let depth = ndc.z;
    return textureSampleCompareLevel(
        csm_depth_array, shadow_sampler, uv, cascade_index, depth
    );
}
```

Cascade selection: compare the fragment's view-space depth against the cascade split distances and use the tightest cascade that contains the fragment.

**Split distances are passed via the frame uniform buffer (group 0).** Extend `Uniforms` in `forward.wgsl` with a `csm_splits: vec4<f32>` field (xyz = far view-space distance of cascades 0/1/2; w reserved for an optional 4th cascade). This piggybacks on the existing group-0 binding — no new binding slot needed. The CPU writes these values each frame from the cascade split calculation.

Cascade selection also needs the fragment's view-space depth, which requires the camera view matrix in the fragment shader. Add a `view_matrix: mat4x4<f32>` field to `Uniforms` (group 0) alongside `csm_splits`. Piggybacking on group 0 keeps binding count unchanged, but it is not free plumbing — it is a real addition to the frame uniform struct.

**No PCF, no filtering.** `nearest` comparison produces hard shadow edges. This is intentional.

---

## Point lights — Cube Shadow Maps

### Layout

Each shadow-casting point light occupies 6 consecutive array layers in a shared **512×512** `texture_depth_2d` with `MAX_POINT_SHADOW_LIGHTS × 6` total layers. The texture is bound as `texture_depth_cube_array` via a `TextureViewDimension::CubeArray` view, so the hardware performs face selection and UV derivation.

### Per-face rendering

6 render passes per point light, one per cube face (+X, −X, +Y, −Y, +Z, −Z). Each pass uses a 90° FOV perspective projection and a view matrix oriented along the face's axis. Renders all world geometry depth-only into the corresponding array layer.

**Y-flip in the projection matrix (required).** WebGPU's cube-sampling UV convention (inherited from D3D/Vulkan) expects texture V to increase toward `+t` on each face's (s, t) parameterization. `Mat4::perspective_rh` combined with `Mat4::look_to_rh` for the face views emits the framebuffer Y flipped relative to that convention — without correction, geometry rendered at screen-top lands where the hardware sampler expects screen-bottom content, producing shadows mirrored across the horizontal axis of each face.

The fix: pre-multiply each face's view-projection matrix by a Y-flip, `Mat4::from_scale(Vec3::new(1.0, -1.0, 1.0)) * proj * view`. This realigns the rasterized content with the cube-sampling UV convention.

**Consequence: front-face culling for point shadow passes.** The Y-flip inverts triangle winding in screen space. The point-shadow pipeline must therefore set `cull_mode: Some(wgpu::Face::Front)` to cull what the rasterizer now sees as back-faces (but are geometrically front-facing). This is a targeted deviation from the forward-pass back-face culling used by directional and spot shadow passes.

### Fragment shader sampling

```
// pseudocode — note: point_shadow_array is texture_depth_cube_array
fn sample_point_shadow(
    light_pos: vec3<f32>,
    frag_world_pos: vec3<f32>,
    light_range: f32,
    shadow_map_index: u32,
    NdotL: f32,
) -> f32 {
    let to_frag = frag_world_pos - light_pos;
    let dist = length(to_frag);
    // Slope-scaled bias: at grazing angles (small NdotL) depth varies rapidly
    // across a shadow texel, so scale bias inversely with NdotL. Hardware
    // depth bias has no effect because the fragment shader writes frag_depth
    // explicitly; bias must be applied shader-side.
    let slope_bias = 0.002 / max(NdotL, 0.05);
    let depth = dist / max(light_range, 0.001) - slope_bias;
    let dir = normalize(to_frag);
    // Cube-array signature: (texture, sampler, coords, array_index, depth_ref).
    // A plain texture_depth_cube omits array_index; the array variant requires it.
    return textureSampleCompareLevel(
        point_shadow_array, shadow_sampler, dir, i32(shadow_map_index), depth
    );
}
```

The cube texture sampler takes a 3D direction vector and selects the correct face automatically. Depth is compared against the stored distance.

`light_range` is read from the existing `direction_and_range.w` slot of the point light's `GpuLight` record (see `postretro/src/lighting.rs` — slot 2). Both the shadow-map fragment shader (write path) and the forward light-loop (read path) MUST use the same `light_range` value, otherwise the comparison is meaningless.

**Per-pass uniform upload: dynamic-offset buffer (required).** The depth-only fragment shader for point lights needs `light_pos` and `light_range`. The natural first attempt — one small uniform buffer, call `queue.write_buffer(buf, 0, params)` before each face pass, rebind — **silently produces wrong shadows**. WebGPU's `queue.write_buffer` calls within a single submit do not interleave with encoded passes; all writes execute at submit start, and multiple writes to the same offset collapse to the last write. Every face (and every light) ends up seeing the *last-written* matrix.

The fix: allocate one uniform buffer wide enough to hold one parameter block per `(light_slot, face)` pair (padded to `min_uniform_buffer_offset_alignment`, typically 256 bytes), write each pass's parameters to a unique offset, and bind the buffer with a **dynamic offset** at `set_bind_group` time. The same architecture applies to the per-pass view-projection matrix (one region per pass slot: `CSM_TOTAL_LAYERS + MAX_POINT_SHADOW_LIGHTS * 6 + MAX_SPOT_SHADOW_LIGHTS` regions total).

This trap is not specific to point lights — it hits any multi-pass shadow architecture. CSM cascades and spot lights use the same dynamic-offset strategy.

**Depth encoding:** Store linear depth (distance from light / light range) rather than perspective-Z. Linear depth avoids the non-linear precision distribution of perspective projection, which matters for omnidirectional shadow maps where nearby faces have very different depth ranges.

**Depth write:** The linear-depth fragment shader writes `@builtin(frag_depth)` explicitly. The depth-target pipeline must have `depth_write_enabled: true` and still use `CompareFunction::Less` against the stored value (fragments with smaller normalized distance win). The comparison sampler on the read side uses `CompareFunction::Less` as well — the two compare functions must match.

**Beyond-range fragments:** `dist / light_range` can exceed 1.0 for fragments past the light's range. WebGPU clips `@builtin(frag_depth)` outside [0, 1], so those fragments leave the depth buffer at the clear value (1.0) — which is the correct "no occluder" answer. Benign but worth knowing.

---

## Spot lights — Single Shadow Map

### Layout

Each shadow-casting spot light renders into a **1024×1024** single `texture_depth_2d`. Uses a perspective projection with the spot's outer cone angle as the FOV.

### Rendering

Single render pass per spot light. The view-projection matrix:

- **View:** `Mat4::look_to_rh(light_pos, light_dir, up)`. Pick `up = Vec3::Z` if `|light_dir.y| > 0.99`, else `up = Vec3::Y`. A spot light pointing along ±Y with `up = Y` produces a singular `look_at`; the guard swaps to Z in that case.
- **Projection:** `Mat4::perspective_rh(fov, aspect=1.0, near=0.1, far=light_range)`. FOV is `2.0 * outer_cone_angle` (the cone half-angle doubled) clamped below π. The `far` plane is the light's range so the depth buffer uses the full [0, 1] range over the cone.

Renders all world geometry depth-only. Uses the standard depth-only pipeline (back-face culling, fixed-function depth write) — no Y-flip required for spot lights because the 2D sample path does the standard NDC→UV Y-flip in the fragment shader.

### Fragment shader sampling

```
// pseudocode
fn sample_spot_shadow(frag_world_pos: vec3<f32>, light_index: u32) -> f32 {
    let light_space_pos = spot_view_proj[light_index] * vec4(frag_world_pos, 1.0);
    let ndc = light_space_pos.xyz / light_space_pos.w;
    let shadow_uv = ndc.xy * 0.5 + 0.5;
    // Y-flip: NDC Y-up vs texture V Y-down. Same pattern as CSM.
    let uv = vec2<f32>(shadow_uv.x, 1.0 - shadow_uv.y);
    let depth = ndc.z;
    // Reject fragments outside the shadow frustum (behind or beyond the cone).
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return 1.0;
    }
    return textureSampleCompareLevel(
        spot_shadow_array, shadow_sampler, uv, light_index, depth
    );
}
```

Spot shadow maps stored in a `texture_depth_2d_array`, one layer per shadow-casting spot light. (The scope header's "single 2D shadow maps" refers to one logical map per light; they are physically packed in an array for bind-group efficiency.)

---

## Shadow map texture organization

| Light type | Texture type | Resolution | Layers |
|-----------|-------------|-----------|--------|
| Directional | `texture_depth_2d_array` | 1024² | 3 per directional light (cascades) |
| Point | `texture_depth_2d` with `MAX_POINT_SHADOW_LIGHTS × 6` layers, viewed as `texture_depth_cube_array` | 512² | 6 per point light |
| Spot | `texture_depth_2d_array` | 1024² | 1 per spot light |

All shadow maps use `Depth32Float` format (same as the main depth buffer) with a `comparison` sampler using `CompareFunction::Less`. The sampler uses `FilterMode::Nearest` for both `min_filter` and `mag_filter` — the hard, chunky edges are part of the retro aesthetic. Using `Linear` here silently turns on hardware PCF-like blending and softens the look.

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
@group(2) @binding(5) var point_shadow_array: texture_depth_cube_array;
@group(2) @binding(6) var spot_shadow_array: texture_depth_2d_array;
@group(2) @binding(7) var<storage, read> spot_view_proj: array<mat4x4<f32>>;
```

The binding layout above is the committed shape — the implementation uses it verbatim. The key constraint: all shadow data must be accessible in the fragment shader during the light loop. The shadow pass render bindings (group 0 for each shadow pass) are separate; they use dynamic-offset uniform buffers for the per-pass view-projection and point-light params — see §Per-face rendering.

---

## GpuLight struct extension

The `GpuLight` struct from sub-plan 3 already reserves a fifth vec4 slot named `shadow_info` (zero-initialized at upload time). Sub-plan 5 activates that slot — no new fields, no size change. Total size stays at **80 bytes** (5 × vec4<f32>).

```
struct GpuLight {                              // 80 bytes — unchanged
    position_and_type: vec4<f32>,
    color_and_falloff_model: vec4<f32>,
    direction_and_range: vec4<f32>,
    cone_angles_and_pad: vec4<f32>,
    shadow_info: vec4<f32>,                    // x: reserved (see note below)
                                               // y: bitcast<f32>(shadow_map_index: u32)
                                               // z: bitcast<f32>(shadow_kind: u32) — 0 none, 1 CSM, 2 cube, 3 spot-2d
                                               // w: unused (reserved)
}
```

`shadow_map_index` is the light's slot in the type-specific pool (CSM directional slot for directionals — the shader multiplies by `CSM_CASCADE_COUNT` to get the base array layer; cube slot for points; array layer for spots). `shadow_kind` is redundant with `light_type` in the current design but is stored explicitly so that an unshadowed light of any type is a single branch on `shadow_kind == 0` without needing to re-derive which shadow array to consult.

**`shadow_info.x` is functionally dead.** The CPU packer writes `1` for lights that received a shadow slot and `0` for those that did not (see `ShadowSlotPool::assign` in `lighting/shadow.rs`), but the forward shader never reads `.x` — the shadow sample path branches solely on `shadow_kind == 0`. The field is effectively documentation for the CPU-side assignment state. Leaving it populated is harmless; a future cleanup can repurpose the slot without touching the shader.

The CPU pack routine in `postretro/src/lighting.rs` (`pack_light`) is updated to write bytes 64..80 according to the resolved shadow assignment instead of zeroing them.

---

## Depth-only pipeline

Two render pipelines are needed — one shared by directional + spot, one for points (because point shadows require a fragment shader and inverted culling).

**Shared pipeline (directional + spot):**
- **Vertex shader:** transforms position only (no UV, no normal needed). Can reuse the world vertex buffer with a simpler vertex shader that reads only position.
- **Fragment shader:** none (true depth-only). Standard perspective/orthographic depth is written by the fixed-function depth output.
- **Color attachments:** none
- **Depth attachment:** the shadow map layer
- **Cull mode:** back-face (same as forward pass).
- **Depth bias:** hardware `DepthBiasState` with `constant`, `slope_scale`, and `clamp = 0.0`. See bias tuning note below.

**Point light pipeline:**
- **Vertex shader:** same as shared pipeline.
- **Fragment shader:** minimal linear-depth writer. `@builtin(frag_depth) = length(frag_pos - light_pos) / light_range`. Fixed-function depth output can't express this.
- **Color attachments:** none
- **Depth attachment:** the shadow map layer (one per cube face per light).
- **Cull mode:** **front-face** — the Y-flip in the point light projection matrix (see §Per-face rendering) inverts triangle winding in screen space. This flip is required, not a tuning choice.
- **Depth bias:** `DepthBiasState::default()` — hardware depth bias has no effect on point shadows because the fragment shader writes `frag_depth` explicitly. Bias is applied **shader-side** in the sampler (see the slope-scaled bias in §Fragment shader sampling above).

**Bias tuning (directional + spot).** The wgpu `DepthBiasState` has `constant: i32`, `slope_scale: f32`, `clamp: f32`. For `Depth32Float` the `constant` units are floating-point ULPs scaled by an implementation-defined factor — old GL-era "constant=2, slope=2.0" values typically need scaling by orders of magnitude. The current implementation uses `constant: 4, slope_scale: 2.0, clamp: 0.0`, arrived at by observation. Tune by starting small, doubling until acne disappears, then checking for peter-panning. Document final values in code.

---

## Acceptance criteria

- [ ] Depth-only render pipeline created for directional + spot shadow passes
- [ ] Separate point shadow pipeline with fragment shader writing `frag_depth` and front-face culling
- [ ] CSM passes render directional lights into cascade array layers
- [ ] Cascade split computed from camera near/far range (log/linear blend, λ=0.5)
- [ ] Cascade ortho projection fits the frustum slice with an extended near plane for off-screen casters
- [ ] Cube shadow map passes render point lights (6 faces each) with Y-flipped projection
- [ ] Point shadow maps encode linear depth; forward shader applies slope-scaled shader-side bias
- [ ] Spot shadow map passes render spot lights with cone-matched perspective projection
- [ ] Per-pass uniforms use dynamic-offset buffers (unique region per pass slot — cascades, cube faces, spot lights) to avoid queue-write coalescing
- [ ] Shadow comparison sampler created with `CompareFunction::Less` and `FilterMode::Nearest`
- [ ] Fragment shader samples the correct shadow map per light during the light loop (branches on `shadow_kind`)
- [ ] `cast_shadows` flag on the map light controls whether a shadow slot is assigned
- [ ] Shadow bias eliminates acne on lit surfaces (`constant: 4, slope_scale: 2.0` for directional/spot; shader-side slope bias for point)
- [ ] Shadow edges are hard/chunky (nearest sampling, no PCF)
- [ ] Unshadowed lights still render correctly (`shadow_kind == 0` skips the sample)
- [ ] Fixed slot pool allocated at level load (2 CSM, 16 point cube, 16 spot 2D)
- [ ] Slot assignment uses `visible_lights` from sub-plan 4 (or all shadow-casters if sub-plan 4 absent), sorted by distance from camera
- [ ] Lights beyond the pool degrade to unshadowed for that frame without error
- [ ] Slot cache avoids re-rendering shadow maps for lights that retained their slot from the previous frame (CSM excepted — camera-dependent)
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
- **`texture_depth_cube_array` is a baseline requirement.** The implementation commits to the cube-array path unconditionally. Target backends (Vulkan, Metal, DX12) all support it. If a future backend target lacks the feature, the fallback would be individual `texture_depth_cube` bindings per point light — but that is a significant architectural change, not a trivial flag flip, and should be scoped as its own plan.
- **Shadow map atlas** (packing all shadow maps into one large texture) is a common optimization but adds complexity. Skip it — the fixed slot pool keeps the number of textures bounded regardless of how many lights the level contains.
- **Front-face vs. back-face culling.** Directional and spot shadow passes use back-face culling (matches the forward pass). Point shadow passes use **front-face culling** because of the mandatory Y-flip in the projection matrix (see §Per-face rendering) — this is a correctness requirement, not an acne-tuning choice. If shadow acne persists after bias tuning, the fix is **not** to flip culling; that breaks the Y-flip invariant. Instead, increase bias or investigate the geometry.
- **Reverse-Z is not used.** The forward pipeline uses standard depth (0 near, 1 far) with `CompareFunction::Less` (verified in `postretro/src/render.rs`). Shadow pipelines must match — do not silently introduce reverse-Z in shadow passes, it will make `Less` comparisons meaningless.
- **Queue-write coalescing trap.** `queue.write_buffer` calls within a single submit do not interleave with encoded passes. All writes execute at submit start; multiple writes to the same buffer offset collapse to the last write. Any per-pass uniform upload (CSM cascades, point light faces, spot lights) must use a dynamic-offset buffer with unique regions per pass — see §Per-face rendering for the architecture. A naive "write + bind, repeat" implementation will silently produce wrong shadows.
