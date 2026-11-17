# perf-ai-tick-cost

Brief · compact · Epic 10 (AI at scale: exact cost cuts) · reads: `context/lib/entity_model.md` §1, §5, §7c, §9 · `context/lib/build_pipeline.md` §Navigation bake (gates and the replan budget) · `context/lib/ai_pathfinding_mt_readiness.md` · `context/lib/rendering_pipeline.md` §12 · `context/lib/testing_guide.md` §Resource bounds · read at ee1e78dcd

**Sequence:** the particle pool (`done/perf-particle-sim-cost`) has landed. This brief lands before `drafts/E10--ai-tick-scheduling`, which relies on its sub-stages, its budgets and its same-target-same-tick contract.

## Problem
Developer-observed defect. In a converged fight, AI cost grows faster than the crowd. The combat-movement stress row (`replan_stress_row`) measured a median AI tick of 1.0, 2.9 and 8.2 ms for 32, 64 and 128 chasers, which is about n^1.6. A callgrind profile at 64 and 128 chasers (`research.md`) splits it into two causes.
- **Target scan, n².** Every enemy scans every pawn every tick, its peers included. `targeting::target_candidates` walks every `PlayerMovement` and `Brain` holder for `target_offers`. On acquisition-due ticks (stride 1 inside 12 m, so almost every tick in a fight), `select_target_with_attacker_ledger` replays the walk and refreshes a `CandidateScope` for each candidate. Each refresh resolves hostility through `FactionRegistry::relationship`, which hashes string keys. This is half the tick at 128.
- **Combat positioning repeats target-side work.** Every enemy on the same target scores the same ring positions through `select_combat_positions_batch`. Grounding (two `NavGraph::region_at` scans plus a static capsule cast), the docked-mover test, the slot-to-target line of sight and the target-side route depend only on the target and the position, yet run once per enemy. Clearance against other agents (`dynamic_placement_is_clear`) scans every agent for every candidate. This is the other half of the tick, and the larger share at 64.

Two shapes outside the fight compound it.
- `NavGraph::region_at` and `resolve_region_at` scan every nav region. Steering's replan gates resolve two points per moving agent per tick through them (`admit_replans`), so steering cost is O(agents × regions): 4,326 regions on `stress-env-volumes`.
- The candidate walk is O(registry slots), not O(pawns). With particles gone from the registry this no longer dominates, but projectiles, props and crates still stride the walk.

The need is anticipated for content: no shipped map fights at these counts today. The owner asked for the capacity (roadmap Epic 10, AI at scale).

When done:
- Per-enemy targeting cost tracks the hostile pawns it must inspect, not every pawn, and does not depend on registry size.
- Target-side positioning work runs once per target and position per batch, not once per enemy.
- Point-to-region lookup does not scan the navmesh.
- Every enemy selects the same target, and claims the same combat slot, on the same tick as today.

## Decisions
- **Target candidates come from a faction-first pawn index, built once per AI pass.**
  - Pawns are player-movement holders plus brain holders, bucketed by faction once per pass. Hostility resolves once per evaluating faction and bucket, from the pass's live sentiment, so no per-pair relationship lookup remains.
  - The think stride is priced from the nearest pawn in the evaluating enemy's hostile buckets only.
  - On due ticks the enemy walks hostile pawns in (distance, group, slot) order, group being today's movement-holders-first scan order, and stops at the first that passes candidacy and line of sight. Retaliation is checked directly against the ≤ 8 recent-attacker ledger entries (`RECENT_ATTACKER_LEDGER_CAPACITY`): `targeting::retaliation_preference` admits only a candidate with an attacker record.
  - A due tick therefore inspects hostile pawns up to the first eligible one, plus ledger attackers, not the full candidate set.
  - A grid inside a faction bucket is added only once that bucket is large, as an internal refinement with identical results.
  - Engine-floor semantics in `entity_model.md` §7c are untouched: ranking, hysteresis, retaliation admission and latch, the candidacy predicate and the raw-distance stride price.
