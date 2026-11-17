# E10--ai-tick-scheduling

Brief · resumable · Epic 10 · reads: `context/lib/entity_model.md` §3, §5, §6, §7c · `context/lib/scripting.md` §11 · `context/lib/rendering_pipeline.md` §Cluster SH residency, §12 · `context/lib/networking.md` · `context/lib/testing_guide.md` §Resource bounds · `context/research/cell-visibility-substrate.md` · `drafts/perf-ai-tick-cost/` · read at bf4f43f1b

**Sequence:** after `perf-ai-tick-cost` (exact cost cuts, same target on the same tick). Merges the roadmap's *Decision cadence* and *Arena relevance* items; *Parallel replans* follows.

## Problem
Owner-requested capability for an anticipated need (roadmap Epic 10, AI at scale): content today does not hit the limit, and `perf-ai-tick-cost` removes the scan behind most of the measured superlinear growth. Every enemy runs its whole tick at 60 Hz, seen or not, engaged or idle, and most enemy timing counts ticks rather than time, so no part of an enemy can run slower without changing behavior. Clients already see enemies only through 30 Hz snapshots, so most of that precision is spent where no player can see it. When done, enemy timing runs on time, each part of an enemy runs at the rate its visibility and engagement need, capped at 30 Hz, and players see the same interpolated enemies on host and client; a designer places Quake/Doom-class encounters at minimum, Serious Sam class as the attempted target, with no engine tuning.

## Decisions
- **Enemy timing runs on time, not tick counts.** Every tick-counted enemy window becomes a duration (inventory in `research.md`). An enemy keeps its attack turns across ticks it does not run. Behavior at 60 Hz is unchanged except where a decision here changes it.
- **Enemies are capped at 30 Hz; players, movers, projectiles and player weapons keep the 60 Hz tick.** Player movement and prediction are tuned per 60 Hz tick and compared tick by tick; movers carry and push players; projectiles need fine sweeps to be dodged and to hit where drawn. Riders and pushes on enemies are applied by the mover system every tick, whatever the enemy's rate.
- **Each part of an enemy runs at its tier's rate.** One per-enemy choice of tier, made on the host's fixed tick before the AI stage; it never reads the renderer, the host camera or frame timing.

  | Tier | Who | Decide | Steering, separation, target LOS, slot scoring, locomotion clip | Attacks, clip moments, facing and aim, hit reactions |
  |---|---|---|---|---|
  | Close | has a target within the close radius | 15 Hz | 30 Hz | 30 Hz |
  | Engaged | has a target beyond it, perceived or not | 15 Hz | 15 Hz | 30 Hz |
  | Idle | perceived, no target | 1 Hz | 15 Hz | 30 Hz |
  | Dormant | unperceived, no target | every 2 s | whole tick at 2 Hz | whole tick at 2 Hz |

  - Every rate and the close radius (about 5 m) are constants expected to be retuned on better test maps. Owner: a slow first notice reads as fair; perfect reflexes do not.
  - An engaged enemy past today's 30 m band acquires on every third decide.
  - 30 Hz steps fall on snapshot ticks, so clients see every enemy state. 15 Hz and slower steps are staggered by a pure function of per-enemy data that does not depend on spawn order or frame split, so assignment replays identically.
  - Amends `entity_model.md` §7c: guards evaluate on decide ticks, and the clip accumulator and moment check advance per 30 Hz step, firing each moment once.
