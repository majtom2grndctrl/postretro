# Animated Light Chunks

## Goal

Produce a baked spatial partition of surface area where every chunk carries a bounded list of animated lights that influence it. Enables the future per-light weight map pipeline to sample short per-chunk light lists instead of iterating every animated light per texel, so memory and shader cost scale with local overlap density rather than total animated-light count.

Prerequisite for the per-light weight map animated-lightmap pipeline (future spec). This spec ships the partition and per-chunk light lists; nothing else consumes them yet.

## Scope

### In scope

- New PRL section `AnimatedLightChunks` (SectionId = 24): header + fixed-stride per-chunk records + flat pool of animated-light indices. Each chunk carries a world-space AABB, parent face index, face-local UV sub-region, and an `(offset, count)` into the flat index pool. Adopts the offset-table + flat-pool *pattern* from `ChunkLightListSection` (ID 23); structurally distinct (leaf-range-indexed per-face records, not a world-space uniform grid).
- "Animated light" = a `MapLight` (`postretro-level-compiler/src/map_data.rs`) with `!bake_only && !is_dynamic && animation.is_some()`. Matches the animated partition in `sh_bake.rs`. The `!bake_only` clause aligns the index space with `AlphaLightsSection` / `LightInfluenceSection`, which are built from the same filter in `pack.rs`.
- Light indices stored in the flat pool index into the **filtered** light list — same namespace as `AlphaLightsSection` and `LightInfluenceSection`. Not the raw `map_data.lights` array.
- Directional animated lights (`InfluenceRecord.radius == f32::MAX`) are skipped by this builder. A directional sphere intersects every chunk AABB, so it would bottom out at min-extent everywhere without useful spatial structure. The weight-map spec handles directional animated lights via a separate whole-face path; spec that out there, not here.
- Compile-time builder: for every face that any (non-directional) animated light's influence sphere overlaps, recursively split the face's UV rectangle until every chunk's overlapping animated-light count is ≤ `MAX_ANIMATED_LIGHTS_PER_CHUNK` or a minimum chunk extent is reached. Faces with zero animated-light overlap produce no chunks.
- Subdivision uses animated-light **influence spheres** (center + radius from `LightInfluenceSection`, filtered to the animated subset and excluding directional) intersected against chunk AABBs.
- Chunk UV is **face-local in world-meter units** along the face's (u,v) basis — the same units as `Chart.uv_min` / `Chart.uv_extent` in `lightmap_bake.rs`. Not 0..1 normalized. The downstream weight-map baker composes face-local UV with each face's atlas placement to address lightmap texels; storing face-local UV keeps the chunk record independent of atlas packing changes.
- Extend `BvhLeaf` with `chunk_range_start: u32` + `chunk_range_count: u32` pointing into the chunk array. Baker emits chunks in flat-leaf-array order (material-bucket-sorted, stable tiebreakers) so each leaf owns a contiguous range. A runtime BVH walk over visible leaves enumerates visible chunks as the union of their ranges — no second spatial structure needed at runtime. Leaves with no animated-light overlap carry `chunk_range_count = 0`.
- Compile-time log line: chunk count, max/mean animated lights per chunk, count of chunks that hit the min-extent floor without satisfying the cap.
- Unit tests in the compiler covering: face with 0 animated lights (no chunk emitted), face with N ≤ cap lights (one chunk), face with > cap lights triggering subdivision, min-extent floor, directional animated light is skipped (no chunks produced from it alone), `bake_only` and `is_dynamic` lights are skipped (not treated as animated even if `animation.is_some()`), index-namespace parity (emitted indices match `LightInfluenceSection` positions), determinism (two builds → byte-identical section).

### Out of scope

