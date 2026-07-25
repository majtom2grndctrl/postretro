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

// How long a corpse lies there before it is removed. Long enough to read the
// death clip and register the kill; short enough that a firefight does not
// leave the floor covered.
const CORPSE_LINGER_MS = 4_000;

// The default death for every `enemy`-tagged entity in the dev mod. Without a
// policy an enemy that reaches zero HP is *already dead* to the engine — the
// death sweep latches it and the AI pass skips it — but nothing plays a clip or
// removes it, so it stands there mid-pose and reads as unkillable. This supplies
// the presentation and the removal.
//
// This is the base of a base/override pair. Selection evicts an earlier matching
// policy with the same `id`, and an override matches only where BOTH its own tag
// and its base's tag are present — so a specialization is always a strict subset
// of this policy's reach, and where it applies this one does not run at all.
export const enemyDeath = defineImpactEvent(
  "dev:enemy-death",
  // Deliberately mod-global with no `levels`: a level-gated base never lands its
  // filter, and its overrides are then dropped as targeting an unknown event.
  { tag: "enemy" },
  (impact) => {
    const target = impact.target;
    const killed = target.healthBefore.gt(0).and(target.healthAfter.le(0));
    const gibbed = target.healthAfter.le(FINISHER_OVERSHOOT);

    return [
      // A gib skips the clip entirely — there is no body left to animate.
      { when: gibbed, do: [target.despawn()] },
      // Down is an edge, so further hits on a corpse cannot restart the timer.
      {
        when: killed.and(gibbed.not()),
        do: [target.playAnim("death"), target.despawn({ afterMs: CORPSE_LINGER_MS })],
      },
    ];
  },
);

// The combat-demo zombie keeps getting back up, which is the point of that
// walkthrough. Authored as an override of `dev:enemy-death` rather than as its
// own event: a separate id would not evict the base, so the zombie would be
// despawned by the default policy AND resurrected by this one on the same hit.
//
// The map tags it `enemy combat-zombie`, which is what makes the override match.
export const combatZombieLifecycle = enemyDeath.override(
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
