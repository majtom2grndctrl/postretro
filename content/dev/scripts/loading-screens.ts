// DEV FIXTURE: mod loading screens. The mod-wide pool picks one of the first
// two trees at random per load; the combat demo's catalog entry overrides it
// with the third. Every tree shows mod imagery from `uiImages`, the loading
// level's name, and a bar bound to the engine's load progress.
import {
  Bar,
  HStack,
  Image,
  Text,
  Tree,
  VStack,
  bindState,
  defineUiTree,
  getDesignTokens,
  getGameState,
} from "postretro/ui";
import { hudTheme } from "./hud";

/// Image registry keys this module draws; `start-script.ts` maps them to PNGs.
export const loadingImages = {
  "dev/loading/hazard": "textures/Level Eleven Games Sci-Fi Texture Pack v1/ConcreteFloor-Hazard-Full-01_64.png",
  "dev/loading/neon": "textures/neon/neon_glow_panel.png",
};

type Rgba = [number, number, number, number];

const COLOR_TRACK: Rgba = [0.04, 0.05, 0.07, 1.0];
const COLOR_HAZARD: Rgba = [0.85, 0.52, 0.05, 1.0];
const COLOR_NEON: Rgba = [0.10, 0.80, 0.90, 1.0];

const { loading } = getGameState();
const { color, font } = getDesignTokens(hudTheme);

function progressBar(fill: Rgba | typeof color.ok, width: number) {
  return Bar({
    bind: bindState(loading.progress, { tween: { durationMs: 200.0, easing: "easeOut" } }),
    max: 1,
    fill,
    background: COLOR_TRACK,
    width,
    height: 8,
  });
}

function levelName(fontSize: number) {
  return Text({ content: "", fontSize, color: color.hud.text, font: font.mono, bind: loading.levelName });
}

/// A hazard stripe plate over the level name and an amber bar.
export const loadingHazard = defineUiTree({
  name: "dev.loading.hazard",
  tree: Tree(
    { anchor: "center", offset: [0.0, 0.0] },
    VStack({ gap: 16, align: "center" }, [
      Image({ asset: "dev/loading/hazard", width: 160, decorative: true }),
      Text({ content: "LOADING", fontSize: 14, color: color.hud.text, font: font.mono }),
      levelName(28),
      progressBar(COLOR_HAZARD, 400),
    ]),
  ),
});

/// Neon panels flanking the level name, a cyan bar beneath.
export const loadingNeon = defineUiTree({
  name: "dev.loading.neon",
  tree: Tree(
    { anchor: "bottom", offset: [0.0, -64.0] },
    VStack({ gap: 12, align: "center" }, [
      HStack({ gap: 24, align: "center" }, [
        Image({ asset: "dev/loading/neon", width: 96, decorative: true }),
        levelName(32),
        Image({ asset: "dev/loading/neon", width: 96, decorative: true }),
      ]),
      progressBar(COLOR_NEON, 640),
    ]),
  ),
});

/// The combat demo's own screen: both images, a wide bar.
export const loadingCombatDemo = defineUiTree({
  name: "dev.loading.combatDemo",
  tree: Tree(
    { anchor: "center", offset: [0.0, 0.0] },
    VStack({ gap: 20, align: "center" }, [
      HStack({ gap: 8, align: "center" }, [
        Image({ asset: "dev/loading/hazard", width: 64, height: 64, decorative: true }),
        Image({ asset: "dev/loading/neon", width: 64, height: 64, decorative: true }),
        Image({ asset: "dev/loading/hazard", width: 64, height: 64, decorative: true }),
      ]),
      Text({ content: "COMBAT DEMO", fontSize: 14, color: COLOR_HAZARD, font: font.mono }),
      levelName(24),
      progressBar(color.ok, 720),
    ]),
  ),
});

/// The mod-wide pool (`loading.tree`).
export const loadingPool = [loadingHazard.name, loadingNeon.name];
