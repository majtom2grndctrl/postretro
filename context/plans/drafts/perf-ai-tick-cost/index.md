# perf-ai-tick-cost

Brief · compact · reads: `context/lib/entity_model.md` §1, §5, §7c, §9 · `context/lib/ai_pathfinding_mt_readiness.md` · `context/lib/rendering_pipeline.md` §12 · `context/lib/testing_guide.md` §Resource bounds · read at 2095e74c6

## Problem
Developer-observed defect from the E24 stress pre-flight. On `stress-env-volumes`, 64 agents cost about 5.8 ms per tick in `sim_ai` and 2.5 ms per tick in steering. The fixed step spirals at 128 agents. The cause is the enemy target scan, `targeting::target_candidates`, run for every enemy every tick through `target_offers`. It walks every slot of the entity registry twice (`EntityRegistry::iter_with_kind` over `PlayerMovement` and `Brain`), so AI cost is O(enemies × registry slots), and on this fixture particles inflate the slot count about 11× (`research.md`). Steering repeats the shape. `agent_steering::admit_replans` resolves two nav regions per moving agent per tick, each through a linear scan of every nav region (`NavGraph::region_at`).

When done:
- Per-enemy targeting cost does not depend on registry size.
- Per-enemy targeting cost tracks the hostile pawns it must inspect, not every pawn.
- Point-to-region lookup does not scan the navmesh.
- Every enemy selects the same target, on the same tick, as today.

## Decisions
- **Target candidates come from a faction-first pawn index, built once per AI pass.**
  - Pawns are player-movement holders plus brain holders, bucketed by faction once per pass. Hostility resolves once per evaluating faction and bucket, from the pass's live sentiment.
  - The think stride is priced from the nearest pawn in the evaluating enemy's hostile buckets only.
  - On due ticks, the enemy walks hostile pawns in (distance, group, slot) order, group being today's movement-holders-first scan order, and stops at the first that passes candidacy and line of sight. Retaliation is checked directly against the ≤ 8 recent-attacker ledger entries (`RECENT_ATTACKER_LEDGER_CAPACITY`): `targeting::retaliation_preference` admits only a candidate with an attacker record.
  - The due-tick set is therefore hostile pawns plus ledger attackers, first eligible, not the full candidate set.
  - A grid inside a faction bucket is added only once that bucket is large, as an internal refinement with identical results.
  - Engine-floor semantics in `entity_model.md` §7c are untouched: ranking, hysteresis, retaliation admission and latch, the candidacy predicate and the raw-distance stride price.
- **`entity_model.md` §9 gains a named exception** to "Spatial partitioning for entity-entity queries (octree, grid)": the AI target-candidate index, faction-bucketed and exact. Every other entity-entity query stays unindexed. Amended at promotion.
- **No cell-visibility broad phase.** `plans/done/E10--enemy-mp-target-selection` reserved the selection seam for one if enemy counts made selection hot. Rejected: the stride is priced on raw distance, which visibility cannot answer, and line of sight measures about 0% of AI cost.
- **Nav point-to-region lookup is indexed at load.**
  - `NavGraph` builds a cell → candidate-regions index over its existing grid when constructed.
  - `region_at` and the snapping `resolve_region_at` return exactly what the linear scans return. That includes stacked floors and off-mesh snap within tolerance.
  - No PRL format change. A load-time build keeps the bake cache untouched and is reversible.
- **AI cost gets sub-stages.**
  - `sim_ai` gains sub-stages for targeting, perception, graph evaluation, and apply with combat slots, on the existing `SimStage` set.
  - `sim_steering` keeps its `sim_steer_agents`, `sim_steer_pose` and `sim_steer_movers` split. Labels follow `rendering_pipeline.md` §12.
- **Budgets are priced on top of the particle pool.** The `sim_ai` and `sim_steer_agents` targets below were set at about 10k registry-resident particles and are re-confirmed against a baseline measured after `perf-particle-sim-cost` lands.
- **Landing order** across the stress pre-flight work:
  1. GPU smoke spike (queued, not drafted).
  2. Particle pool (`perf-particle-sim-cost`).
  3. This brief's nav region index, independent, may land alongside 2.
  4. This brief's pawn index, after the pool.
  5. E24's particle environment rows.
  6. Registry occupancy index, last, its own brief.
- **Non-goals.**
  - Registry occupancy index: the last-landing follow-up, justified by non-particle populations (projectiles, props, crates) with a synthetic population of its own. Registry storage shape: `drafts/registry-column-storage`.
  - Time-slicing perception or LOS: measured at about 0% of AI cost, and it would change reaction latency.
  - Changing think-stride bands.
  - Parallel AI (`ai_pathfinding_mt_readiness.md`): the cost is a cache-hostile walk, not compute.
  - Removing particles from the registry: `perf-particle-sim-cost` owns it.

