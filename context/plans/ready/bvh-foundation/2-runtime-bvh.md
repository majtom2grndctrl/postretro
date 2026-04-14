# Sub-plan 2 — Runtime BVH

> **Parent plan:** [BVH Foundation](./index.md) — read first for goals and architectural commitments.
> **Scope:** all engine-side BVH work. Loader parses the new `Bvh` PRL section into GPU storage buffers, the compute-cull shader is rewritten as a BVH traversal compute, and legacy fallback paths delete. Portal integration: per-leaf visible-cell bitmask check (decision resolved — see below).
> **Crates touched:** `postretro` only.
> **Depends on:** sub-plan 1 (the `Bvh` PRL section must exist and every test map must emit one).
> **Blocks:** sub-plan 3 (check-in needs runtime path working end-to-end).

---

## Description

Update `postretro/src/prl.rs` to parse the new `Bvh` section and expose `node_array` and `leaf_array` as `wgpu::Buffer` storage buffers via `LevelWorld`. Rewrite `postretro/src/compute_cull.rs` as a skip-index WGSL BVH traversal compute shader that frustum-culls and emits `DrawIndexedIndirect` commands into the existing fixed-slot indirect buffer. Delete `CellChunkTable`, `chunks_for_cell`, `determine_prl_visibility`, the `POSTRETRO_FORCE_LEGACY` diagnostic mode, and the CPU-side draw-range reconstruction path. Decide and document how portal DFS output filters BVH leaves.

