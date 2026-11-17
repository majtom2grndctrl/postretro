# E10--ai-tick-scheduling

Brief · resumable · Epic 10 · reads: `context/lib/entity_model.md` §3, §5, §6, §7c · `context/lib/scripting.md` §11 · `context/lib/rendering_pipeline.md` §Cluster SH residency, §12 · `context/lib/networking.md` · `context/lib/testing_guide.md` §Resource bounds · `context/research/cell-visibility-substrate.md` · `ready/perf-ai-tick-cost/` · read at bf4f43f1b

**Sequence:** after `perf-ai-tick-cost` (exact cost cuts, same target on the same tick). Merges the roadmap's *Decision cadence* and *Arena relevance* items; *Parallel replans* follows.

## Problem
Owner-requested capability for an anticipated need (roadmap Epic 10, AI at scale): content today does not hit the limit, and `perf-ai-tick-cost` removes the pairwise target scan and the repeated positioning work behind the measured superlinear growth. Every NPC runs its whole tick at 60 Hz, seen or not, engaged or idle, and most NPC timing counts ticks rather than time, so no part of an NPC can run slower without changing behavior. Clients see NPCs only through 30 Hz snapshots and dormant NPCs are seen by no one, so much of that precision buys nothing a player can see. When done, NPC timing runs on time, each part of an NPC costs only what its visibility and engagement need, and a designer places Quake/Doom-class encounters at minimum, Serious Sam class as the attempted target, with no engine tuning.

## Decisions
- **NPC timing runs on time, not tick counts.** Every tick-counted NPC window becomes a duration (inventory in `research.md`). An NPC keeps its attack turns across ticks it does not run. Behavior at 60 Hz is unchanged except where a decision here changes it.
- **NPCs are capped at 30 Hz; players, movers, projectiles and player weapons keep the 60 Hz tick.** Player movement and prediction are tuned per 60 Hz tick and compared tick by tick; movers carry and push players; projectiles need fine sweeps to be dodged and to hit where drawn.
- **Mover carry and pushes reach every NPC every tick.** Carry and displacement run each tick for every NPC, in a pass apart from its rate-limited sweep, so an NPC fighting on a moving platform or firing from a moving craft stays planted on it. A rider's tick pose interpolates in its mover's local frame and is placed by the mover's current pose, on the host and on clients. This brief builds that client-side rebasing for NPC riders, shaped for `E17--mover-relative-rider-presentation` to reuse for players. Amends `movement.md` §6's single call order and its source-text gate for NPCs.
- **Each part of an NPC runs at its tier's rate.** One per-NPC choice of tier, made on the host's fixed tick before the AI stage; it never reads the renderer, the host camera or frame timing.

  | Tier | Who | Decide | Steering, separation, target LOS, slot scoring, locomotion clip | Attacks, clip moments, facing and aim, hit reactions |
  |---|---|---|---|---|
  | Close | has a target within the close radius | 15 Hz | 30 Hz | 30 Hz |
  | Engaged | has a target beyond it, perceived or not | 15 Hz | 15 Hz | 30 Hz |
  | Idle | perceived, no target | 1 Hz | 15 Hz | 30 Hz |
  | Dormant | unperceived, no target | every 2 s | whole tick at 2 Hz | whole tick at 2 Hz |

  - Every rate and the close radius (about 5 m) are constants expected to be retuned on better test maps. Owner: a slow first notice reads as fair; perfect reflexes do not.
  - An engaged NPC whose target is beyond 100 m acquires on every third decide.
  - NPC steps and the snapshot cadence count the same simulation tick, and each step's state is recorded at its tick. A snapshot carries, for each NPC, every step since the last send, each stamped with its own tick, so clients never miss an NPC state, even on a slow host frame. 15 Hz and slower steps are staggered by a pure function of per-NPC data that does not depend on spawn order or frame split, so assignment replays identically.
  - Amends `entity_model.md` §7c: guards evaluate on decide ticks, and the clip accumulator and moment check advance per 30 Hz step, firing each moment once.
