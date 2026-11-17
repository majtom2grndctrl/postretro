# E10--ai-tick-scheduling

Brief · resumable · Epic 10 · reads: `context/lib/entity_model.md` §3, §6, §7c · `context/lib/scripting.md` §11 · `context/lib/rendering_pipeline.md` §Cluster SH residency, §12 · `context/lib/networking.md` · `context/lib/testing_guide.md` §Resource bounds · `context/research/cell-visibility-substrate.md` · `drafts/perf-ai-tick-cost/` · read at 3366eeeb7

**Sequence:** after `perf-ai-tick-cost` (exact cost cuts, same target on the same tick). Merges the roadmap's *Decision cadence* and *Arena relevance* items; *Parallel replans* follows.

## Problem
Owner-requested capability for an anticipated need (roadmap Epic 10, AI at scale): content today does not hit the limit, and `perf-ai-tick-cost` removes the scan behind most of the measured superlinear growth. Every enemy runs its whole tick at full detail every fixed tick, whether or not any player can perceive it, and most enemy timing counts ticks rather than time, so no part of the tick can run at a lower rate without changing behavior. Nothing lets a designer place a horde arena beside several dormant ones and pay only for the enemies in play. When done, enemy timing runs on time; deciding runs in staggered think pools while acting stays every tick; enemies no player can perceive and with no target run a coarse full tick; and a designer places Quake/Doom-class encounters at minimum, Serious Sam class as the attempted target, with no engine tuning.

## Decisions
- **Enemy timing runs on time, not tick counts.** Every tick-counted enemy window (perception grace, combat-slot hold, launch fall delay, arbitration seniority, weapon-controller steps, replan staleness, stuck windows) becomes a duration, and defense-layer recharge reads the step it is given. An enemy keeps its attack turns across ticks it does not run. This is the foundation both schedules below need; behavior at 60 Hz is unchanged except where a decision here changes it.
- **One per-enemy scheduler owns every enemy's rate.** Decide pools and the coarse tick are two answers from one scheduler, not two mechanisms. It runs on the host's fixed tick in `postretro-sim`, before the AI stage; it never reads the renderer, the host camera, or frame timing.
- **Deciding runs in staggered think pools; acting runs every tick.** Deciding is target acquisition and behavior-graph evaluation; acting is steering, facing, animation, clip time with its `at` check, and attack resolution. Pools are assigned from the enemy's slot, so assignment is identical across frame splits, spawn order and co-op peers. Engine default about 15 Hz, one constant, expected to be retuned once better test maps exist; Quake monsters thought at 10 Hz, and perfect reflexes read as unfair. Amends `entity_model.md` §7c "guards evaluate every tick" to "guards evaluate on decide ticks"; clip time and the moment check stay every tick, as §7c already requires of this work.
- **The distance stride runs inside decide ticks.** Today's 1/4/12-tick acquisition stride becomes the same intervals in time, rounded up to the enemy's decide ticks. `@brain.acquisitionDue` keeps meaning "acquisition runs this tick" and is true only on decide ticks.
- **Think interval may be authored slower, never faster.** An archetype may set `thinkIntervalMs`; the enemy decides no more often than that, and a value below the pool interval clamps to it. Unset means the pool rate. The name leaves `reactionMs` free for a Doom-style wake-up delay later. It is behavior content, so it joins the mod compatibility digest beside `moveSpeed`.
- **Perceivable means inside some player's id-51 set.** Each tick the engine locates every player pawn's cell (host and every participating client pawn) and unions the `CellResidencySet` (id 51) cells reachable from those cells within an AI lead. An enemy is perceivable when its cell is in that union. The union is AI policy. The step under it, player pawn → cell → id-51 set, is engine-owned and reusable; network relevance and combat VFX culling would key it per client, not union it.
  - Owner choice over a new sightline bake. id 51 is the set that keeps a visible cell's lightmap resident, so an enemy it misses already stands in an unlit cell.
  - id 51 is streaming-owned; the AI lead is its own constant, never streaming's runtime L, so streaming retuning never moves AI detail. Doors are not applied: closed doors only widen the set, the safe direction.
  - Armed ghost links couple their cells, per `networking.md` "Ghosts constrain future relevance": a perceived cell's linked cells join the union.
  - A level with no id 51 runs every enemy perceivable.
  - Monster closets get no special case. A closet enemy stays unperceived until it enters the play field; waking from a coarse tick at the reveal or after a teleport reads as fair disorientation, not a defect (owner).
  - id 46 is not used: its `perceivable` answers portal connectivity only, which is true across a connected map.
