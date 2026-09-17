# lighting-scale--adaptive-probe-spacing — plan of record

mode: resumable
status: approved
read at: c269dd906

## Corrections

- The brief's Phase 1 layer map says `run_sh_analysis` already runs at the dense classifier window. Current `pipeline.rs` invokes it after valid-probe compaction and payload-cap enforcement, while retaining dense base snapshots and, when requested, dense post-drop delta snapshots. Keep the byte-preserving projection in that existing finalized-analysis call; keep the emission classifier inside `classify_base_levels`, before compaction, where `DeltaView` still consumes dense payloads.
- The forward texture-budget test is at `crates/renderer/src/render/tests/pipeline_budget_tests.rs`, not directly under `src/render/`; extend that existing test module.
- The Quake light translator is at `crates/level-compiler/src/format/quake_map.rs`. Normalize authored animated directional lights there, where `style`, `*_curve`, and `_animated` are still distinguishable from later data-script membership.
- No cited lighting source changed between the brief's grounded revision `2441b87` and current `main` `c269dd906`. Current format baselines are id 34 v10, id 35 v3, and delta ids 27/41/45 at v6/v4/v4; the planned v11/v4 break remains correctly scoped.
- Owner-approved measurement split: use `content/dev/maps/stress-warren-hallway-inspection-mini.map` for local iteration and recorded local findings, then leave `content/dev/maps/stress-warren-hallway-inspection.map` as the explicit Windows stress/visual runbook. This replaces the local full-warren attempt and the three-map local matrix without changing the measurement fields or quality contract.

## Delegated answers

