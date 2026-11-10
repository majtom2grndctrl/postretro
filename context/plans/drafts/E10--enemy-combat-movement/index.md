# E10--enemy-combat-movement

Brief · resumable · Epic 10 · reads: `context/lib/entity_model.md` §7c · `context/lib/scripting.md` §1, §11, §12 · `context/lib/networking.md` §Combat authority · `context/lib/build_pipeline.md` §Custom FGD, §PRL section IDs · read at 5932287f

## Problem
The owner wants combat that is fun to fight and that level designers can build encounters around. This is a requested capability plus one observed defect.

Today an engaged enemy can only chase, hold, or walk to a fixed point. It cannot strafe while shooting, fire and then relocate, or wait its turn. Every enemy in range attacks at once, so a monster closet's payload reads as noise rather than a pattern a player can learn.

The defect: a rocket-launched enemy turns to face its flight path, because knockback velocity feeds the facing and locomotion choice.

When this is done:
- An author gives enemies strafing, fire-then-relocate and flanking behavior that reads correctly on screen.
- A per-target attack budget keeps idle enemies moving instead of piling on. Attacks draw mod-declared tokens such as melee, ranged and heavy, so a designer chooses how many attack at once, what competes with what, and how often a heavy attack lands. A map-authored attack pool gives an arena its own allowance.
- A launched enemy keeps the yaw it had at impact.

## Decisions
- **Two new engaged motion verbs, `strafe` and `reposition`.** Both resolve goals on the combat-slot ring (`plans/done/E10--enemy-combat-positioning`) at the standoff `chaseTarget` uses. Both hold the target and may carry an action, unlike position goals (`entity_model.md` §7c). The verb set stays closed; `strafe` takes no parameters.
  - **`strafe`** steps 45° around the target from its current bearing, on a ring rotated to that bearing, and keeps going in one direction, reversing when the next slot is unreachable, occupied or blind.
  - **`reposition`** claims the best slot at least one spacing from the slot it held at entry. Its bounds are fixed at entry; within them the pick re-scores each tick around the target's current position, so it follows a moving target and holds on arrival. A pick the batch gives away retries the next tick. With no slot that qualifies, it holds.
  - **One slot spacing** is the distance between neighboring ring slots at standoff. Slots straight inward or outward on the entry bearing never qualify, so `reposition` is never a back-off.
  - **`reposition` takes two optional properties**, both fixed at entry so the goal never chases itself:
    - `arc: [min, max]`, in degrees: how far around the target the slot sits, measured from the bearing from the target to the enemy at entry. 0° is straight back toward the enemy's start, 90° is lateral, 180° is the far side. Either side qualifies. Default `[0, 180]`. The ring steps in 45°, so an arc containing no multiple of 45° is rejected.
    - With no entry bearing, as for an enemy directly above its target, the arc is measured from the bearing to its held slot, or ignored when it holds none.
    - `minTravel`, in metres: how far the slot must be from the enemy's position at entry. Default one slot spacing.
  - **Flanking is `reposition` with a lateral arc.** It reads the enemy's own approach, never the target's facing: aim changes too often to steer by, and a remote co-op player's yaw on the host lags their view.
- **Per-activity `facing`: `"target"` or `"travel"`.** Omitted, the verb's default applies. Existing verbs keep today's rule, and `strafe` and `reposition` face the target. A target-facing mover passes the shipped facing gate unchanged.
- **Knockback never steers facing or picks a stride.** "Launched" means knockback velocity remains.
  - While launched, facing ignores both steering and knockback and holds the impact yaw, except that a target-facing activity re-aims at the normal slew rate.
  - An airborne launch plays `jump_loop` when the mesh declares one, after the player's fall delay (`MeshDescriptor.animations` typedoc). Otherwise it keeps its current state.
