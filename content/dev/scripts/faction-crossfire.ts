// REFERENCE CONTENT — faction-driven crossfire retaliation.
//
// These two map-placeable variants reuse the reference enemy's presentation
// and statechart so a dev map can demonstrate only the relationship policy:
// a volatile raider turns on any recent over-tolerance attacker, while a stoic
// sentinel stays with its nearer player target under the same hit. The engine
// owns the selection rank and retention latch; this file owns faction,
// tolerance, scalar tuning, and the authored stand-down guard.

import { brain, defineEntity } from "postretro";
import type { EntityTypeDescriptor } from "postretro";
import { referenceEnemyEntity } from "./reference-enemy";

export const CROSSFIRE_RAIDERS_FACTION = "crossfire.raiders";
export const CROSSFIRE_SENTINELS_FACTION = "crossfire.sentinels";

export const CROSSFIRE_RAIDER_CLASSNAME = "crossfire_raider";
export const CROSSFIRE_SENTINEL_CLASSNAME = "crossfire_sentinel";

// A finite literal is important here: script data crosses as a JavaScript
// number before the manifest validates the f32 surface. This is f32::MAX, not
// JavaScript's much larger Number.MAX_VALUE.
export const MAX_RETALIATION_TOLERANCE = 3.4028234663852886e38;

export const RAIDER_TOLERANCE = 4;

const referenceBehavior = referenceEnemyEntity.components.behavior;
if (referenceBehavior === undefined || referenceBehavior === null) {
  throw new Error("reference_enemy must retain its behavior graph for the crossfire fixtures");
}

// The retaliation term is intentionally configured as scalar tuning rather
// than an authored rank expression. The wildcard belongs to content: it drops
// the retained retaliator only once the target dies or leaves sight, which is
// the transient stand-down promised by the reference fixture.
const crossfireBehavior = {
  ...referenceBehavior,
  retaliation: {
    windowMs: 1500,
    damageWeight: 1,
    recencyWeight: 0.001,
  },
  transitions: {
    ...referenceBehavior.transitions,
    "*": [
      {
        to: "patrol",
        when: brain.targetDied.or(brain.targetVisible.not()),
      },
    ],
  },
};

function crossfireEnemy(
  canonicalName: string,
  faction: string,
  tolerance: number,
): EntityTypeDescriptor {
  return defineEntity({
    canonicalName,
    components: {
      ...referenceEnemyEntity.components,
      faction,
      tolerance,
      behavior: crossfireBehavior,
    },
  });
}

/** Low-tolerance archetype: damage above four points can steal its target. */
export const crossfireRaiderEntity = crossfireEnemy(
  CROSSFIRE_RAIDER_CLASSNAME,
  CROSSFIRE_RAIDERS_FACTION,
  RAIDER_TOLERANCE,
);

/** Max-tolerance control archetype: identical finite damage never retaliates. */
export const crossfireSentinelEntity = crossfireEnemy(
  CROSSFIRE_SENTINEL_CLASSNAME,
  CROSSFIRE_SENTINELS_FACTION,
  MAX_RETALIATION_TOLERANCE,
);
