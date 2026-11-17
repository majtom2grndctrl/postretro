// Mod frontend hub fixture: catalog-driven level select, game-flow calls and
// the session membership verbs.
//
// This file is a committed TypeScript review gate. It is not imported by the
// dev mod at runtime; `content/dev/scripts/tsconfig.json` includes it so
// `tsc --noEmit -p content/dev/scripts/tsconfig.json` checks the authoring
// surface used by frontend mods.

import {
  defineMapCatalog,
  defineMod,
  defineReaction,
  getGameState,
  type ModMapEntry,
} from "postretro";
import {
  Button,
  QUIT_TO_MENU_ACTION,
  Text,
  Tree,
  VStack,
  bindState,
  defineUiTree,
  endSession,
  hostSession,
  joinSession,
  leaveSession,
  loadLevel,
  restartLevel,
  returnToFrontend,
  stateEquals,
} from "postretro/ui";

const { session } = getGameState();

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

// A client's "Leave game" button.
export const leave = defineReaction("pause.leaveSession", leaveSession());
// A run's end on a map-end crossing: every role runs the same steps.
export const runEnd = defineReaction("run.end", [endSession(), returnToFrontend()]);
// Host and Join buttons. Each reads session.password; joinSession also reads
// session.joinAddress.
export const host = defineReaction("frontend.host", hostSession());
export const join = defineReaction("frontend.join", joinSession());

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
        Button({
          id: "leave-session",
          label: "Leave Game",
          onPress: leave,
        }),
        // A password field and a refusal notice.
        Text({ content: "", mask: true, bind: bindState(session.password) }),
        Text({ content: "WRONG PASSWORD", visibleWhen: stateEquals(session.endCause, "refusedPassword") }),
      ],
    ),
  ),
});

export default defineMod({
  name: "frontend-level-select-fixture",
  id: "postretro.frontend-level-select-fixture",
  version: "dev",
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
    leave,
    runEnd,
    host,
    join,
    ...mapCatalog.map((entry) =>
      defineReaction(`frontend.catalog.${entry.id}`, loadLevel(entry.id)),
    ),
  ],
});
