// E18 closet-reveal set piece. The plate's enter edge fans out to two named
// reactions: the door moves and the pre-placed enemies become aggro-eligible.
// Keep those bodies separate; tag-targeted primitives are fire-time work and
// do not belong in a sequence together.

import { defineReaction, enemies, onTriggerEvent } from "postretro";

const openDoor = defineReaction("closet.openDoor", {
  primitive: "moverStart",
  tag: "closet_door",
  args: {},
});

const releaseCloset = defineReaction(
  "closet.releaseCloset",
  enemies({ tag: "closet_enemies" }).update({ aggro: true }),
);

export function setupLevel() {
  return {
    reactions: [openDoor, releaseCloset],
    triggerEvents: [
      onTriggerEvent(
        { tag: "closet_reveal_plate" },
        "enter",
        [openDoor, releaseCloset],
      ),
    ],
  };
}