- **Every damage check against or by an NPC uses its tick pose.** An NPC's tick pose is its root and skeletal pose interpolated between its last two steps at that tick, the skeleton posed from its animation state's own time (flinches and crossfades included) advanced in game time, so it is deterministic and replays identically. Hitscan, projectile contact, splash, hit zones, the NPC's own melee reach and muzzle and launch origins, and the fire and range gates that clear its attacks all test it; what a player sees is what they hit and what hits them. Physics (doors, crush, pins, separation) and the sight an NPC acquires targets with keep the simulated pose, where the NPC physically is. The host draws the tick pose between ticks, one step behind the simulation, as clients do. The client interpolation delay floor for NPCs covers one 15 Hz step plus jitter, so a client never runs out of the next sample. Locomotion clips read interpolated velocity.
- **Perceivable means inside some player's id-51 set.** The engine locates the cell of every player pawn in play or down, host and clients alike, and unions the `CellResidencySet` (id 51) cells reachable from those cells within an AI lead. An NPC is perceivable when its cell is in that union. The union is AI policy. The step under it, player pawn → cell → id-51 set, is engine-owned and the intended input for network relevance and combat VFX culling, keyed per client. Those consumers accept id 51's sampling; a gap that shows is fixed by refining id 51, never by a PVS bake (owner). Diverges from `cell-visibility-substrate.md`, which names id 46 and zero false negatives for them; amended at promotion.
  - Owner choice over a new sightline bake. id 51 is sampled, so a cell it misses can still be drawn; an NPC there that acquires a player promotes after its next decide. One that cannot acquire the player (another faction, aggro gate closed, candidacy rejects) may step at 2 Hz in view; accepted as rare.
  - id 51 is streaming-owned; the AI lead is its own constant, never streaming's runtime L, so streaming retuning never moves AI detail. It cannot exceed id 51's baked maximum lead. Doors are not applied: closed doors only widen the set, the safe direction.
  - Armed ghost links couple their cells, per `networking.md` "Ghosts constrain future relevance": a perceived cell's linked cells join the union.
  - A level with no id 51, or a tick with no pawn in play or down, runs every NPC perceivable.
  - Monster closets get no special case. A closet NPC stays dormant until it enters the play field; waking at the reveal or after a teleport reads as fair disorientation (owner).