- **Combat positioning shares target-side work within a batch.**
  - Per batch, each (target, generated position, capsule placement) is grounded and tested once: grounding, the docked-mover test, slot-to-target line of sight and the target-side route check. Each enemy still runs its own route from its position and its own clearance test.
  - The rotated reposition ring shares the same cache wherever its generated positions coincide; it needs no ring of its own.
  - Clearance against other agents reads a per-batch spatial bucket of agent positions, with the same spacing rule, claimant exclusion and non-finite handling.
  - Assignment order, hysteresis and the proposal loop are unchanged, so every enemy claims the same slot on the same tick.
- **`entity_model.md` §9 gains two named exceptions** to "Spatial partitioning for entity-entity queries (octree, grid)": the AI target-candidate index (faction-bucketed, exact) and the per-batch combat-positioning clearance bucket. Every other entity-entity query, steering separation included, stays unindexed. Amended at promotion.
- **No cell-visibility broad phase in target selection.** `plans/done/E10--enemy-mp-target-selection` reserved the selection seam for one if enemy counts made selection hot. Rejected: the stride is priced on raw distance, which visibility cannot answer, and line of sight measures about 0% of AI cost. Perception-driven detail levels belong to `E10--ai-tick-scheduling`, which never chooses targets.
- **Nav point-to-region lookup is indexed at load.**
  - `NavGraph` builds a cell → candidate-regions index over its existing grid when constructed.
  - `region_at` and the snapping `resolve_region_at` return exactly what the linear scans return, including stacked floors, the equal-band lower-index tie and off-mesh snap within tolerance.
  - No PRL format change. A load-time build keeps the bake cache untouched and is reversible.
- **AI cost gets sub-stages.**
  - `sim_ai` gains sub-stages for targeting, combat positioning, perception, graph evaluation, and apply with slots and turns, on the existing `SimStage` set.
  - `sim_steering` keeps its `sim_steer_agents`, `sim_steer_pose` and `sim_steer_movers` split. Labels follow `rendering_pipeline.md` §12.
- **Budgets live on the headless stress row.** `replan_stress_row` is the measurement: deterministic counts, no window, and it runs on any machine. The windowed `stress-env-volumes` row stays for steering, where the nav index is the change.
- **Landing order:** the nav index first (it waits on nothing), then the pawn index and positioning cache in either order. The registry occupancy index stays its own later brief.
- **Non-goals.**
  - Registry occupancy index, a later brief justified by non-particle populations. Registry storage shape: `drafts/registry-column-storage`.
  - Decide and act rates, detail tiers and perception: `E10--ai-tick-scheduling`.
  - The shared replan budget's throughput (4 per tick; the fair wait reaches about 0.4 s at 128 chasers). Scaling it changes paths, positions and later targets, so it would break this brief's identical-results rule. `E10--ai-tick-scheduling` owns it (owner, 2026-11-17).
  - Steering separation (`agent_steering::separation`), the same pairwise shape as positioning clearance. Steering grew about linearly to 128 chasers in the profile, so it is not measured hot, and an exact bucket must keep separation's snapshot-order summation. If it turns hot, it can share the clearance bucket under that constraint.
  - Time-slicing perception or LOS: measured at about 0% of AI cost, and it would change reaction latency.
  - Changing think-stride bands, ring shape or slot scoring.
  - Parallel AI (`ai_pathfinding_mt_readiness.md`): the cost is repeated and pairwise work, not compute.

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
- [ ] Batched combat positioning matches the uncached batch on every assignment (slot, score terms, generation index, incumbency) across randomized scenes: several targets, shared and distinct targets, chase and rotated rings, an incumbent inside and outside the switch margin, scarce slots, a candidate blocked by a peer at exactly the spacing, a docked mover over a slot, and a non-finite peer position.
- [ ] In a wave of N enemies on one target, the target-side tests run once per generated position per batch, not N times.
- [ ] Indexed region lookup equals the linear scan for every nav grid cell centre and for randomized points, including stacked floors at one XZ, equal floor-band distances, points just inside and just outside the snap tolerance, and points beyond the grid.
- [ ] Same results over a run: the stress row's chase and strafe waves at 32, 64 and 128 produce identical per-tick targets and slot claims before and after the pawn index and the positioning cache.
- [ ] With CPU timing on, the AI sub-stages appear under `sim_ai` and the steering sub-stages under `sim_steering`, each no larger than its parent. With timing off, none report.
- [ ] Existing AI, steering, positioning and nav suites pass unchanged.
### Manual
- [ ] **Stress row**, release, on the owner's Windows desktop, pinned per `testing_guide.md` §Resource bounds (`cargo test -p postretro-ai --release --lib -- --ignored replan_stress_row --nocapture --test-threads=1`):
  - **Baseline (2026-11-10, same desktop):** median AI tick, chase 1.05 / 3.05 / 8.8 ms and strafe 0.58 / 1.43 / 4.0 ms at 32 / 64 / 128.
  - **Target:** 64 chasers ≤ 1.0 ms median AI tick, and per-enemy cost at 128 at most 1.2× its cost at 64, for chase and strafe. 128 is reported. Replan waits are reported and must match the baseline exactly.
  - The row reports the sub-stages, so the remaining cost is attributed.
