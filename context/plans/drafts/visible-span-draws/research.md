# visible-span-draws — research

Derivation behind `index.md`. Source read at 832e20c8a. The measurements were taken at 97cf912ec, and the only commits between the two are to docs.

## Measurements

Taken 2026-10-01 on the compatibility-floor Mac (Radeon Pro 5300M, Metal). Release build, Auto preset, `sample` profile.

| Map | BVH leaves | Camera indirect draws / frame (steady state) | Driver draw + indirect validation / frame |
|---|---|---|---|
| stress-warren-hallway-inspection | 8,437 | 16,874 | 1.79 ms |
| campaign-test | 774 | 1,548 | 0.16 ms |

- Cost ratio: about 11×. Leaf ratio: about 10.9×. Cost tracks total leaves.
- Where the cost sits:
  - Profile frames: `DrawBatcher::add` and `inject_validation_pass` in wgpu-core, plus the Metal `drawIndexedPrimitives` loop.
  - Call path: under `CommandEncoder::finish` → `encode_render_pass`.
  - CPU stage: timed under `render_submit` (`RenderStage::Submit` scope around `submit_windowed_frame`).
- Draws per frame = 2·L + L·(uncached shadow slots and cube faces). L is the total leaf count. This brief removes only the 2·L camera term, which becomes 2 · (Σ visible span lengths).

## Pins

Verified this session, by symbol.

| Claim | Where |
|---|---|
| Camera draw issue: one `multi_draw_indexed_indirect(buf, off, leaf_count)` per non-empty `BucketRange`. Without multi-draw, it loops `draw_indexed_indirect` per slot instead. | `compute_cull::draw_indirect_buckets` |
| `has_multi_draw_indirect` comes from `DownlevelFlags::INDIRECT_EXECUTION`. | `renderer_init.rs` |
| Depth prepass caller passes no material bind. Forward caller binds the bucket texture per bucket. | `ComputeCullPipeline::draw_indirect` from `renderer_shadow_passes.rs` and `renderer_render_frame.rs` |
| Shadow passes share `draw_indirect_buckets` over whole buckets, per slot sub-region. | `ShadowCullPipeline::draw_slot_indirect` |
| Tree walk writes `index_count = 0` for culled leaves, and for a rejected leaf node. | `bvh_cull.wgsl::cull_main` |
| Candidate path clears the camera indirect and status ranges, then writes only gathered leaves. | `CandidateCullPipeline` dispatch (`clear_buffer`), `candidate_cull.wgsl` |
| wgpu-hal 29.0.1 Metal `draw_indexed_indirect` loops `draw_count` times, one `drawIndexedPrimitives` each. `draw_indexed_indirect_count` is a `//TODO` stub. `MULTI_DRAW_INDIRECT_COUNT` is advertised only by dx12 and vulkan. | `wgpu-hal-29.0.1/src/metal/command.rs`; `dx12/adapter.rs`, `vulkan/adapter.rs` |
| wgpu-core batches per-draw indirect validation. | `indirect_validation::DrawBatcher`, `inject_validation_pass` |
| Bucket ranges: contiguous, derived from the bucket-sorted leaf array. | `BvhTree::derive_bucket_ranges` |

## Load-bearing hypothesis: holds

The hypothesis: the candidate path already expands visible cells' `CellDrawIndex` spans on the CPU, and each span lies inside one bucket's contiguous slot range.

- **Spans are expanded on the CPU.** `gather_candidate_leaves` walks `cell_span_offset[c]..[c+1]` for each deduped visible cell and expands every span into single leaf indices. The span grouping is discarded, so span draws read the index directly rather than reusing the gather's output.
- **Each span lies inside one bucket.** This is enforced at both ends:
  - Bake: `bake_cell_draw_index` breaks a run where the slot order is non-contiguous and where `material_bucket_id` changes.
  - Load: `validate_cell_draw_index` rejects a span that crosses a bucket boundary, a non-maximal same-bucket run, overlap, a non-drawable leaf, and any drawable leaf left uncovered.
- **The leaf sort key is (bucket, cell, index_offset)** (`bvh_build::flatten`). So today each (cell, bucket) pair has at most one span, and within a bucket the spans run in cell-id order. The brief relies on neither fact. The bake contract allows several spans per (cell, bucket), and the visible set arrives unsorted.
- **Caveat that shapes the design.** The gather runs only when `visibility_path_uses_candidate_cull` accepts the path, which means the portal walk and the step-limit fallback. The solid-cell, exterior and no-portals fallbacks run the tree walk, but they still carry a concrete `VisibleCells::Culled` set. So the CPU knows the cell set on every path except draw-all, and span draws can't hang off the gather alone.

## Visible-set sources

| Source | `VisibleCells` | Cull today | Draw ranges after |
|---|---|---|---|
| Portal walk | `Culled` | candidate | spans |
| Step-limit fallback | `Culled` (bounded frustum cull) | candidate | spans |
| Solid cell / exterior / no portals | `Culled` (frustum cull over all cells) | tree walk | spans |
| Empty world | `DrawAll` | tree walk | whole buckets |
| Visible id ≥ index `cell_count` | `Culled` | tree walk (that frame) | whole buckets |
| No index loaded (zero-leaf maps only; a non-empty BVH without one fails load) | — | tree walk | whole buckets |

- **Fog reach is not a draw input.** `fog_reachable` feeds `compute_fog_cell_mask` and `prepare_streamed_sh_compose`, and nothing in world drawing. `VisibilityResult` produces it separately from `visible_cells`.
- **Ids past the tree walk's bitmask.** The tree walk drops cell ids at or past the bitmask capacity (`write_bitmask_from_cells` warns). Span draws over such a cell are still safe: its slots stay zero, so the draws cost time but draw nothing.

## Superset reasoning

Both culls write a nonzero slot only for a leaf in a visible cell. The candidate cull gathers only visible cells' leaves. The tree walk tests each leaf's cell bit. Every drawable leaf of a visible cell lies in one of that cell's spans. So the union of the visible cells' spans covers every slot the cull can make nonzero this frame.

One pre-existing behavior is the exception, and it is harmless. The tree walk jumps a frustum-rejected internal node without rewriting the leaves under it, so a slot written on an earlier frame can stay nonzero. `ShadowCullPipeline::draw_slot_indirect` documents the same property for shadow slots. The candidate path clears every slot each frame, so only tree-walk frames carry stale slots. A stale slot's leaf lies inside a node that is outside the frustum, so it clips. Today it gets drawn and clipped. After this change it is drawn only if its cell is visible. The image is identical either way. This is noted here, not fixed, and it is not in scope.

## Prior commitment

`plans/done/perf-visible-cell-candidate-cull` kept `bucket_ranges` and the per-bucket `draw_indirect_buckets` call count unchanged, with "no per-(cell, bucket) fragmentation". It left compaction as a later decision, to be made on measured pressure. `plans/done/perf-per-region-bvh` warned that call count grows with region × bucket fragmentation.

Both commitments counted wgpu-level calls. The measurement shows the cost is per driver draw: Metal pays once per slot inside a multi-draw. Fragmenting into coalesced visible runs adds at most one wgpu call per run, and it removes every draw over a slot in an invisible cell.