- Maximum emitted node scale — pending Task 2 measurement; choose the highest scale up to 3 that has measured participation without violating the exact gate, and record the fixture histogram rather than assuming the wire maximum is the production operating point.
- 2:1 scale balance — pending Task 2 seam metrics; add the rule only if cross-scale residuals identify a seam not bounded by the existing level-smoothing rule.
- Phase 3 build decision — pending Task 2 per-resource attribution; build L1 nodes and the scale-aware sampler on any scarce-resource win or only-abundant-resource cost, and refuse only a clear unoffset loss in a constrained resource, exactly as the brief delegates.
- Stress-map feasibility — local proof uses `stress-warren-hallway-inspection-mini.map`; the owner will run `stress-warren-hallway-inspection.map` on Windows from the Task 10 runbook, with cold-cache intent, worker count, machine class, wall time, peak RSS when available, and cleanup recorded.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| P1-A1 analyzer is byte-preserving | Focused compiler integration test comparing emitted fixture `.prl` bytes with and without `--sh-analyze` | passed — `sh_analysis_is_byte_preserving_for_compiled_prl` |
| P1-A2 one-scale merge eligibility, blockers, smoothing, and max-scale-0 parity | Constructed hierarchy-classifier unit tests over aligned bricks, delta/protection/partial/misaligned blockers, adjacency, and shipped histogram parity | passed — four `sh_hierarchy::tests` plus analyzer max-scale-0 parity assertion |
| P1-A3 recursive scale-2 merge and blockers | `R-DIR2` constructed hierarchy unit tests over eight pre-merged scale-1 nodes | passed — `recursive_scale_two_merge_requires_all_scale_one_children` |
| P1-A4 recursive no-merge re-smoothing without level promotion | `R-DIR2` no-merge unit test asserting adjacency bound and monotone levels after the k=2 pass | passed — `recursive_no_merge_keeps_face_level_bound_without_raising_levels` |
| P1-A5 `--sh-density-force-scale` parses 0..3 only and has no author/player surface | CLI parser tests plus negative `rg` gate over FGD and player-options schemas | passed — parser range test and compile-time negative existence test |
| P2-A1 id 34 v11/id 35 v4 round-trip and named hierarchy rejects | Format unit tests for scale range, node agreement/alignment/containment/partial rules, L0 scale, L1 corner validity, and delta scale ceiling through the shared validator | achievable as stated |
| P2-A2 stale v10/v3 sections hard-fail with recompile errors | Version-reject unit tests for both section decoders and loader propagation | achievable as stated |
| P2-A3 forced scale respects delta, partial, and protection ceilings and loads | Focused compiler fixture test for `R-FORCE` followed by loader round-trip | achievable as stated |
| P2-A4 scale-0 compatibility and scale>=1 cold determinism | Gate-fixture byte comparison excluding version words; two focused `--no-cache` output hashes for a hierarchy bake | achievable as stated |
| P2-A5 one word builder, carrier parity, and one writer per node slot | Renderer indirection/packing unit tests, render-cpu decode tests, and compose source-shape/writer-election tests for all three compose shaders | achievable as stated |
| P2-A6 no new binding/texture and compose storage remains within eight | Existing forward inventory test plus indirect/direct/animated-direct compose BGL budget tests | achievable as stated |
| P2-A7 every id-41 brick remains scale 0 | Compiler fixture with selected static lights plus format/loader cross-section validation assertion | achievable as stated |
| P2-A8 animated directional light normalizes to static with named warning and static-equivalent coarsening | Translator and compiler integration tests using shared log capture and paired static/animated `light_sun` fixtures | achievable as stated |
| P2-A9 summary reports node histogram and scale-0 pin attribution | Focused summary/log assertion fed from deterministic synthetic stats | achievable as stated |
| P3-A1 scale-aware L1 slots/weights equal shared reconstruction; whole-cell and distinct-tile bounds hold | Render-cpu constructed/property tests mirrored by WGSL source-contract tests at scales 0..3 | achievable as stated if Task 2 selects Phase 3 |
| P3-A2 SDF E[d] decode remains bit-identical | Existing moment packing regression extended across scale-bearing words plus `sdf_shadow.wgsl` source-contract assertion on R/G decode | achievable as stated if Task 2 selects Phase 3 |
| P3-A3 emitted hierarchy reconstruction has zero node failures | Focused analyzer test and fixture report assertion using the exact node-level gate | achievable as stated if Task 2 selects Phase 3 |
| M1 Phase 1 footprint/error/seam measurements | Local table for `stress-warren-hallway-inspection-mini.map` in `research.md`, plus an explicit Windows runbook for `stress-warren-hallway-inspection.map`; both pin 1.0 m spacing, cache mode, machine, workers, wall/RSS when available, cleanup, histograms, bytes, errors, and seam residuals | local measurement blocks Tasks 3 and 7; Windows stress result remains blocking external proof |
| M2 Phase 3 decision recorded from per-resource findings | Decision entry in this plan under Delegated answers with disk/VRAM/bandwidth attribution | manual-analysis; achievable after M1 |
| M3 scale>=1 L2 bakes boot and cover every named receiver without wgpu validation errors | Owner/on-hardware runbook over Phase 1 fixtures with receiver checklist and captured renderer log | manual-visual; blocks landing |
| M4 Phase 3 visual hunt at node faces/open mover paths/lit pools | Owner/on-hardware runbook at default fidelity and forced worst case, recorded as a visual read | manual-visual; blocks landing if Phase 3 is built |
| M5 before/after GPU timing on named adapter | `POSTRETRO_GPU_TIMING=1` 120-frame samples with adapter and timestamp-query availability recorded | manual-performance; `not-yet-evaluable` is an allowed recorded result |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Add the byte-preserving hierarchy projection, recursive merge model, seam/error reporting, and `--sh-density-force-scale`; prove all Phase 1 automated rows with focused tests | integrating executor | — | complete — byte-preserving CLI fixture, recursive topology/blocker tests, parser/surface tests, and touched-target clippy pass |
| 2 | Measure `stress-warren-hallway-inspection-mini.map` locally, record results, resolve maximum scale, 2:1 balance, and the Phase 3 decision, and preserve the full hallway-inspection map for the Windows runbook | integrating executor | 1 | pending |
| 3 | Introduce the shared node-aware reconstruction/stored-set contract, id 34 v11/id 35 v4 wire fields, shared validator, node prefix sum, scale ceilings, and format/loader rejection tests | integrating executor | 2 | pending |
| 4 | Integrate bottom-up hierarchy classification and forced-scale clamping into the compiler; pack id 34/id 35 node payloads; normalize authored animated directional lights; extend summaries and determinism/compatibility tests | integrating executor | 3 | pending |
| 5 | Extend the one runtime indirection word with scale, keep the moments and three compose carriers identical, implement L2 node writer election/copy-through, and re-run binding/texture/storage budget tests | integrating executor | 3, 4 | pending |
| 6 | Produce focused scale>=1 fixture bakes and automated loader/runtime seam proof; prepare the Phase 2 receiver boot checklist without inferring the external visual result | integrating executor | 4, 5 | pending |
| 7 | Apply Task 2's Phase 3 ruling: when selected, add L1 node packing, scale-aware compose arithmetic and sampler fast path with <=8 distinct tiles; otherwise record the measured refusal without changing Decisions | integrating executor | 2, 5 | pending |
| 8 | Extend emitted-reconstruction analysis and dev diagnostics to node scale; complete automated AC coverage and update durable lighting/build-pipeline contracts to the implemented wire and runtime definitions | integrating executor | 6, 7 | pending |
| 9 | Run review-readiness (`cargo fmt --check`, touched-crate checks, focused tests), then `/review-panel` -> `/fix-review-findings` -> focused retest until no concrete finding remains | integrating executor | 8 | pending |
| 10 | Run `/preflight` once, finalize the AC result column, and publish the external M3-M5 runbook; set `status: test-ready` until blocking owner results arrive | integrating executor | 9 | pending |
| 11 | Apply only the reported manual results, move the brief to `done/`, set the final landed status, and commit the plan, brief move, and durable context updates together | integrating executor | 10 and blocking manual proof | pending |

## Implementation ownership and checkpoints

- The integrating executor owns `plan.md`, the shared format/reconstruction contracts, compiler/loader/renderer seams, Cargo commands, and commits.
- No implementation slice is delegated initially: Tasks 1-8 share the node representation and acceptance proofs closely enough that a single owner reduces seam risk. The review-panel workflow may use its own bounded reviewers after integration.
- Resumable checkpoints: commit Task 1 with its plan update; commit Task 2 measurements and delegated answers; then commit each of Tasks 3-8 with its matching plan proof. Never create status-only task commits.
- Task 10 intentionally stops at `test-ready`; external visual/performance proof is never inferred from automated results.
