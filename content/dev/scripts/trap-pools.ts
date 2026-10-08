// E18 trap-pool fixture. The pool roll chooses which closet triggers are live;
// each selected trigger then fires its own ordinary spawnFromSpawner reaction.

import { defineReaction, defineTriggerPool, getMapEntities } from "postretro";

// One named reaction per closet: each fires every spawner member carrying the
// closet's tag.
function spawnCloset(name: string, tag: string) {
  return defineReaction(name, {
    sequence: getMapEntities("spawner", { tag }).flatMap((s) => s.fire()),
  });
}

export function setupLevel() {
  return {
    reactions: [
      spawnCloset("trapPools.spawnClosetA", "trap_pools_closet_a"),
      spawnCloset("trapPools.spawnClosetB", "trap_pools_closet_b"),
      spawnCloset("trapPools.spawnClosetC", "trap_pools_closet_c"),
      spawnCloset("trapPools.spawnClosetD", "trap_pools_closet_d"),
      spawnCloset("trapPools.spawnAmbushA", "trap_pools_ambush_a"),
      spawnCloset("trapPools.spawnAmbushB", "trap_pools_ambush_b"),
      spawnCloset("trapPools.spawnAmbushC", "trap_pools_ambush_c"),
      spawnCloset("trapPools.spawnAmbushD", "trap_pools_ambush_d"),
    ],
    // The local count pool is deliberately separate from the mod-global
    // ambush percentage pool in start-script.ts.
    triggerPools: [defineTriggerPool({ tag: "closet_trap", arm: 2 })],
  };
}
