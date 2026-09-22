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
      panel: [0.01, 0.015, 0.020, 0.75],
      health: {
        background: [0.01, 0.015, 0.020, 0.25],
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
  bind: bindState(player.health, { format: "HP {}", decimalPlaces: 0 }),
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

const weaponLabel = Text({
  content: "--",
  color: color.hud.text,
  font: font.hud.status,
  fontSize: 18.0,
  bind: bindState(player.weapon.current),
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

// Health stands alone in the lower-left corner: just the numeric readout and the
// health bar, so the panel shrink-wraps to its content.
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
      [status, bar],
    ),
  ),
});

// The co-op seat line lives in its own top-left tree with no panel fill, so it
// takes no space (and shows no background) until a session roster fills it in.
export const openSeatsReadout = defineUiTree({
  name: "hud.openSeats",
  alwaysOn: true,
  tree: Tree({ anchor: "topLeft", offset: [24.0, 24.0] }, openSeats),
});

// XP sits centered along the bottom edge, away from the health and ammo groups.
export const xpReadout = defineUiTree({
  name: "hud.xp",
  alwaysOn: true,
  tree: Tree(
    { anchor: "bottom", offset: [0.0, -24.0] },
    VStack(
      {
        padding: spacing.hud.padding,
        align: "center",
        fill: color.hud.panel,
      },
      [xp],
    ),
  ),
});

// Ammo lives in the lower-right corner, headed by the current weapon's name.
export const ammoReadout = defineUiTree({
  name: "hud.ammo",
  alwaysOn: true,
  tree: Tree(
    { anchor: "bottomRight", offset: [-24.0, -24.0] },
    VStack(
      {
        gap: spacing.hud.rowGap,
        padding: spacing.hud.padding,
        align: "end",
        fill: color.hud.panel,
      },
      [
        weaponLabel,
        HStack({ gap: spacing.hud.gap, align: "center" }, [ammo, ammoReserve]),
      ],
    ),
  ),
});

// A single ring makes the changing fire-spread radius legible without a
// stationary aim mark competing at the same center point.
export const spreadReticle = defineUiTree({
  name: "hud.reticle",
  alwaysOn: true,
  tree: Tree(
    { anchor: "center", offset: [0.0, 0.0] },
    Ring({
      // Diameter must be at least twice `radiusRange.max` (below) so the fully
      // bloomed ring fits inside its own box.
      diameter: 200.0,
      radius: bindState(player.spread, {
        tween: {
          durationMs: 90.0,
          easing: "easeOut",
        },
      }),
      // Eight degrees is the rifle's full sustained-fire bloom. Map it from a
      // visible 4 px resting ring to a 200 px ring so sustained fire opens the
      // reticle dramatically.
      radiusRange: { inputMax: 8.0, min: 4.0, max: 100.0 },
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
