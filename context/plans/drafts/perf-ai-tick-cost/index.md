# perf-ai-tick-cost

Brief · compact · Epic 10 (AI at scale: exact cost cuts) · reads: `context/lib/entity_model.md` §1, §5, §7c, §9 · `context/lib/build_pipeline.md` §Navigation bake (gates and the replan budget) · `context/lib/ai_pathfinding_mt_readiness.md` · `context/lib/rendering_pipeline.md` §12 · `context/lib/testing_guide.md` §Resource bounds · read at ee1e78dcd

**Sequence:** the particle pool (`done/perf-particle-sim-cost`) has landed. This brief lands before `drafts/E10--ai-tick-scheduling`, which relies on its sub-stages, its budgets and its same-target-same-tick contract.

## Problem
Developer-observed defect. In a converged fight, AI cost grows faster than the crowd (`replan_stress_row`; baseline under Acceptance, profile in `research.md`). The profile splits it into two causes of about equal size.
- **Target scan is n².** Every enemy walks every pawn, its peers included, every tick (`targeting::target_candidates`), and on acquisition-due ticks refreshes a candidate scope for each one, reading each pawn's faction from a string-keyed map.
- **Combat positioning repeats placement work per enemy.** Every enemy on one target scores the same ring positions (`select_combat_positions_batch`). Grounding, the docked-mover test and the target-side route depend only on the target and the position, yet run once per enemy, and clearance scans every agent for every candidate.

Two shapes outside the fight compound it.
- `NavGraph::region_at` and `resolve_region_at` scan every nav region. Steering's replan gates and positioning's grounding call them per agent per tick, so steering cost is O(agents × regions).
- The candidate walk is O(registry slots), not O(pawns). With particles gone from the registry this no longer dominates, but projectiles, props and crates still stride the walk.

The need is anticipated for content: no shipped map fights at these counts today. The owner asked for the capacity (roadmap Epic 10, AI at scale).

When done:
- Per-enemy targeting cost tracks the hostile pawns it must inspect, not every pawn, and does not depend on registry size.
- Target-side placement work runs once per target and position per batch, not once per enemy.
- Point-to-region lookup does not scan the navmesh.
- Every enemy selects the same target, and claims the same combat slot, on the same tick as today.

## Decisions
- **Target candidates come from a faction-first pawn index, built once per AI pass.**
  - Pawns are player-movement holders plus brain holders, bucketed by faction once per pass. Each pawn's faction is read once per pass, and hostility resolves once per evaluating faction and bucket from the pass's live sentiment.
  - The think stride is priced from the nearest pawn in the evaluating enemy's hostile buckets only.
  - On due ticks the enemy walks hostile pawns in (distance, group, slot) order, group being today's movement-holders-first scan order, and stops at the first that passes candidacy and line of sight. Retaliation is checked against the brain's recent-attacker ledger: `targeting::retaliation_preference` admits only a candidate with an attacker record.
  - Engine-floor semantics in `entity_model.md` §7c are untouched: ranking, hysteresis, retaliation admission and latch, the candidacy predicate and the raw-distance stride price. §7c's "evaluated once per offered candidate" becomes "at most once, in ranking order", amended at promotion.
- **Combat positioning shares target-side placement work within a batch.**
  - Per batch, each (target, generated position, capsule placement) is grounded, tested against docked movers and checked for a tactically direct target route once. Slot-to-target line of sight stays per enemy, because it casts from each enemy's own eye offset. Each enemy also runs its own route and its own clearance test.
  - Clearance against other agents uses an exact per-batch proximity query, with the same spacing rule, claimant exclusion and non-finite handling.
  - Assignment order, hysteresis and the proposal loop are unchanged, so every enemy claims the same slot on the same tick.
