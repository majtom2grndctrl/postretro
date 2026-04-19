# Per-Region BVH (Partitioned by PVS Leaf)

> **Status:** draft
> **Depends on:** Milestone 4 BVH (`context/plans/done/bvh-foundation/`) — specifically the flat DFS + skip-index layout (`2-runtime-bvh.md`), `BvhNode` (`postretro/src/geometry.rs:28-40`) / `BvhLeaf` (`postretro/src/geometry.rs:46-56`) / `BvhTree` (`postretro/src/geometry.rs:61-66`), the `Bvh` PRL section (`postretro-level-format/src/bvh.rs:70-239` `BvhSection`, loaded via `postretro-level-format/src/lib.rs:89`), `ComputeCullPipeline` in `postretro/src/compute_cull.rs`, and the compiler-side BVH build in `postretro-level-compiler/src/bvh_build.rs`. Cross-consumer: SH baker (`postretro-level-compiler/src/sh_bake.rs:12`, `bvh::bvh::Bvh`) and SDF baker (`postretro-level-compiler/src/sdf_bake.rs:8–9`).
> **Related:** `postretro/src/shaders/bvh_cull.wgsl` · `postretro/src/visibility.rs` (portal DFS / `VisibleCells`) · `postretro-level-format/src/bvh.rs:70-239` (`BvhSection`) · `context/lib/build_pipeline.md` §Pipeline

---

## Pre-work — gating measurement

**This plan stays in drafts until a measured cull-shader regression justifies it.** Honors the pivot-trigger language in `bvh-foundation/index.md:39`.

Required before promotion out of drafts:

1. Run `POSTRETRO_GPU_TIMING=1 cargo run --release -p postretro -- assets/maps/occlusion-test.map` and capture per-pass GPU time for the cull compute shader.
2. Same measurement on `assets/maps/test.prl` (or the current representative small-map baseline).
3. Promote only if the cull shader exceeds **0.5 ms per frame** on either map. Below that, global BVH is paying its way; per-region is premature optimization.

Record the measurements (numbers + adapter + build) in this file before moving out of `drafts/`.

---

## Sequencing with sibling plans

**`perf-per-cascade-bvh-cull-csm` lands first on the flat tree; per-region extends its dispatch model after.** Per-region inherits per-cascade's dispatch path — the per-cascade plan establishes the multi-dispatch pattern over the global BVH, and per-region swaps the "one BVH traversed per cascade" model for "N sub-BVHs traversed per cascade". If both plans ship together, per-cascade adapts to per-region's dispatch instead; but the expected order is per-cascade first, per-region second.

---

## Context

### Current state — single global BVH

Milestone 4 commits to a global BVH (`context/plans/done/bvh-foundation/index.md:39`):

> "Global BVH, not per-region. Single flat hierarchy over all static geometry. Per-region is the pivot path if the check-in (sub-plan 3) shows global doesn't hit frame-time parity on cell-heavy maps — designed for as a fallback, not as day-one scope."

The runtime cull shader (`postretro/src/shaders/bvh_cull.wgsl:95–150`) walks the full tree with flat DFS + skip-index in a single workgroup (`@workgroup_size(1, 1, 1)`). Every frame, traversal cost is proportional to the total node count — all frustum-culled subtrees skip via `skip_index`, but every surviving subtree is walked to its leaves. On a map with `N` total leaves and `k` visible leaves, traversal work is roughly `O(k log N + (N - k))` depending on the reject pattern.

The portal DFS already produces a visible-cell set, converted to a 128-word bitmask (`postretro/src/compute_cull.rs:49–51, 286–307`). The shader tests each leaf's `cell_id` against the bitmask (`bvh_cull.wgsl:84–92, 133–135`). The cell/leaf partition exists; the BVH is not aware of it.

### Why partition pays

The portal/PVS system already divides the world into coherent spatial regions (BSP leaves; `cell_id` in `BvhLeaf` is the `FaceMetaV3.leaf_index` on the primitive — *not* the BVH leaf array index, these are different arrays — per `bvh-foundation/index.md:114`). On a typical map, only 10–30% of cells are visible per frame. A global BVH still walks subtrees that contain *only* non-visible cells because the spatial hierarchy doesn't correlate with portal connectivity.

Per-region BVH: partition primitives by source cell (or a coarser cluster of cells), build one sub-BVH per region. Each region's BVH is compact and independent. Runtime traversal:

1. Portal DFS produces visible-cell bitmask.
2. For each visible region, dispatch BVH traversal over that region's sub-BVH.
3. Skip entirely: regions not in the visible set. No traversal cost, not even a root AABB test.

