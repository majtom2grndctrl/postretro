import type { ModManifest, ModMapEntry } from "postretro";
import { defineMod } from "postretro";
import { Bar, getGameState, Image, Text, VStack } from "postretro/ui";

// Mod imagery, a mod-wide loading pool, and a per-map override.
const manifest: ModManifest = defineMod({
  name: "Neon",
  id: "acme.neon",
  version: "1",
  uiImages: { "loading/skyline": "ui/loading/skyline.png" },
  loading: { tree: ["loadingSkyline", "loadingAlley"] },
  maps: [
    { id: "e1m1", path: "maps/e1m1.prl", name: "Entryway", loadingTree: "loadingEntry" },
    { id: "e1m2", path: "maps/e1m2.prl", name: "Docks", loadingTree: ["loadingA", "loadingB"] },
  ],
});

// @ts-expect-error A loading pool holds tree names only.
const badPool: ModMapEntry = { id: "x", path: "maps/x.prl", name: "X", loadingTree: [1] };

const state = getGameState();
const screen = VStack({ align: "center" }, [
  Image({ asset: "loading/skyline", width: 640, decorative: true }),
  Image({ asset: "loading/skyline", width: 64, height: 64, label: "Skyline" }),
  Text({ content: "Loading", bind: state.loading.levelName }),
  Bar({ bind: state.loading.progress, max: 1, fill: [1, 1, 1, 1], background: [0, 0, 0, 1] }),
]);

// @ts-expect-error `width` is a number of logical-reference px.
const badWidth = Image({ asset: "loading/skyline", width: "640", decorative: true });

export { badPool, badWidth, manifest, screen };
