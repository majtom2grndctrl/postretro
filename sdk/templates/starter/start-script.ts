// The starter mod: the smallest entry script the engine accepts.
//
// The SDK bundle publishes this file at `content/base/start-script.ts`, so
// `bin/postretro-tool run` boots to the engine's fallback menu with no content
// at all. Replace the three fields with your game's own, then grow the
// manifest from here — `docs/modding.md` and `docs/scripting-reference.md`
// cover what it can declare. A map's source goes in the `maps/` folder. An
// entry in the manifest's `maps` catalog ships it in a distribution and lets
// a menu of your own offer it; the engine's fallback menu lists no levels, so
// until you build one, open a level with `bin/postretro-tool run maps/<name>.prl`.
import { defineMod } from "postretro";

export default defineMod({
  name: "Starter",
  id: "starter",
  version: "0.1.0",
});
