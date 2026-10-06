# perf-particle-sim-cost

Brief · compact · reads: `context/lib/entity_model.md` §1, §5, §8 · `context/lib/rendering_pipeline.md` §12 · `context/lib/scripting.md` · `context/lib/testing_guide.md` §Resource bounds · read at 90c9b9687

## Problem
Developer-observed defect from the E24 stress pre-flight. At about 10k live particles on `stress-env-volumes`, the particle stages cost 6.6 ms per frame, and particles inflate the fixed tick by about 6.4 ms per tick, about 34 ms per frame (`research.md`). The cause is that each particle is a full registry entity:
- `particle_sim::tick` rewrites three 800-byte component cells per particle per frame, which is about 70% of its time.
- Each particle holds a registry slot, so every slot-walking column iteration in the tick scales with particle count. Particles also share the registry's 65,535-slot space with gameplay spawns.

The render collector adds a per-particle cell locate (`LevelWorld::locate_cell`), about 77% of `particle_collect`.

When done:
- Particles live outside the entity registry.
- Particle CPU is linear in live particles, with a small constant.
- Gameplay sim cost and gameplay spawn capacity do not depend on particle count.
- The particles drawn are the ones drawn today.

## Decisions
- **Particles move to a pool outside the registry.**
  - The pool is structure-of-arrays, presentation-only and game-layer-owned. Expired particles leave by swap-remove.
  - It has no `EntityId`, no components and no per-particle script visibility, as `entity_model.md` §8 already promises. Particles are presentation, never replicated, so no wire change.
  - Divergence: `entity_model.md` §8 ("registry-managed presentation entity") is rewritten at promotion.
- **Every particle producer writes the pool.**
  - The emitter bridge's rate and burst spawns and the weapon impact bursts both write the pool.
  - An impact burst raised inside the fixed tick reaches the pool without registry access. How it travels — a queued request or a pool handle — is the executor's choice.
- **Per-emitter parameters are shared, not per particle.**
  - Curves, drag, buoyancy, collection and tint live once per emitter or burst class. A particle carries only its own state.
  - A live emitter's spin-rate refresh and the orphan rule (complete life at the last rotation) are preserved.
  - Steady-state spawn, integrate and expire perform no per-particle heap allocation.
- **Capacity is bounded and policed like the per-emitter cap.**
  - The pool has a global live-particle budget.
  - Past it, new spawns drop with a rate-limited warning; live particles are never evicted.
  - The per-emitter cap and its headroom tally are kept.
- **Simulation timing is unchanged.** The sim still steps once per rendered frame on frame time, after the emitter bridge and before the light bridge. The documentation drift ("every game-logic tick") is fixed at promotion.
- **The cull result is unchanged.**
  - A particle is drawn exactly when its own position lies in a visible cell (`perf-billboard-emitter`, per-billboard cull).
  - The locate cost is reduced without changing that set, for example by a camera-frustum pre-test before the locate.
  - A conservative superset needs owner sign-off (Open questions).
- **Instrumentation.** `particle_emit`, `particle_sim` and `particle_collect` stay as stage labels over the new path. `particle_collect` was added this session. The live-particle count is reported once per CPU-timing window.
- **Non-goals.**
  - GPU smoke-pass fill cost. It is measured as GPU-bound but not timed on this Mac. The reduced-resolution smoke target stays `perf-billboard-emitter`'s unclaimed fallback.
  - Moving the sim onto fixed ticks.
  - Registry iteration cost for gameplay entities: `perf-ai-tick-cost` owns it, and neither brief blocks the other.

## Acceptance
### Automated
- [ ] Integration matches today's behaviour for one particle over many steps: position, velocity under gravity, buoyancy and drag, size and opacity curves, and spin. It also holds:
  - when the parent emitter's spin rate changes mid-life;
  - after the emitter is despawned (orphan keeps its last rate).
- [ ] Expiry:
  - a particle whose age reaches its lifetime is gone after that step and is not counted toward its emitter's headroom;
  - two particles expiring in one step both leave;
  - swap-remove never skips or double-steps a survivor.
