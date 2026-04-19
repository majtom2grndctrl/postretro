# Sub-plan 8 — SDF Atlas + Sphere-Traced Soft Shadows

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals.
> **Scope:** A baked signed distance field covering all static world geometry, sampled at runtime via sphere tracing for soft shadows on point and spot lights. CSM (sub-plan 5) continues to carry directional/sun shadows; the SDF provides uniform-quality soft shadows for every other shadow-casting light.
> **Crates touched:** `postretro-level-compiler` (baker + PRL section) and `postretro` (runtime sampler + shader).
> **Depends on:** sub-plan 3 (direct lighting — shadows modulate the direct term) and the Milestone 4 BVH (baker ray-casts through it). **Benefits from sub-plan 4** (light influence volumes gate which lights actually sphere-trace each frame).
> **Blocks:** sub-plan 9 (specular maps build on the direct lighting term that shadows feed into).

---

## Description

Point and spot shadows are notoriously uneven in classical pipelines: cube shadow maps have texel-density variation across faces and six rasterization passes per light; 2D spot shadow maps are fine at a narrow cone but don't help the dozens of point lights that fill a typical level. Sphere-tracing a pre-baked SDF of the world gives:

- Uniform quality for point, spot, and any other omnidirectional light type
- Soft penumbrae as a natural property of the cone-traced march (no PCF or VSM plumbing)
- One GPU pass per frame for all lights, not six passes per light
- Zero per-light rasterization cost — cost scales with pixels × visible-lights × march steps

The tradeoff is a bake step (sub-second to minutes depending on resolution), a chunk of VRAM for the SDF atlas, and sphere-trace cost in the fragment shader. Pre-RTX hardware handles this comfortably at low-res + upscaled resolution; the budget needs watching as visible light count grows.

---

## Addressing: chunk-friendly brick indirection

The SDF atlas must be addressable in a way that survives the eventual introduction of the universal chunk primitive (Milestone 8). The baker **must not** hardcode world-space voxel indexing into the runtime sampler. Instead:

- The atlas is a **brick-indexed sparse field**: the world is tiled into bricks (e.g., 8³ voxels each), only bricks that contain surface detail are stored, and a top-level index maps world-space brick coordinates to brick slots in the atlas.
- The top-level index is a 3D texture of `u32` brick slot indices (0 = empty brick, sentinel for "fully outside" or "fully inside" can occupy reserved slot IDs).
- Runtime lookup: `brick_slot = topLevel[worldToBrickCoord(p)]; sdf_value = atlas[brickSlot * brick_volume + localCoord(p)]`.

When chunks land in Milestone 8, the brick table becomes per-chunk (each chunk owns its brick range; the top-level index resolves through the chunk record first). The runtime shader changes are additive, not structural — the brick indirection stays the same. This is the "design now so the migration is additive later" hedge.

---

## Bake pipeline

### Baker stage

A new stage in `prl-build`, scheduled after BVH construction (depends on the Milestone 4 BVH for efficient signed-distance queries). Inputs: the triangle set the BVH was built from. Outputs: a new PRL section — `SdfAtlas` (ID TBD, next available).

1. **World bounds.** Axis-aligned bounds of all static geometry, expanded by a configurable margin (default 1 m) so SDF gradients are valid near surfaces.
2. **Voxel grid.** Voxel size tuned for the target aesthetic and hardware budget. Starting value: **8 cm per voxel**. Brick size: 8³ voxels = 64 cm per brick.
3. **Brick classification.** Walk the world bounds in brick-sized steps. For each brick, query the BVH for any triangle intersecting the brick AABB expanded by one voxel. Classify each brick as:
   - **Empty** (no triangles within reach) — emit a sentinel brick-slot that reports a large positive distance.
   - **Interior** (fully inside solid geometry) — emit a sentinel brick-slot that reports a large negative distance. Determined via parity test from an exterior seed point.
   - **Surface** — brick contains at least one surface voxel; compute per-voxel signed distance.
4. **Per-voxel SDF computation.** For each voxel in a surface brick, compute the signed distance to the nearest triangle via BVH closest-point query. Sign is determined by the winding-consistent nearest-face normal (standard approach).
5. **Brick packing.** Concatenate all surface bricks into a flat atlas buffer. The top-level index records each brick's atlas slot.
6. **Section emission.** Write the `SdfAtlas` PRL section: header (world bounds, voxel size, brick size, atlas dimensions), top-level index as a flat `Vec<u32>`, atlas data as a flat `Vec<i16>` (signed distance quantized to 16-bit; range ±65 m at 2 mm resolution is plenty).

### Bake-time tuning knobs

- `voxel_size_m` — default 0.08. Smaller = sharper shadows, larger atlas, longer bake.
- `brick_size_voxels` — default 8. Larger bricks reduce top-level index size at the cost of sparsity granularity.
- `margin_m` — default 1.0. Expand world bounds so gradients are smooth at surface edges.

