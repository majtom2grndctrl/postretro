# Slice 3 execution checkpoints

Integration branch: `feature/sh-probe-streaming`. Slice 3 was promoted at
`43366324a` and moved in progress at `9c58d19ae` after the latest `origin/main`
merge (`6d6f37979`). Billboard scatter ids 47/48 remain whole-resident in this
slice by owner decision.

| Task | Status | Commit | Verification |
|---|---|---|---|
| 1 loader ownership split | Done | `00ea51b84` | workspace check; loader default/no-default tests; focused code review |
| 2 renderer SH split | Done | `0f39b4f1b` | workspace and dev-tools checks; 34 focused SH tests; exact billboard source-order test; focused code review |
| 3 app/capture split | Done | `93538cd99`, `1c8f49a01` | workspace check; 19 capture-feature tests; focused code review |
| 4 compiler publication split | Done | `b236d4d5c` | compiler check; 55 pack tests; 18 pipeline tests; focused code review |
| 5 shared id-50 codec | Done | `5e1317e97` | 544 level-format tests; compiler cap test; both crate checks; focused code review |
| 6 deterministic compiler chunks | Done | `f71607881` | compiler check; 8 focused id-50 tests; focused code review |
| 7 metadata-only loader manifest | Done | `1451a5bc6` | 205 loader tests; postretro check; capture target compile; physical no-read fixture; focused code review |
| 8 pure residency planner | Done | `a8a386351` | 12 focused planner tests; postretro check; focused code review |
| 9 renderer cluster residency | Done | `c154bfe68`, `347c51dae` | renderer full suite: 564 passed, 1 ignored; workspace check; fmt; 44 focused streaming tests; final code review approved |
| 10 synchronous runtime thin path | Done | `86c94c928`, `340db5a8e`, `1367d938e` | 16 focused app tests; 834 app tests (2 ignored); 206 loader tests; 569 renderer tests (1 ignored); workspace check; fmt; focused code review approved; attended Mac startup exposed and motivated shader-entry and small-map floor fixes; full workspace tests pass after floor fix; visual GPU proof remains open |
| 11 bounded async read/decode | Done | `3795e4bd2` | delayed 250 ms positional-reader tests in app and loader; completion identity/permit regression; postretro check/build; full workspace `cargo test`; fmt; strict Clippy blocked by four pre-existing level-format warnings |
| 12 eviction, growth, hysteresis, diagnostics | Done | `ad08733a7` | 20 controller policy tests, 2 renderer release-order tests, capture JSON test, shared-target app/renderer/loader check, full workspace `cargo test`, fmt, focused code review; strict Clippy blocked by four pre-existing level-format warnings |
| 13 integrated proof and measurement | Done with GPU evidence not-yet-evaluable | `55ceb8a3d` | 536 MiB whole versus 264 MiB streamed pure allocation fixture; app/controller/worker, loader, 1-vs-4 compiler determinism, legacy-body, and renderer binding-budget focused tests; postretro check and fmt; bounded Mac attempt stopped during rebuild before engine launch; no sampled GPU frame |

After each completed phase, check workspace free space. If it falls below
10 GiB, clean only explicit PostRetro Cargo crates with `cargo clean -p`.

Phase 1 disk gate: 32 GiB available after Task 4. No Cargo cleanup needed.
Phase 2 disk gate: 30 GiB available after Task 5. No Cargo cleanup needed.
Phase 3a disk gate: 23 GiB available after Tasks 6–7. No Cargo cleanup needed.
Phase 3b disk gate: 21 GiB available after Task 8. No Cargo cleanup needed.
Phase 4 disk gate: 18 GiB available after Task 9. No Cargo cleanup needed.
Phase 5 disk gate: 24 GiB available after Task 10. During full-workspace
preflight, available space fell below 10 GiB; `cargo clean -p` for PostRetro
packages removed 23.3 GiB of rebuildable artifacts. Strict Clippy stops on
pre-existing level-format warnings, and the broad workspace test run was
interrupted to protect disk space after the affected crate suites passed.
Attended Mac play-test follow-up: streamed animated-direct SH pipeline selected a
nonexistent shader entry (`340db5a8e`), then the app rejected a valid renderer
physical floor below the requested 256 MiB (`1367d938e`). Both now have CPU
regressions. Full `cargo test` passes after the floor fix; strict Clippy still
stops on the same four level-format warnings. The bounded offscreen GPU attempt
could not acquire a Metal adapter in this execution context and exited; no
visual result was published. Post-fix disk gate: 14 GiB available, no cleanup.

