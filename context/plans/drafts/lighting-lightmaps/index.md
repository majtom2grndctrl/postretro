# Lighting — Directional Lightmaps

> **Status:** draft.
> **Depends on:** `lighting-dynamic-flag/` (needs `MapLight.is_dynamic`). `lighting-old-stack-retirement/` should ship first.
> **Concurrent with:** `lighting-sh-amendments/`, `lighting-chunk-lists/`, `lighting-spot-shadows/`.
> **Related:** `context/lib/build_pipeline.md` · `context/plans/done/bvh-foundation/` · `context/lib/rendering_pipeline.md` §4.

---

## Context

Static lights currently have no baked direct-lighting representation — the original lighting foundation explicitly skipped lightmaps in favour of SDF sphere-traced shadows (now retired). This plan adds directional lightmaps: the bumped-lightmap technique from Half-Life 2 / Source engine, where each atlas texel stores irradiance and a dominant incoming light direction. At runtime, normal-mapped surfaces re-evaluate Lambert using the stored direction, recovering per-texel normal-map response to static lighting without tripling atlas memory (radiosity normal mapping's three-basis alternative).

All shadow rays use the Milestone 4 BVH. One acceleration structure, multiple consumers — the pattern established by the original lighting foundation.

---

## Goal

- **Compiler:** new lightmap baker stage in `prl-build` that UV-unwraps geometry, ray-casts per-texel irradiance and dominant direction from static lights, and writes a `Lightmap` PRL section.
- **Runtime:** load the atlas, upload to GPU, and sample per-fragment with bumped-Lambert correction in `forward.wgsl`.

After both tasks ship, static lights contribute hard-shadowed diffuse with correct normal-map response. Specular from static lights is handled by `lighting-chunk-lists/`, not this plan.

---

## Concurrent workstreams

The two tasks can start simultaneously. The runtime task can use a stub — 1×1 white irradiance, neutral direction — until the compiler task produces real `.prl` data. The compiler task does not depend on the runtime at all.

```
Task A (compiler): baker → PRL section ──────────────────┐
                                                          ├── integration test
Task B (runtime): upload + shader sampling ── stub first ┘
```

---

## Task A — Lightmap baker

**Crate:** `postretro-level-compiler` · **New module** under `src/bake/`.

1. **UV unwrap.** Integrate `xatlas` for automatic per-chart UV unwrapping. Pack charts into an atlas with configurable texel density (default: 4 cm/texel; per-level overrideable via a map property).
2. **Per-texel ray-cast.** For each atlas texel, resolve the world-space position on the source face. For each static light (`is_dynamic == false`), cast a shadow ray through the Milestone 4 BVH. If unoccluded, accumulate Lambert × color × falloff into irradiance.
3. **Dominant direction.** Alongside irradiance, track the irradiance-weighted mean incoming light direction. Normalize after all lights are accumulated.
4. **Encoding.** Irradiance: RGB half-float or HDR-packed 8-bit (chosen during implementation based on memory budget). Direction: two-channel octahedral encoding or RGB. Final encoding documented in the PRL section definition.
5. **Edge dilation.** Extend valid texels past chart edges by at least one texel to prevent bilinear bleed across chart seams.
6. **Lightmap UVs.** Emit per-vertex lightmap UV attributes into the geometry vertex stream. Coordinate with the runtime task's vertex layout expectation. This is an extension of the existing vertex format — all consumers of the geometry section need to handle the extra attribute.

**`Lightmap` PRL section.** New section ID in `postretro-level-format`. Section header: width, height, encoding enum, texel density. Irradiance texture bytes followed by direction texture bytes (or co-packed if encoding allows). PRL format coordination note: `lighting-chunk-lists/` also adds a new section ID; the two registrations land independently but must not collide — assign IDs at implementation time against the current max.

### Task A acceptance gates

- Compiling `assets/maps/test.map` produces a `.prl` with a populated `Lightmap` section.
- A test map with a single static point light shows per-texel irradiance falloff matching expected distance-squared (or configured falloff model).
- A static light behind a wall from a surface produces a zero (or near-zero) irradiance texel on the occluded surface.
- Edge-dilation pass runs without leaving un-dilated boundary texels on any chart at 4 cm/texel density.

---

## Task B — Lightmap runtime sampling

**Crate:** `postretro` · **New module:** `src/lighting/lightmap.rs` · **Also modifies:** `src/render/mod.rs`, `src/shaders/forward.wgsl`.

1. **Load + upload.** Parse the `Lightmap` PRL section at level load. Upload atlas as GPU textures. Two separate textures or one RGBA-packed texture depending on encoding choice in Task A. Sampler: linear filtering (lightmaps are continuous-signal data, not nearest-shadow-sample data).
2. **Bind group.** Add entries to group 2 (the lighting group). Coordinate with `lighting-chunk-lists/` and `lighting-spot-shadows/` to avoid binding slot collisions.
3. **Missing-section fallback.** If `Lightmap` is absent, bind a 1×1 white irradiance placeholder and neutral direction. Degraded but functional — compiles run against work-in-progress bakes.
4. **Shader sampling.** Per fragment in `forward.wgsl`, using the lightmap UV vertex attribute from Task A:

```wgsl
let lm_irr = textureSample(lightmap_irr, lm_sampler, in.lightmap_uv).rgb;
let lm_dir = decode_direction(textureSample(lightmap_dir, lm_sampler, in.lightmap_uv));

let mesh_ndotl   = max(dot(in.world_normal, lm_dir), 0.001);
let bumped_ndotl = max(dot(normal_mapped,   lm_dir), 0.0);
let static_direct = lm_irr * (bumped_ndotl / mesh_ndotl);
```

`static_direct` replaces the old per-fragment static-light contribution.

### Task B acceptance gates

- A test map compiled with Task A's output renders with lightmap-baked hard shadows on static geometry.
- With the missing-section fallback active (no `Lightmap` in the `.prl`), the engine runs without panic and static geometry renders with flat-white direct contribution.
- Normal-mapped surface under static lighting shows normal-map detail (bumped-Lambert responds to stored direction).
- Specular is absent — confirming no accidental specular from the lightmap path (specular arrives in `lighting-chunk-lists/`).

---

## Acceptance Criteria (both tasks)

1. `cargo test --workspace` passes.
2. `cargo clippy --workspace -- -D warnings` clean.
3. No new `unsafe`.
4. Task A and Task B acceptance gates above.
5. Toggling a light's `_dynamic` flag: a static light bakes its shadow into the lightmap; marking it dynamic removes it from the bake (lightmap shows no shadow from that light), confirming the `is_dynamic` filter in the baker.
6. Frame time on the dense-light test map does not regress materially (`POSTRETRO_GPU_TIMING=1` before/after).

---

## Out of scope

- Specular from static lights — `lighting-chunk-lists/`.
- Radiosity normal mapping (three-basis lightmaps).
- Lightmap texture compression (BC6H etc.) — raw encoded atlas for now.
- Per-level tuning of texel density beyond the default — retune if evidence demands.
- Dynamic light baking — dynamic lights skip the baker entirely.