- **Directional travel variants.** Under target facing, an activity animation `X` resolves to `X_left`, `X_right` or `X_backward` by travel direction relative to facing, and plays `X` when a variant is missing. The host resolves the name, and it replicates as an ordinary animation state. This extends the player's `walk_backward` naming.
- **The attack budget is an engine mechanism with authored values** (`scripting.md` §1: pacing is taste; the SDK seeds defaults).
  - **Tokens.** An attack starts only with the tokens it draws, as Doom grants tokens per attack type. A mod declares each token with its default budget, `defineAttackToken({ limit, refreshMs })`: how many may be drawn on one target at once, and how long a spent token stays out after its turn ends. `limit` defaults to 3 and `refreshMs` to 0.
  - **Tokens are declared handles**, like damage types (`scripting.md` §2): named by their bindings, registered with `defineMod({ attackTokens })`, and crossing as typed references. A typo fails to compile in TypeScript, and a bare string, a handle of another kind or an unregistered token fails load in both runtimes.
  - **Draws.** Each attack lists its `draws`. A token alone draws one; `melee.with({ cost: 2 })` draws two. A slam drawing `melee.with({ cost: 2 })` and `heavy` takes two melee slots and the one heavy turn, while a jab drawing `melee` keeps queuing beside it. An attack without `draws` draws `defaultToken`, which the SDK declares with the default budget, so a novice's enemies take turns with no setup. `draws: []` opts out and is always granted: the lever for fodder that chips while heavies take turns.
  - **Profiles.** A profile overrides budgets in the same shape, `duel: [melee.with({ limit: 1 })]`, declared in `defineMod({ attackBudgets })`. A token a profile leaves out keeps its declared budget, and `default` is the declared budgets unless the mod overrides it. The SDK's defaults are tuning, not contract.
  - **Grants.** A request is granted when every token it draws has room for its cost, for that pool and target; there is no partial grant. A token with nothing counted against it always admits, so an oversized draw is never locked out.
  - **Refresh.** After a turn ends, its draws stay counted against each token that sets `refreshMs`, for that long. `heavy` at `{ limit: 1, refreshMs: 4000 }` lands one heavy attack, then stays shut 4 s while lighter attacks keep drawing `melee`. The remaining delay is measured each tick against the current value.
  - **Every target has its own allowance.** Counts are per target, so each co-op player, NPC ally and infighting monster is budgeted alike.
  - **Valid values.** `limit` and `cost` must be integers of at least 1, and `refreshMs` a finite number of at least 0. A field in the wrong place (`cost` in a profile, `limit` or `refreshMs` in a draw), one token drawn twice by an attack, or one token overridden twice by a profile rejects.
  - **Host-only, and outside the co-op compatibility digest.** Clients never read the budget.
  - A literal for now. A store-derived expression can be added later without breaking anything; defense layers are the precedent.
- **An attack pool is a logic entity, `ai_attack_pool`** (`@PointClass`): `_tags`, `member_tag`, and `budget_profile` (empty means `default`). Where it sits does not matter; triggers and scripts reach it by its `_tags`, as Quake-family maps reach a relay.
  - **Pools run side by side.** Each starts and stops on its own: trigger `command` `start` and `stop` on its `_tags`, or scripts through `getMapEntities("attackPool")` with `start` and `stop`, as trap members do. Co-op players in two arenas each fight under their arena's profile, and a chained arena wires a `stop` on the one before. `start` on an active pool and `stop` on an inactive one are no-ops. Both apply on the host only, and a level starts with none active.
  - **Membership is the tag, live.** An enemy is a member while it carries `member_tag`, so later spawns join through `spawned_tags`. An enemy in two active pools draws from the one started most recently; an enemy placed or spawned with two pools' member tags warns at compile.
  - **Ending.** On `stop`, members return to the default pool, or to another active pool they belong to.
  - **Validation.** An empty `member_tag`, or two pools sharing one `_tags`, is a compile error naming the entities. A `budget_profile` the mod does not declare warns at level load and uses `default`.
  - Active pools are serializable state: they survive a serialize-and-restore.
  - **Budget only.** "Encounter" stays free for the set-piece concept (roadmap E18-E) and its kill progress, which may later start an attack pool.
