// DESIGN SPIKE — the CANONICAL consolidated pre-spec artifact (v3). Type-checks against
// the real postretro.d.ts + postretro/ui plus proposed.d.ts (the WALLs).
//
// THE MODEL (grounded): the engine owns IMPACT — the single HP-decrement site. DEATH is a
// POLICY the modder writes over impact facts; the engine has no opinion about it. One
// uniform syntax — `entities.query(...).onImpact(...)` — covers the global reference
// behavior AND per-arena overrides. Re-query a narrower set, define its onImpact LATER, and
// it OVERRIDES for the entities both queries match. No named event, no consumer registry.
//
// The three cases below exist to prove the thesis:
//   1. GRUNT     — the author defines death as "health depleted → gone". health<=0 → despawn.
//   2. ARENA     — same syntax, narrower re-query, defined LATER → wins for arena_1 grunts.
//   3. ZOMBIE    — health<=0 is NOT death (Quake). Only a gib-level overshoot kills; otherwise
//                  the zombie DOWNS and stands back up. Same health<=0, opposite meaning.
//
// Why there is no `died()`: a blessed engine "dead" predicate would smuggle death back into
// the engine. The author writes the condition in IR (`healthAfter.le(0)`, `.le(-40)`) so
// death stays theirs. The IR is the calculator, not the death authority.
//
// "postretro" / "postretro/ui" → SHIPPED.  "postretro/proposed" → a WALL.

import { defineStore } from "postretro";
import {
  entities,
  slot,
  type EffectOrGroup,
  type EventBehavior,
  type GatedEffect,
  type ImpactEvent,
  type LevelManifestWithEvents,
} from "postretro/proposed";

const econ = defineStore("arena", {
  deaths: { type: "number", default: 0, network: "shared" },
});
const deaths = slot(econ.state.deaths);

// THE BASELINE POLICY — an ordinary function of impact, not a hoisted event. This is the
// reusable unit: the author decides health-depleted means dead, and the author removes the
// entity (the engine does not auto-despawn at 0 HP). Reuse it anywhere by CALLING it.
function baseGruntDeath(impact: ImpactEvent): readonly EffectOrGroup[] {
  return [
    {
      when: impact.target.healthAfter.le(0), // author's death policy, expressed in IR
      do: [
        impact.target.playAnim("death"),
        impact.source.grant("xp", impact.target.level.times(1.25).times(200)),
        deaths.add(1),
        impact.target.despawn({ afterMs: 1500 }),
      ],
    },
  ];
}

// 1. GRUNT — the global reference behavior: pass the baseline straight through.
function gruntImpact(): EventBehavior {
  return entities.query({ tag: "grunt" }).onImpact(baseGruntDeath);
}

// 2. ARENA OVERRIDE — SAME syntax, narrower query, defined LATER so it wins for arena_1
// grunts. It MUTATES the baseline by reusing it (`...baseGruntDeath(impact)`) instead of
// rewriting it: everything the baseline does, plus a style payout. Because the body is a
// plain array, an override could just as well omit or replace a baseline effect.
function arenaGruntImpact(): EventBehavior {
  return entities.query({ tag: "grunt", zone: "arena_1" }).onImpact((impact) => [
    ...baseGruntDeath(impact),
    { when: impact.target.healthAfter.le(0), do: [impact.source.grant("style", 5)] },
  ]);
}

// 3. ZOMBIE — the thesis in one function. health<=0 does NOT mean dead. Only a gib-level
// overshoot (healthAfter well below 0) truly kills; a mere depletion FLOPS the zombie and it
// stands back up. `lethal`/`downed` are composed booleans (→ shipped `select`).
function zombieImpact(): EventBehavior {
  return entities.query({ tag: "zombie" }).onImpact((impact) => {
    const lethal = impact.target.healthAfter.le(-40); // big overshoot → truly dead (gib)
    const downed = impact.target.healthAfter.le(0).and(lethal.not()); // depleted, not gibbed → flop
    return [
      {
        when: lethal,
        do: [
          impact.target.playAnim("gib"),
          impact.source.grant("xp", 100),
          impact.target.despawn(),
        ],
      },
      {
        when: downed,
        do: [
          impact.target.playAnim("down"),
          impact.target.setHealth(impact.target.level.times(20), { afterMs: 3000 }), // resurrect
        ],
      },
    ];
  });
}

// setupLevel returns the REAL LevelManifest; behaviors are its `events` child, in PRECEDENCE
// ORDER — arenaGruntImpact comes after gruntImpact, so it wins for arena_1 grunts.
export function setupLevel(): LevelManifestWithEvents {
  return {
    reactions: [],
    events: [gruntImpact(), arenaGruntImpact(), zombieImpact()],
  };
}

// === GUARDRAILS — these MUST NOT compile. ===================================
entities.query({ tag: "grunt" }).onImpact((impact) => {
  // @ts-expect-error live JS math on an IR ref — use .times(), not `*`.
  const badXp = 200 * impact.target.level;
  void badXp;
  // @ts-expect-error relational operators don't work on IR refs — use .le(), not `<=`.
  const badCmp = impact.target.healthAfter <= 0;
  void badCmp;
  // @ts-expect-error `source` has no despawn — you despawn the TARGET.
  impact.source.despawn();
  // @ts-expect-error `target` has no grant — you credit the SOURCE.
  impact.target.grant("xp", 5);
  // @ts-expect-error a NumberRef is not a BoolRef gate.
  const badGate: GatedEffect = { when: impact.target.healthAfter, do: [] };
  void badGate;
  return [];
});

// @ts-expect-error a bare {kind:"impact"} is NOT an EventBehavior — the authored effects/IR
// must survive the load→manifest seam; the brand can't be forged.
const badBehavior: EventBehavior = { kind: "impact" };
void badBehavior;
