# E10--enemy-combat-movement

Brief · resumable · Epic 10 · reads: `context/lib/entity_model.md` §7c · `context/lib/scripting.md` §1, §11, §12 · `context/lib/networking.md` §Combat authority · `context/lib/build_pipeline.md` §Custom FGD, §PRL section IDs · read at 5932287f

## Problem
The owner wants combat that is fun to fight and that level designers can build encounters around. This is a requested capability plus one observed defect.

Today an engaged enemy can only chase, hold, or walk to a fixed point. It cannot strafe while shooting, fire and then relocate, or wait its turn. Every enemy in range attacks at once, so a monster closet's payload reads as noise rather than a pattern a player can learn.

The defect: a rocket-launched enemy turns to face its flight path, because knockback velocity feeds the facing and locomotion choice.

When this is done:
- An author gives enemies strafing, fire-then-relocate and flanking behavior that reads correctly on screen.
- A per-target attack budget, seeded by the engine, shaped per mod into named profiles with per-class limits, and given its own pool by a map-authored attack pool, keeps idle enemies moving instead of piling on.
- A launched enemy keeps the yaw it had at impact.

## Decisions
- **Two new engaged motion verbs, `strafe` and `reposition`.** Both resolve goals on the combat-slot ring (`plans/done/E10--enemy-combat-positioning`) at the standoff `chaseTarget` uses. Both hold the target and may carry an action, unlike position goals (`entity_model.md` §7c). The verb set stays closed; `strafe` takes no parameters.
  - **`strafe`** steps to the neighboring ring slot and keeps going in one direction, reversing when the next slot is unreachable, occupied or blind.
  - **`reposition`** claims the best slot at least one spacing from its held slot. It picks again on each entry and holds on arrival. With no slot that qualifies, it holds.
  - **`reposition` takes two optional properties**, both fixed at entry so the goal never chases itself:
    - `arc: [min, max]`, in degrees: how far around the target the slot sits, measured from the bearing from the target to the enemy at entry. 0° is straight back toward the enemy's start, 90° is lateral, 180° is the far side. Either side qualifies. Default `[0, 180]`.
    - `minTravel`, in metres: how far the slot must be from the enemy's position at entry. Default one slot spacing.
  - **Flanking is `reposition` with a lateral arc.** It reads the enemy's own approach, never the target's facing: aim changes too often to steer by, and a remote co-op player's yaw on the host lags their view.
- **Per-activity `facing`: `"target"` or `"travel"`.** Omitted, the verb's default applies. Existing verbs keep today's rule, and `strafe` and `reposition` face the target. A target-facing mover passes the shipped facing gate unchanged.
- **Knockback never steers facing or picks a stride.** "Launched" means knockback velocity remains.
  - While launched, facing ignores both steering and knockback and holds the impact yaw, except that a target-facing activity re-aims at the normal slew rate.
  - An airborne launch plays `jump_loop` when the mesh declares one, after the player's fall delay (`MeshDescriptor.animations` typedoc). Otherwise it keeps its current state.
- **Directional travel variants.** Under target facing, an activity animation `X` resolves to `X_left`, `X_right` or `X_backward` by travel direction relative to facing, and plays `X` when a variant is missing. The host resolves the name, and it replicates as an ordinary animation state. This extends the player's `walk_backward` naming.
- **The attack budget is an engine mechanism with authored values** (`scripting.md` §1: pacing is taste; the engine seeds a default).
  - **Pools.** Every enemy draws from one pool. Members of the active attack pool draw from it, and everyone else from the default pool. Each pool keeps its own held cost per target, so the two pools stack on one player.
  - **Profiles.** A pool's limits come from a named budget profile: a `total` and optional per-class limits. The mod declares profiles in `defineMod({ attackBudgets })`. The default pool uses the profile `default`; the engine seeds `default` as `{ total: 3 }` when the mod omits it. The seeded value is tuning, not contract.
  - **Classes.** An archetype may declare an author-named `attackClass`, so one class can span several archetypes. An archetype with no class, or a class its pool's profile does not limit, is limited by `total` alone.
  - **Grants.** The `total` counts cost; a class limit counts attackers, so `heavy: 1` means one heavy at a time whatever its cost. An enemy is granted a turn when its cost fits its pool's `total` and its class has an attacker free on that target. A `total` with no counted turn held under it always admits, so an oversized archetype is never locked out.
  - **Valid values.** A `total` and every class limit must be an integer of at least 1. `attackCost` is a non-negative integer, default 1: a designer trades slots of the total, so a brute at cost 2 fills two. Cost 0 never counts against anything and is always granted: the lever for fodder that chips while heavies take turns.
  - **Host-only, and outside the co-op compatibility digest.** Clients never read the budget.
  - A literal for now. A store-derived expression can be added later without breaking anything; defense layers are the precedent.