- [ ] The per-emitter cap still clamps bursts and rate spawns to headroom.
- [ ] Past the global budget, spawns drop, the warning fires once per throttle interval, and no live particle is evicted.
- [ ] An impact burst raised during a fixed tick appears in the pool with the same count and orientation as today. The listen host still materializes exactly one local burst for a remote detonation.
- [ ] The registry holds no particle entities after emission, impact and simulation. A level clear and a level reload empty the pool.
- [ ] Steady-state spawn, step and expire allocate nothing per particle, under the allocation probe.
- [ ] The collector draws exactly the set the current per-billboard rule draws, for particles in visible cells, in culled cells, drifted from their emitter, orphaned, and in draw-all frames. Collection batching and the 32-byte instance stride are unchanged.
- [ ] Existing emitter-bridge, particle-sim, particle-render and impact suites pass, re-pointed at the pool.
### Manual
- [ ] Stress, pinned per `testing_guide.md` §Resource bounds:
  - **Run:** `stress-env-volumes.prl` recompiled from the committed map, mixed pose `114.60,2.44,-138.99,45,0`, release with dev-tools on this Intel Mac, `caffeinate`, window frontmost, no screen saver or lock, at least five 120-frame windows after warmup, load average recorded, live count read from the window.
  - **Baseline (2026-10-05), about 10k live:** `particle_sim` 3.5–3.8, `particle_collect` 2.8–2.9 and `particle_emit` 0.24 ms per frame; `sim_tick` 9.8 ms per tick.
  - **Proposed budget at 10k:** `particle_sim` ≤ 0.6 ms, `particle_collect` ≤ 1.0 ms, `particle_emit` ≤ 0.3 ms.
  - **Upstream limit:** the GPU-bound smoke frame caps `total`, so judge stage costs.
  - **Cleanup:** sweep maps and their PRLs deleted.
- [ ] Particle-independent tick: the same run with zero, about 10k and about 20k live particles (emitter-rate sweep). `sim_tick` per tick differs by no more than 10% across the three. Baseline 3.4, 9.8 and 15.2 ms.
- [ ] Visual: smoke, sparks and impact bursts look unchanged in the particle zone (d) and in a weapon test room. Check spin, drift through doorways, and culling at portal edges.

## Path
- **Seams:**
  - producers: `emitter_bridge::EmitterBridge::update` and `spawn_one`, `weapon::impact::spawn_impact_effect_at`;
  - sim: `scripting_systems::particle_sim::tick`;
  - collector: `ParticleRenderCollector::collect_at_tick`;
  - the binary's render-prep call site.
- **Pool owner.** The pool belongs beside the emitter bridge's per-session state, with its type in `postretro-sim` so impact effects can reach it. A candidate precedent for a sim-raised request that is drained after the tick is `netcode::presentation::route_host_world_point_presentation_spawns`. Check its semantics before reusing it.
- **Collection.** Store a resolved collection index per class, not a `String` per particle. That also removes the collector's per-particle hash.
- **Rival:** keep particles as entities and fix the churn in place (in-place `&mut` updates, interned collection ids). It cuts `particle_sim` but leaves slot inflation and the shared slot cap, which is the larger cost.
- **First slice:** the pool and the emitter producer only, with impact bursts still on the registry. Measure the particle-independent-tick row; it falsifies the slot-inflation claim cheaply.
- `main.rs` and `sim/src/sim/mod.rs` are far past 800 lines. Land new code in its own modules.

## Open questions
- **Remove `"particle_state"` from the SDK `ComponentKind` union and the FFI name map?** Recommendation: yes. Scripts are documented never to observe particles, and the project is pre-release ("move fast, break APIs"). — owner — **blocks build**
- **Global live-particle budget value?** Recommendation: 65,536. That is 16 emitters at the full per-emitter cap, and 6.5× the stress fixture. The registry's slot cap no longer limits it. — owner — **blocks build**
- **May the cull draw a conservative superset**, such as per-emitter bounds, if the exact frustum-plus-locate path misses the collect budget? It would change no visible pixel when depth-occluded, but it costs GPU fill. Recommendation: no. Keep exactness and report if the budget is missed. — owner — **blocks build**
- Pool growth strategy (reserve on level load versus grow to budget). — **delegated**
