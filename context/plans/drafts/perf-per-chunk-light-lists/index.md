# Per-Chunk Light Lists — Fragment Loop Optimization

> **Status:** ready
> **Depends on:** `context/plans/in-progress/lighting-foundation/4-light-influence-volumes.md` (runtime `LightInfluence` + `visible_lights` must exist).
> **Related siblings:** `context/plans/drafts/perf-cone-capped-influence-bounds/` · `context/plans/drafts/perf-light-buffer-packing/` · parent `context/plans/in-progress/lighting-foundation/index.md`.

---

## Context

Sub-plan 3 established the flat per-fragment light loop; sub-plan 4 added the per-fragment sphere-bound early-out at `postretro/src/shaders/forward.wgsl:1010–1019`:

```wgsl
for (var i: u32 = 0u; i < light_count; i = i + 1u) {
    let influence = light_influence[i];
    let inf_radius = influence.w;
    if inf_radius <= 1.0e30 {
        let d = in.world_position - influence.xyz;
        if dot(d, d) > inf_radius * inf_radius {
            continue;
        }
    }
    // ... per-type switch, shadow, accumulate
}
```

The shader iterates **every authored light** for every fragment. `forward.wgsl:1008` reads `uniforms.light_count`, which is populated at `render/mod.rs:609` from `geometry.lights.len()` — the total authored set, not a frustum-visible subset. The `visible_lights_indices` buffer built at `render/mod.rs:1402` only feeds shadow-slot allocation; it does not gate the main light loop. Each rejected iteration still costs one `vec4` load plus a 3-component subtract, dot, and compare. At the target density of 500 authored lights per level (cyberpunk interiors — many small coloured point lights packed into tight rooms; parent `index.md` line 16), a typical fragment pays 500 × (load + reject) per pixel.

A per-chunk light list collapses this. The world is partitioned into a spatial lattice of chunks; each chunk stores an index list of the lights whose influence sphere overlaps it. A fragment looks up its chunk once from `world_position`, reads `(offset, count)`, and iterates only the lights in that range. The existing sphere-dot test remains the inner-loop guard (chunks are conservative), but the loop bound shrinks from `light_count` to the per-chunk list length — typically a handful.

This plan also folds in tightening `uniforms.light_count` to `visible_lights.len()` on the CPU side as a cheap adjacent win: the flat fallback path (see Task B) benefits, and it costs one extra upload per frame.

This is a CPU-side spatial index, not tile/cluster binning in screen space. Clustered forward+ is a possible follow-up if this structure still leaves the flat loop as a bottleneck, but it requires a per-frame compute pass keyed on view; a world-space chunk grid is view-independent and can be rebuilt once per level if the light set is static.

**Methodology note.** The 500-lights figure and flat-loop concern come directly from the parent lighting-foundation plan. No profiler data yet — this plan stands on the light-count target and the known shader cost per rejected light.

---

## Goal

Replace per-fragment iteration over all frustum-visible lights with iteration over a per-chunk subset, preserving bit-identical pixel output. Measured cost of the light loop in `POSTRETRO_GPU_TIMING=1` drops on any map with more than a few-dozen lights; no change on sparse maps.

---

## Approach

Three phases. The CPU-side index build and upload (Task A) is independent of shader work; the shader change (Task B) is a drop-in once the buffer exists. Task C is the instrumentation to demonstrate the drop.

Decisions (fixed — not open):

- **Cell size:** 8 m cubes. Matches rough room scale; retune later if profiling demands.
- **Grid alignment:** independent uniform lattice. Do not couple to SDF bricks — separate concerns, separate life cycles.
- **Rebuild cadence:** per-level only. All authored lights are static; gameplay-dynamic lights (Milestone 6+) revisit then.
- **Bind group:** group 2, bindings 10 and 11 (chunk offsets table + flat indices). Group 3 is already fully packed (bindings 0–13 for SH + anim buffers, `forward.wgsl:132–148`) and `max_bind_groups = 4` on some backends, so opening a new group 4 is risky.
- **Cell overflow:** clamp to `MAX_LIGHTS_PER_CELL = 64`, log overflow at load time. Flat-array bookkeeping beats linked-list chasing.

---

### Task A — CPU-side index build + upload

