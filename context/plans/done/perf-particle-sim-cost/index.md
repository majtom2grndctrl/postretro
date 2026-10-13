# perf-particle-sim-cost

Brief · compact · reads: `context/lib/entity_model.md` §1, §5, §8 · `context/lib/rendering_pipeline.md` §7.4, §12 · `context/lib/scripting.md` §10.1 · `context/lib/testing_guide.md` §Resource bounds · read at a307e3f10

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
  - The pool is structure-of-arrays, presentation-only and game-layer-owned. It has no `EntityId`, no components and no per-particle script visibility, and it is never replicated.
  - A particle reaching its lifetime leaves on that step, and every survivor is stepped exactly once. Order within the pool is free: the billboard pass blends One + One, so draw order changes no pixel.
  - This reverses `plans/done/scripting-foundation/plan-3-emitter-entity.md`, whose bounded-total premise `perf-billboard-emitter` Slice 5 voided (`research.md` §Commitment drift).
- **`ComponentKind::ParticleState` is deleted.** The later kinds renumber everywhere the numbering is pinned (`research.md` §Source pins), and `WIRE_VERSION` bumps. No reserved gap is kept. The kind and its `particle` alias leave the FFI and both SDK typedefs (Boundary inventory). Scripts are documented never to observe particles.
- **Producers reach the pool without registry entities.**
  - The emitter bridge, a frame-time owner, writes the pool directly.
  - Every impact burst, from every raise site (`research.md` §Source pins), becomes a plain-data request on its own bounded queue on `EntityRegistry`, drained into the pool at frame time. Precedent: the `push_presentation_spawn` intake, which keeps `PresentationPool`'s rule that "producers never access this pool … directly". Unlike that intake, the impact queue is never routed to clients, who burst from their own prediction or the observer cue.
  - Tradeoff: the queue sits on `EntityRegistry`, the `entities` compile chokepoint. Overflow drops with a rate-limited warning.
- **Per-emitter parameters live in immutable parameter blocks.**
  - Curves, drag, buoyancy, the resolved collection and tint live in a block. A particle carries its own state plus a compact block index, never the parameters or a refcount on them.
  - Spawn-time semantics are preserved: a later emitter change never reaches a live particle. A changed emitter mints a new block lazily, at its next spawn, so blocks minted never exceed spawns, however often a script rewrites the emitter.
  - A block is freed when its last live particle expires. Impact burst classes are static blocks.
  - Spin is the one live per-emitter value, because `setSpinRate` tweens rotate particles already alive. An orphan keeps rotating at its emitter's last spin rate.
  - Steady-state spawn, integrate and expire perform no per-particle heap allocation.
- **Capacity.** A global live-particle budget of 65,536. Past it, new spawns drop with a rate-limited warning; live particles are never evicted. The per-emitter cap and its headroom tally are kept.
- **Simulation timing is unchanged.** The sim steps once per rendered frame on frame time, after the emitter bridge and before the light bridge. Particles are presentation, so they stay off the fixed tick.
- **E24 composes on the pool** (`research.md` §E24 reconciliation). E24 landed without its particle rows; this brief owns them as its final slice, the pool's first consumer of per-particle inputs.
  - Each particle's gravity and push come from a point resolve under `resolve_environment`'s overlap rule, as `resolve_gravity` is for agents. No fluid query runs, because no particle reads it.
  - Position integrates first; that step's velocity update uses the gravity and push at the post-step position. The step therefore takes per-particle gravity and push, not a scalar.
  - This brief's `particle_sim` budget excludes the nested environment-resolve stage, which E24 budgets and which is measured separately.
- **Culling preserves pixels.** A particle whose own position lies outside the visible cells stays undrawn (`perf-billboard-emitter` per-billboard cull). Skipping a particle whose quad cannot reach the screen is allowed, as are exact accelerations of the locate.
- **Proceeds ahead of GPU-side work**, against `rendering_pipeline.md` §12's "GPU side first". This fixture is GPU-bound from 1k particles, so this brief buys roughly zero frame time from 1k to 10k on this Mac. Its wins are avoiding the 20k fixed-step spiral and decoupling gameplay capacity (`research.md` §GPU priority).
- **Next priority after E24**, ending with E24's particle rows, and ahead of `perf-ai-tick-cost`'s pawn index (re-priced on the pool) and the registry occupancy index. An occupancy index landing first would absorb the same ~6.4 ms/tick and pass the particle-independent-tick row before the pool exists. The GPU smoke spike stays queued off this order: not next, diagnosis only (owner, 2026-10-09).
- **Instrumentation.** `particle_emit`, `particle_sim` and `particle_collect` stay as stage labels over the new path. The live-particle count is reported once per CPU-timing window.
- **Docs reconciled at promotion.** `entity_model.md` §2 (`ParticleState` row), §3 (creation and destruction), §5 (bridges that spawn and despawn), §6 (cell linkage), §7 (environment-volume particle row), §8 (rewritten, stating the registry-versus-pool line from the projectile non-goal); `rendering_pipeline.md` §7.4 (collector source); `scripting.md` §10.1 (tick wording, registry-entity and `ParticleState` spin paragraphs).
- **Non-goals.**
  - GPU smoke-pass cost: the queued spike owns it.
  - Registry iteration and storage for gameplay entities: the occupancy-index follow-up and `drafts/registry-column-storage`.
  - Projectile bodies stay registry entities, packed by the same collector. The line between registry and pool is individual addressing: gameplay projectiles are addressed by collision and hit authority, their client display copies by replication. Anything anonymous and disposable belongs in the pool.

