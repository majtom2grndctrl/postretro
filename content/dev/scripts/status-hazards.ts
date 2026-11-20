// The dev mod's status rules and hazards: an igniting impact policy, a cure,
// a burn trap, the incendiary rifle and lava. The Scripting surface example
// (scripts/statuses-demo.ts) is built from these; content/dev registers them
// in start-script.ts, and maps/e24-environment-test.map places the lava pool
// and the trap.
// See: docs/scripting-reference.md (Statuses)

import {
  defineEntity,
  defineFluid,
  defineImpactEvent,
  defineReaction,
  defineTriggerEvent,
  hasStatus,
  npcs,
  players,
  when,
  type TriggerEventParams,
} from "postretro";
import { acid, fireDamage } from "./damage-types";
import { referenceRifleEntity } from "./reference-rifle";
import { burning, corroding, scalding, wet } from "./statuses";

// Fire ignites what is not wet; acid dries it; a burn kill plays its own
// death. The `impact.status(burning)` guard keeps a burn tick (itself fire)
// from reapplying `burning` and so burning forever.
export const ignite = defineImpactEvent("statuses.ignite", { tag: "enemy" }, (impact) => [
  when(
    impact.type(fireDamage).and(impact.status(burning).not()).and(impact.target.hasStatus(wet).not()),
    [impact.target.applyStatus(burning)],
  ),
  when(impact.type(acid), [impact.target.clearStatus(wet)]),
  when(impact.status(burning).and(impact.target.healthAfter.le(0)), [impact.target.playAnim("burn_death")]),
]);

// Clears every player's corrosion.
export const cure = defineReaction("statuses.cure", players().clearStatus(corroding));

// Every volume tagged `burn_trap` sets whoever enters on fire, and kills
// every NPC tagged `trap_victim` with script damage, so its `enemy-death`
// policy plays the death and despawns the corpse. 177 fire kills a reference
// enemy without gibbing it: 120 empties its armor, and the raw 57 left deals
// 71.25 to its 70 health, short of the -3 gib threshold.
export const burnTrapFire = defineReaction("statuses.burnTrap", (on: TriggerEventParams) => [
  on.activators.applyStatus(burning),
  npcs({ tag: "trap_victim" }).damage(177, { type: fireDamage }),
]);
export const burnTraps = defineTriggerEvent({ tag: "burn_trap", event: "enter", fire: [burnTrapFire] });

// A brain transition: flee while burning. No dev brain carries it yet.
export const panic = { to: "flee", when: hasStatus(burning) };

// Lava scalds whatever stands in it, at any depth.
export const lava = defineFluid({
  name: "lava",
  viscosity: 0.6,
  viewTint: [1, 0.35, 0.05, 0.8],
  muffle: 0.9,
  surfaceOpacity: 1,
  status: scalding,
});

// The reference rifle firing fire: ignites enemies through `ignite`.
const rifleWeapon = referenceRifleEntity.components.weapon;
export const incendiaryRifleEntity = defineEntity({
  canonicalName: "incendiary_rifle",
  components: {
    ...referenceRifleEntity.components,
    weapon: { ...rifleWeapon, damage: 8, damageType: fireDamage },
  },
});