- **An attack pool is a logic entity, `ai_attack_pool`** (`@PointClass`): `_tags`, `member_tag`, and `budget_profile` (empty means `default`). Where it sits does not matter; triggers and scripts reach it by its `_tags`, as Quake-family maps reach a relay.
  - **At most one is active per level.** Starting one ends the active one, so chained arenas need no stop wiring. Existing trigger `command` `start` and `stop` on its `_tags` start and end it, and scripts drive the same member as `getMapEntities("attackPool")` with `start` and `stop`, as trap members do. `start` on the active pool and `stop` on an inactive one are no-ops. Both apply on the host only, and a level starts with none active.
  - **Membership is the tag, live.** An enemy is a member while it carries `member_tag`, so later spawns join through `spawned_tags`.
  - **Ending.** On `stop`, or on replacement, members return to the default pool.
  - **Validation.** An empty `member_tag` is a compile error naming the entity. A `budget_profile` the mod does not declare warns at level load and uses `default`.
  - The active pool is serializable state: it survives a serialize-and-restore.
  - **Budget only.** "Encounter" stays free for the set-piece concept (roadmap E18-E) and its kill progress, which may later start an attack pool.
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
  - **Head of line.** Within a pool, a longest waiter that does not fit blocks later grants on that target that compete for the limit it failed: a class limit blocks only its class, and the `total` blocks the whole pool. It blocks until it fits, or until that limit holds no counted turn. Nobody starves, and grunts keep taking turns while a heavy waits on its class.
  - Turns are derived host state and never replicate.
- **Shipped content adopts the vocabulary in this brief.**
  - `content/dev` and `sdk/behaviors/reference`, the patterns every SDK bundle copies.
  - The limitator and rifle squads gain strafe-while-waiting and fire-then-reposition, a three-shot `enemy_rifle_burst`, and `walk_left`, `walk_right` and `walk_backward` states bound from the Mixamo library.
  - Tests pinned to pile-on behavior or to bursts following a new target are updated.
- **Non-goals:**
  - **Back-off and kiting.** `reposition` covers fire-then-move. A third verb waits for a consumer.
  - **Softening attacks from outside the target's view.** An additive follow-up on the budget.
  - **Several active attack pools, or a pool suspending the default pool.** Several pools at once would fight over one player's experience. Splitting one fight into atomic pools waits for a designer who needs it.
  - **Volume membership.** Engines pair a trigger volume that fires on entry with tag-addressed targets; the tag already names who belongs.
  - **Per-attack classes.** A class sits on the archetype. A per-attack override can be added later without breaking anything.
  - **Protective cover and squad tactics.** Their exclusion stands from the line-of-sight and positioning briefs.
  - **Per-activity speed.** The statecharts brief keeps it additive, so a strafe and a run share `moveSpeed` until it lands.
  - **Cover, and flank routes that avoid the target's sight.** Both need exposure data the navmesh does not carry; the cover brief owns them.