Gain: traversal work scales with `sum(log(|region_i|))` over visible regions, not `log(N)` or `O(N)` over the whole level. On maps with many small regions and tight portal visibility, the reduction is large.

### Who benefits

- **Runtime cull shader (`compute_cull.rs`).** Primary beneficiary. Portal visibility hands it a region index list; it walks only those sub-BVHs.
- **SH baker (`postretro-level-compiler/src/sh_bake.rs`).** Ray-casts from probe positions through the BVH. The baker continues to consume a **single flat BVH** unchanged (see the Baker tolerance decision below); per-region sub-BVHs are additive metadata the baker ignores.
- **SDF baker (`sdf_bake.rs`).** Closest-point queries through the BVH. Same: consumes the flat tree unchanged.

---

## Goal

Partition the global BVH into per-region sub-BVHs, indexed by region ID. Runtime cull dispatches only against sub-BVHs of visible regions. Output-visible primitive set is bit-identical to the global BVH (same leaves, same AABB tests). Runtime cull shader time drops measurably on maps with many regions (cell-heavy maps — the exact category `bvh-foundation/index.md:39` called out as the pivot trigger).

---

## Approach

Three tasks. A defines "region" and the compile-time partition. B lands the PRL format change (additive — keeps the flat tree). C rewrites the runtime dispatch.

```
A (region def)  ──── B (PRL format)  ──── C (runtime dispatch)
```

**One BVH, three consumers — preserved.** The flat global BVH remains the primary structure; per-region sub-BVHs are a *second view* over the same primitive set, emitted alongside. Bakers (SH, SDF) continue to consume the flat tree unchanged. Only the runtime cull consumer switches to the per-region view. See "Baker tolerance" under Architectural decisions below.

### Architectural decisions (locked)

- **Baker tolerance — flat tree stays.** `sh_bake.rs` and `sdf_bake.rs` consume the existing single `bvh::Bvh` unchanged. Cross-tree traversal via the portal graph would require modifying the `bvh` crate or writing a custom traversal loop; the bakers run at compile time and are not frame-critical; keeping them unchanged honors the "one BVH, three consumers" pattern from the master index. Per-region sub-BVHs are purely additive metadata for the GPU cull consumer.
- **Region granularity — one sub-BVH per cell (v1).** Region = PVS leaf = `cell_id` directly. No clustering in v1. Clustering is a follow-up if bucket fragmentation measurements (see acceptance) justify it.
- **PRL format — B1 (single section, region offset table).** Extend `BvhSection` with `region_count: u32` and an offset table. Commit; no N-sections alternative.
- **Gate — 0.5 ms cull-shader threshold.** See Pre-work section.

### Known trade-off: bucket fragmentation

Per-`(region, bucket)` contiguous slots multiply `multi_draw_indexed_indirect` call count by `visible_region_count`. For 16 buckets × 50 visible regions, ~800 indirect draws per frame. This is the expected failure mode for cell-heavy maps — the acceptance criteria explicitly measure it.

---

### Task A — Region definition and compile-time partition

**Crate:** `postretro-level-compiler` · **File:** `src/bvh_build.rs` (rewrite) + region-id plumbing upstream

**Region = PVS leaf (BSP empty-leaf) = `cell_id`.** The BSP compiler already produces these; `cell_id` on each primitive is the `FaceMetaV3.leaf_index` (`bvh-foundation/index.md:114`). No new partition invention — reuse the existing one. Clustering is out of scope for v1 (see Future section).

**Region stability.** `cell_id` is a property of the primitive set from geometry extraction — the BVH builder doesn't assign or reorder it. Collecting primitives filtered by `cell_id` before `Bvh::build` produces stable per-region sub-trees deterministically.

**Build — both views.** Emit:

1. The existing flat global BVH (unchanged — bakers consume it).
2. Per-region sub-BVHs: for each `cell_id`, collect `primitives.iter().filter(|p| p.cell_id == region_id)`, build a `bvh::Bvh` from them exactly as `bvh_build.rs` does today, flatten to dense node/leaf arrays. Emit `region_count` sub-trees.

Both views are derived from the same primitive set. No reordering between them beyond per-region sort (below).

**Leaf sort order preserved.** Current invariant: leaves sorted by `material_bucket_id` so each bucket owns a contiguous indirect-draw slot range (`bvh-foundation/index.md:79`). This must hold *per sub-BVH*. Either:

