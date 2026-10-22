// E18 projectile-trap wave: the Scripting surface example as a level script.
// The sweep plate fires the `wave` traps in order, three passes; standing in
// the loop zone makes every trap repeat on one period, phase-staggered so the
// wave rolls. Its Luau twin is trap-wave.luau; both name every reaction so
// their wire matches.
// See: context/lib/scripting.md §12 (Entity addressing)

import { defineReaction, getMapEntities, wait } from "postretro";

const STEP_MS = 250; // gap between neighbouring traps
const PASSES = 3;
const GAP_MS = 1500; // pause between passes

export function setupLevel() {
  const traps = getMapEntities("trap", { tag: "wave" })
    .sort((a, b) => a.position.x - b.position.x);

  // Sweep: PASSES passes, each firing the traps in order. fire() runs one execution.
  const pass = traps.flatMap((t, i) => [...(i ? wait(STEP_MS) : []), ...t.fire()]);
  const sweep = defineReaction("wave.sweep", {
    sequence: Array.from({ length: PASSES }, (_, s) => [...(s ? wait(GAP_MS) : []), ...pass]).flat(),
  });

  // Looping wave: every trap repeats on one period; phase staggers them.
  const periodMs = traps.length * STEP_MS + GAP_MS;
  const loopOn = defineReaction("wave.loopOn", {
    sequence: traps.flatMap((t, i) => t.start({ intervalMs: periodMs, phaseMs: i * STEP_MS })),
  });
  const loopOff = defineReaction("wave.loopOff", { sequence: traps.flatMap((t) => t.stop()) });

  return {
    reactions: [sweep, loopOn, loopOff],
    triggerEvents: [
      ...getMapEntities("trigger", { tag: "sweep_plate" }).map((p) => p.on("enter", [sweep])),
      ...getMapEntities("trigger", { tag: "loop_zone" }).flatMap((z) => [
        z.on("enter", [loopOn]),
        z.on("exit", [loopOff]),
      ]),
    ],
  };
}