**Crate:** `postretro` · **Files:** new `src/lighting/chunk_index.rs`, `src/lighting/mod.rs`, `src/main.rs` (frame hook or level-load hook depending on cadence decision).

**Fix 1: Build the two buffers.**

1. `chunk_table: Vec<[u32; 2]>` — one entry per chunk cell, `[offset, count]` into the index list. Linearized from 3D grid dims by `z * dims.x * dims.y + y * dims.x + x`.
2. `chunk_light_indices: Vec<u32>` — flat concatenation of per-chunk light-index lists. **Values are global indices into `lights[]`**, not into `visible_lights`. Because rebuild is per-level and `visible_lights` is a per-frame frustum subset, chunk lists must not intersect with per-frame visibility.

**Fix 2: Bucketing loop.** Per chunk, compute the chunk's world-space AABB and test each `LightInfluence` sphere against the AABB (closest-point-on-box vs. center, compare against radius). Directional lights (`radius > INFINITY_THRESHOLD`, see `postretro/src/lighting/influence.rs:23`) are added to every chunk. Clamp per-cell count to `MAX_LIGHTS_PER_CELL = 64`; log overflow at load time. Budget: 500 lights × C chunks. For a 256 m³ level at 8 m cells, ~16K sphere-AABB tests. Sub-millisecond at level load.

**Fix 3: Grid bounds.** Take the world AABB from `MapData` extents on the engine side. Do not reach into `postretro-level-format/src/sdf_atlas.rs`; the SDF baker's AABB is compile-time, this index is runtime.

**Fix 4: Upload.** Both buffers as `STORAGE | COPY_DST`; rebuild only on level load under the static-lights assumption.

**Fix 5: Grid uniform struct.** Add a small uniform alongside the storage buffers:

```wgsl
struct ChunkGridInfo {
    origin:    vec3<f32>,
    cell_size: f32,
    dims:      vec3<u32>,
    has_chunk_grid: u32,  // sentinel: 0 = fall back to flat loop
};
```

**Fix 6: Tighten `light_count`.** On the CPU side, upload `uniforms.light_count = visible_lights.len()` (not `geometry.lights.len()`) so the shader's flat-loop fallback also benefits when the chunk grid is disabled.

**Fix 7: Unit tests.** Add `cargo test` coverage for `chunk_index.rs`:

- sphere-vs-AABB bucketing (basic inside/outside/overlap cases)
- directional-light always-present path (infinite radius adds to every cell)
- empty-grid path (zero lights → zero-length indices, `has_chunk_grid = 0` emitted)

---

### Task B — Shader change

**Crate:** `postretro` · **File:** `src/shaders/forward.wgsl`

**Fix 1: Replace the loop header at `forward.wgsl:1009` with a chunk lookup.**

```wgsl
if chunk_grid.has_chunk_grid == 0u {
    // Fallback: flat loop over lights[0..light_count] — identical to pre-change behavior.
    for (var i: u32 = 0u; i < light_count; i = i + 1u) {
        // existing influence-sphere early-out + per-type switch + shadow block
    }
} else {
    let cell = compute_chunk_cell(in.world_position);  // vec3<u32>
    let cell_idx = cell.z * chunk_grid.dims.x * chunk_grid.dims.y
                 + cell.y * chunk_grid.dims.x
                 + cell.x;
    let range = chunk_table[cell_idx];  // (offset, count)
    let offset = range.x;
    let count  = range.y;  // already clamped to MAX_LIGHTS_PER_CELL at build time

    for (var k: u32 = 0u; k < count; k = k + 1u) {
        let i = chunk_light_indices[offset + k];
        // existing influence-sphere early-out at forward.wgsl:1010–1019 stays here
        // existing per-type switch and shadow block unchanged
    }
}
```

**Fix 2: Cell compute.** `compute_chunk_cell` is `floor((world_position - chunk_grid.origin) / chunk_grid.cell_size)` clamped to `chunk_grid.dims - 1`; out-of-grid fragments fall into the edge cell.

**Fix 3: Keep the inner sphere test.** The test at `forward.wgsl:1014–1018` **stays**. The chunk is a conservative bound (AABB-vs-sphere, not point-in-sphere); fragments near chunk corners still need the tight sphere test to reject lights that touch the chunk AABB without touching the fragment.

