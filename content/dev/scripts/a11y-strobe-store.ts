// Store behind `a11y-strobe-test.ts`'s UI-panel strobes. Declared at mod scope
// because level data scripts cannot declare stores; nothing else reads it.

import { defineStore } from "postretro";

export const a11yStrobeStore = defineStore("a11yStrobe", {
  smallPanel: { type: "boolean", default: false },
  largePanel: { type: "boolean", default: false },
});
