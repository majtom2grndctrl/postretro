import {
  damage,
  defineReaction,
  disarmTrigger,
  onTriggerEvent,
  type TriggerEventParams,
} from "postretro";

const damagePresser = defineReaction("fixture.presser.damage", (on: TriggerEventParams) =>
  damage(on.activators, 25),
);
const disarmPlate = defineReaction("fixture.presser.disarm", (on: TriggerEventParams) => ({
  sequence: disarmTrigger(on.trigger),
}));

export function setupLevel() {
  return {
    reactions: [damagePresser, disarmPlate],
    triggerEvents: [
      onTriggerEvent({ tag: "fixture_presser" }, "enter", [damagePresser, disarmPlate]),
    ],
  };
}
