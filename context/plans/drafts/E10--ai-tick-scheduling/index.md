# E10--ai-tick-scheduling

Brief · resumable · Epic 10 · reads: `context/lib/entity_model.md` §3, §5, §6, §7c · `context/lib/scripting.md` §11 · `context/lib/movement.md` §6 · `context/lib/rendering_pipeline.md` §Cluster SH residency, §12 · `context/lib/networking.md` · `context/lib/testing_guide.md` §Resource bounds · `context/research/cell-visibility-substrate.md` · `drafts/perf-ai-tick-cost/` · `spike-visible-rates.md` · read at efdab38d6

**Sequence:** after `perf-ai-tick-cost` (exact cost cuts, same target on the same tick). Merges the roadmap's *Decision cadence* and *Arena relevance* items; *Parallel replans* follows.

## Problem
Owner-requested capability for an anticipated need (roadmap Epic 10, AI at scale): content today does not hit the limit. Every NPC decides every tick, and every NPC runs its whole tick at 60 Hz whether or not any player can perceive it. Most NPC timing counts ticks rather than time, so no part of an NPC can run slower without changing behavior. A spike on a four-arena map (`spike-visible-rates.md`), measured before `perf-ai-tick-cost`, put deciding at about 96% of an engaged NPC's AI tick and NPCs no player can perceive at the larger share of the tick on maps of 500 to 1,000 placed. When done, NPC timing runs on time, NPCs decide only as often as players can notice, NPCs nobody perceives cost little, and a designer places Quake/Doom-class encounters at minimum, Serious Sam class as the attempted target, with no engine tuning.

## Decisions
- **NPC timing runs on time, not tick counts.** Every tick-counted NPC window becomes a duration (inventory in `research.md`). An NPC keeps its attack turns, and any queued grant, across ticks it does not run. Behavior at 60 Hz is unchanged except where a decision here changes it.
- **Each part of an NPC runs at its tier's rate.** One per-NPC choice of tier, made on the host's fixed tick before the AI stage; it never reads the renderer, the host camera or frame timing.

  | Tier | Who | Target acquisition | Decide: guards, move and action selectors, combat-slot choice | Acting: steering along the chosen route or slot, separation, route following, clip time and moments, attacks, facing and aim, hit reactions |
  |---|---|---|---|---|
  | Engaged | has a target, perceived or not | 15 Hz | 15 Hz | every 60 Hz tick |
  | Idle | perceived, no target | 15 Hz, through the notice meter | 15 Hz | every 60 Hz tick |
  | Dormant | unperceived, no target, not `alwaysActive` | every 2 s, through the notice meter | every 2 s | whole tick at 2 Hz |

  - Every rate is a constant expected to be retuned on better test maps. Decide rates set reaction feel as well as cost, so a retune for cost is also a feel change. Per-NPC decide rates are not authored: one fast archetype in a horde would break the staggered pools (owner dropped `thinkIntervalMs`); how fast an NPC notices is authored through the notice meter instead.
  - **Tactical range.** An engaged NPC whose target's XZ distance is beyond its `tacticalRange` acquires on every third decide, and under every engaged motion verb (chase, strafe, reposition) routes toward its target instead of choosing a combat slot. It chooses a slot once its target is 10 m inside the range, so the line does not flap. A slot around a target that far away is stale before the NPC arrives, and choosing one costs a long route search per candidate slot. `tacticalRange` is an optional behavior-root field in metres, default 100, authored per archetype (owner); the engine keeps the behavior beyond it, the author moves the line. A value below the archetype's largest attack standoff plus 10 m is raised to that and warned at load and on every hot reload that commits it, so a reload never fails over it (owner).
  - Decides and dormant steps are staggered by a mixing hash of each NPC's identity (map placement, or spawner and spawn count), which does not depend on spawn order or frame split, so assignment replays identically in co-op and replays. Spawns with no map identity (synthetic placements, spawners without an ordinal) keep the registry index, replay-stable only under a fixed spawn order. Balance across phases is statistical, not exact; exact balance would need phases that depend on which other NPCs exist.
  - Amends `entity_model.md` §7c: guards, move and action selectors and combat-slot choice evaluate on decide ticks, engine-gated rows (a hold's release, a release's exit, `animationDone`) included, so those land up to one decide interval later than at 60 Hz (owner: about 67 ms at 15 Hz, imperceptible). Steering follows the chosen slot every tick; a strafer re-aims each decide. A queued attack grant opens its row at the NPC's first decide after the grant. Route following within an activity (a patrol cursor, a return to anchor) is acting and advances every tick. The clip accumulator and moment check stay on every tick for perceived and engaged NPCs; a dormant NPC's 2 Hz step resolves each moment inside it once.
