# lighting-scale--sparse-layer-cache-and-fused-walk — plan of record

mode: resumable
status: approved
read at: a6ebb7938

## Corrections

- None. `d6e1c8b` is an ancestor of `a6ebb7938`, and `crates/level-compiler/src/`
  plus its tests have no changes between those revisions. The dependency work named by the
  brief is now on `main`; every cited Decision and Path symbol was re-read at `a6ebb7938`.

## Delegated answers

- Sparse record layout — use an interleaved native-endian Pod record
  `{ texel_index: u32, raw_visibility: f32 }`, with `target_layer` in the partition header.
  It preserves one-pass ordered fold/fill, keeps the promised 8-byte stride, and avoids a
  decode pass or delta-index CPU cost.
- Whether `ChunkLightList` moves above atlas preparation — yes. Its audited geometry reads
  are position-only, while its current whole-`GeometryResult` hash includes lightmap UVs;
  moving it into the pre-atlas group removes irrelevant density/scale-region invalidation
  without changing its inputs or bytes.
- Campaign-test reach fraction — measure and record it after Stride 1; do not use it to size
  the implementation. The worst-case 8/48 payload bound already proves the default-budget
  shape independently of the measured fraction.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| A1 Warm composite equals cold direct lightmap before BC6H on two real-stage fixtures, including one multi-layer, across cold, warm-empty, and warm-hit | New `pipeline`/extracted-lightmap-stage end-to-end fixture gate comparing decoded pre-BC6H outputs for all three paths | achievable as stated |