**Fix 4: Binding layout.** Group 2, bindings 10 (chunk_table storage buffer) and 11 (chunk_light_indices storage buffer). The grid metadata uniform slots alongside — pick the next free group-2 binding or group it with the existing forward uniform block.

---

### Task C — Instrumentation and acceptance evidence

**Crate:** `postretro` · **File:** `src/render/mod.rs`

`POSTRETRO_GPU_TIMING=1` already logs per-pass GPU time (per root `CLAUDE.md`). Add a DEBUG-level log at renderer init that reports `chunk_light_indices.len() / chunk_count` (average lights-per-chunk) and the max count across all chunks — the expected per-fragment loop bound. On a dense-light test map, the average should be an order of magnitude below `light_count`; the forward-pass GPU time in the timing log should drop correspondingly.

Golden-image test (or visual diff against pre-change screenshots) proves pixel identity.

---

## Files to modify

| File | Task | Change |
|------|------|--------|
| `postretro/src/lighting/chunk_index.rs` *(new)* | A | Build `chunk_table` + `chunk_light_indices` from `&[LightInfluence]` and `MapData` extents; sphere-vs-AABB test per chunk; `MAX_LIGHTS_PER_CELL = 64` clamp; unit tests (bucketing, directional, empty) |
| `postretro/src/lighting/mod.rs` | A | Re-export `chunk_index`; wire level-load rebuild |
| `postretro/src/render/mod.rs` | A, C | Allocate and bind the two storage buffers + `ChunkGridInfo` uniform on group 2 bindings 10/11; tighten `uniforms.light_count` to `visible_lights.len()`; debug log of avg/max per-chunk counts |
| `postretro/src/shaders/forward.wgsl` | B | Add `ChunkGridInfo` uniform + chunk_table/chunk_light_indices bindings; replace loop header at line 1009 with chunk-indexed iteration guarded by `has_chunk_grid`; keep sphere test at 1010–1019 as inner guard |
| `postretro/src/main.rs` | A | Trigger rebuild at level load |

No changes to `postretro-level-format` or `postretro-level-compiler`. Chunk IDs are runtime-derived.

---

## Acceptance Criteria

1. `cargo test -p postretro` passes.
2. No new `unsafe`.
3. Pixel output identical to pre-change on all test maps (visual diff or golden compare).
4. On a test map with ≥50 lights, `POSTRETRO_GPU_TIMING=1` shows forward-pass GPU time drop by ≥30%; on a sparse (<10 lights) map, no regression.
5. DEBUG-level log at level load reports `avg_lights_per_chunk` and `max_lights_per_chunk`. `avg` is at least 4× smaller than `light_count` on the dense test map.
6. Combined `chunk_table` + `chunk_light_indices` memory footprint stays under 16 MB for the worst-case test map; level load fails with a diagnostic log if exceeded.
7. `cargo clippy -p postretro -- -D warnings` clean.

---

## Out of scope

- **Clustered forward+ (screen-space tile/cluster binning).** This is the natural next step if the per-chunk world-space grid still leaves headroom on the table. Requires a per-frame compute pass; different trade-offs.
- **Dynamic / gameplay lights** (muzzle flashes, projectile glows — Milestone 6+). Chunk rebuild cadence changes if they land; revisit this plan when they do.
- **Per-chunk shadow-slot assignment.** Sub-plan 5 handles shadow-slot allocation by consuming the CPU-side `visible_lights` frustum subset; the `visible_lights` construction itself lives at `postretro/src/lighting/influence.rs:46` (introduced in sub-plan 4). Out of scope here.
- **Tightening spot-light bounds.** Covered by sibling `perf-cone-capped-influence-bounds`.
- **Shrinking `GpuLight` byte footprint.** Covered by sibling `perf-light-buffer-packing`.
- **PRL format changes.** Chunk IDs are world-position-derived at runtime; no on-disk format work required.

---

## Notes

- **Memory cap.** Combined `chunk_table` + `chunk_light_indices` budget is **16 MB**. Fail level load if exceeded (indicates pathological lighting authoring or cell-size misconfiguration); log the actual size so the author can diagnose. At 500 lights × 4 B × 4096 chunks worst-case = 8 MB, so 16 MB provides comfortable headroom.
