# E10--enemy-combat-movement

Brief · resumable · Epic 10 · reads: `context/lib/entity_model.md` §7c · `context/lib/scripting.md` §1, §11, §12 · `context/lib/networking.md` §Combat authority · `context/lib/build_pipeline.md` §Custom FGD, §PRL section IDs · read at 5932287f

## Problem
The owner wants combat that is fun to fight and that level designers can build encounters around. This is a requested capability plus one observed defect.

Today an engaged enemy can only chase, hold, or walk to a fixed point. It cannot strafe while shooting, fire and then relocate, or wait its turn. Every enemy in range attacks at once, so a monster closet's payload reads as noise rather than a pattern a player can learn.

The defect: a rocket-launched enemy turns to face its flight path, because knockback velocity feeds the facing and locomotion choice.

When this is done:
- An author gives enemies strafing and fire-then-relocate behavior that reads correctly on screen.
- A per-target attack budget, seeded by the engine, tunable per mod and given its own pool by a map-authored encounter, keeps idle enemies moving instead of piling on.
- A launched enemy keeps the yaw it had at impact.

## Decisions
- **Two new engaged motion verbs, `strafe` and `reposition`.** Both resolve goals on the combat-slot ring (`plans/done/E10--enemy-combat-positioning`) at the standoff `chaseTarget` uses. Both hold the target and may carry an action, unlike position goals (`entity_model.md` §7c). They stay closed and parameterless.
  - **`strafe`** steps to the neighboring ring slot and keeps going in one direction, reversing when the next slot is unreachable, occupied or blind.
  - **`reposition`** claims the best slot at least one spacing from both its held slot and its position at entry. It picks again on each entry and holds on arrival.
- **Per-activity `facing`: `"target"` or `"travel"`.** Omitted, the verb's default applies. Existing verbs keep today's rule, and `strafe` and `reposition` face the target. A target-facing mover passes the shipped facing gate unchanged.
- **Knockback never steers facing or picks a stride.** "Launched" means knockback velocity remains.
  - While launched, facing ignores both steering and knockback and holds the impact yaw, except that a target-facing activity re-aims at the normal slew rate.
  - An airborne launch plays `jump_loop` when the mesh declares one, after the player's fall delay (`MeshDescriptor.animations` typedoc). Otherwise it keeps its current state.
- **Directional travel variants.** Under target facing, an activity animation `X` resolves to `X_left`, `X_right` or `X_backward` by travel direction relative to facing, and plays `X` when a variant is missing. The host resolves the name, and it replicates as an ordinary animation state. This extends the player's `walk_backward` naming.
- **The attack budget is an engine mechanism with authored values** (`scripting.md` §1: pacing is taste; the engine seeds a default).
  - **Pools.** Every enemy draws from one pool. Members of the active encounter draw from the encounter's pool, and everyone else from the default pool. Each pool has one budget and keeps its own held cost per target, so the two pools stack on one player.
  - **Grants.** An enemy is granted a turn when its pool's held cost on that target plus its own cost fits the pool's budget, or when its pool holds no counted turn on that target, so an oversized archetype is never locked out.
  - **The default pool's budget** is the mod's `attackBudget`, else the engine's seeded 3. The seeded value is tuning, not contract.
  - **Valid values.** A budget must be at least 1. `attackCost` defaults to 1. Cost 0 never counts and is always granted: the lever for fodder that chips while heavies take turns.
  - **Host-only, and outside the co-op compatibility digest.** Clients never read the budget.
  - A literal for now. A store-derived expression can be added later without breaking anything; defense layers are the precedent.
