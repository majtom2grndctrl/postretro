// DEMO CONTENT — mod-global impact lifecycle for the combat demo.
//
// The dummy and zombie descriptors supply targetable health pools. These
// policies supply the game meaning of a hit: an ordinary lethal hit downs the
// target and queues recovery; a follow-up hit while it is down gibs it. Keeping
// this in the dev mod's manifest, rather than the map data script, demonstrates
// the reusable mod-global tier that future enemy, reward, and presentation
// policies build on. Reward behavior is reference content only: the engine has
// no concept of a reward, so a mod replaces these policies wholesale.

import { defineImpactEvent, defineStore, update } from "postretro";

// REFERENCE CONTENT — XP belongs to its earning player, while teamKills is one
// session-wide counter. The two slots share a policy below so the cardinality
// decision is visible in the authoring surface rather than hidden in engine code.
export const progression = defineStore("progression", {
  xp: {
    type: "number",
    default: 0,
    perOwner: true,
    persist: true,
    network: "ownerPrivate",
  },
  teamKills: {
    type: "number",
    default: 0,
    network: "shared",
  },
});

const RESURRECT_DELAY_MS = 3_000;
// This is one authored per-impact raw-overkill rule, not accumulated shell
// damage. Stored health floors at zero, so a 3-damage corpse pellet reads
// `healthAfter = -3`. The 48-HP dummy therefore downs on the final pellet of
// its second 24-damage shell, then its next shell gibs on its first pellet.
// Mods that do not want gibbing omit or replace the despawn branch below.
const FINISHER_OVERSHOOT = -3;

export const combatDummyLifecycle = defineImpactEvent(
  "combat-dummy-lifecycle",
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
  "enemy-death",
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

// REFERENCE CONTENT — this mod-global kill payout grants 8 `shells.buck` per
// dummy kill, plus per-player XP and one shared team-kill count. It is one
// possible economy policy, not engine behavior; a real mod replaces it whole.
export const ammoOnKill = defineImpactEvent(
  "ammo-on-kill",
  { tag: "dummy" },
  (impact) => {
    const killed = impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0));

    return [
      {
        when: killed,
        do: [
          impact.source.grantAmmo("shells.buck", 8),
          // Same reward and policy; only the slot and this owner address decide
          // that XP is one per-player pot rather than one shared session pot.
          update(progression.xp.byPlayer(impact.source), (cur) => cur.plus(10)),
          update(progression.teamKills, (cur) => cur.plus(1)),
        ],
      },
    ];
  },
);

// The combat-demo zombie keeps getting back up, which is the point of that
// walkthrough. Authored as an override of `enemy-death` rather than as its
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