- **An NPC without a target notices through a meter.** Each NPC keeps an awareness value from 0 to 1 per candidate it could acquire, for the nearest few (bounded as the attacker ledger is, 8). On each acquisition check, awareness of a candidate that passes candidacy and line of sight rises by elapsed time over `notice.fillMs`; one that fails drains by elapsed time over `notice.drainMs`. The NPC acquires the first candidate to reach 1, by today's ranking among those that do. In co-op, each player has their own meter, so a hidden player is not spotted because a teammate was. Damage and retaliation skip the meter, and an NPC with a target ranks and switches targets as today, without it. `notice` is an optional behavior-root block; `fillMs` defaults to 1000 (the slow first notice the owner wanted) and `drainMs` to 2000, and a value shorter than one decide interval acts as one interval. Guards read the leading value as `brain.awareness`. Light, stance, movement and distance modifiers, investigate states and client HUD replication of the meter are a later stealth brief; this shape is the one they extend (owner).
- **`alwaysActive` keeps an NPC out of the dormant tier.** An optional behavior-root boolean, also settable per placed entity with the map key `always_active`, the placement winning. Such an NPC runs as idle or engaged whether or not it is perceived, so an unseen escort, race or scripted walker reaches triggers on time. It costs full rate for those NPCs only.
- **Visible NPCs keep acting at 60 Hz.** A lower acting rate measured too small a saving for its cost (`spike-visible-rates.md`). Presentation, hit tests, snapshots and the client interpolation delay are unchanged.
- **Mover carry and pushes reach a dormant NPC every tick.** Carry and displacement run each tick for a dormant NPC, in a pass apart from its 2 Hz sweep, so it stays planted on a moving lift or platform. Amends `movement.md` §6's single call order and its source-text gate for dormant NPCs.
- **Perceivable means inside some player's id-51 set.** The engine locates the cell of every player pawn in play or down, host and clients alike, and unions the `CellResidencySet` (id 51) cells reachable from those cells within an AI lead. The lead has an engine default the executor picks and reports; a map overrides it with the worldspawn key `_ai_wake_lead` (metres). An NPC is perceivable when its cell is in that union. The union is AI policy. The step under it, player pawn → cell → id-51 set, is engine-owned, keyed per client, and recorded as the intended input for network relevance and combat VFX culling. A gap that shows is fixed by refining id 51, never by a PVS bake (owner).
  - Owner choice over a new sightline bake. id 51 is sampled, so a cell it misses can still be drawn; an NPC there that acquires a player moves up after its next decide. One that cannot acquire the player (another faction, aggro gate closed, candidacy rejects) may step at 2 Hz in view; accepted as rare.
  - id 51 is streaming-owned; the AI lead is its own constant, never streaming's runtime L, so streaming retuning never moves AI detail. A lead, default or authored, above id 51's baked maximum is capped to it, with a warning for an authored one. Doors are not applied: closed doors only widen the set, the safe direction.
  - Armed ghost links couple their cells, per `networking.md` "Ghosts constrain future relevance": a perceived cell's linked cells join the union.
  - A level with no id 51, or a tick with no pawn in play or down, runs every NPC perceivable.
  - Monster closets get no special case. A closet NPC stays dormant until it enters the play field; waking at the reveal or after a teleport reads as fair disorientation (owner).
