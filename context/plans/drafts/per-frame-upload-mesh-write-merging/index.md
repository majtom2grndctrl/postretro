# per-frame-upload-mesh-write-merging

Brief · draft · reads: `context/lib/rendering_pipeline.md` §1, §9 · follows `per-frame-upload-batching`

## Problem
Mesh planning interleaves instance and bone-palette writes to different targets. The frame batch deliberately preserves one copy per write. Grouping writes by target could reduce recording overhead if the measured mesh copy count justifies it.

## Gate
Use the batching brief's post-warmup 120-frame writer/copy counters on both maps. Record total copies, mesh copies and bytes alongside render_record and render_submit. Promote only if mesh copies are a substantial measured recording cost. A lower copy count alone does not justify extra machinery.

## Proposed direction
First evaluate contiguous instance/palette uploads at the mesh writer. Compare that with batch-level per-target gathering. Preserve same-range, overlapping and A/B/A order and every submit boundary; never move a write across a drain. Bound and reuse any gathering scratch.

## Acceptance to refine
- World-only, viewmodel-only, combined and neither cases match current GPU bytes and pixels.
- Same-target overlap and interleaved targets retain their last-write result; drain and capture boundaries remain exact.
- Measured mesh copies fall with no new steady-state renderer allocations or staging buffers.
- Record CPU timing and memory before and after on both maps; reject a net regression.

## Evidence
Parent batching brief: `context/plans/in-progress/per-frame-upload-batching/research.md`, mesh inventory and P4/P7/P10. Fill the measured copy gate from the parent's plan before promotion.

Parent final 120-frame observations: campaign median 53.63 copies/batch and 354,640 bytes/frame; hallway 71 copies/batch and 567,496 bytes/frame. Both use one batch/frame, zero direct renderer writes and stable post-warmup buffer creation. Final render_record medians were 1.875 ms / 1.2645 ms respectively, qualified by thermal/memory state in the parent plan. Per-writer/mesh cost has not been measured in the windowed runs; this draft remains gated before promotion.