- Weight map baking. Baking per-light contribution weight maps into texels consumes the chunk section; scoped to the follow-up spec.
- Runtime rendering changes. No shader, renderer, or runtime code touches the new section in this spec.
- Removing animated lights from the static lightmap composite. Stays in the follow-up spec alongside the weight-map path that replaces them.
- Geometry tessellation. Chunks subdivide AABBs/UV regions virtually; the underlying vertex/index buffers are untouched.
- Restructuring `BvhSection` beyond the two-field `BvhLeaf` extension. Leaf semantics, draw iteration, material-bucket sort, and traversal shader are unchanged.
- Non-linear interpolation of `LightAnimation.brightness` curves. Cubic / spline easing is a prerequisite for the future weight-map spec (it drives per-texel temporal response); it lives in plan 2's `LightAnimation` evaluator. Chunks themselves are interpolation-agnostic — they carry spatial light lists, not curves.
- Luxel-space partitioning. Chunks live in texel/UV space on faces; variable-density world-space luxels are a future direction, not this spec.
- Static-light overlap. Static lights are already handled by `ChunkLightListSection`; animated lights get their own section.

## Acceptance criteria

- [ ] `AnimatedLightChunks` section round-trips via `to_bytes` / `from_bytes` and is emitted by `prl-build` for every map with at least one animated light.
- [ ] For a map where all animated-light spheres are disjoint, every emitted chunk has exactly 1 animated-light index.
- [ ] For a synthetic map with K overlapping animated lights covering one face (K > cap), the builder produces multiple chunks for that face and no chunk has more than `MAX_ANIMATED_LIGHTS_PER_CHUNK` light indices — unless the chunk has bottomed out at the `MIN_CHUNK_UV_EXTENT` or `MIN_CHUNK_WORLD_EXTENT` floor (whichever triggers first), in which case the chunk is emitted with up to K indices and the compile log records a warning.
- [ ] Every emitted chunk's face-local UV sub-region lies within the parent face's chart extent (`Chart.uv_min` + `[0, uv_extent]`), and the chunk's world-space AABB contains the face geometry falling in that UV region.
- [ ] Faces with no animated-light-sphere overlap produce no chunks; the leaves owning them carry `chunk_range_count = 0`.
- [ ] For every BVH leaf, the slice `chunks[chunk_range_start..chunk_range_start + chunk_range_count]` contains exactly the chunks whose parent face sits in that leaf, in builder-defined stable order.
- [ ] Summing `chunk_range_count` across all BVH leaves equals the total chunk count. No chunk is shared between leaves; no chunk is unreachable from a leaf.
- [ ] Two compiler runs on the same input produce byte-identical `AnimatedLightChunks` sections and byte-identical `chunk_range_*` fields on `BvhLeaf`.
- [ ] `cargo check -p postretro-level-compiler -p postretro-level-format -p postretro` clean.
- [ ] Unit tests listed in *Scope* pass.

## Tasks

### Task 1: Section definition and `BvhLeaf` extension

Add `AnimatedLightChunks = 24` to `SectionId` in `postretro-level-format/src/lib.rs`, and add the corresponding `24 => Some(Self::AnimatedLightChunks)` arm to `SectionId::from_u32` (line 115) — without it, unknown sections are silently skipped at read time. Define `AnimatedLightChunksSection` in a new format-crate module: fixed header + fixed-stride chunk records (world AABB min/max, face index, face-local UV min/max, `(index_offset, index_count)` into the flat pool, fixed stride) + flat `light_indices: Vec<u32>` pool. Implement `to_bytes` / `from_bytes` symmetrically with neighbouring sections. The offset-table + flat-pool pattern is borrowed from `chunk_light_list.rs`; the rest of the layout is unrelated (no grid metadata).

Extend `BvhLeaf` in `postretro-level-format/src/bvh.rs` with `chunk_range_start: u32` + `chunk_range_count: u32`. `BvhLeaf` grows from 40 to 48 bytes. Five call sites change together:

