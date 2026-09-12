# lighting-scale--shadowmask-cold-working-set — plan of record

mode: resumable
status: approved
read at: 438a84925

## Corrections

- The Path says to reuse `ReachIndex` inversion directly → current
  `direct_sh_bake::ReachIndex` also owns SH-probe coordinate mapping and is
  not reusable as the chart-prune index. The affinity portal filter classifies
  cell centroids, so it also cannot prove that rejecting a whole chart is a
  coverage superset at portal/leaf boundaries. The implemented prune therefore
  reuses the authoritative `affinity_grid::light_aabb` against each chart's
  world AABB; the exact shared texel predicate remains the final authority.
- The brief schedules the id-42 graph-versus-coloring clarification for
  promotion → `build_pipeline.md` already contains that clarification and marks
  the analytic pass as not yet built; landing removes the marker and updates the
  cache and distribution rationale to match the implemented lifecycle.

No compiler source cited by Decisions or Path changed between the brief's
`6168c5c9` read and current source; the only relevant intervening change was the
two-line promoted-context update above.

## Delegated answers

- Resident-partition constant — begin with today's bound of 4 for any retained
  cold chart-layer batching; the layer-outer fill itself holds one partition at
  a time. Change 4 only if the measured implementation requires it, and record
  that evidence here before landing.
- Reach fraction — at density 0.04 on
  `stress-warren-hallway-inspection-mini.map`, the analytic graph prune kept
  52,994 / 647,566 light/chart pairs (8.18%) across 166 selected lights and
  3,901 charts. This was measured by the explicit ignored graph-only fixture
  test; it does not bake rays or write stage-cache entries. The full stress map
  remains reserved for the owner-run manual gate.
- Supported-density margin — no more than 10% slower in the ShadowmaskAtlas
  stage at density 0.16, comparing the same release build, map, machine, and
  worker setting. Record both timings; treat larger regression as a failed
  manual row rather than averaging it away.
- Owner test coordination — when the branch is ready for the 16 GiB Windows
  stress bake, push it to the remote and hand off the exact command; do not ask
  the owner to reproduce an intermediate checkpoint.
- Local bake policy — do not run incremental/warm map bakes on this MacBook;
  their stage cache floods local disk. Local verification uses focused unit
  tests and `cargo check`, and any necessary map compile uses `--no-cache` with
  a small fixture and explicit output cleanup.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| Analytic coverage equals baked coverage across fixtures/golden, including both sides of the shared threshold, occluded zero, beyond-falloff, degenerate charts, and NaN | `analytic_coverage_matches_baked_coverage_fixture_matrix` plus the existing top-level golden | passed in tasks 1 and 3 |