- **Tier changes.** An NPC moves up on the tick it becomes perceivable or gains a target. Damage and an opened aggro gate move it up on the tick they land when that is before the AI stage, otherwise on the next tick, and reach its brain at its next run. Moving up credits the time it has not run to its timers; it moves one ordinary step, so it never jumps. Moving down waits a short hold so an NPC at a set's edge does not flap.
- **A dead NPC leaves the tiers.** On death it becomes a body whose motion starts from its tick pose and runs every tick, so its drawn body and its hits agree from the first tick, and ragdoll physics can later start from the same pose. Clients see the body through ordinary snapshots. A revived NPC rejoins the tiers on its revive tick at its corpse's live pose by the usual rules; time spent dead credits no timers.
- **The dormant tier never skips simulation.** Its whole tick runs with its step scaled. Diverges from `cell-visibility-substrate.md` "must NOT cull authoritative simulation" in letter, not intent: amended to permit a coarser authoritative tick and keep forbidding a skipped one.
- **The replan budget scales with demand.** Each tick's budget is `max(4, ⌈owed / fair wait⌉)`, owed counting NPCs in any replan tier, with the fair wait about 100 ms as a constant stored in whole ticks. Up to 24 owed, the budget stays 4. At 128 chasers the wait falls from about 0.4 s to 100 ms, and the route-query cost per tick grows with the crowd. Moved here from `perf-ai-tick-cost`, which keeps results identical (owner, 2026-11-17).
- **Replan tiers outrank detail level.** The shared per-tick replan budget keeps its tier order (blocked route, then drift, then staleness) and its blocked-route wait bound for every NPC; within a replan tier, higher NPC tiers go first.
- **Perception decides the tier, never targets.** Keeps `perf-ai-tick-cost`'s rejection of a visibility broad phase inside target selection.
- **NPC is the umbrella term** for brain-bearing actors that are not player-controlled, enemies and allies alike (owner). This brief's rules apply to every NPC; the term lands in `entity_model.md`.
- **Docs amended at promotion:** `scripting.md` §11, `entity_model.md` §3, §5, §6 (if an NPC's cell is stored on it), §7c (every every-tick clause), `movement.md` §6, `networking.md` (NPC presentation, drawn-pose hits, and per-step NPC states in the snapshot wire), the substrate doc's no-cull rule and its consumer relation, roadmap E15 Phase 4 and combat VFX culling (id 51, not id 46); the no-PVS ground rule lands in `context/lib/`.
- **Non-goals:**
  - Network relevance and combat VFX culling: later consumers of the per-player step.
  - A sightline PVS bake, a tighter id 46, or door-aware relevance. Owner ground rule: get runtime value from metadata the bake already produces (cells, portals, sampled visibility), even after its source such as the BSP is dropped, instead of adding a Quake-style PVS bake, historically the heaviest bake stage here and in Quake.
  - A hearing model or alert propagation between NPCs: NPCs wake as they do today, through damage, targets and the aggro gate.

## Acceptance

### Automated
Timing on time:
- [ ] Each converted window expires on the first step at or past its duration at 60, 30, 15 and 2 Hz, and never on an earlier step (P18).
- [ ] While an NPC holding a turn skips its AI, its turn still counts against its pool on every skipped tick: no other NPC is granted that capacity and no refresh starts until the holder releases it; a turn that ends while it is not running is released on its next run (P1).
- [ ] An NPC that leaves the AI set (held, quiescent or despawning) still returns holding nothing, requesting nothing, with no wait clock; only a scheduler skip keeps its turns (P2).

Tiers and rates:
- [ ] Each tier runs each part of an NPC at exactly its table rate; only mover carry and push, tick-pose evaluation and corpse motion run every 60 Hz tick.
- [ ] An allied NPC runs the same tiers, rates, tick pose and corpse rules as an enemy.
- [ ] Player movement, movers, projectiles and player weapons still run every 60 Hz tick.
- [ ] Every NPC step reaches clients stamped with its own tick, including on catch-up frames that run several ticks (P26); with many NPCs, every staggered assignment and step tick is identical across frame splits, reversed spawn order, and host versus replay.
- [ ] On the stress fixture, including when NPCs spawn interleaved or at a regular stride, the number of NPCs stepping or deciding on any tick differs from the per-phase mean by at most one (P15).
- [ ] `@brain.acquisitionDue` is true only on decide ticks; an engaged NPC whose target is beyond 100 m acquires on every third, for every spawn tick relative to its phase (P5).
- [ ] The clip-timed jab's first damage lands at its clip moment, within one 30 Hz step, for every phase and frame split (P19).
- [ ] A clip moment that falls inside a 30 Hz or dormant step resolves once, on that step, with its gates evaluated then; none is skipped or fired twice.
- [ ] An NPC crossing the close radius changes steering rate on the next tick.
- [ ] An NPC whose XZ distance from its tick pose to its target's position at the tier choice equals the close radius is close; one just beyond it is engaged.
- [ ] Tier assignment is identical on a headless host and on a listen host whose camera looks elsewhere.
- [ ] An NPC riding a lift or pushed by a door moves with it every tick in every tier: carried, pushed or blocking, never embedded or dropped through (P10).
- [ ] An NPC riding a platform moving at 10 m/s at 15 Hz or 2 Hz is drawn and hit standing on the platform on every tick, never lagging, leading or sunk into it; a standing rider's offset from the platform is unchanged across its step ticks, and its shots leave from where it is drawn.
- [ ] An idle 15 Hz NPC riding a rotating mover is drawn and hit at its carried position on every tick, and its drawn yaw never turns with the mover.
- [ ] An NPC that walks off a moving platform onto the floor, or from the floor onto it, within one 15 Hz or 2 Hz step shows no jump in its drawn or hit pose on any tick of that step (P25).
- [ ] An NPC killed between steps becomes a corpse at its tick pose: no jump in drawn position, the corpse then moves every tick, and a still-hittable corpse is hit where it is drawn on every tick.
- [ ] An NPC killed on a tick it was due to step, by damage before the AI stage or after its step, becomes a corpse with no jump in drawn position and no step taken after death.
- [ ] An NPC killed while riding a platform moving at 10 m/s, at 15 Hz or 2 Hz, is drawn on the platform through its change into a corpse, never lagging or sunk into it.
- [ ] A revived NPC re-enters the tiers on its revive tick at its corpse's live pose without a jump, and its timers are not credited for time dead.
- [ ] On a co-op client, an NPC riding a platform moving at 10 m/s is drawn standing on it on every frame, never lagging or sunk into it.
- [ ] A still-hittable corpse's skeleton and hit zones, death clip included, are identical across frame splits and on replay; a headshot on a corpse lands where its head is drawn.
- [ ] A dormant NPC wandering, patrolling or falling never passes through a wall, floor or ledge it would stop at under 60 Hz.
- [ ] A knocked-back NPC travels the same distance at 30, 15 and 2 Hz as at 60 Hz, within one step's travel, for every authored drag.

Presentation:
- [ ] The host draws every NPC between its last two steps; a 15 Hz NPC shows no hold-then-jump on host or client at the minimum client delay.
- [ ] Host hitscan, a host projectile's direct contact and splash each hit an NPC at its tick pose at every tier; against an NPC moving at least half its run speed, a shot aimed where only its simulated pose stands misses.
- [ ] A projectile and a host hitscan striking the same NPC on the tick it steps test the same pose (P24).
- [ ] An NPC spawned, teleported or set by a script is drawn and hit at its new pose from that tick, never along a path from its old pose (P22).
- [ ] Across every tier change, up or down and mid step, an NPC's tick pose never jumps or moves backwards, and hits on it land where it is drawn on each tick of the change (P23).
- [ ] On the host, an NPC's mesh draw, portal cull, ghost-link draw, attached lights and sound anchors all use its tick pose; a running 15 Hz NPC is never culled while its drawn pose is in view, and its sounds and lights do not lead its body.
- [ ] Raising the NPC interpolation floor leaves remote player pawns, movers and projectiles at today's delay on the same link.
- [ ] An NPC's melee reach and its muzzle and launch origins are measured from its tick pose.
- [ ] Tick poses, skeleton and hit zones included, are identical across frame splits and on replay; a headshot on a 15 Hz NPC lands where its head is drawn.
- [ ] A headshot on a 15 Hz NPC mid flinch and mid crossfade lands where its head is drawn, identically across frame splits and on replay.
- [ ] An NPC's fire gate and range gate test from its tick pose: a shot the gate clears is never stopped by geometry between its tick-pose muzzle and its target, and a melee whose range gate passed reaches what is drawn in range.
- [ ] Doors, crush, pins, separation and target acquisition act on an NPC's simulated pose, not its tick pose: its acquisition line of sight is cast from its simulated eye.
- [ ] An NPC's locomotion clip does not flicker between walk and rest while it moves at a steady speed.

Perception and tier changes:
- [ ] An NPC in a cell inside any player's id-51 set at the AI lead is perceived; one outside every player's set and with no target is dormant.
- [ ] Two players in different arenas each keep their own arena's NPCs perceived; a third, empty arena stays dormant.
- [ ] An NPC moves up on the same tick it becomes perceivable or gains a target, including mid step; an aggro gate opened by a trigger-bound command moves it up that tick, and one opened by a named reaction moves it up for the next tick's AI stage.
- [ ] Damage that lands before the AI stage (projectile impacts, remote hits ready at the input boundary) moves an NPC up that tick; damage during or after it (another NPC's attack, host hitscan, trap shots, remote hits awaiting that tick's fire authorization) moves it up for the next tick's AI stage (P6).
- [ ] An NPC that moves up part way through a step has its timers advanced by the time it had not run and moves exactly one ordinary step (P3).
- [ ] One second after moving up part way through a dormant interval, an NPC's weapon resources, cooldowns, activity time, damage recency and attacker ledger read the same as an NPC that ran at 60 Hz throughout, in every tier and after moving up part way through a dormant interval: no quantity is credited twice (P3, P11).
- [ ] A dormant NPC that gains a target on its own dormant run moves exactly one ordinary step in that tick's steering, and from the next tick runs in its new tier (P21).
- [ ] A dormant NPC decides on every fourth dormant run and on no other tick; a perceived idle NPC decides once a second (P4).
- [ ] A perceived idle NPC that is damaged, or whose aggro gate opens, acts on it at its next decide and runs engaged from then on.
- [ ] An NPC moves down only after the hold; one oscillating across a set edge or the close radius does not flap.
- [ ] An unperceived NPC with a retained target stays engaged, and goes dormant only after losing its target and the hold.
- [ ] A player who teleports or respawns into a dormant arena on a tick has that arena's NPCs perceived on the same tick's AI stage (P7).
- [ ] A dormant NPC at the fastest shipped `moveSpeed` walking toward a player is perceived while still in the AI lead; it is never first seen already inside a cell visible from a player's cell (P8).
- [ ] An NPC spawned, re-using a freed slot, or moved by a script, mover or knockback is classified from its new cell on the next tick, and inherits no cell, hold or phase from a slot's previous occupant (P9).
- [ ] An unperceived hostile NPC whose candidacy admits a player, with line of sight to that player from outside the player's set, acquires the player on its next decide, then moves up (P20).
- [ ] Cells linked by an armed ghost link and teleporter arrival cells count as perceived; a disarmed ghost link does not couple.
- [ ] Pawns of seats in play or down count as perceivers, whatever their health or life state; loading and reconnecting seats have no pawn and do not; a tick with no counting pawn runs every NPC perceived (P17).
- [ ] A level loaded without id 51 runs every NPC perceived on every tick.
- [ ] The AI lead never exceeds the loaded level's id-51 maximum lead; a level baked with a smaller maximum uses that maximum.
- [ ] Changing streaming's runtime lead does not change which NPCs are dormant.
- [ ] A dormant NPC whose route a closed door blocks is served within the blocked-route wait bound even while higher-tier NPCs fill the budget with drift and staleness replans; within one replan tier, every higher-tier request is served first (P13).
- [ ] Replan budget, both sides: with 24 or fewer owed NPCs it is exactly 4; with 128 owed chasers it admits ⌈128 / 6⌉ = 22 per tick, the drift tier's fair wait is at most 6 ticks, and the gate tier's ⌈N / budget⌉ bound holds while gates flip.

Scale (reported per tier on a `tools/gen_stress_map.py` multi-arena fixture, `sim_ai` and `sim_steer_agents` with the new sub-stages):
- [ ] Quake/Doom class (about 64 engaged in view, about 500 placed across arenas): the fixed step never spirals; AI and steering are reported against `perf-ai-tick-cost`'s goals, and a miss is planned as an addendum, not a failed landing.
- [ ] Serious Sam class (about 128 engaged, about 1,000 placed): measured and reported, not gated.
- [ ] Per-tick cost of an NPC in each tier, as a fraction of today's 60 Hz NPC, reported.
- [ ] Scale rows report the worst tick, the one carrying every 30 Hz part, as well as the mean, and the fixed step does not spiral on it.
- [ ] A player teleporting into the most populated dormant arena moves every NPC there up on one tick without spiralling the fixed step; that tick's cost is reported (P14).
- [ ] The perception union costs no per-tick allocation, and its cost per tick is reported against player and NPC counts.
- [ ] Route queries per tick and their cost under the scaled replan budget, reported at 64 and 128 chasers on the stress row and on the multi-arena fixture's largest nav graph.

### Manual
- [ ] Listen-host and co-op client playtest on the stress fixture: NPCs move smoothly at every tier; close melee does not feel soft; telegraphs read on time; NPCs arriving from out of view look natural.
- [ ] Shooting a running NPC on the host lands where it is drawn, and an NPC's melee connects only when it looks in reach.

## Path
- Seams: `crates/ai/src/engine_floor.rs` `think_stride_for_distance`; `targeting::acquisition_due` and `BrainComponent::think_stride_counter`; `attack_turns::lifecycle::admit` (`seen_tick`); `perception` `LOS_GRACE_TICKS`; `combat_slots` `COMBAT_SLOT_HOLD_TICKS`; `launch` `FALL_DELAY_TICKS`; `agent_steering` `REPLAN_STALENESS_TICKS`, the shared replan budget and separation. The fuller per-system inventory, with how each handles time, is in `research.md`.
- Presentation: host NPCs draw through `EntityRegistry::snapshot_transforms` and `interpolated_transform` in `mesh_render`; clients through `RemoteInterpolationBuffer` (`MIN_DELAY_MICROS`). Snapshots today ingest and send once per frame under one tick stamp, and unchanged NPCs are left out.
- Perception: `LevelWorld::locate_cell` per player pawn; id 51 stays loaded whatever the streaming mode. Re-locate an NPC's cell whenever its position changes, whatever moved it (P9).
- First slice: a 15 Hz and a 2 Hz NPC against `collide_and_slide`, separation in a packed horde, and a lift ride. These are the riskiest assumptions.
- Pool key: registry slots follow spawn order through the free list, and the AI's per-tick index is a snapshot position, so neither works; a map placement index for placed NPCs and spawner plus spawn sequence for spawned ones are candidates.
- Tests pinning ticks: `determinism_tests.rs` (one NPC, so any phase key passes by accident; add many), the clip-timed jab's tick pin, many AI tests set the stride counter by hand, `headless_byte_identity.rs`.
- Durations: store them as whole elapsed ticks, not floats, so replays and `headless_byte_identity.rs` stay byte-identical.
- Strongest rival: the roadmap's own shape plus the dormant tier: decide at about 15 Hz, keep acting (steering, clip time, attacks) at 60 Hz for every perceived or engaged NPC. It needs no host interpolation, no drawn-pose hits, no raised client delay and no 33 ms clip quantization. It loses on cost: once `perf-ai-tick-cost` removes the nav scan (most of the 2.5 ms measured for 64 agents), steering's own work (`collide_and_slide`, pairwise separation) still grows with the crowd at 60 Hz. At Serious Sam class that is the bill the 30 Hz cap and 15 Hz steering avoid, at the price of the presentation contract. Re-measure the rival on the stress row once that brief lands.
- Weaker rival: decide pools plus rest-based sleep, as physics engines sleep resting bodies; it does not cover patrollers or fallers in empty arenas.

## Open questions
- Moving-down hold length and the AI lead over id 51 — **delegated**: the executor picks and reports both with measurements on the stress fixture.
- Whether an NPC's cell is cached on the NPC or in a scheduler-side table — **delegated**.