- **`entity_model.md` §9 gains two named exceptions** to "Spatial partitioning for entity-entity queries (octree, grid)": the AI target-candidate index (faction-bucketed, exact) and the exact per-batch proximity query for combat-slot clearance. Every other entity-entity query, steering separation included, stays unindexed. Amended at promotion.
- **No cell-visibility broad phase in target selection.** `plans/done/E10--enemy-mp-target-selection` reserved the selection seam for one if enemy counts made selection hot. Rejected: the stride is priced on raw distance, which visibility cannot answer, and line of sight measures about 0% of AI cost.
- **Nav point-to-region lookup is indexed at load.**
  - `region_at` and the snapping `resolve_region_at` return exactly what the linear scans return, including stacked floors, both tie rules and off-mesh snap within tolerance, without visiting every region.
  - No PRL format change. A load-time build keeps the bake cache untouched and is reversible.
- **AI cost gets sub-stages.** `sim_ai` gains sub-stages for targeting, combat positioning, perception, graph evaluation, and apply with slots and turns, on the existing `SimStage` set. Labels follow `rendering_pipeline.md` §12.
- **Budgets live on the headless stress row.** `replan_stress_row` is the measurement: deterministic counts, no window, and it runs on any machine. The windowed `stress-env-volumes` row stays for steering, where the nav index is the change.
- **Non-goals.**
  - Registry occupancy index, a later brief justified by non-particle populations. Registry storage shape: `drafts/registry-column-storage`.
  - Decide and act rates, detail tiers and perception: `E10--ai-tick-scheduling`, which never chooses targets.
  - The shared replan budget's throughput. Scaling it changes paths, positions and later targets, so it would break this brief's identical-results rule. `E10--ai-tick-scheduling` owns it (owner, 2026-11-17).
  - Steering separation: not measured hot (`research.md` §Adjacent work).
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
- [ ] With 64 brains and 0 versus 60,000 non-pawn entities, every enemy selects the same target, and per-enemy targeting reads no registry column. Only the once-per-pass index build walks the registry, counted at the index build.
- [ ] Batched combat positioning matches the uncached batch, kept as a test-only reference, on every assignment (slot, score terms, generation index, incumbency) across randomized scenes: several targets, shared and distinct targets, chase and rotated rings, an incumbent inside and outside the switch margin, scarce slots, a candidate blocked by a peer at exactly the spacing, a docked mover over a slot, a non-finite peer position, two enemies with different eye offsets on one target where a low obstruction blocks a slot for only one of them, and positions that coincide across two standoffs with different generation indices.
- [ ] In a chase wave sharing one target and standoff, grounding, the docked-mover test and the target-side route each run once per distinct candidate position per batch, counted at the scoring seam. The count is the same for 1 and for 16 enemies.
- [ ] Indexed region lookup equals the linear scan for every nav grid cell centre and for randomized points, including stacked floors at one XZ, equal floor-band distances, points just inside and just outside the snap tolerance, points beyond the grid, points exactly on region min and max edges under a non-integral origin and a cell size other than 1 m, equal-XZ-distance snap ties between two regions, non-finite points, and a region rect reaching past the grid dimensions.
- [ ] On a grid of 4,000 or more regions, a lookup examines only the candidate regions near the point, never every region (P3).
- [ ] Same results over a run: the stress row's chase, strafe and strafe+hits infight waves at 32, 64 and 128 produce identical per-tick targets, retaliation latches and slot claims before and after the pawn index and the positioning cache (P2).
- [ ] With CPU timing on and an engaged enemy, the targeting, combat-positioning, perception, graph-evaluation and apply sub-stages each report under `sim_ai`, and the steering sub-stages under `sim_steering`, each no larger than its parent. Combat positioning reports absent on a tick with no engaged enemy. With timing off, none report.
- [ ] Existing AI, steering, positioning and nav suites pass unchanged.
### Manual
- [ ] **Stress row**, release, on the owner's Windows desktop, pinned per `testing_guide.md` §Resource bounds (`cargo test -p postretro-ai --release --lib -- --ignored replan_stress_row --nocapture --test-threads=1`). A size names the wave; each run adds 8 late chasers, and per-agent cost divides by all agents (72 at the 64 wave).
  - **Baseline (2026-11-10, same desktop):** median AI tick, chase 1.05 / 3.05 / 8.8 ms and strafe 0.58 / 1.43 / 4.0 ms at 32 / 64 / 128. Replan-wait lines are recorded per scenario and size at the pre-change commit.
  - **Target:** the 64 wave ≤ 1.0 ms median AI tick, and median AI tick per agent at the 128 wave at most 1.2× that at the 64 wave, for chase and strafe. 128 is reported. Replan-wait lines match the recorded baseline exactly.
  - Medians come from a timing-off run; sub-stages come from a second, timing-on run, so their scopes do not inflate the median.
  - **Ceiling:** after landing, the measured medians plus 25% become the ceiling later briefs hold.
