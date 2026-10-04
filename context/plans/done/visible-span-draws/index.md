# visible-span-draws

Brief · compact · reads: `context/lib/rendering_pipeline.md` §2, §5 (incl. Indirect-args invariant), §7.1–7.3, §12 (incl. Indirect-Call Validation) · `context/lib/build_pipeline.md` §PRL section IDs (id 37) · read at 4278d8789


Status: landed with gaps. Owner authorized landing on 2026-10-03 after the remaining manual requirements were reported. All 26 automated rows pass; the four manual rows remain unverified. See `plan.md` for results and the owner’s Mac observation.

## Problem
Developer profiling on the compatibility-floor Mac (Radeon Pro 5300M, Metal) found that CPU encode cost grows with a map's total BVH leaf count, not with what the camera sees. The depth prepass and the forward pass each issue one multi-draw per material bucket over the bucket's whole slot range. Culled leaves stay in that range with a zeroed index count. wgpu-hal's Metal backend turns a multi-draw of N into N `drawIndexedPrimitives` calls, so every leaf costs one driver draw per camera pass whether it is visible or not. With release indirect validation already off (`release-indirect-validation`), that per-draw hal cost is about 0.83 ms per frame inside `render_submit` on stress-warren-hallway-inspection (8,437 leaves, about 0.5% visible), and about 0.08 ms on campaign-test (774 leaves, about 18% visible). The figures come from reconciled profiles (`research.md` §Measurements). When this is done, the camera passes issue indirect draws only over the leaf spans of visible cells, per bucket. Encode cost tracks the visible set, and the rendered image is unchanged.

## Decisions
- **Camera passes draw visible spans, not whole buckets.** Each pass draws only the visible cells' spans, per bucket, coalescing abutting spans in a bucket into one multi-draw. A bucket with no visible span gets no draw and no material bind. This diverges from `plans/done/perf-visible-cell-candidate-cull`, which counted wgpu calls. On Metal a multi-draw of N is N driver draws, so the added calls, bounded by visible runs, cost less than the invisible slots they remove (`research.md` §Coalescing rule). The "one multi-draw per bucket" and "byte-for-byte identical draw path" wording in `rendering_pipeline.md` §5, §7.1 and §7.3 changes at promotion.
- **Coalesce only abutting spans. Never bridge a gap.** Gap-bridging is rejected. At its derived break-even it almost never fires, and a threshold large enough to fire is a per-backend tuning knob that loosens the exact draw-count rows (`research.md` §Coalescing rule).
- **Draw ranges come from the frame's drawable visible set and the loaded draw index, never from the cull path that ran.** This keeps the §7.1 property that both camera-cull paths feed one draw path. It covers every concrete set: the portal walk, the step-limit fallback, and the solid-cell, exterior and no-portals fallbacks, which run the tree walk today. Fog reach never feeds draw ranges. It drives the fog cell mask and SH preparation, not world drawing.
- **The ranges cover every leaf the cull can submit, which keeps the §2 superset contract.** A superset set costs extra draws, never a missing surface. Where a set can't be mapped to spans, the passes draw whole buckets, as today. That happens on draw-all (no cell set), when a visible cell id is outside the loaded index (that frame already falls back to the tree walk), and when no draw index is loaded.
- **Both camera passes draw one shared range list per frame.** The forward pass tests depth `Equal` against the prepass. A leaf that one pass draws and the other doesn't renders as a hole or goes missing.
- **The GPU cull is unchanged.** It still zeroes the culled leaves inside visible spans. This brief only removes whole invisible cells from the draw range. Non-goal: GPU index compaction into a dense range. It needs a new pass and persistent GPU state, which takes a plan, not a brief. A compaction into a fixed-size range also saves no Metal draws, because `draw_indexed_indirect_count` is an unimplemented stub on Metal.
- **The shared contract is the `CellDrawIndex` span contract**, which `bvh-leaf-clustering` keeps. This brief relies only on these facts. A span's leaves are contiguous indirect slots. A span lies inside one material bucket's slot range. Every drawable leaf lies in exactly one span of its own cell. It does not rely on one span per (cell, bucket), on leaf granularity, or on how spans are ordered inside a bucket. Clustering shrinks the spans and span draws restrict to visible cells, so the two stack without coordination.
- **Layer: renderer, runtime.** A GPU-free data step turns (visible set, draw index, bucket ranges) into per-bucket draw ranges, and a thin draw layer issues them (`development_guide.md` §4.1). The PRL format, the bake, the visibility crate and the cull shaders are unchanged.
- **Hot-path bound.** Building the ranges costs on the order of the visible spans, not the total leaves. It allocates nothing per frame and runs once per frame, not once per pass (`development_guide.md` §1.4).
- **The indirect-args invariant (§5) holds unchanged.** No indirect arguments change. Every draw offset comes from a load-validated span or a bucket range, and every draw binds the whole checked index array. The owner rules and inventory counts in `indirect_contract_tests` do not change. If a change seems to need them changed, that is a release-safety review point, not a mechanical update.
- **The draw layer executes a pure draw plan.** A GPU-free plan, built from the range list, holds the ordered binds and draws, including the per-slot expansion used without multi-draw-indirect. `draw_indirect_buckets` executes it one to one, so the tests that count draws and binds run against the plan without a GPU. The plan lives in `compute_cull.rs` beside `draw_indirect_buckets`, so the scanner stays unchanged.
- **Non-goals.**
  - Shadow passes. Their cone cull marks every cell visible, so restricting their draws needs a new cone-against-cell test, which is separate cull work. The world-depth cache keeps static lights that fit in it (3 spot and 4 cube units) from refilling. A moving shadow light, or one past capacity, still draws whole buckets every frame. One cube light on the hallway map is about 6·L slots, roughly 2.5 ms. That cost is deferred to its own draft, `shadow-span-draws`, not dismissed.
  - Other backends. The cost argument and the timing proof come from Metal. On Vulkan and DX12 a multi-draw is one native call, so span draws add about one wgpu call per visible run and save little CPU there. That cost is bounded by the visible runs and is not measured here.
  - Kinematic movers. They already use direct draws.

