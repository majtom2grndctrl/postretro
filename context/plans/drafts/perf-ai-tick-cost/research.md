# perf-ai-tick-cost — research

Measured at 90c9b9687 plus the `sim_steer_*` sub-stages, since committed; source re-read at 2095e74c6. Findings that inform the brief but do not decide it.

## Measurement snapshot (2026-10-05)

**Setup.**
- Machine: MacBook Pro, Intel i9-9980HK (8 cores / 16 threads), 32 GB, AMD Radeon Pro 5300M, Metal.
- Build: `--release --features dev-tools`, rebuilt with `CARGO_PROFILE_RELEASE_STRIP=none CARGO_PROFILE_RELEASE_DEBUG=line-tables-only` for symbols. Codegen matches the standard release profile.
- Runs: windowed, `caffeinate -dimsu`, engine window frontmost (checked mid-run), `pgrep -x ScreenSaverEngine` empty before and after every run.
- Load average: 16 at the start of the first baseline run (other agents were building), then 2–6 for every later run.
- Fixture: `stress-env-volumes.prl` recompiled from the committed `.map`, mixed measurement pose `114.60,2.44,-138.99,45,0`.
- Variant maps came from `tools/gen_stress_map.py --preset env-volumes --env-volumes 0` with one knob changed. They were deleted after measurement.
- Stage values are window averages. `sim_*` values are divided by `ticks` to give per-tick figures.
- Profiler: Instruments Time Profiler (`xctrace record --template 'Time Profiler'`), 60 s, 1 ms sampling, main thread only, about 42.7k samples.

**Where `sim_ai` goes.** The table shares the samples under `run_ai_tick_with_host`, with 64 agents and about 10k particles.

| Bucket | Share of AI | ≈ ms per tick (of 5.8) |
|---|---|---|
| `targeting::target_candidates` registry slot walk (`target_offers` every tick, replayed by `select_target_with_attacker_ledger` on due ticks) | 84% | 4.9 |
| └ of which leaf `Option::as_ref` on `Vec<Option<ComponentValue>>` cells (`EntityRegistry::iter_with_kind`) | 50% | 2.9 |
| Faction sentiment lookups and hashing (`LiveFactionSentiment::sentiment`, SipHash) | 6.7% | 0.39 |
| Guard and transition evaluation (`eval_value`, `select_transition_path`, scope refresh) | 0.7% | 0.04 |
| `nav::find_path` reachability | 0.5% | 0.03 |
| `resolve_combat_slots` and `apply_outcomes` | 0.8% | 0.05 |
| LOS raycasts (`collision::line_of_sight`) | ~0% | — |
| Brain snapshot clone, program sync, allocation | <1% | — |

**Where `sim_steering` goes.** The new sub-stages give, per tick at 64 agents: `sim_steer_agents` 2.5 ms, `sim_steer_pose` 0.33 ms and `sim_steer_movers` 0.33 ms. The profile splits `sim_steer_agents` as follows.

| Bucket | Share | Note |
|---|---|---|
| `agent_steering::admit_replans` → `destination_topology_changed` / `blocked_destination_now_directly_routable` → `NavGraph::resolve_region_at` → `region_at` | ~80% | linear scan over all 4,326 nav regions; two lookups per agent with a destination, every tick |
| `collide_and_slide` (`cast_capsule_parry`) | ~13% | |
| `Agent` column slot walk | ~4% | |
| Separation, replans (`find_path`) | ~0% | |

**Agent scaling.** Mixed pose, about 10k particles, about 11.2k registry slots. Agents mostly patrol; at most one held a target.

| Agents | `sim_ai` ms/tick | `sim_steer_agents` ms/tick | `sim_tick` ms/tick | frame ms |
|---|---|---|---|---|
| 8 | 0.80 | 0.30 | 2.26 | 82 (GPU-bound, work 28) |
| 16 | 1.45 | 0.58 | 3.24 | 82 (GPU-bound) |
| 32 | 2.90 | 1.09 | 5.30 | 91 |
| 64 | 5.8 | 2.55 | 9.8 | 89 |
| 128 | 10.5–11.0 | 3.8 | 15.7–16.2 | 200–243 (fixed-step spiral, 11–14 ticks per frame) |