The Milestone 3.5 invariants are preserved unchanged: fixed-slot indirect buffer (no atomic counter, no overflow), `MULTI_DRAW_INDIRECT` feature probe with singular `draw_indexed_indirect` fallback, one `multi_draw_indexed_indirect` call per material bucket, per-leaf cull-status buffer feeding the wireframe overlay (`Alt+Shift+\`).

---

## WGSL compute shader shape

**Traversal strategy: skip-index (flat DFS), not stack-based.**

The BVH is flattened in depth-first order by sub-plan 1. Each internal node carries a `skip_index`: the array index to jump to when the current subtree is rejected (i.e. the index of the next sibling subtree). This eliminates the explicit stack entirely, has no depth cap, and is the standard approach for software GPU BVH traversal.

These WGSL structs must match the binary layout in sub-plan 1 byte-for-byte. The interleaved (vec3 + u32) layout takes advantage of WGSL storage buffer packing: `vec3<f32>` occupies 12 bytes (no implicit padding), so each pair of fields fits in a 16-byte row with no gaps.

```wgsl
struct BvhNode {
    aabb_min: vec3<f32>,           // offset  0, 12 bytes
    skip_index: u32,               // offset 12,  4 bytes — jump here on AABB reject
    aabb_max: vec3<f32>,           // offset 16, 12 bytes
    left_child_or_leaf_index: u32, // offset 28,  4 bytes — leaf_array index if is_leaf, else 0
    flags: u32,                    // offset 32,  4 bytes — bit 0: is_leaf; remaining bits reserved (0)
    _pad: u32,                     // offset 36,  4 bytes
    // stride: 40 bytes
};

struct BvhLeaf {
    aabb_min: vec3<f32>,   // offset  0, 12 bytes
    material_bucket_id: u32, // offset 12,  4 bytes
    aabb_max: vec3<f32>,   // offset 16, 12 bytes
    index_offset: u32,     // offset 28,  4 bytes
    index_count: u32,      // offset 32,  4 bytes
    cell_id: u32,          // offset 36,  4 bytes
    // stride: 40 bytes
};
```

The shader (single invocation, `@workgroup_size(1,1,1)`, walks entire tree per frame):

1. Reads `node_array` and `leaf_array` as storage buffers.
2. Reads `visible_cells_bitmask` (see "Portal DFS integration" below) as a storage buffer.
3. Reads the view frustum planes as a uniform.
4. Iterates `i = 0`; loop while `i < node_count`. For each node:
   - If node AABB fails frustum test → `i = node.skip_index` (skip entire subtree).
   - If AABB passes and node is internal (flags & 1 == 0) → `i += 1` (descend to left child).
   - If AABB passes and node is a leaf (flags & 1 != 0):
     - Fetch `leaf = leaf_array[node.left_child_or_leaf_index]`.
     - Test leaf AABB against frustum (parent may have a larger AABB; leaf test is required for correctness).
     - If leaf AABB passes and `cell_id` is set in bitmask → write `DrawIndexedIndirect` to leaf's fixed slot (`node.left_child_or_leaf_index`); else clear slot (zero index_count).
     - `i += 1`.

No stack, no depth cap, no abort path. One invocation per dispatch suffices at current scene scales; revisit parallelism only if profiling shows it's needed.

**Note:** sub-plan 1 must write nodes in DFS order with correct `skip_index` values during flattening. Left child is always at `i + 1`; `skip_index` encodes the first node of the right sibling subtree.

---

## Portal DFS integration — resolved

**Decision: per-leaf visible-cell bitmask check.**

BVH leaves carry a `cell_id: u32` field. The portal DFS runs first each frame and builds a visible-cell bitmask, which is uploaded to a storage buffer. During BVH traversal, each leaf that passes the frustum test is checked against this bitmask — if its cell isn't visible, no draw command is emitted.

**Bitmask format:**
- Fixed size: 128 `u32` words = 512 bytes, covering up to 4096 cells (well above any map this engine will produce).
- Bit test: `(bitmask[cell_id >> 5u] & (1u << (cell_id & 31u))) != 0u`.
- CPU side: `visibility.rs` converts the existing `VisibleCells::Culled(Vec<u32>)` (flat list of cell IDs) to a bitmask before upload. `DrawAll` variant sets all 128 words to `0xFFFFFFFF`.
- Cleared to zero at frame start before portal DFS runs.
- Buffer: `STORAGE | COPY_DST`, 512 bytes, allocated once at level load, never resized.

Rationale: one traversal, simple shader code, O(1) per-leaf cost. The portal DFS and BVH stay decoupled. The alternative (multi-frustum traversal — one BVH pass per portal-narrowed frustum) was rejected: tighter cull isn't worth N traversals per frame and the added shader complexity.

**Consequence for sub-plan 1:** BVH leaf layout gains `cell_id: u32` (resolved — see `1-compile-bvh.md`).

---

## Acceptance criteria

- [ ] `Bvh` section parses into GPU-ready storage buffers at level load
- [ ] `CellChunkTable`, `chunks_for_cell`, `determine_prl_visibility`, and `POSTRETRO_FORCE_LEGACY` deleted; no references remain
- [ ] Legacy V1/V2 `.prl` files (if any) either load cleanly or fail with a clear version error — no half-broken state
- [ ] Compute shader dispatches each frame; render pass issues zero CPU per-leaf draws
- [ ] All visibility fallback paths feed the BVH traversal: portal traversal, SolidLeafFallback, ExteriorCameraFallback (preserves X-ray behavior), EmptyWorldFallback — portal DFS stays, BVH replaces the cell-chunk cull only
- [ ] Empty visible set → `multi_draw_indexed_indirect` with count 0, no GPU errors
- [ ] `MULTI_DRAW_INDIRECT` absent → fallback issues one `draw_indexed_indirect` call per visible leaf (CPU loop over leaf array); identical visual output
- [ ] Per-leaf cull-status buffer continues to drive the `Alt+Shift+\` wireframe overlay
- [ ] Portal integration choice documented in WGSL header comment with the rejected multi-frustum alternative noted
- [ ] `cargo test -p postretro` passes
- [ ] `cargo clippy -p postretro -- -D warnings` clean

---

## Implementation tasks

1. Extend `postretro/src/prl.rs`: parse `Bvh` section, allocate node-array and leaf-array `wgpu::Buffer` storage buffers, expose them via `LevelWorld`. Delete `CellChunkTable`, `chunks_for_cell`, `determine_prl_visibility`, `POSTRETRO_FORCE_LEGACY`, and the CPU-side draw-range reconstruction path.

2. Update `postretro/src/visibility.rs` to stop producing `DrawRange` output. Portal DFS still produces a visible-cell set; the BVH traversal compute shader consumes it directly.

3. Verify legacy V1/V2 `.prl` files either load cleanly or fail with a clear version error. Run the full test suite; `cargo clippy -p postretro -- -D warnings` clean.

4. Rewrite `postretro/src/compute_cull.rs` as a skip-index BVH traversal compute shader (`@workgroup_size(1,1,1)`). DFS iteration with `skip_index` on AABB reject (no stack, no depth cap); frustum test on internal node AABB; on leaf nodes, fetch the `BvhLeaf` via `node.left_child_or_leaf_index`, test leaf AABB against frustum (required — parent AABB may be larger), then check `cell_id` against bitmask; if both pass, write `DrawIndexedIndirect` to the leaf's fixed slot; else zero the slot. At load time the Rust side scans the sorted leaf array once (O(leaf_count)) to derive a per-bucket `(first_slot, count)` table; `multi_draw_indexed_indirect` uses this table to issue one call per bucket.

5. Implement the portal integration: convert `VisibleCells` to a 128-word bitmask; upload to a 512-byte `STORAGE | COPY_DST` buffer; bind in the compute shader. Clear to zero each frame before portal DFS. `DrawAll` sets all words to `0xFFFFFFFF`. Document the approach in a WGSL header comment, noting the rejected multi-frustum alternative.

6. Preserve Milestone 3.5 invariants: `MULTI_DRAW_INDIRECT` feature probe + singular `draw_indexed_indirect` fallback, one `multi_draw_indexed_indirect` call per material bucket, per-leaf cull-status buffer feeding the wireframe overlay.

7. Edge-case tests: empty visible set, degenerate BVH (single leaf), deeply unbalanced BVH (simulate thin corridor map), first-frame dispatch before steady state.

---

## Notes for implementation

- **Skip-index traversal.** Nodes are written in DFS order by sub-plan 1. `skip_index` on each node points to the first node in the next sibling subtree (i.e. the node to visit when the current subtree is rejected). Left child is always at `current_index + 1`. This is a standard flat BVH traversal — see sub-plan 1's flattening step for how `skip_index` values are computed.
- **Indirect buffer slot assignment.** Each BVH leaf gets a permanent slot in the indirect buffer, indexed by its position in the leaf array. Leaves are sorted by `material_bucket_id` (by sub-plan 1), so each bucket occupies a contiguous range `[first_leaf, first_leaf + count)`. The per-bucket `(first_slot, count)` table is **not stored in the PRL section** — at load time the Rust side scans the sorted leaf array once (O(leaf_count)) to derive it. This scan runs once at level load; it is not on any hot path. The compute shader writes the full `DrawIndexedIndirect` for surviving leaves and zeros `index_count` for culled ones; culled slots produce zero-vertex draws which the GPU discards cheaply. The render pass calls `multi_draw_indexed_indirect` once per bucket using the load-derived ranges.
- **Cell ID on BVH leaves.** `cell_id` = `FaceMetaV3.leaf_index` from the compiler, which equals the BSP leaf index. The runtime treats it as an opaque identifier for bitmask lookup only.