- **An encounter is a map entity, `ai_encounter`** (`@SolidClass`): `_tags`, `encounter_tag`, and `attack_budget` (empty takes the mod value).
  - **At most one is active per level.** Starting one ends the active one. Existing trigger `command` `start` and `stop` on its `_tags` start and end it, and scripts drive the same member as `getMapEntities("encounter")` with `start` and `stop`, as trap members do. `start` on the active encounter and `stop` on an inactive one are no-ops. Both apply on the host only, and a level starts with none active.
  - **Membership.** An enemy is a member while it carries `encounter_tag`, live, so later spawns join through `spawned_tags`. An enemy inside the entity's brushes when it starts is also a member until the encounter ends; leaving the volume does not remove it, and arriving later does not add it. The volume test runs once per start, one point-in-brush test per AI enemy, with no per-tick cost.
  - **Ending.** On `stop`, or on replacement, members return to the default pool.
  - **Validation.** An invalid `attack_budget` warns and takes the mod value. An empty `encounter_tag` is a compile error naming the entity, since tag membership is the encounter's identity; a volume-only encounter is not a case.
  - The active encounter and its volume members are serializable state: they survive a serialize-and-restore.
  - **Load-time declaration.** The entity seeds level data; scripts and triggers change only which encounter is active (`build_pipeline.md` §Load-time gameplay declarations gains it beside `trigger_teleport`).
- **Turn lifetime.**
  - **Marked activities.** An activity marked `attackTurn: true` requests on entry and keeps requesting until granted. The turn holds while any marked activity is on the active path, so a windup → commit → recover chain holds one turn.
  - **Unmarked attacks.** With no marked activity on its path, an attack requests only on a tick it is otherwise ready to fire. It holds the turn until that attack's cooldown ends, and a weapon until its execution clears and then its cooldown.
  - **One turn, one attack.** A second unmarked attack never rides a held turn. It requests behind the existing waiters.
  - **Every turn** holds through an execution already started. A target change cancels that execution and releases the turn, and so does the holder leaving the AI set.
  - A turn held under more than one rule ends when the last of them ends.
  - Lowering a budget, or an enemy changing pool, revokes no turn.
- **The budget is a hard gate, plus two facts.** An attack starts only while its enemy holds a turn. A refused attack commits nothing.
  - `@brain.hasAttackTurn`.
  - `@brain.attackTurnFree`: "a request from me would be granted now". It reads 0 with no target.
- **Arbitration is host-only and deterministic.**
  - It runs as one batch step after compute and before apply fires.
  - The longest wait wins, where a wait starts at the first request after last holding a turn. Slot order breaks ties.
  - Arbitration runs per pool and target, and one pool never blocks another.
  - **Head of line.** Within a pool, a longest waiter that does not fit blocks that pool's later grants on that target until it fits, or until the pool holds no counted turn there. Nobody starves.
  - Turns are derived host state and never replicate.
- **Shipped content adopts the vocabulary in this brief.**
  - `content/dev` and `sdk/behaviors/reference`, the patterns every SDK bundle copies.
  - The limitator and rifle squads gain strafe-while-waiting and fire-then-reposition, a three-shot `enemy_rifle_burst`, and `walk_left`, `walk_right` and `walk_backward` states bound from the Mixamo library.
  - Tests pinned to pile-on behavior or to bursts following a new target are updated.
- **Non-goals:**
  - **Back-off and kiting.** `reposition` covers fire-then-move. A third verb waits for a consumer.
  - **Softening attacks from outside the target's view.** An additive follow-up on the budget.
  - **Several active encounters, or an encounter suspending the default pool.** One active encounter, stacking beside the default pool, is the owner's call.
  - **Script-only encounters.** An encounter is map data; a script starts and stops it.
  - **Protective cover and squad tactics.** Their exclusion stands from the line-of-sight and positioning briefs.
  - **Per-activity speed.** The statecharts brief keeps it additive, so a strafe and a run share `moveSpeed` until it lands.

