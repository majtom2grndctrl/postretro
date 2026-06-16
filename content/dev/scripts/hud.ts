import {
  Bar,
  HStack,
  Text,
  Tree,
  VStack,
  bindState,
  defineTheme,
  defineUiTree,
  getGameState,
} from "postretro";

export const hudTheme = defineTheme({
  colors: {
    "hud.panel": [0.018, 0.026, 0.039, 0.82],
    "hud.health.background": [0.035, 0.045, 0.060, 1.0],
    "hud.text": [0.82, 0.95, 0.98, 1.0],
    critical: [0.86, 0.06, 0.12, 1.0],
    warning: [0.95, 0.62, 0.12, 1.0],
    ok: [0.12, 0.72, 0.40, 1.0],
  },
  fonts: {
    "hud.status": "JetBrains Mono",
    mono: "JetBrains Mono",
  },
  spacing: {
    "hud.gap": 8.0,
    "hud.padding": 14.0,
    "hud.rowGap": 6.0,
  },
});

const { player } = getGameState();
const { color, font, spacing } = hudTheme.tokens;

const status = Text({
  content: "HP --",
  color: color("hud.text"),
  font: font("hud.status"),
  fontSize: 24.0,
  bind: bindState(player.health, { format: "HP {}" }),
});

const bar = Bar({
  bind: bindState(player.health, {
    tween: {
      durationMs: 180.0,
      easing: "easeOut",
    },
  }),
  max: player.maxHealth,
  fill: color("ok"),
  background: color("hud.health.background"),
  styleRanges: {
    max: 1.0,
    entries: [
      { upTo: 0.25, color: color("critical") },
      { upTo: 0.5, color: color("warning") },
      { color: color("ok") },
    ],
  },
});

export const hud = defineUiTree({
  name: "hud",
  alwaysOn: true,
  tree: Tree(
    { anchor: "bottomLeft", offset: [24.0, -24.0] },
    VStack(
      {
        gap: spacing("hud.rowGap"),
        padding: spacing("hud.padding"),
        align: "stretch",
        fill: color("hud.panel"),
      },
      [
        HStack({ gap: spacing("hud.gap"), align: "center" }, [status]),
        bar,
      ],
    ),
  ),
});

export const reticle = defineUiTree({
  name: "hud.reticle",
  alwaysOn: true,
  tree: Tree(
    { anchor: "center", offset: [0.0, 0.0] },
    Text({ content: "+", font: font("mono") }),
  ),
});
