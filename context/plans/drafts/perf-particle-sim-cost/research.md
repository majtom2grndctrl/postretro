# perf-particle-sim-cost — research

Measured at 90c9b9687 plus the `particle_collect` stage, since committed; source re-read at 2095e74c6. Findings that inform the brief but do not decide it.

## Measurement snapshot (2026-10-05)

**Setup.** The method matches `perf-ai-tick-cost/research.md` (same machine, build, run discipline and profiler). Live particle counts come from a throwaway probe that summed the per-emitter survivor tally (`particle_live_counts`) once every 120 frames. It is reverted.

**Live count confirmed.** The fixture's 40 emitters × 50/s × 5 s held 9,840–10,000 live particles. The registry held about 10.9k live entities over about 11.2k slots; without particles it holds about 960.

**Where `particle_sim` goes.** 64 agents, mixed pose, about 10k particles. `particle_sim` measured 3.5–3.8 ms per frame. It runs once per rendered frame on `frame_dt`.

| Bucket (Time Profiler, under `particle_sim::tick`) | Share | ≈ ms per frame |
|---|---|---|
| `set_component::<SpriteVisual>` (an 800-byte `ComponentValue` cell rewritten, the old `String` dropped) | 31% | 1.1 |
| `set_component::<ParticleState>` | 23% | 0.8 |
| `set_component::<Transform>` | 8% | 0.3 |
| Loop body: snapshot `Vec`, `ParticleState` clone (two `Arc` bumps), `SpriteVisual` clone (a `String` allocation), integration, curve evaluation | 25% | 0.9 |
| Drop glue and allocator frames | ~6% | 0.2 |
| `despawn` of expired particles | 2.4% | 0.1 |
| `live_counts` `HashMap` updates | <1% | — |

- **Entity-storage churn accounts for about 70%.** That is the three cell rewrites, the drops and the allocator. Particle work is the remainder, and integration plus curve evaluation are a small part of it.
- The `HashMap` and clone hypotheses were measured and are minor.
- Spawn churn is small: `particle_emit` is 0.24 ms per frame at about 2,000 spawns per second, and despawn is 2.4% of the sim.

**Where `particle_collect` goes.** This stage is new, added by this session. It measured 2.8–2.9 ms per frame at about 10k particles.

| Bucket | Share |
|---|---|
| `LevelWorld::locate_cell` per particle, BSP plane tests included | ~77% |
| Collection `String` hashing (`sprite_to_collection` lookup) | ~8% |
| Packing and the visible-cell `HashSet` probe | remainder |

**Scaling.** Mixed pose, 64 agents, emitter rate swept with 40 emitters × 5 s lifetime.

| Live | `particle_emit` ms/frame | `particle_sim` ms/frame | `particle_collect` ms/frame | `sim_tick` ms/tick | ticks per frame | frame ms |
|---|---|---|---|---|---|---|
| 0 (`--emitters 0`) | 0.01 | 0.01 | 0.03 | 3.4 | 1.6 | 27 |
| 1,000 | 0.05 | 0.34 | 0.29 | 3.8 | 5.3 | 88 (GPU-bound, work 30) |
| 4,920 | 0.13 | 1.66 | 1.31 | 5.9 | 5.4 | 89 (GPU-bound, work 45) |
| ~10,000 | 0.24 | 3.5–3.8 | 2.8–2.9 | 9.8 | 5.4 | 89 |
| ~19,300 | 0.86 | 7.8–8.7 | 4.7–5.5 | 14.0–15.2 | 10.8–13.9 | 184–238 (fixed-step spiral) |

- Sim and collect are linear at about 0.4 µs and 0.28 µs per particle per frame.
- The 20k point used 500 particles per emitter, under the 4,096 per-emitter cap.

**Cost outside the particle stages.**
- **Fixed-tick slot inflation is the largest particle cost.**
  - Every slot-walking column iteration scales with registry slots, and particles are about 90% of slots.
  - `sim_tick` rises from 3.4 to 9.8 ms per tick between 0 and 10k particles. At about 5.3 ticks per frame that is about 34 ms per frame, several times the particle stages' own 6.6 ms.
  - `sim_ai` alone accounts for about 4.7 ms of the 6.4 ms per tick. The rest is spread across pose, mover-blocking, trigger, weapon and projectile column walks.
  - See `perf-ai-tick-cost/research.md`.
