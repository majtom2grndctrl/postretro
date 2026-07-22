// DEMO CONTENT — mod-global impact lifecycle for the combat demo.
//
// The dummy and zombie descriptors supply targetable health pools. These
// policies supply the game meaning of a hit: an ordinary lethal hit downs the
// target and queues recovery; a follow-up hit while it is down gibs it. Keeping
// this in the dev mod's manifest, rather than the map data script, demonstrates
// the reusable mod-global tier that future enemy, reward, and presentation
// policies build on.

import { defineImpactEvent } from "postretro";

const RESURRECT_DELAY_MS = 3_000;
// The reference pistol deals 12 damage. A third hit from 6 HP lands at -6 and
// downs the dummy; the next hit from zero lands at -12 and becomes the finisher.
const FINISHER_OVERSHOOT = -12;

export const combatDummyLifecycle = defineImpactEvent(
  "dev:combat-dummy-lifecycle",
  // `target_dummy` is currently exclusive to combat-demo. Do not use a
  // catalog-level filter here: direct CLI map loads intentionally have no
  // catalog tags, and the walkthrough must work through that normal dev path.
  { tag: "dummy" },
  (impact) => {
    const target = impact.target;
    const killed = target.healthBefore.gt(0).and(target.healthAfter.le(0));
    const gibbed = target.healthAfter.le(FINISHER_OVERSHOOT);

    return [
      // A gib is a level: a downed target may be gibbed by a later hit.
      { when: gibbed, do: [target.despawn()] },
      // Down is an edge, so repeated body hits cannot restart recovery.
      {
        when: killed.and(gibbed.not()),
        do: [target.setHealth(target.maxHealth, { afterMs: RESURRECT_DELAY_MS })],
      },
    ];
  },
);

export const combatZombieLifecycle = defineImpactEvent(
  "dev:combat-zombie-lifecycle",
  // This map-local tag makes the policy work for direct CLI loads without
  // changing the reference enemy's behavior in other dev maps.
  { tag: "combat-zombie" },
  (impact) => {
    const target = impact.target;
    const killed = target.healthBefore.gt(0).and(target.healthAfter.le(0));
    const gibbed = target.healthAfter.le(FINISHER_OVERSHOOT);

    return [
      { when: gibbed, do: [target.despawn()] },
      {
        when: killed.and(gibbed.not()),
        do: [
          target.playAnim("death"),
          target.setHealth(target.maxHealth, { afterMs: RESURRECT_DELAY_MS }),
        ],
      },
    ];
  },
);