- [ ] **Steering**, windowed: `stress-env-volumes.prl` recompiled from the committed map, mixed pose `114.60,2.44,-138.99,45,0`, release with dev-tools, at least five 120-frame windows after warmup, load average recorded. `sim_steer_agents` at 64 agents is re-measured before the nav index and must reach ≤ 0.8 ms/tick after it.

## Path
- **Pawn index.** Seams: `targeting::target_candidates`, `target_offers` and `select_target_with_attacker_ledger`. Build beside the `EnemySnapshot` pass in `run_ai_tick_with_host`: `LiveFactionSentiment` is constructed once there and the registry is borrowed immutably through `compute::evaluate`, so a pre-built index stays exact. The faction leaf is `EntityStateComponent`'s `FACTION_STATE_FIELD`, 0.0 when absent. The retained-target path, `target_candidate`, stays an O(1) lookup.
- **Early stop.** Today every candidate runs `CandidateScope::refresh`, the candidacy program and the perception callback. Confirm none has a side effect a skipped candidate would have produced. The latch needs the nearest eligible hostile, which is the walk's first hit.
- **Positioning cache.** Seams: `select_combat_positions_batch`, `combat_candidates`, `rotated_ring::ring_candidates` and `scoring::score_candidate`. Split `score_candidate` into its target-side half (`grounded_candidate_position`, `overlaps_docked_mover`, `line_of_sight`, `has_tactically_direct_target_path`) and its agent-side half (`dynamic_placement_is_clear`, `find_path_from`). Key the cache on exact position bits, as `same_position_bits` already does. Check whether any target-side input differs per enemy (eye offset, target aim) before sharing it; if so, key on it too.
- **Rival:** one all-pawn XZ grid returning the full candidate set in distance order. Faction-first wins because hostile buckets are usually small and the due-tick walk stops early; a grid can still live inside a large bucket.
- **Nav.** Seams are `NavGraph::region_at`, `resolve_region_at` and `region_snap_tolerance`. Regions carry world XZ rects over a 1 m grid. Positioning's grounding also calls `region_at`, so the index helps both. Also consider caching the planned destination's region on the agent when the plan is built, so `destination_topology_changed` resolves one point per tick, not two.
- **Stages.** The AI crate receives `SimStage` scopes through `AiTickInputs`, because sim owns the tick's stage set. The stress row reads the same scopes.
- **First slice:** the nav index, then the brute-force and uncached-batch equivalence tests before each cache.
- `sim/src/sim/mod.rs`, `agent_steering.rs` and `combat_positioning.rs` are past 800 lines. Extend them in place, or split first in a behaviour-preserving commit.

## Open questions
- Where the pawn index lives (AI crate versus sim), the bucket size at which a bucket gains a grid, and the clearance bucket's cell size. — **delegated**
