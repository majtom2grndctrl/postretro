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
  stateCompare,
  stateEquals,
} from "postretro/ui";
import { progression } from "./combat-lifecycle";
import { shield } from "./defenses";
import { BLEED_OUT_MS } from "./player-death";

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

// Each meter sits under a small caption naming it; the bars carry no number.
const METER_WIDTH = 200.0;
const METER_HEIGHT = 12.0;

const meterLabel = (content: string) =>
  Text({
    content,
    color: color.hud.text,
    font: font.hud.status,
    fontSize: 13.0,
  });

const bar = Bar({
  bind: bindState(player.health, {
    tween: {
      durationMs: 180.0,
      easing: "easeOut",
    },
  }),
  max: player.maxHealth,
  width: METER_WIDTH,
  height: METER_HEIGHT,
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

// The local player's shield sits above health; a pawn without the layer reads
// absent and the bar shows empty.
const shieldBar = Bar({
  bind: shield.current,
  max: shield.capacity,
  width: METER_WIDTH,
  height: METER_HEIGHT,
  fill: [0.3, 0.6, 1, 1],
  background: [0.05, 0.07, 0.1, 0.9],
});

// Health stands alone in the lower-left corner: the shield meter above the
// health meter, each under its caption, so the panel shrink-wraps to them.
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
        VStack({ gap: 2.0, align: "start" }, [meterLabel("SHIELD"), shieldBar]),
        VStack({ gap: 2.0, align: "start" }, [meterLabel("HEALTH"), bar]),
      ],
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

// A cell weapon shows its charge as a bar in place of the ammo counts. Like
// the health bar, the bar's max binds to the weapon's own capacity slot.
const cellBar = Bar({
  bind: bindState(player.cell),
  max: player.cellCapacity,
  width: 160.0,
  height: 12.0,
  visibleWhen: stateEquals(player.weaponResource, "cell"),
  fill: color.ok,
  background: color.hud.health.background,
  styleRanges: {
    max: 1.0,
    entries: [{ upTo: 0.25, color: color.warning }, { color: color.ok }],
  },
});

// A heat weapon fills on shots and cools toward empty between them.
const heatBar = Bar({
  bind: bindState(player.heat),
  max: player.overheatAt,
  width: 160.0,
  height: 12.0,
  visibleWhen: stateEquals(player.weaponResource, "heat"),
  fill: color.warning,
  background: color.hud.health.background,
  styleRanges: {
    max: 1.0,
    entries: [{ upTo: 0.75, color: color.warning }, { color: color.critical }],
  },
});

const chargeMeter = VStack(
  {
    gap: spacing.hud.rowGap,
    align: "end",
    visibleWhen: stateEquals(player.weaponCharging, true),
  },
  [
    Text({
      content: "CHARGE",
      color: color.hud.text,
      font: font.hud.status,
      fontSize: 18.0,
    }),
    Bar({
      bind: bindState(player.weaponChargeProgress),
      max: 1.0,
      width: 160.0,
      height: 12.0,
      fill: color.warning,
      background: color.hud.health.background,
    }),
  ],
);

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
        HStack(
          {
            gap: spacing.hud.gap,
            align: "center",
            visibleWhen: stateEquals(player.weaponResource, "ammo"),
          },
          [ammo, ammoReserve],
        ),
        cellBar,
        heatBar,
        chargeMeter,
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
export const bleedOutMeterTree = defineUiTree({
  name: "hud.bleedOut",
  alwaysOn: true,
  tree: Tree(
    { anchor: "center", offset: [0, 76] },
    VStack(
      {
        gap: 6,
        align: "center",
        visibleWhen: stateEquals(player.lifeState, "down"),
      },
      [
        Text({
          content: "BLEED OUT",
          color: color.critical,
          font: font.hud.status,
          fontSize: 18,
        }),
        Bar({
          bind: bindState(player.lifeStateMs),
          max: BLEED_OUT_MS,
          width: 220,
          height: 12,
          fill: color.critical,
          background: color.hud.health.background,
        }),
      ],
    ),
  ),
});

export const revivingMeterTree = defineUiTree({
  name: "hud.reviving",
  alwaysOn: true,
  tree: Tree(
    { anchor: "center", offset: [0, 100] },
    VStack(
      {
        gap: 6,
        align: "center",
        visibleWhen: stateCompare(player.assistProgress, "greaterThan", 0),
      },
      [
        Text({
          content: "REVIVING",
          color: color.ok,
          font: font.hud.status,
          fontSize: 18,
        }),
        Bar({
          bind: bindState(player.assistProgress),
          max: 1.0,
          width: 220,
          height: 12,
          fill: color.ok,
          background: color.hud.health.background,
        }),
      ],
    ),
  ),
});

// Shown only while a helper's progress on this player is above 0; the bleed-out
// meter already covers the rest of the time spent down.
export const beingRevivedMeterTree = defineUiTree({
  name: "hud.beingRevived",
  alwaysOn: true,
  tree: Tree(
    { anchor: "center", offset: [0, 124] },
    VStack(
      {
        gap: 6,
        align: "center",
        visibleWhen: stateCompare(player.assistReceivedProgress, "greaterThan", 0),
      },
      [
        Text({
          content: "BEING REVIVED",
          color: color.ok,
          font: font.hud.status,
          fontSize: 18,
        }),
        Bar({
          bind: bindState(player.assistReceivedProgress),
          max: 1.0,
          width: 220,
          height: 12,
          fill: color.ok,
          background: color.hud.health.background,
        }),
      ],
    ),
  ),
});
