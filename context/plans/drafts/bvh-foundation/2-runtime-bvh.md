# Sub-plan 2 — Runtime BVH

> **Parent plan:** [BVH Foundation](./index.md) — read first for goals and architectural commitments.
> **Scope:** all engine-side BVH work. Loader parses the new `Bvh` PRL section into GPU storage buffers, the compute-cull shader is rewritten as a BVH traversal compute, and legacy fallback paths delete. Portal integration: per-leaf visible-cell bitmask check (decision resolved — see below).
> **Crates touched:** `postretro` only.
> **Depends on:** sub-plan 1 (the `Bvh` PRL section must exist and every test map must emit one).
> **Blocks:** sub-plan 3 (check-in needs runtime path working end-to-end).

---

## Description

Update `postretro/src/prl.rs` to parse the new `Bvh` section and expose `node_array` and `leaf_array` as `wgpu::Buffer` storage buffers via `LevelWorld`. Rewrite `postretro/src/compute_cull.rs` as a stack-based WGSL BVH traversal compute shader that frustum-culls and emits `DrawIndexedIndirect` commands into the existing fixed-slot indirect buffer. Delete `CellChunkTable`, `chunks_for_cell`, `determine_prl_visibility`, the `POSTRETRO_FORCE_LEGACY` diagnostic mode, and the CPU-side draw-range reconstruction path. Decide and document how portal DFS output filters BVH leaves.

The Milestone 3.5 invariants are preserved unchanged: fixed-slot indirect buffer (no atomic counter, no overflow), `MULTI_DRAW_INDIRECT` feature probe with singular `draw_indexed_indirect` fallback, one `multi_draw_indexed_indirect` call per material bucket, per-leaf cull-status buffer feeding the wireframe overlay (`Alt+Shift+\`).

---

## WGSL compute shader shape

The shader:

1. Reads the node array and leaf array as storage buffers.
2. Reads the portal DFS output (visible-cell bitmask or equivalent) as another storage buffer.
3. Reads the view frustum as a uniform.
4. Walks the BVH top-down from the root, stack-based with a fixed maximum depth (initial cap: 64 — revisit if deep trees surface).
5. For each node, rejects subtrees whose AABB fails the frustum test. For survivors that are leaves, emits a `DrawIndexedIndirect` command into the fixed-slot indirect buffer.

Stack depth cap reached → compute shader aborts traversal cleanly (no invalid writes) and logs once.

---

## Portal DFS integration — resolved

**Decision: per-leaf visible-cell bitmask check.**

BVH leaves carry a `cell_id: u32` field. The portal DFS runs first each frame and writes a visible-cell bitmask to a storage buffer. During BVH traversal, each leaf that passes the frustum test is also checked against this bitmask — if its cell isn't in the visible set, no draw command is emitted.

Rationale: one traversal, simple shader code, cheap per-leaf cost (one `u32` field + one bitmask read per surviving leaf). The portal DFS and BVH stay decoupled — portals do what portals do, BVH does what BVH does. The alternative (multi-frustum traversal — one BVH pass per portal-narrowed frustum) was rejected: tighter cull isn't worth N traversals per frame and the added shader complexity.

**Consequence for sub-plan 1:** the BVH leaf layout in the PRL section gains a `cell_id: u32` field. Sub-plan 1 must include this when defining the leaf record format.

---

## Acceptance criteria

- [ ] `Bvh` section parses into GPU-ready storage buffers at level load
- [ ] `CellChunkTable`, `chunks_for_cell`, `determine_prl_visibility`, and `POSTRETRO_FORCE_LEGACY` deleted; no references remain
- [ ] Legacy V1/V2 `.prl` files (if any) either load cleanly or fail with a clear version error — no half-broken state
- [ ] Compute shader dispatches each frame; render pass issues zero CPU per-leaf draws
- [ ] All visibility fallback paths feed the BVH traversal: portal traversal, SolidLeafFallback, ExteriorCameraFallback (preserves X-ray behavior), EmptyWorldFallback — portal DFS stays, BVH replaces the cell-chunk cull only
- [ ] Empty visible set → `multi_draw_indexed_indirect` with count 0, no GPU errors
- [ ] `MULTI_DRAW_INDIRECT` absent → fallback dispatch mode produces identical visual output
- [ ] Stack depth cap reached → compute shader aborts traversal cleanly and logs once
- [ ] Per-leaf cull-status buffer continues to drive the `Alt+Shift+\` wireframe overlay
- [ ] Portal integration choice documented in WGSL header comment with the rejected alternative noted
- [ ] Visual parity with Milestone 3.5 by eye on every test map (formal parity check is sub-plan 3)
- [ ] `cargo test -p postretro` passes
- [ ] `cargo clippy -p postretro -- -D warnings` clean

---

## Implementation tasks

1. Extend `postretro/src/prl.rs`: parse `Bvh` section, allocate node-array and leaf-array `wgpu::Buffer` storage buffers, expose them via `LevelWorld`. Delete `CellChunkTable`, `chunks_for_cell`, `determine_prl_visibility`, `POSTRETRO_FORCE_LEGACY`, and the CPU-side draw-range reconstruction path.

2. Update `postretro/src/visibility.rs` to stop producing `DrawRange` output. Portal DFS still produces a visible-cell set; the BVH traversal compute shader consumes it directly.

3. Verify legacy V1/V2 `.prl` files either load cleanly or fail with a clear version error. Run the full test suite; `cargo clippy -p postretro -- -D warnings` clean.

4. Rewrite `postretro/src/compute_cull.rs` as a BVH traversal compute shader. Top-down stack-based traversal (fixed max depth 64); frustum test on internal nodes; emit `DrawIndexedIndirect` per surviving leaf into the fixed-slot indirect buffer.

5. Implement the portal integration: per-leaf visible-cell bitmask check. BVH leaves carry `cell_id: u32`; portal DFS writes a visible-cell bitmask to a storage buffer; traversal emits no draw command for leaves whose cell isn't set. Document the approach in a header comment at the top of the WGSL file, noting the rejected multi-frustum alternative.

6. Preserve Milestone 3.5 invariants: `MULTI_DRAW_INDIRECT` feature probe + singular `draw_indexed_indirect` fallback, one `multi_draw_indexed_indirect` call per material bucket, per-leaf cull-status buffer feeding the wireframe overlay.

7. Edge-case tests: empty visible set, degenerate BVH (single leaf), deeply unbalanced BVH (simulate thin corridor map), first-frame dispatch before steady state.

---

## Notes for implementation

- **Stack depth cap = 64.** A balanced BVH over ~10k primitives has depth ~14. The 64 cap is a generous headroom for unbalanced trees on thin-corridor maps. If a real map hits the cap, the right fix is a better build heuristic in sub-plan 1, not a bigger stack.
- **Indirect buffer slot assignment.** Each BVH leaf gets a permanent slot in the indirect buffer, indexed by its leaf array index. The compute shader writes `DrawIndexedIndirect{ index_count: 0 }` for culled leaves and the actual draw for visible ones. The render pass dispatches the entire buffer; culled slots produce zero-vertex draws which the GPU discards cheaply. This is the Milestone 3.5 design preserved.
- **Cell ID on BVH leaves.** The cell ID isn't a BSP leaf index — it's the runtime cell identifier (matching `cell_id` from `rendering_pipeline.md` §5). Sub-plan 1's compiler determines this from BSP leaf at primitive-collection time.