### Scripting surface
```ts
// Proposed design
// start-script.ts — budget profiles per target (engine seeds default { total: 3 })
export default defineMod({
  /* … */
  attackBudgets: {
    default: { total: 3, heavy: 1 },
    arena:   { total: 1 },
  },
});

// rifle trooper behavior (excerpt)
behavior: {
  attackCost: 1,
  attackClass: "rifle",
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
            move:  { animation: "run", motion: "reposition", facing: "travel",
                     reposition: { arc: [60, 120] } },   // flank: lateral to the target
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
  sequence: getMapEntities("attackPool", { tag: "arena" }).flatMap((p) => p.start()),
});
```
```
// TrenchBroom: arena guards get their own allowance, one attacker at a time on
// each player; enemies outside the pool keep the default pool beside it
{
"classname" "ai_attack_pool"
"origin" "0 0 0"
"_tags" "arena"
"member_tag" "arena_guard"
"budget_profile" "arena"
}
```
The Luau twin mirrors it with Luau call syntax (`brain.targetVisible:and(...)`).

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
- [ ] In both runtimes, an `arc` outside 0–180, with min above max or non-finite, a negative or non-finite `minTravel`, or `reposition` properties on an activity whose motion is not `reposition`, rejects with a pathed error.
- [ ] `facing: "travel"` on `reposition` faces travel, and `facing: "target"` on `chaseTarget` faces the target while moving. With no knockback active, an activity without `facing` behaves as today on every existing verb.
- [ ] Sideways travel under target facing selects `walk_left` or `walk_right`, and backward travel selects `walk_backward`. Under travel facing the same motion plays `walk`. A mesh without the variants plays `walk`.

**Knockback**
- [ ] A launched enemy under a non-target-facing activity keeps its yaw from the impact tick until the knockback ends, with steering still running.
- [ ] Under a target-facing activity it slews toward the target by no more than the facing rate per tick.
- [ ] An airborne launch with unchanged activity and steering switches to `jump_loop` after the fall delay and back to the activity's resolved clip on landing. A horizontal shove keeps the ground clip (pin P10).

**Budget**
- [ ] With no `attackBudgets` authored, `default` is `{ total: 3 }`: a fourth cost-1 requester waits.
- [ ] Under budget 2 and cost 1, a third requester never starts until a holder releases. Grants follow pin P1.
- [ ] With three requesters on one tick and one turn free, exactly one is granted. A replay grants the same one.
- [ ] Under budget 2: cost 2 is granted alone and refused beside a cost-1 holder; cost 3 is granted when no counted holder remains. Cost 0 never counts or blocks (pin P8).
- [ ] Under budget 2, a waiting cost-3 enemy among cost-1 enemies that keep a turn held is granted within a bounded wait. No cost-1 enemy is granted past it in the meantime (pin P12).
- [ ] Under `{ total: 3, heavy: 1 }` with cost 1 throughout: a second heavy waits while one heavy holds, and unclassed enemies are granted past it up to the total. A heavy at the head of the line blocks only heavies (pin P14).
- [ ] Two archetypes declaring `attackClass: "heavy"` under `heavy: 1` never hold two turns on one target at once.
- [ ] A mod reload lowering `heavy` from 2 to 1 while two heavies hold revokes neither turn, and no heavy is granted until both release. A reload that removes the active pool's profile makes it use `default` from the next tick.
- [ ] A hot reload that changes an archetype's `attackClass` or `attackCost` releases its holders' turns (pin P5, graph reseat); they re-request under the new class and cost.
- [ ] An archetype whose class the profile does not limit is limited by `total` alone. A heavy of cost 2 under `heavy: 1` is granted when no heavy holds.
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
- [ ] Two manifests differing only in `attackBudgets` or `attackClass` produce the same compatibility digest.