- **Players see interpolated enemies everywhere.** The host draws each enemy between its last two steps, one step behind, as clients do; host hitscan tests the enemy pose the host drew, as client-declared hits already do. The client interpolation delay floor for enemies covers one 15 Hz step plus jitter, so a client never runs out of the next sample. Locomotion clips read interpolated velocity.
- **Perceivable means inside some player's id-51 set.** The engine locates the cell of every player pawn in play or down, host and clients alike, and unions the `CellResidencySet` (id 51) cells reachable from those cells within an AI lead. An enemy is perceivable when its cell is in that union. The union is AI policy. The step under it, player pawn → cell → id-51 set, is engine-owned and reusable; network relevance and combat VFX culling would key it per client, not union it.
  - Owner choice over a new sightline bake. id 51 is sampled, so a cell it misses can still be drawn; an enemy there that acquires a player promotes after its next decide. One that cannot acquire the player (another faction, aggro gate closed, candidacy rejects) may step at 2 Hz in view; accepted as rare.
  - id 51 is streaming-owned; the AI lead is its own constant, never streaming's runtime L, so streaming retuning never moves AI detail. It cannot exceed id 51's baked maximum lead. Doors are not applied: closed doors only widen the set, the safe direction.
  - Armed ghost links couple their cells, per `networking.md` "Ghosts constrain future relevance": a perceived cell's linked cells join the union.
  - A level with no id 51, or a tick with no pawn in play or down, runs every enemy perceivable.
  - Monster closets get no special case. A closet enemy stays dormant until it enters the play field; waking at the reveal or after a teleport reads as fair disorientation (owner).
- **Tier changes.** An enemy moves up on the tick it becomes perceivable or gains a target. Damage and an opened aggro gate move it up on the tick they land when that is before the AI stage, otherwise on the next tick, and reach its brain at its next run. Moving up credits the time it has not run to its timers; it moves one ordinary step, so it never jumps. Moving down waits a short hold so an enemy at a set's edge does not flap.
- **The dormant tier never skips simulation.** Its whole tick runs with its step scaled. Diverges from `cell-visibility-substrate.md` "must NOT cull authoritative simulation" in letter, not intent: amended to permit a coarser authoritative tick and keep forbidding a skipped one.
- **Replan tiers outrank detail level.** The shared per-tick replan budget keeps its tier order (blocked route, then drift, then staleness) and its blocked-route wait bound for every enemy; within a replan tier, higher enemy tiers go first.
- **Perception decides the tier, never targets.** Keeps `perf-ai-tick-cost`'s rejection of a visibility broad phase inside target selection.
- **Docs amended at promotion:** `scripting.md` §11, `entity_model.md` §3, §5, §6 (if an enemy's cell is stored on it), §7c (every every-tick clause), `networking.md` (enemy presentation and the host's drawn-pose hits), the substrate doc's no-cull rule; the no-PVS ground rule lands in `context/lib/`.
- **Non-goals:**
  - Network relevance and combat VFX culling: later consumers of the perception union.
  - A sightline PVS bake, a tighter id 46, or door-aware relevance. Owner ground rule: push PostRetro as far as it goes without a Quake-style PVS bake, historically the heaviest bake stage here and in Quake.
  - A hearing model or alert propagation between enemies: enemies wake as they do today, through damage, targets and the aggro gate.

## Acceptance

### Automated
Timing on time:
- [ ] Each converted window expires on the first step at or past its duration at 60, 30, 15 and 2 Hz, and never on an earlier step (P18).
- [ ] An enemy that runs no AI on a tick keeps its attack turns; one whose turn ends while it is not running releases it on its next run.
- [ ] While an enemy holding a turn skips its AI, its turn still counts against its pool on every skipped tick: no other enemy is granted that capacity, and no refresh starts, until the holder releases it (P1).
- [ ] An enemy that leaves the AI set (held, quiescent or despawning) still returns holding nothing, requesting nothing, with no wait clock; only a scheduler skip keeps its turns (P2).
- [ ] After one second, an enemy's weapon resources, cooldowns, activity time, damage recency and attacker ledger read the same at 60 Hz and in every tier: each advances once per elapsed tick (P11).