- **Tier changes.** An NPC moves up on the tick it becomes perceivable or gains a target. Damage and an opened aggro gate move it up on the tick they land when that is before the AI stage, otherwise on the next tick, and reach its brain at its next run. Moving up credits the time it has not run to its timers; it moves one ordinary step, so it never jumps. Moving down waits a short hold so an NPC at a set's edge does not flap. A dead NPC leaves the tiers; a revived one rejoins on its revive tick by the usual rules, with no credit for time dead.
- **The dormant tier never skips simulation.** Its whole tick runs with its step scaled. Diverges from `cell-visibility-substrate.md` "must NOT cull authoritative simulation" in letter, not intent: amended to permit a coarser authoritative tick and keep forbidding a skipped one.
- **The replan budget scales with demand.** Each tick's budget is `max(4, ⌈owed / fair wait⌉)`, owed counting NPCs in any replan tier, fair wait about 100 ms. Route-query cost per tick grows with the crowd.
- **Replan tiers outrank detail level.** The shared per-tick replan budget keeps its tier order (blocked route, then drift, then staleness) and its blocked-route wait bound for every NPC; within a replan tier, a paced NPC (a leap in flight) goes first, as today, then higher NPC tiers.
- **Perception decides the tier, never targets.** Keeps `perf-ai-tick-cost`'s rejection of a visibility broad phase inside target selection.
- **NPC is the umbrella term** for brain-bearing actors that are not player-controlled, enemies and allies alike (owner). This brief's rules apply to every NPC; the term lands in `entity_model.md`.
- **Docs amended at promotion:** `scripting.md` §11, `entity_model.md` §3, §5, §6 (an NPC's cell is re-located on every move, wherever it is cached), §7c (guards, selectors, slot choice and newly entered activities on decide ticks; strafe re-aiming each decide; the think stride becoming acquisition and decide pools plus `tacticalRange`; dormant steps for Pace, activity speed, release exit and "Clip time is game time"), `movement.md` §6, the substrate doc's no-cull rule and a note that the per-player id-51 step exists for its consumers, the roadmap's E15 Phase 4 and combat VFX culling entries pointing at that step without settling their miss tolerance; the no-PVS ground rule lands in `context/lib/`. Human-facing: `tacticalRange`, `notice`, `alwaysActive` and `brain.awareness` in the SDK types (`postretro.d.ts`, `postretro.d.luau`) and `docs/scripting-reference.md`; `always_active` and `_ai_wake_lead` in `docs/level_design.md`; plus `docs/scripting-reference.md` (guards "re-evaluated every tick", `acquisitionDue`'s think-stride ticks) and the guard doc comments in `sdk/lib/brain.ts` and `brain.luau` ("as of the previous tick", "true from the tick after").
- **Non-goals:**
  - Lower acting rates for perceived or engaged NPCs, and the host interpolation, drawn-pose hits and per-step snapshot wire they need: measured not worth it (`spike-visible-rates.md`); the design is kept in `research.md`.
  - Network relevance and combat VFX culling: later consumers of the per-player step, each deciding in its own brief whether id 51's sampling misses are tolerable for it (a miss here costs an NPC one decide; a missed entity in network relevance is never sent).
  - Projectiles: a projectile ticks at 60 Hz whoever fired it; tiers never step or steer projectiles.
  - A sightline PVS bake, a tighter id 46, or door-aware relevance. Owner ground rule: get runtime value from metadata the bake already produces (cells, portals, sampled visibility), even after its source such as the BSP is dropped, instead of adding a Quake-style PVS bake, historically the heaviest bake stage here and in Quake.
  - A hearing model or alert propagation between NPCs: NPCs wake as they do today, through damage, targets and the aggro gate.
  - Route-query cost inside the tactical range: `perf-ai-tick-cost` (exact cuts). Long-range routing over a coarser graph for very large maps: a later roadmap item.