### Bake cost estimates

For a typical boomer-shooter level (say, 100 × 100 × 30 m with ~40% brick occupancy):
- Top-level bricks: 125 × 125 × 38 ≈ 600k bricks
- Occupied bricks: ~240k × 512 voxels × 2 bytes = ~240 MB raw atlas

That's too much. **Target budget: 32–64 MB atlas per level.** Mitigations:
- Increase voxel size to 12–16 cm for ambient shadow quality (fine shadow detail comes from CSM for the sun and from direct lighting for nearby point lights).
- Quantize distance values further (8-bit normalized distance within brick-local range is a common trick).
- Aggressive brick sparsity (only store bricks within N voxels of a surface; empty-space queries return the sentinel).

Tune on a real level during implementation; 8 cm at 16-bit is the starting point, not the shipping answer.

---

## Runtime: sphere tracing

### Sampler

```
fn sample_sdf(world_pos: vec3<f32>) -> f32 {
    let brick_coord = world_to_brick(world_pos);
    let brick_slot = top_level[brick_index_linear(brick_coord)];
    if (brick_slot == EMPTY_SENTINEL) { return LARGE_POSITIVE; }
    if (brick_slot == INTERIOR_SENTINEL) { return LARGE_NEGATIVE; }
    let local = brick_local_coord(world_pos);
    // Trilinear sample within the brick. Atlas layout is flat; brick_slot *
    // brick_volume gives the base index.
    return trilinear_sample_brick(brick_slot, local);
}
```

The sampler is used for both shadow rays and (later, in Future work) AO and SSR fallbacks.

### Shadow trace

For a point or spot light with `shadow_kind == 2`:

```
fn sample_sdf_shadow(
    frag_world_pos: vec3<f32>,
    light_pos: vec3<f32>,
    light_range: f32,
    cone_half_angle: f32,  // 0 for point; spot's half-angle drives penumbra width
) -> f32 {
    let to_light = light_pos - frag_world_pos;
    let dist_to_light = length(to_light);
    if (dist_to_light > light_range) { return 1.0; }  // beyond range
    let dir = to_light / dist_to_light;

    // March from just above the surface toward the light.
    var t = SELF_SHADOW_BIAS;  // ~2 voxel widths
    var occlusion = 1.0;
    let k = 1.0 / max(tan(cone_half_angle), MIN_CONE_TAN);  // softness factor
    for (var i = 0u; i < MAX_STEPS; i = i + 1u) {
        let p = frag_world_pos + dir * t;
        let d = sample_sdf(p);
        if (d < SELF_SHADOW_EPSILON) { occlusion = 0.0; break; }
        // Soft-shadow accumulation (Inigo Quilez formula)
        occlusion = min(occlusion, k * d / t);
        t = t + max(d, MIN_STEP);
        if (t > dist_to_light) { break; }
    }
    return clamp(occlusion, 0.0, 1.0);
}
```

Starting constants (to be tuned on real levels):
- `MAX_STEPS`: 32
- `MIN_STEP`: `voxel_size_m * 0.5`
- `SELF_SHADOW_BIAS`: `voxel_size_m * 2.0`
- `SELF_SHADOW_EPSILON`: `voxel_size_m * 0.25`
- `MIN_CONE_TAN`: `0.001` (prevents division by zero for point lights, which have effectively zero cone half-angle; they get a hard-edge trace)

**Point lights** use `cone_half_angle = 0` for the hardest edge the SDF can produce (still softer than a cube map because the soft-shadow formula integrates distance proximity). For a softer look, lights can author a `_penumbra_angle` in the FGD — reserved, not in scope for this sub-plan.

**Spot lights** use their cone outer-half-angle directly as the penumbra width. A tight spotlight gets sharp shadows; a wide flood gets soft ones. This is physically motivated and free given the trace machinery.

### Where the trace runs

The sphere trace runs in the **main fragment shader**, in the same light loop as sub-plan 3, gated by `shadow_kind == 2`. There is no separate SDF shadow pass — that's the architectural win over cube shadow maps. Sub-plan 4's `visible_lights` still culls which lights even enter the light loop, so the trace only runs for lights whose influence volume intersects the view.

### Low-res + upscale (deferred optimization)

The target frame budget for SDF shadow tracing is **1–2 ms total across all visible lights**. At native resolution with 30 visible point lights, 32 march steps each, that's likely over budget on pre-RTX hardware. The standard fix:

1. Render the shadow contribution into a half-resolution render target, one channel per visible shadow-casting light (or a packed format).
2. Bilaterally upsample (depth + normal-aware) back to full resolution before the main light loop uses it.

Defer this until profiling shows native-res sphere-tracing is over budget. The first cut traces at native resolution, on a limited-light scene, to validate correctness.

---

## Bind group changes

