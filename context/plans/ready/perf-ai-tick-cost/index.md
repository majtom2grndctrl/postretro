# perf-ai-tick-cost

Brief · resumable · Epic 10 (AI at scale: exact cost cuts) · reads: `context/lib/entity_model.md` §1, §5, §7c, §9 · `context/lib/build_pipeline.md` §Navigation bake (gates and the replan budget) · `context/lib/ai_pathfinding_mt_readiness.md` · `context/lib/rendering_pipeline.md` §12 · `context/lib/testing_guide.md` §Resource bounds · read at efdab38d6

**Sequence:** the particle pool (`done/perf-particle-sim-cost`) has landed. This brief lands before `ready/E10--ai-tick-scheduling`, which relies on its sub-stages, its cost goals and its same-target-same-tick contract.

## Problem
Developer-observed defect. In a converged fight, AI cost grows faster than the crowd (`replan_stress_row`; baseline under Acceptance, profile in `research.md`). The profile splits it into two causes of about equal size.
- **Target scan is n².** Every enemy walks every pawn, its peers included, every tick (`targeting::target_candidates`), and on acquisition-due ticks refreshes a candidate scope for each one, reading each pawn's faction from a string-keyed map.
- **Combat positioning repeats placement work per enemy.** Every enemy on one target scores the same ring positions (`select_combat_positions_batch`). Grounding, the docked-mover test and the target-side route depend only on the target and the position, yet run once per enemy, and clearance scans every agent for every candidate.

On a realistic nav graph a third cause outweighs both. The stress row's graph is one region, so it hides it.
- **Every route query zeroes graph-sized search arrays.** `nav::find_path_from` allocates `g_score` and `came_from` sized to the graph's nodes and `rode_from` to its links before it searches, so each multi-region query costs O(portals) however short the route. Combat positioning runs one per candidate slot per enemy. On a four-arena map of about 4,000 regions this dominated the AI tick (`research.md` §Route query cost).

Two shapes outside the fight compound it.
- `NavGraph::region_at` and `resolve_region_at` scan every nav region. Steering's replan gates and positioning's grounding call them per agent per tick, so steering cost is O(agents × regions).
- The candidate walk is O(registry slots), not O(pawns). With particles gone from the registry this no longer dominates, but projectiles, props and crates still stride the walk.

The need is anticipated for content: no shipped map fights at these counts today. The owner asked for the capacity (roadmap Epic 10, AI at scale).

When done:
- Per-enemy targeting cost tracks the hostile pawns it must inspect, not every pawn, and does not depend on registry size.
- Target-side placement work runs once per target and position per batch, not once per enemy.
- Point-to-region lookup does not scan the navmesh.
- A route query costs what its search visits, not the size of the graph.
- Every enemy selects the same target, and claims the same combat slot, on the same tick as today.

## Decisions
- **Target candidates come from a faction-first pawn index, built once per AI pass.**
  - Pawns are player-movement holders plus brain holders, bucketed by faction once per pass. The candidate scan reads each pawn's faction once per pass, from the bucket, and hostility resolves once per evaluating faction and bucket from the pass's live sentiment.
  - The think stride is priced from the nearest pawn in the evaluating enemy's hostile buckets only.
  - On due ticks the enemy walks hostile pawns in (distance, group, slot) order, group being today's movement-holders-first scan order, and stops at the first that passes candidacy and line of sight. Retaliation is checked against the brain's recent-attacker ledger: `targeting::retaliation_preference` admits only a candidate with an attacker record.
  - Engine-floor semantics in `entity_model.md` §7c are untouched: ranking, hysteresis, retaliation admission and latch, the candidacy predicate and the raw-distance stride price. §7c's "evaluated once per offered candidate" is now "at most once, in ranking order" (amended at promotion).
- **Combat positioning shares target-side placement work within a batch.**
  - Per batch, each (target, generated position, capsule placement) is grounded, tested against docked movers and checked for a tactically direct target route once. Slot-to-target line of sight stays per enemy, because it casts from each enemy's own eye offset. Each enemy also runs its own route and its own clearance test.
  - Clearance against other agents uses an exact per-batch proximity query, with the same spacing rule, claimant exclusion and non-finite handling.
  - Assignment order, hysteresis and the proposal loop are unchanged.