### Scripting surface
```ts
behavior: {
  initial: "idle",
  moveSpeed: 3,
  engagementRadius: 4,
  tacticalRange: 60,      // optional, metres, default 100: beyond it the NPC heads for its target
  notice: { fillMs: 300, drainMs: 4000 },  // optional: a sharp-eyed sentry that stays suspicious
  alwaysActive: false,    // optional: true keeps it simulated at full rate when unseen
  activities: { /* ... */ },
  transitions: { /* ... */ },
},
```
```luau
behavior = {
  initial = "idle",
  moveSpeed = 3,
  engagementRadius = 4,
  tacticalRange = 60, -- optional, metres, default 100
  notice = { fillMs = 300, drainMs = 4000 },
  alwaysActive = false,
  activities = { --[[ ... ]] },
  transitions = { --[[ ... ]] },
},
```
```ts
// a guard reading the meter: turn to look while half aware, before the NPC acquires
{ to: "lookAround", when: brain.awareness.ge(0.5) }
```
Map keys: `always_active` (0/1) on an NPC placement; `_ai_wake_lead` (metres) on `worldspawn`.

Type docs: `notice`: "Optional. How quickly an NPC with no target notices a player or other candidate it may target: awareness rises while it can see them over `fillMs` (default 1000) and falls while it cannot over `drainMs` (default 2000); it targets them at full awareness. Each candidate has its own meter. Being hurt skips it." `brain.awareness`: "The NPC's highest awareness of any candidate, 0 to 1; 1 once it has a target." `alwaysActive`: "Optional. True keeps this NPC at full simulation rate while no player can perceive it. Defaults to false."
`tacticalRange`: "Optional distance in metres to the current target inside which an engaged NPC fights tactically (picks a spot around its target, flanks, strafes, re-targets at full rate); beyond it the NPC heads straight for its target and re-targets a third as often. Must be finite and > 0. Defaults to 100. A value below the largest attack standoff plus 10 m is raised to that, with a warning."

## Acceptance

### Automated
Timing on time:
- [ ] Each converted window expires on the first step at or past its duration at 60 and 2 Hz, and never on an earlier step (P18).
- [ ] While an NPC holding a turn skips its AI, its turn still counts against its pool on every skipped tick: no other NPC is granted that capacity and no refresh starts until the holder releases it; a turn that ends while it is not running is released on its next run, and a queued grant made while it is not running opens on its next run (P1).
- [ ] A queued attack granted to an engaged NPC opens its row at that NPC's next decide, for every decide phase; no turn stays held with its row closed, and other NPCs waiting on that pool are granted no later than one decide interval after their 60 Hz tick, and none starves (P27).
- [ ] A perceived idle NPC on a patrol route, or returning to its anchor, leaves each reached point on the tick it arrives, as at 60 Hz: it never stands at a point waiting for its next decide.
- [ ] An idle NPC with `fillMs` 1000 that sees a player continuously acquires them on the first decide at or past 1 s, and on no earlier one; one that loses sight at 0.6 s and regains it 0.5 s later, with `drainMs` 2000, acquires at about 1.75 s (0.6 filled, 0.25 drained, 0.65 refilled); a non-finite or negative `fillMs` or `drainMs` rejects at load (P31).
- [ ] With two players, an NPC that sees only one fills only that player's meter; the other player, stepping into view later, starts from zero (P31).
- [ ] An idle NPC hurt by a player acquires by today's damage and retaliation rules on its next decide, whatever its meter; an NPC with a target switches targets by today's ranking without a meter.
- [ ] `brain.awareness` reads the highest meter, 0 to 1, and 1 while the NPC has a target; a guard on `brain.awareness.ge(0.5)` fires before acquisition when sight lasts past half of `fillMs`.
- [ ] An `alwaysActive` NPC, set on the archetype or by `always_active 1` on its placement, never enters the dormant tier; `always_active 0` on a placement overrides an archetype's true. An unseen `alwaysActive` walker crosses a trigger volume on the same tick as at 60 Hz.
- [ ] A map with `_ai_wake_lead` uses it as the AI lead; one above the level's baked id-51 maximum is capped to it with a warning; a map without the key uses the engine default.
- [ ] An NPC that leaves the AI set (held, quiescent or despawning) still returns holding nothing, requesting nothing, with no wait clock; only a scheduler skip keeps its turns (P2).

