import { playerEntity } from "./scripts/player";
import { referencePistolEntity } from "./scripts/reference-pistol";
import { referenceEntities } from "../../sdk/behaviors/reference/entities";
import { registerIntroStore } from "./scripts/intro-store";

export function setupMod() {
  // Register the M13 Goal C demo `intro` store namespace before returning the
  // mod descriptor. `defineStore` is a `DefinitionOnly` side-effect call: it
  // reserves `intro.flashColor` so the static UI proxy's level-load write lands
  // and the demo HUD's flash panel binds. The Luau parity twin lives in
  // `./scripts/intro-store.luau` (a reference module, not a second active entry).
  registerIntroStore();

  return {
    name: "dev",
    entities: [playerEntity, referencePistolEntity, ...referenceEntities],
  };
}
