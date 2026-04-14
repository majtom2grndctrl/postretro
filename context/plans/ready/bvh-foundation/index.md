# BVH Foundation

> **Status:** draft — architectural direction locked. Sub-plans fill out as we drill into each one.
> **Milestone:** 4 (BVH Foundation) — see `context/plans/roadmap.md`.
> **Related:** `context/lib/rendering_pipeline.md` §5 · `context/lib/build_pipeline.md`
> **Prerequisite:** Milestone 3.5 (Rendering Foundation Extension). Vertex format with packed normals + tangents and the GPU-driven indirect draw architecture must be in place. This plan reuses both unchanged.
> **Blocks:** Milestone 5 (Lighting Foundation) — the SH baker traverses the BVH built here.
> **Lifecycle:** Per `context/lib/development_guide.md` §1.5, this plan *is* the BVH spec while it's a draft. Decisions land in the spec as they're made; durable knowledge migrates to `context/lib/` when the plan ships.

---

## Goal

Replace Milestone 3.5's per-cell chunk compute cull with a global BVH over all static geometry. Ship with visual parity to Milestone 3.5 — flat ambient, no lighting changes, identical rendered output — but lay the spatial structure that Milestone 5's SH baker needs.

One acceleration structure, two consumers:

- **Runtime:** WGSL compute shader walks the BVH each frame, frustum-culls, emits `DrawIndexedIndirect` commands into the existing fixed-slot indirect buffer.
- **Bake-time (Milestone 5):** CPU `bvh` crate traverses the same tree to ray-cast probe radiance samples.

Same tree, two traversal implementations. No second design pass when Milestone 5 begins.

---

## Why BVH, why now

Milestone 3.5 shipped a per-cell chunk table and compute-cull shader. That works, but Milestone 5 needs a compile-time acceleration structure for baker ray casts anyway — and Milestone 3.5's review flagged finding #1 (`FaceMetaV3.index_offset/index_count` stale after chunk reordering) which reworks the same compiler data path. Doing the BVH refactor now gives us:

- One acceleration structure for runtime cull *and* bake-time ray casts. No second design pass in Milestone 5.
- A clean place to land Milestone 3.5 review finding #1 — the compiler's face→index pipeline gets rewritten end-to-end, so the stale-metadata bug evaporates as a free side effect.
- No backward compat shim. `CellChunks` section id retires, `chunk_grouping.rs` deletes, `compute_cull.rs` rewrites, `POSTRETRO_FORCE_LEGACY` diagnostic deletes with it. Pre-release, own the refactor.

---

## Architectural commitments

These are locked. Sub-plans assume them; debate them at the plan level, not in implementation tickets.