Tiers and rates:
- [ ] Each tier runs each part of an enemy at exactly its table rate, and nothing in an enemy runs faster than 30 Hz.
- [ ] Player movement, movers, projectiles and player weapons still run every 60 Hz tick.
- [ ] 30 Hz steps land on snapshot ticks; with many enemies, every staggered assignment and step tick is identical across frame splits, reversed spawn order, and host versus replay.
- [ ] On the stress fixture, including when enemies spawn interleaved or at a regular stride, the number of enemies stepping or deciding on any tick differs from the per-phase mean by at most one (P15).
- [ ] `@brain.acquisitionDue` is true only on decide ticks; an engaged enemy past 30 m acquires on every third, for every spawn tick relative to its phase (P5).
- [ ] The clip-timed jab's first damage lands at its clip moment, within one 30 Hz step, for every phase and frame split (P19).
- [ ] A clip moment that falls inside a 30 Hz or dormant step resolves once, on that step, with its gates evaluated then; none is skipped or fired twice.
- [ ] An enemy crossing the close radius changes steering rate on the next tick, without a jump in its drawn position.
- [ ] An enemy riding a lift or pushed by a door moves with it every tick in every tier: carried, pushed or blocking, never embedded or dropped through (P10).
- [ ] A dormant enemy wandering, patrolling or falling never passes through a wall, floor or ledge it would stop at under 60 Hz.

Presentation:
- [ ] The host draws every enemy between its last two steps; a 15 Hz enemy shows no hold-then-jump on host or client at the minimum client delay.
- [ ] A host hitscan shot aimed at an enemy's drawn pose hits it, at every tier and speed; one aimed where only its simulated pose stands misses.
- [ ] An enemy's locomotion clip does not flicker between walk and rest while it moves at a steady speed.

Perception and tier changes:
- [ ] An enemy in a cell inside any player's id-51 set at the AI lead is perceived; one outside every player's set and with no target is dormant.
- [ ] Two players in different arenas each keep their own arena's enemies perceived; a third, empty arena stays dormant.
- [ ] An enemy moves up on the same tick it becomes perceivable or gains a target, including mid step; an aggro gate opened by a trigger-bound command moves it up that tick, and one opened by a named reaction moves it up for the next tick's AI stage.
- [ ] Damage that lands before the AI stage (projectile impacts, remote hits ready at the input boundary) moves an enemy up that tick; damage during or after it (another enemy's attack, host hitscan, trap shots, remote hits awaiting that tick's fire authorization) moves it up for the next tick's AI stage (P6).
- [ ] An enemy that moves up part way through a step has its timers advanced by the time it had not run and moves exactly one ordinary step (P3).
- [ ] One second after moving up part way through a dormant interval, an enemy's weapon resources, cooldowns, activity time, damage recency and attacker ledger read the same as an enemy that ran at 60 Hz throughout: no quantity is credited twice (P3, P11).
- [ ] A dormant enemy that gains a target on its own dormant run moves exactly one ordinary step in that tick's steering, and from the next tick runs in its new tier (P21).
- [ ] A dormant enemy decides on every fourth dormant run and on no other tick; a perceived idle enemy decides once a second (P4).
- [ ] A perceived idle enemy that is damaged, or whose aggro gate opens, acts on it at its next decide and runs engaged from then on.
- [ ] An enemy moves down only after the hold; one oscillating across a set edge or the close radius does not flap.
- [ ] An unperceived enemy with a retained target stays engaged, and goes dormant only after losing its target and the hold.
- [ ] A player who teleports or respawns into a dormant arena on a tick has that arena's enemies perceived on the same tick's AI stage (P7).
- [ ] A dormant enemy at the fastest shipped `moveSpeed` walking toward a player is perceived while still in the AI lead; it is never first seen already inside a cell visible from a player's cell (P8).
- [ ] An enemy spawned, re-using a freed slot, or moved by a script, mover or knockback is classified from its new cell on the next tick, and inherits no cell, hold or phase from a slot's previous occupant (P9).
- [ ] An unperceived hostile enemy whose candidacy admits a player, with line of sight to that player from outside the player's set, acquires the player on its next decide, then moves up (P20).
- [ ] Cells linked by an armed ghost link and teleporter arrival cells count as perceived; a disarmed ghost link does not couple.
- [ ] Pawns of seats in play or down count as perceivers, whatever their health or life state; loading and reconnecting seats have no pawn and do not; a tick with no counting pawn runs every enemy perceived (P17).
- [ ] A level loaded without id 51 runs every enemy perceived on every tick.
- [ ] The AI lead never exceeds the loaded level's id-51 maximum lead; a level baked with a smaller maximum uses that maximum.
- [ ] Changing streaming's runtime lead does not change which enemies are dormant.
- [ ] A dormant enemy whose route a closed door blocks is served within the blocked-route wait bound even while higher-tier enemies fill the budget with drift and staleness replans; within one replan tier, every higher-tier request is served first (P13).

