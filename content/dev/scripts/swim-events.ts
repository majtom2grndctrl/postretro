// Swim reactions for the dev mod: a host decision per player when that player
// starts swimming, and a splash on each machine for its own player. The
// manifest wires them (start-script.ts `playerEvents` and `crossings`).
// Luau twin: fluids-swim.luau mirrors them, so every reaction is named —
// derived ids differ per runtime.
// See: context/lib/scripting.md §12 (Player events)

import { defineReaction, defineStore } from "postretro";
import type { PlayerEventParams } from "postretro";
import { playSound } from "postretro/ui";

// Host-local bookkeeping, one value per player: how many times that player has
// started swimming. No `network` and no `persist`, so no durable identity.
export const swimStats = defineStore("swimStats", {
  dives: { type: "number", default: 0, perOwner: true },
});

// Host decision, fired once per player as they start swimming.
export const markSwimming = defineReaction("swim.markSwimming", (on: PlayerEventParams) =>
  on.player.addSlot(swimStats.dives, 1),
);

// Own-state feedback: a local crossing plays it on the machine whose player
// started swimming, with no host round trip.
export const splashSound = defineReaction("swim.splash", playSound("fixtures/pellet_hit"));
