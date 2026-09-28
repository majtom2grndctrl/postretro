// Restart-loop fixture for `a11y-restart-loop.map`: every load holds gameplay
// for 1.5 s, then restarts the level, so a gameplay → loading → gameplay cycle
// repeats until the player quits. Judges the limiter across load cycles on
// hardware (X1). The map sits outside the catalog; no player payload ships it.
// See: context/lib/rendering_pipeline.md §7.8

import { type NamedReactionDescriptor, defineReaction, fire, wait } from "postretro";
import { restartLevel } from "postretro/ui";

export function setupLevel(_ctx: unknown): { reactions: NamedReactionDescriptor[] } {
  const restart = defineReaction("a11y.restartLoop.restart", restartLevel());
  return {
    reactions: [
      restart,
      defineReaction("levelLoad", { sequence: [...wait(1500), ...fire(restart)] }),
    ],
  };
}
