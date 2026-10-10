# perf-particle-sim-cost

Brief · compact · reads: `context/lib/entity_model.md` §1, §5, §8 · `context/lib/rendering_pipeline.md` §7.4, §12 · `context/lib/scripting.md` · `context/lib/testing_guide.md` §Resource bounds · read at 2095e74c6

## Problem
Developer-observed defect from the E24 stress pre-flight. At about 10k live particles on `stress-env-volumes`, the particle stages cost 6.6 ms per frame, and particles inflate the fixed tick by about 6.4 ms per tick, about 34 ms per frame (`research.md`). The cause is that each particle is a full registry entity:
- `particle_sim::tick` rewrites three 800-byte component cells per particle per frame, which is about 70% of its time.
- Each particle holds a registry slot, so every slot-walking column iteration in the tick scales with particle count. Particles also share the registry's 65,535-slot space with gameplay spawns.

The render collector adds a per-particle cell locate (`LevelWorld::locate_cell`), about 77% of `particle_collect`.

When done:
- Particles live outside the entity registry.
- Particle CPU is linear in live particles, with a small constant.
- Gameplay sim cost and gameplay spawn capacity do not depend on particle count.
- No drawn pixel changes.

## Decisions
- **Particles move to a pool outside the registry.**
  - The pool is structure-of-arrays, presentation-only and game-layer-owned. Expired particles leave by swap-remove. Per-emitter SoA chunks (the `plans/done/fx-volumetric-smoke` ring-buffer lineage) are an allowed internal shape.
  - It has no `EntityId`, no components and no per-particle script visibility, as `entity_model.md` §8 already promises. Particles are never replicated, so no wire change.
  - This reverses `plans/done/scripting-foundation/plan-3-emitter-entity.md`, which made particles entities because `MAX_SPRITES` = 512 per emitter kept the total "four digits … No registry redesign needed". `perf-billboard-emitter` Slice 5 voided that premise by lifting the per-collection draw cap; the fixture holds ~10k. §8 is rewritten at promotion.
- **Producers reach the pool without registry entities.**
  - The emitter bridge, a frame-time owner, writes the pool directly.
  - An impact burst (`spawn_impact_effect_at`, raised in the fixed tick and from network ingest) becomes a queued request on bounded registry presentation intake, drained into the pool at frame time. Precedent: `EntityRegistry::push_presentation_spawn` / `drain_presentation_spawns_into`. It keeps `PresentationPool`'s rule that "producers never access this pool … directly".
  - Tradeoff: the queue sits on `EntityRegistry`, the `entities` compile chokepoint. The request stays plain data. Overflow drops with a rate-limited warning.
- **Per-emitter parameters are shared, not per particle.**
  - Curves, drag, buoyancy, collection and tint live once per emitter or burst class. A particle carries only its own state.
  - A live emitter's spin-rate refresh and the orphan rule (complete life at the last rotation) are preserved.
  - Steady-state spawn, integrate and expire perform no per-particle heap allocation.
- **Capacity.** A global live-particle budget of 65,536. Past it, new spawns drop with a rate-limited warning; live particles are never evicted. The per-emitter cap and its headroom tally are kept.
- **Simulation timing is unchanged.** The sim steps once per rendered frame on frame time, after the emitter bridge and before the light bridge. The doc drift ("every game-logic tick") is fixed at promotion.
- **E24 composes on the pool** (`research.md` §E24 reconciliation).
  - The step takes per-particle gravity and push, not a scalar: E24 resolves both per particle, at its position, every step.
  - This brief's `particle_sim` budget excludes E24's nested environment-resolve stage, which E24 budgets separately and which is measured nested.
  - The pool lands before E24's particle rows, which are written against it.
- **Culling preserves pixels.** No drawn pixel changes. A particle whose own position lies outside the visible cells stays undrawn (`perf-billboard-emitter` per-billboard cull). A particle whose quad cannot reach the screen may be skipped before the locate, e.g. a radius-inflated frustum reject. Exact accelerations, such as per-chunk cell-locator descent, are allowed.
- **SDK.** `"particle_state"` leaves the SDK `ComponentKind` union and the FFI name map. Scripts are documented never to observe particles.
- **Proceeds ahead of GPU-side work**, against `rendering_pipeline.md` §12's "GPU side first". This fixture is GPU-bound from 1k particles, so this brief buys roughly zero frame time from 1k to 10k on this Mac. Its wins are avoiding the 20k fixed-step spiral and decoupling gameplay capacity (`research.md` §GPU priority).
- **Landing order** across the stress pre-flight work:
  1. GPU smoke spike (queued, not drafted; `research.md`).
  2. This brief; its first slice measures `particle_collect` and tick.
  3. Nav region index (`perf-ai-tick-cost`), independent, may land alongside 2.
  4. Faction-first pawn index (`perf-ai-tick-cost`), its budget re-priced on the pool.
  5. E24's particle environment rows, against the pool.
  6. Registry occupancy index, last, its own brief, justified by non-particle populations.

  Pool first because it removes the cause by construction. An occupancy index landing first absorbs the same ~6.4 ms/tick and passes the particle-independent-tick row before the pool exists.