| A2 Uncompressed and BC6H encoded section modes remain byte-identical across the fold | Extend the A1 fixture matrix over both irradiance modes and compare `LightmapSection::to_bytes()` | achievable as stated |
| A3 Whole-file one-worker/many-worker determinism and shadowmask identity across cold, warm miss, partition miss, and section hit | Existing worker/shadowmask determinism gates plus a new real-pipeline whole-file matrix with explicit worker counts and cache states | achievable as stated |
| A4 Warm fallback with no layer-bearing lights emits one uncovered plane | Update and retain the all-SDF section-cache fallback gate at the extracted stage seam | achievable as stated |
| A5 Byte-identity reference is independent of the fused walk while sharing only the named leaf kernels | Frozen `lightmap_reference` module plus an automated source/dependency guard proving it cannot call the walk entry point | achievable as stated |
| A6 Frozen reference is compared with cold stage output on both A1 fixtures | Same two-fixture cold-stage-versus-reference gate used by A1 | achievable as stated |
| A7 Unreached texel is absent, contributes nothing, and is absent from shadowmask membership | Sparse codec/reconstruction unit test through baker predicate, fold sink, and shadowmask sink | achievable as stated |
| A8 Fully occluded texel is present with zero visibility, adds positive-zero terms, and writes channel byte zero | Sparse reconstruction/shadowmask edge test with a blocking trace | achievable as stated |
| A9 NaN visibility stays present and matches dense-fold bits and membership semantics | Direct sparse-record bit-pattern test through both sinks | achievable as stated |
| A10 Negative-zero weighted direction followed by an unreached light preserves dense-fold bits | Directional-light P4 regression test using baker math and ordered sparse fold | achievable as stated |
| A11 Values immediately below/above the coverage epsilon are absent/present through the baker's own predicate | Extend the shared-threshold test with adjacent representable `f32` values and sparse writer assertions | achievable as stated |
| A12 Named multi-layer fixture writes less than one tenth of the former dense layer bytes | Stage-cache test inventorying payload bytes written under `lightmap_layer` and comparing with `48 * covered_texels` | achievable as stated |
| A13 No-edit reads no layer; one-light edit re-bakes only edited partitions; unaffected reads become exactly once after fusion | Test-only per-key/stage access counters exercised by no-edit, selected-edit, and unselected-edit real-stage runs | achievable as stated |
| A14 Decodable out-of-covered-set, out-of-bounds, or non-increasing partitions soft-miss and re-bake | Replace exact-dense validation tests with sparse bounds/strict-monotonicity corruption matrix at the real cache caller | achievable as stated |
| A15 Pre-change layer and section entries are unreadable after both epoch bumps | Pin the incremented layer/section epochs and seed entries under the prior versions in a cache miss test | achievable as stated |
| A16 Over-budget live set emits exactly one warning naming read/written total and budget; under-budget/no-cache/release emit none | `postretro-test-log-capture` tests at the end-of-build reporting seam | achievable as stated |
| A17 Repeated touches and write-then-read count each cache key once in the warning live set | Unit tests for deduplicated cache-access accounting plus under-budget repeated-touch regression | achievable as stated |
| A18 A light reaching no texel writes and reuses an empty partition with no fold/channel effect | Multi-layer P3 real-cache regression with bake counter and output assertions | achievable as stated |
| A19 Every SH-block stage completes before atlas preparation; density/scale edits hit SH-block memos and miss lightmap memos | Reporter event-order test plus `cache_cross_bake_tests` density/scale edit matrix, including moved `ChunkLightList` | achievable as stated |
| A20 Cleared entity-shadow selection emits no shadowmask and fills no channel | Real-pipeline fixture forcing absent/unusable direct delta, with section bytes compared to the pre-fusion path | achievable as stated |
| A21 No layer cache read occurs after Lightmap Bake, cold or warm | Stage-aware cache access instrumentation asserted after the fused stage returns | achievable as stated |
| A22 Lightmap-section hit plus shadowmask miss reproduces cold bytes without rebake, reads each selected partition at most once, and performs no later reads | P1 integration matrix for selection-only rekey and deleted/corrupt shadowmask memo | achievable as stated |
| A23 Channel assignment completes before the first fused texel and writes only assigned channels, including two overlapping lights above layer zero | Extend the assignment barrier test and add a multi-layer fused-fill channel test | achievable as stated |
| A24 Lightmap and shadowmask each publish one accurate progress total without overshoot | Reporter/progress tests covering miss, hit, empty, dropped, and fused paths | achievable as stated |
| A25 Build Summary order changes intentionally and TUI sections flatten to the same order with Atlas Preparation under World | Update the exact `planned_stage_contract_pins_order_labels_and_sdf_prediction` vector and `section_table_is_a_contiguous_total_partition` expectations | achievable as stated |
| M1 Three-run campaign-test measurement after each stride records cache bytes, evictions, memo/group hits, and stage times | Integrating executor, scratch cache, default budget, empty/no-edit/one-light-edit runbook | manual-local |
| M2 Injected fused-walk defect fails the cold reference gate | Integrating executor, temporary local mutation followed by restoration and rerun | manual-local |
| M3 Post-fusion cold shadowmask costs graph plus encode with no retrace, and whole cold build is no slower than the recorded baseline | Integrating executor, verbose/timed campaign-test release run compared with research baseline | manual-local |
| M4 Windows owner runs stress-warren hallway release bake at density 0.04 and captures peak working set | Owner, Windows, external runbook; required before landing and also closes the prior brief's pending rows | manual-external-blocking |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Freeze the independent lightmap reference and extract the oversized lightmap, shadowmask, and pipeline stage responsibilities along behavior-preserving seams; add the two-fixture cold-stage-versus-reference gate before changing the writer | integrating executor | — | complete |
| 2 | Implement the sparse 8-byte partition codec, bounds/monotone validation, analytic reconstruction, both cache-epoch bumps, and the full byte/edge/size/end-to-end test matrix | integrating executor | 1 | complete |
| 3 | Add deduplicated per-build cache live-set accounting and the exactly-once over-budget warning; prove no-cache/release silence and complete the Stride-1 campaign measurement | integrating executor | 2 | complete |
| 4 | Add the Atlas Preparation stage, move the SH/delta/selection/billboard-scatter block and `ChunkLightList` before it, remove post-UV SH key churn, and update exact reporter/TUI order contracts | integrating executor | 3 | |
| 5 | Probe both lightmap and shadowmask memos before the walk, fuse cold lightmap, warm sparse writer/fold, and shadowmask fill into the shared per-chart walk, preserve the frozen reference, and prove P1/order/progress/no-late-read behavior | integrating executor | 4 | |
| 6 | Run focused readiness checks and both post-stride measurement runbooks, including the injected-defect and cold no-retrace checks; record every automated and local-manual result | integrating executor | 5 | |
| 7 | Run the required review-panel → fix-review-findings → focused-retest loop, then run `/preflight` once and prepare the Windows external runbook | integrating executor | 6 | |
| 8 | Receive the owner's Windows stress result, update durable build-pipeline contracts and the AC result column, move the brief to `done/`, and land | integrating executor + owner | 7 | |