## Acceptance
### Automated
- [x] Integration matches today's for one particle over many steps, with level-default gravity and zero push: position, velocity, buoyancy and drag, size and opacity curves, spin. It also holds when the emitter's spin rate changes mid-life, and after the emitter despawns (orphan keeps its last rate), including when a new emitter takes its registry slot on the same tick (P7).
- [x] Two particles given different gravity and push in one step each integrate their own.
- [x] Expiry: a particle reaching its lifetime is gone after that step and not counted toward headroom. Two expiring in one step both leave, including when one is the last element, and a step that expires every particle leaves the pool empty. No survivor is skipped or double-stepped (P17).
- [x] The per-emitter cap still clamps bursts and rate spawns to headroom, counting the emitter's particles across every block it has minted (P6).
- [x] Past the global budget, spawns drop, the warning fires once per throttle interval, and no live particle is evicted. The pool never exceeds the budget when impact requests and several emitters spawn on one frame. A full impact-request queue drops with the same throttled warning (P11).
- [x] At the global budget, a burst lands up to the remaining room and drops the rest. A blocked rate spawn carries no backlog into the frame room frees (P10, P12).
- [x] After a level clear, the first budget or queue overflow in the next level warns (P13).
- [x] A particle spawned before an emitter's curves, drag, buoyancy, sprite, lifetime or tint change keeps its spawn-time values for its whole life. A particle spawned after the change, including later on the same frame, takes the new values (P1).
- [x] An emitter rewritten many times between two spawns mints one block, and an emitter rewritten with no spawn mints none. Rewrites of only rate, spin or burst mint none. Live blocks never exceed live particles plus the static impact blocks. Once every emitter particle expires, only the static impact blocks remain (P2, P3, P5).
- [x] A block freed by its last particle's expiry and then reused by another emitter's mint never reaches the first emitter's later spawns (P4).
- [x] Impact bursts mint no blocks. After a level clear the block table holds only the static impact blocks, and the next level's first impact draws from them (P15).
- [x] An impact burst raised during a fixed tick, by client prediction, by an observer cue, and by host hit-declaration ingest appears in the pool on the same rendered frame as today, with the same count and orientation. The predicted owner still receives no second burst, and the listen host still materializes exactly one local burst for a remote detonation.
- [x] A particle spawned by the emitter bridge or drained from an impact request is stepped once, by that frame's delta, before that frame's collect. Impact requests raised across several fixed ticks of one frame drain in raise order, before the emitter bridge spawns (P8, P9).
- [x] A host's impact request is never routed to remote clients. A remote client still materializes exactly one burst per contact (P16).
- [x] The registry holds no particle entities after emission, impact and simulation. A level clear and a reload empty the pool and its pending requests, including a request queued in the last tick before the clear (P14).
- [x] Steady-state spawn, step and expire allocate nothing per particle, under the allocation probe: with the pool and block table at their high-water mark, a frame's allocation count is the same at 100 and at 10,000 live particles, including frames where an emitter is rewritten every frame.
- [x] The collector packs the current rule's set minus at most particles whose quads lie wholly outside the view frustum: in visible cells, in culled cells, drifted from their emitter, orphaned, draw-all frames. A particle centred just outside the frustum whose rotated quad, at its current size, overlaps it is still drawn. Batching and the 32-byte instance stride are unchanged.
- [x] A CPU-timing window reports the live-particle count under `render_prep` beside the particle stages: zero on frames with no live particles, and absent when timing is off (P18).
- [x] A script naming the `particle_state` kind, or its `particle` alias, is rejected by the TypeScript and Luau type-checks, and by the FFI on read and query as an unknown kind, not with the bridge-managed refusal.
- [x] Every surviving component kind's engine discriminant equals its wire constant, with no gap, and a peer at the pre-renumber wire version is refused at handshake.
- [x] Existing emitter-bridge, particle-sim, particle-render and impact suites pass, re-pointed at the pool.
- [x] *(From E24.)* A particle in a push volume accelerates by the push regardless of its buoyancy.
- [x] *(From E24.)* A particle inside a low-gravity volume accelerates at that volume's gravity times its buoyancy. On the particle step it drifts out, it takes level gravity. A zero-buoyancy particle floats in every volume.
- [x] *(From E24.)* On the step a particle's position crosses a volume boundary, that step's velocity update uses the gravity at its post-step position.
### Manual
- [x] Stress, pinned per `testing_guide.md` §Resource bounds:
  - **Run:** `stress-env-volumes.prl` recompiled from the committed map, mixed pose `114.60,2.44,-138.99,45,0`, release with dev-tools on this Intel Mac, `caffeinate`, window frontmost, no screen saver or lock, at least five 120-frame windows after warmup, load average recorded, live count read from the window.
  - **Baseline (2026-10-05), about 10k live:** `particle_sim` 3.5–3.8, `particle_collect` 2.8–2.9 and `particle_emit` 0.24 ms per frame; `sim_tick` 9.8 ms per tick.
  - **Budget at 10k:** `particle_sim` ≤ 0.6 ms excluding any nested environment-resolve stage, `particle_collect` ≤ 1.0 ms, `particle_emit` ≤ 0.3 ms.
  - **Upstream limit:** the GPU-bound smoke frame caps `total`, so judge stage costs.
  - **E24 particle environment stage (from E24):** a stage nested in `particle_sim` times the per-particle environment resolve; budget under 0.5 ms per frame at 10k live particles, excluded from `particle_sim`'s own budget.
  - **Cleanup:** sweep maps and their PRLs deleted.
