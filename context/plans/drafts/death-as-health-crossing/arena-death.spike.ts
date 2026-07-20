// DESIGN SPIKE — the CANONICAL consolidated pre-spec artifact (v2). Type-checks against
// the real postretro.d.ts + postretro/ui plus proposed.d.ts (the WALLs).
//
// THE MODEL (grounded true against shipped code): the engine owns IMPACT — the damage
// chokepoint, the single HP-decrement site. DEATH is not an engine concept; it is
// DERIVED. We DEFINE a named `killed` event ONCE from impact, then bind independent
// consumers to it — reuse is the whole reason death is derived, not engine-owned.
//
// v2 folds in three fixes the review caught against shipped source:
//   • KILL EDGE: `died()` (healthBefore>0 && healthAfter<=0), NOT `healthAfter<0` /
//     `crossedBelow(0)`. Health is FLOORED at 0 (health.rs:329), so a killed target sits
//     at exactly 0 — a strict `< 0` test can NEVER fire. (Passed tsc; failed reality.)
//   • OVERKILL: derived from `amount − healthBefore`, not the (unsatisfiable) `healthAfter`.
//   • COMPOSITION: `executed` shows a COMPOSED boolean via `.and(...)`, which desugars to
//     the shipped `select` opcode (the IR has no and/or/not).
//
// "postretro" / "postretro/ui" → SHIPPED.  "postretro/proposed" → a WALL.

import { defineStore } from "postretro";
import {
  entities,
  slot,
  type EventBehavior,
  type LevelManifestWithEvents,
} from "postretro/proposed";

const econ = defineStore("arena", {
  deaths: { type: "number", default: 0, network: "shared" },
});
const deaths = slot(econ.state.deaths);

const grunts = entities.query({ tag: "grunt" });

// === STAGE 1 — DEFINE the derived `killed` event ONCE. Enrich impact into a firing
// edge + a payload of IR facts. No effects here; this names what "kill" MEANS for the map.
const killed = grunts.defineEvent((impact) => ({
  when: impact.target.died(),                                       // blessed kill edge
  props: {
    target:   impact.target,                                       // pass-through token
    source:   impact.source,                                       // pass-through token
    xp:       impact.target.level.times(1.25).times(200),          // IR — not JS math
    overkill: impact.amount.minus(impact.target.healthBefore).gt(10), // real overkill magnitude
    executed: impact.target.died().and(impact.target.healthBefore.lt(25)), // COMPOSED bool (→ select)
  },
}));

// === STAGE 2a — the arena's own consumer: presentation + economy. Consumers run once at
// load and RETURN effect descriptors; the payload facts are already IR.
function arenaBehavior(): EventBehavior {
  return killed.on((kill) => [
    kill.target.playDeathAnim(),                                   // presentation (baked curve)
    kill.source.grant("xp", kill.xp),                              // consequential — credits SOURCE
    deaths.add(1),                                                 // consequential — retires ProgressTracker
    kill.target.despawn({ afterMs: 1500 }),                       // consequential — timer property
    { when: kill.overkill, do: [kill.source.grant("style", 1)] }, // inline gate
    { when: kill.executed, do: [kill.source.grant("style", 2)] },
  ]);
}

// === STAGE 2b — a SECOND, independent consumer of the SAME event. This is the payoff of
// two-stage: it reuses `killed` without re-deriving the edge, and knows nothing about the
// economy. Under one-stage this handler would have to re-write the (subtle) kill test.
function announcer(): EventBehavior {
  return killed.on((kill) => [
    { when: kill.executed, do: [kill.source.grant("announce_execution", 1)] },
  ]);
}

// === setupLevel returns the REAL LevelManifest; the derived behaviors are its `events`
// child. (This same block lives at ModManifest scope as the global reference behavior;
// a map overrides by adding its own.)
export function setupLevel(): LevelManifestWithEvents {
  return {
    reactions: [],
    events: [arenaBehavior(), announcer()],
  };
}

// === GUARDRAILS — these MUST NOT compile. ===================================
const guard = grunts.defineEvent((impact) => ({
  when: impact.target.died(),
  props: {
    target: impact.target,
    source: impact.source,
    xp: impact.target.level.times(200),
  },
}));
guard.on((kill) => {
  // @ts-expect-error live JS math on an IR ref — use .times(), not `*`.
  const badXp = 200 * kill.xp;
  void badXp;
  // @ts-expect-error relational operators don't work on IR refs — use .gt(), not `>`.
  const badCmp = kill.target.healthAfter > 0;
  void badCmp;
  // @ts-expect-error `source` has no despawn — you despawn the TARGET.
  kill.source.despawn();
  // @ts-expect-error `target` has no grant — you credit the SOURCE.
  kill.target.grant("xp", 5);
  // @ts-expect-error `overkill` was never enriched onto THIS event — the payload is exact.
  void kill.overkill;
  return [];
});

// @ts-expect-error a bare {kind:"impact"} is NOT an EventBehavior — the authored data
// (gated effects + IR) must survive the load→manifest seam; the brand can't be forged.
const badBehavior: EventBehavior = { kind: "impact" };
void badBehavior;
