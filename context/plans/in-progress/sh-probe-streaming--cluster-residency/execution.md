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
| 4 compiler publication split | Running | — | — |
| 5–13 streaming and proof | Pending | — | — |

After each completed phase, check workspace free space. If it falls below
10 GiB, clean only explicit PostRetro Cargo crates with `cargo clean -p`.
