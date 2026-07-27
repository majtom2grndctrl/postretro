// DEMO CONTENT — data script for `combat-demo.map` (M10 entity health + damage).
//
// Reactions are surfaced through `setupLevel`'s returned `LevelManifest`, NOT
// through the mod manifest. The map wires this file in via its worldspawn
// `data_script` KVP; the engine runs `setupLevel(ctx)` at level load and drains
// `{ reactions, triggerEvents }` into the per-level registries.
//
// This file declares the combat reactions and a trigger-fed reference pickup:
//
//   1. A `progress` reaction named `combatDummyProgress` over the `dummy` spawn
//      tag. Its denominator (the
//      number of tagged `target_dummy` entities) is captured at level load; as
//      the player shoots dummies dead, the death sweep feeds each kill into the
//      progress tracker. When `killed / total >= at` (here 0.5 — half the
//      dummies), it fires the named event `dummiesCleared` exactly once.
//
//   2. An `applyDamage` reaction NAMED `dummiesCleared`, targeting the `player`
//      tag. When the progress reaction fires `dummiesCleared`, the event is
//      dispatched through the death-event drain (`fire_named_event_with_sequences`),
//      which is the ONLY drain that invokes primitive handlers. The handler
//      routes `amount: 35` through the `apply_damage` chokepoint on every
//      `player`-tagged entity, so the player's HP drops and the readonly
//      `player.health` HUD slot follows.
//
//   3. `ammoPickup` grants the trigger's activators 24 `bullets.light` ammo on
//      the `ammo_pickup` volume's enter edge. This is reference content only:
//      the engine has no concept of a reward, so a mod replaces the policy.
//
// Why this chain and not a simpler one:
//   - `levelLoad` fires before the first rendered frame, so an `applyDamage`
//     hung off `levelLoad` would drop HP invisibly (and there is nothing dead
//     yet). The damage must be *gameplay-driven* — hence the `progress` trigger.
//   - The plain `fire_named_event` drains (movement / weapon event names) never
//     invoke primitive handlers. Only a `progress` `fire` (which routes through
//     the death-event drain) can drive a visible HUD drop. So the event name the
//     progress reaction fires MUST match the name on the `applyDamage` reaction.
//
// Tag discipline: the `dummy` tag is EXCLUSIVE to the target dummies — the
// progress denominator counts ALL entities carrying the tag, so the player (and
// anything else) must NOT share it. The player carries its own `player` tag.
//
// See content/dev/maps/combat-demo.README.md for the full end-to-end walkthrough.

import {
  type NamedReactionDescriptor,
  type TriggerEventDescriptor,
  defineReaction,
  grantAmmo,
  onTriggerEvent,
} from "postretro";

// Half the dummies must die before the player takes the retaliation hit.
const KILL_FRACTION = 0.5;
// One finite, positive hit. Sized so the drop is obvious on the HUD without
// killing the player (player max is 100).
const RETALIATION_DAMAGE = 35;
// The event name fired by progress and handled by the applyDamage reaction.
const RETALIATION_EVENT = "dummiesCleared";
const PROGRESS_REACTION = "combatDummyProgress";
const AMMO_PICKUP_REACTION = "combat.ammoPickup";

export function setupLevel(_ctx: unknown): {
  reactions: NamedReactionDescriptor[];
  triggerEvents: TriggerEventDescriptor[];
} {
  const reactions: NamedReactionDescriptor[] = [];

  // (a) Progress threshold over the dummy tag. `fire` names the event emitted
  //     when `killed / total >= at`.
  reactions.push(
    defineReaction(PROGRESS_REACTION, {
      progress: { tag: "dummy", at: KILL_FRACTION, fire: RETALIATION_EVENT },
    }),
  );

  // (b) applyDamage reaction NAMED `dummiesCleared`, targeting the player tag.
  //     Fired by the progress threshold above through the death-event drain.
  reactions.push(
    defineReaction(RETALIATION_EVENT, {
      primitive: "applyDamage",
      tag: "player",
      args: { amount: RETALIATION_DAMAGE },
    }),
  );

  // The volume is a repeating dispenser in v1: it deliberately does not
  // disarm after paying the current activators.
  const ammoPickup = defineReaction(AMMO_PICKUP_REACTION, (on) =>
    grantAmmo(on.activators, "bullets.light", 24),
  );
  reactions.push(ammoPickup);

  const triggerEvents: TriggerEventDescriptor[] = [
    onTriggerEvent({ tag: "ammo_pickup" }, "enter", [ammoPickup]),
  ];

  return { reactions, triggerEvents };
}
