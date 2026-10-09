// DEV FIXTURE: mod loading screens. Each catalog map's entry names its own tree,
// which draws a screenshot of that map full-window behind the level name and a
// bar bound to the engine's load progress. Path loads with no catalog entry fall
// back to the mod-wide pool, a plain tree with no imagery.
import {
  Bar,
  Text,
  Tree,
  VStack,
  bindState,
  defineUiTree,
  getDesignTokens,
  getGameState,
} from "postretro/ui";
import { hudTheme } from "./hud";

/// Catalog maps with a loading screenshot. Each shot is a capture scene in
/// `ui/loading/scenes/`; re-run it after editing the map.
const SCREENSHOT_MAPS = [
  "campaign-test",
  "kinematic-platform",
  "movement-feel",
  "stress-warren-hallway-inspection",
  "combat-demo",
] as const;

type ScreenshotMap = (typeof SCREENSHOT_MAPS)[number];

type Rgba = [number, number, number, number];

const COLOR_TRACK: Rgba = [0.04, 0.05, 0.07, 1.0];
const COLOR_NEON: Rgba = [0.1, 0.8, 0.9, 1.0];
const COLOR_SCRIM: Rgba = [0.01, 0.015, 0.02, 0.8];

const PANEL_WIDTH = 440;
const PANEL_PADDING = 16;

const { loading } = getGameState();
const { color, font } = getDesignTokens(hudTheme);

function imageKey(mapId: ScreenshotMap): string {
  return `dev/loading/${mapId}`;
}

/// Image registry keys this module draws; `start-script.ts` maps them to PNGs.
export const loadingImages = Object.fromEntries(
  SCREENSHOT_MAPS.map((mapId) => [imageKey(mapId), `ui/loading/${mapId}.png`]),
);

/// The loading tree a catalog entry names for `mapId`.
export function loadingTreeName(mapId: ScreenshotMap): string {
  return `dev.loading.${mapId}`;
}

/// The level name and progress bar on a dark panel in the bottom-left corner,
/// over the map's screenshot when it has one.
function loadingTree(name: string, background?: string) {
  return defineUiTree({
    name,
    tree: Tree(
      {
        anchor: "bottomLeft",
        offset: [32.0, -32.0],
        ...(background ? { background: { image: background } } : {}),
      },
      VStack({ gap: 10, padding: PANEL_PADDING, width: PANEL_WIDTH, fill: COLOR_SCRIM }, [
        Text({ content: "LOADING", fontSize: 14, color: COLOR_NEON, font: font.mono }),
        Text({ content: "", fontSize: 28, color: color.hud.text, font: font.mono, bind: loading.levelName }),
        Bar({
          bind: bindState(loading.progress, { tween: { durationMs: 200.0, easing: "easeOut" } }),
          max: 1,
          fill: COLOR_NEON,
          background: COLOR_TRACK,
          width: PANEL_WIDTH - 2 * PANEL_PADDING,
          height: 8,
        }),
      ]),
    ),
  });
}

/// One tree per screenshot map.
export const mapLoadingTrees = SCREENSHOT_MAPS.map((mapId) =>
  loadingTree(loadingTreeName(mapId), imageKey(mapId)),
);

/// The fallback for loads with no catalog entry.
export const loadingPlain = loadingTree("dev.loading.plain");

/// The mod-wide pool (`loading.tree`).
export const loadingPool = [loadingPlain.name];