- **The fixed step amplifies it.** A slower frame runs more catch-up ticks, and at 20k the loop spirals.
- **GPU smoke pass (suspected, not timed).** See §GPU priority.
- **Other render-side costs are small.** `rec_smoke` recording is about 0.005 ms. `render_submit` is about 4.8 ms with or without particles. The light bridge was about 0.1% of main-thread samples. The fog and mesh paths did not match under the symbol names searched; that is unverified, not zero.
- **Registry snapshot.** `snapshot_transforms` walks every slot once per tick, about 0.6% of main-thread samples.

## Lifetime and capacity

- Particles share the registry's `u16` slot space (65,535) with gameplay entities.
- Weapon impact bursts (`weapon::impact::spawn_impact_effect_at`) and the emitter bridge both `try_spawn` particle entities. A full registry drops the particle with a warning, and it also starves any gameplay spawn that follows.
- The per-emitter cap (`MAX_SPRITES` = 4,096) bounds one emitter. Nothing bounds the total.
- Representations that coexist per particle today:
  - three `Option<ComponentValue>` cells of 800 bytes each, plus empty cells in every other column of that slot;
  - the `previous_transforms` entry;
  - the tick's snapshot `Vec`;
  - the render collector's packed 32-byte instance.

## Consumers of particle entities

| Consumer | Use |
|---|---|
| `emitter_bridge::spawn_one` (binary) | rate and burst spawns |
| `weapon::impact::spawn_impact_effect_at` (sim) | impact bursts, including the listen-host local burst for remote detonations |
| `particle_sim::tick` (sim) | integration, expiry, per-emitter survivor tally |
| `ParticleRenderCollector::collect_at_tick` (binary) | cell cull and packing |
| observability component listing (`observability/mod.rs`) | kind enumeration |
| SDK `ComponentKind` union (`sdk/types/postretro.d.ts`: `"particle_state"`) and the `ffi.rs` name map | **modder-visible**: a script can name the kind |
| netcode | tests only; particles are never replicated (`replication.rs` allow-list) |

## Established techniques considered

General engine practice, not re-verified against sources this session.
- **SoA particle pools outside the entity system, with swap-remove on death.** Unreal Niagara and Cascade CPU data sets, Godot `CPUParticles`, and most id-lineage engines work this way. Particles have no identity, no components, and nothing addresses them individually. This matches this engine's contract that scripts never observe individual particles (`entity_model.md` §8).
- **Shared per-emitter parameters.** Curves, drag and buoyancy are referenced by an emitter or burst class rather than copied per particle.
- **Per-particle cull alternatives**, judged against pixel preservation:
  - a radius-inflated camera-frustum reject before the cell locate: preserves pixels and is cheap;
  - per-chunk cell-locator descent, splitting a chunk only where its bounds straddle a plane: exact;
  - a cached cell refreshed only when a particle crosses its cell's bounds: exact only if cells are convex and the test uses the cell's planes;
  - per-emitter conservative bounds: draws particles in culled cells. Pixels survive only where depth occludes them, which nothing guarantees, and it costs fill. Out.

## GPU priority

- At the mixed pose, frame time is about 27 ms with no emitters and 82–89 ms with any emitter live. At 1k particles the frame is 88 ms with about 30 ms of CPU work, so about 60 ms of GPU cost appears with any emitter live and does not grow with count. `wait_acquire` is 44–58 ms at 1k, 5k and 10k alike.
- `rendering_pipeline.md` §12: a window dominated by `wait_acquire` is GPU-bound, "so work on the GPU side first". This brief proceeds anyway. On this Mac it buys roughly zero frame time between 1k and 10k. Its player-visible wins are elsewhere: the 20k point spirals the fixed step (184–238 ms frames, 11–14 ticks per frame), and gameplay spawn capacity and tick cost stop depending on particles. Both hold on any GPU.
- The adapter lacks `TIMESTAMP_QUERY`. A Metal System Trace was not taken: the screen locked before it could run.

