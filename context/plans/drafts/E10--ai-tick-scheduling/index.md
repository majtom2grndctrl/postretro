# E10--ai-tick-scheduling

Brief · resumable · Epic 10 · reads: `context/lib/entity_model.md` §3, §5, §6, §7c · `context/lib/scripting.md` §11 · `context/lib/rendering_pipeline.md` §Cluster SH residency, §12 · `context/lib/networking.md` · `context/lib/testing_guide.md` §Resource bounds · `context/research/cell-visibility-substrate.md` · `drafts/perf-ai-tick-cost/` · read at bf4f43f1b

**Sequence:** after `perf-ai-tick-cost` (exact cost cuts, same target on the same tick). Merges the roadmap's *Decision cadence* and *Arena relevance* items; *Parallel replans* follows.

## Problem
Owner-requested capability for an anticipated need (roadmap Epic 10, AI at scale): content today does not hit the limit, and `perf-ai-tick-cost` removes the scan behind most of the measured superlinear growth. Every enemy runs its whole tick at full detail every fixed tick, whether or not any player can perceive it, and most enemy timing counts ticks rather than time, so no part of the tick can run at a lower rate without changing behavior. Nothing lets a designer place a horde arena beside several dormant ones and pay only for the enemies in play. When done, enemy timing runs on time; deciding runs in staggered think pools while acting stays every tick; enemies no player can perceive and with no target run a coarse full tick; and a designer places Quake/Doom-class encounters at minimum, Serious Sam class as the attempted target, with no engine tuning.