- **`entity_model.md` §9 gains one named exception** to "Spatial partitioning for entity-entity queries (octree, grid)": the exact per-batch proximity query for combat-slot clearance. The faction-first pawn index buckets by category, not space, so §9 does not cover it. Recorded in §9 at promotion.
- **No cell-visibility broad phase in target selection.** `plans/done/E10--enemy-mp-target-selection` reserved the selection seam for one if enemy counts made selection hot. Rejected: the stride is priced on raw distance, which visibility cannot answer, and line of sight measures about 0% of AI cost.
- **Nav point-to-region lookup is indexed at load.**
  - `region_at` and the snapping `resolve_region_at` return exactly what the linear scans return, including stacked floors, both tie rules and off-mesh snap within tolerance, without visiting every region.
  - No PRL format change. A load-time build keeps the bake cache untouched and is reversible.
- **Route queries reuse search scratch.** A route query touches only the nodes it visits, and its routes, costs, tie order and failure cases are identical to today's. No PRL format change.
- **AI cost gets sub-stages.** `sim_ai` gains sub-stages for targeting (candidate line of sight included), combat positioning (its own stage, carved out of slot resolution), perception, graph evaluation, and apply with slots and turns, on the existing `SimStage` set. Labels follow `rendering_pipeline.md` §12.
- **Performance is measured, not gated.** Two headless measurements: `replan_stress_row` (packed waves, one-region graph) and a multi-arena row on a graph of about 4,000 regions. Both are deterministic, windowless and run on any machine. The windowed `stress-env-volumes` row stays for steering, where the nav index is the change. The cost goals below are reported against; a miss is planned as an addendum that adjusts performance levers, not a failed landing (owner). Identical results are gated.
- **Non-goals.**
  - Registry occupancy index, a later brief justified by non-particle populations. Registry storage shape: `drafts/registry-column-storage`.
  - Decide and act rates, detail tiers and perception: `E10--ai-tick-scheduling`, which never chooses targets.
  - The shared replan budget's throughput. Scaling it changes paths, positions and later targets, so it would break this brief's identical-results rule. `E10--ai-tick-scheduling` owns it.
  - Steering separation: not measured hot (`research.md` §Adjacent work).
  - Far-targeted enemies routing every candidate slot: `E10--ai-tick-scheduling`'s tactical range (authored `tacticalRange`, default 100 m) routes them toward the target instead. This brief cuts each search's cost, not the number of searches.
  - Route-search pruning by a lower bound on path length: deferred (owner). After scratch lands, the multi-arena row's positioning sub-stage shows whether route search still dominates; if so, an addendum adds it. Its bound must survive rides, riders on lifts and clearance standoffs (`research.md`).
  - Time-slicing perception or LOS: it changes reaction latency.
  - Parallel AI (`ai_pathfinding_mt_readiness.md`): the cost is repeated and pairwise work, not compute.

## Acceptance
### Automated
- [ ] Target selection matches a brute-force reference across randomized scenes, on the selected target, the retaliation latch, the stride-pricing distance and the order among equal-distance candidates. Today's scan stays as a test-only reference. The scenes cover:
  - one and several player pawns;
  - peer brains, and several factions with asymmetric sentiment;
  - a retained target, including a hysteresis tie;
  - a hostile ledger attacker, and a non-hostile pawn admitted only by retaliation over tolerance;
  - a candidacy predicate or failed line of sight rejecting the nearest several hostile pawns;
  - the evaluating enemy excluded;
  - zero candidates, and zero hostile buckets;
  - a quiescent brain (zero health, or despawn queued) that stays a candidate; an inventory-owned pawn; a pawn holding both player movement and a brain; a pawn with no transform; a pawn with no faction state; a pawn at a non-finite position;
  - pawns with a non-index faction value (NaN, 0.5), and -0.0 beside 0.0;
  - a ledger attacker that is not a pawn, despawned, or inventory-owned; a ledger attacker that is the retained target or the evaluating enemy; two over-tolerance ledger attackers with equal rank, recorded in the reverse of scan order.
