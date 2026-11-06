// The dev mod's damage types. Each matrix says how much one raw point takes
// from each pool kind, keyed by layer name or `health`; a missing pool is ×1.
// See: docs/scripting-reference.md (Defense layers and damage types)

import { defineDamageType } from "postretro";

// Bullets: armor shrugs off a fifth. The dev mod's default, so anything that
// names no type (bare melee, crushers, `.damage(n)`) deals it.
export const ballistic = defineDamageType({ armor: 0.8 });
// Energy: strips shields, halved by armor.
export const plasma = defineDamageType({ shield: 1.5, armor: 0.5 });
// Cracks armor and flesh.
export const explosive = defineDamageType({ armor: 1.5, health: 1.25, shield: 0.75 });
export const shock = defineDamageType({ shield: 2, armor: 0.5, health: 0.75 });
export const corrosive = defineDamageType({ shield: 0.5, armor: 2 });
// Slips past shields; standing armor stops it whole.
export const flechette = defineDamageType({ shield: "bypass", armor: 0 });