- **Unperceived, targetless enemies run a coarse full tick at 2 Hz.** The whole enemy tick runs, with its step scaled, staggered by slot across the coarse interval; it is never skipped or frozen. An enemy with a retained target stays at full detail even when unperceived. Promotion is immediate on the tick an enemy becomes perceivable, takes damage, gains a target, or has its aggro gate opened; demotion waits a short hold so an enemy at a set's edge does not flap. A coarse step keeps movement collision correct: no tunnelling through walls, floors or ledges.
  - Diverges from `cell-visibility-substrate.md` "must NOT cull authoritative simulation" in letter, not intent: a coarse tick still simulates every enemy. Amended to permit a coarser authoritative tick and keep forbidding a skipped one.
- **Perception decides detail level, never targets.** Keeps `perf-ai-tick-cost`'s rejection of a visibility broad phase inside target selection.
- **Docs amended at promotion:** `scripting.md` §11 (`acquisitionDue` and guard timing), `entity_model.md` §3 (every entity participates each tick → at its scheduled rate), §6 (if an enemy's cell is stored on it), §7c (above), the substrate doc's no-cull rule.
- **Non-goals:**
  - Network relevance and combat VFX culling: later consumers of the perception union.
  - A sightline PVS bake, a tighter id 46, or door-aware relevance.
  - A hearing model or alert propagation between enemies: enemies wake as they do today, through damage, targets and the aggro gate.
  - Parallel replans: the roadmap's next brief.
  - Scheduling across several simulated worlds; the scheduler takes its world as input so it does not foreclose it.

### Scripting surface
```ts
// thinkIntervalMs is optional. Unset: the engine's pool rate (about 15 Hz).
// Below the pool interval it clamps to the pool interval.
behavior: {
  initial: "idle",
  moveSpeed: 3,
  thinkIntervalMs: 200, // this archetype decides at most every 200 ms
  activities: { /* … */ },
},
```

## Acceptance

### Automated
Timing on time:
- [ ] Each converted window expires after the same elapsed time at 60 Hz, at decide-pool rate and at 2 Hz, and not one step earlier.
- [ ] An enemy that runs no AI on a tick keeps its attack turns; one whose turn ends while it is not running releases it on its next run.
- [ ] Defense-layer recharge over one second matches at 60 Hz and at 2 Hz.

Decide pools:
- [ ] With many enemies, pool assignment and every decision tick are identical across frame splits, reversed spawn order, and host versus replay.
- [ ] Deciding runs about 15 times a second per enemy by default; acting (steering, facing, animation) runs every tick for a perceived enemy.
- [ ] The clip-timed jab still lands on its pinned tick at any frame split.
- [ ] `@brain.acquisitionDue` is true only on decide ticks, and near, mid and far enemies acquire at their stride intervals rounded up to decide ticks.
- [ ] `thinkIntervalMs` above the pool interval decides no more often than its value; unset decides at the pool rate; below the pool interval clamps to it. The Scripting surface example runs as a `content/dev` fixture in TypeScript and Luau.
- [ ] Changing an archetype's `thinkIntervalMs` changes the mod compatibility digest.

Perception and the coarse tick:
- [ ] An enemy in a cell inside any player's id-51 set at the AI lead runs full detail; one outside every player's set and with no target runs at 2 Hz.
- [ ] Two players in different arenas each keep their own arena's enemies at full detail; a third, empty arena runs coarse.
- [ ] A coarse enemy promotes on the same tick it becomes perceivable, takes damage, gains a target, or has its aggro gate opened, including mid coarse step.
- [ ] A coarse enemy demotes only after the hold; one oscillating across a set edge does not flap.
- [ ] An unperceived enemy with a retained target stays full detail, and drops to coarse only after losing its target and the hold.
- [ ] Cells linked by an armed ghost link and teleporter arrival cells count as perceived; a disarmed ghost link does not couple.
- [ ] A level with no id 51 runs every enemy at full detail.
- [ ] A coarse enemy wandering, patrolling or falling never passes through a wall, floor or ledge it would stop at under 60 Hz.
- [ ] Changing streaming's runtime lead does not change which enemies run coarse.

Scale (reported per tier on a `tools/gen_stress_map.py` multi-arena fixture, `sim_ai` and `sim_steer_agents` with the new sub-stages):
- [ ] Quake/Doom class (about 64 engaged in view, about 500 placed across arenas): the fixed step never spirals and AI plus steering holds `perf-ai-tick-cost`'s budgets.
- [ ] Serious Sam class (about 128 engaged, about 1,000 placed): measured and reported, not gated.
- [ ] Per-tick cost of a coarse enemy, as a fraction of an engaged one, reported.
- [ ] The perception union costs no per-tick allocation, and its cost per tick is reported against player and enemy counts.

### Manual
- [ ] Listen-host playtest on the stress fixture: no visible judder or stepping on any enemy in view; enemies arriving from out of view look natural.
- [ ] Co-op client view of the same encounter is unchanged.
- [ ] A slow-`thinkIntervalMs` archetype reads as slower to react than a default one.

## Path
- Seams: `crates/ai/src/engine_floor.rs` `think_stride_for_distance`; `targeting::acquisition_due` and `BrainComponent::think_stride_counter`; `attack_turns::lifecycle::admit` (`seen_tick`); `perception` `LOS_GRACE_TICKS`; `combat_slots` `COMBAT_SLOT_HOLD_TICKS`; `launch` `FALL_DELAY_TICKS`; `agent_steering` `REPLAN_STALENESS_TICKS` and the shared replan budget; `defense_layers`. The fuller per-system inventory, with how each handles time, is in `research.md`.
- Perception: `LevelWorld::locate_cell` per player pawn; id 51 parsed for streaming. Check every load path keeps id 51 in memory when lightmap streaming is off. Enemy cells update only when an enemy moves.
- Coarse step collision: check `collide_and_slide` at a 0.5 s step first; sub-step movement inside a coarse tick if it is not swept. This is the riskiest assumption and the first slice.
- Listen-host presentation draws enemies from the live registry, blended between ticks; a coarse enemy in view would step, which the perception rule and immediate promotion exist to prevent.
- Tests pinning ticks: `determinism_tests.rs` (one enemy, so slot pools pass by accident; add many), many AI tests set the stride counter by hand, `headless_byte_identity.rs`.
- Durations: store them as whole elapsed ticks, not floats, so replays and `headless_byte_identity.rs` stay byte-identical.
- Strongest rival: decide pools plus rest-based sleep, as physics engines sleep resting bodies (grounded, still, targetless enemies skip their tick until damage, a target, the aggro gate or a push). No id-51 dependency and no coarse step, and it covers idle enemies in view, but not patrollers or fallers in empty arenas, and it waits on measuring idle cost first. Rejected by the owner's 2 Hz coarse tick and build-at-target-scale stance.
- Packaging rival: separate cadence and arena briefs. Rejected: both need the same time conversion and scheduler, and two briefs would rewrite §7c and the tick-pinning tests twice.

## Open questions
- Demotion hold length and the AI lead over id 51 — **delegated**: the executor picks and reports both with measurements on the stress fixture.
- Whether an enemy's cell is cached on the enemy or in a scheduler-side table — **delegated**.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Think interval | `think_interval_ms` on the behavior descriptor | mod digest field beside `move_speed` | `thinkIntervalMs` | `thinkIntervalMs` | n/a |
