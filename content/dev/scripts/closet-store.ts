// Store-only module for the closet-reveal set piece (E18 Task 6). Split out
// from closet-reveal.ts so the slot declaration can be imported by both the
// level script and the mod manifest (`start-script.ts`) without importing the
// level script itself into the mod bundle — that would evaluate its reactions
// in both bundles. `LevelManifest` has no `stores` key: a store's
// `network: "shared"` slot only becomes a `ReplicationScope::SharedGlobal`
// slot when its handle is registered through `defineMod({ stores: [...] })`.
// Precedent: content/dev/scripts/run-counter.ts.

import { defineStore } from "postretro";

// `alarm` replicates to every connected client; each client's crossing
// watcher (declared in closet-reveal.ts) turns the write into a local
// `setLightAnimation` cue on the closet_alarm light.
export const closetStore = defineStore("closetReveal", {
  alarm: { type: "number", default: 0, network: "shared" },
});
