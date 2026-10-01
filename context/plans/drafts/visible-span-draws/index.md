# visible-span-draws

Brief · compact · reads: `context/lib/rendering_pipeline.md` §2, §5, §7.1–7.3, §12 · `context/lib/build_pipeline.md` §PRL section IDs (id 37) · read at 832e20c8a

## Problem
Developer profiling on the compatibility-floor Mac (Radeon Pro 5300M, Metal) found that CPU encode cost grows with a map's total BVH leaf count, not with what the camera sees. The depth prepass and the forward pass each issue one multi-draw per material bucket over the bucket's whole slot range. Culled leaves stay in that range with a zeroed index count. The Metal backend turns a multi-draw of N into N driver draws, and wgpu-core validates each indirect draw, so every leaf costs one driver draw per camera pass whether it is visible or not. On stress-warren-hallway-inspection (8,437 leaves), indirect-draw driver calls and validation cost 1.79 ms per frame inside `render_submit`, against 0.16 ms on campaign-test (774 leaves). When this is done, the camera passes issue indirect draws only over the leaf spans of visible cells, per bucket. Encode cost tracks the visible set, and the rendered image is unchanged.

## Decisions
- **Camera passes draw visible spans, not whole buckets.** Each pass draws only the visible cells' spans, per bucket, coalescing abutting spans in the same bucket into one multi-draw. A bucket with no visible span gets no draw and no material bind. This diverges from `plans/done/perf-visible-cell-candidate-cull`, which kept the per-bucket call count unchanged. That rule counted wgpu calls. On Metal the cost is per driver draw, and a multi-draw of N is N driver draws, so cutting N saves work, while the added calls are bounded by the number of coalesced visible runs. The "one multi-draw per bucket" wording in `rendering_pipeline.md` §5 and §7.3 changes at promotion.
- **Draw ranges come from the frame's drawable visible set and the loaded draw index, never from the cull path that ran.** This keeps the §7.1 property that both camera-cull paths feed one draw path. It covers every concrete set: the portal walk, the step-limit fallback, and the solid-cell, exterior and no-portals fallbacks, which run the tree walk today. Fog reach never feeds draw ranges. It drives the fog cell mask and SH preparation, not world drawing.
- **The ranges cover every leaf the cull can submit, which keeps the §2 superset contract.** A superset set costs extra draws, never a missing surface. Where a set can't be mapped to spans, the passes draw whole buckets, as today. That happens on draw-all (no cell set), when a visible cell id is outside the loaded index (that frame already falls back to the tree walk), and when no draw index is loaded.
- **Both camera passes draw one shared range list per frame.** The forward pass tests depth `Equal` against the prepass. A leaf that one pass draws and the other doesn't renders as a hole or goes missing.
- **The GPU cull is unchanged.** It still zeroes the culled leaves inside visible spans. This brief only removes whole invisible cells from the draw range. Non-goal: GPU index compaction into a dense range. It needs a new pass and persistent GPU state, which takes a plan, not a brief. A compaction into a fixed-size range also saves no Metal draws, because `draw_indexed_indirect_count` is an unimplemented stub on Metal.
- **The shared contract is the `CellDrawIndex` span contract**, which `bvh-leaf-clustering` keeps. This brief relies only on these facts. A span's leaves are contiguous indirect slots. A span lies inside one material bucket's slot range. Every drawable leaf lies in exactly one span of its own cell. It does not rely on one span per (cell, bucket), on leaf granularity, or on how spans are ordered inside a bucket. Clustering shrinks the spans and span draws restrict to visible cells, so the two stack without coordination.
- **Layer: renderer, runtime.** A GPU-free data step turns (visible set, draw index, bucket ranges) into per-bucket draw ranges, and a thin draw layer issues them (`development_guide.md` §4.1). The PRL format, the bake, the visibility crate and the cull shaders are unchanged.
- **Hot-path bound.** Building the ranges costs on the order of the visible spans, not the total leaves. It allocates nothing per frame and runs once per frame, not once per pass (`development_guide.md` §1.4).
- **Non-goals.**
  - Shadow passes. Their cone cull treats every cell as visible, so there is no cell set to restrict the draws by. Revisit separately.
  - Turning off wgpu indirect validation. `release-indirect-validation` owns that, and validation stays on here.
  - Kinematic movers. They already use direct draws.

## Acceptance

