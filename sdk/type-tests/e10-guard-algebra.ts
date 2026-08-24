import { brain, defineEntity, runtime, state } from "postretro";

const fluentGuard = brain.targetDistance
  .le(2)
  .and(brain.targetHostile)
  .or(state("stunned").eq(1).not())
  .and(brain.timeInActivityMs.between(0, 500));

const directGuard = runtime.and(
  brain.hasTarget,
  runtime.and(brain.targetVisible, runtime.not(brain.targetDied)),
);

defineEntity({
  components: {
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      activities: { idle: { animation: "idle", motion: "hold" } },
      transitions: { idle: [{ when: fluentGuard, to: "idle" }] },
    },
  },
});

const nativeNot = !brain.targetHostile;
defineEntity({
  components: {
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      activities: { idle: { animation: "idle", motion: "hold" } },
      transitions: {
        idle: [{
          // @ts-expect-error Guard positions accept RuntimeValue, not a native boolean.
          when: nativeNot,
          to: "idle",
        }],
      },
    },
  },
});

void directGuard;