- **Turn lifetime.** A turn is for one attack and holds that attack's draws.
  - **No two turns share a token.** An enemy never holds two turns that draw the same token, so its claw and its rifle may overlap while two melee attacks take turns. Another attack never rides a held turn: an attack sharing a token with one its enemy holds requests once that turn ends.
  - **One request at a time.** An enemy requests for the attack it would start now. A request it drops stops blocking its tokens (pin P19).
  - **Marked activities.** An activity marked `attackTurn: "slam"` requests a turn for `slam` on entry and keeps requesting until granted. The turn holds while any activity marked for that attack is on the active path, so a windup → commit → recover chain holds one turn. Entering a sibling marked for another attack releases it and requests behind existing waiters.
  - **Marks never disagree.** Load rejects a mark nested inside a mark for another attack, and an action under a mark that fires another attack, each with a pathed error.
  - **Unmarked attacks.** With no marked activity on its path, an attack requests only on a tick it is otherwise ready to fire. It holds the turn until that attack's cooldown ends, and a weapon until its execution clears and then its cooldown.
  - **Every turn** holds through an execution already started. A target change cancels that execution and releases the turn, and so does the holder leaving the AI set.
  - A turn held under more than one rule ends when the last of them ends.
  - Lowering a budget, or an enemy changing pool, revokes no turn.
- **The budget is a hard gate, plus two facts.** An attack starts only while its enemy holds a turn for it. A refused attack commits nothing.
  - `brain.hasAttackTurn`: a turn is held.
  - `attackTurnFree(attack)`: true when a request for this attack would be granted now, false with no target. A free function beside `state(name)` and `defense(layer)`; its argument names one of the archetype's attacks, checked at load.
- **Arbitration is host-only and deterministic.**
  - It runs as one batch step after compute and before apply fires.
  - The longest wait wins, where a wait starts at the first request after last holding a turn. Slot order breaks ties.
  - Arbitration runs per pool and target, and one pool never blocks another.
  - **Head of line, per token.** Within a pool and target, for each token, the longest waiter whose draw does not fit blocks later grants that draw that token. It blocks until it fits, or until that token holds no counted turn. Nobody starves, and jabs keep drawing `melee` while a slam waits on `heavy`.
  - Turns are derived host state and never replicate.
- **Shipped content adopts the vocabulary in this brief.**
  - `content/dev` and `sdk/behaviors/reference`, the patterns every SDK bundle copies.
  - The limitator and rifle squads gain strafe-while-waiting and fire-then-reposition, a three-shot `enemy_rifle_burst`, and `walk_left`, `walk_right` and `walk_backward` states bound from the Mixamo library.
  - The dev mod declares `melee`, `ranged` and `heavy` tokens as in the Scripting surface. The reference enemy's slam draws two melee and the heavy, its jab one melee; the shadow hunter's claw draws melee and the troopers' rifles draw ranged.
- **Non-goals:**
  - **Back-off and kiting.** `reposition` covers fire-then-move. A third verb waits for a consumer.
  - **Softening attacks from outside the target's view.** An additive follow-up on the budget.
  - **A pool suspending the default pool.** Enemies outside every active pool keep drawing from the default; one arena never pauses the rest of the level.
  - **Volume membership.** Engines pair a trigger volume that fires on entry with tag-addressed targets; the tag already names who belongs.
  - **Per-activity speed.** The statecharts brief keeps it additive, so a strafe and a run share `moveSpeed` until it lands.
  - **Cover, squad tactics, and flank routes that avoid the target's sight.** They need exposure data the navmesh does not carry; the cover brief owns them.