Tiers and rates:
- [ ] Each tier runs each part of an NPC at exactly its table rate; mover carry and push run every tick for dormant NPCs.
- [ ] An allied NPC runs the same tiers and rates as an enemy.
- [ ] With many NPCs, every staggered acquisition, decide and dormant step is identical across frame splits, reversed spawn order (for NPCs with a map identity), and host versus replay.
- [ ] A perceived idle NPC's guards, including a patrol pause, a clip's `animationDone` exit and a release exit, run at 15 Hz.
- [ ] A reposition entered beyond its tactical range flanks within its arc once it chooses slots (P30).
- [ ] An NPC with a 5 m and a 120 m attack and no `tacticalRange` runs its line at 130 m, with the warning, whichever attack is selected.
- [ ] On the stress fixture, including NPCs from two spawners counting from zero, placements whose ordinals step by 4, and interleaved spawns, the busiest decide or dormant phase carries at most the per-phase mean plus 2√mean + 1, and no phase is empty at 64 NPCs or more (P15).
- [ ] Target acquisition, counted at the candidate scan, runs only on decide ticks, and `@brain.acquisitionDue` is true on each of them; an engaged NPC beyond its tactical range acquires on every third, for every spawn tick relative to its phase (P5).
- [ ] An engaged NPC beyond its tactical range claims no combat slot and routes toward its target under chase, strafe and reposition alike; none stands still for want of a slot. Its target coming 10 m inside the range, it chooses a slot at its next decide; a target stepping just past the range and back does not drop a held slot (P28). With `tacticalRange` omitted the line is at 100 m; authored at 60, at 60 m. On the multi-arena row, NPCs beyond their range report AI cost per NPC against an engaged NPC's inside it.
- [ ] `tacticalRange` rejects at load a non-finite or non-positive value, as other behavior fields do. A value below the largest attack standoff plus 10 m (30 m with a 45 m attack) runs at 55 m and warns, at load and on each hot reload that commits it; a reload that changes the value moves the line for live NPCs from their next decide, without dropping a held slot inside both lines. The Scripting surface example lowers identically in TypeScript and Luau.
- [ ] Measured from its entry tick, the clip-timed jab, the brute's combo (three jabs 320 ms apart after a 2,000 ms hold) and a leap land no earlier than their 60 Hz ticks and at most one decide interval later, for every decide phase and frame split; clip moments themselves still resolve on the tick they fall.
- [ ] A clip moment inside a dormant step resolves once, on that step, with its gates evaluated then; none is skipped or fired twice, except a repeating release whose passes inside one step whiff as they do inside one tick today.
- [ ] A dormant NPC riding a lift or pushed by a door moves with it every tick, carried exactly once on a tick it also steps: carried, pushed or blocking, never embedded or dropped through (P29).
- [ ] Tier assignment is identical on a headless host and on a listen host whose camera looks elsewhere.
- [ ] A dormant NPC wandering, patrolling or falling never passes through a wall, floor or ledge it would stop at under 60 Hz.
- [ ] A knocked-back dormant NPC travels the same distance at 2 Hz as at 60 Hz, within one step's travel, for every authored drag.
- [ ] Hit tests, host presentation, snapshots and the client interpolation delay for perceived NPCs are unchanged: the existing networking, hit-zone and walk-rate loopback suites pass unchanged.