**Queued GPU smoke spike** (not drafted; the owner is running another spike first).
- Lead: all 40 fixture emitters use `smoke_puff`, which has a baked normal map (`content/dev/textures/smoke_puff/smoke_puff_00_normal.png`, added by the `billboard-specular-shimmer` thin slice, a5fcc1a2c). A normal slot makes a collection a specular-shimmer material, and shimmer evaluates every static chunk-light record per fragment (`rendering_pipeline.md` §7.4). Count-independent cost points at per-fragment work over screen coverage, not per-instance work.
- Cheap tests:
  - swap the fixture's sprite to `smoke_puff_isotropic` (same art, no normal slot);
  - a pose with emitters live but out of view;
  - a Metal System Trace at the mixed pose.
- `perf-billboard-emitter` names a reduced-resolution smoke target as its unclaimed fallback lever.

## E24 reconciliation

- E24 (`drafts/E24--water-and-environment-volumes`) resolves gravity and push per particle, at its own position, on every particle step. Buoyancy scales the resolved gravity; push is unscaled. The step therefore needs per-particle acceleration inputs; a scalar `gravity` parameter, as `particle_sim::tick` takes today, does not survive.
- E24 times the resolve as a stage nested in `particle_sim`, budgeted under 0.5 ms per frame at 10k on this fixture. Counting it inside this brief's 0.6 ms would leave the pool 0.1 ms, so this brief's budget excludes it and the nested stage is measured separately.
- E24's pre-flight figures (AI about 7.4 ms/tick, particle sim about 4.5 ms/frame) predate both perf briefs and do not gate it.

## Impact transport

- Producers of `spawn_impact_effect_at` today: `projectile_stage` (two sites) and `weapon_stage/commands/local.rs` inside the fixed tick; `netcode::presentation::ingest_client_presentation_messages` for a received built-in splash spawn (client); `netcode::ingest_hit_declaration` for a client-declared splash detonation (host).
- `PresentationPool` (`sim/src/sim/presentation_pool.rs`) "accepts registry intake only; producers never access this pool or renderer state directly". The registry's `presentation_spawns` queue is that intake: `push_presentation_spawn` drops past `MAX_PENDING_PRESENTATION_SPAWNS` (128) silently, and `drain_presentation_spawns_into` moves requests into reusable caller storage. The world-point queue beside it counts overflow for a warning; the impact queue should too.
- A pool handle threaded into sim producers would break that rule and couple the fixed tick to frame-time state, so it is not an option.
- `netcode::presentation::route_host_world_point_presentation_spawns` is host→remote fan-out of world-point requests, not intake. It is not a precedent here.
- One impact request is one burst of `IMPACT_PARTICLE_COUNT` (9) particles, so the queue bounds bursts, not particles.

## Rival: occupancy index plus in-place fixes

An occupancy index makes column walks O(members), and in-place `&mut` updates plus interned collection ids cut the churn. The pool still wins:
- Particles carry `Transform`. `snapshot_transforms` and every Transform-column walk still scale with particles.
- The shared `u16` slot cap remains, so particles still starve gameplay spawns.
- Each particle slot still holds one 800-byte cell in every column (`registry-column-storage`).

## Commitment drift

- `plans/done/scripting-foundation/plan-3-emitter-entity.md` made particles entities on the premise: "Concurrent particle count is bounded per-emitter by the existing `MAX_SPRITES = 512`. Even with dozens of emitters the total fits four digits … No registry redesign needed." `MAX_SPRITES` is now 4,096, and `perf-billboard-emitter` Slice 5 lifted the 4,096-per-collection draw truncation, so nothing bounds the total.
- `entity_model.md` §8 says particles are registry entities and the sim runs "every game-logic tick". Both change with this brief. The sim already runs per rendered frame.
- `entity_model.md` §5 lists the particle sim among the bridges that "may spawn or despawn entities".
- Reconcile both at promotion.
