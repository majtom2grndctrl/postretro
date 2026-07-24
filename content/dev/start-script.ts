import { defineMod, defineTriggerPool } from "postretro";
import { playerEntity } from "./scripts/player";
import { referencePistolEntity } from "./scripts/reference-pistol";
import { animDemoGruntEntity } from "./scripts/anim-demo-grunt";
import { targetDummyEntity } from "./scripts/target-dummy";
import { cyberpunkWarriorEntity } from "./scripts/cyberpunk-warrior";
import { sciFiTrooperEntity } from "./scripts/sci-fi-trooper";
import { referenceEntities } from "../../sdk/behaviors/reference/entities";
import { hud, hudTheme, reloadMeterTree, reticle } from "./scripts/hud";
import { pauseMenu } from "./scripts/pause-menu";
import { frontendMenu, frontendReactions, mapCatalog } from "./scripts/frontend-menu";
import {
  combatDummyLifecycle,
  combatZombieLifecycle,
} from "./scripts/combat-lifecycle";

export default defineMod({
  name: "dev",
  maps: mapCatalog,
  // Non-default on purpose: the dev mod dogfoods the mod-authored bloom surface,
  // so a regression in the profile wire is visible on every dev launch.
  render: {
    bloom: {
      resolution: "quarter",
      pixelated: true,
    },
  },
  frontend: {
    menuTree: frontendMenu.name,
    backgroundLevel: "combat-demo",
    camera: {
      position: [-3.25, 1.25, -0.25],
      yaw: 0.0,
      pitch: -0.08,
    },
  },
  uiTrees: [hud, reticle, reloadMeterTree, pauseMenu, frontendMenu],
  theme: hudTheme,
  reactions: frontendReactions,
  // The combat demo's unique target tags make these mod-global policies work
  // for both catalog and direct CLI map loads.
  events: [combatDummyLifecycle, combatZombieLifecycle],
  // Fixture-only mod-global tier: this composes on the tagged trap-pools map
  // while its level-local script owns the independent closet_trap count pool.
  triggerPools: [
    defineTriggerPool({
      tag: "ambush_trap",
      armPercentage: 50,
      levels: ["trap-pools"],
    }),
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
    // Character meshes for map-cast variety. Map-placeable via
    // `"classname" "cyberpunk_warrior_f"` / `"sci_fi_trooper_m"`; see
    // content/dev/maps/campaign-test.map.
    cyberpunkWarriorEntity,
    sciFiTrooperEntity,
    ...referenceEntities,
  ],
});
