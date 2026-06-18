import { defineMod } from "postretro";
import { playerEntity } from "./scripts/player";
import { referencePistolEntity } from "./scripts/reference-pistol";
import { animDemoGruntEntity } from "./scripts/anim-demo-grunt";
import { targetDummyEntity } from "./scripts/target-dummy";
import { referenceEntities } from "../../sdk/behaviors/reference/entities";
import { hud, hudTheme, reticle } from "./scripts/hud";
import { pauseMenu } from "./scripts/pause-menu";

export default defineMod({
  name: "dev",
  uiTrees: [hud, reticle, pauseMenu],
  theme: hudTheme,
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
});