## External runbook

Pending implementation. Before landing, the owner must run one `prl-build --release` compile of
`stress-warren-hallway-inspection.map` on Windows at lightmap density `0.04`, capture peak process
working set out of band, and report success/failure plus the peak. The exact command and expected
output artifact will be filled in after Task 7 against the final CLI surface.

## Execution log

### Task 1 — complete

- Extracted the frozen monolithic reference to `lightmap_bake/reference.rs`, cached-stage
  orchestration to `pipeline/lightmap_stage.rs`, and shadowmask fill mechanics to
  `shadowmask_bake/fill.rs` without changing production behavior.
- Added an automated source guard preventing the reference module from naming the future fused
  walk entry point.
- Extended the cold-stage/reference gate across single- and multi-layer fixtures and both
  uncompressed and BC6H section encodings.
- Proof: focused compiler check; oracle-isolation and worker-count determinism tests; the expanded
  `layered_cold_bake_matches_reference_and_repeats_byte_identically` gate; and the ignored real-map
  `lightmap_composite_equals_monolithic_on_fixtures` gate all pass.

### Task 2 — complete

- Replaced the 48-byte dense layer record with the interleaved 8-byte
  `{ texel_index, raw_visibility }` record and moved `target_layer` into the partition header.
- Sparse writers now omit analytically unreached texels while retaining zero and NaN visibility;
  warm folding reconstructs the exact unshadowed term without tracing, and derives coverage plus
  fallback normals from the prepared chart walk.
- Validation now accepts empty/sparse partitions and rejects wrong-layer, out-of-bounds,
  out-of-covered-set, duplicate, and non-increasing records as soft misses. Layer and section
  epochs are pinned at 6 and 3, with prior-epoch miss coverage.
- Proof: sparse codec/edge/size suites, all lightmap-layer tests, all shadowmask tests, the ignored
  real-map monolithic equivalence gate, and the complete 1,169-test `prl-build` suite pass. The
  named forced-multi-layer payload is below one tenth of its former dense-record bytes.

### Task 3 — complete

- `StageCache` clones now share a per-build map of successfully read/written cache digests and
  their on-disk byte sizes. Repeated touches and write-then-read paths therefore count each key
  once, including the entry header.
- The CLI reports that deduplicated live set after both successful and failed plain/TUI builds,
  but emits exactly one warning only when it exceeds the configured budget. Exact `--release`
  and explicit `--no-cache` paths have no cache handle and remain silent.
- Proof: compiler check plus the focused live-set deduplication, exact warning text/count,
  under-budget silence, and exact-build reporting-seam tests pass.
- Stride-1 campaign measurement (`campaign-test.map`, 4 workers, density 0.04, default 2 GiB
  budget, fresh scratch cache; light edit was entity 14 intensity 150 -> 151 and was restored):
  - empty cache: 7,415 entries, 391,312 KiB allocated (about 382 MiB), 0 evictions;
    `lightmap_section` 0 hit / 1 miss, `sh_group` 0 hit / 3,306 misses; Lightmap 27.60s,
    SH 74.96s, total 186.55s;
  - no edit: unchanged 7,415 entries and 391,312 KiB, 0 evictions;
    `lightmap_section` 1 hit / 0 misses, `sh_group` 3,306 hits / 0 misses; Lightmap 0.09s,
    SH 0.47s, total 4.51s;
  - one-light edit: 8,437 entries, 497,172 KiB allocated (492,946,862 apparent bytes),
    0 evictions; `lightmap_section` 0 hit / 1 miss, `sh_group` 2,292 hits / 1,014 misses;
    Lightmap 7.25s, SH 37.81s, total 51.64s.
