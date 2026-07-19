import { defineReaction, world } from "postretro";

export function setupLevel(_ctx: unknown) {
  const lights = world.query({ component: "light", tag: "script_wave" });
  return {
    reactions: [
      defineReaction("levelLoad", {
        sequence: lights.flatMap((light) =>
          light.pulse({ min: 0.2, max: 1.0, periodMs: 1000 }),
        ),
      }),
    ],
  };
}