- [ ] **Steering**, windowed: `stress-env-volumes.prl` recompiled from the committed map, mixed pose `114.60,2.44,-138.99,45,0`, release with dev-tools, at least five 120-frame windows after warmup, load average recorded. `sim_steer_agents` at 64 agents is re-measured before the nav index and must reach ≤ 0.8 ms/tick after it.

## Path
- **Landing order:** the nav index first (it waits on nothing), then the pawn index and positioning cache in either order.
- **Pawn index.** Seams: `targeting::target_candidates`, `target_offers`, `select_target_with_attacker_ledger` and `CandidateScope::refresh`, which should take the faction from the bucket rather than re-read it. Build beside the `EnemySnapshot` pass in `run_ai_tick_with_host`: `LiveFactionSentiment` is constructed once there and the registry is borrowed immutably through `compute::evaluate`, so a pre-built index stays exact. Do not reuse the snapshot pass's filter: it drops quiescent enemies, which stay candidates. Key buckets on exact faction bits. The retained-target path, `target_candidate`, stays an O(1) lookup.
- **Early stop.** `CandidateScope::refresh`, the candidacy program and the perception callback were checked for side effects a skipped candidate would have produced; none found (`research.md`). The latch needs the nearest eligible hostile, which is the walk's first hit. A ledger attacker still passes the pawn, inventory, self and exclude filters, and equal retaliation ranks still break in scan order.
- **Large buckets.** A grid inside a faction bucket can be added once that bucket is large, with identical results.
- **Positioning cache.** Seams: `select_combat_positions_batch`, `combat_candidates`, `rotated_ring::ring_candidates` and `scoring::score_candidate`. Split `score_candidate` into its shared half (`grounded_candidate_position`, `overlaps_docked_mover`, `has_tactically_direct_target_path`) and its per-enemy half (`dynamic_placement_is_clear`, `line_of_sight`, `find_path_from`). Key the cache on exact position bits, as `same_position_bits` already does. Rotated rings follow each enemy's bearing, so they share little (`research.md`).
- **Rival:** one all-pawn XZ grid returning the full candidate set in distance order. Faction-first wins because hostile buckets are usually small and the due-tick walk stops early; a grid can still live inside a large bucket.
- **Nav.** Seams are `NavGraph::region_at`, `resolve_region_at` and `region_snap_tolerance`. A cell → candidate-regions index over the existing grid is the expected shape. Also consider caching the planned destination's region on the agent when the plan is built, so `destination_topology_changed` resolves one point per tick, not two.
- **Stages.** The AI crate receives `SimStage` scopes through `AiTickInputs`, because sim owns the tick's stage set. The stress row needs harness work to read them.
- **First slice:** the nav index, then the brute-force and uncached-batch equivalence tests and the P2 digest before each cache.
- `sim/src/sim/mod.rs`, `agent_steering.rs` and `combat_positioning.rs` are past 800 lines. Extend them in place, or split first in a behaviour-preserving commit.

## Open questions
- Where the pawn index lives (AI crate versus sim), the bucket size at which a bucket gains a grid, and the clearance query's cell size. — **delegated**