### Scripting surface
```ts
// Proposed design
// start-script.ts — mod-wide budget per target (engine seeds 3 when omitted)
export default defineMod({ /* … */ attackBudget: 2 });

// rifle trooper behavior (excerpt)
behavior: {
  attackCost: 1,
  attacks: { shoot: { weapon: "enemy_rifle_burst", standoffDistance: 6 } },
  activities: {
    engage: {
      animation: "walk",
      layers: {
        offense: {
          initial: "wait",
          activities: {
            wait:  { animation: "walk", motion: "strafe" },
            burst: { animation: "idle_aiming", motion: "hold", action: { attack: "shoot" }, attackTurn: true },
            move:  { animation: "run", motion: "reposition", facing: "travel" },
          },
          transitions: {
            wait:  [{ to: "burst", when: brain.attackTurnFree.and(brain.targetVisible) }],
            burst: [{ to: "move", when: brain.attacksFiredInActivity.ge(1).and(brain.timeInActivityMs.ge(900)) },
                    { to: "wait", when: brain.hasAttackTurn.not().and(brain.timeInActivityMs.ge(100)) }],
            move:  [{ to: "wait", when: brain.timeInActivityMs.ge(1500) }],
          },
        },
      },
    },
  },
}

// level script (setupLevel) — optional; a trigger with `target_tag arena`
// and `command start` does the same with no script
defineReaction("arena.start", {
  sequence: getMapEntities("encounter", { tag: "arena" }).flatMap((e) => e.start()),
});
```
```
// TrenchBroom: the arena, one attacker at a time on each player
{
"classname" "ai_encounter"
"_tags" "arena"
"encounter_tag" "arena_guard"
"attack_budget" "1"
// brushes: the arena floor; enemies inside at start join too
}
```
The Luau twin mirrors it with Luau call syntax (`brain.targetVisible:and(...)`).

## Acceptance
### Automated
**Motion and facing**
- [ ] A `strafe` enemy moves around its target at standoff while facing it, and fires its action mid-move when facing, range and line of sight pass.
- [ ] `strafe` reverses at a blocked, occupied or blind next slot. With neither neighbor usable it holds its slot, still facing the target, and does not flip direction on consecutive ticks. A strafer that loses its next slot to another on one tick reverses once (pin P9).
- [ ] `reposition` arrives at least one spacing from both its position and its held slot at entry, including after a `hold` predecessor and with no slot held. Re-entering picks again. With no valid slot it holds rather than oscillating.
- [ ] `facing: "travel"` on `reposition` faces travel, and `facing: "target"` on `chaseTarget` faces the target while moving. With no knockback active, an activity without `facing` behaves as today on every existing verb.
- [ ] Sideways travel under target facing selects `walk_left` or `walk_right`, and backward travel selects `walk_backward`. Under travel facing the same motion plays `walk`. A mesh without the variants plays `walk`.

**Knockback**
- [ ] A launched enemy under a non-target-facing activity keeps its yaw from the impact tick until the knockback ends, with steering still running.
- [ ] Under a target-facing activity it slews toward the target by no more than the facing rate per tick.
- [ ] An airborne launch with unchanged activity and steering switches to `jump_loop` after the fall delay and back to the activity's resolved clip on landing. A horizontal shove keeps the ground clip (pin P10).