- Both stages are linear in agent count.
- AI costs about 90 µs per agent per tick at 11.2k slots. The cost per agent is O(registry slots), not O(agents), so the quadratic term in the candidate scan (agents × pawns) is hidden today.
- Steering costs about 40 µs per moving agent per tick, which is O(nav regions).

**Registry-size dependence.** 64 agents at the same pose. Only the particle count changes.

| Live particles | Registry slots | `sim_ai` ms/tick | `sim_tick` ms/tick |
|---|---|---|---|
| 0 | ~960 | 1.05 | 3.4 |
| 1,000 | ~2,040 | 1.49 | 3.8 |
| 4,920 | ~6,120 | 3.28 | 5.9 |
| ~10,000 | ~11,200 | 5.8 | 9.8 |
| ~19,300 | ~22,000 | 9.3–10.2 | 14.0–15.2 |

- `sim_ai` grows by about 0.43 µs per registry slot per tick. That is about 7 ns per slot per agent.
- Every other column-walking sim stage grows too, between 0 and 10k particles: `sim_steer_pose` 0.07 → 0.32 ms, `sim_steer_movers` 0.07 → 0.32 ms, `sim_triggers` 0.04 → 0.21 ms, `sim_weapons` 0.014 → 0.10 ms, `sim_projectiles` 0.009 → 0.095 ms.

**Idle versus engaged.**

| Pose | Held targets | `sim_ai` ms/tick | `particle_sim` ms/frame |
|---|---|---|---|
| mixed | 1 | 5.6–5.9 | 3.5–3.8 |
| water-only zone (no agents within 100 m) | 0 | 5.5 → 7.1 over the run | 3.8 → 4.3 |
| arena corner | 0 | 4.5 → 7.5 over the run | 3.2 → 4.5 |

- Within these runs the growth over a run is machine-wide, not AI-specific. `particle_sim`, `render` and `render_submit`, none of which depend on AI, rose by the same factor (about 1.3×). The `sim_ai`/`particle_sim` ratio stayed at 1.6–1.65.
- With zero particles, every stage, render included, rose about 1.7× over 80 s. That fits the Mac regime drift in `rendering_pipeline.md` §12 (machine-state confounders).
- **Not measured: genuinely engaged agents.** The engage variant (6-room warren, 11 agents within 15 m of the pose) never produced a window, because the screen locked and the engine renders nothing while locked.
- From source, an engaged near agent adds these per tick, all measured at under 1% of AI time with one engaged agent:
  - one `line_of_sight` ray;
  - a `find_path` reachability query on every acquisition-due tick (stride 1 within 12 m);
  - budgeted replans (`REPLAN_BUDGET_PER_TICK` = 4).
- The previously reported growth "as agents converge" is therefore unconfirmed. Re-measure it before treating engagement as a cost driver.

## Cause chain

- `EntityRegistry` stores each kind as `Vec<Option<ComponentValue>>`, indexed by slot. `size_of::<Option<ComponentValue>>()` = 800 bytes.
- `iter_with_kind` walks every slot and reads each live slot's cell in the column. A walk of a sparse column therefore costs O(registry slots) and touches one 800-byte-strided cell per live entity.
- `target_candidates` chains two such walks (`PlayerMovement`, `Brain`). For each `Brain` holder it also calls `get_component::<PlayerMovementComponent>`.
- It runs once per enemy per tick (`target_offers`), and again on acquisition-due ticks.
- Particles are registry entities, so the slot count is about 11× the gameplay entity count on this fixture.
- `NavGraph::region_at` is a linear scan over every region. `resolve_region_at` adds a second linear snap scan when the point is off-mesh.

## Established techniques considered

