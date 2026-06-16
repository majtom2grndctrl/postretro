import {
  Bar,
  HStack,
  Text,
  Tree,
  VStack,
  bindState,
  getGameState,
} from "postretro";

export const hudTheme = {
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
};

export function buildHud() {
  const { player } = getGameState();

  const status = Text({
    content: "HP --",
    color: "hud.text",
    font: "hud.status",
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
    fill: "ok",
    background: "hud.health.background",
    styleRanges: {
      max: 1.0,
      entries: [
        { upTo: 0.25, color: "critical" },
        { upTo: 0.5, color: "warning" },
        { color: "ok" },
      ],
    },
  });

  const healthTree = Tree(
    { anchor: "bottomLeft", offset: [24.0, -24.0] },
    VStack(
      {
        gap: "hud.rowGap",
        padding: "hud.padding",
        align: "stretch",
        fill: "hud.panel",
      },
      [
        HStack({ gap: "hud.gap", align: "center" }, [status]),
        bar,
      ],
    ),
  );

  const reticleTree = Tree(
    { anchor: "center", offset: [0.0, 0.0] },
    Text({ content: "+", font: "mono" }),
  );

  return {
    uiTrees: [
      { name: "hud", tree: healthTree, alwaysOn: true },
      { name: "hud.reticle", tree: reticleTree, alwaysOn: true },
    ],
    theme: hudTheme,
  };
}
