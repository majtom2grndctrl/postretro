// Shared store declaration for the closet-reveal set piece.
// Separate from level reactions so the mod manifest can register it safely.

import { defineStore } from "postretro";

// `alarm` replicates to every connected client; each client's crossing
// watcher (declared in closet-reveal.ts) turns the write into a local
// `setLightAnimation` cue on the closet_alarm light.
export const closetStore = defineStore("closetReveal", {
  alarm: { type: "number", default: 0, network: "shared" },
});