- `postretro-level-format/src/bvh.rs` — struct, `LEAF_STRIDE`, `to_bytes` / `from_bytes`. Update the stale comment at `bvh.rs:19-23` which currently points only at `compute_cull.rs`.
- `postretro/src/shaders/bvh_cull.wgsl` — the WGSL `struct BvhLeaf` (the actual GPU-side layout) must be extended with the two scalar u32 fields, keeping the scalar-fields-not-vec3 discipline documented in the WGSL file.
- `postretro/src/geometry.rs` — the engine-side `BvhLeaf` struct (lines 52-61) mirrors the WGSL struct byte-for-byte and must gain `chunk_range_start: u32` + `chunk_range_count: u32`.
- `postretro/src/prl.rs` — the format→engine `BvhLeaf` converter (line 313) maps each field explicitly; add the two new fields to the mapping. The `FormatBvhLeaf` test fixture at line 1066 will also need the new fields (normal struct-update fallout).
- `postretro/src/compute_cull.rs` — `serialize_bvh_leaves` (currently writes 40 bytes/leaf; update the `* 40` capacity hint to `* 48` and add two new field writes). Two tests update: `bvh_leaf_serialization_is_40_bytes` flips to 48 (rename the test too); in `wgsl_struct_strides_are_40_bytes`, only the `BvhLeaf` stride assertion changes to 48 — `BvhNode` stays at 40.

Leaves on maps without animated lights carry `chunk_range_count = 0`.

