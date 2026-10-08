import {
  defineReaction,
  getMapEntities,
  type TriggerEventParams,
} from "postretro";

const damagePresser = defineReaction("fixture.presser.damage", (on: TriggerEventParams) =>
  on.activators.damage(25),
);
const disarmPlate = defineReaction("fixture.presser.disarm", (on: TriggerEventParams) => ({
  sequence: on.trigger.disarm(),
}));

export function setupLevel() {
  return {
    reactions: [damagePresser, disarmPlate],
    triggerEvents: getMapEntities("trigger", { tag: "fixture_presser" }).map((plate) =>
      plate.on("enter", [damagePresser, disarmPlate]),
    ),
  };
}
