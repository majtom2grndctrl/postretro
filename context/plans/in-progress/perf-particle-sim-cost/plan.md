# perf-particle-sim-cost — plan of record

mode: compact
status: test-ready
read at: db24d5b65

## Corrections
- No source files changed between brief read-at `a307e3f10` and claim `db24d5b65`; grounded Decision reads remain valid.
- The proposed first slice leaves impacts in the registry; implementation will keep that intermediate state only until impact intake integration. Final proof and timing cover the complete pool path.
- Manual performance and visual proof require the specified frontmost Intel Mac run. They remain blocking and will be reported as outstanding, never inferred from automated tests.

## Delegated answers
- Pool growth: reserve the bounded particle columns at construction/level setup; retain high-water capacities across clear. Reuse block and spin slots. This avoids per-particle allocation in warmed spawn/step/expire paths without registry storage.

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| 1 | Pool integration reference, live-spin and generational orphan tests | achievable as stated | pass |
| 2 | Pool distinct per-particle acceleration test | achievable as stated | pass |
| 3 | Pool swap-remove/expiry/headroom tests (P17) | achievable as stated | pass |
| 4 | Emitter headroom across parameter blocks (P6) | achievable as stated | pass |
| 5 | Pool/registry overflow, throttle and mixed-producer tests (P11) | achievable as stated | pass |
| 6 | Partial burst and blocked-rate backlog tests (P10, P12) | achievable as stated | pass |
| 7 | Pool and intake clear/throttle reset tests (P13) | achievable as stated | pass |
| 8 | Spawn-time parameter snapshot tests (P1) | achievable as stated | pass |
| 9 | Lazy block mint/rewrite/free tests (P2, P3, P5) | achievable as stated | pass |
| 10 | Block reuse with previous emitter resuming (P4) | achievable as stated | pass |
| 11 | Static impact blocks and clear/reuse tests (P15) | achievable as stated | pass |
| 12 | Repoint fixed-tick, prediction, observer and host-ingest impact suites | achievable as stated | pass |
| 13 | Frame drain → emit → sim → collect seam tests (P8, P9) | achievable as stated | pass |
| 14 | Host intake routing and remote contact tests (P16) | achievable as stated | pass |
| 15 | Registry independence and level-clear/reload tests (P14) | achievable as stated | pass |
| 16 | Existing allocation probe: warmed 100 vs 10,000 spawn/step/expire and rewrite frames | achievable as stated | pass |
| 17 | Collector visible-cell, drift, orphan, draw-all, rotated-edge and stride tests | achievable as stated | pass |
| 18 | CPU-timing count zero/off/window tests (P18) | achievable as stated | pass |
| 19 | FFI unknown-kind tests; generated typedef checks and TS/Luau rejection fixtures | achievable as stated | pass |
| 20 | Exhaustive discriminant/wire drift and old-peer refusal tests | achievable as stated | pass |
| 21 | Focused emitter, particle, collector and impact suites | achievable as stated | pass |
| 22 | Pool/collision push test independent of buoyancy | achievable as stated | pass |
| 23 | Pool/collision low-gravity, exit and zero-buoyancy tests | achievable as stated | pass |
| 24 | Post-position boundary gravity test | achievable as stated | pass |
| 25 | Executor-run release stress measurement with owner maintaining frontmost/unlocked Mac; 20 post-warmup windows | measured, blocking | fail: exclusive sim and environment exceed budgets; emit/collect pass |
| 26 | Executor-run 0/~10k/~20k emitter sweep; 21/20/20 post-warmup windows | measured | pass: 3.42% spread in per-tick cost |
| 27 | Owner: particle zone and weapon room visual checks | manual, blocking | outstanding manual proof |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | SoA pool, immutable blocks, spin slots, integration/expiry/capacity proof; collector with conservative frustum reject | pool + collector workers; integrating executor owns API | — | implemented; focused pool/collector proof passed |
| 2 | Bounded impact intake; remove particle component and reconcile discriminants, wire, FFI and SDK | boundary worker; integrating executor owns seams | 1 API | implemented; boundary and real TS/Luau proof passed |
| 3 | Emitter/frame/session integration, impact materialization, E24 point resolve, timing count and lifecycle/ordering/allocation proofs | integrating executor | 1, 2 | implemented; frame/lifecycle/network/allocation proof passed |
| 4 | Focused readiness, review-panel/fix loop, final preflight; record all results and external runbook | integrating executor | 3 | complete; review/fixes and final feature-complete preflight passed |

## Hot-path constraints
- No registry particle slots, per-particle strings, component rewrites, refcount churn or allocations.
- Spawn clamps before minting; blocks and spin slots reclaim only after the last referencing particle.
- Point gravity/push resolve after position integration; no fluid work; separately timed under particle_sim.
- Collector uses resolved collection indices, retained scratch and conservative radius/frustum rejection before exact own-position cell lookup.
- Impact requests drain in raise order before emitters, and never enter transport.