Scale (reported per tier on a `tools/gen_stress_map.py` multi-arena fixture, `sim_ai` and `sim_steer_agents` with the new sub-stages):
- [ ] Quake/Doom class (about 64 engaged in view, about 500 placed across arenas): the fixed step never spirals and AI plus steering holds `perf-ai-tick-cost`'s budgets.
- [ ] Serious Sam class (about 128 engaged, about 1,000 placed): measured and reported, not gated.
- [ ] Per-tick cost of an enemy in each tier, as a fraction of today's 60 Hz enemy, reported.
- [ ] A player teleporting into the most populated dormant arena moves every enemy there up on one tick without spiralling the fixed step; that tick's cost is reported (P14).
- [ ] The perception union costs no per-tick allocation, and its cost per tick is reported against player and enemy counts.

### Manual
- [ ] Listen-host and co-op client playtest on the stress fixture: enemies move smoothly at every tier; close melee does not feel soft; telegraphs read on time; enemies arriving from out of view look natural.
- [ ] Shooting a running enemy on the host lands where it is drawn.

## Path
- Seams: `crates/ai/src/engine_floor.rs` `think_stride_for_distance`; `targeting::acquisition_due` and `BrainComponent::think_stride_counter`; `attack_turns::lifecycle::admit` (`seen_tick`); `perception` `LOS_GRACE_TICKS`; `combat_slots` `COMBAT_SLOT_HOLD_TICKS`; `launch` `FALL_DELAY_TICKS`; `agent_steering` `REPLAN_STALENESS_TICKS`, the shared replan budget and separation. The fuller per-system inventory, with how each handles time, is in `research.md`.
- Presentation: host enemies draw through `EntityRegistry::snapshot_transforms` and `interpolated_transform` in `mesh_render`; clients through `RemoteInterpolationBuffer` (`MIN_DELAY_MICROS`). Unchanged enemies are left out of snapshots, so a 15 Hz enemy appears in every second one.
- Perception: `LevelWorld::locate_cell` per player pawn; id 51 stays loaded whatever the streaming mode. Re-locate an enemy's cell whenever its position changes, whatever moved it (P9).
- First slice: a 15 Hz and a 2 Hz enemy against `collide_and_slide`, separation in a packed horde, and a lift ride. These are the riskiest assumptions.
- Pool key: registry slots follow spawn order through the free list, and the AI's per-tick index is a snapshot position, so neither works; a map placement index for placed enemies and spawner plus spawn sequence for spawned ones are candidates.
- Tests pinning ticks: `determinism_tests.rs` (one enemy, so any phase key passes by accident; add many), the clip-timed jab's tick pin, many AI tests set the stride counter by hand, `headless_byte_identity.rs`.
- Durations: store them as whole elapsed ticks, not floats, so replays and `headless_byte_identity.rs` stay byte-identical.
- Strongest rival: decide pools plus rest-based sleep, as physics engines sleep resting bodies. No id-51 dependency and no dormant step, and it covers idle enemies in view, but not patrollers or fallers in empty arenas. Rejected by the owner's dormant tier, 30 Hz cap and build-at-target-scale stance.

## Open questions
- Moving-down hold length and the AI lead over id 51 — **delegated**: the executor picks and reports both with measurements on the stress fixture.
- Whether an enemy's cell is cached on the enemy or in a scheduler-side table — **delegated**.