### Scripting surface
```ts
// Proposed design
// budget.ts — tokens named by their bindings; each object is the token's default budget
export const melee  = defineAttackToken({ limit: 3 });
export const ranged = defineAttackToken({ limit: 2 });
export const heavy  = defineAttackToken({ limit: 1, refreshMs: 4000 });

// start-script.ts — a profile overrides only what an arena changes
export default defineMod({
  /* … */
  attackTokens: [melee, ranged, heavy],
  attackBudgets: {
    arena: [melee.with({ limit: 1 }), ranged.with({ limit: 1 })],
  },
});

// reference enemy behavior (excerpt): a slam takes two melee slots and the one
// heavy turn; jabs keep drawing melee while heavy refreshes
behavior: {
  attacks: {
    jab:  { draws: [melee], damage: 8, maxRange: 2, cooldownMs: 1200 },
    slam: { draws: [melee.with({ cost: 2 }), heavy], damage: 14, maxRange: 3.5, cooldownMs: 1800 },
  },
  /* … */
}

// rifle trooper behavior (excerpt)
behavior: {
  attacks: {
    shoot: { draws: [ranged], weapon: "enemy_rifle_burst", standoffDistance: 6 },
  },
  activities: {
    engage: {
      animation: "walk",
      layers: {
        offense: {
          initial: "wait",
          activities: {
            wait:  { animation: "walk", motion: "strafe" },
            burst: { animation: "idle_aiming", motion: "hold", action: { attack: "shoot" }, attackTurn: "shoot" },
            move:  { animation: "run", motion: "reposition", facing: "travel",
                     reposition: { arc: [60, 120] } },   // flank: lateral to its approach
          },
          transitions: {
            wait:  [{ to: "burst", when: attackTurnFree("shoot").and(brain.targetVisible) }],
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
  sequence: getMapEntities("attackPool", { tag: "arena" }).flatMap((p) => p.start()),
});
```
```
// TrenchBroom: arena guards get their own allowance, one melee and one ranged
// attacker at a time on each player; enemies outside it keep the default beside it
{
"classname" "ai_attack_pool"
"origin" "0 0 0"
"_tags" "arena"
"member_tag" "arena_guard"
"budget_profile" "arena"
}
```
The Luau twin mirrors it with Luau call syntax: `defineAttackToken("heavy", { limit = 1, refreshMs = 4000 })`, `melee:with({ cost = 2 })`, `brain.targetVisible:and(...)`.

## Acceptance
### Automated
**Motion and facing**
- [ ] A `strafe` enemy moves around its target at standoff while facing it, and fires its action mid-move when facing, range and line of sight pass.
- [ ] `strafe` reverses at a blocked, occupied or blind next slot. With neither neighbor usable it holds its slot, still facing the target, and does not flip direction on consecutive ticks. A strafer that loses its next slot to another on one tick reverses once (pin P9).
- [ ] `reposition` arrives at least one spacing from both its position and its held slot at entry, including after a `hold` predecessor and with no slot held. Re-entering picks again. With no valid slot it holds rather than oscillating.
- [ ] `arc: [45, 135]` includes the 45° and 135° slots on both sides; `arc: [90, 90]` picks only the two lateral slots; `arc: [180, 180]` only the far slot. With no slot in the arc it holds.
- [ ] The arc and the entry position stay fixed while the target moves during the move. Re-entering measures them again (pin P16).
- [ ] `minTravel: 8` refuses a slot 7 m from the entry position and accepts one at 9 m. A `minTravel` beyond every candidate holds without error and without flipping its goal.
- [ ] Two repositioners entering on one tick with the same bearing and arc claim distinct slots, and a replay assigns the same ones.
- [ ] A serialize-and-restore mid-`reposition` or mid-`strafe` follows pin P17.
- [ ] In both runtimes, an `arc` outside 0–180, with min above max or non-finite, a negative or non-finite `minTravel`, an arc containing no multiple of 45°, or `reposition` properties on an activity whose motion is not `reposition`, rejects with a pathed error.
- [ ] `facing: "travel"` on `reposition` faces travel, and `facing: "target"` on `chaseTarget` faces the target while moving. With no knockback active, an activity without `facing` behaves as today on every existing verb.
- [ ] Sideways travel under target facing selects `walk_left` or `walk_right`, and backward travel selects `walk_backward`. Under travel facing the same motion plays `walk`. A mesh without the variants plays `walk`.
- [ ] Strafing after a `reposition` steps 45° around the target from its current bearing, with no snap to the shared ring.
- [ ] A `reposition` entered directly above its target measures its arc from its held slot, or ignores the arc with none. Slots straight inward or outward on the entry bearing never qualify.

