// Teleporter acceptance fixture. Luau twin: teleporters.luau.
// Explicit names and ordered member lists keep the installed manifests equal.
import { defineImpactEvent, defineReaction, getMapEntities, npcs } from "postretro";
import type { TeleportEventParams } from "postretro";

// The dev mod registers the enemy-death base. Only this override is installed
// here: successful removal, rather than a zero-HP corpse, advances progress.
const enemyDeath = defineImpactEvent("enemy-death", { tag: "enemy" }, () => []);
const closetDeath = enemyDeath.override({ tag: "teleport_closet" }, (impact) => [{
  when: impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0)),
  do: [impact.target.despawn()],
}]);

export function setupLevel() {
  const gate = getMapEntities("teleporter", { tag: "vault_gate" });
  const shift = getMapEntities("teleporter", { tag: "era_shift" });
  const stress = getMapEntities("teleporter", { tag: "stress_route" });
  if (stress.length > 0) {
    const arm = defineReaction("stress.arm", { sequence: stress.flatMap((t) => t.arm()) });
    return {
      reactions: [arm],
      triggerEvents: getMapEntities("trigger", { tag: "stress_button" }).map((t) => t.on("enter", [arm])),
    };
  }
  // Matched baselines keep the same script but contain no teleporter members.
  if (gate.length === 0 && shift.length === 0) return { reactions: [], triggerEvents: [], teleportEvents: [] };

  const button = getMapEntities("trigger", { tag: "vault_button" });
  const plate = getMapEntities("trigger", { tag: "closet_plate" });
  const doors = getMapEntities("mover", { tag: "closet_door" });
  const open = defineReaction("gate.open", { sequence: gate.flatMap((t) => t.arm()) });
  const shut = defineReaction("gate.shut", { sequence: gate.flatMap((t) => t.disarm()) });
  const cleared = defineReaction("closet.cleared", { progress: { tag: "closet", at: 1.0, fire: "gate.open" } });
  const reveal = defineReaction("closet.reveal", {
    sequence: [...doors.flatMap((door) => door.start()), npcs({ tag: "closet" }).update({ aggro: true })],
  });
  const timeShift = defineReaction("era.shift", { sequence: shift.flatMap((t) => t.arm()) });
  const reset = defineReaction("era.reset", { sequence: shift.flatMap((t) => t.disarm()) });
  const oneWay = defineReaction("gate.used", (on: TeleportEventParams) => on.teleporter.disarm());
  return {
    reactions: [open, shut, cleared, reveal, timeShift, reset, oneWay],
    events: [closetDeath],
    triggerEvents: [
      ...button.map((t) => t.on("enter", [open])),
      ...plate.map((t) => t.on("enter", [shut, reveal])),
      ...getMapEntities("trigger", { tag: "era_button" }).map((t) => t.on("enter", [timeShift])),
      ...getMapEntities("trigger", { tag: "era_plate" }).map((t) => t.on("enter", [timeShift])),
      ...getMapEntities("trigger", { tag: "era_reset" }).map((t) => t.on("enter", [reset])),
    ],
    teleportEvents: gate.map((t) => t.on("teleport", [oneWay])),
  };
}