## Acceptance

Ordering ids (O1–O17) refer to `research.md` §Frame orderings.

### Automated
Draw ranges follow the visible set:
- [x] Across both camera passes, indirect draws per pass equal the summed span lengths of the distinct visible cells. A cell named twice counts once.
- [x] A visible set that leaves out some of a bucket's cells draws fewer slots in that bucket than the bucket holds.
- [x] A set that names every cell draws every drawable leaf, with no more draws than the whole-bucket path.
- [x] An empty visible set, or one whose cells own no spans, issues zero indirect draws and no material binds, without error, including on the frame after a nonempty set. Neither falls back to whole buckets (O3, O4).
- [x] One visible cell draws exactly that cell's spans.
- [x] A bucket with no visible span issues no draw and no material bind. A bucket with one visible span issues a draw in both camera passes and a material bind in the forward pass only.
- [x] On a headless frame over a level whose visible set leaves out some cells, both camera passes draw the list built from that set, and it holds fewer slots than the level's buckets. The same frame with a draw-all set draws whole buckets.
- [x] After a level install, the first frame draws ranges built from the new level's draw index and bucket ranges, even when its visible set equals the previous level's last set. A draw-all level followed by a celled level draws spans on the celled level's first frame (O11, O12).
- [x] Building the ranges reads only the visible cells' spans and walks each bucket range at most once. On a synthetic world with many cells and one visible cell, it touches no span or leaf of any other cell.

Coalescing:
- [x] Two abutting visible spans in one bucket go out as one draw call, whichever order the visible set names their cells. Two spans in one bucket separated by a gap of even one slot go out as two. Two abutting spans that straddle a bucket boundary never merge (O8).
- [x] A bucket with K maximal visible runs issues exactly K draw calls and one forward material bind. The per-draw fallback also binds once per bucket.
- [x] No drawn range includes a slot outside a visible span.

Every visible-set path:
- [x] Every leaf the camera cull submits lies inside a drawn range. This holds on synthetic worlds for the portal walk, the step-limit fallback, and the solid-cell, exterior and no-portals fallbacks, and in the on-demand stress-map probes.
- [x] The on-demand stress-map probes include stress-warren-hallway-inspection. At each probe pose, campaign-test included, every submitted leaf lies inside a drawn range, and the slots drawn per camera pass equal the visible cells' summed span lengths. The probe reports the coalesced runs and the drawn slots per camera pass, and the plan of record records both beside the map's total leaves.
- [x] On a headless frame given a fog-reach set that differs from its drawable set, the camera passes draw the drawable set's spans exactly.
- [x] A draw-all frame draws whole buckets. A cell set that names every cell takes the span path.
- [x] A visible cell id past the loaded index draws whole buckets for that frame, on the portal path and on the solid-cell and exterior paths alike, and whether the bad id comes before or after valid ones. The next frame whose ids are all in range draws spans. A set whose largest id is the last valid cell takes the span path (O5, O6, O7).
- [x] With no draw index loaded, the camera passes draw whole buckets. With one loaded, they draw spans.