- Sort each sub-BVH's leaves locally; accept that bucket ranges are now per-region. `multi_draw_indexed_indirect` issues one call per `(region, bucket)` pair instead of per bucket.
- Sort globally first (primitives keyed by `(region_id, material_bucket_id)`), then build sub-BVHs over each region's slice. Bucket ordering within a region matches global.

Prefer the latter — it keeps bucket-level draw batching tight.

---

### Task B — PRL format: region offset table

**Crates:** `postretro-level-format`, `postretro-level-compiler` · **Files:** `postretro-level-format/src/bvh.rs`, `postretro-level-compiler/src/pack.rs`

Today the `Bvh` PRL section (`postretro-level-format/src/lib.rs:89`; `BvhSection` at `postretro-level-format/src/bvh.rs:70-239`, `HEADER_SIZE = 16` at line 81) is a single flattened section with one node array and one leaf array for the flat tree.

**Decision: B1.** Extend `BvhSection` with:

- Existing flat node/leaf arrays, unchanged. Bakers continue to consume these.
- New header fields: `region_count: u32` appended to the header; `HEADER_SIZE` grows.
- A region offset table `[(node_offset, node_count, leaf_offset, leaf_count); region_count]` written immediately after the expanded header, before the flat node array.
- Per-region sub-tree nodes and leaves written as additional flat arrays after the existing flat arrays. Each sub-BVH's `skip_index` values are region-local (relative to that region's node slice); runtime adds the region's node offset before indexing.

The offset table is `region_count * 16` bytes — trivial. Sub-tree payload adds one flattened tree per cell, but each is tiny (10–50 primitives → ~15 nodes each).

**Header migration.** No backward compat (pre-release per memory). Replace the payload shape. Update `postretro-level-format/src/bvh.rs` readers and writers (`BvhSection` read/write paths and `HEADER_SIZE`). Update the compiler packer (`postretro-level-compiler/src/pack.rs:12, 437`).

---

### Task C — Runtime: per-region dispatch

**Crate:** `postretro` · **Files:** `src/compute_cull.rs`, `src/shaders/bvh_cull.wgsl`, `src/visibility.rs`

Loader splits the flat node/leaf buffers into region-indexed slices. Storage buffers stay as single flat buffers (no per-region buffer allocation — just offsets); the shader is parameterized by `(region_node_offset, region_node_count)`.

**Dispatch model.** Portal DFS already produces a visible-cell set — convert it to a visible-region set (if region ≠ cell, aggregate). Push the list to the GPU as a storage buffer or uniforms. The compute shader then:

1. Reads `visible_regions: array<u32>` (N entries, `region_count` u32s).
2. For each entry, if the region's `node_count > 0`, walks nodes `[region_node_offset, region_node_offset + region_node_count)` with the same flat-DFS + skip-index loop. `skip_index` values in nodes are region-local; shader adds `region_node_offset` before indexing.

**Workgroup parallelism.** Today `@workgroup_size(1, 1, 1)` (`bvh_cull.wgsl:94`) — one invocation walks the tree. With N regions, dispatch `N` workgroups and have each invocation walk one region. Free parallelism. Workgroup size stays 1 unless we also parallelize *within* a region (out of scope).

**Cell check stays.** Sub-BVH scope matches region scope, but a leaf's `cell_id` may still be individually gated by the bitmask (e.g., region = cluster of 4 cells, only 2 visible). Keep the `cell_is_visible` test (`bvh_cull.wgsl:84–92`) as today.

**Indirect buffer layout unchanged.** Each BVH leaf still owns a permanent indirect-draw slot at its global leaf-array position. Per-region ordering is applied at build time (Task A); the runtime's `bucket_ranges` (`compute_cull.rs:89`) just reflects a per-`(region, bucket)` partition after the sort change. `multi_draw_indexed_indirect` call count goes up proportionally — worth measuring, may want coarser aggregation.

**Bakers (SH, SDF) — unchanged.** Per the Baker tolerance decision, `sh_bake.rs` and `sdf_bake.rs` continue to consume the flat global BVH exactly as today. The per-region offset table and sub-tree arrays are additive PRL payload the bakers ignore. No code changes in the bakers for this plan.

---

## Files to modify

