# Sub-plan 5 — Sun Shadows (CSM)

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals.
> **Scope:** Cascaded shadow maps for directional lights (the sun). Hard-edged, high-detail shadows for the primary light source, serving as a sharpness overlay on top of the SDF penumbra path from sub-plan 9.
> **Crates touched:** `postretro` only.
> **Depends on:** sub-plan 3 (direct lighting must be working — shadows modulate the direct term). **Benefits from sub-plan 4** (light influence volumes; for directional lights the influence volume is effectively infinite, but the same machinery gates CSM slot assignment).
> **Blocks:** nothing directly. Sub-plans 6–9 are independent of CSM.

---

## Description

Directional lights (sun / sky) are shadowed via cascaded shadow maps. Point and spot shadows are handled by sub-plan 9's sphere-traced SDF path, which gives uniform-quality soft shadows across all omnidirectional lights without the six-face cube map overhead. CSM is retained for the sun because (a) the sun is the highest-contrast light in most maps and wants hard-edge detail, (b) sphere-tracing the entire visible frustum from a directional light is expensive relative to an orthographic rasterization, and (c) CSM already matches the chunky aesthetic when sampled nearest.

Shadow resolution is intentionally modest. Chunky shadow edges with nearest-neighbor sampling are part of the retro aesthetic — not a defect to fix with PCF or VSM.

---

## Shadow pass structure

CSM passes run **after** the compute BVH cull and **before** the opaque forward pass.

Updated frame order:
1. CPU portal traversal → visible-cell bitmask
2. Compute BVH cull (existing)
3. **CSM passes** (new — one per cascade per directional light)
4. Opaque forward pass (existing, now with shadow sampling)
5. Debug wireframe overlay (existing, optional)

### Which lights get CSM

Only directional lights with `cast_shadows == true` receive CSM. Point and spot shadow contribution comes from sub-plan 9 (SDF sphere-trace); their `shadow_kind` branches in the fragment shader route to the SDF path.

---

## Directional lights — Cascaded Shadow Maps (CSM)

### Layout

3 cascades (start with 3; add a 4th if coverage is insufficient on test maps). Each cascade is a **1024×1024** depth texture layer in a `texture_depth_2d_array`.

Cascade split scheme: practical split (logarithmic/linear blend, λ ≈ 0.5) based on the camera's near/far range. Each cascade covers a progressively larger frustum slice.

### Per-cascade rendering

For each cascade:
1. Compute the light-space orthographic projection matrix that fits the cascade's frustum slice (algorithm below).
2. Render all world geometry (same vertex buffer, same index buffer) with a depth-only pipeline. No color attachment — only the depth attachment targeting the cascade's array layer.
3. Draw the entire world index buffer in one indexed draw. The shadow pass does not use the per-frame BVH cull state — it renders all static geometry from the light's viewpoint. (Shadow-specific culling is a follow-up optimization if shadow pass cost is measurable.)

### Cascade ortho fitting algorithm

Tightly fitting an orthographic projection to a frustum slice is the most error-prone piece of CSM. The algorithm uses a **bounding-sphere** approach for rotation-invariant extent, which is essential for texel snapping:

1. **Recover the 8 frustum-slice corners in world space.** The cascade covers view-space depths `[split_near, split_far]`. Map each to NDC Z (wgpu's standard depth is [0, 1]), then unproject the 8 corners `(±1, ±1, ndc_z)` through `inverse(view_proj)` with a w-divide to land in world space.
2. **Compute the bounding sphere** of the 8 corners — center = mean, radius = max distance from center. Ceil the radius to suppress floating-point noise across frames.
3. **Build a light-space view matrix.** `Mat4::look_to_rh(Vec3::ZERO, light_dir, up)`. Pick an `up` vector that is not parallel to `light_dir`: `up = Vec3::Z` if `|light_dir.y| > 0.99`, else `up = Vec3::Y`. Using a singular `up` produces a NaN view matrix and invisible shadows.
4. **Construct a fixed-extent AABB** centered on the sphere in light space: min = light_center − radius, max = light_center + radius. Because the sphere radius depends only on the frustum slice shape (not its orientation), the AABB extent is invariant under camera rotation — this is what makes texel snapping stable.
5. **Snap the xy origin to the shadow texel grid.** Compute texel size as `2 × radius / shadow_resolution`, then `snapped_min_x = floor(min.x / texel_x) * texel_x` and shift the max by the same delta. Same for y.
6. **Push the near plane back** (toward the light) by a generous margin — typical value 500 world units — so shadow casters that live behind the camera frustum slice but still shadow into it are captured. A too-tight near plane clips off-screen occluders and causes shadow pop-in at cascade boundaries.

**Reference implementation:** `postretro/src/lighting/shadow.rs::fit_cascade_bounds` and `cascade_ortho_matrix`.

### Fragment shader sampling

```
// pseudocode
fn sample_csm(frag_world_pos: vec3<f32>, cascade_index: u32) -> f32 {
    let light_space_pos = csm_view_proj[cascade_index] * vec4(frag_world_pos, 1.0);
    // Perspective divide — for orthographic projection w == 1 so this is a
    // no-op, but always divide so the pattern matches other shadow pseudocode
    // and is robust if the cascade ever uses a non-ortho matrix.
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

**No PCF, no filtering.** `nearest` comparison produces hard shadow edges. This is intentional — sub-plan 9's SDF path provides the penumbrae on top when soft edges are wanted.

---

## Shadow map texture organization

| Light type | Texture type | Resolution | Layers | Source |
|-----------|-------------|-----------|--------|--------|
| Directional | `texture_depth_2d_array` | 1024² | 3 per directional light (cascades) | this sub-plan |
| Point | — | — | — | sub-plan 9 (SDF sphere-trace) |
| Spot | — | — | — | sub-plan 9 (SDF sphere-trace) |

CSM uses `Depth32Float` format (same as the main depth buffer) with a `comparison` sampler using `CompareFunction::Less`. The sampler uses `FilterMode::Nearest` for both `min_filter` and `mag_filter` — the hard, chunky edges are part of the retro aesthetic. Using `Linear` here silently turns on hardware PCF-like blending and softens the look.

CSM textures are allocated from a **fixed slot pool** at level load. Directional lights are rare (0–2 per level), so the pool is small.

---

## Shadow-slot pool sizing

| Pool | Slot count | Per-slot VRAM | Total VRAM |
|------|-----------|--------------|------------|
| CSM (directional) | 2 lights × 3 cascades = 6 layers | 1024² × 4 bytes = 4 MB | 24 MB |

The SDF path (sub-plan 9) has its own texture budget — no cube or spot shadow map pools exist in this pipeline.

### Slot assignment policy

Directional lights always get a CSM slot (0–2 lights, pool of 2). No sort needed — the pool is large enough to cover all directional lights in any supported level.

---

## Bind group changes

Extend **group 2 (lighting)** with CSM bindings. Sub-plan 9 adds its own SDF bindings alongside these:

```
@group(2) @binding(0) var<storage, read> lights: array<GpuLight>;             // from sub-plan 3
@group(2) @binding(1) var<storage, read> light_influence: array<vec4<f32>>;   // from sub-plan 4
@group(2) @binding(2) var shadow_sampler: sampler_comparison;
@group(2) @binding(3) var csm_depth_array: texture_depth_2d_array;
@group(2) @binding(4) var<storage, read> csm_view_proj: array<mat4x4<f32>>;
// bindings 5+ reserved for sub-plan 9 (SDF atlas, sampler, metadata)
```

The shadow pass render bindings (group 0 for each shadow pass) are separate; they use dynamic-offset uniform buffers for the per-pass view-projection — see §Per-cascade rendering.

---

## GpuLight struct — shadow info

The `GpuLight` struct from sub-plan 3 reserves a fifth vec4 slot named `shadow_info` (zero-initialized at upload time). Sub-plan 5 populates it for directional lights; sub-plan 9 populates it for point and spot lights. Total size stays at **80 bytes** (5 × vec4<f32>).

```
struct GpuLight {                              // 80 bytes
    position_and_type: vec4<f32>,
    color_and_falloff_model: vec4<f32>,
    direction_and_range: vec4<f32>,
    cone_angles_and_pad: vec4<f32>,
    shadow_info: vec4<f32>,                    // x: reserved
                                               // y: bitcast<f32>(shadow_map_index: u32)
                                               // z: bitcast<f32>(shadow_kind: u32)
                                               //    0 = none, 1 = CSM, 2 = SDF sphere-trace
                                               // w: unused (reserved)
}
```

`shadow_map_index` for CSM is the light's directional slot (the shader multiplies by `CSM_CASCADE_COUNT` to get the base array layer). Sub-plan 9 reinterprets this field for SDF lights.

`shadow_kind == 0` (unshadowed) skips all shadow sampling. `shadow_kind == 1` routes to `sample_csm`. `shadow_kind == 2` routes to `sample_sdf_shadow` from sub-plan 9.

The CPU pack routine in `postretro/src/lighting.rs` (`pack_light`) writes bytes 64..80 according to the resolved shadow assignment.

---

## Depth-only pipeline

Single depth-only render pipeline for CSM:

- **Vertex shader:** transforms position only (no UV, no normal needed). Can reuse the world vertex buffer with a simpler vertex shader that reads only position.
- **Fragment shader:** none (true depth-only). Standard orthographic depth is written by the fixed-function depth output.
- **Color attachments:** none
- **Depth attachment:** the cascade's array layer
- **Cull mode:** back-face (same as forward pass).
- **Depth bias:** hardware `DepthBiasState` with `constant`, `slope_scale`, and `clamp = 0.0`. See bias tuning note below.

**Bias tuning.** The wgpu `DepthBiasState` has `constant: i32`, `slope_scale: f32`, `clamp: f32`. For `Depth32Float` the `constant` units are floating-point ULPs scaled by an implementation-defined factor — old GL-era "constant=2, slope=2.0" values typically need scaling by orders of magnitude. The current implementation uses `constant: 4, slope_scale: 2.0, clamp: 0.0`, arrived at by observation. Tune by starting small, doubling until acne disappears, then checking for peter-panning. If acne persists at low resolutions, consider normal-offset bias (shift the receiver along its surface normal by a small amount in light space) as a follow-up — it scales better than constant+slope bias at low CSM resolutions.

---

## Per-pass uniform upload

The depth-only shader for CSM needs the per-cascade view-projection matrix. The natural first attempt — one small uniform buffer, call `queue.write_buffer(buf, 0, vp)` before each cascade pass, rebind — **silently produces wrong shadows**. WebGPU's `queue.write_buffer` calls within a single submit do not interleave with encoded passes; all writes execute at submit start, and multiple writes to the same offset collapse to the last write. Every cascade ends up seeing the last-written matrix.

The fix: allocate one uniform buffer wide enough to hold one matrix block per cascade slot (padded to `min_uniform_buffer_offset_alignment`, typically 256 bytes), write each pass's matrix to a unique offset, and bind the buffer with a **dynamic offset** at `set_bind_group` time.

This trap is not specific to CSM — it hits any multi-pass GPU architecture.

---

## Acceptance criteria

- [x] Depth-only render pipeline created for CSM passes
- [x] CSM passes render directional lights into cascade array layers
- [x] Cascade split computed from camera near/far range (log/linear blend, λ=0.5)
- [x] Cascade ortho projection uses bounding-sphere extent for rotation-invariant texel snapping
- [x] Near plane pushed back for off-screen casters
- [x] Per-pass uniforms use dynamic-offset buffers (unique region per cascade slot) to avoid queue-write coalescing
- [x] Shadow comparison sampler created with `CompareFunction::Less` and `FilterMode::Nearest`
- [x] Fragment shader samples the correct cascade during the light loop (branches on `shadow_kind == 1`)
- [x] `cast_shadows` flag on the map light controls whether a CSM slot is assigned
- [x] Shadow bias eliminates acne on lit surfaces (`constant: 4, slope_scale: 2.0`)
- [x] Shadow edges are hard/chunky (nearest sampling, no PCF)
- [x] Unshadowed lights still render correctly (`shadow_kind == 0` skips the sample)
- [x] Fixed CSM slot pool allocated at level load (2 directional lights × 3 cascades)
- [x] Property-based test (`proptest`): small yaw rotations produce either identical bounds or a clean single-texel step
- [x] All test maps render with correct sun shadow coverage and no light leaks at shadow edges
- [x] `cargo test -p postretro` passes
- [x] `cargo clippy -p postretro -- -D warnings` clean

---

## Implementation tasks

1. ✅ Create depth-only render pipeline (vertex-only, no color attachment).
2. ✅ Implement CSM: cascade split calculation, per-cascade bounding-sphere fitting, texel snapping, per-cascade depth render pass, cascade array texture allocation.
3. ✅ Allocate CSM shadow map textures at level load. Create shadow sampler with comparison function.
4. ✅ Extend `GpuLight` with shadow info. Upload CSM view-projection matrices to a dynamic-offset buffer.
5. ✅ Extend fragment shader light loop: sample CSM per shadow-casting directional light, multiply direct contribution by shadow factor.
6. ✅ Tune depth bias to eliminate shadow acne across test maps.
7. ✅ Add texel-snapping proptest for shimmer stability.

---

## Shadow map update cadence

All map-authored lights in Postretro are static (position, direction, color, and range are baked into the PRL and never mutated at runtime). The sun is static — but **CSMs depend on the camera frustum** (cascade splits and bounds change with view), so they re-render every frame regardless.

With up to 2 directional lights × 3 cascades × full-scene geometry, this is 6 depth passes per frame — the dominant CPU cost in this sub-plan. Shadow-specific BVH culling per cascade becomes more valuable at dense levels; add it as a follow-up optimization if profiling shows CSM rendering is a bottleneck.

---

## Interaction with portal visibility

The forward pass is driven by the CPU portal-traversal visible-cell bitmask. **CSM passes ignore that bitmask entirely** — shadow maps are rendered from the light's viewpoint, not the camera's, so the camera's visible cells are not a meaningful cull set for a shadow pass.

Do not reuse the camera visibility bitmask to skip shadow geometry. If CSM cost becomes a problem, add per-cascade frustum culling driven by the cascade's own bounds, not the camera's portal set.

---

## Notes for implementation

- **No shadow-specific BVH cull.** The initial cut renders all world geometry into every cascade. This is simple and correct. If profiling shows CSM passes are expensive, add per-cascade frustum culling as a follow-up — the BVH is already there.
- **Reverse-Z is not used.** The forward pipeline uses standard depth (0 near, 1 far) with `CompareFunction::Less` (verified in `postretro/src/render.rs`). CSM pipelines must match — do not silently introduce reverse-Z in shadow passes, it will make `Less` comparisons meaningless.
- **Queue-write coalescing trap.** `queue.write_buffer` calls within a single submit do not interleave with encoded passes. All writes execute at submit start; multiple writes to the same buffer offset collapse to the last write. Per-cascade uniform upload must use a dynamic-offset buffer with unique regions per cascade.
- **Normal-offset bias at low resolutions.** If CSM resolution is reduced for aesthetic reasons (e.g., 256²), constant + slope-scaled depth bias becomes brittle — surfaces near-parallel to the light direction develop acne that bias can only mask with peter-panning. Normal-offset bias (shift the receiver along its surface normal in light space before depth comparison) scales more gracefully and should be the first tuning lever if the constant/slope approach can't find a clean window.