Subsequent Mac play-test follow-up: streamed id-45 retained its animated-baked
roster in the manifest, but the renderer sized/validated its forward light tail
from the omitted whole section. This rejected every light-bridge snapshot and
left scripted dynamic lights at their prior values. The renderer now reads the
roster and affinity indices from the active SH storage mode for initialization,
reload, and capacity sizing. A bounded 45-second `campaign-test` GPU run reached
the first level frame with no light-bridge rejection (the prior run rejected on
every frame); visual animation still needs owner confirmation. The separate
animated-lightmap dispatch-limit failure remains. Focused renderer tests,
workspace `cargo test`, and formatting pass. Strict Clippy still stops on four
pre-existing level-format warnings. Disk gate: 13 GiB available, no cleanup.

Phase 6 disk gate: 12 GiB available after Task 11, no cleanup. A bounded
40-second Mac engine attempt was terminated by its hard cutoff before the map
reached its first level frame, so it is not a visual proof. The async worker
manager keeps four named jobs, retains one permit through ready/install/drop,
and joins the old generation before creating a new pool on reload. The full
workspace test suite passes; strict Clippy still stops on the same four
pre-existing level-format warnings.

Owner cleanup preference (2026-09-23): on the next low-disk cleanup, wait for
Cargo to become idle and clean a broader explicit set of churn-heavy PostRetro
packages with `cargo clean -p` (app, renderer, level-loader, level-format,
level-compiler as warranted), still never a bare workspace-wide `cargo clean`.

Phase 7 disk gate: 10.5 GiB available after Task 12 preflight and commit; no
cleanup needed. Sync-proof remains a deterministic no-eviction baseline, while
async mode enables the eviction policy. Renderer-confirmed releases drive the
controller ledger. Capture reports exact current host-phase bytes and explicit
high-water *upper bounds* because worker and controller ledgers have independent
historical maxima.

During Phase 8, free space approached the 10 GiB gate before additional Cargo
checks. With Cargo idle, an anticipatory crate-scoped cleanup of `postretro`,
`postretro-renderer`, `postretro-level-loader`, `postretro-level-format`,
`postretro-level-compiler`, `postretro-render-cpu`, and `postretro-render-data`
removed 60,721 rebuildable files (Cargo reported 19.2 GiB); free space rose to
about 23.5 GiB. No source, map, report, or worktree files were removed.

Phase 8 disk gate: 20.79 GiB available after the focused rebuild and Task 13
checkpoint; no further cleanup needed. GPU frame-time, seam, and retirement
observations are explicitly not-yet-evaluable on this host rather than treated
as a failing implementation gate; see `measurements/sh-probe-streaming/cluster-residency.md`.

Post-Task-13 warning cleanup checkpoint: `c5c6ff80d` removed obsolete SH
streaming helpers, gated capture/test-only diagnostics, and resolved mechanical
level-format/loader lints. Default and capture `cargo check` pass without
streaming dead-code warnings; focused tests and full workspace `cargo test`
pass. `cargo fmt --check` passes. Strict workspace Clippy now advances through
level-format and loader, but stops on 24 renderer findings (mostly pre-existing
high-arity GPU APIs and mechanical lint suggestions). Phase-end disk check:
14.5 GiB available; no cleanup needed. Final review panel and plan landing
remain pending an allowance/owner-scope decision, not a code or test failure.

Review-panel checkpoint (2026-09-23): the async session and renderer handoff
slices found three substantive lifecycle gaps. Reload now cancels and retires
workers without joining an in-flight positional read on the frame path, while
retaining the old manifest/file and delaying the replacement pool until all four
workers finish. Renderer compose success now reaches the app/capture promotion
latch; a presented or read-back frame alone is not a successful compose.
Malformed over-cap or duplicate-ready drain batches reject before renderer
residency mutation. Focused delayed-read/reload, batch, and promotion-signal
regressions pass; touched-crate checks, `cargo fmt --check`, and full workspace
`cargo test` pass. Strict Clippy still stops on the same 24 renderer findings.
The panel has reviewed two bounded slices, not the entire branch. Phase-end
disk gate: about 11 GiB available, so no cleanup was needed.
