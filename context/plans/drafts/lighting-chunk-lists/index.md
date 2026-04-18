# Lighting — Per-Chunk Light Lists + Specular

> **Status:** draft. Supersedes `context/plans/drafts/perf-per-chunk-light-lists/` — that plan is absorbed here and can be deleted when this plan moves to `ready/`.
> **Depends on:** `lighting-dynamic-flag/` (compiler task needs `MapLight.is_dynamic`). `lighting-old-stack-retirement/` should ship first.
> **Concurrent with:** `lighting-lightmaps/`, `lighting-sh-amendments/`, `lighting-spot-shadows/`.
> **Related:** `context/lib/rendering_pipeline.md` §4 · `context/plans/in-progress/lighting-foundation/4-light-influence-volumes.md` (existing per-frustum culling, unchanged).

---

## Context

Lightmaps cover diffuse and shadow from static lights. Specular from static lights still needs a runtime path — lightmaps are view-independent and cannot store specular. This plan adds:

- A **spec-only light buffer**: per-static-light `(position, color, range)` uploaded once at level load, evaluated per-fragment for Blinn-Phong specular.
- A **per-chunk light list**: the world partitioned into a chunk grid; each chunk stores the indices of nearby visible static lights. Per-fragment, the shader looks up its chunk and iterates only those lights — typically a handful regardless of total authored count. Bounds the per-fragment specular cost at 500-light densities.
- **Per-light visibility masks** computed offline: BVH ray-casts at build time filter lights that cannot reach a chunk through geometry. Masks are baked into the chunk lists — zero runtime visibility test, bounded specular leak to within-chunk radius.

Blinn-Phong specular evaluated here is also used by the dynamic pool path in `lighting-spot-shadows/`. That plan depends on the specular utility function introduced here.

---

## Goal

- **Compiler:** build a spatially-partitioned light index with offline visibility filtering and write it as a `ChunkLightList` PRL section.
- **Runtime:** populate the spec-only buffer from static lights, upload the chunk list, and per-fragment iterate the chunk-local subset for Blinn-Phong specular.

---

## Concurrent workstreams

Both tasks can start simultaneously. The runtime task uses a flat full-buffer fallback (iterate all static lights with no chunk lookup) until the compiler task produces a populated `ChunkLightList` section. The fallback produces correct output; it's just unconstrained on per-fragment iteration count.

```
Task A (compiler): chunk grid + visibility masks → PRL section ─── independent
Task B (runtime): spec buffer + chunk lookup + Blinn-Phong ──────── independent (fallback active)
```

---

## Task A — Chunk grid builder + visibility masks

**Crate:** `postretro-level-compiler` · **New module** under `src/bake/`.

1. **Grid definition.** Derive world AABB from `MapData` extents. Subdivide into uniform cubic chunks (default: 8 m side, retunable per-level). Linearize with `z * dims.x * dims.y + y * dims.x + x`.
2. **Per-chunk bucketing.** For each chunk, test each static light's influence sphere against the chunk AABB (closest-point-on-box vs. sphere center, compare against radius). Directional lights (infinite range) are added to every chunk. Dynamic lights are excluded.
3. **Visibility mask filter.** For each `(light, chunk)` pair that passes the sphere-AABB test, cast a small number of shadow rays (default: 4) from the light position to representative sample points inside the chunk. If at least one ray is unoccluded through the Milestone 4 BVH, retain the light in the chunk's list. If all rays are blocked, drop it. The filter runs offline; the resulting list is already visibility-filtered.
4. **Per-chunk count cap.** Clamp each chunk's list to a maximum count (default: 64). Log any overflow at bake time with chunk coordinates and the count of dropped lights.
5. **Output.** Flat index buffer + offset table (`[offset: u32, count: u32]` per chunk).

**`ChunkLightList` PRL section.** New section ID in `postretro-level-format`. Section payload: grid metadata (origin, cell size, dims, has_grid sentinel) + offset table + flat index list. PRL format coordination note: `lighting-lightmaps/` also adds a new section ID; assign IDs at implementation time against the current max, ensuring no collision.

