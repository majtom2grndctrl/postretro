// Shared store declaration for the player-events example: each player's
// level, and the XP the most recent level-up happened at.
// Separate from the level script so the mod manifest can register it.

import { defineStore } from "postretro";

// XP is the dev mod's per-owner `progression.xp`, credited per kill
// (`combat-lifecycle.ts`). `level` belongs to its player; `lastLevelUpXp` is
// one shared value every client sees.
export const leveling = defineStore("leveling", {
  level: { type: "number", default: 1, perOwner: true, network: "ownerPrivate" },
  lastLevelUpXp: { type: "number", default: 0, network: "shared" },
});
