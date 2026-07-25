// Switch-entity fixture. The switch brush in switch-demo.map is visible, solid,
// and pressable; its `on_fire` KVP names this reaction, so pressing use on the
// console starts the door mover.

import { defineReaction } from "postretro";

const openDoor = defineReaction("switchDemo.openDoor", {
  primitive: "moverStart",
  tag: "switch_demo_door",
  args: {},
});

export function setupLevel() {
  return { reactions: [openDoor] };
}
