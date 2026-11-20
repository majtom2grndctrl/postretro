// The dev mod's statuses: damage over time, or with `damage: 0` a marker
// other rules read. Each is named by its binding; register each in
// `defineMod({ statuses })`. Presentation reuses dev assets: the plasma-bolt
// sprite tinted orange for embers, fixture sounds for the ticks.
// Luau twin: statuses-demo.luau.
// See: docs/scripting-reference.md (Statuses)

import { defineStatus, emitter } from "postretro";
import { acid, fireDamage } from "./damage-types";

// Stacks to three per applier; each stack burns 4 fire every half second for
// three seconds from its last application.
export const burning = defineStatus({
  damage: 4,
  type: fireDamage,
  tickMs: 500,
  durationMs: 3000,
  stacking: "stack",
  maxStacks: 3,
  presentation: {
    emitter: emitter({
      sprite: "plasma_bolt",
      rate: 24,
      lifetime: 0.6,
      velocity: [0, 1.5, 0],
      spread: 0.4,
      buoyancy: 0.5,
      size_over_lifetime: [0.35, 0.1],
      opacity_over_lifetime: [1, 0],
      color: [1, 0.45, 0.1],
    }),
    tickSound: "fixtures/pellet_hit",
  },
});

// What lava applies (scripts/status-hazards.ts) every tick a body is in it:
// one tick lands a quarter second after leaving, then it is gone.
export const scalding = defineStatus({
  damage: 10,
  type: fireDamage,
  tickMs: 250,
  durationMs: 250,
  presentation: { tickSound: "fixtures/bite" },
});

// Until cleared: the `cure` reaction, or a respawn.
export const corroding = defineStatus({ damage: 2, type: acid, tickMs: 1000 });

// A marker: never deals damage. `ignite` will not light a wet target.
export const wet = defineStatus({ damage: 0, tickMs: 500, durationMs: 5000 });
