# visible-span-draws — research

Derivation behind `index.md`. Source first read at 832e20c8a and rechecked at 4278d8789. The seams the brief names changed only mechanically in between: the upload-queue type and let-chain rewrites. The measurements were taken at 97cf912ec.

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

### After release indirect validation (re-baselined 2026-10-03)

`release-indirect-validation` landed at 984604de7, after these measurements, and turns indirect validation off in release builds. The table above is the validation-on baseline. A reconciliation of the same `sample` runs (`drafts/bvh-leaf-clustering/research.md` §Reconciled render_submit) splits it, in ms per frame for stress / campaign:
- wgpu-hal Metal `draw_indexed_indirect`, per draw: **0.83 / 0.08**. This is what this brief removes for invisible cells, about 49 ns per slot.
- Indirect validation: 1.29 / 0.54, now gone in release.
- Other per-draw `encode_render_pass` work: about 0 with validation off.

Visible leaves: about 0.5% on stress (37–41 of 8,437) and about 18% on campaign (138–145 of 774). The expected saving is about 0.8 ms on stress and at most about 0.065 ms gross on campaign. Each extra wgpu call per coalesced run eats into that campaign margin, which is why the coalescing rule matters. After validation-off, the measured hallway `render_submit` median is 3.81 ms (`release-indirect-validation` plan of record).

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
| Every BVH leaf names an in-range, drawable cell. So any leaf the tree walk can submit (nonzero index count, visible cell) is a drawable leaf, and it lies in one of its cell's spans. | `prl_loader::validate_bvh_leaf_cells`, `validate_cell_draw_index` |
| Every windowed and capture frame records the depth prepass and the forward pass through one recorder. It does so after that frame's camera cull and under the same world-render gate. Level install rebuilds the camera cull, the candidate cull and the draw index together. Draw-all comes only from a world with no cells, which the loader rejects for a non-empty BVH, so it never draws geometry from a loaded PRL. | `Renderer::record_scene_passes`, `record_pre_scene_compute`, `install_level_geometry`; visibility `EmptyWorld`; `prl_loader::validate_bvh_leaf_cells` |
| Camera world draws and the pre-scene cull block share one gate: the cull block runs only under `render_world`. The depth prepass and forward world draw also require `has_geometry && index_count > 0`. | `record_pre_scene_compute` (`if render_world`); `record_depth_and_sdf_passes`; `record_scene_passes` forward block |
| Pre-scene compute can return `false` after the cull, when a streamed indirect SH compose fails, and the frame still records and draws. | `record_pre_scene_compute` tail; `record_scene_passes` (`compose_succeeded &=`) |
| Shadow depth passes record after pre-scene compute and before the depth prepass. | `record_scene_passes`: `record_spot_shadow_depth`, `record_cube_shadow_depth`, then `record_depth_and_sdf_passes` |
| Windowed and offscreen-capture frames both record through `record_scene_passes`. A failed surface acquire returns before it. | `render_frame_indirect`; `capture_frame_indirect`, `capture_measurement_frame_indirect` |
| Level install replaces `cell_draw_index`, `compute_cull`, `candidate_cull` and both shadow culls in one call. Release installs empty geometry, which leaves `compute_cull = None`. | `Renderer::install_level_geometry`, `release_level_resources` |
| The out-of-range check lives in the gather, so it runs only on candidate-eligible paths. It returns on the first bad id, and the caller discards the partial output. | `gather_candidate_leaves`; `record_pre_scene_compute` routing match |
| `draw_indirect_buckets` takes a concrete `wgpu::RenderPass`. The scanner allows indirect draw method calls only inside it, and it excludes `#[cfg(test)]` modules. | `compute_cull.rs`; `indirect_contract_tests.rs` `rust_violations`, inventory test |
| A GPU-backed offscreen renderer harness exists and skips when no adapter is present. Renderer tests install a per-thread counting allocator. | `render/uploads/renderer_tests.rs` `renderer()`; `renderer/src/lib.rs` `#[global_allocator]` |
| The stress probe table covers stress-warren, stress-warren-crates and campaign-test, at portal-path poses only. | `candidate_cull_probes::PROBES` |
| The candidate path clears the camera indirect range every frame. The tree walk leaves the slots under a frustum-rejected node as they were. | `CandidateCullPipeline::dispatch` (`clear_buffer`); `bvh_cull.wgsl::cull_main` skip |

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