### Automated
Draw ranges follow the visible set:
- [ ] Across both camera passes, indirect draws per pass equal the summed span lengths of the distinct visible cells. A cell named twice counts once.
- [ ] A visible set that leaves out some of a bucket's cells draws fewer slots in that bucket than the bucket holds.
- [ ] A set that names every cell draws every drawable leaf, with no more draws than the whole-bucket path.
- [ ] An empty visible set issues zero indirect draws and no material binds, without error.
- [ ] One visible cell draws exactly that cell's spans.
- [ ] A bucket with no visible span issues no draw and no material bind. A bucket with one visible span issues both.

Coalescing:
- [ ] Two abutting visible spans in one bucket go out as one draw call. Two non-abutting spans in one bucket go out as two. Two abutting spans that straddle a bucket boundary never merge.

Every visible-set path:
- [ ] Every leaf the camera cull submits lies inside a drawn range. This holds on synthetic worlds for the portal walk, the step-limit fallback, and the solid-cell, exterior and no-portals fallbacks, and in the on-demand stress-map probes.
- [ ] A frame whose fog-reach set differs from its drawable set draws the drawable set's spans exactly.
- [ ] A draw-all frame draws whole buckets. A cell set that names every cell takes the span path.
- [ ] A visible cell id past the loaded index draws whole buckets for that frame. A set whose largest id is the last valid cell takes the span path.
- [ ] With no draw index loaded, the camera passes draw whole buckets. With one loaded, they draw spans.

Pass agreement and regression guards:
- [ ] On every path above, the depth prepass and the forward pass issue identical ranges within a frame.
- [ ] Without multi-draw-indirect support, the per-draw fallback issues the same ranges one slot at a time.
- [ ] Shadow passes still draw whole buckets (regression guard).
- [ ] Rebuilding a frame's draw ranges for an unchanged visible set allocates nothing.

### Manual
- [ ] On this Mac, in a release build with the Auto preset and no tracer attached, record the `[CpuTiming]` `render_submit` and `work` medians on stress-warren-hallway-inspection and campaign-test, before and after. `render_submit` falls on stress-warren-hallway-inspection and does not rise on campaign-test. Record the numbers in the plan of record.
- [ ] A `sample` profile of render-pass encoding on both maps, before and after, shows the time in indirect-draw driver calls plus wgpu indirect validation falling from its 1.79 ms and 0.16 ms baselines (symbols in `research.md`). Report the after figures.
- [ ] Headless captures at fixed poses are byte-identical before and after: a portal-walk pose on each map, plus one solid-cell or exterior pose.
- [ ] No visual change in play on either map.

## Path
- Seams:
  - `gather_candidate_leaves` is the precedent for the CSR walk, the visible-cell dedupe and scratch reuse. It expands spans into single leaves, so the span grouping has to be read from the index directly.
  - `draw_indirect_buckets` is shared with `ShadowCullPipeline::draw_slot_indirect`. Keep it for shadows and add a range-list variant for the camera.
  - The camera callers are `ComputeCullPipeline::draw_indirect` from `renderer_shadow_passes.rs` (depth prepass) and `renderer_render_frame.rs` (forward).
  - `record_pre_scene_compute` is where the frame's visible set, path and index already meet.
- Gate: `visibility_path_uses_candidate_cull` picks the cull, not the draw ranges. Span draws apply to every concrete set.
- Shape:
  - Chosen: dedupe the visible cells and collect their spans, sort the spans by leaf start, and merge abutting spans in the same bucket. Then walk `bucket_ranges` with a cursor, binding the material once per bucket that holds runs.
  - Strongest rival: a per-leaf visibility bitmap scanned over all leaves. It is simpler to write, but it costs O(total leaves) per frame, which is the cost this brief removes.
- Proof:
  - `candidate_cull_mirror` is the CPU oracle to extend for coverage parity.
  - `candidate_cull_probes` holds the `#[ignore]` stress-map probes.
  - `RenderStage::Submit` is the `render_submit` stage.
- First slice: the pure range builder, plus the coverage-parity mirror test on a synthetic world with two buckets and an abutting pair. Then wire it into the forward pass and depth prepass on the portal path, and confirm the draw count on stress-warren-hallway-inspection.
- Oversized files:
  - `compute_cull.rs` runs past 1,200 lines, about 790 of them non-test. Put the range builder in its own module. If the span draw lands beside `draw_indirect_buckets`, first split the draw-issue functions out as their own behavior-preserving commit.
  - `renderer_render_frame.rs` is past 1,000 lines. Keep the additions to it at the call site.
- Comments to correct: the `candidate_cull.rs` header and the §7.1 text both say the draw path is byte-for-byte unchanged and fixed per map. After this change the draw ranges vary with the visible set, though they still don't depend on which cull path ran. Correct both.

## Open questions
None.