**Knockback**
- [ ] A launched enemy under a non-target-facing activity keeps its yaw from the impact tick until the knockback ends, with steering still running.
- [ ] Under a target-facing activity it slews toward the target by no more than the facing rate per tick.
- [ ] An airborne launch with unchanged activity and steering switches to `jump_loop` after the fall delay and back to the activity's resolved clip on landing. A horizontal shove keeps the ground clip (pin P10).

**Budget**

Rows draw one token unless they say otherwise; "budget N" is its `limit`.
- [ ] With no `attackBudgets` and no `draws`, every attack draws `defaultToken` at limit 3: a fourth requester on one target waits.
- [ ] Under budget 2 and cost 1, a third requester never starts until a holder releases. Grants follow pin P1.
- [ ] With three requesters on one tick and one turn free, exactly one is granted. A replay grants the same one.
- [ ] Under budget 2: cost 2 is granted alone and refused beside a cost-1 holder; cost 3 is granted when nothing is counted. An attack with `draws: []` never counts or blocks (pin P8).
- [ ] Under budget 2, a waiting cost-3 enemy among cost-1 enemies that keep a turn held is granted within a bounded wait. No cost-1 enemy is granted past it in the meantime (pin P12).
- [ ] Under `melee` at 3 and `heavy` at 1: a second slam (`melee` ×2 and `heavy`) waits while one holds, and jabs (`melee`) are granted past it while `melee` has room. A slam at the head of the line blocks only `heavy` draws and the `melee` it cannot fit (pin P14).
- [ ] Slams from two archetypes drawing `heavy` at limit 1 never hold two turns on one target at once.
- [ ] Per-token head of line: with a slam waiting on `heavy` at the head and a cost-3 `melee` draw waiting behind it, a later cost-1 `melee` request is not granted past the cost-3 draw (pin P18).
- [ ] A grant needs room in every token drawn: a slam with room in `melee` but not in `heavy` is refused, and takes neither.
- [ ] Tokens never block one another's attacks: under `melee` and `ranged` at 1, one enemy holds a melee and a ranged turn at once. Its second melee attack waits for its melee turn to end, and its wait starts then.
- [ ] Refresh: under `heavy` at `{ limit: 1, refreshMs: 4000 }`, a second slam is granted no sooner than 4 s after the first's turn ends, while jabs keep being granted on `melee` (pin P26).
- [ ] A profile override changes only the fields it names: `melee.with({ limit: 1 })` keeps `melee`'s declared `refreshMs`, and tokens the profile leaves out keep their declared budgets.
- [ ] Every target has its own allowance: three enemies attacking a co-op player and three attacking an NPC ally are budgeted separately, and so are monsters infighting.
- [ ] One enemy with a grenade drawing `lobber` and a claw drawing `melee` follows pin P19: while `lobber` is full, `attackTurnFree("grenade")` is false and `attackTurnFree("claw")` true.
- [ ] A waiter that switches from a draw blocking `melee` to one that fits stops blocking `melee`, and both it and a later cost-1 request are granted that tick (pin P24).
- [ ] Load rejects a mark nested inside a mark for another attack, and an action under a mark that fires another attack, each with a pathed error.
- [ ] Entering a sibling marked for `jab` releases the slam turn and requests a jab turn behind existing waiters: an older waiter is granted first (pin P21).
- [ ] An enemy entering a sibling marked for `jab` mid-execution of its `slam` holds the slam turn until the execution clears, and its jab waits for that release (pin P20).
- [ ] A request names the attack the fire step resolves (pin P25).
- [ ] A waiter's clock follows pin P22: a pause keeps its place, and a target switch queues it behind the new target's older waiters.
- [ ] A mod reload lowering `heavy` from 2 to 1 while two slams hold revokes neither turn, and no slam is granted until both release. A reload that removes an active pool's profile makes it use `default` from the next tick. A reload that lowers `refreshMs` frees a refreshing slot once the new delay has passed.
- [ ] A hot reload that changes an attack's `draws` releases its holders' turns, and they re-request under the new draws. A reload that leaves them unchanged leaves held turns standing.
- [ ] A refused attack commits nothing and fires on its first granted tick in the same activity (pin P2).
- [ ] Under budget 1, three contact attackers ready together: at most one holds a turn on any tick. Each holds until its own cooldown ends, even after leaving its attacking activity. Each attacks within a bounded window.
- [ ] Under budget 1, an enemy alternating two unmarked attacks drawing one token never starts the second under the first's turn; the second requests once that turn ends.
- [ ] Under budget 1, an unmarked three-shot weapon attack holds its turn through the execution and its cooldown. A ready neighbor never starts between shots (pin P4).
- [ ] A granted unmarked attack that does not start that tick releases its turn (pin P3).
- [ ] An unmarked attack requests no turn while out of range or line of sight, so a ready neighbor under budget 1 is granted at once.
- [ ] A turn holds across marked siblings for one attack with no release-and-regrant gap. It holds after leaving a marked activity mid-burst until the execution clears.
- [ ] A target switch mid-burst cancels the burst and releases the turn. No shot lands on the new target from that burst (pin P6).
- [ ] No turn leaks across any release case in pin P5, and a returning enemy re-requests.
- [ ] `hasAttackTurn` and `attackTurnFree(attack)` follow pin P7. `hasAttackTurn` is true while any turn is held, whatever the path's current action. `attackTurnFree` is false with no target, leaves out the reader's own pending request, and is false for an attack sharing a token with a turn the reader holds (pin P23).
- [ ] A serialize-and-restore follows pin P11.
- [ ] Two manifests differing only in `attackTokens`, `attackBudgets` or `draws` produce the same compatibility digest.