- **Instrumentation.** `particle_emit`, `particle_sim` and `particle_collect` stay as stage labels over the new path. The live-particle count is reported once per CPU-timing window.
- **Non-goals.** GPU smoke-pass cost (the queued spike owns it). Moving the sim onto fixed ticks. Registry iteration and storage for gameplay entities (the occupancy-index follow-up; `drafts/registry-column-storage`).

## Acceptance
### Automated
- [ ] Integration matches today's for one particle over many steps, with level-default gravity and zero push: position, velocity, buoyancy and drag, size and opacity curves, spin. It also holds when the emitter's spin rate changes mid-life, and after the emitter despawns (orphan keeps its last rate).
- [ ] Two particles given different gravity and push in one step each integrate their own.
- [ ] Expiry: a particle reaching its lifetime is gone after that step and not counted toward headroom; two expiring in one step both leave; swap-remove never skips or double-steps a survivor.
- [ ] The per-emitter cap still clamps bursts and rate spawns to headroom.
- [ ] Past the global budget, spawns drop, the warning fires once per throttle interval, and no live particle is evicted. A full impact-request queue drops with the same throttled warning.
- [ ] An impact burst raised during a fixed tick, and one raised by client or host network ingest, appear in the pool on the same rendered frame as today, with the same count and orientation. The listen host still materializes exactly one local burst for a remote detonation.
- [ ] The registry holds no particle entities after emission, impact and simulation. A level clear and a reload empty the pool and its pending requests.
- [ ] Steady-state spawn, step and expire allocate nothing per particle, under the allocation probe.
- [ ] The collector packs the current rule's set minus only particles whose quads lie wholly outside the view frustum: in visible cells, in culled cells, drifted from their emitter, orphaned, draw-all frames. A particle centred just outside the frustum whose quad overlaps it is still drawn. Batching and the 32-byte instance stride are unchanged.
- [ ] A script naming the `particle_state` kind is rejected by type-check and by the FFI.
- [ ] Existing emitter-bridge, particle-sim, particle-render and impact suites pass, re-pointed at the pool.
### Manual
- [ ] Stress, pinned per `testing_guide.md` §Resource bounds:
  - **Run:** `stress-env-volumes.prl` recompiled from the committed map, mixed pose `114.60,2.44,-138.99,45,0`, release with dev-tools on this Intel Mac, `caffeinate`, window frontmost, no screen saver or lock, at least five 120-frame windows after warmup, load average recorded, live count read from the window.
  - **Baseline (2026-10-05), about 10k live:** `particle_sim` 3.5–3.8, `particle_collect` 2.8–2.9 and `particle_emit` 0.24 ms per frame; `sim_tick` 9.8 ms per tick.
  - **Budget at 10k:** `particle_sim` ≤ 0.6 ms excluding any nested environment-resolve stage, `particle_collect` ≤ 1.0 ms, `particle_emit` ≤ 0.3 ms.
  - **Upstream limit:** the GPU-bound smoke frame caps `total`, so judge stage costs.
  - **Cleanup:** sweep maps and their PRLs deleted.
- [ ] Particle-independent tick: the same run with zero, about 10k and about 20k live particles (emitter-rate sweep). `sim_tick` per tick differs by no more than 10% across the three. Baseline 3.4, 9.8 and 15.2 ms.
- [ ] Visual: smoke, sparks and impact bursts look unchanged in the particle zone (d) and a weapon test room. Check spin, drift through doorways, culling at portal edges and particles at the screen edge.

## Path
- **Seams.** Producers: `EmitterBridge::update` and `spawn_one`; `weapon::impact::spawn_impact_effect_at`, called from `projectile_stage`, `weapon_stage/commands/local.rs`, `netcode::presentation::ingest_client_presentation_messages` (client) and `netcode::ingest_hit_declaration` (host). Sim: `particle_sim::tick`. Collector: `ParticleRenderCollector::collect_at_tick` / `collect_sprite`, which also packs projectile bodies; those stay. The binary's render-prep call site.
- **Intake.** `PresentationSpawn` is defined in `postretro-foundation` and queued on the registry; the impact request can follow that split. `EntityRegistry::clear_for_level_unload` already clears the existing intake queues.
- **Pool owner.** Beside the emitter bridge's per-session state, with its type in `postretro-sim`.
- **Collection.** Store a resolved collection index per class, not a `String` per particle. That removes the collector's per-particle hash.
- **SDK sites.** `ffi.rs` maps the name both ways and rejects it in `setComponent`; the SDK `ComponentValue` union and the typedef fixtures carry it too.
- **Rival:** an occupancy index plus in-place churn fixes. The pool still wins (`research.md` §Rival).
- **First slice:** the pool, the emitter producer and the collector over the pool with the frustum pre-test; impacts stay on the registry. Measure `particle_collect` at 10k against ≤ 1.0 ms, the riskiest unproven claim, and the particle-independent-tick row. Slot inflation is already shown by the `--emitters 0` row.
- `main.rs` and `sim/src/sim/mod.rs` are far past 800 lines. Land new code in its own modules.

## Open questions
- Pool growth strategy (reserve on level load versus grow to budget). — **delegated**
