import { playerEntity } from "./scripts/player";
import { referencePistolEntity } from "./scripts/reference-pistol";
import { animDemoGruntEntity } from "./scripts/anim-demo-grunt";
import { targetDummyEntity } from "./scripts/target-dummy";
import { referenceEntities } from "../../sdk/behaviors/reference/entities";
import { buildHud } from "./scripts/hud";
import { buildPauseMenu, PAUSE_MENU_TREE } from "./scripts/pause-menu";

export function setupMod() {
  const hud = buildHud();
  const pauseMenu = buildPauseMenu();

  return {
    name: "dev",
    uiTrees: [
      ...hud.uiTrees,
      { name: PAUSE_MENU_TREE, tree: pauseMenu, alwaysOn: false },
    ],
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
