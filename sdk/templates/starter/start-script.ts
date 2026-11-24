// The starter mod: the smallest entry script the engine accepts.
//
// The SDK bundle publishes this file at `content/base/start-script.ts`, so
// `bin/postretro-tool run` boots to the engine's fallback menu with no content
// at all. Replace the three fields with your game's own, then grow the
// manifest from here — `docs/modding.md` and `docs/scripting-reference.md`
// cover what it can declare. A map goes in `maps/`, and a catalog entry in
// `maps` is what offers it in the menu and ships it in a distribution.
import { defineMod } from "postretro";

export default defineMod({
  name: "Starter",
  id: "starter",
  version: "0.1.0",
});
