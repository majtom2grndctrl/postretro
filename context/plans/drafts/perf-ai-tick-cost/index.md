# perf-ai-tick-cost

Brief · compact · reads: `context/lib/entity_model.md` §1, §5, §7c · `context/lib/ai_pathfinding_mt_readiness.md` · `context/lib/rendering_pipeline.md` §12 · `context/lib/testing_guide.md` §Resource bounds · read at 90c9b9687

## Problem
Developer-observed defect from the E24 stress pre-flight. On `stress-env-volumes`, 64 agents cost about 5.8 ms per tick in `sim_ai` and 2.5 ms per tick in steering. The fixed step spirals at 128 agents. The cause is the enemy target scan, `targeting::target_candidates`, run for every enemy every tick through `target_offers`. It walks every slot of the entity registry twice (`EntityRegistry::iter_with_kind` over `PlayerMovement` and `Brain`), so AI cost is O(enemies × registry slots), and on this fixture particles inflate the slot count about 11× (`research.md`). Steering repeats the shape. `agent_steering::admit_replans` resolves two nav regions per moving agent per tick, each through a linear scan of every nav region (`NavGraph::region_at`).

When done:
- Per-agent AI cost no longer depends on how many unrelated entities the registry holds.
- The candidate scan grows sub-quadratically with pawn count.
- Point-to-region lookup does not scan the navmesh.
- Every enemy selects the same target, on the same tick, as today.

## Decisions
- **Column iteration is O(members), in slot order.**
  - The registry keeps a per-kind occupancy index, updated at every write that changes membership (set, remove, despawn, clear, deserialize).
  - `iter_with_kind` and its siblings visit only members, still in slot-index order. Every existing caller keeps its deterministic order and gains the speedup.
  - Storage layout and the mutation API are unchanged. That stays inside the `perf-billboard-emitter` non-goal on storage redesign.
- **Target candidates come from a per-tick pawn index, not the registry.**
  - Pawns are player-movement holders plus brain holders.
  - The index is built once per AI pass. It answers two questions exactly: the nearest hostile candidate, which prices the stride, and the full candidate set in distance order on due ticks.
  - It uses a uniform XZ grid with ring search. The result must equal the brute-force scan's, tie order included.
  - Hostility is resolved from a faction-index table built once per pass, not a hashed lookup per pair.
  - Engine-floor semantics in `entity_model.md` §7c are untouched: ranking, hysteresis, retaliation admission, the candidacy predicate and the raw-distance stride price.
- **Nav point-to-region lookup is indexed at load.**
  - `NavGraph` builds a cell → candidate-regions index over its existing grid when constructed.
  - `region_at` and the snapping `resolve_region_at` return exactly what the linear scans return. That includes stacked floors and off-mesh snap within tolerance.
  - No PRL format change. A load-time build keeps the bake cache untouched and is reversible.
- **AI cost gets sub-stages.**
  - `sim_ai` gains sub-stages for targeting, perception, graph evaluation, and apply with combat slots, on the existing `SimStage` set.
  - `sim_steering` keeps the `sim_steer_agents`, `sim_steer_pose` and `sim_steer_movers` split this session added. Labels follow `rendering_pipeline.md` §12.
- **Non-goals.**
  - Time-slicing perception or LOS: measured at about 0% of AI cost, and it would change reaction latency.
  - Changing think-stride bands.
  - Parallel AI (`ai_pathfinding_mt_readiness.md`): the cost is a cache-hostile walk, not compute.
  - Removing particles from the registry: `perf-particle-sim-cost` owns that, and neither brief blocks the other.

## Acceptance
### Automated
- [ ] Target selection matches a brute-force reference across randomized scenes, on the selected target, the stride-pricing distance and the order among equal-distance candidates. The scenes cover:
  - one and several player pawns;
  - peer brains;
  - a retained target, including a hysteresis tie;
  - a non-hostile pawn admitted only by retaliation over tolerance;
  - a candidacy predicate that rejects the nearest pawn;
  - the evaluating enemy excluded;
  - zero candidates.
