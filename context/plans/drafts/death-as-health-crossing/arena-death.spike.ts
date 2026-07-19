// DESIGN SPIKE — the CANONICAL pre-spec artifact. Type-checks against the real
// postretro.d.ts + postretro/ui plus proposed.d.ts (the WALLs).
//
// THE MODEL (grounded true against shipped code): the engine owns IMPACT — the
// damage chokepoint, the single HP-decrement site through which every damage
// application flows. DEATH is not an engine concept; it is DERIVED from impact.
// So we don't listen to a pre-baked `onDeath` — we DEFINE death (and overkill, and
// xp) by deriving them from the one impact event.
//
// Two rules the guardrails at the bottom enforce:
//   1. Derived facts are IR EXPRESSIONS, not live JS (no VM at fire time):
//      `impact.target.level.times(1.25)`, never `200 * impact.target.level`.
//   2. Subject (damaged) vs source (damager) are distinct: you despawn the
//      subject, you credit the source.
//
// "postretro" / "postretro/ui" → SHIPPED.  "postretro/proposed" → a WALL.

import { defineStore } from "postretro";
import { entities, slot, type GatedEffect, type LevelManifestWithEvents } from "postretro/proposed";

const econ = defineStore("arena", {
  deaths: { type: "number", default: 0, network: "shared" },
});
const deaths = slot(econ.state.deaths);

// === setupLevel returns a LevelManifest; the derived behaviors are its `events`
// child. (This same block lives at ModManifest scope as the global reference
// behavior; a map overrides by adding its own.)
export function setupLevel(): LevelManifestWithEvents {
  const grunts = entities.query({ tag: "grunt" });

  return {
    reactions: [],
    events: [
      // DEFINE death by DERIVING it from impact. Facts are IR expressions the
      // engine evaluates at impact time; effects are gated on those facts.
      grunts.onImpact((impact) => {
        const isKill     = impact.target.crossedBelow(0);              // BoolRef edge — once per death
        const isOverkill = impact.target.healthAfter.lt(-10);          // BoolRef
        const xpReward   = impact.target.level.times(1.25).times(200); // NumberRef — IR, not JS math

        return [
          { when: isKill, do: [
            impact.target.playDeathAnim(),                // presentation
            impact.source.grant("xp", xpReward),          // consequential — credits the SOURCE
            deaths.add(1),                                // consequential — retires ProgressTracker
            impact.target.despawn({ afterMs: 1500 }),     // consequential — timer property
          ] },
          { when: isOverkill, do: [ impact.source.grant("style", 1) ] },
        ];
      }),
    ],
  };
}

// === GUARDRAILS — these MUST NOT compile. ===================================
const grunts = entities.query({ tag: "grunt" });
grunts.onImpact((impact) => {
  // @ts-expect-error live JS math on an IR ref — use .times(), not `*`.
  const badXp = 200 * impact.target.level;
  void badXp;
  // @ts-expect-error a NumberRef is not a BoolRef gate.
  const badGate: GatedEffect = { when: impact.target.healthAfter, do: [] };
  void badGate;
  // @ts-expect-error `source` has no despawn — you despawn the SUBJECT.
  impact.source.despawn();
  // @ts-expect-error `target` has no grant — you credit the SOURCE.
  impact.target.grant("xp", 5);
  return [];
});
