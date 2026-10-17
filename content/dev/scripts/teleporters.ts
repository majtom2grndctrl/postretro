// Teleporter acceptance fixture. Luau twin: teleporters.luau.
// Explicit names and ordered member lists keep the installed manifests equal.
import { defineImpactEvent, defineReaction, getMapEntities, npcs } from "postretro";
import type { SequenceStep, TeleportEventParams } from "postretro";

// The purple gate's lamp brightness: dim while the gate is disarmed, bright while armed.
const LAMP_DISARMED = 0.2;
const LAMP_ARMED = 1.0;

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
  // Every gate arm and disarm below also sets the lamp, so it tracks the armed state.
  const lamps = getMapEntities("light", { tag: "gate_lamp" });
  const lamp = (armed: boolean): SequenceStep[] => lamps.map((light) => ({
    id: light.id,
    primitive: "setLightAnimation" as const,
    args: { periodMs: 1000, brightness: [armed ? LAMP_ARMED : LAMP_DISARMED] },
  }));
  // The gate starts disarmed (`enabled_on_spawn` 0), so its lamp starts dim.
  const lampAtLoad = defineReaction("levelLoad", { sequence: lamp(false) });
  // Matched baselines keep the same script but contain no teleporter members.
  // The hub baseline keeps the lamp, which stays dim with no gate to arm.
  if (gate.length === 0 && shift.length === 0) {
    return { reactions: lamps.length > 0 ? [lampAtLoad] : [], triggerEvents: [], teleportEvents: [] };
  }

  const button = getMapEntities("trigger", { tag: "vault_button" });
  const plate = getMapEntities("trigger", { tag: "closet_plate" });
  const doors = getMapEntities("mover", { tag: "closet_door" });
  const gateState = (armed: boolean): SequenceStep[] => [
    ...gate.flatMap((t) => (armed ? t.arm() : t.disarm())),
    ...lamp(armed),
  ];
  const open = defineReaction("gate.open", { sequence: gateState(true) });
  const shut = defineReaction("gate.shut", { sequence: gateState(false) });
  const cleared = defineReaction("closet.cleared", { progress: { tag: "closet", at: 1.0, fire: "gate.open" } });
  const reveal = defineReaction("closet.reveal", {
    sequence: [...doors.flatMap((door) => door.start()), npcs({ tag: "closet" }).update({ aggro: true })],
  });
  const timeShift = defineReaction("era.shift", { sequence: shift.flatMap((t) => t.arm()) });
  const reset = defineReaction("era.reset", { sequence: shift.flatMap((t) => t.disarm()) });
  const oneWay = defineReaction("gate.used", (on: TeleportEventParams) => ({
    sequence: [on.teleporter.disarm(), ...lamp(false)],
  }));
  return {
    reactions: [lampAtLoad, open, shut, cleared, reveal, timeShift, reset, oneWay],
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
