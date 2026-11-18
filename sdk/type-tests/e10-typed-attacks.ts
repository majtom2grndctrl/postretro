import { attackTurnFree, attackTurnHeld, defineAttack, defineEntity, defineStance } from "postretro";

const overhead = defineStance({ animation: "smash_windup", holdMs: [500, 1200] });
const slam = defineAttack({
  damage: 14,
  maxRange: 3.5,
  cooldownMs: 1800,
  beginningStance: overhead,
  release: { animation: "smash_strike", at: "hit" },
});
const jab = defineAttack("jab", { damage: 8, maxRange: 2, cooldownMs: 400 });

// Handles type-check everywhere an attack is meant.
defineEntity({
  components: {
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      attacks: [slam, jab],
      activities: {
        idle: { animation: "idle", motion: "hold" },
        swing: { animation: "slam", action: { attack: slam, at: "hit" }, attackTurn: slam },
      },
      transitions: {
        idle: [{ to: "swing", when: attackTurnFree(slam).and(attackTurnHeld(jab).not()) }],
        swing: [{ to: "idle" }],
      },
    },
  },
});

// A string where an attack handle is expected fails type-checking.
defineEntity({
  components: {
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      // @ts-expect-error The `attacks` list takes attack handles, not names.
      attacks: ["slam"],
      activities: { idle: { animation: "idle", motion: "hold" } },
      transitions: {},
    },
  },
});

defineEntity({
  components: {
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      // @ts-expect-error The retired name-keyed `attacks` map is not a list of handles.
      attacks: { slam: { damage: 14, maxRange: 3.5, cooldownMs: 1800 } },
      activities: { idle: { animation: "idle", motion: "hold" } },
      transitions: {},
    },
  },
});

defineEntity({
  components: {
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      activities: {
        // @ts-expect-error `action.attack` takes an attack handle, not its name.
        idle: { animation: "idle", action: { attack: "slam" } },
      },
      transitions: {},
    },
  },
});

defineEntity({
  components: {
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      activities: {
        // @ts-expect-error `attackTurn` takes an attack handle, not its name.
        idle: { animation: "idle", attackTurn: "slam" },
      },
      transitions: {},
    },
  },
});

// @ts-expect-error `attackTurnFree` takes an attack handle, not its name.
attackTurnFree("slam");

// @ts-expect-error `attackTurnHeld` takes an attack handle, not its name.
attackTurnHeld("slam");

// @ts-expect-error A stance handle is not an attack.
attackTurnHeld(overhead);

defineAttack({
  damage: 1,
  maxRange: 1,
  cooldownMs: 1,
  // @ts-expect-error `beginningStance` takes a stance handle, not its name.
  beginningStance: "overhead",
});

defineAttack({
  damage: 1,
  maxRange: 1,
  cooldownMs: 1,
  // @ts-expect-error `endingStance` takes a stance handle, not an attack.
  endingStance: jab,
});

// Rows take a stance or an attack handle as well as an activity name, and a
// level's `initial` may be a stance.
defineEntity({
  components: {
    behavior: {
      initial: overhead,
      moveSpeed: 3,
      activities: { approach: { animation: "walk" } },
      transitions: {
        approach: [{ to: overhead }],
        overhead: [{ to: slam }, { to: "approach" }],
      },
    },
  },
});

defineEntity({
  components: {
    behavior: {
      // @ts-expect-error `initial` takes an activity name or a stance, never an attack.
      initial: slam,
      moveSpeed: 3,
      activities: { approach: { animation: "walk" } },
      transitions: {},
    },
  },
});
