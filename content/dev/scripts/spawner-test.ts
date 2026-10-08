// E18 runtime-spawner test, and the scripts-first half of the monster-closet
// pair. spawner-test.map carries no reaction wiring — its trigger and mover
// hold only their _tags handles — so every causal edge lives here. The plate's
// enter edge fans out to three named reactions: the alarm light snaps red, the
// closet door moves, and each closet spawner member materializes fresh AI
// enemies at runtime. This exercises the spawnFromSpawner path (and the
// round-1 feet-to-center transform fix) rather than the pre-placed
// closet-reveal aggro gate.
//
// NOTE on timing: all three reactions dispatch on the same tick — the light
// turns red the same instant the door starts and the enemies spawn.

import { defineReaction, getMapEntities } from "postretro";

export function setupLevel() {
  const openDoor = defineReaction("closet.openDoor", {
    sequence: getMapEntities("mover", { tag: "closet_door" }).flatMap((m) => m.start()),
  });
  // Address the tagged spotlight by handle. A one-shot (playCount 1) color
  // animation whose only keyframe is red drives the light red and settles it
  // there — the bridge writes the final color back as the static component
  // color on completion, so it holds rather than reverting.
  const alarmLights = getMapEntities("light", { tag: "alarm_light" });
  // Each closet spawner fires its own batch: a member step, not a tag search.
  const spawnEnemies = defineReaction("closet.spawnEnemies", {
    sequence: getMapEntities("spawner", { tag: "closet_spawner" }).flatMap((s) => s.fire()),
  });
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
    triggerEvents: getMapEntities("trigger", { tag: "closet_reveal_plate" }).map((plate) =>
      plate.on("enter", [openDoor, spawnEnemies, turnRed]),
    ),
  };
}