**Budget**
- [ ] With no `attackBudget` authored, budget 3 applies: a fourth cost-1 requester waits.
- [ ] Under budget 2 and cost 1, a third requester never starts until a holder releases. Grants follow pin P1.
- [ ] With three requesters on one tick and one turn free, exactly one is granted. A replay grants the same one.
- [ ] Under budget 2: cost 2 is granted alone and refused beside a cost-1 holder; cost 3 is granted when no counted holder remains. Cost 0 never counts or blocks (pin P8).
- [ ] Under budget 2, a waiting cost-3 enemy among cost-1 enemies that keep a turn held is granted within a bounded wait. No cost-1 enemy is granted past it in the meantime (pin P12).
- [ ] A refused attack commits nothing and fires on its first granted tick in the same activity (pin P2).
- [ ] Under budget 1, three contact attackers ready together: at most one holds a turn on any tick. Each holds until its own cooldown ends, even after leaving its attacking activity. Each attacks within a bounded window.
- [ ] Under budget 1, an enemy alternating two unmarked attacks never starts the second under the first's turn while another enemy waits.
- [ ] Under budget 1, an unmarked three-shot weapon attack holds its turn through the execution and its cooldown. A ready neighbor never starts between shots (pin P4).
- [ ] A granted unmarked attack that does not start that tick releases its turn (pin P3).
- [ ] An unmarked attack requests no turn while out of range or line of sight, so a ready neighbor under budget 1 is granted at once.
- [ ] A turn holds across marked siblings with no release-and-regrant gap. It holds after leaving a marked activity mid-burst until the execution clears.
- [ ] A target switch mid-burst cancels the burst and releases the turn. No shot lands on the new target from that burst (pin P6).
- [ ] No turn leaks across any release case in pin P5, and a returning enemy re-requests.
- [ ] `hasAttackTurn` and `attackTurnFree` follow pin P7. `attackTurnFree` reads 0 with no target.
- [ ] A serialize-and-restore follows pin P11.
- [ ] Two manifests differing only in `attackBudget` produce the same compatibility digest.

**Encounters**
- [ ] Starting an `ai_encounter` by trigger `start` and by script `start` each make its members draw from a pool with its `attack_budget`. An empty value takes the mod value; an invalid one warns and takes it.
- [ ] Separate pools: an encounter at budget 1 and the default pool at budget 2 on one player give at most three counted holders. A head-of-line waiter in one pool never blocks a grant in the other.
- [ ] Starting a second encounter ends the first. Its members return to the default pool on that tick, and so does every member on `stop`. A repeated `start` does not recapture the volume, and `stop` on an inactive encounter changes nothing.
- [ ] Membership: a tag holder present at start, a later spawn whose `spawned_tags` carry the tag, and an untagged enemy inside the brushes at start are members. An untagged enemy that enters the volume after start is not, and a volume member that leaves stays one.
- [ ] An enemy leaving a pool while holding a turn keeps it. The turn counts against its new pool until it ends by its own rule, and no grant in that pool fits around it (pin P13).
- [ ] An empty `encounter_tag` is a compile error naming the entity. A manifest budget below 1, a negative `attackCost` or a non-finite value rejects with a pathed error in both runtimes.
- [ ] A level with no `ai_encounter` behaves as the default pool alone.
- [ ] Start and stop requests follow pin P14: a trigger-bound start applies before that tick's arbitration, and two starts on one tick leave the later-issued one active.
- [ ] A volume member that left the brushes is still a member after a serialize-and-restore, and an enemy that entered after start is not (pin P15).
- [ ] An encounter active when its level restarts or changes is not active in the next install, and its members draw from the default pool from that install's first tick.
- [ ] A volume member that despawns stops being a member; an untagged enemy later spawned into its registry slot is not one.
- [ ] While an encounter with an empty `attack_budget` is active, a mod reload that changes `attackBudget` changes that encounter's pool budget from the next tick.
- [ ] Two levels differing only in an `ai_encounter`'s `attack_budget` or brushes produce the same level content digest.

**Surface**
- [ ] The Scripting surface example parses identically in TypeScript and Luau, and runs as `content/dev` content. After `arena.start` fires, a tagged guard draws from a budget-1 pool.

### Manual
- [ ] On `movement-feel`, the shipped rifle squad under its authored budget: shooters relocate after a burst while waiters strafe and take over. A closet reveal on `closet-reveal` releases its payload without a pile-on. A rocket-launched knight keeps its facing in flight.
- [ ] Co-op loopback: a client sees the same strafing, directional clips and launch facing as the host.

