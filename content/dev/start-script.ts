import { defineFaction, defineMod, defineTriggerPool, sentiment } from "postretro";
import { playerEntity } from "./scripts/player";
import { referencePistolEntity } from "./scripts/reference-pistol";
import { referenceRifleEntity } from "./scripts/reference-rifle";
import { referenceShotgunEntity } from "./scripts/reference-shotgun";
import {
  referencePlasmaBoltEntity,
  referenceRocketEntity,
} from "./scripts/reference-projectiles";
import {
  wieldableFixtureAutoEntity,
  wieldableFixturePressEntity,
} from "./scripts/wieldable-fixture-weapons";
import { animDemoGruntEntity } from "./scripts/anim-demo-grunt";
import { targetDummyEntity } from "./scripts/target-dummy";
import { cyberpunkWarriorEntity } from "./scripts/cyberpunk-warrior";
import { sciFiTrooperEntity } from "./scripts/sci-fi-trooper";
import { referenceEnemyEntity } from "./scripts/reference-enemy";
import { enemyRifleEntity } from "./scripts/enemy-rifle";
import { limitatorEntity } from "./scripts/limitator";
import {
  CROSSFIRE_RAIDERS_FACTION,
  CROSSFIRE_SENTINELS_FACTION,
  MAX_RETALIATION_TOLERANCE,
  RAIDER_TOLERANCE,
  crossfireRaiderEntity,
  crossfireSentinelEntity,
} from "./scripts/faction-crossfire";
import { referenceEntities } from "../../sdk/behaviors/reference/entities";
import {
  hud,
  hudTheme,
  reloadMeterTree,
  spreadReticle,
} from "./scripts/hud";
import { pauseMenu } from "./scripts/pause-menu";
import { frontendMenu, frontendReactions, mapCatalog } from "./scripts/frontend-menu";
import {
  ammoOnKill,
  combatDummyLifecycle,
  combatZombieLifecycle,
  enemyDeath,
  progression,
} from "./scripts/combat-lifecycle";
import { runCounter } from "./scripts/run-counter";
import { closetStore } from "./scripts/closet-store";
import {
  damagedEnemyBar,
  damagedEnemyOverlay,
  damageNumber,
} from "./scripts/combat-presentation";

export default defineMod({
  name: "dev",
  id: "postretro.dev",
  version: "dev",
  switching: {
    commitOnDirectSelect: true,
    cycleCommitDwellMs: 128,
    blockDuringReload: false,
  },
  maps: mapCatalog,
  // Dogfoods the mod-authored bloom surface on every dev launch. `resolution`
  // is left at the engine default (`half`); `pixelated: true` is the non-default
  // field, so a regression in the pixelated wire is visible here. (Set
  // `resolution` to `quarter`/`eighth` to also exercise the resolution wire.)
  render: {
    bloom: {
      resolution: "quarter",
      pixelated: false,
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
  uiTrees: [hud, spreadReticle, reloadMeterTree, pauseMenu, frontendMenu],
  // DEV FIXTURE — these remain global so any dev map using the shared combat
  // policies exposes floating damage and recently-damaged enemy feedback.
  presentationTemplates: [damageNumber, damagedEnemyBar],
  presentationOverlays: damagedEnemyOverlay,
  theme: hudTheme,
  reactions: frontendReactions,
  // The combat demo's unique target tags make these mod-global policies work
  // for both catalog and direct CLI map loads. `enemyDeath` must precede its
  // `combatZombieLifecycle` override: registration order is iteration order, and
  // an override registered before its base is dropped as targeting an unknown
  // event.
  events: [
    combatDummyLifecycle,
    enemyDeath,
    ammoOnKill,
    combatZombieLifecycle,
  ],
  stores: [runCounter, progression, closetStore],
  // Fixture-only mod-global tier: this composes on the tagged trap-pools map
  // while its level-local script owns the independent closet_trap count pool.
  triggerPools: [
    defineTriggerPool({
      tag: "ambush_trap",
      armPercentage: 50,
      levels: ["trap-pools"],
    }),
  ],
  // Reference relationship content for the two crossfire archetypes below.
  // Same-raider hits are neutral until damage exceeds the raider's local
  // tolerance; sentinel relations are directional hostile defaults so maps can
  // also stage cross-faction provokers. The per-archetype values in the entity
  // module deliberately win over these pair fallbacks.
  factions: [
    defineFaction(CROSSFIRE_RAIDERS_FACTION),
    defineFaction(CROSSFIRE_SENTINELS_FACTION),
  ],
  sentiment: [
    sentiment(CROSSFIRE_RAIDERS_FACTION, CROSSFIRE_RAIDERS_FACTION, {
      sentiment: 0,
      tolerance: RAIDER_TOLERANCE,
    }),
    sentiment(CROSSFIRE_RAIDERS_FACTION, CROSSFIRE_SENTINELS_FACTION, {
      sentiment: -1,
      tolerance: RAIDER_TOLERANCE,
    }),
    sentiment(CROSSFIRE_SENTINELS_FACTION, CROSSFIRE_RAIDERS_FACTION, {
      sentiment: -1,
      tolerance: MAX_RETALIATION_TOLERANCE,
    }),
    sentiment(CROSSFIRE_SENTINELS_FACTION, CROSSFIRE_SENTINELS_FACTION, {
      sentiment: 0,
      tolerance: MAX_RETALIATION_TOLERANCE,
    }),
  ],
  entities: [
    playerEntity,
    referencePistolEntity,
    referenceRifleEntity,
    referenceShotgunEntity,
    referencePlasmaBoltEntity,
    referenceRocketEntity,
    wieldableFixtureAutoEntity,
    wieldableFixturePressEntity,
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
    referenceEnemyEntity,
    enemyRifleEntity,
    limitatorEntity,
    // Crossfire reference controls. Place `crossfire_raider` with either a
    // raider or sentinel peer to observe retaliation; `crossfire_sentinel`
    // remains on its player target under the same incoming damage.
    crossfireRaiderEntity,
    crossfireSentinelEntity,
    ...referenceEntities,
  ],
});