General engine practice, not re-verified against sources this session.
- **Occupancy bitset per component kind** (as in `hibitset` from specs and legion). Iteration visits set bits in slot order, at O(slots/64 + members). Slot-order determinism is preserved, which a swap-remove sparse set (EnTT-style) would not give. Moved to the last-landing occupancy-index follow-up.
- **Uniform grid / spatial hash for proximity queries** (Detour crowd `dtProximityGrid`, most crowd and boids systems). It is rebuilt each tick from pawn positions. A ring search in order of distance returns the exact nearest result.
- **Grid or BV-tree point-to-polygon lookup on a navmesh** (Detour `findNearestPoly` uses a per-tile BV tree). A navmesh that is already grid-rasterised maps directly to a cell → region list.
- **Time-sliced or budgeted perception and LOS** (Unreal AI Perception's per-frame sense budget). This does not apply: LOS is about 0% of measured cost. It also changes reaction latency, which is a behaviour change.
- **Parallel AI** (`ai_pathfinding_mt_readiness.md`). This does not apply: the measured cost is a cache-hostile registry walk, not compute, and the registry is single-threaded by contract.

## Faction-first pawn index

- `target_offers` prices the stride from hostile candidates only (`is_hostile` over the candidate's faction leaf). Non-hostile pawns stay in the raw scan only so selection can admit them through retaliation.
- `select_target_with_attacker_ledger` admits a non-hostile candidate only when `retaliation_preference` returns a rank, and that requires `has_attacker_record`, which `CandidateScope::refresh` sets from the brain's ledger. The ledger holds at most `RECENT_ATTACKER_LEDGER_CAPACITY` (8) entries. Retaliation candidates are therefore a subset of ledger attackers.
- Among non-retaliation candidates the winner is the nearest eligible hostile; ties go to the earlier candidate (`total_cmp(..).is_lt()`), i.e. movement holders before brain-only holders, then slot order. A walk in (distance, group, slot) order that stops at the first eligible hostile returns the same winner, and that winner is also the nearest eligible hostile the retaliation latch compares against.
- Hostility is a function of (evaluating faction, candidate faction) only, through `LiveFactionSentiment`, which `run_ai_tick_with_host` builds once per pass. So hostility resolves per bucket pair, not per pawn pair. The measured per-pair sentiment hashing (6.7% of AI) goes with it.
- The rival, a single all-pawn XZ grid, returns the full candidate set in distance order on due ticks. That is more than the selection rule needs. It also mixes allied pawns into every ring search. In a converged horde, the case the spread-patrol rows never measured, one faction surrounds one player: the hostile bucket holds one pawn and the allied bucket holds the horde.

## Rejected: cell-visibility broad phase

`plans/done/E10--enemy-mp-target-selection` (Decisions and Risks, "Selection cost") reserved the `visible`/selection seam for a view-independent cell-visibility broad phase "if enemy counts ever make it hot". It is the wrong tool here:
- The stride price is raw XZ distance to the nearest hostile, independent of visibility. A visibility filter cannot answer it.
- Line of sight measured about 0% of AI cost. The cost is the candidate walk.

## Registry occupancy index: a later brief

A per-kind occupancy index lands last, as its own brief, not here:
- Landing it before the particle pool would absorb the same particle-driven slot cost and void the pool's attribution row.
- Once particles leave the registry, this fixture's slot count is about 960, so a particle-sweep registry-independence row is vacuous. The follow-up proves itself on a synthetic non-particle population (projectiles, props, crates).

## Adjacent work

- `plans/done/E10--enemy-mp-target-selection`, `E10--enemy-aggro-model` and `E10--combat-perception-facts` define the selection semantics this brief must preserve: candidate order, hysteresis, retaliation and the stride price.
- `plans/done/perf-billboard-emitter`'s "columnar / batched registry mutation API" non-goal covered that plan's Slice 4 only. It is not a project rule. The storage mismatch is `drafts/registry-column-storage`'s.
- `drafts/perf-particle-sim-cost` removes particles from the registry. That removes this fixture's slot inflation, but not the O(slots) shape.
- `entity_model.md` §9 lists "Spatial partitioning for entity-entity queries (octree, grid)" as a non-goal. The pawn index is a named exception, amended at promotion.