Perception and tier changes:
- [ ] An NPC in a cell inside any player's id-51 set at the AI lead is perceived; one outside every player's set and with no target is dormant.
- [ ] Two players in different arenas each keep their own arena's NPCs perceived; a third, empty arena stays dormant.
- [ ] An NPC moves up on the same tick it becomes perceivable or gains a target, including mid step; an aggro gate opened by a trigger-bound command moves it up that tick, and one opened by a named reaction moves it up for the next tick's AI stage.
- [ ] Damage that lands before the AI stage (projectile impacts, remote hits ready at the input boundary) moves an NPC up that tick; damage during or after it (another NPC's attack, host hitscan, trap shots, remote hits awaiting that tick's fire authorization) moves it up for the next tick's AI stage (P6).
- [ ] An NPC that moves up part way through a dormant step has its timers advanced by the time it had not run and moves exactly one ordinary step (P3).
- [ ] One second after moving up part way through a dormant interval, an NPC's weapon resources, cooldowns, activity time, damage recency and attacker ledger read the same as an NPC that ran at 60 Hz throughout: no quantity is credited twice (P3, P11).
- [ ] A dormant NPC that gains a target on its own dormant run moves exactly one ordinary step in that tick's steering, and from the next tick runs engaged (P21).
- [ ] A dormant NPC checks acquisition and decides on every fourth dormant run and on no other tick; a perceived idle NPC checks acquisition and decides at 15 Hz (P4).
- [ ] A perceived idle NPC that is damaged, or whose aggro gate opens, acts on it at its next decide and runs engaged from then on.
- [ ] An NPC moves down only after the hold; one oscillating across a set edge does not flap.
- [ ] An unperceived NPC with a retained target stays engaged, and goes dormant only after losing its target and the hold.
- [ ] A dead NPC leaves the tiers; a revived one rejoins on its revive tick, and its timers are not credited for time dead.
- [ ] A player who teleports or respawns into a dormant arena on a tick has that arena's NPCs perceived on the same tick's AI stage (P7).
- [ ] A dormant NPC at the fastest shipped `moveSpeed` walking toward a player is perceived while still in the AI lead; it is never first seen already inside a cell visible from a player's cell (P8).
- [ ] An NPC spawned, re-using a freed slot, or moved by a script, mover or knockback is classified from its new cell on the next tick, and inherits no cell, hold or phase from a slot's previous occupant (P9).
- [ ] An unperceived hostile NPC whose candidacy admits a player, with line of sight to that player from outside the player's set, acquires the player on its next decide, then moves up (P20).
- [ ] Cells linked by an armed ghost link and teleporter arrival cells count as perceived; a disarmed ghost link does not couple.
- [ ] Pawns of seats in play or down count as perceivers, whatever their health or life state; loading and reconnecting seats have no pawn and do not; a tick with no counting pawn runs every NPC perceived (P17).
- [ ] A level loaded without id 51 runs every NPC perceived on every tick.
- [ ] The AI lead never exceeds the loaded level's id-51 maximum lead; a level baked with a smaller maximum uses that maximum.
- [ ] Changing streaming's runtime lead does not change which NPCs are dormant.
- [ ] A dormant NPC whose route a closed door blocks is served within the blocked-route wait bound even while higher-tier NPCs fill the budget with drift and staleness replans; within one replan tier, every paced request and then every higher-tier request is served first (P13).
- [ ] Replan budget, both sides: with 24 or fewer owed NPCs it is exactly 4; with 128 owed chasers it admits ⌈128 / 6⌉ = 22 per tick, the drift tier's fair wait is at most 6 ticks, and the gate tier's ⌈N / budget⌉ bound holds while gates flip.