- [x] Particle-independent tick: the same run with zero, about 10k and about 20k live particles (emitter-rate sweep). `sim_tick` per tick differs by no more than 10% across the three. Baseline 3.4, 9.8 and 15.2 ms.
- [x] Visual: smoke, sparks and impact bursts look unchanged in the particle zone (d) and a weapon test room. Check spin, drift through doorways, culling at portal edges and particles at the screen edge.

## Path
- **Seams.** Producers: `EmitterBridge::update` and `spawn_one`; the impact raise sites in `research.md` §Source pins. Sim: `particle_sim::tick`. Collector: `ParticleRenderCollector::collect_at_tick` / `collect_sprite`, which also packs projectile bodies. The binary's render-prep call site.
- **Intake.** `PresentationSpawn` is defined in `postretro-foundation` and queued on the registry; the impact request can follow that split. `EntityRegistry::clear_for_level_unload` clears the existing intake queues by name, so the new queue joins that list.
- **Pool owner.** Beside the emitter bridge's per-session state, with its type in `postretro-sim`.
- **Expiry.** Swap-remove or a free list. Expiry is not first-in-first-out (lifetimes vary within a burst), so a ring buffer does not fit without hole-skipping.
- **Culling.** A radius-inflated frustum reject before the locate; per-chunk cell-locator descent as an exact acceleration.
- **Collection.** The block's resolved collection index removes the collector's per-particle `String` hash.
- **SDK sites.** `ffi.rs` maps the name and alias both ways and rejects the kind in `setComponent`; both SDK typedefs and both typedef fixtures carry it.
- **Rival:** an occupancy index plus in-place churn fixes. The pool still wins (`research.md` §Rival).
- **Rival:** a GPU compute sim. Rejected: the frame is already GPU-bound, E24's resolve is a CPU tree in `postretro-collision`, and the cell cull is CPU-side. The block table keeps the door open.
- **Spin.** A per-emitter live spin slot that freezes its rate when its emitter despawns keeps the orphan rule without per-particle spin.
- **First slice:** the pool, the emitter producer and the collector over the pool with the frustum pre-test; impacts stay on the registry. Measure `particle_collect` at 10k against ≤ 1.0 ms, the riskiest unproven claim, and the particle-independent-tick row. Slot inflation is already shown by the `--emitters 0` row.
- `main.rs` and `sim/src/sim/mod.rs` are far past 800 lines. Land new code in its own modules.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| `particle_state` kind (removed) | `ComponentKind::ParticleState` and `ComponentValue::ParticleState` deleted; later `#[repr(u16)]` values shift down one | `netcode::component_kind_discriminant` and `net` `COMPONENT_KIND_*` follow the shift; `WIRE_VERSION` bumps from 29 | leaves `ComponentKind` and `ComponentValue` in `sdk/types/postretro.d.ts` and `typedef/tests/fixtures/expected.d.ts`; `ParticleState` type removed | same in `sdk/types/postretro.d.luau` and `expected.d.luau` | n/a |
| `particle` FFI alias (removed) | leaves the `ffi.rs` name map; an unknown kind on read, query and set | n/a | never in the union | never in the union | n/a |

## Open questions
- Pool growth strategy (reserve on level load versus grow to budget). — **delegated**
