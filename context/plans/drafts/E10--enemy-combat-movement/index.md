# E10--enemy-combat-movement

Brief · resumable · Epic 10 · reads: `context/lib/entity_model.md` §7c · `context/lib/scripting.md` §11 · `context/lib/networking.md` §Combat authority · read at 5932287f

## Problem
The owner wants combat that is more fun to fight and easier to design encounters around. This is a requested capability, plus one observed defect. Today an engaged enemy can only chase, hold, or walk to a fixed point. It cannot strafe while shooting, fire a few shots and then relocate, or wait its turn while others attack. Every enemy in range attacks at once, so a group reads as noise rather than a pattern a player can learn. The defect: a rocket-launched enemy turns to face its flight path, because knockback is added into the same velocity that facing and locomotion read. When this is done, an author can give an enemy strafing and fire-then-relocate behavior that reads correctly on screen. A mod can cap how many enemies attack one target at a time, so idle enemies keep moving instead of piling on. A launched enemy keeps the yaw it had at impact.

## Decisions
- **Two new engaged motion verbs, `strafe` and `reposition`.** Both resolve their goals on the combat-slot ring (`plans/done/E10--enemy-combat-positioning`) at the standoff `chaseTarget` would use. Both hold the target and may carry an action, unlike position goals (`entity_model.md` §7c).
  - **`strafe`** steps to the neighboring ring slot and keeps going in one direction. It reverses when the next slot is unreachable, occupied, or blind to the target.
  - **`reposition`** claims the best slot outside the spacing radius of the one it held, picks a fresh slot on each entry, and holds there once it arrives.
- Verbs stay closed and parameterless: an activity selects an engine behavior, it does not describe one. Back-off and kiting are a non-goal. `reposition` covers fire-then-move, and a third verb waits for a consumer.
- **Per-activity `facing`: `"target"` or `"travel"`.**
  - Leaving it out keeps the verb's default. The existing verbs keep today's `facing_direction` rule, and `strafe` and `reposition` face the target.
  - Facing the target while moving is what lets strafe-fire pass the shipped facing gate. That gate is unchanged.
- **Knockback never steers facing or picks a stride.** Facing and locomotion read steering velocity only, never the knockback part.
  - A launched enemy holds its impact yaw.
  - An enemy whose active activity faces the target re-aims at the normal slew rate while airborne.
  - A launched enemy plays `jump_loop` when its mesh declares one. That mirrors the player's launch rule (`MeshDescriptor.animations` typedoc). Otherwise it keeps its current state rather than a travel cycle.
- **Directional travel variants.** While an enemy moves under a target facing, its activity animation `X` resolves to `X_left`, `X_right` or `X_backward` from travel direction relative to facing. A missing variant plays `X`. This extends the player's `walk_backward` naming to the side, and matches the shipped Mixamo rifle clip library.
- **Attack budget: the engine owns the mechanism, the mod owns the numbers.**
  - **Budget and cost.** A mod-wide `attackBudget` per target goes in the `ModManifest`, as a top-level scalar beside `factionSentimentDecay`, and joins the compatibility digest. An archetype's `behavior.attackCost` defaults to 1.
  - **Absent means unlimited.** With no budget authored, every request is granted and behavior is unchanged.
  - **When a turn is granted.** The held cost plus the requester's cost must fit the budget, or nobody holds a turn on that target, so an oversized archetype is never locked out.
- **Turns are scoped to activities.**
  - An activity marked `attackTurn: true` requests a turn on entry and keeps requesting until granted. The turn is held while that activity is on the active path.
  - An attack activity outside any such activity requests on its own entry.
  - A turn is released on leaving that activity, on a target change, and on quiescence or despawn.
- **The budget is a hard gate, plus two facts.**
  - With a budget authored, an attack starts only while its enemy holds a turn on its target. A weapon sequence that has already started keeps its shipped cancellation rules.
  - Guards read `@brain.hasAttackTurn` and `@brain.attackTurnFree`. Both are appended to the tail of `BRAIN_INPUTS` (`scripting.md` §11, append-only).
  - The gate is engine floor, like line of sight (`perception::fire_gate`), so no graph can opt out of fairness.
- **Arbitration is host-only and deterministic.** Requests resolve during the serial apply pass. The longest-waiting requester wins, measured since it last held a turn, and existing slot order breaks ties. Turns are derived host state and never replicated: clients see only the resulting activities.
- **Non-goals:**
  - **Softening attacks from outside the target's view.** A follow-up item on the budget.
  - **Protective cover and squad tactics.** Their exclusion stands from the line-of-sight and positioning briefs.
  - **Per-activity speed.** The statecharts brief keeps that additive.