| Every prune rejection has no analytically covered chart texel | `shadowmask_chart_prune_is_coverage_superset` | passed |
| A fully pruned selected light keeps its node, channel, bytes, and later channel ordering | `pruned_zero_coverage_light_keeps_node_and_channel_table` | passed |
| No light-and-texel structure remains; sole light-scaling allocation is `n*n` bytes (114,244 bytes at 338 lights) | `shadowmask_graph_storage_is_one_byte_per_light_pair`; source review confirms deleted membership/index types | passed — membership/index types deleted |
| Light-scaling residency is independent of atlas layers at fixed plane/light count, excluding the output | `shadowmask_graph_storage_is_layer_count_independent` | passed — adjacency depends only on light count; partition residency remains bounded by W |
| Exactly one full-size output allocation; conversion is in-place or absent | grep/review of the fill path; add a counting-allocator test only if review finds an ambiguous second allocation | passed — direct serial `Vec<u8>` fill, no conversion |
| Output allocation occurs exactly once on cold, cached miss, and wholly filtered cached paths | `shadowmask_output_allocates_once_on_every_fill_path` using test-only allocation instrumentation at the allocation seam | passed |
| Coloring runs once after every graph item joins | `shadowmask_coloring_waits_for_complete_graph` | passed |
| Pause/resume and mid-pass `-j` lowering preserve bytes without preemption/deadlock | `shadowmask_graph_pause_and_permit_retarget_preserve_output` | passed |
| Progress advances during graph construction | `shadowmask_progress_advances_during_graph_pass` | passed |
| Cross-layer shared texel refuses one channel; disjoint lights may share one | `analytic_graph_respects_cross_layer_overlap_and_disjoint_reuse` via synthetic coverage seam | passed |
| Reversed graph work order and worker count preserve adjacency/channel table | `analytic_graph_is_order_and_worker_count_independent` | passed |
| Cold, cold cache miss, warm partition miss, section hit, and analytic route are byte-identical | `top_level_cached_and_analytic_paths_match_multilayer_five_way_golden` | passed |
| Zero selection, wholly filtered selection, out-of-range slot, one-layer literal, and two-layer golden stay unchanged | focused degenerate-path tests plus existing multilayer golden | achievable as stated |
| Unchanged rebuild hits section memo; one-light change misses it, re-runs graph without cache reads, and reuses other partitions | `one_light_change_reruns_graph_and_reuses_unchanged_partitions` | achievable as stated |
| Shadowmask stage epoch and lightmap-layer key remain unchanged; pre-restructure cache reads without re-bake | constants asserted at existing values plus `pre_analytic_layer_cache_is_reused_without_rebake` fixture bytes captured before restructuring | achievable as stated |
| Coloring-dropped partition behavior is stated and next-run hit rate matches it | `dropped_light_partitions_are_cached_then_hit_next_compile`; rule: populate every selected partition, including globally dropped lights, so a later assignment change can reuse it | passed |
| Mixed warm/baked partitions reach published total exactly once with no overshoot | `mixed_layer_cache_hits_and_misses_complete_shadowmask_progress` updated for layer-outer fill | passed |
| Progress stays short until finalization and section memo write on cached and uncached paths | `shadowmask_final_progress_unit_follows_section_memo_write` | achievable as stated |
| Stage publishes one determinate total and completes only with the section | `shadowmask_publishes_one_total_and_completes_with_section` | achievable as stated |
| Two unchanged compiles at one and many workers emit identical id-42 bytes on a named fixture | `shadowmask_fixture_is_deterministic_across_rebuilds_and_workers` using `stress-warren-hallway-inspection-mini.map` or a smaller named id-42 fixture if its runtime is lower | achievable as stated |
| Density 0.04 full stress compile completes on owner's 16 GiB Windows machine with peak RSS | owner, out-of-band RSS and elapsed-time capture | manual-stress |
| Density 0.04 bytes match a headroom reference build when available | owner/reference machine; record not run if unavailable | manual-conditional |
| Supported-density stress-map bytes stay unchanged and ShadowmaskAtlas time regresses no more than 10% | owner or integrating executor, density 0.16 before/after release timing on the same machine/settings | manual-performance |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Add the shared coverage predicate and coverage-only chart walk; prove analytic/baked equivalence across the fixture matrix before changing graph construction | integrating executor | — | done — `analytic_coverage` (2), threshold (1), degenerate walk (1), NaN inclusion (1) |
| 2 | Split the 3,600+ line `shadowmask_bake.rs` by responsibility without behavior changes; run existing shadowmask tests and commit the split alone | integrating executor | 1 | done — assignment module extracted; `shadowmask_bake::tests` 45 passed |
| 3 | Build the pruned parallel analytic graph pass from shared affinity reach data, preserving every selected node; measure and record the mini-warren reach fraction, barrier, ordering, pause, worker-count, progress, and adjacency-storage proofs | integrating executor | 2 | done — 54 shadowmask tests passed; mini-warren kept 8.18% of pairs |
| 4 | Replace membership assembly with layer-outer partition fill into one output allocation on cold and warm section-miss paths; preserve cache keys/epochs, define dropped-partition population, and pin filtered-selection allocation and final progress ordering | integrating executor | 3 | done — 55 active shadowmask tests passed; one allocation and dropped-partition warm-hit proofs added |
| 5 | Complete lifecycle, degeneracy, determinism, cache, golden-byte, and allocation review gates; run focused compiler tests after each seam and confirm every filter executes tests | integrating executor | 4 | pending |
| 6 | Run preflight, review panel, fix/retest loops, update durable build-pipeline contracts, populate AC results, move the brief to `done/`, and commit the landing | integrating executor | 5 | pending |

## Resume notes

- Decisions and Acceptance are unchanged. A failed equivalence gate blocks this
  plan and returns the bounded-membership fallback to the owner.
- Shared contracts, `plan.md`, Cargo verification, commits, and final integration
  remain with the integrating executor.
- Owner approved this plan with remote Windows stress testing at the test-ready
  checkpoint and no incremental/warm bakes on the MacBook.
- Do not bump `SHADOWMASK_ATLAS_STAGE_VERSION` or
  `lightmap_layer::LAYER_FORMAT_VERSION`; graph discovery is not a byte or layer
  computation change.