**Attack pools**
- [ ] Starting an `ai_attack_pool` by trigger `start` and by script `start` each make its members draw from a pool with its profile's limits. An empty `budget_profile` uses `default`; an undeclared one warns at level load and uses `default`.
- [ ] Separate pools: an attack pool under `{ total: 1 }` and the default pool under `{ total: 2 }` on one player give at most three counted holders. A head-of-line waiter in one pool never blocks a grant in the other.
- [ ] `start` on the active pool changes nothing: held turns, wait clocks and members stand. A pool `start` on a connected client changes nothing.
- [ ] Starting a second attack pool ends the first. Its members return to the default pool on that tick, and so does every member on `stop`. `stop` on an inactive pool changes nothing.
- [ ] Membership: a tag holder present at start and a later spawn whose `spawned_tags` carry the tag are members; an enemy whose tag is gone is not.
- [ ] An enemy leaving a pool while holding a turn keeps it. The turn counts against its new pool until it ends by its own rule, and no grant in that pool fits around it (pin P13).
- [ ] An empty `member_tag` is a compile error naming the entity. In both runtimes, a profile `total` or class limit below 1 or non-integer, a negative or non-integer `attackCost`, an empty `attackClass` or a non-finite value rejects with a pathed error.
- [ ] A level with no `ai_attack_pool` behaves as the default pool alone.
- [ ] Start and stop requests follow pin P15: a trigger-bound start applies before that tick's arbitration, and two starts on one tick leave the later-issued one active.
- [ ] An attack pool active when its level restarts or changes is not active in the next install, and its members draw from the default pool from that install's first tick.
- [ ] While an attack pool uses `default`, a mod reload that changes `default` changes that pool's limits from the next tick.
- [ ] Two levels differing only in an `ai_attack_pool`'s `budget_profile` produce the same level content digest.

**Surface**
- [ ] The Scripting surface example parses identically in TypeScript and Luau, and runs as `content/dev` content. After `arena.start` fires, a tagged guard draws from an attack pool under the `arena` profile.

### Manual
- [ ] On `movement-feel`, the shipped rifle squad under its authored budget: shooters relocate after a burst while waiters strafe and take over. A closet reveal on `closet-reveal` releases its payload without a pile-on. A rocket-launched knight keeps its facing in flight.
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
- **Shape.** Turns live on `BrainComponent`, as the held target, the counted pool and the wait clock (all transient). The attack pool's state lives on its own serializable component; its profile resolves by name each tick, so a mod reload applies. Per-pool, per-target totals are recounted each tick. Rival: a per-target map on `AiRuntime`, rejected as a second source of truth to clear on despawn.
- **Attack-pool rivals** are argued in `research.md` (Attack pool precedent).
- **Slice order.** The knockback facing split first: it touches the fewest seams and proves the velocity separation strafe needs. Then the verbs and the scorer's per-query constraints, turns and arbitration, profiles and classes, the attack pool entity, and last the shipped content that proves them together.
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
| turn marking | `BehaviorActivityDescriptor::attack_turn: bool` | `"attackTurn"` | `attackTurn?` | `attackTurn` | n/a |
| archetype cost | `BehaviorGraphDescriptor::attack_cost` | `"attackCost"` | `attackCost?` | `attackCost` | n/a |
| archetype class | `BehaviorGraphDescriptor::attack_class: Option<String>` | `"attackClass"` | `attackClass?` | `attackClass` | n/a |
| budget profiles | `ModManifestResult` field, name → `{ total, classes }` | `"attackBudgets"`: `{ [profile]: { total, [class]: limit } }` | `attackBudgets?` | `attackBudgets` | n/a |
| attack pool entity | attack pool component | id-29 map-entity record | — | — | `ai_attack_pool` (`@PointClass`): `_tags`, `member_tag`, `budget_profile(string)`; empty means `default` |
| attack pool member | attack pool member handle | — | `getMapEntities("attackPool")` → `start`, `stop` | same | trigger `command` `start` / `stop` |
| turn held | `BRAIN_INPUTS` tail | `"@brain.hasAttackTurn"` | `brain.hasAttackTurn` | `brain.hasAttackTurn` | n/a |
| turn free | `BRAIN_INPUTS` tail, after the above | `"@brain.attackTurnFree"` | `brain.attackTurnFree` | `brain.attackTurnFree` | n/a |
| directional variants | host resolution | state names `X_left`, `X_right`, `X_backward` | — | — | n/a |