| File | Task | Change |
|------|------|--------|
| `postretro-level-compiler/src/bvh_build.rs` | A | Keep global flat BVH build; additionally partition primitives by `cell_id` and emit one `bvh::Bvh` per region with region-local `skip_index`; preserve bucket ordering within each region |
| `postretro-level-format/src/bvh.rs` | B | Extend `BvhSection` (currently `:70-239`, `HEADER_SIZE = 16` at `:81`) with `region_count` and `RegionOffsets: [(node_offset, node_count, leaf_offset, leaf_count)]` table appended before per-region sub-tree arrays; update readers/writers |
| `postretro-level-compiler/src/pack.rs` | B | Pack per-region offset table and sub-tree arrays alongside the existing flat node/leaf arrays |
| `postretro/src/compute_cull.rs` | C | Load region offset table at init; dispatch `region_count` workgroups; pass visible-region list as storage buffer |
| `postretro/src/shaders/bvh_cull.wgsl` | C | Read `visible_regions` and `region_offsets` bindings; parameterize traversal loop by region slice; apply `region_node_offset` to `skip_index` values |
| `postretro/src/visibility.rs` | C | Aggregate visible cells → visible regions when region ≠ cell; upload per-frame |

---

## Acceptance

1. Output-visible primitive set is bit-identical before and after. Compare the indirect draw buffer's emitted `(index_count > 0)` leaves across the change — same set of leaves emitted, same index counts. Add a debug assertion or one-shot diff.
2. On a cell-heavy map (e.g. `occlusion-test.map` once fleshed out, or a generated map with ≥100 cells), runtime cull-shader GPU time drops measurably (`POSTRETRO_GPU_TIMING=1` — see CLAUDE.md). Target: ≥30% reduction on maps where visible-region count is <50% of total.
3. Baker outputs (SH section, SDF section) are bit-identical before and after — bakers continue to consume the flat tree unchanged, so byte-identity is expected. Use existing bake determinism tests.
4. **No map regresses.** Specifically measure both failure modes via `POSTRETRO_GPU_TIMING=1`:
   - **Small single-region maps** (overhead failure mode): per-region dispatch overhead must stay under the gain. Cull-shader time must not exceed the pre-change baseline.
   - **Cell-heavy maps** (bucket fragmentation failure mode): `multi_draw_indexed_indirect` call count rises proportional to `visible_region_count × bucket_count`; total frame time — including the draw pass, not just the cull shader — must not regress. If this regresses, the clustering follow-up (Future section) is triggered.
5. `cargo test -p postretro` and `cargo test -p postretro-level-compiler` pass.
6. `cargo clippy -- -D warnings` clean across workspace.
7. No new `unsafe`.

---

## Out of scope

- Changing the BVH build algorithm — stays SAH/binned via the `bvh` crate.
- Changing PVS format or portal traversal — `visibility.rs` changes only the cell→region aggregation, not the underlying algorithm.
- Runtime BVH refit for dynamic geometry — static world only, baked partition.
- Parallelism within a region's traversal — workgroup stays `@workgroup_size(1)`; one-invocation-per-region is the parallelism unit.
- Chunk-based region assignment (Milestone 8's chunk primitive). If chunks become the region unit later, this plan's scheme is additive: region ID becomes chunk ID.
- Changing `BvhLeaf.cell_id` semantics or the 128-word visible-cell bitmask.

---

## Open Questions

1. **Visible-region set representation.** Computed from `VisibleCells` on the CPU. Aggregation cost is `O(visible_cells)` per frame. Bitmask form vs. flat list — both are cheap; match the existing 128-word bitmask pattern for uniformity, or switch to a flat index list to avoid scanning empty words in the dispatch path. Decide during Task C implementation based on shader ergonomics.

*Previously open, now resolved by architectural decisions (see Approach §Architectural decisions):*

- ~~Region granularity~~ → **per-cell, v1.** Clustering moved to Future section.
- ~~PRL format B1 vs. B2~~ → **B1.**
- ~~Baker tolerance~~ → **flat tree stays; bakers unchanged.**
- ~~Bucket fragmentation acceptable?~~ → **known trade-off; measured in acceptance criterion 4.**
- ~~Pivot trigger condition~~ → **0.5 ms cull-shader gate in Pre-work section.**

---

## Future

- **Cell clustering.** If per-cell sub-BVHs produce bucket fragmentation that regresses total frame time on cell-heavy maps (acceptance criterion 4 failure), cluster adjacent cells into groups of ~1k primitives. Cluster assignment belongs in the BSP → portal stage; cluster ID replaces cell ID in the region partition. The PRL format and runtime dispatch from this plan carry over unchanged — only the Task A partition step changes.
- **Parallelism within a region's traversal.** Workgroup size stays 1 in v1. If a few large regions dominate total dispatch time, parallelize the DFS within those regions.