Scale (reported per tier on `perf-ai-tick-cost`'s multi-arena row and the packed stress row, `sim_ai` and `sim_steer_agents` with its sub-stages; measured, not gated):
- [ ] Quake/Doom class (about 64 engaged in view, about 500 placed across arenas) and Serious Sam class (about 128 engaged, about 1,000 placed): the fixed step never spirals. AI and steering are reported against `perf-ai-tick-cost`'s goals and the spike's estimates; a miss is planned as an addendum, not a failed landing.
- [ ] Per-tick cost of an NPC in each tier, as a fraction of today's 60 Hz NPC, reported.
- [ ] Scale rows report the worst tick as well as the mean.
- [ ] A player teleporting into the most populated dormant arena moves every NPC there up on one tick without spiralling the fixed step; that tick's cost is reported (P14).
- [ ] The perception union costs no per-tick allocation, and its cost per tick is reported against player and NPC counts.
- [ ] Route queries per tick and their cost under the scaled replan budget, reported at 64 and 128 chasers on both rows; on the multi-arena row with far-band NPCs at 500 and 1,000 placed and moving targets, their route queries per tick and the replan wait of in-arena engaged NPCs are reported.

### Manual
- [ ] Listen-host and co-op client playtest on the multi-arena fixture: NPCs react at a fair pace, telegraphs read on time, and NPCs arriving from out of view look natural.

## Path
- Seams: `crates/ai/src/engine_floor.rs` `think_stride_for_distance`; `targeting::acquisition_due` and `BrainComponent::think_stride_counter`; `attack_turns::lifecycle::admit` (`seen_tick`) and `graph_eval::granted_last_tick` (`HeldTurn::granted_tick`, true on the NPC's next run after its grant); `perception` `LOS_GRACE_TICKS`; `combat_slots` `COMBAT_SLOT_HOLD_TICKS`; `launch` `FALL_DELAY_TICKS`; `agent_steering` `REPLAN_STALENESS_TICKS`, the shared replan budget and separation. The fuller per-system inventory, with how each handles time, is in `research.md`.
- Perception: `LevelWorld::locate_cell` per player pawn; id 51 stays loaded whatever the streaming mode. Re-locate an NPC's cell whenever its position changes, whatever moved it (P9).
- First slice: a 2 Hz dormant NPC against `collide_and_slide` and a lift ride, and decide pools on the stress row. The spike's throwaway stride patch (`spike-visible-rates.diff`) saw no tunnelling at 2 Hz.
- Pool key: registry slots follow spawn order through the free list, and the AI's per-tick index is a snapshot position, so neither works. `BehaviorDrawSalt` carries the identity, but only its hashed seed reaches the brain; store the salt word on the NPC at seeding and hash it for phase (a Fibonacci-style multiply spreads strided ordinals). Give the `Entity` fallback, also used by spawners with no map ordinal, an identity that does not follow spawn order.
- Notice meter and `perf-ai-tick-cost`'s early stop: an NPC without a target walks hostile candidates past the first eligible one, up to the meter's bound, testing line of sight for each; the early stop still serves NPCs with a target. Store the meters beside the attacker ledger on the brain.
- Queued grants: `graph_eval::granted_last_tick` compares `granted_tick + 1` with `seen_tick`; it becomes "granted since this NPC's last decide".
- Tests pinning ticks: `determinism_tests.rs` (extend its role-keyed reversed-`SpawnOrder` rows, `role_activity_rolls` and `role_hold_releases`, to phase), the clip-timed jab's tick pin, `shipped_brute_combo_lands_three_jabs_320_ms_apart_after_a_2000_ms_hold`, `release_repeat_tests`, `release_exit_tests`, `activity_speed_tests`, many AI tests set the stride counter by hand, `headless_byte_identity.rs`.
- Durations: store them as whole elapsed ticks, not floats, so replays and `headless_byte_identity.rs` stay byte-identical.
- Strongest rival, now rejected by measurement: the earlier draft of this brief, which also capped visible NPCs at 30 Hz with 15 Hz steering. Kept in `research.md`.
- Weaker rival: decide pools plus rest-based sleep, as physics engines sleep resting bodies; it does not cover patrollers or fallers in empty arenas.

## Open questions
- Moving-down hold length and the default AI lead over id 51 — **delegated**: the executor picks and reports both with measurements on the multi-arena row.
- Whether an NPC's cell is cached on the NPC or in a scheduler-side table — **delegated**.