`MAX_ANIMATED_LIGHTS_PER_CHUNK = 4`. Shared format-crate constant, warn-log (rate-limited, mirrors plan 3's `MAX_SPRITES` policy) when the min-extent floor forces an overflow. Rationale in *Settled decisions*.

### Task 2: Builder

Add a new module to `postretro-level-compiler` (sibling to `bvh_build`, `lightmap_bake`, `chunk_light_list_bake`) that consumes:

- the flattened `BvhSection`
- the animated subset of the filtered light list, computed via `!bake_only && !is_dynamic && animation.is_some() && influence.radius != f32::MAX` on `MapLight` / `InfluenceRecord` pairs
- per-face UV bounds in world-meter units along each face's (u,v) basis — the same per-face `Chart` data `lightmap_bake.rs` already computes

`Chart` is currently a private struct in `lightmap_bake.rs`. Either expose it (preferred — no new data) or introduce a small public `FaceUvBounds { origin: Vec3, u_axis: Vec3, v_axis: Vec3, uv_min: [f32; 2], uv_extent: [f32; 2] }` returned from `bake_lightmap` alongside the existing output. Note: the function is `bake_lightmap` (singular, line 108); its current return type is `Result<LightmapSection, LightmapBakeError>` — exposing chart data is a signature change (e.g. `Result<(LightmapSection, Vec<Chart>), LightmapBakeError>`). The builder needs origin + axes to project UV back to world space.

Iterate leaves in flat-leaf-array order; for each leaf, iterate the face(s) it owns; for each face recursively subdivide in face-local UV along the longest UV axis, projecting the split back to world space via the face's (origin, u_axis, v_axis) basis to intersection-test against influence spheres. Termination: light count ≤ cap, or UV extent below `MIN_CHUNK_UV_EXTENT`, or world extent below `MIN_CHUNK_WORLD_EXTENT`.

Split axis is UV-space because the downstream weight map is UV-indexed; world-space splits produce non-axis-aligned UV regions the weight-map baker cannot address cleanly.

Driving the outer loop from leaves (rather than from the face array) lets the builder write each leaf's chunks contiguously and stamp `chunk_range_*` on the leaf in one pass. Today `BvhPrimitive` construction (`bvh_build.rs` — see the comment block at line 81 and the construction at ~line 102) collapses `(face, material_bucket)` to one primitive per face, so each leaf owns exactly one face. The inner face loop is defensively written for multi-face leaves but runs once today.

Determinism: visit leaves in the already-deterministic flat-leaf-array order (material-bucket-sorted with stable tiebreakers from `bvh_build`), feed lights in animation-descriptor order, break split-decision ties with stable comparisons. No floating-point hashing; no parallel iteration.

### Task 3: Wire into `prl-build`

Call the builder after `build_bvh` and the lightmap bake have materialized, respectively, the flat leaf array and per-face UV bounds. Pass the mutable `BvhSection` so the builder can stamp `chunk_range_start` / `chunk_range_count` on leaves. Emit the `AnimatedLightChunks` section. Log chunk statistics. Skip the section entirely when the map has no animated lights — no placeholder record needed; the absent SectionId is the signal. No other compiler stage depends on the output.

### Task 4: Tests

Compiler unit tests covering the cases enumerated in *Scope*. A synthetic `InfluenceRecord` fixture is sufficient — no full map compile needed for the algorithm tests. Add one test that walks every `BvhLeaf` and asserts the `chunk_range_*` invariants listed in the acceptance criteria (contiguous, no-overlap, total-sum).

## Sequencing

**Phase 1 (sequential):** Task 1 — section type blocks builder.
**Phase 2 (sequential):** Task 2 — builder needs the section type.
**Phase 3 (concurrent):** Task 3, Task 4 — both consume the builder; wiring and unit tests are independent.

## Rough sketch

Chunk record layout (illustrative; final field order may shuffle to pack cleanly):

```rust
#[repr(C)]
pub struct AnimatedLightChunk {
    aabb_min: [f32; 3],
    face_index: u32,
    aabb_max: [f32; 3],
    index_offset: u32,   // into AnimatedLightChunksSection.light_indices
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    index_count: u32,
    _pad: u32,
}
```

This section is CPU-only (no WGSL mirror; runtime code doesn't consume it in this spec). Padding is for natural struct alignment; final field order may change to eliminate it.

`cell_id` is intentionally absent — the owning `BvhLeaf` already carries it, and leaves never span cells. Consumers reach cell identity via the leaf. `face_index` is kept so the weight-map baker can look up the face's UV→world basis without a leaf indirection.

Builder skeleton:

```
animated_lights = [
    (filtered_index, InfluenceRecord)
    for (filtered_index, map_light) in filtered_lights.enumerate()
    if !map_light.is_dynamic
       && map_light.animation.is_some()
       && influence[filtered_index].radius != f32::MAX
]

for face in faces with animated-light overlap:
    lights_overlapping = animated_lights whose sphere intersects face_world_aabb
    recurse(face, uv_bounds=face.uv_bounds, lights_overlapping)

recurse(face, uv_bounds, candidate_lights):
    chunk_world_aabb = project uv_bounds to world via face basis
    hits = candidate_lights.filter(sphere.intersects(chunk_world_aabb))
    if hits is empty:
        return                              // prune: no chunk emitted
    if hits.len() <= CAP or below min extent:
        emit chunk with hits
        return
    axis = longest dimension of uv_bounds
    mid = midpoint along axis
    recurse(face, left_uv_half, hits)
    recurse(face, right_uv_half, hits)
```

The min-extent floor guarantees termination. `candidate_lights` is passed in so every recursion filters a shrinking superset, not the full animated-light list. An empty-hits branch prunes subregions no animated light reaches — no zero-light chunks are ever emitted.

`BvhLeaf` extension (two fields, eight bytes):

```rust
// postretro-level-format/src/bvh.rs — grown from 40 to 48 bytes.
#[repr(C)]
pub struct BvhLeaf {
    // ... existing fields ...
    pub chunk_range_start: u32,
    pub chunk_range_count: u32,
}
```

Key files touched:
- `postretro-level-format/src/lib.rs` — SectionId
- `postretro-level-format/src/bvh.rs` — `BvhLeaf` extension, `LEAF_STRIDE`, (de)serialize, fix cross-reference comment at lines 19-23
- `postretro-level-format/src/animated_light_chunks.rs` — new
- `postretro/src/shaders/bvh_cull.wgsl` — extend WGSL `struct BvhLeaf` (GPU-side layout)
- `postretro/src/geometry.rs` — extend engine-side `BvhLeaf` struct (mirrors WGSL byte-for-byte)
- `postretro/src/prl.rs` — update format→engine `BvhLeaf` converter and test fixture
- `postretro/src/compute_cull.rs` — update `serialize_bvh_leaves` (40→48 bytes/leaf), capacity hint, and both impacted tests
- `postretro-level-compiler/src/lightmap_bake.rs` — expose per-face UV bounds (either make `Chart` public or add a `FaceUvBounds` output)
- `postretro-level-compiler/src/bvh_build.rs` — initialize `chunk_range_*` to zero at flatten time (the chunk builder stamps them later)
- `postretro-level-compiler/src/animated_light_chunks.rs` — new
- `postretro-level-compiler/src/main.rs` (or wherever sections are assembled) — emit call

## Open questions

1. **Skewed UV layouts.** Faces whose UV→world mapping is highly non-uniform (long thin UV strips, or multi-patch UVs from a future lightmap packer) may subdivide inefficiently in UV space. For the current lightmap packer (one UV island per face, axis-aligned on the face plane) UV-space splitting is fine. Flag as a future concern if the packer changes.

## Settled decisions

- **Offset+pool layout, not inline arrays.** Chunk records carry `(index_offset, index_count)` into a flat `light_indices: Vec<u32>` pool — the same *pattern* as `ChunkLightListSection` (ID 23), though the surrounding structure differs (this section is leaf-range-indexed per-face records; `ChunkLightList` is a world-space uniform grid). Inline `[u32; CAP]` arrays would waste bytes on the common case of chunks with 1–2 lights.
- **Light-index namespace = filtered (AlphaLights / LightInfluence) table.** The u32 values in the flat pool are indices into the same filtered list `AlphaLightsSection` and `LightInfluenceSection` are built from (`!bake_only` applied in `pack.rs`). The builder filters further to the animated subset in-place; the stored index is into the post-`!bake_only` list, not into a new animated-only table. The weight-map spec reads `LightInfluenceSection[index]` directly.
- **Directional animated lights skipped.** An `InfluenceRecord` with `radius == f32::MAX` (the directional sentinel) intersects every chunk AABB by construction — subdivision would bottom out at min-extent for every chunk it overlaps without producing any useful spatial narrowing. Skip these in this builder; the weight-map spec handles directional animated lights via a separate whole-face path (sun-sweeps, etc.).
- **Face-local UV in world-meter units, not lightmap-atlas UV and not 0..1 normalized.** The builder recurses in face-local UV because the face's UV→world basis is defined in that space. The chosen units match `Chart.uv_min` / `Chart.uv_extent` in `lightmap_bake.rs` (meters along the face's (u,v) basis) — no rescaling between the lightmap bake and this builder, no division by per-face extent at chunk-consume time. Storing face-local UV keeps chunks invariant under atlas re-packing. The weight-map baker composes face-local UV with each face's atlas placement at consumption time.
- **No zero-light chunks.** Faces with no animated-light-sphere overlap produce no chunks. Under the BVH-leaf-range model, the zero-overlap case costs one u32 per leaf (`chunk_range_count = 0`) — cheaper than synthesizing a covering chunk and carrying an empty `light_indices` slice. The weight-map baker falls back to the static lightmap for texels absent from the section.
- **Cap and floor values.** `MAX_ANIMATED_LIGHTS_PER_CHUNK = 4` (matches expected shader-side light-loop unroll and typical overlap density). `MIN_CHUNK_UV_EXTENT = 1.0 / lightmap_texel_density` meters (one lightmap texel — finer subdivision cannot be addressed by the UV-indexed weight-map baker; `texel_density` here is the lightmap bake's texels-per-meter, so the reciprocal is meters-per-texel, consistent with `Chart`'s world-meter UV units). `MIN_CHUNK_WORLD_EXTENT = 0.01` meters (guards against degenerate UV→world projections on tiny or skewed faces). Numbers revisited against the follow-up weight-map spec's shader cost budget; if the cap changes the constant is the single source of truth in the format crate.
- **Sibling section + BVH-leaf range index** (not full BVH structural change). `BvhSection` is not restructured; `BvhLeaf` grows two `u32`s pointing into the sibling `AnimatedLightChunks` array. Preserves draw iteration, material-bucket sort, and traversal shader semantics while giving the runtime a zero-cost path from visible leaves to visible animated-light chunks. Considered and rejected: (a) modifying `BvhLeaf` to hold light indices directly (forces face tessellation, large blast radius), (b) fully parallel sibling section with no BVH index (forces a second spatial query at runtime).
- **Non-linear brightness interpolation is a plan-2 concern, not this one.** Cubic / spline easing for `LightAnimation.brightness` is a prerequisite for the future weight-map spec (it drives per-texel temporal response). Chunks are interpolation-agnostic — they store spatial light lists, not curves. Land the interpolation upgrade in plan 2 before the weight-map spec ships; it does not gate this spec.
