// E18 runtime-spawner test, and the scripts-first half of the monster-closet
// pair. spawner-test.map carries no reaction wiring — its trigger and mover
// hold only their _tags handles — so every causal edge lives here. The plate's
// enter edge fans out to three named reactions: the alarm light snaps red, the
// closet door moves, and the spawner materializes fresh AI enemies at runtime.
// This exercises the spawnFromSpawner path (and the round-1 feet-to-center
// transform fix) rather than the pre-placed closet-reveal aggro gate. Keep the
// bodies separate; tag-targeted primitives are fire-time work and do not belong
// in a sequence together.
//
// NOTE on timing: all three reactions dispatch on the same tick — the light
// turns red the same instant the door starts and the enemies spawn. A timed
// "light reddens, beat, doors slam open" pause is not expressible yet (no wait
// step in the SequenceStep union; onComplete chains synchronously). Tracked on
// the roadmap under Epic 18 (timed / delayed reaction steps).

import { defineReaction, onTriggerEvent, spawner, world } from "postretro";

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
  // Address the tagged spotlight by handle. A one-shot (playCount 1) color
  // animation whose only keyframe is red drives the light red and settles it
  // there — the bridge writes the final color back as the static component
  // color on completion, so it holds rather than reverting.
  const alarmLights = world.query({ component: "light", tag: "alarm_light" });
  const turnRed = defineReaction("closet.turnRed", {
    sequence: alarmLights.map((light) => ({
      id: light.id,
      primitive: "setLightAnimation" as const,
      args: {
        periodMs: 200,
        phase: null,
        playCount: 1,
        startActive: true,
        brightness: null,
        color: [{ x: 1, y: 0, z: 0 }],
        direction: null,
      },
    })),
  });

  return {
    reactions: [openDoor, spawnEnemies, turnRed],
    triggerEvents: [
      onTriggerEvent(
        { tag: "closet_reveal_plate" },
        "enter",
        [openDoor, spawnEnemies, turnRed],
      ),
    ],
  };
}
