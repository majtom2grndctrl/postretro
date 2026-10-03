# visible-span-draws — plan of record

mode: compact
status: active
read at: 1b1c1caeb

## Corrections
- `record_pre_scene_compute` now lives in `render/renderer_pre_scene.rs`, not `renderer_render_frame.rs`; build the shared camera ranges at that actual pre-scene seam.
- No crate source changed from the brief read revision 4278d8789 through the claim commit; grounded Decision reads remain valid.
- Acceptance rows below are numbered in their original order for unambiguous proof tracking. GPU-backed frame tests use the existing offscreen adapter harness and report adapter execution explicitly.

## Delegated answers
- None; the brief has no open questions.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| 1. Across both camera passes, indirect draws per pass equal the summed span lengths of the distinct visible cells. A cell named twice counts once. | range-builder and pure draw-plan tests | achievable as stated |
| 2. A visible set that leaves out some of a bucket's cells draws fewer slots in that bucket than the bucket holds. | range-builder and pure draw-plan tests | achievable as stated |
| 3. A set that names every cell draws every drawable leaf, with no more draws than the whole-bucket path. | range-builder and pure draw-plan tests | achievable as stated |
| 4. An empty visible set, or one whose cells own no spans, issues zero indirect draws and no material binds, without error, including on the frame after a nonempty set. Neither falls back to whole buckets (O3, O4). | range-builder and pure draw-plan tests | achievable as stated |
| 5. One visible cell draws exactly that cell's spans. | range-builder and pure draw-plan tests | achievable as stated |
| 6. A bucket with no visible span issues no draw and no material bind. A bucket with one visible span issues a draw in both camera passes and a material bind in the forward pass only. | range-builder and pure draw-plan tests | achievable as stated |
| 7. On a headless frame over a level whose visible set leaves out some cells, both camera passes draw the list built from that set, and it holds fewer slots than the level's buckets. The same frame with a draw-all set draws whole buckets. | headless renderer frame integration tests and draw-plan trace | achievable as stated |
| 8. After a level install, the first frame draws ranges built from the new level's draw index and bucket ranges, even when its visible set equals the previous level's last set. A draw-all level followed by a celled level draws spans on the celled level's first frame (O11, O12). | headless renderer frame integration tests and draw-plan trace | achievable as stated |
| 9. Building the ranges reads only the visible cells' spans and walks each bucket range at most once. On a synthetic world with many cells and one visible cell, it touches no span or leaf of any other cell. | range-builder and pure draw-plan tests | achievable as stated |
| 10. Two abutting visible spans in one bucket go out as one draw call, whichever order the visible set names their cells. Two spans in one bucket separated by a gap of even one slot go out as two. Two abutting spans that straddle a bucket boundary never merge (O8). | range-builder and pure draw-plan tests | achievable as stated |
| 11. A bucket with K maximal visible runs issues exactly K draw calls and one forward material bind. The per-draw fallback also binds once per bucket. | range-builder and pure draw-plan tests | achievable as stated |
| 12. No drawn range includes a slot outside a visible span. | range-builder and pure draw-plan tests | achievable as stated |
| 13. Every leaf the camera cull submits lies inside a drawn range. This holds on synthetic worlds for the portal walk, the step-limit fallback, and the solid-cell, exterior and no-portals fallbacks, and in the on-demand stress-map probes. | candidate_cull_mirror and ignored stress_map_probes | achievable as stated |
| 14. The on-demand stress-map probes include stress-warren-hallway-inspection. At each probe pose, campaign-test included, every submitted leaf lies inside a drawn range, and the slots drawn per camera pass equal the visible cells' summed span lengths. The probe reports the coalesced runs and the drawn slots per camera pass, and the plan of record records both beside the map's total leaves. | candidate_cull_mirror and ignored stress_map_probes | achievable as stated |
| 15. On a headless frame given a fog-reach set that differs from its drawable set, the camera passes draw the drawable set's spans exactly. | headless renderer frame integration tests and draw-plan trace | achievable as stated |
| 16. A draw-all frame draws whole buckets. A cell set that names every cell takes the span path. | range-builder and pure draw-plan tests | achievable as stated |
| 17. A visible cell id past the loaded index draws whole buckets for that frame, on the portal path and on the solid-cell and exterior paths alike, and whether the bad id comes before or after valid ones. The next frame whose ids are all in range draws spans. A set whose largest id is the last valid cell takes the span path (O5, O6, O7). | range-builder and pure draw-plan tests | achievable as stated |
| 18. With no draw index loaded, the camera passes draw whole buckets. With one loaded, they draw spans. | range-builder and pure draw-plan tests | achievable as stated |
| 19. On every path above, the depth prepass and the forward pass issue identical ranges within a frame (O10). | headless renderer frame integration tests and draw-plan trace | achievable as stated |
| 20. A recorded frame builds its draw ranges once, before the depth prepass. Neither camera pass rebuilds them. | headless renderer frame integration tests and draw-plan trace | achievable as stated |
| 21. Shadow depth passes recorded between the range build and the camera passes leave the camera list unchanged. On a frame with an occupied shadow slot, the depth prepass and the forward pass draw exactly the list built from the frame's visible set (O9). | headless renderer frame integration tests and draw-plan trace | achievable as stated |
| 22. A frame draws the ranges built from its own visible set. Consecutive frames with different visible sets, on the candidate path, the tree-walk path, and switching between them, never draw the previous frame's ranges. Consecutive frames with the same set draw the same ranges. Path switching is proved on consecutive headless frames that alternate a portal path and a solid-cell path (O1, O2). | headless renderer frame integration tests and draw-plan trace | achievable as stated |
| 23. Without multi-draw-indirect support, the per-draw fallback issues the same ranges one slot at a time. | range-builder and pure draw-plan tests | achievable as stated |
| 24. Shadow passes still draw whole buckets (regression guard). | range-builder and pure draw-plan tests | achievable as stated |
| 25. Once the builder has handled a visible set at least as large, rebuilding the ranges for any set, changed or unchanged, allocates nothing. | range-builder and pure draw-plan tests | achievable as stated |
| 26. The indirect-contract scanner passes, and its owner rules and inventory counts are unchanged from the read revision. This is a diff gate on `indirect_contract_tests.rs`, not a behavior test. | indirect_contract_tests plus unchanged-file diff gate | achievable as stated |
| 27. On this Mac, in a release build with the Auto preset, indirect validation at its release default (off, `WGPU_VALIDATION_INDIRECT_CALL` unset), the window in front and no tracer attached, record the `[CpuTiming]` `render_submit` and `work` medians on stress-warren-hallway-inspection and campaign-test, before and after. Use at least five windows under the same recorded shadow-cache state, as in `release-indirect-validation`. `render_submit` falls on stress-warren-hallway-inspection and does not rise on campaign-test. Record the numbers in the plan of record. | owner, release in-engine runbook | manual proof blocks landing |
| 28. A `sample` profile of render-pass encoding on both maps, before and after on the same build base, shows the time in wgpu-hal Metal `draw_indexed_indirect` (the `drawIndexedPrimitives` loop) falling. Report the before and after figures. At 4278d8789 the reconciled baselines were about 0.83 ms and 0.08 ms. | owner, release in-engine runbook | manual proof blocks landing |
| 29. Headless captures at fixed poses are byte-identical before and after: a portal-walk pose on each map, plus one solid-cell or exterior pose. | owner, release in-engine runbook | manual proof blocks landing |
| 30. No visual change in play on either map. | owner, release in-engine runbook | manual proof blocks landing |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Pure visible-span builder, two-bucket coverage slice, reusable scratch and bounds proofs | bounded builder worker + integrating executor | — | pending |
| 2 | Shared camera range lifecycle and pure draw plan; preserve whole-bucket shadows | integrating executor | 1 | pending |
| 3 | Synthetic cull coverage and stress-map probes with counts | bounded probe worker | 1 | pending |
| 4 | Headless frame ordering, path switches, reinstall and shadow interleave proofs | integrating executor | 2 | pending |
| 5 | Focused readiness, review/fix loop, final preflight and AC results | integrating executor | 3, 4 | pending |

## Manual runbook

Manual rows block landing. At the same fixed poses on campaign-test and stress-warren-hallway-inspection, use release Auto, WGPU_VALIDATION_INDIRECT_CALL unset, window foreground and no tracer during timing. Record at least five complete 120-frame CpuTiming windows with the same recorded shadow-cache state, report medians of render_submit and work for baseline 1b1c1caeb and feature. Separately record sample profiles on each build/map and report the Metal draw_indexed_indirect time. Compare byte-identical headless captures at both portal poses plus a solid/exterior pose. Owner plays both maps and confirms no visual change. Exact commands and measured probe counts will be added after implementation.
