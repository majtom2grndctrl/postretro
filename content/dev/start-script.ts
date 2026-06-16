import { playerEntity } from "./scripts/player";
import { referencePistolEntity } from "./scripts/reference-pistol";
import { animDemoGruntEntity } from "./scripts/anim-demo-grunt";
import { targetDummyEntity } from "./scripts/target-dummy";
import { referenceEntities } from "../../sdk/behaviors/reference/entities";
import { introStore } from "./scripts/intro-store";
import { pauseMenuStore } from "./scripts/pause-menu-store";

export function setupMod() {
  return {
    name: "dev",
    // Store declarations commit only after this manifest validates. The shared
    // modules remain pure: importing them does not touch engine state.
    stores: [
      introStore.declaration,
      pauseMenuStore.declaration,
    ],
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
