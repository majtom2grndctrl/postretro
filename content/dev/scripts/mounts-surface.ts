// The `mesh.mounts` Scripting surface example as dev content: a limitator-shaped
// archetype mounting both of its rifles on one bake, and a body-built weapon
// naming a muzzle socket instead of a bake. Not placed in any map; the canonical
// names keep it clear of the real `limitator` and `cannon_brute`.
//
// Hand-authored beside `mounts-surface.luau`: the scripting-core twin parser
// test keeps their lowered descriptors equal.
// See: context/lib/entity_model.md §2 (Weapon mounts)

import { attackTurnFree, brain, defineAttack, defineEntity, defineStance } from "postretro";
import type { EntityTypeDescriptor } from "postretro";
import { armCannonEntity as armCannon } from "./arm-cannon";
import { ranged } from "./attack-tokens";
import { enemyRifleBurstEntity as burstRifle, enemyRifleLongEntity as longRifle } from "./enemy-rifle";

const aim = defineStance({ animation: "idle_aiming", holdMs: 500 });
const settle = defineStance({ animation: "idle_aiming" });
const burst = defineAttack({
  draws: [ranged],
  weapon: "enemy_rifle_burst",
  standoffDistance: 6,
  beginningStance: aim,
  endingStance: settle,
  release: { animation: "shoot", at: false },
});
const longshot = defineAttack({
  draws: [ranged],
  weapon: "enemy_rifle_long",
  standoffDistance: 6,
  beginningStance: aim,
  endingStance: settle,
  release: { animation: "shoot", at: false },
});

export const surfaceLimitatorEntity: EntityTypeDescriptor = defineEntity({
  canonicalName: "surface_limitator",
  components: {
    mesh: {
      model: "models/limitator/model.gltf",
      // Both rifles on the one bake: they share `hand_r` with one model.
      mounts: [
        { item: burstRifle, attachments: { hand_r: "models/ar_4/limitator.gltf" } },
        { item: longRifle, attachments: { hand_r: "models/ar_4/limitator.gltf" } },
      ],
      animations: {
        idle: { clip: "idle", loop: true },
        idle_aiming: { clip: "idle_aiming", loop: true },
        shoot: { clip: "firing_rifle", loop: true, crossfadeMs: 80 },
      },
      defaultState: "idle",
    },
    behavior: {
      initial: "close",
      moveSpeed: 4,
      activities: {
        close: { animation: "idle", motion: "chaseTarget" },
      },
      transitions: {
        close: [{ to: aim, when: brain.targetVisible.and(attackTurnFree(burst)) }],
        aim: [{ to: burst }, { to: longshot, when: brain.targetDistance.gt(8) }],
        // Leave once the activity fired, or its gun was held by near cover.
        settle: [
          {
            to: "close",
            when: brain.attacksFiredInActivity.ge(1).or(brain.attacksHeldInActivity.ge(1)),
          },
        ],
      },
    },
  },
});

const cannonShot = defineAttack({
  draws: [ranged],
  weapon: "arm_cannon",
  standoffDistance: 8,
  beginningStance: aim,
  endingStance: settle,
  release: { animation: "shoot", at: false },
});

// A weapon built into the body: `cannon_tip` is a skin joint on the model.
export const surfaceCannonBruteEntity: EntityTypeDescriptor = defineEntity({
  canonicalName: "surface_cannon_brute",
  components: {
    mesh: {
      model: "models/mutant_arm_cannon/model.gltf",
      mounts: [{ item: armCannon, muzzleSocket: "cannon_tip" }],
      animations: {
        idle: { clip: "idle", loop: true },
        idle_aiming: { clip: "idle", loop: true },
        shoot: { clip: "smash_strike", loop: false, interrupt: "snap" },
      },
      defaultState: "idle",
    },
    behavior: {
      initial: "close",
      moveSpeed: 3,
      activities: {
        close: { animation: "idle", motion: "chaseTarget" },
      },
      transitions: {
        close: [{ to: aim, when: brain.targetVisible }],
        aim: [{ to: cannonShot }],
        settle: [{ to: "close", when: brain.timeInActivityMs.ge(400) }],
      },
    },
  },
});
