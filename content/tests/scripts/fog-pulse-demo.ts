import {
  fogPulse,
  type NamedReactionDescriptor,
  registerReaction,
  world,
} from "postretro";

export function registerLevelManifest(_ctx: unknown) {
  // Demo: every fog volume tagged "pulse-fog" gets a density pulse animation
  // built via the `fogPulse` constructor. Mirrors `arena-lights.ts`'s shape:
  // a tag-filtered `world.query`, a step array, and a `registerReaction`
  // wrapper.
  const reactions: NamedReactionDescriptor[] = [];

  const fogs = world.query({ component: "fog_volume", tag: "pulse-fog" });
  if (fogs.length > 0) {
    for (const fog of fogs) {
      const steps = fogPulse(fog.id, 0.2, 1.0, 2000);
      reactions.push(registerReaction("levelLoad", { sequence: steps }));
    }
  }

  return { reactions };
}