## Decisions
- **Enemy timing runs on time, not tick counts.** Every tick-counted enemy window becomes a duration (inventory in `research.md`). An enemy keeps its attack turns across ticks it does not run. This is the foundation both schedules below need; behavior at 60 Hz is unchanged except where a decision here changes it.
- **Each enemy has one rate at a time, chosen on the host's fixed tick before the AI stage.** The rate is its decide pool or the coarse tick. The choice never reads the renderer, the host camera, or frame timing.
- **Deciding runs in staggered think pools; acting runs every tick.** Deciding is target acquisition and behavior-graph evaluation; acting is steering, facing, animation, clip time with its `at` check, and attack resolution. An enemy's pool is a pure function of per-enemy data that does not depend on spawn order or frame split, so assignment replays identically. Engine default about 15 Hz, one constant, expected to be retuned once better test maps exist; Quake monsters thought at 10 Hz, and perfect reflexes read as unfair. Amends `entity_model.md` §7c "guards evaluate every tick" to "guards evaluate on decide ticks"; clip time and the moment check stay every tick, as §7c already requires of this work.
- **The distance stride keeps one far band.** Near and mid enemies acquire on every decide tick; far enemies (past today's 30 m band) acquire on every third. `@brain.acquisitionDue` keeps meaning "acquisition runs this tick" and is true only on decide ticks.
- **Perceivable means inside some player's id-51 set.** The engine locates the cell of every player pawn in play or downed, host and clients alike, and unions the `CellResidencySet` (id 51) cells reachable from those cells within an AI lead. An enemy is perceivable when its cell is in that union. The union is AI policy. The step under it, player pawn → cell → id-51 set, is engine-owned and reusable; network relevance and combat VFX culling would key it per client, not union it.
  - Owner choice over a new sightline bake. id 51 is sampled, so a cell it misses can still be drawn; an enemy there that acquires a player promotes on its next coarse run. One that cannot acquire the player (another faction, aggro gate closed, candidacy rejects) may step at 2 Hz in view; accepted as rare.
  - id 51 is streaming-owned; the AI lead is its own constant, never streaming's runtime L, so streaming retuning never moves AI detail. It cannot exceed id 51's baked maximum lead. Doors are not applied: closed doors only widen the set, the safe direction.
  - Armed ghost links couple their cells, per `networking.md` "Ghosts constrain future relevance": a perceived cell's linked cells join the union.
  - A level with no id 51, or a tick with no pawn in play or downed, runs every enemy perceivable.
  - Monster closets get no special case. A closet enemy stays unperceived until it enters the play field; waking from a coarse tick at the reveal or after a teleport reads as fair disorientation, not a defect (owner).
- **Unperceived, targetless enemies run a coarse full tick at 2 Hz.** The whole enemy tick runs, with its step scaled, staggered across the coarse interval by the same per-enemy data; it is never skipped or frozen. An enemy with a retained target stays at full detail even when unperceived. An enemy promotes on the tick it becomes perceivable or gains a target. Damage and an opened aggro gate promote it on the tick they land when that is before the AI stage, otherwise on the next tick. On promotion, the time it has not yet run credits its timers; it moves only one ordinary tick, so it never jumps. Demotion waits a short hold so an enemy at a set's edge does not flap. A coarse step keeps movement collision correct: no tunnelling through walls, floors or ledges.
  - Diverges from `cell-visibility-substrate.md` "must NOT cull authoritative simulation" in letter, not intent: a coarse tick still simulates every enemy. Amended to permit a coarser authoritative tick and keep forbidding a skipped one.
- **Replan tiers outrank detail level.** The shared per-tick replan budget keeps its tier order (blocked route, then drift, then staleness) and its blocked-route wait bound for every enemy; within a tier, full-detail enemies go first.
- **Perception decides detail level, never targets.** Keeps `perf-ai-tick-cost`'s rejection of a visibility broad phase inside target selection.
- **Docs amended at promotion:** `scripting.md` §11, `entity_model.md` §3, §5 (update order), §6 (if an enemy's cell is stored on it), §7c (every every-tick clause), the substrate doc's no-cull rule; the no-PVS ground rule lands in `context/lib/`.
- **Non-goals:**
  - Network relevance and combat VFX culling: later consumers of the perception union.
  - A sightline PVS bake, a tighter id 46, or door-aware relevance. Owner ground rule: push PostRetro as far as it goes without a Quake-style PVS bake, historically the heaviest bake stage here and in Quake.
  - A hearing model or alert propagation between enemies: enemies wake as they do today, through damage, targets and the aggro gate.

## Acceptance

### Automated
Timing on time:
- [ ] Each converted window expires on the first step at or past its duration at 60 Hz, at decide-pool rate and at 2 Hz, and never on an earlier step (P18).
- [ ] An enemy that runs no AI on a tick keeps its attack turns; one whose turn ends while it is not running releases it on its next run.
- [ ] While an enemy holding a turn skips its AI, its turn still counts against its pool on every skipped tick: no other enemy is granted that capacity, and no refresh starts, until the holder releases it (P1).
- [ ] An enemy that leaves the AI set (held, quiescent or despawning) still returns holding nothing, requesting nothing, with no wait clock; only a scheduler skip keeps its turns (P2).
- [ ] After one second, an enemy's weapon resources, cooldowns, activity time, damage recency and attacker ledger read the same at 60 Hz and coarse: each advances once per elapsed tick (P11).

Decide pools:
- [ ] With many enemies, pool assignment and every decision tick are identical across frame splits, reversed spawn order, and host versus replay.
- [ ] Each enemy decides exactly once per pool interval by default; acting (steering, facing, animation) runs every tick for every full-detail enemy, perceived or holding a retained target.
- [ ] On the stress fixture, including when enemies spawn interleaved or at a regular stride, the number of enemies deciding on any tick differs from the per-pool mean by at most one (P15).
- [ ] The clip-timed jab's first damage lands 17 ticks after its entry tick for every pool phase, at any frame split (P19).
- [ ] `@brain.acquisitionDue` is true only on decide ticks; near and mid enemies acquire on every decide tick and far enemies on every third, for every spawn tick relative to the enemy's pool phase (P5).

Perception and the coarse tick:
- [ ] An enemy in a cell inside any player's id-51 set at the AI lead runs full detail; one outside every player's set and with no target runs at 2 Hz.
- [ ] An unperceived, targetless enemy runs its AI and steering on exactly one tick in each coarse interval, and on no other tick until promoted.
- [ ] Two players in different arenas each keep their own arena's enemies at full detail; a third, empty arena runs coarse.
- [ ] A coarse enemy promotes on the same tick it becomes perceivable or gains a target, including mid coarse step; an aggro gate opened by a trigger-bound command promotes it that tick, and one opened by a named reaction promotes it for the next tick's AI stage.
- [ ] Damage that lands on a coarse enemy before the AI stage (projectile impacts, remote hits ready at the input boundary) promotes it that tick; damage during or after it (another enemy's attack, host hitscan, trap shots, remote hits awaiting that tick's fire authorization) promotes it for the next tick's AI stage (P6).
- [ ] A coarse enemy promoted part way through its interval has its timers advanced by the time it had not run and moves exactly one ordinary tick on the promotion tick (P3).
- [ ] One second after a promotion part way through a coarse interval, the enemy's weapon resources, cooldowns, activity time, damage recency and attacker ledger read the same as an enemy that ran at 60 Hz throughout: no quantity is credited twice (P3, P11).
- [ ] A coarse enemy that gains a target on its own coarse run moves exactly one ordinary tick in that tick's steering, and from the next tick runs at full detail (P21).
- [ ] A coarse enemy whose route a closed door blocks is served within the blocked-route wait bound even while full-detail enemies fill the budget with drift and staleness replans; within one tier, every full-detail request is served before any coarse one (P13).
- [ ] Pawns of seats in play or down count as perceivers, whatever their health or life state; loading and reconnecting seats have no pawn and do not; a tick with no counting pawn runs every enemy at full detail (P17).
- [ ] A level loaded without id 51 runs every enemy at full detail on every tick.
- [ ] The AI lead never exceeds the loaded level's id-51 maximum lead; a level baked with a smaller maximum uses that maximum.
- [ ] Every coarse tick is a decide tick: acquisition runs, `@brain.acquisitionDue` is true and transitions evaluate, whatever the enemy's pool phase or distance band (P4).
- [ ] A clip moment that falls inside a coarse step resolves once, on that step, with its gates evaluated then; none is skipped or fired twice.
- [ ] A player who teleports or respawns into a dormant arena on a tick has that arena's enemies at full detail on the same tick's AI stage (P7).
- [ ] A coarse enemy at the fastest shipped `moveSpeed` walking toward a player is promoted while still in the AI lead; it is never first seen at full detail already inside a cell visible from a player's cell (P8).
- [ ] An enemy spawned, re-using a freed slot, or moved by a script, mover or knockback is classified from its new cell on the next tick, and inherits no cell, hold or coarse phase from a slot's previous occupant (P9).
- [ ] An unperceived hostile enemy whose candidacy admits a player, with line of sight to that player from outside the player's set, acquires the player on its next run, then promotes (P20).
- [ ] A coarse enemy demotes only after the hold; one oscillating across a set edge does not flap.
- [ ] An unperceived enemy with a retained target stays full detail, and drops to coarse only after losing its target and the hold.
- [ ] Cells linked by an armed ghost link and teleporter arrival cells count as perceived; a disarmed ghost link does not couple.
- [ ] A coarse enemy wandering, patrolling or falling never passes through a wall, floor or ledge it would stop at under 60 Hz.
- [ ] A coarse enemy riding a lift or standing in a closing door's sweep ends on the same side of the mover as at 60 Hz: carried, pushed or blocking, never embedded or dropped through (P10).
- [ ] Changing streaming's runtime lead does not change which enemies run coarse.

Scale (reported per tier on a `tools/gen_stress_map.py` multi-arena fixture, `sim_ai` and `sim_steer_agents` with the new sub-stages):
- [ ] Quake/Doom class (about 64 engaged in view, about 500 placed across arenas): the fixed step never spirals and AI plus steering holds `perf-ai-tick-cost`'s budgets.
- [ ] Serious Sam class (about 128 engaged, about 1,000 placed): measured and reported, not gated.
- [ ] Per-tick cost of a coarse enemy, as a fraction of an engaged one, reported.
- [ ] On the stress fixture, a player teleporting into the most populated dormant arena promotes every enemy there on one tick without spiralling the fixed step; that tick's cost is reported (P14).
- [ ] The perception union costs no per-tick allocation, and its cost per tick is reported against player and enemy counts.

### Manual
- [ ] Listen-host playtest on the stress fixture: no visible judder or stepping on any enemy in view; enemies arriving from out of view look natural.
- [ ] Co-op client view of the same encounter is unchanged.

## Path
- Seams: `crates/ai/src/engine_floor.rs` `think_stride_for_distance`; `targeting::acquisition_due` and `BrainComponent::think_stride_counter`; `attack_turns::lifecycle::admit` (`seen_tick`); `perception` `LOS_GRACE_TICKS`; `combat_slots` `COMBAT_SLOT_HOLD_TICKS`; `launch` `FALL_DELAY_TICKS`; `agent_steering` `REPLAN_STALENESS_TICKS` and the shared replan budget. The fuller per-system inventory, with how each handles time, is in `research.md`.
- Perception: `LevelWorld::locate_cell` per player pawn; id 51 parsed for streaming. Check every load path keeps id 51 in memory when lightmap streaming is off. Re-locate an enemy's cell whenever its position changes, whatever moved it (P9).
- Coarse step collision: check `collide_and_slide` at a 0.5 s step first; sub-step movement inside a coarse tick if it is not swept. This is the riskiest assumption and the first slice.
- Listen-host presentation draws enemies from the live registry, blended between ticks; a coarse enemy in view would step, which the perception rule and promotion exist to prevent.
- Pool key: registry slots follow spawn order through the free list, and the AI's per-tick index is a snapshot position, so neither works; a map placement index for placed enemies and spawner plus spawn sequence for spawned ones are candidates.
- Tests pinning ticks: `determinism_tests.rs` (one enemy, so any pool key passes by accident; add many), many AI tests set the stride counter by hand, `headless_byte_identity.rs`.
- Durations: store them as whole elapsed ticks, not floats, so replays and `headless_byte_identity.rs` stay byte-identical.
- Strongest rival: decide pools plus rest-based sleep, as physics engines sleep resting bodies (grounded, still, targetless enemies skip their tick until damage, a target, the aggro gate or a push). No id-51 dependency and no coarse step, and it covers idle enemies in view, but not patrollers or fallers in empty arenas, and it waits on measuring idle cost first. Rejected by the owner's 2 Hz coarse tick and build-at-target-scale stance.
- Packaging rival: separate cadence and arena briefs. Rejected: both need the same time conversion and scheduler, and two briefs would rewrite §7c and the tick-pinning tests twice.

## Open questions
- Demotion hold length and the AI lead over id 51 — **delegated**: the executor picks and reports both with measurements on the stress fixture.
- Whether an enemy's cell is cached on the enemy or in a scheduler-side table — **delegated**.