### Task A acceptance gates

- Compiling `assets/maps/test.map` produces a `.prl` with a populated `ChunkLightList` section.
- Average per-chunk light count logged at bake time; on a map with ≥50 static lights, average is at least 4× smaller than total static light count.
- Two-room test case (wall between a light and a far chunk): the far chunk's list does not contain the light (visibility mask filter confirmed).
- Per-chunk overflow logs at bake time when a chunk exceeds 64 lights.

---

## Task B — Spec-only buffer, chunk lookup, Blinn-Phong

**Crate:** `postretro` · **New module:** `src/lighting/spec_buffer.rs` · **Also modifies:** `src/lighting/chunk_list.rs` *(new)*, `src/render/mod.rs`, `src/shaders/forward.wgsl`.

1. **Spec-only light buffer.** At level load, populate a storage buffer with one entry per static light: `(position: vec3<f32>, color: vec3<f32>, range: f32)` — ~32 bytes per light. Upload once, read-only at runtime. Dynamic lights excluded.
2. **Chunk list upload.** Parse the `ChunkLightList` PRL section. Upload grid metadata as a uniform; offset table and flat indices as storage buffers. Missing-section fallback: `has_chunk_grid = 0`, shader iterates the full spec buffer.
3. **Bind group.** Add storage buffer entries to group 2. Coordinate with `lighting-lightmaps/` and `lighting-spot-shadows/` to avoid binding slot collisions.
4. **Per-chunk iteration in shader.** Per fragment, compute chunk cell from `world_position`, look up `(offset, count)`, iterate only those lights for specular. Fallback to full buffer if `has_chunk_grid == 0`.
5. **Blinn-Phong specular.** Normalized Blinn-Phong, implemented as a shared utility function in `forward.wgsl`:

```wgsl
fn blinn_phong(L: vec3<f32>, V: vec3<f32>, N: vec3<f32>,
               color: vec3<f32>, spec_exp: f32, spec_int: f32) -> vec3<f32> {
    let H   = normalize(L + V);
    let NdH = max(dot(N, H), 0.0);
    return color * pow(NdH, spec_exp) * spec_int;
}
```

Specular exponent and intensity sourced from a per-material specular map (texture convention; authoring tooling is out of scope). Applied in both the per-chunk static iteration and the dynamic-pool direct loop (used by `lighting-spot-shadows/`).

6. **Distance falloff + influence range.** Attenuate the specular contribution by the same falloff model as the stored light type. Reject lights outside their influence range entirely (the chunk list is a conservative spatial index; the influence range provides the tight per-light rejection).

### Task B acceptance gates

- Specular highlights appear on test geometry under static lights.
- Disabling the spec buffer (swap for a 1-element all-zero stub) makes specular highlights vanish, confirming the path.
- On a map with ≥50 static lights: `POSTRETRO_GPU_TIMING=1` forward-pass GPU time is lower with the chunk list active than with the full-buffer fallback. If not measurably lower, log the avg/max per-chunk count and confirm the list was populated correctly before investigating.
- On a sparse map (<10 lights): no regression versus the full-buffer fallback.

---

## Acceptance Criteria (both tasks)

1. `cargo test --workspace` passes.
2. `cargo clippy --workspace -- -D warnings` clean.
3. No new `unsafe`.
4. Task A and Task B acceptance gates above.
5. Combined `ChunkLightList` section memory (offset table + index list) stays under 16 MB for the test map. Log actual size at bake time; fail the bake with a diagnostic if exceeded.
6. Level load time does not regress materially with the chunk list parse and spec-buffer population added.

---

## Out of scope

- Diffuse from static lights at runtime — covered by `lighting-lightmaps/`.
- Dynamic light specular — the `blinn_phong` utility function introduced here is used by `lighting-spot-shadows/`; no further specular work in this plan.
- PBR shading.
- Clustered forward+ (screen-space tile/cluster binning) — the per-chunk world-space grid is the chosen approach.
- Per-level tuning of chunk cell size — 8 m default; retune only if evidence demands.