**Attack pools**
- [ ] Starting an `ai_attack_pool` by trigger `start` and by script `start` each make its members draw from a pool with its profile's limits. An empty `budget_profile` uses `default`; an undeclared one warns at level load and uses `default`.
- [ ] Separate pools: an attack pool at limit 1 and the default pool at limit 2 on one player give at most three counted holders. A head-of-line waiter in one pool never blocks a grant in the other.
- [ ] `start` on an active pool changes nothing: held turns, wait clocks and members stand. A pool `start` on a connected client changes nothing.
- [ ] Two pools started on one level both apply: each member draws from its own pool, and stopping one returns its members to the default pool on that tick without touching the other. `stop` on an inactive pool changes nothing.
- [ ] An enemy carrying both active pools' member tags draws from the one started most recently. A placed or spawner-tagged enemy carrying two pools' member tags warns at compile.
- [ ] Membership: a tag holder present at start and a later spawn whose `spawned_tags` carry the tag are members.
- [ ] An enemy leaving a pool while holding a turn keeps it. The turn counts against its new pool until it ends by its own rule, and no grant in that pool fits around it (pin P13).
- [ ] An empty `member_tag` is a compile error naming the entity. In both runtimes, a `limit` or `cost` below 1 or non-integer, a negative or non-finite `refreshMs`, a field in the wrong place, a token drawn twice by one attack or overridden twice by one profile, a token reference that is a bare string, a handle of another kind or unregistered, a duplicate token name, or an `attackTurn` or `attackTurnFree` naming no attack of its archetype rejects with a pathed error. A TypeScript token reference that names no declaration fails to compile.
- [ ] A level with no `ai_attack_pool` behaves as the default pool alone.
- [ ] Start and stop requests follow pin P15: a trigger-bound start applies before that tick's arbitration, and a start and a stop of one pool on one tick apply in issue order.
- [ ] Attack pools active when their level restarts or changes are not active in the next install, and their members draw from the default pool from that install's first tick.
- [ ] While an attack pool uses `default`, a mod reload that changes `default` changes that pool's limits from the next tick.
- [ ] Two levels differing only in an `ai_attack_pool`'s `budget_profile` produce the same level content digest.