- [ ] Iterating any component kind yields the same ids, in the same order, as a full slot walk after each of these: spawn, set, remove, despawn with slot reuse, generation retirement, level clear and deserialize.
- [ ] Iteration cost does not depend on unrelated entities: iterating 64 brain holders in a registry that also holds 60,000 entities of other kinds stays within a stated bench threshold.
- [ ] Indexed region lookup equals the linear scan for every nav grid cell centre and for randomized points, including:
  - stacked floors at one XZ;
  - points just inside and just outside the snap tolerance;
  - points beyond the grid.
- [ ] With CPU timing on, the AI sub-stages appear under `sim_ai` and the steering sub-stages under `sim_steering`, each no larger than its parent. With timing off, none report.
- [ ] Existing AI, steering and nav suites pass unchanged.
### Manual
- [ ] Stress, pinned per `testing_guide.md` §Resource bounds:
  - **Run:** `stress-env-volumes.prl` recompiled from the committed map, mixed pose `114.60,2.44,-138.99,45,0`, release with dev-tools on this Intel Mac, `caffeinate`, window frontmost, no screen saver or lock, at least five 120-frame windows after warmup, load average recorded.
  - **Baseline (2026-10-05):** `sim_ai` 5.8 ms/tick and `sim_steer_agents` 2.5 ms/tick, at about 10k particles.
  - **Proposed budget:** `sim_ai` ≤ 1.0 ms/tick and `sim_steer_agents` ≤ 0.8 ms/tick.
  - **Upstream limit:** the GPU-bound smoke frame (about 82 ms) caps frame-time gains, so judge per-tick stage costs, not `total`.
  - **Cleanup:** sweep maps and their PRLs deleted.
- [ ] Registry independence: the same run with the emitter rate swept to give about 1k and about 20k live particles. `sim_ai` per tick differs by no more than 15% between the two.
- [ ] Agent scaling at 64 and 256 agents, from `--enemies` variants of the fixture. Per-agent `sim_ai` cost at 256 is at most 1.5× its cost at 64.
- [ ] Engaged agents: the 6-room warren variant (`--preset warren --grid 3 2 1 --enemies 64`), posed among 11 agents. Before and after windows are recorded. This is the first engaged measurement; the baseline row in `research.md` is missing.

## Path
- **Registry.** Seams are `EntityRegistry::iter_with_kind`, `query_by_component_and_tag` and `for_each_with_kind_mut`. A per-kind word bitset over slots is the leading shape. Membership writes are `set_component*`, `remove_component*`, `despawn`, `spawn` (which seeds three kinds) and clear and deserialize. Rival: an EntityId list per kind, which would need sorting to keep slot order.
- **Pawn index.** Seams are `targeting::target_candidates`, `target_offers` and `select_target_with_attacker_ledger`. Build the index beside the `EnemySnapshot` pass in `run_ai_tick_with_host`. The retained-target path, `target_candidate`, stays an O(1) lookup. Rival: only the occupancy index, leaving the scan O(enemies × pawns). Measured, that would be fine at 64 agents but not at hundreds.
- **Nav.** Seams are `NavGraph::region_at`, `resolve_region_at` and `region_snap_tolerance`. Regions carry world XZ rects over a 1 m grid (238×322 on this fixture). Also consider caching the planned destination's region on the agent when the plan is built, so `destination_topology_changed` resolves one point per tick, not two.
- **Stages.** The AI crate receives `SimStage` scopes through `AiTickInputs`, because sim owns the tick's stage set.
- **First slice:** the occupancy index alone. Re-run the registry-independence row. It should close most of the 4.7 ms/tick particle-driven gap before the pawn index lands.
- `sim/src/sim/mod.rs` and `agent_steering.rs` are past 800 lines. Extend them in place, or split first in a behaviour-preserving commit.

## Open questions
- Accept the proposed budgets (`sim_ai` ≤ 1.0 ms/tick and `sim_steer_agents` ≤ 0.8 ms/tick at 64 agents on this Mac)? Recommendation: yes. Zero particles already measures 1.05 ms/tick for AI, while still paying about 1k-slot walks. — owner — **blocks build**
- Should the registry occupancy index land here, or as its own brief ahead of both perf briefs? Recommendation: here. AI is its largest measured consumer, and its first-consumer proof is the registry-independence row. — owner — **blocks build**
- Where the pawn index lives (AI crate versus sim) and its grid cell size. — **delegated**
