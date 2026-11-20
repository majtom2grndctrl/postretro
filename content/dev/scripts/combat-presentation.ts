// DEV FIXTURE — passive combat feedback registered by the dev mod.
//
// The templates stay mod-global so any dev map using the dummy/enemy impact
// policies can exercise the presentation substrate without map-local setup.

import { runtime, type DamageType, type Impact, type Status } from "postretro";
import {
  Bar,
  Text,
  VStack,
  damagedEnemies,
  defineOverlay,
  definePresentationTemplate,
  fact,
  present,
} from "postretro/ui";
import {
  acid,
  ballistic,
  corrosive,
  explosive,
  fireDamage,
  flechette,
  plasma,
  shock,
} from "./damage-types";
import { armor, shield } from "./defenses";
import { burning, corroding, scalding } from "./statuses";

type Rgba = [number, number, number, number];

// One floating number per hit, colored by the hit's damage type; a status
// tick floats a smaller number in its status's color.
const damageNumberProps = (color: Rgba, fontSize = 20.0) => ({
  root: Text({
    content: "0",
    fontSize,
    color,
    bind: fact.number("value", { format: "{}", decimalPlaces: 0 }),
  }),
  lifetimeMs: 750,
  // Device pixels. Each pellet spawns its own number at its hit point; the
  // scatter separates pellets that land a few centimetres apart.
  motion: { rise: 40, easing: "easeOut" as const },
  fade: { startMs: 425 },
  spawnScatter: { radius: 8 },
});

export const ballisticDamageNumber = definePresentationTemplate(damageNumberProps([1.0, 1.0, 1.0, 1.0]));
export const shockDamageNumber = definePresentationTemplate(damageNumberProps([0.25, 0.95, 1.0, 1.0]));
export const explosiveDamageNumber = definePresentationTemplate(damageNumberProps([1.0, 0.86, 0.2, 1.0]));
export const plasmaDamageNumber = definePresentationTemplate(damageNumberProps([0.86, 0.45, 1.0, 1.0]));
export const corrosiveDamageNumber = definePresentationTemplate(damageNumberProps([0.55, 1.0, 0.25, 1.0]));
export const flechetteDamageNumber = definePresentationTemplate(damageNumberProps([1.0, 0.55, 0.2, 1.0]));
export const fireDamageNumber = definePresentationTemplate(damageNumberProps([1.0, 0.42, 0.1, 1.0]));
export const acidDamageNumber = definePresentationTemplate(damageNumberProps([0.8, 1.0, 0.1, 1.0]));

// Status ticks: no contact point, so each floats from the target's position.
export const burningDamageNumber = definePresentationTemplate(damageNumberProps([1.0, 0.6, 0.15, 1.0], 16.0));
export const scaldingDamageNumber = definePresentationTemplate(damageNumberProps([1.0, 0.2, 0.08, 1.0], 16.0));
export const corrodingDamageNumber = definePresentationTemplate(damageNumberProps([0.8, 1.0, 0.1, 1.0], 16.0));

export const damageNumberTemplates = [
  ballisticDamageNumber,
  shockDamageNumber,
  explosiveDamageNumber,
  plasmaDamageNumber,
  corrosiveDamageNumber,
  flechetteDamageNumber,
  fireDamageNumber,
  acidDamageNumber,
  burningDamageNumber,
  scaldingDamageNumber,
  corrodingDamageNumber,
];

const damageNumberByType: [DamageType, (typeof damageNumberTemplates)[number]][] = [
  [ballistic, ballisticDamageNumber],
  [shock, shockDamageNumber],
  [explosive, explosiveDamageNumber],
  [plasma, plasmaDamageNumber],
  [corrosive, corrosiveDamageNumber],
  [flechette, flechetteDamageNumber],
  [fireDamage, fireDamageNumber],
  [acid, acidDamageNumber],
];

const damageNumberByStatus: [Status, (typeof damageNumberTemplates)[number]][] = [
  [burning, burningDamageNumber],
  [scalding, scaldingDamageNumber],
  [corroding, corrodingDamageNumber],
];

// Impact-policy rows that float the hit's points dealt: a status tick in its
// status's color, any other hit in its type's color, never both. Every dev hit
// is typed: an untyped hit takes the mod's default, ballistic
// (`defaultDamageType` in start-script.ts). A marker status (`wet`) never
// dispatches an impact, so floats nothing.
export const damageNumberRows = (impact: Impact) => {
  const statusTick = damageNumberByStatus
    .map(([status]) => impact.status(status))
    .reduce((any, tick) => any.or(tick));
  return [
    ...damageNumberByStatus.map(([status, template]) => ({
      when: impact.status(status),
      do: [present(template, impact.amount)],
    })),
    ...damageNumberByType.map(([type, template]) => ({
      when: impact.type(type).and(statusTick.not()),
      do: [present(template, impact.amount)],
    })),
  ];
};

export const damagedEnemyBar = definePresentationTemplate({
  root: VStack(
    { gap: 2.0 },
    [
      // The outer pool: shield or armor, whichever the enemy carries. Hidden on
      // an enemy with neither.
      Bar({
        bind: fact.number("shieldFraction", {
          tween: { durationMs: 160, easing: "easeOut" },
        }),
        max: 1.0,
        width: 96.0,
        height: 5.0,
        fill: [0.35, 0.68, 1.0, 1.0],
        background: [0.05, 0.07, 0.1, 0.9],
        visibleWhen: fact.bool("hasShield"),
      }),
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
  // The shield pair reads each enemy's outer pool: shield plus armor. Dev
  // enemies carry one or the other, and an entity without a layer reads 0 / 0,
  // so the sum is whichever it has. Hide-at-full needs health and this pair
  // both full.
  over: damagedEnemies({
    lingerMs: 2_500,
    hideAtFull: true,
    shield: {
      value: (entity) =>
        runtime.add(entity.defense(shield).current, entity.defense(armor).current),
      max: (entity) =>
        runtime.add(entity.defense(shield).capacity, entity.defense(armor).capacity),
    },
  }),
  template: damagedEnemyBar,
  maxVisible: 8,
});
