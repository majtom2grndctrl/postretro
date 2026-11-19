import type { ModManifest, SavesManifest } from "postretro";
import { defineMod, defineStore } from "postretro";

// `persist: "run"` is a global-slot scope; per-owner slots persist only in the character.
const meta = defineStore("meta", {
  episode2: { type: "boolean", default: false, persist: true },
});
const run = defineStore("run", {
  generatorOn: { type: "boolean", default: false, persist: "run" },
  power: { type: "number", default: 0, persist: "run" },
});
const hero = defineStore("hero", {
  xp: { type: "number", default: 0, perOwner: true, persist: true },
});

const manifest: ModManifest = defineMod({
  name: "Saves",
  id: "acme.saves",
  version: "1",
  saves: { characterSlots: 4, runSlots: 3 },
  stores: [meta, run, hero],
});

const runXp = defineStore("bad", {
  // @ts-expect-error Per-owner run scope is refused: a host never saves a guest's run state.
  xp: { type: "number", default: 0, perOwner: true, persist: "run" },
});

// @ts-expect-error A saves block declares both counts.
const missingRuns: SavesManifest = { characterSlots: 2 };

// @ts-expect-error Slot counts are numbers.
const textCount: SavesManifest = { characterSlots: "2", runSlots: 1 };

export { manifest, missingRuns, runXp, textCount };