## External runbook
- Recompile committed `stress-env-volumes.map`; release + dev-tools; `caffeinate`; frontmost, unlocked Intel Mac; pose `114.60,2.44,-138.99,45,0`; record load average and at least five 120-frame windows after warmup.
- At ~10k: particle_sim exclusive ≤0.6 ms, particle_collect ≤1.0 ms, particle_emit ≤0.3 ms; nested environment ≤0.5 ms. Compare sim_tick/ticks at zero/~10k/~20k (≤10%). Delete temporary sweep maps/PRLs.
- Smoke/sparks/impacts visually unchanged in particle zone (d) and weapon room, including spin, doorway drift, portal edges and screen edges.
- No landing until AC25 meets its budgets and AC27's blocking visual result arrives; then wait for owner “land the plane”.

## Verification evidence (implementation)
- `cargo check -p postretro --features dev-tools`: pass.
- `cargo test -p postretro-sim --lib --features dev-tools particle_pool`: 15 passed, including original integration equivalence, generational orphan spin, block recycling, allocation probe and E24 volume seams.
- `cargo test -p postretro-sim --lib --features dev-tools weapon::impact::tests`: 8 passed.
- `cargo test -p postretro --bin postretro --features dev-tools particle_`: 30 passed, 2 intentional ignored profiling tests.
- `cargo test -p postretro --bin postretro --features dev-tools emitter_bridge`: 31 passed, including full-frame allocation-count equality at 100/10,000 with repeated emitter rewrites.
- Worker boundary checks: entities intake 4 + FFI 1; net handshake 19 + constant 1; all passed.
- Full TypeScript suite: cached TypeScript 6 `tsc -p sdk/type-tests`, pass including removed kind/alias/value/export negatives.
- Actual Luau static checker: official Luau 0.742 x86_64 analyzer built from source in `/tmp/postretro-luau-analyzer-build`. `python3 sdk/type-tests/luau/check_anonymous_particles.py /tmp/postretro-luau-analyzer-build/luau-analyze`: pass; positive retained emitter exits 0 without diagnostics, negative particle_state/particle exits 1 with exactly two matching TypeErrors. The runner extracts the exact generated SDK ComponentKind alias because the stock CLI cannot load declaration files; it maintains no duplicate union.

## Review and fixes
- Review-panel: ten fresh GPT-6.1 Sol reviewers covered pool/E24 correctness, adversarial lifecycle, boundary contract verification, emitter/frame integration, collector correctness, hygiene/comment drift for each slice, and the full impact queue-to-render/reload seam. No runtime correctness defects found.
- Three distinct mechanical findings fixed by single-file GPT-6 Luna workers: standard `[Particles]` overflow warning prefix; wire-30 assertion explanation; misleading frame-loop comment replaced with own-position cell culling and conservative frustum pretest wording.
- Focused fix retest passed: engine check with dev-tools, four intake tests and nineteen handshake tests.
- Initial final preflight: formatting and feature-complete all-targets Clippy passed. Workspace tests caught a stale netcode assertion expecting wire 29 (689 passed, one failed in that target). Reconciled only that expected-current version/message to the brief-required 30; application protocol and snapshot assertions remain unchanged. A Rust search found only the intentional pre-renumber peer-refusal case still naming 29. Corrected assertion matched one test and passed; the interrupted workspace gate was rerun in full and passed. Manual ACs 25–27 remain blocking.

