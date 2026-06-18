// Mod frontend hub fixture: catalog-driven level select and game-flow calls.
//
// This file is a committed TypeScript review gate. It is not imported by the
// dev mod at runtime; `content/dev/scripts/tsconfig.json` includes it so
// `tsc --noEmit -p content/dev/scripts/tsconfig.json` checks the authoring
// surface used by frontend mods.

import {
  defineMapCatalog,
  defineMod,
  defineReaction,
  type ModMapEntry,
} from "postretro";
import {
  Button,
  QUIT_TO_MENU_ACTION,
  Text,
  Tree,
  VStack,
  defineUiTree,
  loadLevel,
  restartLevel,
  returnToFrontend,
} from "postretro/ui";

const mapCatalog = defineMapCatalog([
  {
    id: "campaign-test",
    path: "maps/campaign-test.prl",
    name: "Campaign Test",
    tags: ["campaign", "recommended"],
  },
  {
    id: "combat-demo",
    path: "maps/combat-demo.prl",
    name: "Combat Demo",
    tags: ["campaign", "combat"],
  },
  {
    id: "arena-lights",
    path: "maps/arena-lights.prl",
    name: "Arena Lights",
    tags: ["lighting", "sandbox"],
  },
]);

function hasTag(tag: string): (entry: ModMapEntry) => boolean {
  return (entry) => entry.tags?.includes(tag) ?? false;
}

const campaignMaps = mapCatalog.filter(hasTag("campaign"));
const lightingMaps = mapCatalog.filter(hasTag("lighting"));

function levelButton(entry: ModMapEntry) {
  const startMap = defineReaction(`frontend.start.${entry.id}`, loadLevel(entry.id));
  return Button({
    id: `start-${entry.id}`,
    label: entry.name,
    onPress: startMap,
  });
}

const restartCurrentLevel = defineReaction("frontend.restartCurrentLevel", restartLevel());
const backToFrontend = defineReaction("frontend.returnToFrontend", returnToFrontend());

const levelSelect = defineUiTree({
  name: "frontend.levelSelect.fixture",
  tree: Tree(
    {
      anchor: "center",
      offset: [0, 0],
      captureMode: "capture",
      initialFocus: `start-${campaignMaps[0]?.id ?? mapCatalog[0].id}`,
      accessibleName: "Level select",
      role: "group",
    },
    VStack(
      { gap: 10, padding: 18, align: "stretch", focus: { policy: "linear", wrap: true } },
      [
        Text({ content: "Campaign" }),
        ...campaignMaps.map(levelButton),
        Text({ content: "Lighting" }),
        ...lightingMaps.map(levelButton),
        Button({
          id: "restart-current-level",
          label: "Restart Current Level",
          onPress: restartCurrentLevel,
        }),
        Button({
          id: "return-to-frontend",
          label: "Return To Menu",
          onPress: backToFrontend,
        }),
        Button({
          id: "quit-to-menu-action",
          label: "Quit To Menu Action",
          onPress: QUIT_TO_MENU_ACTION,
        }),
      ],
    ),
  ),
});

export default defineMod({
  name: "frontend-level-select-fixture",
  maps: mapCatalog,
  frontend: {
    menuTree: levelSelect.name,
    backgroundLevel: "arena-lights",
    camera: {
      position: [0, 1.7, -4],
      yaw: 0,
      pitch: 0,
    },
  },
  uiTrees: [levelSelect],
  reactions: [
    restartCurrentLevel,
    backToFrontend,
    ...mapCatalog.map((entry) =>
      defineReaction(`frontend.catalog.${entry.id}`, loadLevel(entry.id)),
    ),
  ],
});
