# sh-compose-contributing-rows — plan of record

mode: compact
status: active
read at: 2bc52d471

## Corrections
- None. No source under `crates/` changed between the brief's `read at` (b9d0a4645) and 2bc52d471. The cited symbols were re-read anyway: `compose_row_membership`, `allocate_sparse_rows`, `SparsePool::{install_undoable,evict}`, `plan_frame_into`, `PassStaleness`, `parse_sparse_rows`, and the capture report lifecycle JSON. They match the brief and R4.

## Delegated answers
- Membership source: `contributing = ref present ∧ sparse_pools[section].row_pairs[row]` is non-empty. The pair is the CPU mirror of the value compose reads (R4). Because the ref is in the conjunction, membership flips only on ref flips, and those already reach `compose_membership_touched`.
- Report shape: `ShComposePassDiagnostics` gains `entry_rows_composed`, a gauge beside `rows_composed` for the same frame. For Pass B a row counts when it has id-45 entries or sits in that frame's Pass A plan.
- Commit seam: the CPU half of each dispatch (planner commit, diagnostics, dirty consumption, indirect epoch) moved into `commit_{indirect,static_direct,animated_direct}_compose` in `frame.rs`. The harness calls the same code the dispatch does, with no hand mirror.
- Partial eviction: the synthetic map gained `with_sparse_row_owner`, so one row's sparse owner differs from its base owner.
- Tests: most behavioural rows run the real install → `prepare_compose_frame` → commit path on the synthetic streamed map. The map gains per-section zero-entry rows, and `fog_draw_all` or mover regions provide the gate. A CPU commit helper mirrors the dispatch's planner commit and dirty clear. No GPU is involved.

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| 1 Membership = installed entry presence | `contributing_rows_tests::membership_follows_installed_entry_counts` | achievable as stated | pass |
| 2 Mixed install: all resident, only entry rows contributing, from install | same test (install-derived) | achievable as stated | pass |
| 3 Zero-entry gated row not planned, per-source only | `per_source_trigger_plans_only_entry_carrying_rows` | achievable as stated | pass |
| 4 Entry row planned that frame | same test | achievable as stated | pass |
| 5 Retained zero-payload records planned (27, 45, 41) | `zero_payload_records_keep_their_rows_contributing` | achievable as stated | pass |
| 6 Pass B upstream (P11) | `pass_b_upstream_follows_id41_entries` | achievable as stated | pass |
| 7 All-zero gated rows: empty plan, no lag (P9) | `trigger_over_only_zero_entry_rows_plans_nothing` | achievable as stated | pass |
| 8 Install / slot reuse / partial eviction (P1, P4, P14) | `residency_changes_plan_zero_entry_rows_once`, `partial_eviction_plans_its_rows_regardless_of_gate` | achievable as stated | pass |
| 9 Control/mask change and force-full plan every resident row (P2) | `control_change_and_force_full_plan_zero_entry_rows` | achievable as stated | pass |
| 10 Pass A rewrite marks Pass B pending regardless of id-45 | `pass_a_rewrite_reaches_pass_b_without_id45_entries` | achievable as stated | pass |
| 11 Gate leave/re-enter (P3) | `zero_entry_row_reenters_gate_current` | achievable as stated | pass |
| 12 Deactivation tail (P5–P7) | `deactivation_tail_plans_only_entry_carrying_rows` | achievable as stated | pass |
| 13 Touched queue on mixed map (P12, P13) | `touched_rows_cover_every_compose_membership_change`, run over both maps, with a rollback after sparse allocation | achievable as stated | pass |
| 14 Zero-entry row in resident union and dirty set; unions = rebuild | `zero_entry_rows_join_resident_unions_and_dirty_sets` | achievable as stated | pass |
| 15 Oracle suite passes | `cargo test -p postretro-renderer --lib sh_streaming` (includes `compose_plan_oracle`) | achievable as stated | pass |
| 16 Capture test with entry-row assertions | `sampled_row_gate_capture_matches_full_resident_at_stepped_times` (`--features capture -- --ignored`) | achievable as stated | pass: gated t050 indirect 112/112, Pass A 0, Pass B 15/15 entry rows; 18/18 sampleable; byte-identical to full-resident |
| 17 Scope grep | `git diff --name-only main` filtered on `.wgsl` / level-format / level-compiler / level-loader | achievable as stated | pass (empty) |
| 18 Mac arena + station rows/ms before/after | owner, in-engine | manual | outstanding |
| 19 Station Pass B = id-45 rows ∪ Pass A rows | owner, in-engine | manual | outstanding |
| 20 Mac atlas byte check | owner, throwaway branch | manual | outstanding |

Gate: `cargo fmt --check` ✓; `cargo clippy --target-dir target/preflight-clippy -- -D warnings` ✓; `cargo test` ✓ (60 targets, 9374 passed); `cargo check --release` ✓; `crate-graph --check` ✓; `dev-tools` renderer clippy and tests ✓; `capture` report tests ✓. Known and pre-existing: `cargo clippy --all-targets --features postretro/capture` fails in `capture/prepared.rs`, `capture/scene.rs`, `capture_shadowmask_groups.rs` and at `report.rs:27`. Each is outside this diff.

Manual-proof aid: the capture report now carries `entry_rows_composed` per pass, beside `rows_composed`. The `[SH streaming]` log line and the dev-tools tab do not show it. AC 18 and 19 can read the split from a capture at each pose, or from the spike's `RowCountWindow`.

## Review loop
- Round 1 panel: 2 correctness tracers + adversarial tester (Opus) and hygiene/drift (Sonnet). No 🔴 code findings.
- Acted on:
  - P12 rollback now fails after sparse allocation.
  - Pass B `entry_rows_composed` doc softened (the predicate is unchanged, per the AC).
  - Direct-epoch seam comment.
  - Test-helper empty-plan guard.
  - Capture precondition now asserts `evictions == 0`.
  - `PassMembership` doc.
  - `rendering_pipeline.md` §4 "Not built yet" drift removed.
- Not acted on:
  - `warm_clusters == 0`: the real gated capture has 8 warm clusters, so the tracer's premise was false.
  - P15 automated dense-growth readback test (GPU). The brief assigns P15 to the manual byte check, so it is offered to the owner as a follow-up.

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Narrow `compose_row_membership` to entry-carrying rows, add zero-entry rows to the synthetic map, land ACs 1–2, 14 | integrating executor | — | done: `contributing_rows_tests` |
| 2 | Frame-path behaviour tests, ACs 3–13 | integrating executor | 1 | done: 13 tests; 10 fail on the old ref-only membership, and the other 3 are preservation guards |
| 3 | `entry_rows_composed` diagnostic through report, plus capture test assertions (AC 16), then run the capture | integrating executor | 1 | done: capture passes; gated t050 indirect 112/112, Pass B 15/15 entry rows |
| 4 | Oracle and full renderer gate (AC 15), scope grep (AC 17) | integrating executor | 2, 3 | done: `sh_streaming` 153 pass; scope grep empty |