## Coalescing rule (2026-10-03)

An independent second opinion read the wgpu 29 path. The figures are estimated from source, not measured.
- **Record side.** `render_pass_multi_draw_indexed_indirect` does `resolve_buffer_id` (registry read and `Arc` clone), then pushes the command.
- **Replay side.** `multi_draw_indirect` runs:
  - `State::is_ready`;
  - `flush_bindings_helper`, which is near-free when no bind group is dirty;
  - the usage, destroyed and overrun checks;
  - an init-tracker `create_action`;
  - `merge_single` buffer tracking;
  - then hal Metal `draw_indexed_indirect`.
- **Result.** About 100–250 ns fixed per call (~150 ns typical), against about 49 ns per slot. Break-even gap ≈ 2–5 slots.

Within a bucket, spans run in cell order, so a gap between two visible spans holds at least one invisible cell's leaves in that bucket. Gap-bridging at its derived threshold therefore almost never fires, and a larger threshold would be a per-backend tuning knob. Campaign-test under abutting-only merge: about 100 runs × 150 ns + 145 slots × 49 ns ≈ 22 µs per pass, against 774 × 49 ns ≈ 38 µs today.

Not verified: the exact cost of the lock wrapper, cache effects, and the GPU-side cost of an empty indirect draw. This adapter has no timestamp queries.

## Frame orderings

From `/review-brief` (2026-10-03). Acceptance rows cite these ids.

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| O1 | Visible set changes between frames | frame N set A, frame N+1 set B | N+1 draws ranges built from B only. No run from A survives. |
| O2 | Cull path switches | candidate frame, then a tree-walk frame (solid cell / exterior), then candidate again | Each frame draws ranges built from its own set. The path never feeds the ranges. |
| O3 | Empty after nonempty | frame N nonempty set, frame N+1 `Culled([])` | N+1 issues zero draws and no binds. It never reuses N's list and never falls back to whole buckets. |
| O4 | Zero spans, nonempty set | `Culled` names only cells that own no spans | Zero draws, no binds. Not whole buckets. |
| O5 | Out-of-range id after valid ids | `Culled([valid, …, ≥cell_count])` | Whole buckets that frame. The ranges built for the earlier valid ids are discarded, as the gather discards its partial `out`. |
| O6 | Out-of-range id on a tree-walk path | Solid-cell / exterior frame naming an id ≥ `cell_count` | Whole buckets that frame. The gather never runs on this path, so the range builder must detect the id itself. |
| O7 | Recovery after out-of-range | frame N out-of-range, frame N+1 all in range | N+1 draws spans. No fallback latches across frames. |
| O8 | Visible set out of slot order | `Culled([B, A])`, where A's span ends where B's begins, same bucket | One draw, as for `Culled([A, B])`. |
| O9 | Shadow passes between build and camera draws | pre-scene build → spot shadow → cube shadow → depth prepass → forward | Both camera passes draw the pre-scene list unchanged. Shadow draws never write it. |
| O10 | Depth prepass then forward, same frame | prepass reads the list, then forward reads it | Identical ranges. Nothing between them rebuilds the list. |
| O11 | Level reinstall between frames | level A frame with set S, install level B, level B frame with set S | B's frame draws ranges from B's index and bucket ranges, even when S is unchanged. |
| O12 | Draw-all and span mode across installs | empty-world level (`DrawAll`), then install a celled level | The celled level's first frame draws spans. Whole-bucket mode does not persist. |
| O13 | Pre-scene compose fails after the cull | the cull block runs, then the streamed indirect SH compose returns `false` | The frame still draws, from its own set's ranges. The build comes before the only early return. |
| O14 | Surface acquire fails | the frame is skipped before `record_scene_passes` | No build, no draw. The next recorded frame builds from its own set. |
| O15 | `render_world == false` | menu / frontend frame | No build, no camera draw. Both sit behind the same flag. |
| O16 | Capture vs windowed | an offscreen capture frame, then a windowed frame, same set | Same ranges. Both record through one scene-pass function. |
| O17 | Stale tree-walk slot after its cell leaves the set | tree-walk frame N writes leaf L nonzero; frame N+1's tree walk skips L's frustum-rejected node, and L's cell is not visible | L is no longer drawn. It was clipped before, so the image does not change. |