**Surface**
- [ ] The Scripting surface example parses identically in TypeScript and Luau, and runs as `content/dev` content. After `arena.start` fires, a tagged guard draws from an attack pool under the `arena` profile. Two reference enemies on one player never slam within 4 s of each other, and while one slams, the other's jab is granted. Three rifle troopers on one player never hold more than two ranged turns at once.

### Manual
- [ ] On `movement-feel`, the shipped rifle squad under its authored budget: shooters relocate after a burst while waiters strafe and take over. A closet reveal on `closet-reveal` releases its payload without a pile-on. A rocket-launched reference enemy keeps its facing in flight.
- [ ] Co-op loopback: a client sees the same strafing, directional clips and launch facing as the host.

## Path
- **Seams to read first.**
  - `facing_direction` and step 4 of `evaluate` (`crates/ai/src/compute.rs`).
  - `apply_outcomes` (`crates/ai/src/apply.rs`): cooldown and the per-activity count commit with the effect.
  - `resolve_combat_slots` / `select_combat_positions_batch`: the batch step precedent for arbitration.
  - `tick_impl`, `path_state` and `AgentComponent::knockback_velocity` (`crates/sim/src/agent_steering.rs`).
  - The trap member (`getMapEntities("trap")`, a point entity carried by the id-29 map-entity record) as the precedent for a map entity with `start` and `stop` from triggers and scripts.
  - `trigger_direct_commands.rs` routes a trigger's `command` by member kind. Script `start` and `stop` steps run on the trigger's tick only through `CONSEQUENTIAL_PRIMITIVES` (`trigger_bindings/partition.rs`) and `command_binding.rs`. The build-time member table (`MapMemberKind`, `script_light_membership.rs`) needs the new kind too.
  - The spawner's `spawned_tags` path (`crates/sim/src/spawner.rs`), which carries tag membership to later spawns.