Extend **group 2 (lighting)** with SDF bindings (adding to sub-plan 5's CSM bindings):

```
@group(2) @binding(5) var sdf_atlas: texture_3d<f32>;              // per-voxel signed distance
@group(2) @binding(6) var sdf_sampler: sampler;                    // trilinear
@group(2) @binding(7) var<storage, read> sdf_top_level: array<u32>; // brick slot index
@group(2) @binding(8) var<uniform> sdf_meta: SdfMeta;              // bounds, sizes, sentinels
```

`SdfMeta` packs world-bounds min/max, voxel size, brick size, top-level dimensions, and sentinel brick-slot IDs.

---

## GpuLight integration

Point and spot lights with `cast_shadows == true` are assigned `shadow_kind == 2` at light-upload time. `shadow_map_index` is unused for SDF lights (no per-light resource) — the field is zeroed. All shadow state lives in the shared SDF atlas.

---

## Acceptance criteria

- [ ] `SdfAtlas` PRL section defined: header + top-level index + flat brick atlas
- [ ] Baker integrates into `prl-build` after BVH stage; consumes the BVH for closest-point queries
- [ ] Brick classification distinguishes empty / interior / surface bricks; sentinel slots encoded for empty + interior
- [ ] Baker emits valid SDF for all test maps under the default voxel size; bake completes in under 5 minutes on representative levels
- [ ] Atlas size stays within a 64 MB budget on representative levels (tune voxel size per map if needed)
- [ ] Engine loads `SdfAtlas` section into GPU resources (3D texture + storage buffer + uniform block)
- [ ] Fragment shader `sample_sdf` returns correct signed distances (validated via a debug visualization mode)
- [ ] Fragment shader `sample_sdf_shadow` produces soft penumbrae for spot lights with correct cone-angle-driven width
- [ ] Point lights produce hard-edged SDF shadows (no cone softening)
- [ ] Lights with `cast_shadows == false` skip the trace
- [ ] Shadow integrates with sub-plan 4 influence-volume culling (unseen lights do not trace)
- [ ] All test maps render correctly under the unified SDF+CSM shadow pipeline
- [ ] `cargo test -p postretro-level-compiler` and `-p postretro` pass
- [ ] `cargo clippy ... -D warnings` clean on both crates

---

## Implementation tasks

1. **PRL schema** — define `SdfAtlas` section in `postretro-level-format`, with header + top-level index + atlas body layout.
2. **Baker core** — implement brick classification + per-voxel SDF computation against the BVH in `postretro-level-compiler`. Flat atlas packing. Unit tests on small analytic scenes (single cube, two spheres).
3. **Baker integration** — wire into `prl-build` pipeline after BVH stage. Add CLI flags for voxel size and brick size overrides.
4. **Engine loader** — parse `SdfAtlas` section, upload 3D texture + top-level storage buffer + meta uniform.
5. **Shader — sample_sdf** — implement brick-indexed trilinear sample in a standalone WGSL module. Debug visualization mode (render distance as grayscale) to validate.
6. **Shader — sample_sdf_shadow** — sphere trace with soft-shadow accumulation. Integrate into the sub-plan 3 light loop, gated by `shadow_kind == 2`.
7. **Slot assignment** — update the light-upload path so point/spot shadow-casters get `shadow_kind = 2`; directionals keep `shadow_kind = 1` (CSM).
8. **Tune** — pick a test map with varied light setups, walk through march step count / bias / voxel size, establish defaults that hold up.

---

## Notes

- **The sphere trace is the hot path.** Every ms burned here crowds out everything else in the fragment shader. Establish a budget and a measurement habit early. If 30 visible lights × 32 steps blows the budget, the answer is low-res-plus-upscale (deferred task above), not cutting quality knobs in the steady-state trace.
- **Self-shadow bias is resolution-dependent.** 8 cm voxels → 16 cm bias is typical. If voxel size changes, retune.
- **Interior sentinel correctness matters.** Baking a static hero prop inside a solid wall will produce garbage lighting if the interior sentinel is misclassified. A parity test from a known-exterior seed point (the player-start origin) is the standard approach.
- **Chunk migration readiness.** When Milestone 8's chunk primitive lands, the top-level index grows a per-chunk indirection: instead of a single world-space grid, each chunk owns its local brick range and the top-level resolves through the chunk record first. The runtime `sample_sdf` function stays roughly the same shape; the baker does more work (one SDF region per chunk). Keep the baker's brick packing and atlas layout factored so the chunk-aware variant is additive.
- **AO is a free follow-up.** Once the SDF is there, short-range occlusion cones for ambient shading come cheap. Out of scope for this sub-plan but worth noting as a natural next feature.
- **Moving geometry (kinematic clusters, Milestone 9) casts no SDF shadow until that milestone explicitly addresses dynamic SDF contribution.** Until then, the SDF captures only static world geometry. This is a known gap; clusters rely on the CSM path (if sun-lit) or cast no dynamic shadow at all.