### Manual commands
From the workspace root, after automated work finishes (no concurrent builds):

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/stress-env-volumes.map -o content/dev/maps/stress-env-volumes.prl --sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache
uptime
caffeinate -dimsu env POSTRETRO_CPU_TIMING=1 RUST_LOG=info cargo run -p xtask -- run --release --features dev-tools -- content/dev/maps/stress-env-volumes.prl --start-pose=114.60,2.44,-138.99,45,0
```

Keep the window frontmost and the Mac unlocked. Drop warmup, then retain at least five complete windows. Read `particle_live`, `particle_emit`, `particle_sim`, `particle_environment` and `particle_collect`; exclusive particle simulation = particle_sim − particle_environment. `sim_tick / ticks` gives per-tick cost. Use stage averages, since the smoke frame is GPU-bound.

For the tick sweep, make task-owned map copies beside the fixture, changing only each `billboard_emitter` entity's rate to 0, 50 or 100 (same 5s lifetime and all other entities/geometry unchanged). Bake/run each with identical flags and pose. Read actual live counts from each window, compare per-tick costs (≤10%), then delete copies and sweep PRLs. The 50/s committed fixture is the ~10k budget run. Record load average for every run.

- Additional focused gates: network impact 4, host-ingest 5, remote-splash 2, discriminants 3, binary client impact 6, presentation capture 11, sprite registration 8, conditioned host activation 55, actual level reload 1, typedef 43, removed-kind world-query 1, fixed-tick splash seam 1, collision randomized overlap/property proof 1 — all passed. Host-ingest initially hit sandbox localhost-bind refusal; the same five tests passed with socket access. A mistaken `host_local_splash` filter matched zero and is not counted; corrected full test name matched one and passed.

## Final automated gate
- `cargo fmt --check`: pass.
- `cargo clippy --workspace --all-targets --features postretro/dev-tools,postretro/capture --target-dir target/preflight-clippy -- -D warnings`: pass.
- `cargo test --workspace --features postretro/dev-tools,postretro/capture`: pass; 10,930 passed, zero failed, 57 ignored across 68 test/doc-test target summaries. The ignored tests retain their existing on-demand status; manual ACs are not inferred from them.
- Final logs: `/tmp/postretro-particle-preflight-fmt-final.log`, `/tmp/postretro-particle-preflight-clippy-final.log`, `/tmp/postretro-particle-preflight-focused-final.log`, `/tmp/postretro-particle-preflight-test-final.log`.
- Review/fix loop resolved three panel findings and the full-gate stale assertion; no unresolved code findings or Decision/Acceptance changes. Durable context reconciliation remains reserved for the authorized landing step.
- Final disk check: 21 GiB available; no cache cleanup needed.
- Branch: `perf-particle-sim-cost`. ACs 1–24 pass. Subsequent AC25/AC26 results are recorded below; AC27 remains outstanding. Remain in `in-progress/` at `test-ready` while the performance failure and visual gate block landing.

## AC25 / AC26 execution — 2026-10-12

The owner authorized the executor to run both measurements and maintained the Mac unlocked, with the engine frontmost and no other heavy workloads. The executor ran three sequential cases at implementation commit `c091463d2` without source changes or concurrent builds. Evidence, all sampled windows, machine snapshots, fixture/binary hashes and exact commands are in [`acceptance-2026-10-12.json`](acceptance-2026-10-12.json).

- Machine: Intel i9-9980HK, 8 physical / 16 logical CPUs, 32 GiB RAM, AMD Radeon Pro 5300M / Metal, macOS 26.6.2, AC power. `CPU_Speed_Limit=100` and no thermal/performance warnings before and after each run. `ScreenSaverEngine` was absent before each launch and at every CPU window.
- Release build with dev-tools and release scripts-build sidecar; direct engine launch after build is equivalent to the recorded xtask launch, with no Cargo work during measurements. `POSTRETRO_CPU_TIMING=1`, `RUST_LOG=info`, `caffeinate -dimsu`; no Rayon, backend or validation overrides.
- All three maps rebaked with `--sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache`. Loader confirmed 1,000 environment volumes and the pinned pose for every case. The two temporary maps changed only the rate of all 40 emitters to 0 or 100; inverse replacement exactly recovered the committed 50-rate source.
- Existing display settings stayed unchanged across cases: exclusive 3584×2240 display mode, `render_resolution="third"`, vsync on.
- Discarded the first two 120-frame windows and every window starting before 30 seconds. Retained every complete window ending by 140 seconds of in-level elapsed time. No timing windows were cherry-picked; 20/21/20 windows were retained for rates 50/0/100. Per-tick cost is summed `sim_tick` averages divided by summed `ticks` averages. Timing log precision is 0.001 ms and 0.1 ticks.

### AC25 — fail

20 retained windows at a mean of 9,954 live particles. Values are milliseconds per frame; ranges are window averages. Exclusive particle simulation subtracts the nested environment time from its parent in each window.

| Stage | Mean | Window-average range | Budget | Result |
|---|---|---|---|---|
| particle_sim, exclusive | 0.818 | 0.726–0.834 | ≤0.600 | fail |
| particle_environment | 1.617 | 1.435–1.646 | <0.500 | fail |
| particle_collect | 0.508 | 0.455–0.517 | ≤1.000 | pass |
| particle_emit | 0.063 | 0.059–0.065 | ≤0.300 | pass |

The inclusive particle_sim mean was 2.435 ms. The failure is measured proof: AC25 needs optimization and another pinned run before landing; automated correctness results do not establish its performance budgets.

### AC26 — pass

| Emitter rate | Mean live particles | Retained windows | sim_tick per tick (ms) | Load before → after (1 / 5 / 15 min) |
|---|---|---|---|---|
| 0 | 0 | 21 | 3.271 | 3.68 / 8.03 / 10.41 → 3.46 / 6.26 / 9.30 |
| 50 | 9,954 | 20 | 3.163 | 3.68 / 11.32 / 11.88 → 3.01 / 8.05 / 10.45 |
| 100 | 19,907 | 20 | 3.197 | 3.31 / 6.13 / 9.22 → 2.72 / 4.84 / 8.21 |

Maximum/minimum per-tick cost differs by **3.42%**, below the 10% limit. Runs occurred at 14:42–14:49 America/Los_Angeles; elevated longer-term load averages include preparation builds and bakes, which had finished before measurements.

### Cleanup and remaining work

- Engine test processes stopped after sampling. Temporary rate-0/rate-100 maps and PRLs deleted; committed-map rebake retained as a normal ignored build product. Scratch measurement logs and runner scripts removed after preserving durable evidence.
- Only the acceptance record and this plan changed after implementation; no source behavior changed in this measurement session.
- AC25 fails, AC26 passes, AC27 remains outstanding. Status stays `test-ready`; the brief is not ready to land.
