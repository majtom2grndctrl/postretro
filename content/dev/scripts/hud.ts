import {
  Bar,
  HStack,
  Ring,
  Text,
  Tree,
  VStack,
  bindState,
  defineTheme,
  defineUiTree,
  getGameState,
  getDesignTokens,
  stateEquals,
} from "postretro/ui";
import { progression } from "./combat-lifecycle";

export const hudTheme = defineTheme({
  color: {
    hud: {
      panel: [0.018, 0.026, 0.039, 0.82],
      health: {
        background: [0.035, 0.045, 0.060, 1.0],
      },
      text: [0.82, 0.95, 0.98, 1.0],
    },
    critical: [0.86, 0.06, 0.12, 1.0],
    warning: [0.95, 0.62, 0.12, 1.0],
    ok: [0.12, 0.72, 0.40, 1.0],
  },
  font: {
    hud: {
      status: "JetBrains Mono",
    },
    primary: "JetBrains Mono",
    mono: "JetBrains Mono",
  },
  spacing: {
    hud: {
      gap: 8.0,
      padding: 14.0,
      rowGap: 6.0,
    },
  },
});

const { player, session } = getGameState();
const { color, font, spacing } = getDesignTokens(hudTheme);

const status = Text({
  content: "HP --",
  color: color.hud.text,
  font: font.hud.status,
  fontSize: 24.0,
  bind: bindState(player.health, { format: "HP {}" }),
});

const ammo = Text({
  content: "AMMO -- / --",
  color: color.hud.text,
  font: font.hud.status,
  fontSize: 24.0,
  bind: bindState(player.ammo, { format: "AMMO {}" }),
});

const ammoReserve = Text({
  content: "/ --",
  color: color.hud.text,
  font: font.hud.status,
  fontSize: 24.0,
  bind: bindState(player.ammoReserve, { format: "/ {}" }),
});

const xp = Text({
  content: "XP --",
  color: color.hud.text,
  font: font.hud.status,
  fontSize: 24.0,
  bind: bindState(progression.xp, { format: "XP {}" }),
});

const openSeats = Text({
  content: "",
  color: color.hud.text,
  font: font.hud.status,
  fontSize: 18.0,
  bind: bindState(session.openSeats, { format: "OPEN SEATS {}" }),
});

const bar = Bar({
  bind: bindState(player.health, {
    tween: {
      durationMs: 180.0,
      easing: "easeOut",
    },
  }),
  max: player.maxHealth,
  fill: color.ok,
  background: color.hud.health.background,
  styleRanges: {
    max: 1.0,
    entries: [
      { upTo: 0.25, color: color.critical },
      { upTo: 0.5, color: color.warning },
      { color: color.ok },
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
        gap: spacing.hud.rowGap,
        padding: spacing.hud.padding,
        align: "stretch",
        fill: color.hud.panel,
      },
      [
        HStack({ gap: spacing.hud.gap, align: "center" }, [status, ammo, ammoReserve, xp]),
        bar,
        openSeats,
      ],
    ),
  ),
});

export const reticle = defineUiTree({
  name: "hud.reticle",
  alwaysOn: true,
  tree: Tree(
    { anchor: "center", offset: [0.0, 0.0] },
    Ring({
      diameter: 28.0,
      radius: 10.0,
      thickness: 2.0,
      fill: color.hud.text,
    }),
  ),
});

const reloadMeter = Bar({
  bind: bindState(player.reloadProgress),
  max: 1.0,
  width: 120.0,
  height: 24.0,
  visibleWhen: stateEquals(player.reloadActive, true),
  exitFade: { durationMs: 500.0 },
  fill: color.ok,
  background: color.hud.health.background,
});

export const reloadMeterTree = defineUiTree({
  name: "hud.reloadMeter",
  alwaysOn: true,
  tree: Tree({ anchor: "center", offset: [0.0, 36.0] }, reloadMeter),
});
