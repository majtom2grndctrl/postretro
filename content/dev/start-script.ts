import { playerEntity } from "./scripts/player";
import { referencePistolEntity } from "./scripts/reference-pistol";
import { animDemoGruntEntity } from "./scripts/anim-demo-grunt";
import { targetDummyEntity } from "./scripts/target-dummy";
import { referenceEntities } from "../../sdk/behaviors/reference/entities";
import { buildHud } from "./scripts/hud";
import { pauseMenuStore } from "./scripts/pause-menu-store";

export function setupMod() {
  const hud = buildHud();

  return {
    name: "dev",
    // Store declarations commit only after this manifest validates. The shared
    // modules remain pure: importing them does not touch engine state.
    stores: [
      pauseMenuStore.declaration,
    ],
    uiTrees: hud.uiTrees,
    theme: hud.theme,
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
