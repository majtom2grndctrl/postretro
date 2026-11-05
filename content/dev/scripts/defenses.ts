// The dev mod's defense layer kinds: pools that drain before health. Each is
// named by its binding at build. `start-script.ts` registers both through
// `defineMod({ defenseLayers })`; descriptors attach them in
// `components.health.layers`.
// See: docs/scripting-reference.md (Defense layers and damage types)

import { defineDefenseLayer } from "postretro";

export const shield = defineDefenseLayer();
export const armor = defineDefenseLayer();
