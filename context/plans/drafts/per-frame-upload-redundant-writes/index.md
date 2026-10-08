# per-frame-upload-redundant-writes

Brief · draft · reads: `context/lib/rendering_pipeline.md` §1, §4 · follows `per-frame-upload-batching`

## Problem
Deferred uploads remove driver staging churn, but still copy unchanged or overwritten data. The light bridge writes sentinel shadow slots, then slot assignment rewrites the lights buffer almost every frame. Both writes now cost CPU copying and GPU copies. Start with this pair; do not assume other writers are redundant without measured evidence.

## Proposed direction
Measure writer bytes and copies on campaign-test and stress-warren-hallway-inspection under the batching brief's pinned conditions. Remove a write only when the final bytes and CPU commit mirrors stay identical. Consider composing bridge data and slot patches in retained CPU storage before one upload. Keep skipped-frame, failed-drain, capture and ordering semantics.

## Acceptance to refine
- GPU proofs compare final lights records, shadow-slot assignments and light counts with the current bridge-then-slot path, including no lights, changing slots, promotions and two consecutive acquire failures.
- A rejected upload never advances the bridge commit or lights mirror.
- Per-writer counters show the suppressed writes and bytes on both maps; record CPU timing before and after.
- Unchanged frames allocate no new renderer scratch after warmup.

## Evidence
Parent batching brief: `context/plans/in-progress/per-frame-upload-batching/research.md`, per-frame write inventory and P9/P12. Record the parent's completed measurements here before promotion.

Parent final 120-frame observations: campaign median 53.63 copies/batch and 354,640 bytes/frame; hallway 71 copies/batch and 567,496 bytes/frame. Both use one batch/frame, zero direct renderer writes and stable post-warmup buffer creation. Final render_record medians were 1.875 ms / 1.2645 ms respectively, qualified by thermal/memory state in the parent plan. Per-writer/mesh cost has not been measured in the windowed runs; this draft remains gated before promotion.
