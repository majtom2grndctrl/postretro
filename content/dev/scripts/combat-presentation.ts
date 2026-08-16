// DEV FIXTURE — passive combat feedback registered by the dev mod.
//
// The templates stay mod-global so any dev map using the dummy/enemy impact
// policies can exercise the presentation substrate without map-local setup.

import {
  Bar,
  Text,
  VStack,
  damagedEnemies,
  defineOverlay,
  definePresentationTemplate,
  fact,
} from "postretro/ui";

export const damageNumber = definePresentationTemplate("dev.damageNumber", {
  root: Text({
    content: "0",
    fontSize: 20.0,
    color: [1.0, 0.83, 0.24, 1.0],
    bind: fact.number("value", { format: "{}" }),
  }),
  lifetimeMs: 750,
  motion: { rise: 0.45, easing: "easeOut" },
  fade: { startMs: 425 },
  spawnScatter: { radius: 0.12 },
});

export const damagedEnemyBar = definePresentationTemplate("dev.damagedEnemyBar", {
  root: VStack(
    { gap: 2.0 },
    [
      Bar({
        bind: fact.number("healthFraction", {
          tween: { durationMs: 160, easing: "easeOut" },
        }),
        max: 1.0,
        width: 96.0,
        height: 8.0,
        fill: [0.12, 0.78, 0.38, 1.0],
        background: [0.05, 0.07, 0.1, 0.9],
        styleRanges: {
          max: 1.0,
          entries: [
            { upTo: 0.25, color: [0.92, 0.12, 0.16, 1.0] },
            { upTo: 0.5, color: [0.96, 0.64, 0.1, 1.0] },
            { color: [0.12, 0.78, 0.38, 1.0] },
          ],
        },
      }),
    ],
  ),
  lifetimeMs: 1,
  motion: { rise: 0.0, easing: "linear" },
  fade: { startMs: 1 },
  spawnScatter: { radius: 0.0 },
  worldAnchor: { socket: "head", offsetY: 0.35 },
});

export const damagedEnemyOverlay = defineOverlay({
  over: damagedEnemies({ lingerMs: 2_500, hideAtFull: true }),
  template: damagedEnemyBar,
  maxVisible: 8,
});
