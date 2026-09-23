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
| 9–13 GPU integration and proof | Pending | — | — |

After each completed phase, check workspace free space. If it falls below
10 GiB, clean only explicit PostRetro Cargo crates with `cargo clean -p`.

Phase 1 disk gate: 32 GiB available after Task 4. No Cargo cleanup needed.
Phase 2 disk gate: 30 GiB available after Task 5. No Cargo cleanup needed.
Phase 3a disk gate: 23 GiB available after Tasks 6–7. No Cargo cleanup needed.
Phase 3b disk gate: 21 GiB available after Task 8. No Cargo cleanup needed.