- **Shape.** Turns live on `BrainComponent`: each held turn's target, attack and counted pool, and the wait clock (all transient). Refreshing counts live with the pool's per-target, per-token tally. Each attack pool's state lives on its own serializable component; its profile resolves by name each tick, so a mod reload applies. Per-pool, per-target, per-token counts are recounted each tick. Rival: a per-target map on `AiRuntime`, rejected as a second source of truth to clear on despawn.
- **Attack-pool rivals** are argued in `research.md` (Attack pool precedent).
- **Slice order.** The knockback facing split first: it touches the fewest seams and proves the velocity separation strafe needs. Then the verbs and the scorer's per-query constraints, turns and arbitration, tokens, profiles and refresh, the attack pool entity, and last the shipped content that proves them together.
- **Named turn fact.** `attackTurnFree(attack)` follows the `state(name)` / `defense(layer)` path (`sdk/lib/brain.luau`; resolution in `crates/foundation/src/brain.rs`), but projects `IrType::Bool` where those project `Number`, and checks its name against the archetype's attacks rather than the mod vocabulary (`crates/scripting-core/src/runtime/types.rs` walks `behavior.attacks` beside the guard walk). `hasAttackTurn` stays a fixed brain input; the typedef drift test pins `brain` to `BRAIN_INPUTS`.
- **Hot reload.** A reload reseats no live brain, so an attack's `draws` resolve from the live descriptor at each grant and refresh on descriptor generation, as `resolved_projectile_attacks` does (`crates/ai/src/brain_programs.rs`).
- **Watch for.**
  - Combat-slot hysteresis never selects anything in this caller (`research.md`), so `reposition` must pass its exclusions explicitly. The batch scans no challengers while an incumbent is retained (`scan_challengers` in `combat_slots.rs`), so on entry `reposition` forces the scan and drops its incumbent.
  - Test the arc on the ring index's generated angle, not a measured angle, so a bound on a ring direction includes it.
  - `score_candidate` and the batch take per-query constraints (arc, minimum travel, exclusions) rather than fixed terms. `strafe` and `reposition` are the first consumers; the cover brief adds a mode. The shared ring is eight world-axis directions, so an arc narrower than 45° would hold or not depending on approach angle. `reposition` generates its candidates on a ring rotated to the entry bearing instead, which leaves the shared ring and the searches the perf brief measures unchanged.
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
| reposition properties | `BehaviorActivityDescriptor::reposition: Option<RepositionParams { arc_deg: [f32; 2], min_travel_m: f32 }>` | `"reposition"`: `{ "arc"?, "minTravel"? }` | `reposition?: { arc?: [number, number]; minTravel?: number }` | `reposition` | n/a |
| turn marking | `BehaviorActivityDescriptor::attack_turn: Option<String>` | `"attackTurn"`: the attack's name | `attackTurn?: string` | `attackTurn` | n/a |
| token handle | `AuthoredHandle` of kind `attackToken`, with its declared `{ limit, refreshMs }` | kinded handle, as damage types cross; registration carries each token's budget | `defineAttackToken({ limit?, refreshMs? })` → `AttackToken` | `defineAttackToken(name, { limit?, refreshMs? })` | n/a |
| default token | SDK-declared token at the default budget | `{ "kind": "attackToken", "name": "default" }` | `defaultToken` export | `defaultToken` | n/a |
| token reference | handle, or handle with fields | `{ "token", "limit"?, "refreshMs"? }` in a profile; `{ "token", "cost"? }` in a draw | `token.with({ ... })` → `AttackTokenRef` | `token:with({ ... })` | n/a |
| attack draws | `AttackParams::draws: Option<Vec<TokenDraw>>`; `None` draws `defaultToken` | `"draws"`: token references | `draws?: (AttackToken \| AttackTokenRef)[]` | `draws` | n/a |
| registration | `ModManifestResult` field | `"attackTokens"`: handles with budgets | `attackTokens?` | `attackTokens` | n/a |
| budget profiles | `ModManifestResult` field, name → overrides | `"attackBudgets"`: `{ [profile]: [token reference] }` | `attackBudgets?` | `attackBudgets` | n/a |
| attack pool entity | attack pool component | id-29 map-entity record | — | — | `ai_attack_pool` (`@PointClass`): `_tags`, `member_tag`, `budget_profile(string)`; empty means `default` |
| attack pool member | attack pool member handle | — | `getMapEntities("attackPool")` → `start`, `stop` | same | trigger `command` `start` / `stop` |
| turn held | `BRAIN_INPUTS` tail | `"@brain.hasAttackTurn"` | `brain.hasAttackTurn` | `brain.hasAttackTurn` | n/a |
| turn free | named Bool input, resolved per attack at load | `"@attackTurnFree.<attack>"` | `attackTurnFree(attack: string)` | `attackTurnFree(attack)` | n/a |
| directional variants | host resolution | state names `X_left`, `X_right`, `X_backward` | — | — | n/a |
