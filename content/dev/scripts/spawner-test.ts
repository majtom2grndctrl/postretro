// E18 runtime-spawner test. The plate's enter edge fans out to two named
// reactions: the closet door moves, and the spawner materializes fresh AI
// enemies at runtime. This exercises the spawnFromSpawner path (and the
// round-1 feet-to-center transform fix) rather than the pre-placed
// closet-reveal aggro gate. Keep the bodies separate; tag-targeted primitives
// are fire-time work and do not belong in a sequence together.

import { defineReaction, onTriggerEvent, spawner } from "postretro";

const openDoor = defineReaction("closet.openDoor", {
  primitive: "moverStart",
  tag: "closet_door",
  args: {},
});

const spawnEnemies = defineReaction(
  "closet.spawnEnemies",
  spawner({ tag: "closet_spawner" }).fire(),
);

export function setupLevel() {
  return {
    reactions: [openDoor, spawnEnemies],
    triggerEvents: [
      onTriggerEvent(
        { tag: "closet_reveal_plate" },
        "enter",
        [openDoor, spawnEnemies],
      ),
    ],
  };
}
