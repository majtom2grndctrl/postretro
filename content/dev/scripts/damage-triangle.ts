// DEV FIXTURE — the damage-triangle playtest: a shock gun, a corrosive gun,
// and a shielded reference-enemy variant, beside the armored reference enemy.
// Shock strips shields (×2) and is halved by armor; corrosive eats armor (×2)
// and is halved by shields. Map-placeable by canonical name.
// See: docs/scripting-reference.md (Defense layers and damage types)

import { defineEntity, defineReaction, npcs } from "postretro";
import { corrosive, shock } from "./damage-types";
import { shield } from "./defenses";
import { referenceEnemyEntity } from "./reference-enemy";
import { referenceRifleEntity } from "./reference-rifle";

const rifleWeapon = referenceRifleEntity.components.weapon;

export const arcRifleEntity = defineEntity({
  canonicalName: "arc_rifle",
  components: {
    ...referenceRifleEntity.components,
    weapon: { ...rifleWeapon, damage: 12, damageType: shock },
  },
});

export const acidRifleEntity = defineEntity({
  canonicalName: "acid_rifle",
  components: {
    ...referenceRifleEntity.components,
    weapon: { ...rifleWeapon, damage: 12, damageType: corrosive },
  },
});

// The reference enemy's components, with a recharging shield in place of its
// armor and its flesh resistance.
export const shieldedEnemyEntity = defineEntity({
  canonicalName: "reference_enemy_shielded",
  components: {
    ...referenceEnemyEntity.components,
    health: {
      ...referenceEnemyEntity.components!.health!,
      max: 70,
      layers: [{ layer: shield, capacity: 60, rechargeRate: 15 }],
      resist: {},
    },
  },
});

// Typed script damage: a 40-point shock overload on every turret-tagged NPC.
export const overload = defineReaction("turret.overload", npcs({ tag: "turret" }).damage(40, { type: shock }));