## Path
- **Seams to read first.**
  - `facing_direction` and step 4 of `evaluate` (`crates/ai/src/compute.rs`).
  - `apply_outcomes` (`crates/ai/src/apply.rs`): cooldown and the per-activity count commit with the effect.
  - `resolve_combat_slots` / `select_combat_positions_batch`: the batch step precedent for arbitration.
  - `tick_impl`, `path_state` and `AgentComponent::knockback_velocity` (`crates/sim/src/agent_steering.rs`).
  - The trap member (`getMapEntities("trap")`, carried by the id-29 map-entity record) as the precedent for a map entity with `start` and `stop` from triggers and scripts. For brushes, the precedent is the id-55 Teleporters convex-leaf record plus its id-29 companion; id 44 holds AABBs only.
  - `trigger_direct_commands.rs` routes a trigger's `command` by member kind. Script `start` and `stop` steps run on the trigger's tick only through `CONSEQUENTIAL_PRIMITIVES` (`trigger_bindings/partition.rs`) and `command_binding.rs`. The build-time member table (`MapMemberKind`, `script_light_membership.rs`) needs the new kind too.
  - `trigger_teleport`'s brush peel and union-of-convex-brushes test as the precedent for the start-time volume test.
  - The spawner's `spawned_tags` path (`crates/sim/src/spawner.rs`), which carries tag membership to later spawns.
- **Shape.** Turns live on `BrainComponent`, as the held target, the counted pool and the wait clock (all transient). The encounter's state lives on its own serializable component; an empty budget stays unresolved, so a later change to the mod value still applies. Per-pool, per-target totals are recounted each tick. Rival: a per-target map on `AiRuntime`, rejected as a second source of truth to clear on despawn.
- **Encounter-model rivals, rejected.** Per-enemy overrides in one shared pool (the earlier draft): a budget-1 group at the head of the line held back every other enemy on its target. A script-only `startEncounter` command: map-only authors could not build an encounter. Trigger command words carrying the budget in `command_arg`: the encounter's config would live on whichever trigger starts it.
- **First slice.** The knockback facing split. It touches the fewest seams and proves the velocity separation strafe needs.
- **Watch for.**
  - Combat-slot hysteresis never selects anything in this caller (`research.md`), so `reposition` must pass its exclusions explicitly.
  - `drafts/perf-ai-tick-cost` touches the same batch. Add a wave-scale stress row for arbitration against its baseline.
- **Files past ~800 lines:** check `compute.rs` and `apply.rs` before extending them.

## Open questions
- The `strafe` direction on first entry — **delegated**: deterministic, from the better-scored neighbor. Report the rule.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| strafe verb | `MotionVerb::Strafe` | `"strafe"` | `"strafe"` | `"strafe"` | n/a |
| reposition verb | `MotionVerb::Reposition` | `"reposition"` | `"reposition"` | `"reposition"` | n/a |
| activity facing | `BehaviorActivityDescriptor::facing: Option<FacingPolicy>` | `"facing"`: `"target"` \| `"travel"` | `facing?` | `facing` | n/a |
| turn marking | `BehaviorActivityDescriptor::attack_turn: bool` | `"attackTurn"` | `attackTurn?` | `attackTurn` | n/a |
| archetype cost | `BehaviorGraphDescriptor::attack_cost` | `"attackCost"` | `attackCost?` | `attackCost` | n/a |
| mod budget | `ModManifestResult` field | `"attackBudget"` | `attackBudget?` | `attackBudget` | n/a |
| encounter entity | encounter component | PRL record (section delegated) | — | — | `ai_encounter` (`@SolidClass`): `_tags`, `encounter_tag`, `attack_budget(integer)`; empty budget inherits |
| encounter member | encounter member handle | — | `getMapEntities("encounter")` → `start`, `stop` | same | trigger `command` `start` / `stop` |
| turn held | `BRAIN_INPUTS` tail | `"@brain.hasAttackTurn"` | `brain.hasAttackTurn` | `brain.hasAttackTurn` | n/a |
| turn free | `BRAIN_INPUTS` tail, after the above | `"@brain.attackTurnFree"` | `brain.attackTurnFree` | `brain.attackTurnFree` | n/a |
| directional variants | host resolution | state names `X_left`, `X_right`, `X_backward` | — | — | n/a |
