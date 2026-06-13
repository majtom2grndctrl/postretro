import { playerEntity } from "./scripts/player";
import { referencePistolEntity } from "./scripts/reference-pistol";
import { animDemoGruntEntity } from "./scripts/anim-demo-grunt";
import { targetDummyEntity } from "./scripts/target-dummy";
import { referenceEntities } from "../../sdk/behaviors/reference/entities";
import { registerIntroStore } from "./scripts/intro-store";
import { registerPauseMenuStore } from "./scripts/pause-menu-store";

export function setupMod() {
  // Register the M13 Goal C demo `intro` store namespace before returning the
  // mod descriptor. `defineStore` is a `DefinitionOnly` side-effect call: it
  // reserves `intro.flashColor` so the static UI proxy's level-load write lands
  // and the demo HUD's flash panel binds. The Luau parity twin lives in
  // `./scripts/intro-store.luau` (a reference module, not a second active entry).
  registerIntroStore();

  // Register the M13 Goal F (Task 5) demo `audio.master` store namespace so the
  // engine pause menu's volume slider has a writable slot to bind: the slider
  // `setState`s `audio.master`, and the engine's App-side consumer applies the
  // amplitude (→ dB) to the audio main bus. Declared at mod init so the slot
  // exists before the first slider write.
  registerPauseMenuStore();

  return {
    name: "dev",
    entities: [
      playerEntity,
      referencePistolEntity,
      // DEMO: M10 skinned-animation grunt. Map-placeable via
      // `"classname" "anim_demo_grunt"`; see content/dev/maps/anim-demo.map.
      animDemoGruntEntity,
      // DEMO: M10 entity health + damage target. Map-placeable via
      // `"classname" "target_dummy"`; see content/dev/maps/combat-demo.map.
      targetDummyEntity,
      ...referenceEntities,
    ],
  };
}
