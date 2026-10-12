# perf-particle-sim-cost — plan of record

mode: compact
status: active
read at: db24d5b65

## Corrections
- No source files changed between brief read-at `a307e3f10` and claim `db24d5b65`; grounded Decision reads remain valid.
- The proposed first slice leaves impacts in the registry; implementation will keep that intermediate state only until impact intake integration. Final proof and timing cover the complete pool path.
- Manual performance and visual proof require the specified frontmost Intel Mac run. They remain blocking and will be reported as outstanding, never inferred from automated tests.

## Delegated answers
- Pool growth: reserve the bounded particle columns at construction/level setup; retain high-water capacities across clear. Reuse block and spin slots. This avoids per-particle allocation in warmed spawn/step/expire paths without registry storage.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| 1 | Pool integration reference, live-spin and generational orphan tests | achievable as stated |
| 2 | Pool distinct per-particle acceleration test | achievable as stated |
| 3 | Pool swap-remove/expiry/headroom tests (P17) | achievable as stated |
| 4 | Emitter headroom across parameter blocks (P6) | achievable as stated |
| 5 | Pool/registry overflow, throttle and mixed-producer tests (P11) | achievable as stated |
| 6 | Partial burst and blocked-rate backlog tests (P10, P12) | achievable as stated |
| 7 | Pool and intake clear/throttle reset tests (P13) | achievable as stated |
| 8 | Spawn-time parameter snapshot tests (P1) | achievable as stated |
| 9 | Lazy block mint/rewrite/free tests (P2, P3, P5) | achievable as stated |
| 10 | Block reuse with previous emitter resuming (P4) | achievable as stated |
| 11 | Static impact blocks and clear/reuse tests (P15) | achievable as stated |
| 12 | Repoint fixed-tick, prediction, observer and host-ingest impact suites | achievable as stated |
| 13 | Frame drain → emit → sim → collect seam tests (P8, P9) | achievable as stated |
| 14 | Host intake routing and remote contact tests (P16) | achievable as stated |
| 15 | Registry independence and level-clear/reload tests (P14) | achievable as stated |
| 16 | Existing allocation probe: warmed 100 vs 10,000 spawn/step/expire and rewrite frames | achievable as stated |
| 17 | Collector visible-cell, drift, orphan, draw-all, rotated-edge and stride tests | achievable as stated |
| 18 | CPU-timing count zero/off/window tests (P18) | achievable as stated |
| 19 | FFI unknown-kind tests; generated typedef checks and TS/Luau rejection fixtures | achievable as stated |
| 20 | Exhaustive discriminant/wire drift and old-peer refusal tests | achievable as stated |
| 21 | Focused emitter, particle, collector and impact suites | achievable as stated |
| 22 | Pool/collision push test independent of buoyancy | achievable as stated |
| 23 | Pool/collision low-gravity, exit and zero-buoyancy tests | achievable as stated |
| 24 | Post-position boundary gravity test | achievable as stated |
| 25 | Owner: pinned release stress run, five windows; sim/collect/emit and nested environment budgets | manual, blocking |
| 26 | Owner: pinned 0/~10k/~20k emitter sweep, sim_tick within 10% | manual, blocking |
| 27 | Owner: particle zone and weapon room visual checks | manual, blocking |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | SoA pool, immutable blocks, spin slots, integration/expiry/capacity proof; collector with conservative frustum reject | pool + collector workers; integrating executor owns API | — | pending |
| 2 | Bounded impact intake; remove particle component and reconcile discriminants, wire, FFI and SDK | boundary worker; integrating executor owns seams | 1 API | pending |
| 3 | Emitter/frame/session integration, impact materialization, E24 point resolve, timing count and lifecycle/ordering/allocation proofs | integrating executor | 1, 2 | pending |
| 4 | Focused readiness, review-panel/fix loop, final preflight; record all results and external runbook | integrating executor | 3 | pending |

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
- No landing until blocking manual results arrive; then wait for owner “land the plane”.