Pass agreement and regression guards:
- [x] On every path above, the depth prepass and the forward pass issue identical ranges within a frame (O10).
- [x] A recorded frame builds its draw ranges once, before the depth prepass. Neither camera pass rebuilds them.
- [x] Shadow depth passes recorded between the range build and the camera passes leave the camera list unchanged. On a frame with an occupied shadow slot, the depth prepass and the forward pass draw exactly the list built from the frame's visible set (O9).
- [x] A frame draws the ranges built from its own visible set. Consecutive frames with different visible sets, on the candidate path, the tree-walk path, and switching between them, never draw the previous frame's ranges. Consecutive frames with the same set draw the same ranges. Path switching is proved on consecutive headless frames that alternate a portal path and a solid-cell path (O1, O2).
- [x] Without multi-draw-indirect support, the per-draw fallback issues the same ranges one slot at a time.
- [x] Shadow passes still draw whole buckets (regression guard).
- [x] Once the builder has handled a visible set at least as large, rebuilding the ranges for any set, changed or unchanged, allocates nothing.
- [x] The indirect-contract scanner passes, and its owner rules and inventory counts are unchanged from the read revision. This is a diff gate on `indirect_contract_tests.rs`, not a behavior test.

### Manual
- [ ] On this Mac, in a release build with the Auto preset, indirect validation at its release default (off, `WGPU_VALIDATION_INDIRECT_CALL` unset), the window in front and no tracer attached, record the `[CpuTiming]` `render_submit` and `work` medians on stress-warren-hallway-inspection and campaign-test, before and after. Use at least five windows under the same recorded shadow-cache state, as in `release-indirect-validation`. `render_submit` falls on stress-warren-hallway-inspection and does not rise on campaign-test. Record the numbers in the plan of record.
- [ ] A `sample` profile of render-pass encoding on both maps, before and after on the same build base, shows the time in wgpu-hal Metal `draw_indexed_indirect` (the `drawIndexedPrimitives` loop) falling. Report the before and after figures. At 4278d8789 the reconciled baselines were about 0.83 ms and 0.08 ms.
- [ ] Headless captures at fixed poses are byte-identical before and after: a portal-walk pose on each map, plus one solid-cell or exterior pose.
- [ ] No visual change in play on either map.

## Path
- Seams:
  - `gather_candidate_leaves` is the precedent for the CSR walk, the visible-cell dedupe and scratch reuse. It expands spans into single leaves, so the span grouping has to be read from the index directly.
  - `draw_indirect_buckets` is shared with `ShadowCullPipeline::draw_slot_indirect`, and `indirect_contract_tests` allows no other function to call the hal draw. Generalize it to take a list of draw ranges grouped by bucket, rather than adding a second draw function. Shadows pass their whole buckets as that list.
  - The camera callers are `ComputeCullPipeline::draw_indirect` from `renderer_shadow_passes.rs` (depth prepass) and `renderer_render_frame.rs` (forward). The scanner pins both call sites, with the whole index buffer bound.
  - `record_pre_scene_compute` is where the frame's visible set, path and index meet. Build the range list there, once per frame, before the cull-path `match`, so every arm sees it. Do not build it inside `ComputeCullPipeline::dispatch`: that function runs only on the tree-walk arm, and the candidate arm calls `CandidateCullPipeline::dispatch` instead. Storing the list on the compute cull pipeline leaves `draw_indirect`'s signature and call sites unchanged, and gives both camera passes one list by construction.
- Gate: `visibility_path_uses_candidate_cull` picks the cull, not the draw ranges. Span draws apply to every concrete set.
- Shape:
  - Chosen: dedupe the visible cells and collect their spans, sort the spans by leaf start, and merge abutting spans in the same bucket. Then walk `bucket_ranges` with a cursor, binding the material once per bucket that holds runs.
  - Strongest rival: gap-bridging (Decisions). A per-leaf visibility bitmap is only a variant of the chosen shape, at O(total leaves) per frame.
  - One named predicate decides the merge.
- Proof:
  - `candidate_cull_mirror` is the CPU oracle to extend for coverage parity.
  - `candidate_cull_probes` holds the `#[ignore]` stress-map probes.
  - `RenderStage::Submit` is the `render_submit` stage.
- First slice: the pure range builder, plus the coverage-parity mirror test on a synthetic world with two buckets and an abutting pair. Then wire it into the forward pass and depth prepass on the portal path, and confirm the draw count on stress-warren-hallway-inspection.
- Oversized files:
  - `compute_cull.rs` runs past 1,200 lines. Put the range builder in its own module. Keep `draw_indirect_buckets` in `compute_cull.rs`, because the scanner names its owner by file. Moving it means changing the scanner's owner rule in the same commit, under the review point above.
  - `renderer_render_frame.rs` is past 1,000 lines. Keep the additions to it at the call site.
- Comments to correct: the `candidate_cull.rs` header says the draw path is byte-for-byte unchanged and fixed per map. After this change the draw ranges vary with the visible set, though they still don't depend on which cull path ran. Correct it to match the §7.1 text updated at promotion.

## Open questions
None.