- **Global BVH, not per-region.** Single flat hierarchy over all static geometry. Per-region is the pivot path if the check-in (sub-plan 3) shows global doesn't hit frame-time parity on cell-heavy maps — designed for as a fallback, not as day-one scope.
- **Software traversal only.** No hardware ray tracing. Target is pre-RTX hardware and wgpu (which doesn't expose hardware RT regardless). Runtime traversal is a WGSL compute shader over a flat node/leaf storage buffer. Bake-time traversal is CPU through the `bvh` crate. Same structure, two traversal implementations, zero hardware assumptions.
- **Portals stay.** Portal DFS still produces the visible-cell set — BVH replaces per-chunk frustum culling, not occlusion culling. Portal output feeds the BVH traversal compute shader; the integration shape lands in sub-plan 2.
- **Fixed-slot indirect buffer preserved.** Milestone 3.5's no-atomic-counter design survives this refactor: each BVH leaf gets a permanent indirect-buffer slot, so overflow stays architecturally impossible.
- **No backward compat.** Pre-release. `CellChunks` section id retires, legacy paths delete, no compat shims.

---

## Pipeline

```
prl-build (compile time)              postretro (runtime)
────────────────────────              ─────────────────────

geometry + portals                    .prl loader
  ↓                                     ↓
global BVH build (bvh crate)          BVH storage buffer upload
  ↓                                     ↓
flatten → node[] + leaf[]             portal DFS → visible cells
  ↓                                     ↓
write Bvh PRL section                 WGSL BVH traversal compute
                                        ↓
                                      indirect draw buffer
                                        ↓
                                      multi_draw_indexed_indirect
                                        (one call per material bucket)
```

---

## Data contracts

These cross-sub-plan contracts are pinned here. Sub-plans reference them; don't duplicate.

**Node/leaf byte layout (storage buffers)**
- Node stride: 40 bytes (6×f32 + 4×u32). Natural (4-byte) alignment; no additional padding required for WGSL storage buffers.
- Leaf stride: 40 bytes (6×f32 + 4×u32 including `cell_id`). Same rule.
- Nodes written in DFS order. Each node carries `skip_index` (u32) pointing to the next sibling subtree root — the value to jump to on AABB reject. Left child is always at `current_index + 1`.

**Leaf sort order and indirect buffer slot assignment**
- Leaves are sorted by `material_bucket_id` in the flat leaf array. Each material bucket owns a contiguous slice `[first_leaf, first_leaf + count)`.
- Each leaf's position in the leaf array is its permanent indirect buffer slot index. No atomic counter. Overflow is architecturally impossible.
- The per-bucket `(first_slot, count)` table is **not stored in the PRL section**. At load time the runtime scans the sorted leaf array once (O(leaf_count)) to derive it. This scan runs once; it is not on a hot path. `multi_draw_indexed_indirect` issues one call per bucket using the derived ranges.

**Visible-cell bitmask**
- Fixed 128 `u32` words = 512 bytes, covering up to 4096 cells.
- Bit test: `(bitmask[cell_id >> 5u] & (1u << (cell_id & 31u))) != 0u`.
- `VisibleCells::DrawAll` → all 128 words `0xFFFFFFFF`.
- Buffer cleared to zero at frame start; portal DFS sets bits for visible cells before compute dispatch.

---

## Sub-plans

This plan has three sub-files, executed in order:

1. **[1-compile-bvh.md](./1-compile-bvh.md)** — Compile-time BVH construction in `prl-build`. Builds the BVH, flattens to dense node/leaf arrays, defines and writes the new `Bvh` PRL section. Retires `chunk_grouping.rs` and `CellChunks`. Folds in Milestone 3.5 review finding #1.

2. **[2-runtime-bvh.md](./2-runtime-bvh.md)** — Runtime loader, GPU upload, and WGSL BVH traversal compute shader. Deletes `CellChunkTable`, `chunks_for_cell`, `determine_prl_visibility`, `POSTRETRO_FORCE_LEGACY`. Decides and documents how portal DFS output filters BVH leaves.

3. **[3-checkin.md](./3-checkin.md)** — Lightweight check-in. Manual screenshot review for visual parity, frame-time spot check on a cell-heavy map, sign-off before Milestone 5 begins. If parity fails, decide whether to pivot to per-region BVH.

Sub-plans 1 and 2 are concrete bodies of implementation work, each sized to fit one or two execution-agent dispatches. Sub-plan 3 is a conversation gate, not an implementation task.

---

## Resolved questions

| Question | Decision | Rationale |
|----------|----------|-----------|
| BVH spatial strategy | **Global BVH, not per-region** | Try global first. Per-region is the pivot path if global underperforms on cell-heavy maps. Own the refactor; don't pre-optimize for a scaling problem that may never arrive. |
| BVH / CellChunks coexistence | **BVH replaces CellChunks entirely** | No backward compat shim. `CellChunks` section id retires; `chunk_grouping.rs`, `CellChunkTable`, `chunks_for_cell`, `POSTRETRO_FORCE_LEGACY`, and `determine_prl_visibility` all delete. Pre-release — own it. |
| Runtime BVH traversal | **WGSL compute shader over flat storage buffers** | No Rust crate ships GPU BVH traversal for wgpu. Pre-RTX hardware target + wgpu doesn't expose hardware RT regardless. Custom shader is the established pattern. |
| Bake-time BVH traversal | **CPU `bvh` crate — same tree, different traversal** | One acceleration structure, two consumers. Milestone 5's baker calls the `bvh` crate directly; no GPU round-trip at compile time. |
| Milestone 3.5 review finding #1 | **`FaceMetaV3.index_offset/index_count` retired** | Sub-plan 1 rewrites the compiler's face → index pipeline end-to-end; these fields are removed from `GeometryV3`. All index ranges are owned by BVH leaves. |
| `cell_id` provenance | **`FaceMetaV3.leaf_index` direct** | Faces never straddle BSP leaf boundaries — the geometry extractor splits hulls at leaf boundaries. `cell_id = leaf_index` at primitive collection; no new tracking needed. |
| GPU traversal strategy | **Skip-index (flat DFS), not stack-based** | BVH flattened in DFS order with `skip_index` per node pointing to next sibling. No explicit stack, no depth cap, simpler shader. Standard approach for software GPU BVH. |
| Visible-cell representation | **128-word bitmask (512 bytes fixed)** | O(1) per-leaf test vs. O(n) linear scan. `VisibleCells` Vec converted to bitmask on CPU before upload. Fixed size covers up to 4096 cells; never needs resizing. |
| Indirect buffer slot partitioning | **Leaves sorted by `material_bucket_id`; contiguous per-bucket ranges** | Each bucket owns a contiguous slice of the leaf array (and thus the indirect buffer). `multi_draw_indexed_indirect` issues one call per bucket with `(first_slot, count)`. No per-bucket sub-buffers needed. |

---

## When this plan ships

Durable architectural decisions migrate to `context/lib/`:

- Global BVH rationale and the per-region pivot condition — document the decision and the fallback, so a future contributor knows why we chose global and what would trigger a pivot.
- `Bvh` PRL section layout (header + flat node/leaf arrays, byte shape, endianness).
- WGSL BVH traversal shader structure — the skip-index (flat DFS) traversal pattern, portal bitmask integration shape — lands as a new section in `rendering_pipeline.md` §5 (replacing the cell-chunk description).
- `bvh` crate usage for compile-time primitive build; flatten-to-buffer lowering.
- Finding #1 postmortem note: `FaceMetaV3` stale-index-range bug and how it dissolved under the pipeline rewrite.

The plan document itself is ephemeral per `development_guide.md` §1.5.