- [ ] Across two consecutive AI ticks where the first tick's apply changes pawns, the second tick still matches the reference (P1).
- [ ] With several hostile pawns and a larger allied horde, a due-tick selection refreshes candidate facts for no allied pawn that lacks an attacker record, and for no hostile pawn farther than the first eligible one. Line of sight runs only for pawns that pass candidacy up to that one, plus ledger attackers. Counted at the candidate-refresh seam, the refresh count is the same with 0 and with 128 allied peers.
- [ ] With 0 versus 60,000 non-pawn entities, every enemy selects the same target, and registry column walks per AI pass (counted behind the registry's `test-support` feature) are the same for 1 and for 64 brains.
- [ ] Batched combat positioning matches the uncached batch, kept as a test-only reference, on every assignment (slot, score terms, generation index, incumbency) across randomized scenes: several targets, shared and distinct targets, chase and rotated rings, an incumbent inside and outside the switch margin, scarce slots, a candidate blocked by a peer at exactly the spacing, a docked mover over a slot, a non-finite peer position, two enemies with different eye offsets on one target where a low obstruction blocks a slot for only one of them, and positions that coincide across two standoffs with different generation indices.
- [ ] In a fresh chase batch with no incumbents, enemies clear of the ring, one shared target and standoff, grounding, the docked-mover test and the target-side route each run at most once per distinct candidate position, counted at the scoring seam, for 1 and for 16 enemies.
- [ ] Clearance examines only agents near the candidate, never every agent in the batch (P4).
- [ ] Across two consecutive batches where a mover docks over a slot and a gate closes between them, the second batch matches the uncached batch: nothing is kept across batches.
- [ ] Indexed region lookup equals the linear scan for every nav grid cell centre and for randomized points, including stacked floors at one XZ, equal floor-band distances, points just inside and just outside the snap tolerance, points beyond the grid, points exactly on region min and max edges under a non-integral origin and a cell size other than 1 m, equal-XZ-distance snap ties between two regions, non-finite points, and a region rect reaching past the grid dimensions.
- [ ] On a grid of 4,000 or more regions, a lookup examines only the candidate regions near the point, never every region (P3).
- [ ] Same results over a run: the stress row's chase, strafe and strafe+hits infight waves at 32, 64 and 128 produce identical per-tick targets, retaliation latches and slot claims before and after the pawn index and the positioning cache (P2).
- [ ] Route queries with reused scratch return the same path, cost and failure as a fresh-allocation reference, kept test-only, across randomized start and goal pairs on a graph of about 4,000 regions: stacked floors, closed gates, mover links and rides, off-mesh endpoints within and beyond snap tolerance, unreachable goals, and a query run straight after one that failed part way, and the orderings in P5.
- [ ] After a warm-up query, a route search between adjacent regions allocates nothing inside the search, with the `alloc_probe` window around the search step alone, before reconstruction and funnel. Per-node state initialized or reset by a search, counted at the scratch, is the same on the 4,000-region graph as on a 10-region graph for the same two-region route.
- [ ] With CPU timing on and an engaged enemy, the targeting, combat-positioning, perception, graph-evaluation and apply sub-stages each report under `sim_ai`, and the steering sub-stages under `sim_steering`, each no larger than its parent. Combat positioning reports absent on a tick with no engaged enemy (regression guard: positioning already returns before any batch then). With timing off, none report.
- [ ] Existing AI, steering, positioning and nav suites pass unchanged.
### Manual
- [ ] **Stress row**, release, on the owner's Windows desktop, pinned per `testing_guide.md` §Resource bounds (`cargo test -p postretro-ai --release --lib -- --ignored replan_stress_row --nocapture --test-threads=1`). A size names the wave; per-agent cost divides by all agents, late chasers included.
  - **Baseline:** medians, replan-wait lines and the P2 digest are recorded per scenario and size at the pre-change commit, after `E10--enemy-stances-and-telegraphs` merges (its chase leash changes chase positions and replans), on the same desktop. For reference, 2026-11-10 before that merge: chase 1.05 / 3.05 / 8.8 ms and strafe 0.58 / 1.43 / 4.0 ms at 32 / 64 / 128.
  - **Goal, reported:** the 64 wave ≤ 1.0 ms median AI tick, and median AI tick per agent at the 128 wave at most 1.2× that at the 64 wave, for chase and strafe. 128 is reported, AI and steering medians both. Replan-wait lines match the recorded baseline exactly (gated: identical results).
  - Medians come from a timing-off run; sub-stages come from a second, timing-on run, so their scopes do not inflate the median.
- [ ] **Multi-arena row**, release, same desktop: four arenas of at least 64×64 m on a graph of about 4,000 regions, a spread wave of 64 with 436 off-arena enemies (500 placed) and of 128 with 872 (1,000 placed), off-arena split idle, patrolling and far-targeted. Each role's cost is the difference between whole-pass runs (wave alone, then adding idle and patrol, then far), so no hook enters the AI pass. AI and steering medians and the positioning sub-stage are reported before and after each change. Spike references are in `research.md`.
- [ ] **Steering**, windowed: `stress-env-volumes.prl` recompiled from the committed map, mixed pose `114.60,2.44,-138.99,45,0`, release with dev-tools, at least five 120-frame windows after warmup, load average recorded. `sim_steer_agents` at 64 agents is re-measured before the nav index and reported after it, against a goal of ≤ 0.8 ms/tick.

## Path
- **Pawn index.** Seams: `targeting::target_candidates`, `target_offers`, `select_target_with_attacker_ledger` and `CandidateScope::refresh`, which should take the faction from the bucket rather than re-read it. Build beside the `EnemySnapshot` pass in `run_ai_tick_with_host`: `LiveFactionSentiment` is constructed once there and the registry is borrowed immutably through `compute::evaluate`, so a pre-built index stays exact. Do not reuse the snapshot pass's filter: it drops quiescent enemies, which stay candidates. Key buckets on exact faction bits. The retained-target path, `target_candidate`, stays an O(1) lookup.
- **Early stop.** `CandidateScope::refresh`, the candidacy program and the perception callback were checked for side effects a skipped candidate would have produced; none found (`research.md`). The latch needs the nearest eligible hostile, which is the walk's first hit. A ledger attacker still passes the pawn, inventory, self and exclude filters, and equal retaliation ranks still break in scan order.
- **Positioning cache.** Seams: `select_combat_positions_batch`, `combat_candidates`, `rotated_ring::ring_candidates` and `scoring::score_candidate`. Split `score_candidate` into its shared half (`grounded_candidate_position`, `overlaps_docked_mover`, `has_tactically_direct_target_path`) and its per-enemy half (`dynamic_placement_is_clear`, `line_of_sight`, `find_path_from`). Key the cache on exact position bits, as `same_position_bits` already does. Rotated rings follow each enemy's bearing, so they share little (`research.md`).
- **Rival:** one all-pawn XZ grid returning the full candidate set in distance order. Faction-first wins because hostile buckets are usually small and the due-tick walk stops early; a grid can still live inside a large bucket.
- **Nav.** Seams are `NavGraph::region_at`, `resolve_region_at` and `region_snap_tolerance`. A cell → candidate-regions index over the existing grid is the expected shape; the spike's throwaway CSR index matched the linear scans on 20k points (`ready/E10--ai-tick-scheduling/spike-visible-rates.md`; patch in git history).
- **Route scratch.** Seam: `nav/path.rs` search setup (`g_score`, `came_from`, `rode_from`, the open heap) behind every A* entry point (`find_path`, `find_path_gated`, `find_path_from` and the reachability and positioning queries built on them). A generation stamp is the expected reset; every per-node field resets on first touch, including `came_from` for a boarding node seeded from the start region. `NavGraph` is shared and immutable and no query runs inside another, so the scratch lives with the caller, one per AI pass and steering pass, which keeps the parallel-replan path in `ai_pathfinding_mt_readiness.md` open (one scratch per worker). The spike's multi-arena harness (`spike_arena_row`) is in the throwaway diff; its per-role AI filter is not to be kept.
- **Stages.** The AI crate receives `SimStage` scopes through `AiTickInputs`, because sim owns the tick's stage set. The stress row needs harness work to read them.
- **First slice and order:** the nav index and route scratch first (they wait on nothing), then the brute-force and uncached-batch equivalence tests and the P2 digest before each cache; the pawn index and positioning cache land in either order.
- `sim/src/sim/mod.rs`, `agent_steering.rs` and `combat_positioning.rs` are past 800 lines. Extend them in place, or split first in a behaviour-preserving commit.

## Open questions
- Where the pawn index lives (AI crate versus sim), whether and at what size a faction bucket gains a grid, and the clearance query's cell size. — **delegated**