### Scripting surface
```ts
// Proposed design
// start-script.ts
export default defineMod({ /* … */ attackBudget: 2 });

// rifle trooper behavior (excerpt)
behavior: {
  attackCost: 1,
  // enemy_rifle_burst: a three-shot activation program
  attacks: { shoot: { weapon: "enemy_rifle_burst", standoffDistance: 6 } },
  activities: {
    engage: {
      animation: "walk",
      layers: {
        offense: {
          initial: "wait",
          activities: {
            wait:  { animation: "walk", motion: "strafe" },             // faces target, steps the ring
            burst: { animation: "aim", motion: "hold", action: { attack: "shoot" }, attackTurn: true },
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
```
The Luau twin mirrors it field for field.

## Acceptance
### Automated
**Motion**
- [ ] A `strafe` enemy moves around its target at standoff while facing it, and fires its action mid-move when facing, range and line of sight pass.
- [ ] `strafe` reverses at a blocked, occupied or blind next slot. Two strafers on one target never claim the same slot on one tick.
- [ ] `reposition` arrives at a slot outside the spacing radius of the slot held at entry. Re-entering picks again. With no other valid slot, it holds rather than oscillating.
- [ ] `facing: "travel"` on `reposition` faces travel. `facing: "target"` on `chaseTarget` faces the target while moving. An activity without `facing` behaves exactly as today on every existing verb.
- [ ] Sideways travel under target facing selects `walk_left` or `walk_right`, and backward travel selects `walk_backward`. A mesh without those states plays `walk`.

**Knockback**
- [ ] Yaw is unchanged on the impact tick and through the flight of an enemy launched under a non-target-facing activity.
- [ ] Under a target-facing activity, the launched enemy slews toward the target by no more than the facing rate per tick.
- [ ] A launched enemy plays `jump_loop` when declared, and never a travel cycle driven by knockback speed.

**Budget**
- [ ] Without `attackBudget`, every existing AI and determinism test passes unchanged.
- [ ] With budget 2 and cost 1, a third enemy in its turn activity never starts an attack until a holder leaves. On the tick a holder leaves, the longest-waiting requester is granted.
- [ ] With three requesters on one tick and one turn free, exactly one is granted. Replaying the same run grants the same one.
- [ ] Cost 2 under budget 2 is granted alone and refused beside a cost-1 holder. Cost 3 under budget 2 is granted when no one holds a turn.
- [ ] A turn is released on target death, target switch, the holder's death and leaving the activity. No turn leaks across any of these.
- [ ] A three-shot enemy weapon fires all three shots inside one held turn. Releasing the turn mid-sequence follows the shipped cancellation rules and starts nothing new.
- [ ] `hasAttackTurn` and `attackTurnFree` read 0 and 1 correctly for a holder, a waiter, and an enemy with no target.
- [ ] `attackBudget` and `attackCost` reject a non-finite or negative value with a pathed error in both runtimes. The Scripting surface example parses identically in TypeScript and Luau.

### Manual
- [ ] On `movement-feel`, a rifle squad under budget 2: two fire while the others strafe. The shooters relocate after a burst and the waiters take over. A rocket-launched knight keeps its facing in flight.
- [ ] Co-op loopback: a client sees the same strafing, directional clips and launch facing as the host.

## Path
- **Seams to read first.**
  - `facing_direction` and step 4 of `evaluate` in `crates/ai/src/compute.rs`.
  - `apply_outcomes` in `crates/ai/src/apply.rs`, where serial arbitration already re-checks outcomes before `commit_attack_fire` and `AiWeaponControllers::advance_actor`.
  - `resolve_combat_slots` and `select_combat_positions_batch`.
  - `agent_steering::path_state`, which exports `velocity`. The knockback part is `AgentComponent::knockback_velocity`.
  - `steering_for` and `position_goal_steering`.
- **Shape.** Turns live on `BrainComponent` as the held target plus the wait clock. Per-target totals are recounted each tick in apply order. The rival is a per-target map on `AiRuntime`. Rejected: it adds a second source of truth that must be cleared on despawn.
- **First slice.** The facing split and the knockback fix. They touch the fewest seams and prove the steering-velocity separation that strafe depends on.
- **Watch for.** The combat-slot hysteresis never selects anything in this caller: the previous slot is not passed once the hold expires. `reposition`'s exclusion needs the held slot passed explicitly.
- **Prior art.** `drafts/perf-ai-tick-cost` touches the same batch. Read it before widening the slot query.

## Open questions
- The `strafe` direction on first entry — **delegated**: deterministic, from the better-scored neighbor. Report the rule.
- Whether `reposition`'s exclusion radius is the slot spacing or a multiple of it — **delegated**: tune on `movement-feel` and report.