## Acceptance
### Automated
- [ ] Target selection matches a brute-force reference across randomized scenes, on the selected target, the retaliation latch, the stride-pricing distance and the order among equal-distance candidates. The scenes cover:
  - one and several player pawns;
  - peer brains, and several factions with asymmetric sentiment;
  - a retained target, including a hysteresis tie;
  - a hostile ledger attacker, and a non-hostile pawn admitted only by retaliation over tolerance;
  - a candidacy predicate or failed line of sight rejecting the nearest several hostile pawns;
  - the evaluating enemy excluded;
  - zero candidates, and zero hostile buckets.
- [ ] In a converged horde (one faction around one player), a due-tick selection evaluates candidacy and line of sight for no pawn beyond the first eligible hostile, plus ledger attackers.
- [ ] Per-enemy targeting cost for 64 brains does not change when the registry also holds 60,000 non-pawn entities, within a stated bench threshold. Only the once-per-pass index build may grow.
- [ ] Indexed region lookup equals the linear scan for every nav grid cell centre and for randomized points, including:
  - stacked floors at one XZ;
  - points just inside and just outside the snap tolerance;
  - points beyond the grid.
- [ ] With CPU timing on, the AI sub-stages appear under `sim_ai` and the steering sub-stages under `sim_steering`, each no larger than its parent. With timing off, none report.
- [ ] Existing AI, steering and nav suites pass unchanged.
### Manual
- [ ] Stress, pinned per `testing_guide.md` §Resource bounds:
  - **Run:** `stress-env-volumes.prl` recompiled from the committed map, mixed pose `114.60,2.44,-138.99,45,0`, release with dev-tools on this Intel Mac, `caffeinate`, window frontmost, no screen saver or lock, at least five 120-frame windows after warmup, load average recorded.
  - **Baseline (2026-10-05), registry-resident particles:** `sim_ai` 5.8 ms/tick and `sim_steer_agents` 2.5 ms/tick at about 10k particles; `sim_ai` 1.05 ms/tick at zero. Re-measured on the pool before the pawn index lands.
  - **Target, re-confirmed on the pool:** `sim_ai` ≤ 1.0 ms/tick and `sim_steer_agents` ≤ 0.8 ms/tick.
  - **Upstream limit:** the GPU-bound smoke frame (about 82 ms) caps frame-time gains, so judge per-tick stage costs, not `total`.
  - **Cleanup:** sweep maps and their PRLs deleted.
- [ ] Agent scaling at 64 and 256 agents, from `--enemies` variants of the fixture, in two poses: spread patrols (the mixed pose) and a converged horde, one faction around one player. Per-agent `sim_ai` cost at 256 is at most 1.5× its cost at 64 in each.
- [ ] Engaged agents: the 6-room warren variant (`--preset warren --grid 3 2 1 --enemies 64`), posed among 11 agents. Before and after windows are recorded. This is the first engaged measurement; the baseline row in `research.md` is missing.

## Path
- **Pawn index.** Seams: `targeting::target_candidates`, `target_offers` and `select_target_with_attacker_ledger`. Build beside the `EnemySnapshot` pass in `run_ai_tick_with_host`: `LiveFactionSentiment` is constructed once there and the registry is borrowed immutably through `compute::evaluate`, so a pre-built index stays exact. The faction leaf is `EntityStateComponent`'s `FACTION_STATE_FIELD`, 0.0 when absent. The retained-target path, `target_candidate`, stays an O(1) lookup.
- **Early stop.** Today every candidate runs `CandidateScope::refresh`, the candidacy program and the perception callback. Confirm none has a side effect a skipped candidate would have produced. The latch needs the nearest eligible hostile, which is the walk's first hit.
- **Rival:** one all-pawn XZ grid returning the full candidate set in distance order. Faction-first wins because hostile buckets are usually small and the due-tick walk stops early; a grid can still live inside a large bucket.
- **Nav.** Seams are `NavGraph::region_at`, `resolve_region_at` and `region_snap_tolerance`. Regions carry world XZ rects over a 1 m grid (238×322 on this fixture). Also consider caching the planned destination's region on the agent when the plan is built, so `destination_topology_changed` resolves one point per tick, not two.
- **Stages.** The AI crate receives `SimStage` scopes through `AiTickInputs`, because sim owns the tick's stage set.
- **First slice:** the nav index, which does not wait for the pool; re-run the stress row for `sim_steer_agents`. For the pawn index, land the brute-force equivalence test before the index.
- `sim/src/sim/mod.rs` and `agent_steering.rs` are past 800 lines. Extend them in place, or split first in a behaviour-preserving commit.

## Open questions
- Re-confirm the `sim_ai` and `sim_steer_agents` targets against the baseline measured on the pool. — owner — **blocks build** of the pawn index only; the nav index may land first.
- Where the pawn index lives (AI crate versus sim), and the bucket size at which a bucket gains a grid. — **delegated**
