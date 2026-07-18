// E18 trap-pool fixture. The pool roll chooses which closet triggers are live;
// each selected trigger then fires its own ordinary spawnFromSpawner reaction.

import { defineReaction, defineTriggerPool, spawner } from "postretro";

const spawnClosetA = defineReaction(
  "trapPools.spawnClosetA",
  spawner({ tag: "trap_pools_closet_a" }).fire(),
);
const spawnClosetB = defineReaction(
  "trapPools.spawnClosetB",
  spawner({ tag: "trap_pools_closet_b" }).fire(),
);
const spawnClosetC = defineReaction(
  "trapPools.spawnClosetC",
  spawner({ tag: "trap_pools_closet_c" }).fire(),
);
const spawnClosetD = defineReaction(
  "trapPools.spawnClosetD",
  spawner({ tag: "trap_pools_closet_d" }).fire(),
);

const spawnAmbushA = defineReaction(
  "trapPools.spawnAmbushA",
  spawner({ tag: "trap_pools_ambush_a" }).fire(),
);
const spawnAmbushB = defineReaction(
  "trapPools.spawnAmbushB",
  spawner({ tag: "trap_pools_ambush_b" }).fire(),
);
const spawnAmbushC = defineReaction(
  "trapPools.spawnAmbushC",
  spawner({ tag: "trap_pools_ambush_c" }).fire(),
);
const spawnAmbushD = defineReaction(
  "trapPools.spawnAmbushD",
  spawner({ tag: "trap_pools_ambush_d" }).fire(),
);

export function setupLevel() {
  return {
    reactions: [
      spawnClosetA,
      spawnClosetB,
      spawnClosetC,
      spawnClosetD,
      spawnAmbushA,
      spawnAmbushB,
      spawnAmbushC,
      spawnAmbushD,
    ],
    // The local count pool is deliberately separate from the mod-global
    // ambush percentage pool in start-script.ts.
    triggerPools: [defineTriggerPool({ tag: "closet_trap", arm: 2 })],
  };
}
